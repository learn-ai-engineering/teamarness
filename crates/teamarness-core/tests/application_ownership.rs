#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
#[path = "../test-support/test_directory.rs"]
mod test_directory;
use test_directory::TestDirectory;

use teamarness_core::{CoreError, CoreRuntime, CoreRuntimeConfig, StorageError};

fn directory() -> TestDirectory {
    TestDirectory::new("application-owner")
}
fn config(path: &std::path::Path) -> CoreRuntimeConfig {
    CoreRuntimeConfig::new(path).with_exclusive_application_ownership()
}

#[tokio::test]
async fn duplicate_owner_refuses_before_database_recovery_and_drop_reopens_same_identity() {
    let directory = directory();
    let first = CoreRuntime::open_loopback_for_tests(config(&directory))
        .await
        .unwrap();
    let identity = first.public_identity();
    // Corruption would be detected by recovery if the losing owner reached it.
    let connection = rusqlite::Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute_batch("BEGIN IMMEDIATE; DELETE FROM identity_subjects;")
        .unwrap();
    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(config(&directory)).await,
        Err(CoreError::Storage(StorageError::ApplicationAlreadyOwned))
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    first.shutdown().await.unwrap();
    drop(first);
    assert!(directory.join("application.lock").is_file());
    let reopened = CoreRuntime::open_loopback_for_tests(config(&directory))
        .await
        .unwrap();
    assert_eq!(identity, reopened.public_identity());
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn unsafe_lock_paths_are_refused_without_changing_their_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for kind in ["symlink", "hardlink", "directory", "public"] {
        let directory = directory();
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        runtime.shutdown().await.unwrap();
        drop(runtime);
        let target = directory.with_extension("unrelated");
        fs::write(&target, b"untouched").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let lock = directory.join("application.lock");
        match kind {
            "symlink" => symlink(&target, &lock).unwrap(),
            "hardlink" => fs::hard_link(&target, &lock).unwrap(),
            "directory" => fs::create_dir(&lock).unwrap(),
            _ => {
                fs::write(&lock, b"public").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        assert!(matches!(
            CoreRuntime::open_loopback_for_tests(config(&directory)).await,
            Err(CoreError::Storage(StorageError::InvalidApplicationLock))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::remove_dir_all(&directory).unwrap();
        fs::remove_file(target).unwrap();
    }
}

#[tokio::test]
async fn exec_child_cannot_retain_application_ownership() {
    let directory = directory();
    let runtime = CoreRuntime::open_loopback_for_tests(config(&directory))
        .await
        .unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = CoreRuntime::open_loopback_for_tests(config(&directory)).await;
    child.kill().unwrap();
    child.wait().unwrap();
    let reopened = reopened.expect("exec must not inherit the ownership descriptor");
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn retirement_requires_core_shutdown_and_permanently_fences_commands() {
    let directory = directory();
    let runtime = CoreRuntime::open_loopback_for_tests(config(&directory))
        .await
        .unwrap();
    assert!(runtime.retire_application_ownership().is_err());
    assert!(
        CoreRuntime::open_loopback_for_tests(config(&directory))
            .await
            .is_err()
    );
    runtime.shutdown().await.unwrap();
    // Successful Core shutdown alone retains ownership for Host/application cleanup.
    assert!(
        CoreRuntime::open_loopback_for_tests(config(&directory))
            .await
            .is_err()
    );
    runtime.retire_application_ownership().unwrap();
    assert!(runtime.set_display_name("must not write").is_err());
    let replacement = CoreRuntime::open_loopback_for_tests(config(&directory))
        .await
        .unwrap();
    assert_eq!(replacement.public_identity(), runtime.public_identity());
    replacement.shutdown().await.unwrap();
    drop(replacement);
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn application_death_releases_lock_while_exec_backend_survives() {
    // Re-enter this same public-interface example as the owning application.
    if let Some(path) = std::env::var_os("TEAMARNESS_OWNERSHIP_CHILD_DIRECTORY") {
        let path = PathBuf::from(path);
        let runtime = CoreRuntime::open_loopback_for_tests(config(&path))
            .await
            .unwrap();
        #[expect(
            clippy::zombie_processes,
            reason = "the test kills this application and explicitly kills its surviving Backend from the parent"
        )]
        let backend = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        fs::write(path.join("backend.pid"), backend.id().to_string()).unwrap();
        fs::write(path.join("ready"), runtime.public_identity().to_string()).unwrap();
        std::future::pending::<()>().await;
        unreachable!();
    }
    let directory = directory();
    let mut application = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "application_death_releases_lock_while_exec_backend_survives",
            "--nocapture",
        ])
        .env(
            "TEAMARNESS_OWNERSHIP_CHILD_DIRECTORY",
            directory.as_os_str(),
        )
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !directory.join("ready").exists() {
        if tokio::time::Instant::now() >= deadline {
            application.kill().unwrap();
            application.wait().unwrap();
            panic!("child application never reported readiness");
        }
        // poll-interval: bounded wait for the child application's readiness file.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let identity = fs::read_to_string(directory.join("ready")).unwrap();
    let pid: i32 = fs::read_to_string(directory.join("backend.pid"))
        .unwrap()
        .parse()
        .unwrap();
    application.kill().unwrap();
    application.wait().unwrap();
    let backend = nix::unistd::Pid::from_raw(pid);
    assert!(nix::sys::signal::kill(backend, None).is_ok());
    let reopened = CoreRuntime::open_loopback_for_tests(config(&directory)).await;
    nix::sys::signal::kill(backend, nix::sys::signal::Signal::SIGKILL).unwrap();
    let reopened = reopened.expect("surviving exec Backend must not retain application ownership");
    assert_eq!(reopened.public_identity().to_string(), identity);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn terminal_release_unlocks_even_a_descriptor_temporarily_duplicated_during_spawn() {
    use std::os::unix::fs::MetadataExt;
    for retire in [false, true] {
        let directory = directory();
        let runtime = CoreRuntime::open_loopback_for_tests(config(&directory))
            .await
            .unwrap();
        let identity = runtime.public_identity();
        let metadata = fs::metadata(directory.join("application.lock")).unwrap();
        // macOS /dev/fd opens duplicate the same open file description, as a
        // child can temporarily hold between fork and close-on-exec. Exercise
        // that kernel behavior without unsafe fork hooks or a startup sleep.
        let duplicate = fs::read_dir("/dev/fd")
            .unwrap()
            .find_map(|entry| {
                let file = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(entry.ok()?.path())
                    .ok()?;
                let observed = file.metadata().ok()?;
                (observed.dev() == metadata.dev() && observed.ino() == metadata.ino())
                    .then_some(file)
            })
            .expect("the application's ownership descriptor exists");
        runtime.shutdown().await.unwrap();
        if retire {
            runtime.retire_application_ownership().unwrap();
        }
        drop(runtime);
        let replacement = CoreRuntime::open_loopback_for_tests(config(&directory)).await;
        drop(duplicate);
        let replacement =
            replacement.expect("terminal release must unlock, not merely close one descriptor");
        assert_eq!(replacement.public_identity(), identity);
        assert!(
            CoreRuntime::open_loopback_for_tests(config(&directory))
                .await
                .is_err()
        );
        replacement.shutdown().await.unwrap();
        drop(replacement);
        fs::remove_dir_all(directory).unwrap();
    }
}
