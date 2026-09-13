use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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

#[tokio::test]
async fn first_open_creates_one_identity_and_reopen_loads_it() {
    let directory = private_data_directory("identity-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let identity = runtime.public_identity();
    assert_eq!(runtime.display_name(), "This device");
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert_eq!(reopened.public_identity(), identity);
    assert_eq!(reopened.display_name(), "This device");
    reopened.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn concurrent_first_open_still_creates_one_identity() {
    let directory = private_data_directory("identity-concurrent");
    let config = CoreRuntimeConfig::new(&directory);
    let (first, second) =
        tokio::join!(CoreRuntime::open(config.clone()), CoreRuntime::open(config));
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
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    connection
        .execute("DELETE FROM identity_subjects", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open(config).await,
        Err(CoreError::Storage(StorageError::CorruptIdentity))
    ));

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn display_name_updates_are_validated_and_durable() {
    let directory = private_data_directory("display-name");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();

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

    let reopened = CoreRuntime::open(config).await.unwrap();
    assert_eq!(reopened.display_name(), "Zoë's laptop");
    reopened.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn signatures_are_publicly_verifiable_and_separated_by_context() {
    let directory = private_data_directory("signing");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
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
    runtime.shutdown().await.unwrap();

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn identity_database_and_directory_have_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = private_data_directory("permissions");
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory))
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
