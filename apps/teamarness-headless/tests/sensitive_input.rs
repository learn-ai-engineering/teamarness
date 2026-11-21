use std::os::unix::fs::{PermissionsExt, symlink};
use teamarness_headless::input;

#[test]
fn private_regular_inputs_are_bounded_and_never_follow_symlinks_or_change_modes() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("input");
    std::fs::write(&file, b"CANARY_PRIVATE_INPUT").unwrap();
    for mode in [0o644, 0o640, 0o604, 0o666] {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(input::read_private(&file, 100).is_err());
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        input::read_private(&file, 20).unwrap(),
        b"CANARY_PRIVATE_INPUT"
    );
    assert!(input::read_private(&file, 19).is_err());
    let link = root.path().join("symlink");
    symlink(&file, &link).unwrap();
    assert!(input::read_private(&link, 100).is_err());
    assert!(input::read_private(root.path(), 100).is_err());
    assert!(input::read_private(&root.path().join("missing"), 100).is_err());
    let fifo = root.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        input::read_private(&fifo, 100).is_err(),
        "no blocked open on a FIFO"
    );
    assert_eq!(input::read_bounded(&b"1234"[..], 4).unwrap(), b"1234");
    assert!(input::read_bounded(&b"12345"[..], 4).is_err());
    std::fs::write(&file, vec![b'x'; input::MAX_INPUT_BYTES + 1]).unwrap();
    assert!(input::read_private(&file, input::MAX_INPUT_BYTES).is_err());
}

#[test]
fn json_requires_exactly_one_value_and_parser_diagnostics_do_not_echo_input() {
    for bytes in [
        b"{} {}".as_slice(),
        b"{} trailing CANARY",
        b"{\"secret\":\"CANARY\"",
        b"\xffCANARY",
    ] {
        let error = input::json::<serde_json::Value>(bytes).unwrap_err();
        assert!(!error.to_string().contains("CANARY"));
    }
    assert_eq!(
        input::json::<serde_json::Value>(b"  {\"value\":1} \n").unwrap()["value"],
        1
    );
}

#[test]
fn invalid_cli_secret_inputs_leave_no_state_and_no_canary_in_diagnostics() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, b"SECRET_CANARY_FOR_INVITATION").unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o600)).unwrap();
    let data = root.path().join("not-created");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .arg("--data-dir")
        .arg(&data)
        .args([
            "workspace",
            "join",
            "--person-name",
            "Ada",
            "--invitation-file",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_CANARY_FOR_INVITATION"));
    assert!(!data.exists());
}
