#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

pub use teamarness_package_host::{
    InstallationPublicIdentity, PackageErrorCode, PackageErrorEnvelope, PackageIdentity,
    PackageSourceKind, WorkspaceId,
};
use tempfile::TempDir;

#[path = "../src/data_directory.rs"]
mod data_directory;
#[path = "../src/host_store.rs"]
mod host_store;
#[path = "../src/manifest.rs"]
mod manifest;

fn ids(
    package: &str,
    workspace: &str,
) -> (InstallationPublicIdentity, WorkspaceId, PackageIdentity) {
    (
        InstallationPublicIdentity::new("installation1").unwrap(),
        WorkspaceId::new(workspace).unwrap(),
        package.parse().unwrap(),
    )
}

#[test]
fn restart_source_and_version_changes_reuse_the_persisted_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let (installation, workspace, package) = ids("com.example.one", "workspace1");
    let first = {
        let store = host_store::HostStore::open(temp.path()).unwrap();
        data_directory::allocate(&store, temp.path(), &installation, &workspace, &package).unwrap()
    };
    let second = {
        let store = host_store::HostStore::open(temp.path()).unwrap();
        data_directory::allocate(&store, temp.path(), &installation, &workspace, &package).unwrap()
    };
    assert_eq!(first.path, second.path);
    assert_eq!(first.continuity_id, second.continuity_id);
    assert!(
        first
            .path
            .ends_with(manifest::sha256(package.as_str().as_bytes()))
    );
    assert!(temp.path().join("packages/package-host.sqlite3").is_file());
}

#[test]
fn workspaces_packages_and_installations_receive_isolated_directories() {
    let temp = tempfile::tempdir().unwrap();
    let store = host_store::HostStore::open(temp.path()).unwrap();
    let mut paths = std::collections::BTreeSet::new();
    for (installation, workspace, package) in [
        ids("com.example.one", "workspace1"),
        ids("com.example.one", "workspace2"),
        ids("com.example.two", "workspace1"),
        (
            InstallationPublicIdentity::new("installation2").unwrap(),
            WorkspaceId::new("workspace1").unwrap(),
            "com.example.one".parse().unwrap(),
        ),
    ] {
        paths.insert(
            data_directory::allocate(&store, temp.path(), &installation, &workspace, &package)
                .unwrap()
                .path,
        );
    }
    assert_eq!(paths.len(), 4);
}

#[test]
fn directories_are_owner_only_and_symlink_components_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let store = host_store::HostStore::open(temp.path()).unwrap();
    let (installation, workspace, package) = ids("com.example.one", "workspace1");
    let grant =
        data_directory::allocate(&store, temp.path(), &installation, &workspace, &package).unwrap();
    for ancestor in grant.path.ancestors().take(5) {
        if ancestor.starts_with(temp.path().join("packages")) {
            assert_eq!(
                fs::metadata(ancestor).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    let other = tempfile::tempdir().unwrap();
    let bad = tempfile::tempdir().unwrap();
    fs::create_dir_all(bad.path().join("packages/data")).unwrap();
    symlink(other.path(), bad.path().join("packages/data/v1")).unwrap();
    let bad_store = host_store::HostStore::open(bad.path()).unwrap();
    let error =
        data_directory::allocate(&bad_store, bad.path(), &installation, &workspace, &package)
            .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::DataDirectoryPermission);
}

#[test]
fn migration_has_versioned_continuity_and_process_lease_tables() {
    let sql = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_package_host.sql"),
    )
    .unwrap();
    for table in [
        "package_data_directories",
        "admissions",
        "diagnostics",
        "process_leases",
    ] {
        assert!(sql.contains(table), "missing {table}");
    }
    assert!(sql.contains("path_version"));
}

#[test]
fn identity_components_cannot_escape_the_data_root() {
    assert!(InstallationPublicIdentity::new("..").is_err());
    assert!(WorkspaceId::new(".").is_err());
}

#[test]
fn protocol_path_must_be_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let root = TempDir::new().unwrap();
    let non_utf8 = std::ffi::OsString::from_vec(vec![0xff]);
    let path = root.path().join(non_utf8);
    assert_eq!(
        data_directory::protocol_path(&path).unwrap_err().code,
        PackageErrorCode::DataDirectoryUnrepresentable
    );
}
