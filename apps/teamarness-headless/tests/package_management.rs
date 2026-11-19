mod support;

use serde_json::{Value, json};
use std::path::Path;
use support::packages::{self, PERSONAL, TEAM};
use teamarness_headless::control::Operation;
use teamarness_headless::{ControlError, ErrorDetails};
use teamarness_package_host::PackageErrorCode;

fn install(directory: &Path, identity: &str) -> Operation {
    Operation::PackagesInstall {
        directory: directory.to_owned(),
        expected_identity: identity.parse().unwrap(),
        accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
    }
}
fn host_code(error: ControlError) -> PackageErrorCode {
    match error.details.unwrap() {
        ErrorDetails::Host { envelope } => envelope.code,
        other => panic!("expected Host failure, got {other:?}"),
    }
}
fn instance<'a>(snapshot: &'a Value, package: &str) -> &'a Value {
    snapshot["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"]["packageIdentity"] == package)
        .unwrap()
}
fn data_directory(
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
}

#[test]
fn unknown_personal_fixture_is_built_from_current_source_not_a_prebuilt_artifact() {
    assert!(packages::executable().is_file());
}

// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.ID-R01.
#[tokio::test]
async fn inspect_consent_admission_live_generations_and_retained_data_do_not_disturb_team() {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let before = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(
        before["packages"].as_array().unwrap().len(),
        1,
        "Team fixture admitted"
    );
    let team_before = instance(&before, TEAM).clone();
    assert_eq!(team_before["lifecycle"], "active");
    let installation = running.app.status().await.installation;
    let data = data_directory(&fixture, &installation, &workspace);
    let directory = packages::stage(&fixture.root.path().join("independent"), PERSONAL);
    let inspected = running
        .call(Operation::PackagesInspect {
            directory: directory.clone(),
        })
        .await
        .unwrap();
    assert_eq!(inspected["identity"], PERSONAL);
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesInspect {
                    directory: ".".into()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::InvalidRegistration
    );
    let symlink = fixture.root.path().join("source-symlink");
    std::os::unix::fs::symlink(&directory, &symlink).unwrap();
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesInspect { directory: symlink })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::InvalidRegistration
    );

    assert!(
        !data.exists(),
        "inspection never executed the eager Backend"
    );
    assert_eq!(
        running.call(Operation::PackagesList {}).await.unwrap()["packages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesInstall {
                    directory: directory.clone(),
                    expected_identity: PERSONAL.parse().unwrap(),
                    accepted_risks: vec![]
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::TrustEvidenceMissing
    );
    assert_eq!(
        host_code(
            running
                .call(install(&directory, "org.other.identity"))
                .await
                .unwrap_err()
        ),
        PackageErrorCode::IdentityMismatch
    );
    assert!(!data.exists());
    running.call(install(&directory, PERSONAL)).await.unwrap();
    let installed = running.call(Operation::PackagesList {}).await.unwrap();
    let first_generation = instance(&installed, PERSONAL)["processGeneration"]
        .as_u64()
        .unwrap();
    assert!(first_generation > 0);
    assert_eq!(instance(&installed, PERSONAL)["lifecycle"], "active");
    assert_eq!(instance(&installed, TEAM), &team_before);
    assert_eq!(
        std::fs::read_to_string(data.join("activations")).unwrap(),
        "1"
    );
    let retained = std::fs::read(data.join("retained")).unwrap();
    running
        .call(Operation::PackagesReload {
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let reloaded = running.call(Operation::PackagesList {}).await.unwrap();
    let reloaded_generation = instance(&reloaded, PERSONAL)["processGeneration"]
        .as_u64()
        .unwrap();
    assert_eq!(reloaded_generation, first_generation + 1);
    assert_eq!(instance(&reloaded, TEAM), &team_before);
    running
        .call(Operation::PackagesRestart {
            workspace: workspace.clone(),
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let restarted = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(
        instance(&restarted, PERSONAL)["processGeneration"],
        reloaded_generation + 1
    );
    running
        .call(Operation::PackagesPause {
            workspace: workspace.to_uppercase(),
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let paused = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(instance(&paused, PERSONAL)["lifecycle"], "suspended");
    running
        .call(Operation::PackagesResume {
            workspace: workspace.clone(),
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let resumed = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(instance(&resumed, PERSONAL)["lifecycle"], "active");
    assert_eq!(instance(&resumed, TEAM), &team_before);
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesReload {
                    package: TEAM.parse().unwrap()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::NotRegistered
    );
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesRemove {
                    package: TEAM.parse().unwrap()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::NotRegistered
    );
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesRestart {
                    workspace,
                    package: TEAM.parse().unwrap()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::OverrideRequired
    );
    running
        .call(Operation::PackagesRemove {
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(std::fs::read(data.join("retained")).unwrap(), retained);
    assert_eq!(
        running.call(Operation::PackagesList {}).await.unwrap()["instances"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    running.call(install(&directory, PERSONAL)).await.unwrap();
    assert_eq!(std::fs::read(data.join("retained")).unwrap(), retained);
    let final_snapshot = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(instance(&final_snapshot, TEAM), &team_before);
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted.call(Operation::PackagesList {}).await.unwrap()["packages"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "registration is Installation-wide and persisted"
    );
    assert_eq!(std::fs::read(data.join("retained")).unwrap(), retained);
    restarted.stop().await;
}

#[tokio::test]
async fn invalid_digest_target_and_team_collision_refuse_before_execution() {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let installation = running.app.status().await.installation;
    let data = data_directory(&fixture, &installation, &workspace);
    let wrong_digest = packages::stage(&fixture.root.path().join("wrong-digest"), PERSONAL);
    packages::edit_manifest(&wrong_digest, |manifest| {
        manifest["backends"][0]["sha256"] = json!("0".repeat(64))
    });
    assert_eq!(
        host_code(
            running
                .call(install(&wrong_digest, PERSONAL))
                .await
                .unwrap_err()
        ),
        PackageErrorCode::ArtifactChanged
    );
    let unsupported = packages::stage(&fixture.root.path().join("wrong-target"), PERSONAL);
    packages::edit_manifest(&unsupported, |manifest| {
        manifest["backends"][0]["target"] = json!("unsupported-native-target")
    });
    assert_eq!(
        host_code(
            running
                .call(install(&unsupported, PERSONAL))
                .await
                .unwrap_err()
        ),
        PackageErrorCode::TargetUnsupported
    );
    let collision = packages::stage(&fixture.root.path().join("team-collision"), TEAM);
    assert_eq!(
        host_code(running.call(install(&collision, TEAM)).await.unwrap_err()),
        PackageErrorCode::OverrideRequired
    );
    assert!(!data.exists());
    assert_eq!(
        running.call(Operation::PackagesList {}).await.unwrap()["packages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    running.stop().await;
}

#[tokio::test]
async fn actual_cli_install_uses_one_private_json_consent_and_explicit_ids() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = support::Fixture::new();
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let directory = packages::stage(&fixture.root.path().join("personal"), PERSONAL);
    let consent = fixture.root.path().join("consent.json");
    std::fs::write(
        &consent,
        br#"{"acceptedRisks":["filesystem","network","process"]}"#,
    )
    .unwrap();
    std::fs::set_permissions(&consent, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = running
        .cli(&[
            "packages",
            "install",
            "--directory",
            directory.to_str().unwrap(),
            "--package",
            PERSONAL,
            "--input",
            consent.to_str().unwrap(),
        ])
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for args in [
        vec![
            "packages",
            "pause",
            "--workspace",
            &workspace,
            "--package",
            PERSONAL,
        ],
        vec![
            "packages",
            "resume",
            "--workspace",
            &workspace,
            "--package",
            PERSONAL,
        ],
        vec!["packages", "reload", "--package", PERSONAL],
        vec!["packages", "remove", "--package", PERSONAL],
    ] {
        assert!(running.cli(&args).await.status.success());
    }
    std::fs::write(
        &consent,
        br#"{"acceptedRisks":["SECRET_CONSENT_CANARY"]} {}"#,
    )
    .unwrap();
    let error = running
        .cli(&[
            "packages",
            "install",
            "--directory",
            directory.to_str().unwrap(),
            "--package",
            PERSONAL,
            "--input",
            consent.to_str().unwrap(),
        ])
        .await;
    assert_eq!(error.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&error.stderr).contains("SECRET_CONSENT_CANARY"));
    running.stop().await;
}

#[tokio::test]
async fn installation_wide_install_reload_remove_cover_all_workspaces_and_preserve_team_generations()
 {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let app = teamarness_headless::application::Application::open_with(
        fixture.config.clone(),
        |config| async {
            let core = teamarness_core::CoreRuntime::open_loopback_for_tests(config).await?;
            core.create_workspace_with_person_name("One", "Person One")
                .unwrap();
            core.create_workspace_with_person_name("Two", "Person Two")
                .unwrap();
            Ok(core)
        },
    )
    .await
    .unwrap();
    let running = support::Running::start(app).await;
    let before = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(before["instances"].as_array().unwrap().len(), 2);
    let team = before["instances"].clone();
    let directory = packages::stage(&fixture.root.path().join("personal"), PERSONAL);
    running.call(install(&directory, PERSONAL)).await.unwrap();
    let installed = running.call(Operation::PackagesList {}).await.unwrap();
    let personal: Vec<Value> = installed["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["key"]["packageIdentity"] == PERSONAL)
        .cloned()
        .collect();
    assert_eq!(personal.len(), 2);
    packages::edit_manifest(&directory, |manifest| {
        manifest["backends"][0]["sha256"] = json!("0".repeat(64))
    });
    assert_eq!(
        host_code(
            running
                .call(Operation::PackagesReload {
                    package: PERSONAL.parse().unwrap()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::ArtifactChanged
    );
    let correct = packages::digest(&std::fs::read(packages::executable()).unwrap());
    packages::edit_manifest(&directory, |manifest| {
        manifest["backends"][0]["sha256"] = json!(correct)
    });
    running
        .call(Operation::PackagesReload {
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    let reloaded = running.call(Operation::PackagesList {}).await.unwrap();
    for previous in personal {
        let current = reloaded["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == previous["key"])
            .unwrap();
        assert!(
            current["processGeneration"].as_u64().unwrap()
                > previous["processGeneration"].as_u64().unwrap()
        );
        assert_eq!(current["lifecycle"], "active");
    }
    running
        .call(Operation::PackagesRemove {
            package: PERSONAL.parse().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        running.call(Operation::PackagesList {}).await.unwrap()["instances"],
        team
    );
    running.stop().await;
}

#[tokio::test]
async fn team_pause_resume_are_refused_before_dispatch_without_changing_generations_or_core_sync() {
    let issuer_fixture = support::Fixture::new();
    packages::bundle(&issuer_fixture);
    let (app, workspace, invitation) = support::inviter(&issuer_fixture).await;
    let issuer = support::Running::start(app).await;
    let joiner_fixture = support::Fixture::new();
    let joiner = support::Running::start(joiner_fixture.open().await).await;
    joiner
        .call(Operation::WorkspaceJoin {
            invitation,
            person_name: "Peer".into(),
        })
        .await
        .unwrap();
    let before = issuer.call(Operation::PackagesList {}).await.unwrap();
    let team = instance(&before, TEAM).clone();
    for operation in [
        Operation::PackagesPause {
            workspace: workspace.clone(),
            package: TEAM.parse().unwrap(),
        },
        Operation::PackagesResume {
            workspace: workspace.clone(),
            package: TEAM.parse().unwrap(),
        },
    ] {
        let error = issuer.call(operation).await.unwrap_err();
        assert!(!error.outcome_uncertain);
        assert_eq!(host_code(error), PackageErrorCode::OverrideRequired);
        assert_eq!(
            instance(
                &issuer.call(Operation::PackagesList {}).await.unwrap(),
                TEAM
            ),
            &team
        );
    }
    // An absent instance retains the generic Host's existing absence verdict.
    assert_eq!(
        host_code(
            issuer
                .call(Operation::PackagesPause {
                    workspace: "0".repeat(32),
                    package: TEAM.parse().unwrap()
                })
                .await
                .unwrap_err()
        ),
        PackageErrorCode::InvalidLifecycleState
    );
    let id = issuer
        .call(Operation::AgentAdd {
            workspace: workspace.clone(),
            name: "Sync after refusal".into(),
        })
        .await
        .unwrap()["id"]
        .clone();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let roster = joiner
                .call(Operation::AgentList {
                    workspace: workspace.clone(),
                })
                .await
                .unwrap();
            if roster.as_array().unwrap().iter().any(|row| row["id"] == id) {
                break;
            }
            // poll-interval: observe actual Core membership synchronization after lifecycle refusals.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        instance(
            &issuer.call(Operation::PackagesList {}).await.unwrap(),
            TEAM
        ),
        &team
    );
    joiner.stop().await;
    issuer.stop().await;
}

async fn lifecycle_effect_then_lost_reply(command: &str, fault: &str, counter: &str) {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let directory = packages::stage(&fixture.root.path().join("personal"), PERSONAL);
    running.call(install(&directory, PERSONAL)).await.unwrap();
    let before = running.call(Operation::PackagesList {}).await.unwrap();
    let team = instance(&before, TEAM).clone();
    let data = data_directory(
        &fixture,
        &running.app.status().await.installation,
        &workspace,
    );
    // The ordinary public lifecycle remains successful before arming the one-shot fixture fault.
    for action in ["pause", "resume", "restart"] {
        let output = running
            .cli(&[
                "packages",
                action,
                "--workspace",
                &workspace,
                "--package",
                PERSONAL,
            ])
            .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if command == "resume" {
        assert!(
            running
                .cli(&[
                    "packages",
                    "pause",
                    "--workspace",
                    &workspace,
                    "--package",
                    PERSONAL
                ])
                .await
                .status
                .success()
        );
    }
    let count = || -> u64 {
        if counter == "activations" {
            std::fs::read_to_string(data.join(counter))
                .unwrap()
                .parse()
                .unwrap()
        } else {
            serde_json::from_slice::<Value>(&std::fs::read(data.join("state.json")).unwrap())
                .unwrap()[counter]
                .as_u64()
                .unwrap()
        }
    };
    // Restart starts a fresh State, so ensure the saved counters exist even in the new process.
    let prior = count();
    std::fs::write(data.join("lose-lifecycle-reply"), fault).unwrap();
    let output = running
        .cli(&[
            "packages",
            command,
            "--workspace",
            &workspace,
            "--package",
            PERSONAL,
        ])
        .await;
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        count(),
        prior + 1,
        "effect committed exactly once before terminal reply loss"
    );
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Check the unchanged original Host envelope independently of the outer conservative verdict.
    let envelope = &error["details"]["envelope"];
    assert_eq!(envelope["code"], "process_exited", "{error}");
    assert_eq!(
        envelope["retryable"], true,
        "original Host retryability retained"
    );
    assert_eq!(envelope["stage"], "lifecycle");
    assert_eq!(envelope["namespace"], "teamarness.package-host");
    assert_eq!(envelope["version"], 2);
    assert_eq!(envelope["affectedInstance"]["packageIdentity"], PERSONAL);
    assert_eq!(envelope["affectedInstance"]["workspaceId"], workspace);
    assert_eq!(error["outcomeUncertain"], true, "{error}");
    assert_eq!(error["retryable"], false, "{error}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("outcome_unknown: inspect state; do not replay automatically")
    );
    assert_eq!(
        instance(
            &running.call(Operation::PackagesList {}).await.unwrap(),
            TEAM
        ),
        &team
    );
    assert_eq!(
        count(),
        prior + 1,
        "read-only observation never replays the lifecycle mutation"
    );
    running.stop().await;
    assert_eq!(
        count(),
        prior + 1,
        "shutdown retains the effect without replay"
    );
}

#[tokio::test]
async fn public_lifecycle_selector_team_missing_and_invalid_state_refusals_remain_certain() {
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let directory = packages::stage(&fixture.root.path().join("personal"), PERSONAL);
    running.call(install(&directory, PERSONAL)).await.unwrap();
    assert!(
        running
            .cli(&[
                "packages",
                "pause",
                "--workspace",
                &workspace,
                "--package",
                PERSONAL
            ])
            .await
            .status
            .success()
    );
    for (action, selected_workspace, package, code) in [
        (
            "restart",
            workspace.as_str(),
            PERSONAL,
            "invalid_lifecycle_state",
        ),
        (
            "pause",
            "00000000000000000000000000000000",
            PERSONAL,
            "invalid_lifecycle_state",
        ),
        (
            "resume",
            "00000000000000000000000000000000",
            PERSONAL,
            "invalid_lifecycle_state",
        ),
        (
            "restart",
            workspace.as_str(),
            "org.unregistered.proof",
            "not_registered",
        ),
        ("pause", workspace.as_str(), TEAM, "override_required"),
        ("resume", workspace.as_str(), TEAM, "override_required"),
        ("restart", workspace.as_str(), TEAM, "override_required"),
    ] {
        let output = running
            .cli(&[
                "packages",
                action,
                "--workspace",
                selected_workspace,
                "--package",
                package,
            ])
            .await;
        assert_eq!(output.status.code(), Some(1));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["details"]["envelope"]["code"], code);
        assert_eq!(error["outcomeUncertain"], false, "{error}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("outcome_unknown"));
    }
    let invalid = running
        .cli(&[
            "packages",
            "pause",
            "--workspace",
            "invalid",
            "--package",
            PERSONAL,
        ])
        .await;
    assert_eq!(invalid.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("outcome_unknown"));
    running.stop().await;
}

#[tokio::test]
async fn public_pause_retains_durable_effect_and_reports_unknown_after_lost_reply() {
    lifecycle_effect_then_lost_reply("pause", "suspend", "suspends").await;
}
#[tokio::test]
async fn public_resume_retains_durable_effect_and_reports_unknown_after_lost_reply() {
    lifecycle_effect_then_lost_reply("resume", "recover", "recovers").await;
}
#[tokio::test]
async fn public_restart_retains_activation_effect_and_reports_unknown_after_lost_reply() {
    lifecycle_effect_then_lost_reply("restart", "restart", "activations").await;
}

#[tokio::test]
async fn local_team_override_keeps_team_lifecycle_restrictions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = support::Fixture::new();
    packages::bundle(&fixture);
    let initialized = fixture.open().await;
    initialized
        .shutdown_by(tokio::time::Instant::now() + teamarness_headless::control::SHUTDOWN_TIMEOUT)
        .await;
    drop(initialized);
    let directory = packages::stage(&fixture.root.path().join("team-override"), TEAM);
    let registration = fixture
        .config
        .data_directory
        .join("packages/registrations.v1.json");
    std::fs::write(&registration, serde_json::to_vec(&json!({
        "schemaVersion":1,"registrations":[{"directory":directory,"expectedIdentity":TEAM,"overrideApplication":true,
        "trust":{"kind":"trusted_native_code_v1","acceptedRisks":["filesystem","network","process"]}}]
    })).unwrap()).unwrap();
    std::fs::set_permissions(registration, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let before = running.call(Operation::PackagesList {}).await.unwrap();
    assert_eq!(before["packages"][0]["source"], "local");
    assert_eq!(before["registrations"][0]["tier"], "team");
    for operation in [
        Operation::PackagesPause {
            workspace: workspace.clone(),
            package: TEAM.parse().unwrap(),
        },
        Operation::PackagesResume {
            workspace,
            package: TEAM.parse().unwrap(),
        },
    ] {
        assert_eq!(
            host_code(running.call(operation).await.unwrap_err()),
            PackageErrorCode::OverrideRequired
        );
        assert_eq!(
            running.call(Operation::PackagesList {}).await.unwrap()["instances"],
            before["instances"]
        );
    }
    running.stop().await;
}
