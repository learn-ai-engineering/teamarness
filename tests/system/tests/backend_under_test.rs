//! The Backend these system tests drive must be built from current sources.
//!
//! This is the regression guard for CR-012: the tests once read whatever
//! `npm run build` had staged under `target/teamarness-packages`, which cargo
//! has no staleness edge to, so a Backend that panicked on startup still passed
//! them. Resolving the executable through `CARGO_BIN_EXE_*` makes that
//! impossible; this test fails loudly if anything reintroduces a prebuilt path.

#![cfg(unix)]

#[allow(dead_code)]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use support::harness::{backend_executable, workspace_root};

/// The sources that must be compiled into the Backend under test: the Package
/// itself and the two crates it is allowed to depend on (ARC-10).
const BACKEND_SOURCES: [&str; 3] = [
    "packages/conversations-package/backend/src",
    "crates/teamarness-package-sdk/src",
    "crates/teamarness-package-protocol/src",
];

#[test]
fn the_backend_under_test_is_built_from_current_sources() {
    let executable = backend_executable();
    let built = modified(&executable);

    assert!(
        !executable.starts_with(workspace_root().join("target/teamarness-packages")),
        "the Backend under test is a prebuilt staged artifact ({}); \
         cargo cannot rebuild it for these tests, so a Backend regression would pass them (CR-012)",
        executable.display(),
    );

    for source in BACKEND_SOURCES {
        let directory = workspace_root().join(source);
        for file in rust_sources(&directory) {
            assert!(
                modified(&file) <= built,
                "{} is newer than the Backend under test ({}); \
                 the tests are running a stale Backend (CR-012)",
                file.display(),
                executable.display(),
            );
        }
    }
}

fn rust_sources(directory: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    for entry in fs::read_dir(directory).expect("a Backend source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    sources
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("cannot stat {}: {error}", path.display()))
        .modified()
        .expect("a modification time")
}
