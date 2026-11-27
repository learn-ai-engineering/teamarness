use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

#[path = "../test-support/test_directory.rs"]
mod test_directory;
use test_directory::TestDirectory;

#[tokio::test]
async fn installation_directory_is_removed_after_shutdown_and_reopen() {
    let directory = TestDirectory::new("installation-cleanup");
    let parent = directory.parent().unwrap().to_path_buf();
    assert!(!directory.exists(), "the fixture path starts absent");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let identity = runtime.public_identity();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    assert!(directory.join("core.sqlite3").is_file());
    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(reopened.public_identity(), identity);
    reopened.shutdown().await.unwrap();
    drop(reopened);
    drop(directory);
    assert!(!parent.exists(), "the owner removes the entire fixture");
}

#[test]
fn panic_removes_nested_database_files_without_manual_cleanup() {
    let directory = TestDirectory::new("panic-cleanup");
    let parent = directory.parent().unwrap().to_path_buf();
    let result = catch_unwind(AssertUnwindSafe(move || {
        fs::create_dir_all(directory.join("nested")).unwrap();
        let connection = rusqlite::Connection::open(directory.join("nested/core.sqlite3")).unwrap();
        connection
            .execute_batch("CREATE TABLE test(value TEXT);")
            .unwrap();
        panic!("simulate a failed assertion before fixture cleanup");
    }));
    assert!(result.is_err());
    assert!(!parent.exists(), "unwinding must remove the fixture parent");
}

#[test]
fn deletion_recreation_and_regular_files_remain_owned() {
    let directory = TestDirectory::new("replaceable-path");
    let parent = directory.parent().unwrap().to_path_buf();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("data"), b"first").unwrap();
    fs::remove_dir_all(&directory).unwrap();
    fs::write(&directory, b"a regular file instead of a directory").unwrap();
    drop(directory);
    assert!(!parent.exists());
}

#[test]
fn equal_fixture_names_have_independent_owners() {
    let first = TestDirectory::new("same-name");
    let second = TestDirectory::new("same-name");
    assert_ne!(first.as_ref(), second.as_ref());
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    drop(first);
    assert!(second.exists());
}
