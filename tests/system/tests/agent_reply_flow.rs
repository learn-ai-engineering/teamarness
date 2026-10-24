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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use support::harness::{
    self, Addressing, ConversationClient, GENERAL_ID, PACKAGE_ID, create_invitation, introduce,
    object, open_core, open_host, open_mount, poll_until, shutdown,
};
use teamarness_package_host::{
    PackageIdentity, PackageInstanceKey, PackageSourceKind, PackageTrustKind, PackageValue,
    WorkspaceId,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AGENT_PACKAGE_ID: &str = "dev.teamarness.agent";
const REPLY: &str = "Hello from the stub model";
const KEY: &str = "sk-system-test-key-never-logged";

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
