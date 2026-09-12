use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use resonance_core::{
    CoreError, CoreRuntime, CoreRuntimeConfig, IdentityError, SigningContext, StorageError,
};
use rusqlite::Connection;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn first_open_creates_one_identity_and_reopen_loads_it() {
    let directory = private_data_directory("identity-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    let runtime = CoreRuntime::open(config.clone()).unwrap();
    let identity = runtime.public_identity();
    assert_eq!(runtime.display_name(), "This device");
    drop(runtime);

    let reopened = CoreRuntime::open(config).unwrap();
    assert_eq!(reopened.public_identity(), identity);
    assert_eq!(reopened.display_name(), "This device");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn concurrent_first_open_still_creates_one_identity() {
    let directory = private_data_directory("identity-concurrent");
    let config = CoreRuntimeConfig::new(&directory);
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();

    for _ in 0..2 {
        let config = config.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            CoreRuntime::open(config).unwrap().public_identity()
        }));
    }
    barrier.wait();

    let first = threads.remove(0).join().unwrap();
    let second = threads.remove(0).join().unwrap();
    assert_eq!(first, second);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn authenticated_reopen_rejects_a_missing_local_identity_subject() {
    let directory = private_data_directory("identity-authenticated-reopen");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).unwrap();
    drop(runtime);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute("DELETE FROM identity_subjects", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config),
        Err(CoreError::Storage(StorageError::CorruptIdentity))
    ));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn display_name_updates_are_validated_and_durable() {
    let directory = private_data_directory("display-name");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).unwrap();

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
    drop(runtime);

    let reopened = CoreRuntime::open(config).unwrap();
    assert_eq!(reopened.display_name(), "Zoë's laptop");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn signatures_are_publicly_verifiable_and_separated_by_context() {
    let directory = private_data_directory("signing");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
    let membership = SigningContext::new("resonance.membership-operation/v1").unwrap();
    let invitation = SigningContext::new("resonance.invitation/v1").unwrap();
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

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn identity_database_and_directory_have_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("permissions");
    CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();

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
