use std::process::Command;

#[test]
fn offline_commands_and_invalid_arguments_never_create_state() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("must-not-exist");
    for arguments in [
        vec!["--help"],
        vec!["--version"],
        vec!["serve"],
        vec!["status"],
        vec!["nonsense"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
            .args(&arguments)
            .env("HOME", root.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if arguments[0].starts_with("--") { 0 } else { 2 })
        );
        assert!(!data.exists());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    let output = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .args(["--data-dir", data.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!data.exists(), "clients cannot initialize Installations");
}

#[test]
fn malformed_paths_and_serve_only_flags_fail_before_any_data_directory_side_effect() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let data = root.path().join("data");
    let path = data.to_str().unwrap();
    for args in [
        vec!["serve", "--data-dir", path, "--data-dir", path],
        vec!["--data-dir", path, "status", "--application-root", "/tmp"],
        vec![
            "serve",
            "--data-dir",
            path,
            "--application-root",
            "/does-not-exist",
        ],
        vec!["serve", "--data-dir", path, "status"],
        vec!["serve", "--data-dir", path, "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!data.exists());
    }
    let long = root.path().join("x".repeat(120));
    let output = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .args([
            std::ffi::OsStr::new("serve"),
            std::ffi::OsStr::new("--data-dir"),
            long.as_os_str(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!long.exists());
}

#[test]
fn unavailable_client_does_not_restrict_or_initialize_an_existing_unrelated_directory() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .args([
            std::ffi::OsStr::new("--data-dir"),
            root.path().as_os_str(),
            std::ffi::OsStr::new("status"),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
