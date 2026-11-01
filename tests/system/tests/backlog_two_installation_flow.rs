//! Two Installations replicate the Backlog and converge per field.
//!
//! Hermetic like the Conversations two-Profile flow: real `CoreRuntime`s on loopback, real
//! Package Hosts, real SQLite and the real Backlog Backend process. The scenario covers:
//! (a) an Epic and a Ticket created on A appear on B; (b) B is offline while A edits, and B
//! catches up when it reconnects; (c) concurrent changes to different fields on A and B both
//! survive; (d) concurrent changes to the same field converge to one value on both, and neither
//! save is refused.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use support::harness::{
    Addressing, ConversationClient, assert_no_plaintext_sentinel, create_invitation, introduce,
    open_core, open_host, open_mount, poll_until, shutdown, stage_backlog,
};
use teamarness_core::CoreRuntime;
use teamarness_package_host::{
    OpenUiSubscription, PackageHost, PackageHostCommand, PackageHostNotification,
    PackageHostSubscriptionSelector, PackageIdentity, PackageInstanceKey, PackageValue, UiMountId,
    UiSubscriptionId, WorkspaceId,
};

const BACKLOG: &str = "dev.teamarness.backlog";
const CONVERGENCE: Duration = Duration::from_secs(30);

/// One Installation's private paths, so it can be stopped and reopened.
struct Paths {
    core: PathBuf,
    private: PathBuf,
}

struct Profile {
    core: Arc<CoreRuntime>,
    host: PackageHost,
    mount: UiMountId,
}

impl Profile {
    async fn open(application: &Path, paths: &Paths, key: &PackageInstanceKey) -> Self {
        let core = open_core(Addressing::Loopback, &paths.core).await;
        Self::with_core(application, paths, key, core).await
    }

    async fn with_core(
        application: &Path,
        paths: &Paths,
        key: &PackageInstanceKey,
        core: Arc<CoreRuntime>,
    ) -> Self {
        let host = open_host(application, paths.private.clone(), core.clone()).await;
        let mount = open_mount(&host, key).await;
        Self { core, host, mount }
    }

    async fn stop(self) {
        shutdown(self.host).await;
        self.core.shutdown().await.unwrap();
    }

    async fn request(&self, name: &str, operation: &str, payload: Value) -> Value {
        let client = ConversationClient {
            host: &self.host,
            mount: &self.mount,
        };
        let payload: PackageValue = serde_json::from_value(payload).unwrap();
        serde_json::to_value(client.request(name, operation, payload).await).unwrap()
    }

    /// Waits for a `backlog.changes` snapshot that satisfies `ready`, through the Host.
    async fn wait_for_snapshot(&self, name: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let subscription = UiSubscriptionId::new(name).unwrap();
        self.host
            .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
                mount: self.mount.clone(),
                subscription: subscription.clone(),
                operation: "backlog.changes".into(),
                payload: PackageValue::Object(Default::default()),
            }))
            .await
            .unwrap();
        let mut events = self
            .host
            .subscribe(PackageHostSubscriptionSelector::UiEvents {
                mount: self.mount.clone(),
                subscription,
            })
            .await
            .unwrap();
        tokio::time::timeout(CONVERGENCE, async {
            loop {
                if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                    let snapshot = serde_json::to_value(&event.payload).unwrap();
                    if ready(&snapshot) {
                        return snapshot;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no matching snapshot for {name}"))
    }

    async fn get(&self, name: &str, ticket_id: &str) -> Value {
        self.request(name, "backlog.ticket.get", json!({ "ticketId": ticket_id }))
            .await
    }

    /// The Ticket, or `None` while this Installation has not projected it.
    async fn try_get(&self, name: &str, ticket_id: &str) -> Option<Value> {
        let client = ConversationClient {
            host: &self.host,
            mount: &self.mount,
        };
        if client
            .request_fails(
                name,
                "backlog.ticket.get",
                serde_json::from_value(json!({ "ticketId": ticket_id })).unwrap(),
            )
            .await
        {
            return None;
        }
        Some(self.get(name, ticket_id).await)
    }
}

/// Verifies: TEAM.SYS.BACKLOG-R03, TEAM.SYS.BACKLOG-R04, TEAM.SYS.BACKLOG-R10 (AC4, AC5, AC7,
/// AC26).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn two_installations_replicate_and_converge_per_field() {
    let root = tempfile::tempdir().unwrap();
    let application = stage_backlog(root.path());
    let alice_paths = Paths {
        core: root.path().join("alice-core"),
        private: root.path().join("alice-private"),
    };
    let bob_paths = Paths {
        core: root.path().join("bob-core"),
        private: root.path().join("bob-private"),
    };

    let alice_core = open_core(Addressing::Loopback, &alice_paths.core).await;
    let workspace_id = alice_core.create_workspace("Backlog flow").unwrap().id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(BACKLOG).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice = Profile::with_core(&application, &alice_paths, &key, alice_core).await;

    // (a) An Epic and a Ticket created on A appear on B.
    let epic_id = alice
        .request(
            "create-epic",
            "backlog.epic.create",
            json!({ "title": "qualification-sentinel-epic" }),
        )
        .await["epicId"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = alice
        .request(
            "create-ticket",
            "backlog.ticket.create",
            json!({ "epicId": epic_id, "title": "qualification-sentinel-title",
                    "body": "qualification-sentinel-body", "priority": "low" }),
        )
        .await;
    let ticket_id = created["ticketId"].as_str().unwrap().to_owned();
    let invitation = create_invitation(&alice.core, workspace_id).await;
    let bob_core = open_core(Addressing::Loopback, &bob_paths.core).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob = Profile::with_core(&application, &bob_paths, &key, bob_core).await;
    let original = alice.get("alice-original", &ticket_id).await;
    assert!(
        poll_until(CONVERGENCE, || async {
            bob.try_get("bob-sees-ticket", &ticket_id).await.as_ref() == Some(&original)
        })
        .await,
        "Bob did not receive Alice's Epic and Ticket"
    );
    assert_eq!(original["title"], "qualification-sentinel-title");
    assert_eq!(original["epicId"], epic_id);
    let listed = bob
        .wait_for_snapshot("bob-listing", |snapshot| {
            snapshot["epics"][0]["title"] == "qualification-sentinel-epic"
        })
        .await;
    assert_eq!(listed["epics"][0]["epicId"], epic_id);
    assert_eq!(listed["tickets"][0]["ticketId"], ticket_id);
    assert_eq!(listed["tickets"][0]["priority"], "low");

    // (b) B is offline while A edits; B catches up when it reconnects.
    bob.stop().await;
    let edited = alice
        .request(
            "alice-offline-edit",
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": original["version"],
                    "priority": "high" }),
        )
        .await;
    alice
        .request(
            "alice-offline-rename",
            "backlog.epic.rename",
            json!({ "epicId": epic_id, "title": "qualification-sentinel-epic-renamed" }),
        )
        .await;
    let bob = Profile::open(&application, &bob_paths, &key).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);
    let caught_up = alice.get("alice-caught-up", &ticket_id).await;
    assert_eq!(caught_up["version"], edited["version"]);
    assert!(
        poll_until(CONVERGENCE, || async {
            bob.get("bob-caught-up", &ticket_id).await == caught_up
        })
        .await,
        "Bob did not catch up after reconnecting"
    );
    bob.wait_for_snapshot("bob-renamed", |snapshot| {
        snapshot["epics"][0]["title"] == "qualification-sentinel-epic-renamed"
    })
    .await;
    // Bob's reopened Backend rebuilt the title and body it had seen before from sealed rows.
    assert_eq!(caught_up["title"], "qualification-sentinel-title");
    assert_eq!(caught_up["body"], "qualification-sentinel-body");

    // (c) and (d): both save while neither has seen the other's change.
    let base = caught_up["version"].clone();
    bob.stop().await;
    let alice_saved = alice
        .request(
            "alice-concurrent",
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": base,
                    "body": "qualification-sentinel-alice-body",
                    "title": "qualification-sentinel-alice-title" }),
        )
        .await;
    assert!(alice_saved["version"].is_string());
    alice.stop().await;
    let bob = Profile::open(&application, &bob_paths, &key).await;
    let bob_saved = bob
        .request(
            "bob-concurrent",
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": base, "status": "done",
                    "title": "qualification-sentinel-bob-title" }),
        )
        .await;
    assert!(
        bob_saved["version"].is_string(),
        "Bob's unseen-concurrent save was refused"
    );
    let alice = Profile::open(&application, &alice_paths, &key).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);
    assert!(
        poll_until(CONVERGENCE, || async {
            let on_alice = alice.get("alice-converged", &ticket_id).await;
            let on_bob = bob.get("bob-converged", &ticket_id).await;
            on_alice == on_bob
                && on_alice["body"] == "qualification-sentinel-alice-body"
                && on_alice["status"] == "done"
        })
        .await,
        "concurrent saves did not converge"
    );
    let converged = alice.get("alice-final", &ticket_id).await;
    assert_eq!(converged, bob.get("bob-final", &ticket_id).await);
    // (c) Different fields: Alice's body and Bob's status both survive, with the priority
    // from before.
    assert_eq!(converged["priority"], "high");
    // (d) The same field converges to one of the two saves, identically on both.
    let title = converged["title"].as_str().unwrap();
    assert!(
        title == "qualification-sentinel-alice-title"
            || title == "qualification-sentinel-bob-title",
        "{title}"
    );
    let latest = std::cmp::max(
        alice_saved["version"].as_str().unwrap(),
        bob_saved["version"].as_str().unwrap(),
    );
    assert_eq!(converged["version"], latest);
    let winner = if latest == alice_saved["version"].as_str().unwrap() {
        "qualification-sentinel-alice-title"
    } else {
        "qualification-sentinel-bob-title"
    };
    assert_eq!(title, winner, "the later save's title wins on both");

    alice.stop().await;
    bob.stop().await;
    for directory in [
        &alice_paths.core,
        &alice_paths.private,
        &bob_paths.core,
        &bob_paths.private,
    ] {
        assert_no_plaintext_sentinel(directory);
    }
}
