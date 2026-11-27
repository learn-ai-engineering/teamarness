use std::path::Path;
use std::process::Command;

#[path = "../../../tests/support/cargo_fixture.rs"]
mod cargo_fixture;

#[test]
fn nested_cargo_pins_the_compiler_and_reports_json_stdout_despite_conflicting_ambient_toolchain() {
    const PROBE: &str = "TEAMARNESS_CARGO_FIXTURE_PROBE";
    if std::env::var_os(PROBE).is_none() {
        // Isolate ambient environment changes from the concurrent test process.
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "nested_cargo_pins_the_compiler_and_reports_json_stdout_despite_conflicting_ambient_toolchain", "--nocapture"])
            .env(PROBE, "1")
            .env("RUSTUP_TOOLCHAIN", "stable")
            .env("RUSTC", "rustc")
            .env("CARGO_MANIFEST_DIR", "/not/the/repository")
            .env("CARGO_PKG_NAME", "injected-caller-metadata")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            cargo_fixture::diagnostics(&output)
        );
        return;
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("cargo-fixture-probe");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"fixture-compiler-proof\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"fixture-compiler-proof\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("build.rs"),
        r#"
fn main() {
    let output = std::process::Command::new("rustc").arg("--version").output().unwrap();
    let version = String::from_utf8(output.stdout).unwrap();
    assert!(version.starts_with("rustc 1.99.1 "), "nested compiler: {version}");
    assert_eq!(std::env::var("CARGO_PKG_NAME").unwrap(), "fixture-compiler-proof");
}
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "compile_error!(\"fixture_expected_compiler_diagnostic\");\n",
    )
    .unwrap();
    let mut command = cargo_fixture::command(&repository);
    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "CARGO_PKG_NAME" && value.is_none())
    );
    let output = command
        .args([
            "check",
            "--offline",
            "--locked",
            "--message-format=json",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()
        .unwrap();
    let diagnostic = cargo_fixture::diagnostics(&output);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("fixture_expected_compiler_diagnostic"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("compiler-message"), "{diagnostic}");
    assert!(
        diagnostic.contains("fixture_expected_compiler_diagnostic"),
        "{diagnostic}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
