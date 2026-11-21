mod support;

use std::ffi::OsString;
use teamarness_headless::cli;

#[test]
fn explicit_onboarding_commands_parse_without_opening_storage() {
    for args in [
        vec!["identity", "show"],
        vec!["identity", "set-name", "--name", "Local Installation"],
        vec!["workspace", "list"],
        vec![
            "workspace",
            "join",
            "--person-name",
            "Ada",
            "--invitation-file",
            "-",
        ],
        vec!["workspace", "members", "--workspace", &"a".repeat(32)],
        vec![
            "agent",
            "add",
            "--workspace",
            &"a".repeat(32),
            "--name",
            "Helper",
        ],
    ] {
        let arguments = [vec!["--data-dir", "/tmp/not-created"], args].concat();
        assert!(cli::parse(arguments.into_iter().map(OsString::from)).is_ok());
    }
}

#[test]
fn no_creation_manager_author_override_or_implicit_selection() {
    for args in [
        vec!["workspace", "create", "--name", "Team"],
        vec!["workspace", "members"],
        vec!["workspace", "members", "--workspace", "Team"],
        vec![
            "workspace",
            "join",
            "--person-name",
            "Ada",
            "--invitation",
            "CANARY",
        ],
        vec![
            "agent",
            "add",
            "--workspace",
            &"a".repeat(32),
            "--name",
            "Helper",
            "--principal",
            &"b".repeat(64),
        ],
    ] {
        let arguments = [vec!["--data-dir", "/tmp/not-created"], args].concat();
        assert!(cli::parse(arguments.into_iter().map(OsString::from)).is_err());
    }
}

use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use teamarness_headless::control::{self, Operation, Request};

// Verifies: TEAM.SYS.ID-R05, TEAM.SYS.ID-R09, TEAM.SYS.MEM-R09.
#[tokio::test]
async fn real_invitation_names_roster_and_restart_persist_between_two_installations() {
    let issuer = support::Fixture::new();
    let joiner = support::Fixture::new();
    let (app, workspace, invitation) = support::inviter(&issuer).await;
    let issuer = support::Running::start(app).await;
    let joiner_running = support::Running::start(joiner.open().await).await;
    let identity = joiner_running
        .call(Operation::IdentityShow {})
        .await
        .unwrap();
    assert_ne!(
        identity["installation"],
        issuer.call(Operation::IdentityShow {}).await.unwrap()["installation"]
    );
    let file = joiner.root.path().join("invitation");
    std::fs::write(&file, &invitation).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = joiner_running
        .cli(&[
            "workspace",
            "join",
            "--person-name",
            "Ada",
            "--invitation-file",
            file.to_str().unwrap(),
        ])
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["membershipCommitted"],
        true
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&invitation));
    for args in [
        vec!["identity", "set-name", "--name", "Node Two"],
        vec![
            "workspace",
            "rename-self",
            "--workspace",
            &workspace,
            "--name",
            "Ada Two",
        ],
    ] {
        assert!(joiner_running.cli(&args).await.status.success());
    }
    let roster = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let roster = joiner_running
                .call(Operation::WorkspaceMembers {
                    workspace: workspace.clone(),
                })
                .await
                .unwrap();
            if roster
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == "Issuer")
            {
                break roster;
            }
            // poll-interval: wait for the issuer's replicated name before comparing restart state.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(roster.as_array().unwrap().len(), 2);
    assert!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "Ada Two")
    );
    assert_eq!(
        joiner_running
            .call(Operation::AgentList {
                workspace: workspace.clone()
            })
            .await
            .unwrap(),
        json!([])
    );
    joiner_running.stop().await;
    let restarted = support::Running::start(joiner.open().await).await;
    let current = restarted.call(Operation::IdentityShow {}).await.unwrap();
    assert_eq!(current["installation"], identity["installation"]);
    assert_eq!(current["displayName"], "Node Two");
    assert_eq!(
        restarted.call(Operation::WorkspaceList {}).await.unwrap()[0]["id"],
        workspace
    );
    assert_eq!(
        restarted
            .call(Operation::WorkspaceMembers { workspace })
            .await
            .unwrap(),
        roster
    );
    restarted.stop().await;
    issuer.stop().await;
}

// Verifies: TEAM.SYS.ID-R09, TEAM.SYS.MEM-R08.
#[tokio::test]
async fn agent_stewardship_names_and_removal_use_core_authority_without_actor_override() {
    let issuer_fixture = support::Fixture::new();
    let joiner_fixture = support::Fixture::new();
    let (app, workspace, invitation) = support::inviter(&issuer_fixture).await;
    let issuer = support::Running::start(app).await;
    let joiner = support::Running::start(joiner_fixture.open().await).await;
    joiner
        .call(Operation::WorkspaceJoin {
            invitation,
            person_name: "Steward".into(),
        })
        .await
        .unwrap();
    let agent = joiner
        .call(Operation::AgentAdd {
            workspace: workspace.clone(),
            name: "Helper".into(),
        })
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agents = joiner
        .call(Operation::AgentList {
            workspace: workspace.clone(),
        })
        .await
        .unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert_eq!(agents[0]["name"], "Helper");
    joiner
        .call(Operation::AgentRename {
            workspace: workspace.clone(),
            agent: agent.clone(),
            name: "Assistant".into(),
        })
        .await
        .unwrap();
    let bad = joiner
        .call(Operation::AgentAdd {
            workspace: workspace.clone(),
            name: " ".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(bad.details, Some(teamarness_headless::ErrorDetails::Core { ref code, .. }) if code == "InvalidName")
    );
    // Poll the real replicated roster, not a guessed settling sleep.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let roster = issuer
                .call(Operation::AgentList {
                    workspace: workspace.clone(),
                })
                .await
                .unwrap();
            if roster
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == agent)
            {
                break;
            }
            // poll-interval: bounded public-roster observation, never mutation retry.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let error = issuer
        .call(Operation::AgentRename {
            workspace: workspace.clone(),
            agent: agent.clone(),
            name: "Not mine".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error.details, Some(teamarness_headless::ErrorDetails::Core { ref code, .. }) if code == "NotOwnPerson")
    );
    issuer
        .call(Operation::AgentRemove {
            workspace: workspace.clone(),
            agent: agent.clone(),
        })
        .await
        .unwrap();
    assert!(
        issuer
            .call(Operation::AgentRemove { workspace, agent })
            .await
            .is_err()
    );
    joiner.stop().await;
    issuer.stop().await;
}

#[tokio::test]
async fn disconnected_mutation_observer_does_not_cancel_or_duplicate_agent_add() {
    let fixture = support::Fixture::new();
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let mut stream = tokio::net::UnixStream::connect(fixture.socket())
        .await
        .unwrap();
    control::write_frame(
        &mut stream,
        &Request {
            major: 1,
            id: 101,
            operation: Operation::AgentAdd {
                workspace: workspace.clone(),
                name: "Exactly once".into(),
            },
        },
    )
    .await
    .unwrap();
    // Observe only a frame-prefix byte, never the terminal result. This establishes server
    // admission before disconnect (macOS cannot authenticate an already-closed peer).
    use tokio::io::AsyncReadExt;
    stream.read_u8().await.unwrap();
    drop(stream);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let agents = running
                .call(Operation::AgentList {
                    workspace: workspace.clone(),
                })
                .await
                .unwrap();
            if agents
                .as_array()
                .unwrap()
                .iter()
                .any(|agent| agent["name"] == "Exactly once")
            {
                assert_eq!(agents.as_array().unwrap().len(), 1);
                break;
            }
            // poll-interval: bounded public-roster observation, never mutation retry.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    running.stop().await;
    let restarted = support::Running::start(fixture.open().await).await;
    assert_eq!(
        restarted
            .call(Operation::AgentList { workspace })
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    restarted.stop().await;
}

#[tokio::test]
async fn malformed_and_authentic_expired_invitations_refuse_without_membership() {
    let fixture = support::Fixture::new();
    let (app, _, invitation) = support::inviter_with_expiry(&fixture, true).await;
    let issuer = support::Running::start(app).await;
    let joiner_fixture = support::Fixture::new();
    let joiner = support::Running::start(joiner_fixture.open().await).await;
    let malformed = joiner
        .call(Operation::WorkspaceJoin {
            invitation: "SECRET_INVITATION_CANARY".into(),
            person_name: "Ada".into(),
        })
        .await
        .unwrap_err();
    assert!(!malformed.outcome_uncertain);
    assert!(!malformed.to_string().contains("SECRET_INVITATION_CANARY"));
    let expired = joiner
        .call(Operation::WorkspaceJoin {
            invitation,
            person_name: "Ada".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(expired.details, Some(teamarness_headless::ErrorDetails::Core { ref code, .. }) if code == "Expired")
    );
    assert!(!expired.outcome_uncertain);
    assert_eq!(
        joiner.call(Operation::WorkspaceList {}).await.unwrap(),
        json!([])
    );
    joiner.stop().await;
    issuer.stop().await;
}

#[tokio::test]
async fn committed_join_reports_later_reconciliation_failure_without_replaying_membership() {
    use support::packages;
    let issuer_fixture = support::Fixture::new();
    let (app, workspace, invitation) = support::inviter(&issuer_fixture).await;
    let issuer = support::Running::start(app).await;
    let fixture = support::Fixture::new();
    let joiner = support::Running::start(fixture.open().await).await;
    let directory = packages::stage(&fixture.root.path().join("personal"), packages::PERSONAL);
    joiner
        .call(Operation::PackagesInstall {
            directory,
            expected_identity: packages::PERSONAL.parse().unwrap(),
            accepted_risks: vec!["filesystem".into(), "network".into(), "process".into()],
        })
        .await
        .unwrap();
    // No Workspace instances exist yet. Refuse their future data directory, not Core storage.
    let data = fixture.config.data_directory.join("packages/data");
    assert!(!data.exists());
    std::fs::write(&data, b"not a directory").unwrap();
    let error = joiner
        .call(Operation::WorkspaceJoin {
            invitation,
            person_name: "Joined".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        teamarness_headless::ErrorCode::ReconciliationFailed
    );
    assert!(
        matches!(error.details, Some(teamarness_headless::ErrorDetails::Joined { membership_committed: true, ref workspace, .. }) if workspace["id"].is_string())
    );
    assert_eq!(
        joiner.call(Operation::WorkspaceList {}).await.unwrap()[0]["id"],
        workspace
    );
    assert_eq!(
        issuer
            .call(Operation::WorkspaceMembers { workspace })
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    std::fs::remove_file(data).unwrap();
    joiner.stop().await;
    issuer.stop().await;
}

#[test]
fn personal_install_names_the_expected_package_with_the_required_package_selector() {
    let args = [
        "--data-dir",
        "/tmp/not-created",
        "packages",
        "install",
        "--directory",
        "/tmp/source",
        "--package",
        "org.unfamiliar.headless-proof",
        "--input",
        "-",
    ];
    assert!(cli::parse(args.into_iter().map(OsString::from)).is_ok());
}

#[tokio::test]
async fn broken_client_output_reports_unknown_without_replaying_the_committed_mutation() {
    use std::os::fd::OwnedFd;
    use std::process::Stdio;
    let fixture = support::Fixture::new();
    let (app, workspace, _) = support::inviter(&fixture).await;
    let running = support::Running::start(app).await;
    let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .arg("--data-dir")
        .arg(&fixture.config.data_directory)
        .args([
            "agent",
            "add",
            "--workspace",
            &workspace,
            "--name",
            "Once despite broken output",
        ])
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
        .wait_with_output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("outcome_unknown"));
    let roster = running
        .call(Operation::AgentList { workspace })
        .await
        .unwrap();
    assert_eq!(roster.as_array().unwrap().len(), 1);
    assert_eq!(roster[0]["name"], "Once despite broken output");
    assert!(roster[0].get("keys").is_none());
    running.stop().await;
}

#[tokio::test]
async fn upstream_errors_are_explicit_json_results_not_routine_diagnostic_details() {
    let fixture = support::Fixture::new();
    let running = support::Running::start(fixture.open().await).await;
    let output = running.cli(&["identity", "set-name", "--name", " "]).await;
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("explicit bounded JSON error result");
    assert_eq!(error["details"]["family"], "identity");
    assert_eq!(error["details"]["code"], "InvalidDisplayName");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("details"));
    running.stop().await;
}
