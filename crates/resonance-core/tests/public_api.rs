use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{CoreRuntime, CoreRuntimeConfig, PublicIdentity};
use serde::{Deserialize, Serialize};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn temporary_directory() -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-public-api-{}-{sequence}",
        std::process::id()
    ))
}

fn compile_fails(fixture: &Path, expected_marker: &str, shared_target: &Path) {
    let fixture_name = fixture.file_stem().unwrap().to_str().unwrap();
    let crate_directory = temporary_directory();
    fs::create_dir_all(crate_directory.join("src")).unwrap();
    fs::copy(fixture, crate_directory.join("src/main.rs")).unwrap();
    let core_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::write(
        crate_directory.join("Cargo.toml"),
        format!(
            "[package]\nname = \"resonance-core-ui-{fixture_name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nresonance-core = {{ path = {:?} }}\n",
            core_directory
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--offline",
            "--manifest-path",
            crate_directory
                .join("Cargo.toml")
                .to_str()
                .expect("temporary path is UTF-8"),
            "--target-dir",
            shared_target.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{} unexpectedly compiled",
        fixture.display()
    );
    assert!(
        stderr.contains(expected_marker),
        "{} failed without the expected public-boundary diagnostic `{expected_marker}`:\n{stderr}",
        fixture.display()
    );

    fs::remove_dir_all(crate_directory).unwrap();
}

#[test]
fn public_identity_has_a_stable_validated_string_serde_contract() {
    fn assert_serde_traits<T: Serialize + for<'de> Deserialize<'de>>() {}

    assert_serde_traits::<PublicIdentity>();

    let directory = temporary_directory();
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
    let identity = runtime.public_identity();
    let display = identity.to_string();

    let serialized = serde_json::to_string(&identity).unwrap();
    assert_eq!(serialized, format!("\"{display}\""));
    assert_eq!(
        serde_json::from_str::<PublicIdentity>(&serialized).unwrap(),
        identity
    );
    assert!(serde_json::from_str::<PublicIdentity>(r#""not-a-public-identity""#).is_err());

    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn external_consumers_cannot_cross_the_runtime_boundary() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui");
    let target = temporary_directory();
    let cases = [
        ("secret_key.rs", "SecretKey"),
        ("raw_secret_bytes.rs", "secret_key"),
        ("serialized_secret.rs", "serialize_secret_key"),
        ("raw_identity_bytes.rs", "as_bytes"),
        ("sqlite_connection.rs", "connection"),
        ("private_adapter.rs", "membership"),
        ("mutable_group_crdt.rs", "group_crdt_mut"),
        ("endpoint_ticket.rs", "endpoint_ticket"),
    ];

    for (fixture, marker) in cases {
        compile_fails(&fixtures.join(fixture), marker, &target);
    }

    fs::remove_dir_all(target).unwrap();
}
