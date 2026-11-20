//! Cargo commands for source-built test fixtures, including dependencies built outside the repo.
use std::path::Path;
use std::process::{Command, Output};

pub fn command(repository: &Path) -> Command {
    let toolchain = std::fs::read_to_string(repository.join("rust-toolchain.toml")).unwrap();
    let channel = toolchain
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = \""))
        .and_then(|line| line.strip_suffix('"'))
        .expect("repository pins a Rust toolchain channel");
    // env!("CARGO") is the real Cargo binary, not the rustup proxy. Without an
    // explicit toolchain its rustc subprocess can select the default in a registry cwd.
    let mut command = Command::new("rustup");
    command
        .args(["run", channel, "cargo"])
        .current_dir(repository)
        .env("RUSTUP_TOOLCHAIN", channel)
        .env_remove("RUSTC");
    // ring tracks injected Cargo-test Package metadata as build inputs. Do not
    // recompile/replace live fixture executables because of the caller's metadata.
    for (key, _) in std::env::vars_os() {
        if key == "CARGO_MANIFEST_DIR" || key.to_string_lossy().starts_with("CARGO_PKG_") {
            command.env_remove(key);
        }
    }
    command
}

pub fn diagnostics(output: &Output) -> String {
    format!(
        "{}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
