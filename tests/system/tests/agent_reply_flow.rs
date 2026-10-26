//! The Agent reply flow across two Profiles.
//!
//! Alice stewards an Agent and runs the real Agent Package against a stub completion endpoint
//! on loopback; Bob is a second Installation with Conversations only. Bob addresses the Agent
//! by name, the reply appears in `#general` on both Installations attributed to the Agent's own
//! key, and an unaddressed Message produces nothing. Hermetic: real `CoreRuntime`s on loopback,
//! real Package Hosts, real Backend processes, nothing off this machine.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use support::harness::{
    self, Addressing, ConversationClient, GENERAL_ID, PACKAGE_ID, create_invitation, introduce,
    object, open_core, open_host, open_mount, poll_until, shutdown,
};
use teamarness_package_host::{
    PackageHostCommand, PackageIdentity, PackageInstanceKey, PackageSourceKind, PackageTrustKind,
    PackageValue, UiRequest, UiRequestId, WorkspaceId,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AGENT_PACKAGE_ID: &str = "dev.teamarness.agent";
const REPLY: &str = "Hello from the stub model";
const KEY: &str = "sk-system-test-key-never-logged";
static NEXT_BINDING_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Explicit external qualification: build the standalone project immediately before running.
/// Never silently skip, substitute a fixture, or claim this proves an unbuilt source revision.
fn register_agent(private: &Path) {
    let directory = fs::canonicalize(std::env::var_os("TEAMARNESS_AGENT_PACKAGE_DIR").expect(
        "build the standalone Agent, then set TEAMARNESS_AGENT_PACKAGE_DIR to its dist directory",
    ))
    .expect("the external Agent Package Directory exists");
    let packages = private.join("packages");
    fs::create_dir_all(&packages).unwrap();
    fs::set_permissions(&packages, fs::Permissions::from_mode(0o700)).unwrap();
    let registration = packages.join("registrations.v1.json");
    fs::write(
        &registration,
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "registrations": [{
                "directory": directory,
                "expectedIdentity": AGENT_PACKAGE_ID,
                "trust": {
                    "kind": "trusted_native_code_v1",
                    "acceptedRisks": ["filesystem", "network", "process"]
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(registration, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A stub OpenAI-compatible endpoint: every `POST` is answered with one fixed completion.
async fn stub_endpoint() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0_u8; 4096];
                let header_end = loop {
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let head = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
                let content_length = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while bytes.len() < header_end + content_length {
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                }
                assert!(
                    head.contains(&format!("Bearer {KEY}")),
                    "the stub saw no bearer key"
                );
                counter.fetch_add(1, Ordering::SeqCst);
                let body =
                    json!({"choices": [{"message": {"role": "assistant", "content": REPLY}}]})
                        .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (base_url, requests)
}

/// Select the offered major through the actual Agent UI, never infer a grant from binding.
async fn enable_conversations(agent: &ConversationClient<'_>) {
    let offered = serde_json::to_value(
        agent
            .request("packages", "agent.packages.get", object([]))
            .await,
    )
    .unwrap();
    let conversations = offered["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["contract"] == PACKAGE_ID)
        .expect("Conversations is offered by the real Host");
    assert_eq!(conversations["available"], true);
    assert_eq!(conversations["enabled"], false);
    assert!(conversations["enabledMajor"].is_null());
    let major = conversations["major"].as_u64().unwrap();
    let enabled = serde_json::to_value(
        agent
            .request(
                "enable-conversations",
                "agent.packages.set",
                object([
                    ("contract", PackageValue::String(PACKAGE_ID.into())),
                    ("enabled", PackageValue::Boolean(true)),
                    ("major", PackageValue::Unsigned(major)),
                ]),
            )
            .await,
    )
    .unwrap();
    let selected = enabled["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["contract"] == PACKAGE_ID)
        .unwrap();
    assert_eq!(selected["enabled"], true);
    assert_eq!(selected["enabledMajor"], major);
}

/// Observe the actual Package-owned binding refusal, not a generic request failure.
async fn binding_refused(
    agent: &ConversationClient<'_>,
    principal: Option<String>,
    expected: &str,
) {
    let (operation, payload) = match principal {
        Some(principal) => (
            "agent.binding.set",
            object([("principalId", PackageValue::String(principal))]),
        ),
        None => ("agent.binding.clear", object([])),
    };
    let error = agent
        .host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: agent.mount.clone(),
            request: UiRequestId::new(format!(
                "refused-{expected}-{operation}-{}",
                NEXT_BINDING_REQUEST.fetch_add(1, Ordering::Relaxed)
            ))
            .unwrap(),
            operation: operation.into(),
            payload,
        }))
        .await
        .expect_err("binding must refuse before Core mutation");
    if error.package_error.is_none() {
        eprintln!(
            "binding refusal Host envelope: {}",
            serde_json::to_string(&error).unwrap()
        );
        public_host_state(agent.host, "binding-refusal-failure").await;
    }
    let owned = error.package_error.expect("Agent-owned binding refusal");
    assert_eq!(owned.code, expected);
    assert_eq!(owned.retryable, expected == "agent.binding_busy");
}

/// The `#general` rows of `client`, as JSON.
async fn general(client: &ConversationClient<'_>, request_id: &str) -> Vec<Value> {
    let page = client.channel_open(request_id, GENERAL_ID, 64).await;
    serde_json::to_value(&page).unwrap()["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn agent_rows<'a>(rows: &'a [Value], agent_key: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["author"] == agent_key)
        .collect()
}

fn progress(root: &Path) -> Option<Value> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == "progress.json") {
            return serde_json::from_slice(&fs::read(path).ok()?).ok();
        }
        if path.is_dir()
            && let Some(value) = progress(&path)
        {
            return Some(value);
        }
    }
    None
}

fn files_contain(root: &Path, needle: &str) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            files_contain(&path, needle)
        } else {
            fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
        }
    })
}

/// Packaging smoke only: real admission, capability grants, activation and a UI-contract request.
/// No model endpoint, delivery/backlog replay, or live developer profile access.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn local_agent_package_loads_from_registration() {
    let app = harness::StagedApplication::new();
    let root = app.root.path();
    let alice_core = open_core(Addressing::Loopback, &root.join("alice-core")).await;
    let workspace = alice_core.create_workspace("Personal Agent load").unwrap();
    let private = root.join("alice-host");
    register_agent(&private);
    let alice_host = open_host(&app.application_path, private, alice_core.clone()).await;
    let snapshot = alice_host.snapshot().await;
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    let agent = snapshot
        .packages
        .iter()
        .find(|package| package.identity.as_str() == AGENT_PACKAGE_ID)
        .expect("the real local Agent is admitted");
    assert_eq!(agent.source, PackageSourceKind::Local);
    assert_eq!(agent.trust, PackageTrustKind::TrustedNativeCodeV1);
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(AGENT_PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace.id().to_string()).unwrap(),
    );
    let mount = open_mount(&alice_host, &key).await;
    let client = ConversationClient {
        host: &alice_host,
        mount: &mount,
    };
    let status = serde_json::to_value(
        client
            .request("load-status", "agent.status.get", object([]))
            .await,
    )
    .unwrap();
    assert!(status["state"].is_string(), "{status}");
    client
        .request("load-settings", "agent.settings.get", object([]))
        .await;

    let bob_core = open_core(Addressing::Loopback, &root.join("bob-core")).await;
    bob_core.create_workspace("No local Agent").unwrap();
    let bob_host = open_host(
        &app.application_path,
        root.join("bob-host"),
        bob_core.clone(),
    )
    .await;
    assert!(
        bob_host
            .snapshot()
            .await
            .packages
            .iter()
            .all(|package| package.identity.as_str() != AGENT_PACKAGE_ID)
    );
    shutdown(bob_host).await;
    shutdown(alice_host).await;
    bob_core.shutdown().await.unwrap();
    alice_core.shutdown().await.unwrap();
}

/// Verifies: AC2, AC3 — the Agent answers once when addressed, on both Installations, as itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn the_agent_replies_once_when_addressed_across_two_profiles() {
    let (endpoint, requests) = stub_endpoint().await;
    let alice_app = harness::StagedApplication::new();
    let root = alice_app.root.path();

    // Alice: Steward of the Agent, runs Conversations and the Agent Package.
    let alice_core = open_core(Addressing::Loopback, &root.join("alice-core")).await;
    let workspace = alice_core.create_workspace("Agent flow").unwrap();
    let workspace_id = workspace.id();
    alice_core
        .set_own_person_name(workspace_id, "Alice")
        .unwrap();
    let agent = alice_core.add_agent(workspace_id, "Scout").unwrap();
    let agent_key = alice_core
        .principal(workspace_id, agent)
        .unwrap()
        .unwrap()
        .keys()[0]
        .to_string();
    assert_ne!(agent_key, alice_core.public_identity().to_string());
    let alice_private = root.join("alice-host");
    register_agent(&alice_private);
    let alice_host = open_host(
        &alice_app.application_path,
        alice_private.clone(),
        alice_core.clone(),
    )
    .await;
    let conversations_key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let agent_package_key = PackageInstanceKey::new(
        PackageIdentity::from_str(AGENT_PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_mount = open_mount(&alice_host, &conversations_key).await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    alice
        .post_message("alice-history", GENERAL_ID, "@Scout this is history")
        .await;

    // The Steward configures the Agent Package through its own UI operations.
    let agent_mount = open_mount(&alice_host, &agent_package_key).await;
    let agent_ui = ConversationClient {
        host: &alice_host,
        mount: &agent_mount,
    };
    agent_ui
        .request(
            "agent-settings",
            "agent.settings.set",
            object([(
                "settings",
                object([
                    ("endpointBaseUrl", PackageValue::String(endpoint.clone())),
                    ("model", PackageValue::String("stub/model".into())),
                    (
                        "systemPrompt",
                        PackageValue::String("You are the team's scout.".into()),
                    ),
                    ("contextMessages", PackageValue::Unsigned(8)),
                ]),
            )]),
        )
        .await;
    agent_ui
        .request(
            "agent-key",
            "agent.key.set",
            object([("apiKey", PackageValue::String(KEY.into()))]),
        )
        .await;
    let candidates = serde_json::to_value(
        &agent_ui
            .request("agent-candidates", "agent.binding.candidates", object([]))
            .await,
    )
    .unwrap();
    assert_eq!(candidates["agents"].as_array().unwrap().len(), 1);
    assert_eq!(candidates["agents"][0]["principalId"], agent.to_string());
    assert_eq!(candidates["agents"][0]["name"], "Scout");
    agent_ui
        .request(
            "agent-bind",
            "agent.binding.set",
            object([("principalId", PackageValue::String(agent.to_string()))]),
        )
        .await;
    enable_conversations(&agent_ui).await;
    assert!(
        poll_until(Duration::from_secs(15), || async {
            let status = serde_json::to_value(
                &agent_ui
                    .request("agent-status", "agent.status.get", object([]))
                    .await,
            )
            .unwrap();
            assert!(!status.to_string().contains(KEY));
            status["state"] == "ready"
        })
        .await,
        "the Agent Package did not become ready"
    );

    // Bob: a second Installation with Conversations only.
    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(Addressing::Loopback, &root.join("bob-core")).await;
    introduce(Addressing::Loopback, &[&alice_core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob_app = harness::StagedApplication::new();
    let bob_host = open_host(
        &bob_app.application_path,
        root.join("bob-host"),
        bob_core.clone(),
    )
    .await;
    let bob_mount = open_mount(&bob_host, &conversations_key).await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    assert!(
        poll_until(Duration::from_secs(30), || async {
            general(&bob, "bob-history")
                .await
                .iter()
                .any(|row| row["markdown"] == "@Scout this is history")
        })
        .await,
        "Bob did not receive Alice's history"
    );

    // Bob addresses the Agent; the reply reaches both Installations as the Agent.
    bob.post_message("bob-mention", GENERAL_ID, "@Scout hello from Bob")
        .await;
    for (client, who) in [(&alice, "alice"), (&bob, "bob")] {
        assert!(
            poll_until(Duration::from_secs(30), || async {
                let rows = general(client, &format!("{who}-reply")).await;
                agent_rows(&rows, &agent_key)
                    .iter()
                    .any(|row| row["markdown"] == REPLY)
            })
            .await,
            "the Agent's reply did not reach {who}"
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // An unaddressed Message yields nothing, and the history mention was never answered.
    bob.post_message("bob-unaddressed", GENERAL_ID, "nothing for the agent here")
        .await;
    assert!(
        poll_until(Duration::from_secs(30), || async {
            general(&alice, "alice-unaddressed")
                .await
                .iter()
                .any(|row| row["markdown"] == "nothing for the agent here")
        })
        .await
    );
    // negative-wait: a reply would follow the Message by one completion round trip; a bounded
    // window after it arrived is the only way to show none came.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let rows = general(&alice, "alice-final").await;
    let replies = agent_rows(&rows, &agent_key);
    assert_eq!(replies.len(), 1, "{rows:?}");
    assert_eq!(replies[0]["markdown"], REPLY);
    assert_eq!(replies[0]["isLocal"], false);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let bob_rows = general(&bob, "bob-final").await;
    assert_eq!(agent_rows(&bob_rows, &agent_key).len(), 1);

    // Bob goes offline. Alice advances both display order and durable Agent progress.
    shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    alice
        .post_message(
            "offline-alice-one",
            GENERAL_ID,
            "Alice advances while Bob is offline",
        )
        .await;
    alice
        .post_message("offline-alice-two", GENERAL_ID, "Alice advances again")
        .await;
    let head =
        serde_json::to_value(alice.channel_open("advanced-head", GENERAL_ID, 64).await).unwrap();
    let head_revision = head["revision"].as_str().unwrap().parse::<u64>().unwrap();
    let alice_position = head["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["markdown"] == "Alice advances again")
        .unwrap()["position"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        poll_until(Duration::from_secs(15), || async {
            progress(&alice_private).is_some_and(|value| {
                value["channels"][GENERAL_ID]["after"]
                    .as_u64()
                    .is_some_and(|after| after >= head_revision)
            })
        })
        .await,
        "Agent did not acknowledge Alice's offline advances"
    );

    // A reopened loopback peer has no address-book introduction yet, so this append is
    // authored from Bob's older causal view using his real persisted Core and content key.
    let bob_core = open_core(Addressing::Loopback, &root.join("bob-core")).await;
    let bob_host = open_host(
        &bob_app.application_path,
        root.join("bob-host"),
        bob_core.clone(),
    )
    .await;
    let bob_mount = open_mount(&bob_host, &conversations_key).await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    bob.post_message(
        "offline-bob-mention",
        GENERAL_ID,
        "@Scout a late arrival from Bob",
    )
    .await;
    let bob_page = general(&bob, "offline-bob-page").await;
    let late_position = bob_page
        .iter()
        .find(|row| row["markdown"] == "@Scout a late arrival from Bob")
        .unwrap()["position"]
        .as_str()
        .unwrap();
    assert!(
        late_position < alice_position.as_str(),
        "fixture must deliver below the previously acknowledged display position"
    );
    introduce(Addressing::Loopback, &[&alice_core, &bob_core]);
    for (client, who) in [(&alice, "alice"), (&bob, "bob")] {
        assert!(
            poll_until(Duration::from_secs(30), || async {
                agent_rows(
                    &general(client, &format!("{who}-late-reply")).await,
                    &agent_key,
                )
                .len()
                    == 2
            })
            .await,
            "late lower-sort mention was not answered on {who}"
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 2);

    // Stop only the Agent harness, queue more than the former 256-message scan window in
    // the real Provider, then resume without sending another Message/notification.
    agent_ui
        .request("pause-agent-key", "agent.key.clear", object([]))
        .await;
    alice
        .post_message(
            "backlog-mention",
            GENERAL_ID,
            "@Scout oldest queued backlog mention",
        )
        .await;
    for index in 0..256 {
        alice
            .post_message(
                &format!("backlog-{index}"),
                GENERAL_ID,
                "ordinary queued backlog",
            )
            .await;
    }
    let backlog_head =
        serde_json::to_value(alice.channel_open("backlog-head", GENERAL_ID, 1).await).unwrap();
    let backlog_revision = backlog_head["revision"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    agent_ui
        .request(
            "resume-agent-key",
            "agent.key.set",
            object([("apiKey", PackageValue::String(KEY.into()))]),
        )
        .await;
    assert!(
        poll_until(Duration::from_secs(30), || async {
            progress(&alice_private).is_some_and(|value| {
                value["channels"][GENERAL_ID]["after"]
                    .as_u64()
                    .is_some_and(|after| after >= backlog_revision)
            }) && requests.load(Ordering::SeqCst) == 3
        })
        .await,
        "real Provider backlog was not fully acknowledged and answered"
    );
    for (client, who) in [(&alice, "alice"), (&bob, "bob")] {
        assert!(
            poll_until(Duration::from_secs(30), || async {
                let rows = general(client, &format!("{who}-backlog-reply")).await;
                let replies = agent_rows(&rows, &agent_key);
                replies.len() == 1 && replies[0]["markdown"] == REPLY
            })
            .await,
            "backlog reply did not reach {who}"
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 3);

    // The key stays in the Package Data Directory and nowhere else.
    assert!(!files_contain(&alice_private.join("packages/logs"), KEY));
    assert!(!files_contain(&root.join("alice-core"), KEY));

    shutdown(bob_host).await;
    shutdown(alice_host).await;
    bob_core.shutdown().await.unwrap();
    alice_core.shutdown().await.unwrap();
}

/// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.CONV-R18
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn authorized_reply_keeps_old_actor_during_rebind() {
    reply_at_rebind_boundary(ReplyBoundary::Authorized).await;
}

/// A reply not yet authorized must not acquire the replacement Agent's identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn cancelled_reply_before_authorization_never_posts_as_new_agent() {
    reply_at_rebind_boundary(ReplyBoundary::Preauthorization).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn uncertain_reply_requires_full_host_retirement_before_rebinding() {
    reply_at_rebind_boundary(ReplyBoundary::UncertainRecovery).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn disabled_pending_reply_is_refused_without_replay_after_rebind() {
    reply_at_rebind_boundary(ReplyBoundary::DisabledCancellation).await;
}

#[derive(PartialEq)]
enum ReplyBoundary {
    Preauthorization,
    Authorized,
    UncertainRecovery,
    DisabledCancellation,
}

async fn reply_at_rebind_boundary(boundary: ReplyBoundary) {
    let (endpoint, mut model_requests, requests) = controlled_model().await;
    let app = harness::StagedApplication::new();
    let core = open_core(Addressing::Loopback, &app.root.path().join("core")).await;
    let workspace = core.create_workspace("Actor fence").unwrap();
    let a = core.add_agent(workspace.id(), "Scout").unwrap();
    let b = core.add_agent(workspace.id(), "Other").unwrap();
    let a_key = core.principal(workspace.id(), a).unwrap().unwrap().keys()[0].to_string();
    let b_key = core.principal(workspace.id(), b).unwrap().unwrap().keys()[0].to_string();
    let private = app.root.path().join("host");
    register_agent(&private);
    let host = open_host(&app.application_path, private.clone(), core.clone()).await;
    let key = |package: &str| {
        PackageInstanceKey::new(
            package.parse().unwrap(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        )
    };
    let mount = open_mount(&host, &key(PACKAGE_ID)).await;
    let conversations = ConversationClient {
        host: &host,
        mount: &mount,
    };
    let agent_mount = open_mount(&host, &key(AGENT_PACKAGE_ID)).await;
    let agent = ConversationClient {
        host: &host,
        mount: &agent_mount,
    };
    agent
        .request(
            "settings",
            "agent.settings.set",
            object([(
                "settings",
                object([
                    ("endpointBaseUrl", PackageValue::String(endpoint)),
                    ("model", PackageValue::String("stub/model".into())),
                    ("systemPrompt", PackageValue::String(String::new())),
                    ("contextMessages", PackageValue::Unsigned(0)),
                ]),
            )]),
        )
        .await;
    agent
        .request(
            "key",
            "agent.key.set",
            object([("apiKey", PackageValue::String(KEY.into()))]),
        )
        .await;
    agent
        .request(
            "bind-a",
            "agent.binding.set",
            object([("principalId", PackageValue::String(a.to_string()))]),
        )
        .await;
    enable_conversations(&agent).await;
    // Wait for this Channel's baseline, not just creation of the progress file.
    assert!(
        poll_until(Duration::from_secs(15), || async {
            progress(app.root.path()).is_some_and(|value| value["channels"][GENERAL_ID].is_object())
        })
        .await
    );
    conversations
        .post_message("mention-a", GENERAL_ID, "@Scout hello")
        .await;
    // Metadata and reply context precede this held model response. The only next
    // command is the harness's final message.post (withheld from model tools).
    let final_text = model_request(&mut model_requests).await;
    let mut observations = host.observe_ports();
    let pause = if boundary == ReplyBoundary::Authorized {
        host.pause_next_command_authorized_for_tests(AGENT_PACKAGE_ID.parse().unwrap())
    } else {
        host.pause_next_command_actor_for_tests(AGENT_PACKAGE_ID.parse().unwrap())
    };
    let _release = pause.resume_on_drop();
    final_text
        .answer
        .send(json!({"role":"assistant","content":REPLY}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), pause.paused())
        .await
        .expect("final command reached the selected Host boundary");
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(
        !general(&conversations, "before-provider-dispatch")
            .await
            .iter()
            .any(|row| row["markdown"] == REPLY)
    );
    if boundary == ReplyBoundary::DisabledCancellation {
        // Cancel the real reply/harness awaiter, not the independently owned command.
        agent
            .request(
                "disable-held-reply",
                "agent.packages.set",
                object([
                    ("contract", PackageValue::String(PACKAGE_ID.into())),
                    ("enabled", PackageValue::Boolean(false)),
                ]),
            )
            .await;
    }
    binding_refused(&agent, Some(b.to_string()), "agent.binding_busy").await;
    binding_refused(&agent, None, "agent.binding_busy").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a),
        "busy must leave actual Core binding A"
    );
    assert!(agent_rows(&general(&conversations, "held").await, &b_key).is_empty());
    println!("timeline: held command; bind/unbind busy; actual Core remains A");
    if boundary == ReplyBoundary::UncertainRecovery {
        // Do not change SDK/Host deadlines. Observe the actual post terminal while held.
        assert!(
            poll_until(Duration::from_secs(15), || async {
                let status = serde_json::to_value(
                    agent
                        .request("timeout-status", "agent.status.get", object([]))
                        .await,
                )
                .unwrap();
                status["state"] == "error" && status["lastError"]["code"] == "agent.post_failed"
            })
            .await,
            "real held post did not time out"
        );
        binding_refused(&agent, Some(b.to_string()), "agent.binding_uncertain").await;
        binding_refused(&agent, None, "agent.binding_uncertain").await;
        assert_eq!(
            core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
            Some(a)
        );
        println!(
            "timeline: actual deadline terminal while preauthorization held; binding_uncertain; Core A"
        );
        use teamarness_package_host::{RecoverPackage, SuspendPackage};
        host.execute(PackageHostCommand::Suspend(SuspendPackage {
            instance: key(AGENT_PACKAGE_ID),
        }))
        .await
        .unwrap();
        host.execute(PackageHostCommand::Recover(RecoverPackage {
            instance: key(AGENT_PACKAGE_ID),
        }))
        .await
        .unwrap();
        binding_refused(&agent, Some(b.to_string()), "agent.binding_uncertain").await;
        println!("timeline: Suspend/Recover did not clear uncertainty");
        // Keep this old Host and its pause alive across full supported retirement.
        tokio::time::timeout(Duration::from_secs(30), shutdown(host.clone()))
            .await
            .expect("Host retirement must complete without releasing old work");
        println!("timeline: whole Host shutdown complete; old preauthorization pause still held");
        let replacement = open_host(&app.application_path, private, core.clone()).await;
        let new_mount = open_mount(&replacement, &key(AGENT_PACKAGE_ID)).await;
        assert_ne!(
            new_mount, agent_mount,
            "replacement needs a fresh public mount"
        );
        let fresh = ConversationClient {
            host: &replacement,
            mount: &new_mount,
        };
        let saved = serde_json::to_value(
            fresh
                .request("retained", "agent.packages.get", object([]))
                .await,
        )
        .unwrap();
        let selected = saved["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["contract"] == PACKAGE_ID)
            .unwrap();
        assert_eq!(selected["enabled"], true);
        assert_eq!(selected["enabledMajor"], selected["major"]);
        fresh
            .request(
                "fresh-bind-b",
                "agent.binding.set",
                object([("principalId", PackageValue::String(b.to_string()))]),
            )
            .await;
        assert_eq!(
            core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
            Some(b)
        );
        println!(
            "timeline: reopened same store/Core; new mount; retained selection; Core B; releasing OLD pause"
        );
        pause.resume();
        let new_conversations_mount = open_mount(&replacement, &key(PACKAGE_ID)).await;
        let new_conversations = ConversationClient {
            host: &replacement,
            mount: &new_conversations_mount,
        };
        let rows = general(&new_conversations, "old-work-after-retirement").await;
        assert!(
            !rows
                .iter()
                .any(|row| row["markdown"] == REPLY && row["author"] == b_key),
            "old work authorized as B: {rows:?}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "old trigger replayed on reopen"
        );
        new_conversations
            .post_message("fresh-mention", GENERAL_ID, "@Other fresh work")
            .await;
        let fresh_reply = model_request(&mut model_requests).await;
        assert!(fresh_reply.body.to_string().contains("@Other fresh work"));
        fresh_reply
            .answer
            .send(json!({"role":"assistant","content":REPLY}))
            .unwrap();
        assert!(
            poll_until(Duration::from_secs(15), || async {
                agent_rows(&general(&new_conversations, "fresh-b-reply").await, &b_key).len() == 1
            })
            .await,
            "fresh B work did not succeed"
        );
        let rows = general(&new_conversations, "fresh-final").await;
        assert_eq!(agent_rows(&rows, &b_key).len(), 1);
        assert_eq!(agent_rows(&rows, &b_key)[0]["markdown"], REPLY);
        assert_eq!(agent_rows(&rows, &b_key)[0]["isLocal"], false);
        assert!(agent_rows(&rows, &a_key).is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        println!("timeline: no stale B effect/replay; fresh B signed reply exactly once");
        shutdown(replacement).await;
        core.shutdown().await.unwrap();
        return;
    }
    pause.resume();
    if boundary == ReplyBoundary::DisabledCancellation {
        let refusal = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let event = observations.recv().await.unwrap();
                if event.consumer_package == AGENT_PACKAGE_ID
                    && event.kind == "command"
                    && event.stage == teamarness_package_host::PortStage::Rejected
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        println!("disabled final actual terminal: {refusal:?}");
        let status = serde_json::to_value(
            agent
                .request("disabled-terminal-status", "agent.status.get", object([]))
                .await,
        )
        .unwrap();
        println!(
            "disabled final actual Agent status: {status}; Core: {:?}",
            core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap()
        );
        public_host_state(&host, "disabled-final-terminal").await;
        assert_eq!(refusal.code, Some("port_not_enabled"));
        assert!(!refusal.outcome_uncertain);
        assert!(refusal.provider_id.is_none());
        assert!(
            !general(&conversations, "disabled-no-effect")
                .await
                .iter()
                .any(|row| row["markdown"] == REPLY)
        );
    } else {
        // Ready follows the actual final-post result, not just Provider writer acceptance.
        assert!(
            poll_until(Duration::from_secs(15), || async {
                let status = serde_json::to_value(
                    agent
                        .request("settled", "agent.status.get", object([]))
                        .await,
                )
                .unwrap();
                status["state"] == "ready"
                    && general(&conversations, "reply")
                        .await
                        .iter()
                        .any(|row| row["markdown"] == REPLY)
            })
            .await,
            "owned post did not reach a certain terminal"
        );
        let rows = general(&conversations, "attribution").await;
        let replies: Vec<_> = rows.iter().filter(|row| row["markdown"] == REPLY).collect();
        assert_eq!(replies.len(), 1, "command replayed: {rows:?}");
        assert_eq!(replies[0]["author"], a_key, "old reply lost A attribution");
        assert_eq!(replies[0]["isLocal"], false);
    }
    if boundary == ReplyBoundary::DisabledCancellation {
        assert!(
            poll_until(Duration::from_secs(15), || async {
                let result = host
                    .execute(PackageHostCommand::UiRequest(UiRequest {
                        mount: agent_mount.clone(),
                        request: UiRequestId::new(format!(
                            "bind-after-refusal-{}",
                            NEXT_BINDING_REQUEST.fetch_add(1, Ordering::Relaxed)
                        ))
                        .unwrap(),
                        operation: "agent.binding.set".into(),
                        payload: object([("principalId", PackageValue::String(b.to_string()))]),
                    }))
                    .await;
                match result {
                    Ok(_) => true,
                    Err(error) => {
                        let owned = error
                            .package_error
                            .as_ref()
                            .expect("binding must return a specific Agent-owned admission refusal");
                        assert_eq!(owned.code, "agent.binding_busy", "{error:?}");
                        assert!(owned.retryable);
                        assert_eq!(
                            core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
                            Some(a)
                        );
                        false
                    }
                }
            })
            .await,
            "owner did not settle after certain authorization refusal"
        );
    } else {
        agent
            .request(
                "bind-b-after-terminal",
                "agent.binding.set",
                object([("principalId", PackageValue::String(b.to_string()))]),
            )
            .await;
    }
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(b)
    );
    if boundary == ReplyBoundary::DisabledCancellation {
        enable_conversations(&agent).await;
    }
    // The completed binding and status round trip are the public reconciliation barrier.
    agent
        .request("bound-status", "agent.status.get", object([]))
        .await;
    let rows = general(&conversations, "after-bind").await;
    assert_eq!(
        rows.iter().filter(|row| row["markdown"] == REPLY).count(),
        usize::from(boundary != ReplyBoundary::DisabledCancellation)
    );
    assert!(
        agent_rows(&rows, &b_key).is_empty(),
        "A text posted as B: {rows:?}"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "trigger replayed after rebind"
    );
    conversations
        .post_message("fresh-b-mention", GENERAL_ID, "@Other fresh work")
        .await;
    let fresh_reply = model_request(&mut model_requests).await;
    assert!(fresh_reply.body.to_string().contains("@Other fresh work"));
    fresh_reply
        .answer
        .send(json!({"role":"assistant","content":"fresh B result"}))
        .unwrap();
    assert!(
        poll_until(Duration::from_secs(15), || async {
            let rows = general(&conversations, "fresh-b-result").await;
            agent_rows(&rows, &b_key).len() == 1
        })
        .await
    );
    let rows = general(&conversations, "all-final-replies").await;
    let old_rows = agent_rows(&rows, &a_key);
    assert_eq!(
        old_rows.len(),
        usize::from(boundary != ReplyBoundary::DisabledCancellation)
    );
    if let Some(row) = old_rows.first() {
        assert_eq!(row["markdown"], REPLY);
    }
    assert_eq!(agent_rows(&rows, &b_key).len(), 1);
    assert_eq!(agent_rows(&rows, &b_key)[0]["markdown"], "fresh B result");
    assert_eq!(agent_rows(&rows, &b_key)[0]["isLocal"], false);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    println!(
        "timeline: certain terminal; signed A exactly once; B bound; fresh signed B; no replay"
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

/// Verifies: TEAM.SYS.PKG-R13
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn cancelled_binding_waiter_cannot_reopen_dispatch_early() {
    use teamarness_package_host::{CancelUiRequest, PackageHostCommand, UiRequest, UiRequestId};
    let (endpoint, mut model_requests, model_count) = controlled_model().await;
    let app = harness::StagedApplication::new();
    stage_qualification_provider(&app.application_path);
    let core = open_core(Addressing::Loopback, &app.root.path().join("core")).await;
    let workspace = core.create_workspace("Binding owner").unwrap();
    let a = core.add_agent(workspace.id(), "Scout").unwrap();
    let b = core.add_agent(workspace.id(), "Other").unwrap();
    let private = app.root.path().join("host");
    register_agent(&private);
    let host = open_host(&app.application_path, private.clone(), core.clone()).await;
    let key = PackageInstanceKey::new(
        AGENT_PACKAGE_ID.parse().unwrap(),
        WorkspaceId::new(workspace.id().to_string()).unwrap(),
    );
    let mount = open_mount(&host, &key).await;
    let agent = ConversationClient {
        host: &host,
        mount: &mount,
    };
    configure_loopback_model(&agent, endpoint).await;
    agent
        .request(
            "bind-a",
            "agent.binding.set",
            object([("principalId", PackageValue::String(a.to_string()))]),
        )
        .await;
    select_qualification(&agent).await;
    let provider_key = PackageInstanceKey::new(
        QUALIFICATION.parse().unwrap(),
        WorkspaceId::new(workspace.id().to_string()).unwrap(),
    );
    let provider_mount = open_mount(&host, &provider_key).await;
    let provider = ConversationClient {
        host: &host,
        mount: &provider_mount,
    };
    chat_send(&agent, "held A intent before binding transition").await;
    let late_a = model_request(&mut model_requests).await;
    assert!(late_a.body["tools"].is_array());
    let pause = host.pause_next_binding_write_for_tests(AGENT_PACKAGE_ID.parse().unwrap());
    let _release = pause.resume_on_drop();
    let binding_host = host.clone();
    let binding_mount = mount.clone();
    let pending = tokio::spawn(async move {
        binding_host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: binding_mount,
                request: UiRequestId::new("cancel-bind-b").unwrap(),
                operation: "agent.binding.set".into(),
                payload: object([("principalId", PackageValue::String(b.to_string()))]),
            }))
            .await
    });
    tokio::time::timeout(Duration::from_secs(15), pause.paused())
        .await
        .unwrap();
    host.execute(PackageHostCommand::CancelUiRequest(CancelUiRequest {
        mount: mount.clone(),
        request: UiRequestId::new("cancel-bind-b").unwrap(),
    }))
    .await
    .unwrap();
    assert!(pending.await.unwrap().is_err());
    // A round trip through the SDK reader ensures the preceding cancel has been handled.
    agent
        .request("after-cancel", "agent.status.get", object([]))
        .await;
    binding_refused(&agent, None, "agent.binding_busy").await;
    binding_refused(&agent, Some(a.to_string()), "agent.binding_busy").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a),
        "cancelled waiter must leave Core work owned and withheld"
    );
    let interrupted = serde_json::to_value(
        agent
            .request("held-chat-view", "agent.chat.get", object([]))
            .await,
    )
    .unwrap();
    assert_eq!(interrupted["pending"], false);
    assert_eq!(interrupted["lastError"]["code"], "agent.chat_interrupted");
    assert!(provider_rows(&provider).await.is_empty());
    pause.resume();
    assert!(
        poll_until(Duration::from_secs(15), || async {
            let status = serde_json::to_value(
                agent
                    .request("binding-settled", "agent.status.get", object([]))
                    .await,
            )
            .unwrap();
            status["agent"]["principalId"] == b.to_string()
                && core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap() == Some(b)
        })
        .await,
        "owned mutation did not reconcile actual Core B after UI cancellation"
    );
    agent
        .request(
            "disable-before-independent",
            "agent.packages.set",
            object([
                ("contract", PackageValue::String(QUALIFICATION.into())),
                ("enabled", PackageValue::Boolean(false)),
            ]),
        )
        .await;
    let all_off = serde_json::to_value(
        agent
            .request("all-off", "agent.packages.get", object([]))
            .await,
    )
    .unwrap();
    assert!(
        all_off["packages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["enabled"] == false)
    );
    chat_send(&agent, "independent chat after binding terminal").await;
    let direct = model_request(&mut model_requests).await;
    assert!(direct.body.get("tools").is_none());
    let mut text_with_unsolicited_call = tool_call(
        "qualification__sample_write",
        json!({"value":"must-not-dispatch"}),
    );
    text_with_unsolicited_call["content"] = json!("independent after binding");
    direct.answer.send(text_with_unsolicited_call).unwrap();
    chat_idle(&agent).await;
    let chat = serde_json::to_value(
        agent
            .request("independent-result", "agent.chat.get", object([]))
            .await,
    )
    .unwrap();
    assert_eq!(
        chat["entries"].as_array().unwrap().last().unwrap()["content"],
        "independent after binding"
    );
    assert!(provider_rows(&provider).await.is_empty());
    assert_eq!(model_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(b)
    );
    select_qualification(&agent).await;
    late_a
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"late-A"}),
        ))
        .unwrap();
    chat_send(&agent, "fresh B after cancelled binding waiter").await;
    model_request(&mut model_requests)
        .await
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"fresh-B"}),
        ))
        .unwrap();
    let terminal = model_request(&mut model_requests).await;
    let b_key = core.principal(workspace.id(), b).unwrap().unwrap().keys()[0].to_string();
    let expected = json!({"value":"fresh-B","actor":{"kind":"agent","principal":b.to_string(),"identity":b_key}});
    assert_eq!(tool_result(&terminal.body), expected);
    terminal
        .answer
        .send(json!({"role":"assistant","content":"B done"}))
        .unwrap();
    chat_idle(&agent).await;
    assert_eq!(provider_rows(&provider).await, vec![expected]);
    assert_eq!(
        model_count.load(Ordering::SeqCst),
        4,
        "late A intent or unsolicited tool replayed"
    );
    agent
        .request("clear-after-terminal", "agent.binding.clear", object([]))
        .await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        None
    );
    let status = serde_json::to_value(
        agent
            .request("final-status", "agent.status.get", object([]))
            .await,
    )
    .unwrap();
    assert!(status["agent"].is_null());
    println!(
        "binding timeline: actual UI waiter cancelled; bind/unbind busy; Core A while held; owned Core B after release; subsequent clear succeeds"
    );
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

async fn public_host_state(host: &teamarness_package_host::PackageHost, label: &str) {
    let snapshot = host.snapshot().await;
    let diagnostics: Vec<_> = snapshot.diagnostics.iter().map(|error| json!({"code":error.code,"stage":error.stage,"package":error.affected_package,"instance":error.affected_instance})).collect();
    eprintln!(
        "public Host state {label}: {}",
        json!({"state":snapshot.state,"instances":snapshot.instances,"diagnostics":diagnostics})
    );
}

const QUALIFICATION: &str = "dev.teamarness.qualification";

/// This contract is assembled only inside a disposable test application.
fn stage_qualification_provider(application: &Path) {
    use sha2::{Digest, Sha256};
    let digest = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    let root = application.join(QUALIFICATION);
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let binary = fs::read(env!(
        "CARGO_BIN_EXE_agent-qualification-provider-under-test"
    ))
    .unwrap();
    let ui = b"export function mount() { return () => {}; }";
    let ui_contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{
        "proof.rows":{"request":{"type":"object","additionalProperties":false},"response":{"type":"object"}}
    },"subscriptions":{}})).unwrap();
    let ports = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{
        "offset":{"type":"integer","minimum":7,"maximum":7},
        "value":{"type":"string","minLength":1,"maxLength":64}
    },"contracts":[{"identity":QUALIFICATION,"major":3,"minor":0,
        "description":"Test-only independent sample query and actor-observed writes.","ports":[
        {"name":"runtime.guide","kind":"query","description":"Read ordinary operational instructions, never schemas.",
            "request":{"type":"object","additionalProperties":false},"response":{"type":"object","required":["markdown"],"properties":{"markdown":{"type":"string","maxLength":32768}},"additionalProperties":false}},
        {"name":"sample.read","kind":"query","description":"Add thirty-five to the supplied offset.",
            "request":{"type":"object","required":["offset"],"properties":{"offset":{"$ref":"#/$defs/offset"}},"additionalProperties":false},"response":{"type":"object"}},
        {"name":"sample.write","kind":"command","description":"Append a value once with the actual Host-authored actor.",
            "request":{"type":"object","required":["value"],"properties":{"value":{"$ref":"#/$defs/value"}},"additionalProperties":false},"response":{"type":"object"}}
    ]}]})).unwrap();
    fs::write(root.join("backend/provider"), &binary).unwrap();
    fs::set_permissions(
        root.join("backend/provider"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui-contract.json"), &ui_contract).unwrap();
    fs::write(root.join("ports.json"), &ports).unwrap();
    let manifest = json!({"schemaVersion":1,"identity":QUALIFICATION,"version":"1.0.0",
        "presentation":{"displayName":"Qualification"},"protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":"backend/provider","size":binary.len(),"sha256":digest(&binary)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],"contract":{"path":"ui-contract.json","sha256":digest(&ui_contract)}},
        "portContract":{"path":"ports.json","sha256":digest(&ports)},"capabilities":[]});
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let path = application.join("application-packages.v1.json");
    let mut registrations: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    registrations["packages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"directory":QUALIFICATION}));
    fs::write(path, serde_json::to_vec(&registrations).unwrap()).unwrap();
    println!(
        "fixture: source-built qualification provider backend sha256={} ports sha256={}",
        digest(&binary),
        digest(&ports)
    );
}

struct ModelRequest {
    body: Value,
    answer: tokio::sync::oneshot::Sender<Value>,
}

/// Only the external model is scripted. Every metadata/Port/actor/result path is real.
async fn controlled_model() -> (
    String,
    tokio::sync::mpsc::Receiver<ModelRequest>,
    Arc<AtomicUsize>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let sender = sender.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0_u8; 4096];
                let header_end = loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let head = String::from_utf8_lossy(&bytes[..header_end]);
                assert!(head.contains(&format!("Bearer {KEY}")));
                let length = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                while bytes.len() < header_end + length {
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let body = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                let (answer, response) = tokio::sync::oneshot::channel();
                sender.send(ModelRequest { body, answer }).await.unwrap();
                let Ok(message) = response.await else {
                    return;
                };
                let body = json!({"choices":[{"message":message}]}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (endpoint, receiver, requests)
}

async fn model_request(requests: &mut tokio::sync::mpsc::Receiver<ModelRequest>) -> ModelRequest {
    tokio::time::timeout(Duration::from_secs(15), requests.recv())
        .await
        .expect("model request deadline")
        .unwrap()
}

fn tool_call(name: &str, arguments: Value) -> Value {
    json!({"role":"assistant","content":null,"tool_calls":[{"id":"qualification-call","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]})
}

fn tool_result(body: &Value) -> Value {
    let message = body["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(message["role"], "tool");
    serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
}

async fn chat_send(agent: &ConversationClient<'_>, content: &str) {
    agent
        .request(
            "chat",
            "agent.chat.send",
            object([("content", PackageValue::String(content.into()))]),
        )
        .await;
}

async fn chat_idle(agent: &ConversationClient<'_>) {
    assert!(
        poll_until(Duration::from_secs(15), || async {
            let chat = serde_json::to_value(
                agent
                    .request("chat-status", "agent.chat.get", object([]))
                    .await,
            )
            .unwrap();
            chat["pending"] == false
        })
        .await,
        "chat did not settle"
    );
}

async fn provider_rows(provider: &ConversationClient<'_>) -> Vec<Value> {
    serde_json::to_value(provider.request("rows", "proof.rows", object([])).await).unwrap()["rows"]
        .as_array()
        .unwrap()
        .clone()
}

/// Generic discovery, exact schemas, query/command actor results and abandoned actual commands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn unknown_provider_model_tools_retain_owned_commands_and_authorized_actor() {
    let (endpoint, mut requests, model_count) = controlled_model().await;
    let app = harness::StagedApplication::new();
    stage_qualification_provider(&app.application_path);
    let core = open_core(Addressing::Loopback, &app.root.path().join("core")).await;
    let workspace = core.create_workspace("Unknown Provider").unwrap();
    let a = core.add_agent(workspace.id(), "Scout").unwrap();
    let b = core.add_agent(workspace.id(), "Other").unwrap();
    let a_key = core.principal(workspace.id(), a).unwrap().unwrap().keys()[0].to_string();
    let b_key = core.principal(workspace.id(), b).unwrap().unwrap().keys()[0].to_string();
    let private = app.root.path().join("host");
    register_agent(&private);
    let host = open_host(&app.application_path, private, core.clone()).await;
    let mut observations = host.observe_ports();
    let key = |id: &str| {
        PackageInstanceKey::new(
            id.parse().unwrap(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        )
    };
    let mount = open_mount(&host, &key(AGENT_PACKAGE_ID)).await;
    let agent = ConversationClient {
        host: &host,
        mount: &mount,
    };
    let provider_mount = open_mount(&host, &key(QUALIFICATION)).await;
    let provider = ConversationClient {
        host: &host,
        mount: &provider_mount,
    };
    agent
        .request(
            "settings",
            "agent.settings.set",
            object([(
                "settings",
                object([
                    ("endpointBaseUrl", PackageValue::String(endpoint)),
                    ("model", PackageValue::String("stub/model".into())),
                    ("systemPrompt", PackageValue::String(String::new())),
                    ("contextMessages", PackageValue::Unsigned(0)),
                ]),
            )]),
        )
        .await;
    agent
        .request(
            "key",
            "agent.key.set",
            object([("apiKey", PackageValue::String(KEY.into()))]),
        )
        .await;
    // Direct chat is independent before binding or selection.
    chat_send(&agent, "independent chat").await;
    let direct = model_request(&mut requests).await;
    assert!(direct.body.get("tools").is_none());
    direct
        .answer
        .send(json!({"role":"assistant","content":"direct"}))
        .unwrap();
    chat_idle(&agent).await;
    agent
        .request(
            "bind-a",
            "agent.binding.set",
            object([("principalId", PackageValue::String(a.to_string()))]),
        )
        .await;
    let offered = serde_json::to_value(
        agent
            .request("catalog", "agent.packages.get", object([]))
            .await,
    )
    .unwrap();
    let row = offered["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(row["major"], 3);
    assert_eq!(row["enabled"], false);
    let conversations = offered["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["contract"] == PACKAGE_ID)
        .unwrap();
    assert_eq!(
        conversations["enabled"], false,
        "built-in participation must remain opt-in"
    );
    agent
        .request(
            "enable",
            "agent.packages.set",
            object([
                ("contract", PackageValue::String(QUALIFICATION.into())),
                ("enabled", PackageValue::Boolean(true)),
                (
                    "major",
                    PackageValue::Unsigned(row["major"].as_u64().unwrap()),
                ),
            ]),
        )
        .await;
    chat_send(&agent, "read guide, query, then write").await;
    let guide = model_request(&mut requests).await;
    let functions: Vec<_> = guide.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| &tool["function"])
        .collect();
    let read = functions
        .iter()
        .find(|tool| tool["name"] == "qualification__sample_read")
        .unwrap();
    assert_eq!(
        read["parameters"],
        json!({"type":"object","required":["offset"],"properties":{"offset":{"type":"integer","minimum":7,"maximum":7}},"additionalProperties":false})
    );
    assert!(
        functions
            .iter()
            .any(|tool| tool["name"] == "qualification__sample_write")
    );
    assert!(!functions.iter().any(|tool| {
        tool["name"]
            .as_str()
            .unwrap()
            .starts_with("conversations__")
    }));
    guide
        .answer
        .send(tool_call("qualification__runtime_guide", json!({})))
        .unwrap();
    let query = model_request(&mut requests).await;
    assert!(
        tool_result(&query.body)["markdown"]
            .as_str()
            .unwrap()
            .contains("Use the tool schemas")
    );
    query
        .answer
        .send(tool_call("qualification__sample_read", json!({"offset":7})))
        .unwrap();
    let command = model_request(&mut requests).await;
    assert_eq!(
        tool_result(&command.body),
        json!({"answer":42,"actor":{"kind":"agent","principal":a.to_string(),"identity":a_key}})
    );
    // No Conversations grant, so there are no background Port queries/subscriptions. Metadata
    // is complete before the held model response. Its sole next operation is this command.
    let authorized =
        host.pause_next_command_authorized_for_tests(AGENT_PACKAGE_ID.parse().unwrap());
    let _release = authorized.resume_on_drop();
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"authorized-A"}),
        ))
        .unwrap();
    authorized.paused().await;
    binding_refused(&agent, Some(b.to_string()), "agent.binding_busy").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a)
    );
    assert!(provider_rows(&provider).await.is_empty());
    authorized.resume();
    let final_text = model_request(&mut requests).await;
    assert_eq!(
        tool_result(&final_text.body),
        json!({"value":"authorized-A","actor":{"kind":"agent","principal":a.to_string(),"identity":a_key}})
    );
    final_text
        .answer
        .send(json!({"role":"assistant","content":"verified result 42"}))
        .unwrap();
    chat_idle(&agent).await;
    println!(
        "generic timeline: unknown major3 metadata -> guide -> query42 -> precise authorized command -> actual Provider A"
    );

    // Abort the actual chat/outer turn while its SDK command is held before authorization.
    chat_send(&agent, "cancel owned write").await;
    let command = model_request(&mut requests).await;
    let pause = host.pause_next_command_actor_for_tests(AGENT_PACKAGE_ID.parse().unwrap());
    let _release_pending = pause.resume_on_drop();
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"cancelled-A"}),
        ))
        .unwrap();
    pause.paused().await;
    public_host_state(&host, "before-chat-clear").await;
    let cleared = agent
        .request("cancel-chat", "agent.chat.clear", object([]))
        .await;
    eprintln!(
        "actual chat.clear result: {}",
        serde_json::to_value(cleared).unwrap()
    );
    public_host_state(&host, "after-chat-clear").await;
    binding_refused(&agent, Some(b.to_string()), "agent.binding_busy").await;
    binding_refused(&agent, None, "agent.binding_busy").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a)
    );
    pause.resume();
    assert!(
        poll_until(Duration::from_secs(15), || async {
            provider_rows(&provider).await.len() == 2
        })
        .await
    );
    let rows = provider_rows(&provider).await;
    assert_eq!(
        rows[1],
        json!({"value":"cancelled-A","actor":{"kind":"agent","principal":a.to_string(),"identity":a_key}})
    );
    // A subsequent successful model turn proves owner settlement before the next identity choice.
    chat_send(&agent, "ordinary command refusal").await;
    let command = model_request(&mut requests).await;
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"certain-refusal"}),
        ))
        .unwrap();
    let terminal = model_request(&mut requests).await;
    eprintln!(
        "ordinary refusal actual complete tool error envelope: {}",
        tool_result(&terminal.body)
    );
    while let Ok(event) = observations.try_recv() {
        if event.consumer_package == AGENT_PACKAGE_ID
            && event.contract.as_deref() == Some(QUALIFICATION)
            && event.port.as_deref() == Some("sample.write")
        {
            eprintln!("public ordinary command observation: {event:?}");
        }
    }
    assert_eq!(
        tool_result(&terminal.body),
        json!({"error":"provider_error","retryable":false,"details":{"provider":{"code":"qualification.refused","retryable":false,"details":{}}}})
    );
    terminal
        .answer
        .send(json!({"role":"assistant","content":"certain refusal observed"}))
        .unwrap();
    chat_idle(&agent).await;
    // This A tool snapshot is held at the model boundary, before command admission.
    chat_send(&agent, "late A intent must not enqueue after B").await;
    let late_a = model_request(&mut requests).await;
    assert!(late_a.body["tools"].is_array());
    agent
        .request(
            "bind-b",
            "agent.binding.set",
            object([("principalId", PackageValue::String(b.to_string()))]),
        )
        .await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(b)
    );
    chat_idle(&agent).await;
    late_a
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"late-A"}),
        ))
        .unwrap();
    chat_send(&agent, "fresh B write").await;
    let command = model_request(&mut requests).await;
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"fresh-B"}),
        ))
        .unwrap();
    let terminal = model_request(&mut requests).await;
    assert_eq!(tool_result(&terminal.body)["actor"]["identity"], b_key);
    terminal
        .answer
        .send(json!({"role":"assistant","content":"B done"}))
        .unwrap();
    chat_idle(&agent).await;
    let rows = provider_rows(&provider).await;
    assert_eq!(rows.len(), 3, "no replay of cancelled or refused command");
    assert_eq!(rows[2]["value"], "fresh-B");
    assert_eq!(rows[2]["actor"]["identity"], b_key);
    println!(
        "generic timeline: cancelled outer chat retains actual A command; certain refusal releases; B work succeeds without replay"
    );
    agent
        .request(
            "disable",
            "agent.packages.set",
            object([
                ("contract", PackageValue::String(QUALIFICATION.into())),
                ("enabled", PackageValue::Boolean(false)),
            ]),
        )
        .await;
    let before_disabled = model_count.load(Ordering::SeqCst);
    chat_send(&agent, "disabled direct chat").await;
    let disabled = model_request(&mut requests).await;
    assert!(disabled.body.get("tools").is_none());
    disabled
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"disabled"}),
        ))
        .unwrap();
    chat_idle(&agent).await;
    let refused = serde_json::to_value(
        agent
            .request("disabled-refusal", "agent.chat.get", object([]))
            .await,
    )
    .unwrap();
    assert_eq!(refused["lastError"]["code"], "agent.model_error");
    assert_eq!(model_count.load(Ordering::SeqCst), before_disabled + 1);
    assert_eq!(provider_rows(&provider).await, rows);
    chat_send(&agent, "independent after disabled refusal").await;
    model_request(&mut requests)
        .await
        .answer
        .send(json!({"role":"assistant","content":"independent"}))
        .unwrap();
    chat_idle(&agent).await;
    assert_eq!(provider_rows(&provider).await, rows);
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

async fn configure_loopback_model(agent: &ConversationClient<'_>, endpoint: String) {
    agent
        .request(
            "model-settings",
            "agent.settings.set",
            object([(
                "settings",
                object([
                    ("endpointBaseUrl", PackageValue::String(endpoint)),
                    ("model", PackageValue::String("stub/model".into())),
                    ("systemPrompt", PackageValue::String(String::new())),
                    ("contextMessages", PackageValue::Unsigned(0)),
                ]),
            )]),
        )
        .await;
    agent
        .request(
            "model-key",
            "agent.key.set",
            object([("apiKey", PackageValue::String(KEY.into()))]),
        )
        .await;
}

async fn select_qualification(agent: &ConversationClient<'_>) {
    let offered = serde_json::to_value(
        agent
            .request("qualification-offer", "agent.packages.get", object([]))
            .await,
    )
    .unwrap();
    let row = offered["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(row["major"], 3);
    assert_eq!(row["enabled"], false);
    agent
        .request(
            "qualification-enable",
            "agent.packages.set",
            object([
                ("contract", PackageValue::String(QUALIFICATION.into())),
                ("enabled", PackageValue::Boolean(true)),
                (
                    "major",
                    PackageValue::Unsigned(row["major"].as_u64().unwrap()),
                ),
            ]),
        )
        .await;
}

/// Whole-Host retirement is the supported recovery seam, not a per-Agent restart promise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a freshly built external Agent and TEAMARNESS_AGENT_PACKAGE_DIR"]
async fn uncertain_generic_command_requires_full_host_retirement_before_rebinding() {
    use teamarness_package_host::{RecoverPackage, SuspendPackage};
    let (endpoint, mut requests, count) = controlled_model().await;
    let app = harness::StagedApplication::new();
    stage_qualification_provider(&app.application_path);
    let core = open_core(Addressing::Loopback, &app.root.path().join("core")).await;
    let workspace = core.create_workspace("Generic recovery").unwrap();
    let a = core.add_agent(workspace.id(), "Scout").unwrap();
    let b = core.add_agent(workspace.id(), "Other").unwrap();
    let a_key = core.principal(workspace.id(), a).unwrap().unwrap().keys()[0].to_string();
    let b_key = core.principal(workspace.id(), b).unwrap().unwrap().keys()[0].to_string();
    let private = app.root.path().join("host");
    register_agent(&private);
    let host = open_host(&app.application_path, private.clone(), core.clone()).await;
    let key = |id: &str| {
        PackageInstanceKey::new(
            id.parse().unwrap(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        )
    };
    let mount = open_mount(&host, &key(AGENT_PACKAGE_ID)).await;
    let agent = ConversationClient {
        host: &host,
        mount: &mount,
    };
    configure_loopback_model(&agent, endpoint).await;
    agent
        .request(
            "bind-a",
            "agent.binding.set",
            object([("principalId", PackageValue::String(a.to_string()))]),
        )
        .await;
    select_qualification(&agent).await;
    let provider_mount = open_mount(&host, &key(QUALIFICATION)).await;
    let provider = ConversationClient {
        host: &host,
        mount: &provider_mount,
    };
    chat_send(&agent, "write whose preauthorization will time out").await;
    let command = model_request(&mut requests).await;
    let pause = host.pause_next_command_actor_for_tests(AGENT_PACKAGE_ID.parse().unwrap());
    let _release = pause.resume_on_drop();
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"old-A"}),
        ))
        .unwrap();
    pause.paused().await;
    binding_refused(&agent, Some(b.to_string()), "agent.binding_busy").await;
    binding_refused(&agent, None, "agent.binding_busy").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a)
    );
    let terminal = model_request(&mut requests).await;
    assert_eq!(
        tool_result(&terminal.body),
        json!({"error":"agent.tool_outcome_unknown","retryable":false})
    );
    // protocol's outer capability timeout cancels this preauthorization future
    // before broker.call; there is deliberately no broker terminal observation.
    let snapshot = host.snapshot().await;
    assert_eq!(
        snapshot.state,
        teamarness_package_host::PackageHostState::Running
    );
    let instance = snapshot
        .instances
        .iter()
        .find(|slot| slot.key == key(AGENT_PACKAGE_ID))
        .unwrap();
    assert_eq!(instance.process_generation, Some(1));
    assert_eq!(
        instance.lifecycle,
        teamarness_package_host::LifecycleState::Active
    );
    assert!(snapshot.diagnostics.is_empty());
    public_host_state(&host, "generic-after-held-deadline").await;
    terminal
        .answer
        .send(json!({"role":"assistant","content":"unknown; never replay"}))
        .unwrap();
    chat_idle(&agent).await;
    binding_refused(&agent, Some(b.to_string()), "agent.binding_uncertain").await;
    binding_refused(&agent, None, "agent.binding_uncertain").await;
    assert!(provider_rows(&provider).await.is_empty());
    // Command uncertainty is not a global latch on unrelated query/direct-chat work.
    chat_send(&agent, "unrelated query remains live").await;
    model_request(&mut requests)
        .await
        .answer
        .send(tool_call("qualification__sample_read", json!({"offset":7})))
        .unwrap();
    let query = model_request(&mut requests).await;
    assert_eq!(
        tool_result(&query.body),
        json!({"answer":42,"actor":{"kind":"agent","principal":a.to_string(),"identity":a_key}})
    );
    query
        .answer
        .send(json!({"role":"assistant","content":"query live"}))
        .unwrap();
    chat_idle(&agent).await;
    binding_refused(&agent, Some(b.to_string()), "agent.binding_uncertain").await;
    host.execute(PackageHostCommand::Suspend(SuspendPackage {
        instance: key(AGENT_PACKAGE_ID),
    }))
    .await
    .unwrap();
    host.execute(PackageHostCommand::Recover(RecoverPackage {
        instance: key(AGENT_PACKAGE_ID),
    }))
    .await
    .unwrap();
    binding_refused(&agent, Some(b.to_string()), "agent.binding_uncertain").await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(a)
    );
    tokio::time::timeout(Duration::from_secs(30), shutdown(host.clone()))
        .await
        .expect("whole Host must retire while old pause is held");
    println!(
        "generic timeline: uncertainty/refusal survives unrelated query and Suspend/Recover; whole Host retires while old context held"
    );
    let replacement = open_host(&app.application_path, private, core.clone()).await;
    let new_mount = open_mount(&replacement, &key(AGENT_PACKAGE_ID)).await;
    assert_ne!(mount, new_mount);
    let fresh = ConversationClient {
        host: &replacement,
        mount: &new_mount,
    };
    let saved = serde_json::to_value(
        fresh
            .request("retained", "agent.packages.get", object([]))
            .await,
    )
    .unwrap();
    let selected = saved["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["contract"] == QUALIFICATION)
        .unwrap();
    assert_eq!(selected["enabled"], true);
    assert_eq!(selected["enabledMajor"], 3);
    fresh
        .request(
            "fresh-bind-b",
            "agent.binding.set",
            object([("principalId", PackageValue::String(b.to_string()))]),
        )
        .await;
    assert_eq!(
        core.bound_agent(workspace.id(), AGENT_PACKAGE_ID).unwrap(),
        Some(b)
    );
    pause.resume();
    let new_provider_mount = open_mount(&replacement, &key(QUALIFICATION)).await;
    let new_provider = ConversationClient {
        host: &replacement,
        mount: &new_provider_mount,
    };
    assert!(provider_rows(&new_provider).await.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 4, "old command/turn replayed");
    chat_send(&fresh, "fresh B command after whole Host retirement").await;
    let command = model_request(&mut requests).await;
    assert!(
        command
            .body
            .to_string()
            .contains("fresh B command after whole Host retirement")
    );
    command
        .answer
        .send(tool_call(
            "qualification__sample_write",
            json!({"value":"fresh-B"}),
        ))
        .unwrap();
    let terminal = model_request(&mut requests).await;
    let expected = json!({"value":"fresh-B","actor":{"kind":"agent","principal":b.to_string(),"identity":b_key}});
    assert_eq!(tool_result(&terminal.body), expected);
    terminal
        .answer
        .send(json!({"role":"assistant","content":"fresh B succeeded"}))
        .unwrap();
    chat_idle(&fresh).await;
    assert_eq!(provider_rows(&new_provider).await, vec![expected]);
    assert_eq!(count.load(Ordering::SeqCst), 6);
    println!(
        "generic timeline: retained major3 selection/new mount/Core B; old pause released; only fresh B effect once, no old-A/replay"
    );
    shutdown(replacement).await;
    core.shutdown().await.unwrap();
}
