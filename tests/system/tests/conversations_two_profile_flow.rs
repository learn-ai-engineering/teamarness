//! The two-Profile recovery matrix, run twice over the same scenario.
//!
//! The default run is hermetic: real `CoreRuntime`s, real Iroh endpoints bound
//! to loopback, real SQLite, a real Conversations Backend process. Nothing off
//! this machine is contacted, so `cargo test --workspace` passes offline.
//! The production-relay run of the identical scenario is `#[ignore]`d and runs
//! under `npm run verify:conversations`, where it also qualifies the one thing
//! loopback cannot: relay-mediated addressing and reachability (SF-002).
#![cfg(unix)]

mod support;

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::harness::{
    self, Addressing, ConversationClient, GENERAL_ID, PACKAGE_ID, assert_no_plaintext_sentinel,
    assert_projection_is_duplicate_free, channel_id, channel_is_hidden, channel_projection,
    create_invitation, has_channel, has_message_on_page, introduce, message_projection, object,
    open_core, open_host, open_mount, poll_short, poll_until, shutdown, snapshot_has_warning,
};
use teamarness_core::CoreRuntime;
use teamarness_package_host::{
    OpenUiSubscription, PackageHostCommand, PackageHostNotification,
    PackageHostSubscriptionSelector, PackageIdentity, PackageInstanceKey, PackageValue,
    UiSubscriptionId, WorkspaceId,
};

/// Verifies: TEAM.SYS.CONV-R17 — two independent Core/Host/Backend profiles.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_remote_message_keeps_author_while_person_name_changes() {
    let application = support::harness::StagedApplication::new();
    let root = application.root.path();
    let alice_core = open_core(Addressing::Loopback, &root.join("name-alice-core")).await;
    let workspace = alice_core.create_workspace("Names").unwrap();
    let workspace_id = workspace.id();
    alice_core
        .set_own_person_name(workspace_id, "Alice")
        .unwrap();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_host = open_host(
        &application.application_path,
        root.join("name-alice-host"),
        alice_core.clone(),
    )
    .await;
    let alice_mount = open_mount(&alice_host, &key).await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    alice
        .post_message("names-old-message", GENERAL_ID, "authored before rename")
        .await;

    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(Addressing::Loopback, &root.join("name-bob-core")).await;
    introduce(Addressing::Loopback, &[&alice_core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob_host = open_host(
        &application.application_path,
        root.join("name-bob-host"),
        bob_core.clone(),
    )
    .await;
    let bob_mount = open_mount(&bob_host, &key).await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    assert!(
        poll_short(|| async {
            has_message_on_page(
                &bob.channel_open("names-before", GENERAL_ID, 32).await,
                "authored before rename",
            )
        })
        .await
    );

    let subscription = UiSubscriptionId::new("names-across-peers").unwrap();
    bob_host
        .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount: bob_mount.clone(),
            subscription: subscription.clone(),
            operation: "conversations.principals".into(),
            payload: object([]),
        }))
        .await
        .unwrap();
    let mut events = bob_host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: bob_mount.clone(),
            subscription,
        })
        .await
        .unwrap();
    let author = alice_core.public_identity().to_string();
    let contains_name = |payload: &PackageValue, expected: &str| {
        let PackageValue::Object(fields) = payload else {
            return false;
        };
        let Some(PackageValue::Array(principals)) = fields.get("principals") else {
            return false;
        };
        principals.iter().any(|principal| {
            matches!(principal, PackageValue::Object(principal)
            if principal.get("identity") == Some(&PackageValue::String(author.clone()))
            && principal.get("kind") == Some(&PackageValue::String("person".into()))
            && principal.get("name") == Some(&PackageValue::String(expected.into())))
        })
    };
    let saw_initial = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap()
                && contains_name(&event.payload, "Alice")
            {
                break true;
            }
        }
    })
    .await
    .unwrap();
    assert!(saw_initial);
    alice_core
        .set_own_person_name(workspace_id, "Alice 🌻")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap()
                && contains_name(&event.payload, "Alice 🌻")
            {
                break;
            }
        }
    })
    .await
    .expect("renamed Person reaches Bob's Conversations UI stream");
    let page = bob.channel_open("names-after", GENERAL_ID, 32).await;
    let PackageValue::Object(fields) = page else {
        panic!("expected history page")
    };
    let Some(PackageValue::Array(messages)) = fields.get("messages") else {
        panic!("expected Messages")
    };
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, PackageValue::Object(row)
        if row.get("markdown") == Some(&PackageValue::String("authored before rename".into()))
        && row.get("author") == Some(&PackageValue::String(author.clone()))))
    );
    shutdown(bob_host).await;
    shutdown(alice_host).await;
    bob_core.shutdown().await.unwrap();
    alice_core.shutdown().await.unwrap();
}

/// Verifies: TEAM.SYS.TRANS-R08, TEAM.SYS.CONV-R04, TEAM.SYS.CONV-R12
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn empty_general_member_posts_without_creator_record() {
    empty_general_first_post(true).await;
}

/// Verifies: TEAM.SYS.TRANS-R08, TEAM.SYS.CONV-R12
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn empty_general_member_posts_with_both_schedulers_running() {
    empty_general_first_post(false).await;
}

async fn empty_general_first_post(pause_holder: bool) {
    let application = support::harness::StagedApplication::new();
    let alice_core = open_core(
        Addressing::Loopback,
        &application.root.path().join("alice-core"),
    )
    .await;
    let workspace = alice_core.create_workspace("Empty general").unwrap();
    let workspace_id = workspace.id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_host = open_host(
        &application.application_path,
        application.root.path().join("alice-private"),
        alice_core.clone(),
    )
    .await;
    let alice_mount = open_mount(&alice_host, &key).await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    let initial = alice.snapshot("alice-empty-general").await;
    assert!(has_channel(&initial, "general"));
    assert!(
        message_projection(&alice_host, &alice_mount)
            .await
            .is_empty()
    );

    if pause_holder {
        // Prevent a holder-initiated session from masking the requester wake.
        alice_core.pause_sync_scheduler_for_tests(true);
    }
    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(
        Addressing::Loopback,
        &application.root.path().join("bob-core"),
    )
    .await;
    introduce(Addressing::Loopback, &[&alice_core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob_host = open_host(
        &application.application_path,
        application.root.path().join("bob-private"),
        bob_core.clone(),
    )
    .await;
    let bob_mount = open_mount(&bob_host, &key).await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    assert!(
        poll_until(Duration::from_secs(20), || async {
            let PackageValue::Object(fields) = bob.snapshot("bob-empty-availability").await else {
                return false;
            };
            fields.get("availability") == Some(&PackageValue::String("current".into()))
        })
        .await,
        "Bob did not acquire empty #general history access before record idle: backend={:?} core={:?}",
        match bob.snapshot("bob-empty-final-state").await {
            PackageValue::Object(fields) => fields.get("availability").cloned(),
            _ => None,
        },
        bob_core
            .subscribe_records(workspace_id, PACKAGE_ID, None)
            .unwrap()
            .availability()
            .unwrap()
            .availability,
    );
    assert!(
        message_projection(&alice_host, &alice_mount)
            .await
            .is_empty(),
        "Alice authored no Package record"
    );
    bob.post_message("bob-first-post", GENERAL_ID, "first Member Message")
        .await;
    assert!(
        poll_short(|| async {
            has_message_on_page(
                &alice.channel_open("alice-first-post", GENERAL_ID, 50).await,
                "first Member Message",
            )
        })
        .await,
        "Bob's first Message did not reach Alice"
    );
    shutdown(alice_host).await;
    shutdown(bob_host).await;
    alice_core.shutdown().await.unwrap();
    bob_core.shutdown().await.unwrap();
}

/// Verifies: TEAM.SYS.CONV-R06, TEAM.SYS.CONV-R10, TEAM.SYS.CONV-R11, TEAM.SYS.CONV-R12
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn complete_two_profile_recovery_matrix() {
    two_profile_recovery_matrix(Addressing::Loopback).await;
}

/// The same matrix over public relays: the realistic addressing the hermetic
/// run cannot reach. Verifies: TEAM.SYS.CONV-R06, TEAM.SYS.CONV-R10,
/// TEAM.SYS.CONV-R11, TEAM.SYS.CONV-R12, TEAM.SYS.TRANS-R06.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "production-relay qualification; run with npm run verify:conversations"]
async fn complete_two_profile_recovery_matrix_over_production_relays() {
    two_profile_recovery_matrix(Addressing::ProductionRelay).await;
}

async fn two_profile_recovery_matrix(addressing: Addressing) {
    let application = support::harness::StagedApplication::new();
    let application_path = application.application_path.clone();
    let alice_core_path = application.root.path().join("alice-core");
    let bob_core_path = application.root.path().join("bob-core");
    let alice_private = application.root.path().join("alice-private");
    let bob_private = application.root.path().join("bob-private");

    let alice_core = open_core(addressing, &alice_core_path).await;
    let workspace = alice_core.create_workspace("Conversations flow").unwrap();
    let workspace_id = workspace.id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_host = open_host(&application_path, alice_private.clone(), alice_core.clone()).await;
    let alice_mount = open_mount(&alice_host, &key).await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };

    alice
        .post_message(
            "qualification-sentinel-pre-admission",
            GENERAL_ID,
            "qualification-sentinel-pre-admission",
        )
        .await;
    alice
        .create_channel("pre-admission-channel", "shared-channel")
        .await;
    let alice_snapshot = alice.snapshot("alice-pre-admission").await;
    let shared_channel = channel_id(&alice_snapshot, "shared-channel").unwrap();

    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(addressing, &bob_core_path).await;
    introduce(addressing, &[&alice_core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob_host = open_host(&application_path, bob_private.clone(), bob_core.clone()).await;
    let bob_mount = open_mount(&bob_host, &key).await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };

    assert!(
        poll_short(|| async {
            let bob_snapshot = bob.snapshot("bob-admission-history").await;
            let bob_page = bob.channel_open("bob-admission-page", GENERAL_ID, 50).await;
            has_channel(&bob_snapshot, "shared-channel")
                && has_message_on_page(&bob_page, "qualification-sentinel-pre-admission")
        })
        .await,
        "newly admitted Bob did not receive Alice's existing history"
    );

    // Stop both profiles.
    harness::shutdown(alice_host).await;
    alice_core.shutdown().await.unwrap();
    drop(alice_core);
    harness::shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    drop(bob_core);

    // Alice reopens, sends offline messages, creates offline channel, stops.
    let (alice_core, alice_host, alice_mount) = reopen_profile(
        addressing,
        &application_path,
        &alice_core_path,
        &alice_private,
        &key,
    )
    .await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    alice
        .post_message(
            "alice-offline-message",
            GENERAL_ID,
            "qualification-sentinel-alice-offline",
        )
        .await;
    alice
        .create_channel("alice-offline-channel", "offline-team")
        .await;
    harness::shutdown(alice_host).await;
    alice_core.shutdown().await.unwrap();
    drop(alice_core);

    // Bob reopens, sends offline messages, creates offline channel, stops.
    let (bob_core, bob_host, bob_mount) = reopen_profile(
        addressing,
        &application_path,
        &bob_core_path,
        &bob_private,
        &key,
    )
    .await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    bob.post_message(
        "bob-offline-message",
        GENERAL_ID,
        "qualification-sentinel-bob-offline",
    )
    .await;
    bob.create_channel("bob-offline-channel", "offline-team")
        .await;
    harness::shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    drop(bob_core);

    // Both reopen simultaneously to trigger sync reconciliation.
    let (alice_core, alice_host, alice_mount) = reopen_profile(
        addressing,
        &application_path,
        &alice_core_path,
        &alice_private,
        &key,
    )
    .await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    let (bob_core, bob_host, bob_mount) = reopen_profile(
        addressing,
        &application_path,
        &bob_core_path,
        &bob_private,
        &key,
    )
    .await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    // Both endpoints bound fresh ports on reopen, so re-introduce them.
    introduce(addressing, &[&alice_core, &bob_core]);

    assert!(
        poll_until(Duration::from_secs(30), || async {
            let alice_msgs = message_projection(&alice_host, &alice_mount).await;
            let bob_msgs = message_projection(&bob_host, &bob_mount).await;
            let alice_channels = channel_projection(&alice.snapshot("alice-reconnected").await);
            let bob_channels = channel_projection(&bob.snapshot("bob-reconnected").await);
            alice_msgs.len() == 3
                && alice_msgs == bob_msgs
                && alice_channels == bob_channels
                && alice_channels
                    .iter()
                    .filter(|(_, slug)| slug.starts_with("offline-team"))
                    .count()
                    == 2
        })
        .await,
        "disconnected projections did not reconcile automatically"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(alice_projection, bob_projection);
    assert_projection_is_duplicate_free(&alice_projection);

    alice
        .hide_channel("hide-shared-channel", &shared_channel)
        .await;
    assert!(
        poll_short(|| async {
            channel_is_hidden(&bob.snapshot("bob-hidden-channel").await, &shared_channel)
        })
        .await,
        "Channel hide did not converge"
    );
    assert!(
        bob.request_fails(
            "post-hidden-channel",
            "conversations.message.post",
            object([
                ("channelId", PackageValue::String(shared_channel.clone())),
                (
                    "markdown",
                    PackageValue::String("qualification-sentinel-hidden-post".into()),
                ),
            ]),
        )
        .await,
        "post-hide Message was accepted locally"
    );

    let projection_before_malformed = message_projection(&alice_host, &alice_mount).await;
    alice_core
        .append_record(
            workspace_id,
            PACKAGE_ID,
            "message.post",
            b"malformed-qualification-record",
        )
        .unwrap();
    assert!(
        poll_short(|| async {
            snapshot_has_warning(&alice.snapshot("alice-warning").await)
                && snapshot_has_warning(&bob.snapshot("bob-warning").await)
        })
        .await,
        "malformed record warning did not converge"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(projection_before_malformed, alice_projection);
    assert_eq!(alice_projection, bob_projection);

    shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    drop(bob_core);
    let (bob_core, bob_host, bob_mount) = reopen_profile(
        addressing,
        &application_path,
        &bob_core_path,
        &bob_private,
        &key,
    )
    .await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };
    introduce(addressing, &[&alice_core, &bob_core]);
    assert_eq!(
        message_projection(&alice_host, &alice_mount).await,
        message_projection(&bob_host, &bob_mount).await,
        "Bob did not resume the persisted projection after restart"
    );
    assert!(snapshot_has_warning(
        &bob.snapshot("bob-restart-warning").await
    ));
    alice
        .post_message(
            "qualification-sentinel-after-restart",
            GENERAL_ID,
            "qualification-sentinel-after-restart",
        )
        .await;
    assert!(
        poll_short(|| async {
            has_message_on_page(
                &bob.channel_open("bob-after-restart-page", GENERAL_ID, 50)
                    .await,
                "qualification-sentinel-after-restart",
            )
        })
        .await,
        "Bob's resumed cursor did not deliver a later Message"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(alice_projection, bob_projection);
    assert_projection_is_duplicate_free(&bob_projection);

    shutdown(alice_host).await;
    alice_core.shutdown().await.unwrap();
    drop(alice_core);
    shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    drop(bob_core);

    for directory in [alice_core_path, bob_core_path, alice_private, bob_private] {
        assert_no_plaintext_sentinel(&directory);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "ordinary-machine timing qualification; run with npm run verify:conversations"]
async fn ordinary_machine_qualification_meets_delivery_and_catch_up_targets() {
    const QUEUED_MESSAGES: usize = 100;
    const REACHABLE_TARGET: Duration = Duration::from_secs(10);
    const CATCH_UP_TARGET: Duration = Duration::from_secs(30);

    eprintln!(
        "conversations qualification environment: os={} arch={} cpus={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, usize::from)
    );

    let application = support::harness::StagedApplication::new();
    let application_path = application.application_path.clone();
    let alice_core_path = application.root.path().join("alice-core");
    let bob_core_path = application.root.path().join("bob-core");
    let alice_private = application.root.path().join("alice-private");
    let bob_private = application.root.path().join("bob-private");

    // Timing is measured over the addressing the application really uses.
    let addressing = Addressing::ProductionRelay;
    let alice_core = open_core(addressing, &alice_core_path).await;
    let workspace = alice_core.create_workspace("Qualification").unwrap();
    let workspace_id = workspace.id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = open_core(addressing, &bob_core_path).await;
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);

    let alice_host = open_host(&application_path, alice_private.clone(), alice_core.clone()).await;
    let alice_mount = open_mount(&alice_host, &key).await;
    let bob_host = open_host(&application_path, bob_private.clone(), bob_core.clone()).await;
    let bob_mount = open_mount(&bob_host, &key).await;
    let alice = ConversationClient {
        host: &alice_host,
        mount: &alice_mount,
    };
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };

    // Seed an initial message so both profiles have a base for the channel.
    alice
        .post_message(
            "timing-seed",
            GENERAL_ID,
            "qualification-sentinel-timing-seed",
        )
        .await;
    assert!(
        poll_short(|| async {
            let snapshot = bob.snapshot("timing-seed").await;
            let PackageValue::Object(fields) = &snapshot else {
                return false;
            };
            let Some(PackageValue::Array(channels)) = fields.get("channels") else {
                return false;
            };
            !channels.is_empty()
        })
        .await,
        "Bob did not establish the initial synchronized scope"
    );

    shutdown(bob_host).await;
    bob_core.shutdown().await.unwrap();
    drop(bob_core);

    // Queue messages from Alice while Bob is offline.
    for index in 0..QUEUED_MESSAGES {
        alice
            .post_message(
                &format!("qualification-queued-{index}"),
                GENERAL_ID,
                &format!("qualification-sentinel-{index:04}"),
            )
            .await;
    }
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    assert_eq!(alice_projection.len(), QUEUED_MESSAGES + 1);
    assert_projection_is_duplicate_free(&alice_projection);

    // Bob reconnects and should catch up within the target.
    let catch_up_started = Instant::now();
    let (bob_core, bob_host, bob_mount) = reopen_profile(
        addressing,
        &application_path,
        &bob_core_path,
        &bob_private,
        &key,
    )
    .await;
    let bob = ConversationClient {
        host: &bob_host,
        mount: &bob_mount,
    };

    // Poll for the last queued message to appear in Bob's channel.open page,
    // proving the sync session delivered records within the target.
    let last_sentinel = format!("qualification-sentinel-{:04}", QUEUED_MESSAGES - 1);
    let caught_up = poll_until(CATCH_UP_TARGET, || async {
        let page = bob.channel_open("bob-catchup-check", GENERAL_ID, 64).await;
        has_message_on_page(&page, &last_sentinel)
    })
    .await;
    if !caught_up {
        let page = bob.channel_open("bob-catchup-final", GENERAL_ID, 64).await;
        let PackageValue::Object(fields) = &page else {
            panic!("Bob's page after reconnect was not an object")
        };
        let count = fields.get("messages").map_or(0, |m| {
            if let PackageValue::Array(msgs) = m {
                msgs.len()
            } else {
                0
            }
        });
        panic!(
            "Bob did not catch up before the qualification deadline: projected_in_page={}",
            count
        );
    }
    let catch_up_elapsed = catch_up_started.elapsed();
    eprintln!(
        "conversations qualification timing: queued_records={} catch_up_ms={} target_ms={}",
        QUEUED_MESSAGES,
        catch_up_elapsed.as_millis(),
        CATCH_UP_TARGET.as_millis()
    );

    // Test reachable delivery from Bob to Alice.
    let reachable_started = Instant::now();
    bob.post_message(
        "reachable-message",
        GENERAL_ID,
        "qualification-reachable-sentinel",
    )
    .await;
    let delivered = poll_until(REACHABLE_TARGET, || async {
        let alice_page = alice
            .channel_open("alice-reachable-page", GENERAL_ID, 50)
            .await;
        has_message_on_page(&alice_page, "qualification-reachable-sentinel")
    })
    .await;
    assert!(
        delivered,
        "reachable delivery exceeded the qualification deadline"
    );
    let reachable_elapsed = reachable_started.elapsed();
    assert!(
        reachable_elapsed < REACHABLE_TARGET,
        "reachable delivery exceeded deadline: {}ms > {}ms",
        reachable_elapsed.as_millis(),
        REACHABLE_TARGET.as_millis()
    );
    eprintln!(
        "conversations qualification timing: reachable_delivery_ms={} target_ms={}",
        reachable_elapsed.as_millis(),
        REACHABLE_TARGET.as_millis()
    );

    shutdown(alice_host).await;
    shutdown(bob_host).await;
    alice_core.shutdown().await.unwrap();
    bob_core.shutdown().await.unwrap();
    drop(alice_core);
    drop(bob_core);

    for directory in [alice_core_path, bob_core_path, alice_private, bob_private] {
        assert_no_plaintext_sentinel(&directory);
    }
    eprintln!("conversations qualification persistence scan: plaintext_sentinel=absent");
}

// ------------------------------------------------------------------
// Helpers (not exported to support because they're test-specific wiring)
// ------------------------------------------------------------------

async fn reopen_profile(
    addressing: Addressing,
    application: &std::path::Path,
    core_path: &std::path::Path,
    private_path: &std::path::Path,
    key: &PackageInstanceKey,
) -> (
    Arc<CoreRuntime>,
    teamarness_package_host::PackageHost,
    teamarness_package_host::UiMountId,
) {
    let core = open_core(addressing, core_path).await;
    let host = open_host(application, private_path.to_owned(), core.clone()).await;
    let mount = open_mount(&host, key).await;
    (core, host, mount)
}
