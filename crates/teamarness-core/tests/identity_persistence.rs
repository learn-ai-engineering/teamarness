use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use teamarness_core::{
    CoreError, CoreRuntime, CoreRuntimeConfig, IdentityError, SigningContext, StorageError,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

#[tokio::test]
async fn first_open_creates_one_identity_and_reopen_loads_it() {
    let directory = private_data_directory("identity-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let identity = runtime.public_identity();
    assert_eq!(runtime.display_name(), "This device");
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.public_identity(), identity);
    assert_eq!(reopened.display_name(), "This device");
    reopened.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn concurrent_first_open_still_creates_one_identity() {
    let directory = private_data_directory("identity-concurrent");
    let config = CoreRuntimeConfig::new(&directory);
    let (first, second) = tokio::join!(
        CoreRuntime::open_loopback_for_tests(config.clone()),
        CoreRuntime::open_loopback_for_tests(config)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.public_identity(), second.public_identity());
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn authenticated_reopen_rejects_a_missing_local_identity_subject() {
    let directory = private_data_directory("identity-authenticated-reopen");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute("DELETE FROM identity_subjects", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(config).await,
        Err(CoreError::Storage(StorageError::CorruptIdentity))
    ));

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn display_name_updates_are_validated_and_durable() {
    let directory = private_data_directory("display-name");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();

    for invalid in [
        "",
        " leading",
        "trailing ",
        "line\nbreak",
        "name/with/slash",
        &"x".repeat(65),
    ] {
        assert_eq!(
            runtime.set_display_name(invalid).unwrap_err(),
            IdentityError::InvalidDisplayName
        );
    }

    runtime.set_display_name("Zoë's laptop").unwrap();
    assert_eq!(runtime.display_name(), "Zoë's laptop");
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.display_name(), "Zoë's laptop");
    reopened.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn signatures_are_publicly_verifiable_and_separated_by_context() {
    let directory = private_data_directory("signing");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let membership = SigningContext::new("teamarness.membership-operation/v1").unwrap();
    let invitation = SigningContext::new("teamarness.invitation/v1").unwrap();
    let payload = b"same payload";

    let signature = runtime.sign(&membership, payload).unwrap();

    assert!(
        runtime
            .public_identity()
            .verify(&membership, payload, &signature)
            .is_ok()
    );
    assert_eq!(
        runtime
            .public_identity()
            .verify(&invitation, payload, &signature)
            .unwrap_err(),
        IdentityError::InvalidSignature
    );
    assert_eq!(
        runtime
            .public_identity()
            .verify(&membership, b"changed", &signature)
            .unwrap_err(),
        IdentityError::InvalidSignature
    );
    runtime.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn identity_database_and_directory_have_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("permissions");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let directory_mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    let database_mode = fs::metadata(directory.join("core.sqlite3"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(directory_mode, 0o700);
    assert_eq!(database_mode, 0o600);

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
// Verifies: TEAM.SYS.ID-R07
async fn open_refuses_symlinked_data_directory() {
    let directory = private_data_directory("symlink-target");
    let link = private_data_directory("symlink-link");
    fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(&directory, &link).unwrap();

    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&link)).await,
        Err(CoreError::Storage(StorageError::InvalidDataDirectory))
    ));

    fs::remove_file(link).unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
// Verifies: TEAM.SYS.ID-R07
async fn open_refuses_non_empty_directory_without_marker() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("unrelated-contents");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(directory.join("unrelated.txt"), b"not ours").unwrap();

    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory)).await,
        Err(CoreError::Storage(StorageError::InvalidDataDirectory))
    ));

    // A refused directory is never restricted, and gains no marker.
    let mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert!(!directory.join(".teamarness-private-directory").exists());
    assert!(!directory.join("core.sqlite3").exists());

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
// Verifies: TEAM.SYS.ID-R07
async fn open_writes_marker_and_adopts_legacy_directory() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("marker");
    let marker = directory.join(".teamarness-private-directory");
    let config = CoreRuntimeConfig::new(&directory);

    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let identity = runtime.public_identity();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    assert!(marker.is_file());
    assert_eq!(
        fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // A directory this branch created, whose marker was lost, is adopted on
    // the strength of its core database and marked again.
    fs::remove_file(&marker).unwrap();
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.public_identity(), identity);
    reopened.shutdown().await.unwrap();
    assert!(marker.is_file());

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
// Verifies: TEAM.SYS.ID-R07
async fn sqlite_journal_inherits_private_database_mode() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("journal-permissions");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let database = directory.join("core.sqlite3");
    let journal = directory.join("core.sqlite3-journal");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "journal_mode", "PERSIST")
        .unwrap();
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE installation_identity SET display_name = 'Journal' WHERE singleton_id = 1;
             COMMIT;",
        )
        .unwrap();
    assert!(
        journal.is_file(),
        "PERSIST leaves the journal file in place"
    );
    assert_eq!(
        fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
        0o600,
        "the rollback journal inherits the database file's mode"
    );
    drop(connection);

    fs::remove_dir_all(directory).unwrap();
}
