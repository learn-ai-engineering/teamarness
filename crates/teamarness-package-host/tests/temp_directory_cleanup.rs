#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::panic::catch_unwind;
use std::path::{Path, PathBuf};

#[path = "../../../tests/support/temp_dir.rs"]
mod tempfile;

fn readonly_cache(root: &Path) -> PathBuf {
    let cache = root.join("cache/ui");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("index.js"), b"export const value = 1;").unwrap();
    fs::set_permissions(cache.join("index.js"), fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(root.join("cache"), fs::Permissions::from_mode(0o500)).unwrap();
    cache
}

#[test]
fn drop_removes_readonly_cache_without_changing_permissions_during_the_test() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let cache = readonly_cache(&path);
    assert_eq!(
        fs::metadata(cache).unwrap().permissions().mode() & 0o777,
        0o500
    );
    drop(root);
    assert!(!path.exists());
}

#[test]
fn panic_removes_readonly_cache_without_manual_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    assert!(
        catch_unwind(move || {
            readonly_cache(root.path());
            panic!("simulate a failed fixture assertion");
        })
        .is_err()
    );
    assert!(!path.exists());
}

#[test]
fn cleanup_never_follows_symlinks_or_changes_external_targets() {
    let external = tempfile::tempdir().unwrap();
    let cache = readonly_cache(external.path());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    symlink(&cache, path.join("external-directory")).unwrap();
    symlink(cache.join("index.js"), path.join("external-file")).unwrap();
    symlink(path.join("missing"), path.join("dangling")).unwrap();
    root.close().unwrap();
    assert!(!path.exists());
    assert_eq!(
        fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        fs::read(cache.join("index.js")).unwrap(),
        b"export const value = 1;"
    );
    // Clean up the independently owned target only after checking it was untouched.
    external.close().unwrap();
}
