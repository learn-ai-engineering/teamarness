mod support;

use serde_json::{Value, json};
use support::packages::{self, PERSONAL};
use teamarness_headless::control::Operation;

fn operations(workspace: &str) -> Operation {
    Operation::PackageOperations {
        workspace: workspace.into(),
        package: PERSONAL.parse().unwrap(),
    }
}
fn schema(workspace: &str, operation: &str) -> Operation {
    Operation::PackageSchema {
        workspace: workspace.into(),
        package: PERSONAL.parse().unwrap(),
        operation: operation.into(),
    }
}
fn request(workspace: &str, digest: &str, operation: &str, payload: Value) -> Operation {
    Operation::PackageRequest {
        workspace: workspace.into(),
        package: PERSONAL.parse().unwrap(),
        operation: operation.into(),
        contract_digest: digest.into(),
        payload,
    }
}
async fn setup(fixture: &support::Fixture) -> (support::Running, String, std::path::PathBuf) {
    let (app, workspace, _) = support::inviter(fixture).await;
    let running = support::Running::start(app).await;
    let directory = packages::stage(&fixture.root.path().join("independent"), PERSONAL);
    running
        .call(Operation::PackagesInstall {
            directory: directory.clone(),
            expected_identity: PERSONAL.parse().unwrap(),
            accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
        })
        .await
        .unwrap();
    (running, workspace, directory)
}

// Verifies: TEAM.SYS.PKG-R21.
#[tokio::test]
async fn admitted_metadata_and_unknown_package_settings_use_existing_validated_requests() {
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let metadata = running.call(operations(&workspace)).await.unwrap();
    assert!(
        metadata["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op["name"] == "proof.settings.set" && op["kind"] == "request")
    );
    let digest = metadata["contractDigest"].as_str().unwrap();
    let schema = running
        .call(schema(&workspace, "proof.settings.set"))
        .await
        .unwrap();
    assert_eq!(schema["contractDigest"], digest);
    assert_eq!(schema["schema"]["$defs"]["settings"]["type"], "object");
    assert_eq!(
        running
            .call(request(
                &workspace,
                digest,
                "proof.settings.set",
                json!({"value":"saved"})
            ))
            .await
            .unwrap(),
        json!({"value":"saved"})
    );
    assert_eq!(
        running
            .call(request(&workspace, digest, "proof.settings.get", json!({})))
            .await
            .unwrap(),
        json!({"value":"saved"})
    );
    // Resolving this UI resource would fail its cache permission check. Requests never read it.
    use std::os::unix::fs::PermissionsExt;
    let cache = std::fs::read_dir(fixture.config.data_directory.join("packages/ui-cache/v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("ui/index.js");
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        running
            .call(request(&workspace, digest, "proof.settings.get", json!({})))
            .await
            .unwrap(),
        json!({"value":"saved"})
    );
    std::fs::set_permissions(cache, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        running
            .app
            .status()
            .await
            .host
            .instances
            .iter()
            .all(|instance| instance.mount_state.is_none()),
        "every non-rendered lease closed"
    );
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted
            .call(request(&workspace, digest, "proof.settings.get", json!({})))
            .await
            .unwrap(),
        json!({"value":"saved"})
    );
    restarted.stop().await;
}

fn envelope(
    error: teamarness_headless::ControlError,
) -> teamarness_package_host::PackageErrorEnvelope {
    match error.details.unwrap() {
        teamarness_headless::ErrorDetails::Host { envelope } => *envelope,
        other => panic!("expected original Host envelope: {other:?}"),
    }
}
async fn digest(running: &support::Running, workspace: &str) -> String {
    running.call(operations(workspace)).await.unwrap()["contractDigest"]
        .as_str()
        .unwrap()
        .into()
}
async fn status(running: &support::Running, workspace: &str, digest: &str) -> Value {
    running
        .call(request(workspace, digest, "proof.status", json!({})))
        .await
        .unwrap()
}

#[tokio::test]
async fn exact_payload_bound_and_pre_dispatch_refusals_leave_package_calls_unchanged() {
    use teamarness_package_host::PackageErrorCode;
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let digest = digest(&running, &workspace).await;
    let maximum = json!({"value":"x".repeat(256 * 1024 - 12)});
    assert_eq!(serde_json::to_vec(&maximum).unwrap().len(), 256 * 1024);
    assert_eq!(
        running
            .call(request(
                &workspace,
                &digest,
                "proof.settings.set",
                maximum.clone()
            ))
            .await
            .unwrap(),
        maximum
    );
    let before = status(&running, &workspace, &digest).await;
    let mut deep = json!(null);
    for _ in 0..66 {
        deep = json!([deep]);
    }
    for (operation, payload) in [
        (
            "proof.settings.set",
            json!({"value":"x".repeat(256 * 1024 - 11)}),
        ),
        ("proof.settings.set", json!({"value":deep})),
        ("proof.settings.set", json!({"value":false})),
        ("proof.unknown", json!({})),
        ("proof.watch", json!({})),
    ] {
        let error = running
            .call(request(&workspace, &digest, operation, payload))
            .await
            .unwrap_err();
        assert!(!error.outcome_uncertain);
        assert_eq!(envelope(error).code, PackageErrorCode::UiContractViolation);
        assert_eq!(status(&running, &workspace, &digest).await, before);
    }
    let stale = running
        .call(request(
            &workspace,
            &"0".repeat(64),
            "proof.settings.set",
            json!({"value":"not-dispatched"}),
        ))
        .await
        .unwrap_err();
    assert_eq!(
        stale.code,
        teamarness_headless::ErrorCode::ContractDigestMismatch
    );
    assert!(!stale.outcome_uncertain);
    assert_eq!(status(&running, &workspace, &digest).await, before);
    running.stop().await;
}

#[tokio::test]
async fn package_errors_invalid_responses_and_bounded_one_time_secrets_remain_distinct() {
    use teamarness_package_host::PackageErrorCode;
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let digest = digest(&running, &workspace).await;
    let package_error = running
        .call(request(&workspace, &digest, "proof.error", json!({})))
        .await
        .unwrap_err();
    assert!(
        package_error.outcome_uncertain,
        "the shared UI envelope also drops uncertainty on late Package errors"
    );
    assert!(!package_error.retryable);
    let package = envelope(package_error);
    assert_eq!(package.code, PackageErrorCode::BackendError);
    let owned = package.package_error.unwrap();
    assert_eq!(owned.code, "proof.deliberate");
    assert!(!owned.retryable);
    assert_eq!(owned.details["reason"], "expected");
    let invalid_error = running
        .call(request(&workspace, &digest, "proof.invalid", json!({})))
        .await
        .unwrap_err();
    assert!(
        invalid_error.outcome_uncertain,
        "a response violation does not retract Package effects"
    );
    assert!(!invalid_error.retryable);
    let invalid = envelope(invalid_error);
    assert_eq!(invalid.code, PackageErrorCode::UiContractViolation);
    assert!(invalid.package_error.is_none());
    assert_eq!(
        running
            .call(request(&workspace, &digest, "proof.secret", json!({})))
            .await
            .unwrap(),
        json!({"secret":"ONE_TIME_SECRET_CANARY"})
    );
    assert_eq!(
        running
            .call(request(&workspace, &digest, "proof.secret", json!({})))
            .await
            .unwrap(),
        json!({"secret":null})
    );
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted
            .call(request(&workspace, &digest, "proof.secret", json!({})))
            .await
            .unwrap(),
        json!({"secret":null})
    );
    restarted.stop().await;
}

#[tokio::test]
async fn metadata_is_admitted_not_live_source_and_reload_changes_complete_definitions_and_digest() {
    let fixture = support::Fixture::new();
    let (running, workspace, directory) = setup(&fixture).await;
    let old = running
        .call(schema(&workspace, "proof.settings.set"))
        .await
        .unwrap();
    let old_operations = running.call(operations(&workspace)).await.unwrap();
    let listed = old_operations["operations"].as_array().unwrap();
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0]["name"].as_str() <= pair[1]["name"].as_str())
    );
    assert!(
        listed
            .iter()
            .any(|op| op == &json!({"name":"proof.watch","kind":"subscription"}))
    );
    let mut document = packages::request_contract();
    document["$defs"]["unused"]["description"] = json!("new admission, including unused defs");
    let next_digest = packages::write_contract(&directory, &document);
    assert_ne!(old["contractDigest"], next_digest);
    assert_eq!(
        running
            .call(schema(&workspace, "proof.settings.set"))
            .await
            .unwrap(),
        old
    );
    assert_eq!(
        running.call(operations(&workspace)).await.unwrap(),
        old_operations
    );
    running
        .call(Operation::PackagesReload {
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let current = running
        .call(schema(&workspace, "proof.settings.set"))
        .await
        .unwrap();
    assert_eq!(current["contractDigest"], next_digest);
    assert_eq!(current["schema"]["$defs"], document["$defs"]);
    assert_eq!(
        current["schema"]["requests"],
        json!({"proof.settings.set":document["requests"]["proof.settings.set"]})
    );
    assert_eq!(current["schema"]["subscriptions"], json!({}));
    assert_eq!(
        running
            .call(request(
                &workspace,
                old["contractDigest"].as_str().unwrap(),
                "proof.settings.set",
                json!({"value":"stale"})
            ))
            .await
            .unwrap_err()
            .code,
        teamarness_headless::ErrorCode::ContractDigestMismatch
    );
    assert_eq!(status(&running, &workspace, &next_digest).await["calls"], 0);
    // Existing admission refusal also governs metadata; malformed source cannot replace admission.
    document["unexpected"] = json!(true);
    packages::write_contract(&directory, &document);
    assert_eq!(
        envelope(
            running
                .call(Operation::PackagesReload {
                    package: PERSONAL.parse().unwrap()
                })
                .await
                .unwrap_err()
        )
        .code,
        teamarness_package_host::PackageErrorCode::UiContractViolation
    );
    assert_eq!(
        envelope(
            running
                .call(schema(&workspace, "proof.settings.set"))
                .await
                .unwrap_err()
        )
        .code,
        teamarness_package_host::PackageErrorCode::InvalidLifecycleState
    );
    running.stop().await;
}

fn durable_state(
    fixture: &support::Fixture,
    installation: &str,
    workspace: &str,
) -> std::path::PathBuf {
    fixture
        .config
        .data_directory
        .join("packages/data/v1")
        .join(installation)
        .join(workspace)
        .join(packages::digest(PERSONAL.as_bytes()))
        .join("state.json")
}
async fn wait_started(file: &std::path::Path, count: u64) {
    // Fixture-owned durable marker supplies deterministic timing, not a guessed sleep.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if std::fs::read(file)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| value["started"] == count)
            {
                break;
            }
            // poll-interval: observe fixture's durable start marker before disconnect or reload.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture mutation reached its handler");
}
async fn observe_status(running: &support::Running, workspace: &str, digest: &str) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match running
                .call(request(workspace, digest, "proof.status", json!({})))
                .await
            {
                Ok(value) => return value,
                Err(error) if error.code == teamarness_headless::ErrorCode::Busy => {
                    // poll-interval: read-only observation waits for owned lease cleanup.
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(error) => panic!("unexpected status failure: {error:?}"),
            }
        }
    })
    .await
    .expect("operator lease and capacity released")
}

#[tokio::test]
async fn busy_does_not_replace_a_lease_and_observer_disconnect_does_not_cancel_or_replay_work() {
    use teamarness_headless::control::{Request, write_frame};
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let digest = digest(&running, &workspace).await;
    let file = durable_state(
        &fixture,
        &running.app.status().await.installation,
        &workspace,
    );
    let mut stream = tokio::net::UnixStream::connect(fixture.socket())
        .await
        .unwrap();
    write_frame(
        &mut stream,
        &Request {
            major: 1,
            id: 10,
            operation: request(&workspace, &digest, "proof.mutate", json!({"delayMs":500})),
        },
    )
    .await
    .unwrap();
    wait_started(&file, 1).await;
    let busy = running
        .call(request(
            &workspace.to_uppercase(),
            &digest,
            "proof.settings.set",
            json!({"value":"must-not-run"}),
        ))
        .await
        .unwrap_err();
    assert_eq!(busy.code, teamarness_headless::ErrorCode::Busy);
    assert!(!busy.outcome_uncertain);
    drop(stream);
    let finished = observe_status(&running, &workspace, &digest).await;
    assert_eq!(finished, json!({"calls":1,"started":1,"mutations":1}));
    assert_eq!(
        status(&running, &workspace, &digest).await,
        finished,
        "no replay"
    );
    assert_eq!(
        running
            .call(request(
                &workspace,
                &digest,
                "proof.settings.get",
                json!({})
            ))
            .await
            .unwrap(),
        json!({"value":""})
    );
    // More than the global operation cap in sequence proves permits are eventually released.
    for _ in 0..12 {
        status(&running, &workspace, &digest).await;
    }
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        status(&restarted, &workspace, &digest).await["mutations"],
        1
    );
    restarted.stop().await;
}

#[tokio::test]
async fn concurrent_reload_invalidates_the_old_generation_without_translating_or_replaying_requests()
 {
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let digest = digest(&running, &workspace).await;
    let file = durable_state(
        &fixture,
        &running.app.status().await.installation,
        &workspace,
    );
    let pending = running.call(request(
        &workspace,
        &digest,
        "proof.mutate",
        json!({"delayMs":5000}),
    ));
    let reload = async {
        wait_started(&file, 1).await;
        running
            .call(Operation::PackagesReload {
                package: PERSONAL.parse().unwrap(),
            })
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(pending, reload);
    let error = result.expect_err("old generation cannot return a new generation's result");
    assert!(
        error.outcome_uncertain,
        "dispatched mutation cannot be claimed to have no effect: {error:?}"
    );
    assert!(!error.retryable);
    assert_eq!(
        envelope(error).code,
        teamarness_package_host::PackageErrorCode::ProcessExited
    );
    let current = observe_status(&running, &workspace, &digest).await;
    assert_eq!(current["started"], 1);
    assert_eq!(current["calls"], 1);
    assert_eq!(current["mutations"], 0);
    running.stop().await;
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.ID-R10.
#[tokio::test]
async fn fixture_binding_is_its_own_capability_with_stewardship_isolation_and_persistence() {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let (app, workspace, invitation) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let directory = packages::stage(&fixture.root.path().join("independent"), PERSONAL);
    running
        .call(Operation::PackagesInstall {
            directory,
            expected_identity: PERSONAL.parse().unwrap(),
            accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
        })
        .await
        .unwrap();
    let digest = digest(&running, &workspace).await;
    let agent = running
        .call(Operation::AgentAdd {
            workspace: workspace.clone(),
            name: "Fixture stewarded".into(),
        })
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        running
            .call(request(
                &workspace,
                &digest,
                "proof.binding.current",
                json!({})
            ))
            .await
            .unwrap(),
        json!({"agent":null})
    );
    let bound = running
        .call(request(
            &workspace,
            &digest,
            "proof.binding.set",
            json!({"principalId":agent}),
        ))
        .await
        .unwrap();
    assert_eq!(bound["agent"]["principalId"], agent);
    assert_eq!(bound["agent"]["name"], "Fixture stewarded");
    let mut team = request(&workspace, &digest, "proof.binding.current", json!({}));
    if let Operation::PackageRequest { package, .. } = &mut team {
        *package = packages::TEAM.parse().unwrap();
    }
    assert_eq!(
        running.call(team.clone()).await.unwrap(),
        json!({"agent":null}),
        "another Package's binding did not change"
    );
    let extra = running
        .call(request(
            &workspace,
            &digest,
            "proof.binding.set",
            json!({"principalId":agent,"package":packages::TEAM}),
        ))
        .await
        .unwrap_err();
    assert!(!extra.outcome_uncertain);
    assert_eq!(
        envelope(extra).code,
        teamarness_package_host::PackageErrorCode::UiContractViolation
    );
    assert_eq!(
        running.call(team.clone()).await.unwrap(),
        json!({"agent":null})
    );

    let peer_fixture = support::Fixture::new();
    let peer = support::Running::start(peer_fixture.open().await).await;
    peer.call(Operation::WorkspaceJoin {
        invitation,
        person_name: "Other steward".into(),
    })
    .await
    .unwrap();
    let peer_directory = packages::stage(&peer_fixture.root.path().join("independent"), PERSONAL);
    peer.call(Operation::PackagesInstall {
        directory: peer_directory,
        expected_identity: PERSONAL.parse().unwrap(),
        accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
    })
    .await
    .unwrap();
    // Join already includes the creator's current Agent; a foreign Person cannot bind it.
    let denied = envelope(
        peer.call(request(
            &workspace,
            &digest,
            "proof.binding.set",
            json!({"principalId":agent}),
        ))
        .await
        .unwrap_err(),
    );
    assert_eq!(denied.package_error.unwrap().code, "agent_not_stewarded");
    peer.stop().await;
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted
            .call(request(
                &workspace,
                &digest,
                "proof.binding.current",
                json!({})
            ))
            .await
            .unwrap(),
        bound
    );
    assert_eq!(
        restarted
            .call(request(
                &workspace,
                &digest,
                "proof.binding.clear",
                json!({})
            ))
            .await
            .unwrap(),
        json!({"agent":null})
    );
    assert_eq!(
        restarted
            .call(request(
                &workspace,
                &digest,
                "proof.binding.current",
                json!({})
            ))
            .await
            .unwrap(),
        json!({"agent":null})
    );
    assert_eq!(restarted.call(team).await.unwrap(), json!({"agent":null}));
    restarted.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted
            .call(request(
                &workspace,
                &digest,
                "proof.binding.current",
                json!({})
            ))
            .await
            .unwrap(),
        json!({"agent":null})
    );
    restarted.stop().await;
}

#[test]
fn package_cli_requires_explicit_workspace_package_operation_digest_and_private_input() {
    use std::ffi::OsString;
    let base = ["--data-dir", "/tmp/not-created", "package"];
    for tail in [
        vec!["operations"],
        vec!["operations", "--package", PERSONAL],
        vec!["operations", "--workspace", &"a".repeat(32)],
        vec![
            "schema",
            "--workspace",
            &"a".repeat(32),
            "--package",
            PERSONAL,
        ],
        vec![
            "request",
            "--workspace",
            &"a".repeat(32),
            "--package",
            PERSONAL,
            "--operation",
            "proof.status",
            "--input",
            "-",
        ],
        vec![
            "request",
            "--workspace",
            &"a".repeat(32),
            "--package",
            PERSONAL,
            "--operation",
            "proof.status",
            "--contract-digest",
            &"b".repeat(64),
        ],
        vec![
            "request",
            "--workspace",
            &"a".repeat(32),
            "--package",
            PERSONAL,
            "--operation",
            "proof.status",
            "--contract-digest",
            &"b".repeat(64),
            "--input",
            "-",
            "--json",
            "{}",
        ],
    ] {
        assert!(
            teamarness_headless::cli::parse(base.iter().chain(tail.iter()).map(OsString::from))
                .is_err()
        );
    }
}

#[tokio::test]
async fn actual_cli_uses_admitted_metadata_and_private_file_json_and_reports_lost_acknowledgement()
{
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let fixture = support::Fixture::new();
    let (running, workspace, _) = setup(&fixture).await;
    let listing = running
        .cli(&[
            "package",
            "operations",
            "--workspace",
            &workspace,
            "--package",
            PERSONAL,
        ])
        .await;
    assert!(listing.status.success());
    let listing: Value = serde_json::from_slice(&listing.stdout).unwrap();
    let digest = listing["contractDigest"].as_str().unwrap();
    let schema = running
        .cli(&[
            "package",
            "schema",
            "--workspace",
            &workspace,
            "--package",
            PERSONAL,
            "--operation",
            "proof.mutate",
        ])
        .await;
    assert!(schema.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&schema.stdout).unwrap()["contractDigest"],
        digest
    );
    let input = fixture.root.path().join("input.json");
    std::fs::write(&input, br#"{"delayMs":0}"#).unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o600)).unwrap();
    let arguments = [
        "package",
        "request",
        "--workspace",
        &workspace,
        "--package",
        PERSONAL,
        "--operation",
        "proof.mutate",
        "--contract-digest",
        digest,
        "--input",
        input.to_str().unwrap(),
    ];
    let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .arg("--data-dir")
        .arg(running.app.data_directory())
        .args(arguments)
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
        .wait_with_output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("outcome_unknown"));
    assert_eq!(
        status(&running, &workspace, digest).await,
        json!({"calls":1,"started":1,"mutations":1})
    );
    for bytes in [b"{} {}".as_slice(), br#"{"secret":"PRIVATE_JSON_CANARY""#] {
        std::fs::write(&input, bytes).unwrap();
        let rejected = running.cli(&arguments).await;
        assert_eq!(rejected.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&rejected.stderr).contains("PRIVATE_JSON_CANARY"));
    }
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(running.cli(&arguments).await.status.code(), Some(2));
    assert_eq!(status(&running, &workspace, digest).await["calls"], 1);
    running.stop().await;
}
