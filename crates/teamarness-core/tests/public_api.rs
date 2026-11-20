use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, PublicIdentity};

#[path = "../../../tests/support/cargo_fixture.rs"]
mod cargo_fixture;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

// Cargo must prune unrelated workspace roots from a copied lock before --locked
// checking the isolated consumer. Only the consumer itself may be newly resolved.
fn locked_package_headers(lock: &str) -> Vec<&str> {
    lock.split("\n[[package]]\n")
        .skip(1)
        .map(|package| package.split("\ndependencies = [").next().unwrap().trim())
        .collect()
}

fn assert_locked_closure(repository_lock: &str, fixture_lock: &str, consumer: &str) {
    let locked = locked_package_headers(repository_lock);
    let consumer = format!("name = {consumer:?}\nversion = \"0.0.0\"");
    for package in locked_package_headers(fixture_lock) {
        assert!(
            package == consumer || locked.contains(&package),
            "isolated fixture selected a dependency outside the repository lock:\n{package}"
        );
    }
}

#[test]
fn isolated_lock_refuses_dependency_version_or_checksum_drift() {
    let locked = "version = 4\n\n[[package]]\nname = \"zerocopy-derive\"\nversion = \"0.8.57\"\nsource = \"registry+proof\"\nchecksum = \"locked\"\n";
    assert_locked_closure(locked, locked, "consumer");
    for drift in [
        locked.replace("0.8.57", "0.8.63"),
        locked.replace("locked", "changed"),
    ] {
        assert!(
            std::panic::catch_unwind(|| assert_locked_closure(locked, &drift, "consumer")).is_err()
        );
    }
}

fn temporary_directory() -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-public-api-{}-{sequence}",
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
            "[package]\nname = \"teamarness-core-ui-{fixture_name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nteamarness-core = {{ path = {:?} }}\n",
            core_directory
        ),
    )
    .unwrap();

    let repository = core_directory.join("../..");
    let repository_lock = fs::read_to_string(repository.join("Cargo.lock")).unwrap();
    let consumer = format!("teamarness-core-ui-{fixture_name}");
    fs::write(
        crate_directory.join("Cargo.lock"),
        format!("{repository_lock}\n[[package]]\nname = {consumer:?}\nversion = \"0.0.0\"\ndependencies = [\"teamarness-core\"]\n"),
    )
    .unwrap();
    let pruned = cargo_fixture::command(&repository)
        .args(["update", "--offline", "--workspace", "--manifest-path"])
        .arg(crate_directory.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        pruned.status.success(),
        "{}",
        cargo_fixture::diagnostics(&pruned)
    );
    let fixture_lock = fs::read_to_string(crate_directory.join("Cargo.lock")).unwrap();
    assert_locked_closure(&repository_lock, &fixture_lock, &consumer);

    let output = cargo_fixture::command(&repository)
        .args([
            "check",
            "--quiet",
            "--offline",
            "--locked",
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
    assert_eq!(
        fixture_lock,
        fs::read_to_string(crate_directory.join("Cargo.lock")).unwrap()
    );
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

    eprintln!("public boundary refusal executed: {fixture_name} ({expected_marker})");
    fs::remove_dir_all(crate_directory).unwrap();
}

#[tokio::test]
async fn public_identity_has_a_stable_validated_string_serde_contract() {
    fn assert_serde_traits<T: Serialize + for<'de> Deserialize<'de>>() {}

    assert_serde_traits::<PublicIdentity>();

    let directory = temporary_directory();
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let identity = runtime.public_identity();
    let display = identity.to_string();

    let serialized = serde_json::to_string(&identity).unwrap();
    assert_eq!(serialized, format!("\"{display}\""));
    assert_eq!(
        serde_json::from_str::<PublicIdentity>(&serialized).unwrap(),
        identity
    );
    assert!(serde_json::from_str::<PublicIdentity>(r#""not-a-public-identity""#).is_err());

    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

/// Verifies: TEAM.SYS.ID-R01, TEAM.SYS.TRANS-R03
#[tokio::test]
async fn external_consumers_cannot_cross_the_runtime_boundary() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui");
    let target = temporary_directory();
    let cases = [
        ("secret_key.rs", "SecretKey"),
        ("agent_secret_key.rs", "agent_secret_key"),
        ("raw_secret_bytes.rs", "secret_key"),
        ("serialized_secret.rs", "serialize_secret_key"),
        ("raw_identity_bytes.rs", "as_bytes"),
        ("sqlite_connection.rs", "connection"),
        ("private_adapter.rs", "membership"),
        ("mutable_group_crdt.rs", "group_crdt_mut"),
        ("endpoint_ticket.rs", "endpoint_ticket"),
        ("endpoint_handle.rs", "endpoint"),
        ("router_handle.rs", "router"),
        ("connection_handle.rs", "connection"),
        ("stream_handle.rs", "stream"),
        ("workspace_bootstrap.rs", "WorkspaceBootstrap"),
        ("canonical_membership_bytes.rs", "canonical_bytes"),
        ("invitation_token.rs", "InvitationToken"),
        ("invitation_claim.rs", "InvitationClaim"),
        ("issue_invitation.rs", "issue_invitation"),
        ("create_invitation_claim.rs", "create_invitation_claim"),
        ("redeem_invitation.rs", "redeem_invitation"),
        ("configuration_error.rs", "unresolved import"),
        ("replicate_records_to.rs", "replicate_records_to"),
        (
            "authenticate_membership_operation.rs",
            "authenticate_membership_operation",
        ),
    ];

    for (fixture, marker) in cases {
        compile_fails(&fixtures.join(fixture), marker, &target);
    }

    fs::remove_dir_all(target).unwrap();
}
