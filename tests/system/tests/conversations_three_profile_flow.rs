//! Three Conversations profiles over production Iroh relays.
//!
//! This is production-relay qualification, not part of `npm run check`: it
//! needs public relays and network access. The same convergence scenario is
//! covered hermetically by the Core `runtime_tests` cluster tests, which use
//! real `CoreRuntime`s and real Iroh endpoints over local addressing.
#![cfg(unix)]

// Every integration test binary compiles the shared harness and uses a subset.
#[allow(dead_code)]
mod support;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use support::harness::{
    ConversationClient, GENERAL_ID, PACKAGE_ID, Profile, assert_no_plaintext_sentinel,
    assert_projection_is_duplicate_free, create_invitation, has_message_on_page,
    message_projection, open_host, open_mount, poll_short, poll_until, shutdown,
};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{PackageIdentity, PackageInstanceKey, PackageValue, WorkspaceId};

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(60);

/// Verifies: TEAM.SYS.MEM-R10, TEAM.SYS.TRANS-R06, TEAM.SYS.CONV-R03
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "production-relay qualification; run with npm run verify:conversations"]
async fn three_profiles_converge_after_late_admission() {
    let application = support::harness::StagedApplication::new();
    let application_path = application.application_path.clone();
    let root = application.root.path().to_owned();

    let alice_core = Arc::new(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.join("alice-core")))
            .await
            .unwrap(),
    );
    let workspace = alice_core.create_workspace("Three profiles").unwrap();
    let workspace_id = workspace.id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice = open_profile(&application_path, &root, "alice", alice_core, &key).await;
    alice
        .client()
        .post_message(
            "three-profile-alice-first",
            GENERAL_ID,
            "three-profile-sentinel-alice",
        )
        .await;

    // Bob joins, then Carol joins later: Carol's admission is the one an
    // earlier Member only learns through membership replication.
    let bob = join_profile(
        &application_path,
        &root,
        "bob",
        &alice.core,
        workspace_id,
        &key,
    )
    .await;
    assert!(
        poll_short(|| async { sees(&bob.client(), "three-profile-sentinel-alice").await }).await,
        "bob did not receive alice's first message"
    );

    let carol = join_profile(
        &application_path,
        &root,
        "carol",
        &alice.core,
        workspace_id,
        &key,
    )
    .await;
    assert!(
        poll_short(|| async { sees(&carol.client(), "three-profile-sentinel-alice").await }).await,
        "carol did not receive alice's first message"
    );

    bob.client()
        .post_message(
            "three-profile-bob-first",
            GENERAL_ID,
            "three-profile-sentinel-bob",
        )
        .await;
    carol
        .client()
        .post_message(
            "three-profile-carol-first",
            GENERAL_ID,
            "three-profile-sentinel-carol",
        )
        .await;

    let everyones = [
        "three-profile-sentinel-alice",
        "three-profile-sentinel-bob",
        "three-profile-sentinel-carol",
    ];
    for (name, profile) in [("alice", &alice), ("bob", &bob), ("carol", &carol)] {
        assert!(
            poll_until(CONVERGENCE_TIMEOUT, || async {
                sees_all(&profile.client(), &everyones).await
            })
            .await,
            "{name} did not converge on every profile's message"
        );
    }

    // Bob and Carol never connected to each other through the manager's
    // Installation, so the manager going away must not stop them converging.
    shutdown(alice.host).await;
    alice.core.shutdown().await.unwrap();
    drop(alice.core);

    bob.client()
        .post_message(
            "three-profile-bob-after",
            GENERAL_ID,
            "three-profile-sentinel-bob-after-manager",
        )
        .await;
    carol
        .client()
        .post_message(
            "three-profile-carol-after",
            GENERAL_ID,
            "three-profile-sentinel-carol-after-manager",
        )
        .await;
    assert!(
        poll_until(CONVERGENCE_TIMEOUT, || async {
            sees(&bob.client(), "three-profile-sentinel-carol-after-manager").await
        })
        .await,
        "bob did not receive carol's message with the manager closed"
    );
    assert!(
        poll_until(CONVERGENCE_TIMEOUT, || async {
            sees(&carol.client(), "three-profile-sentinel-bob-after-manager").await
        })
        .await,
        "carol did not receive bob's message with the manager closed"
    );

    for profile in [&bob, &carol] {
        let projection = message_projection(&profile.host, &profile.mount).await;
        assert_projection_is_duplicate_free(&projection);
    }

    let paths = [
        alice.core_path,
        alice.private_path,
        bob.core_path.clone(),
        bob.private_path.clone(),
        carol.core_path.clone(),
        carol.private_path.clone(),
    ];
    shutdown(carol.host).await;
    carol.core.shutdown().await.unwrap();
    shutdown(bob.host).await;
    bob.core.shutdown().await.unwrap();
    for path in paths {
        assert_no_plaintext_sentinel(&path);
    }
}

async fn sees(client: &ConversationClient<'_>, markdown: &str) -> bool {
    let page: PackageValue = client
        .channel_open("three-profile-page", GENERAL_ID, 50)
        .await;
    has_message_on_page(&page, markdown)
}

async fn sees_all(client: &ConversationClient<'_>, markdown: &[&str]) -> bool {
    let page: PackageValue = client
        .channel_open("three-profile-page", GENERAL_ID, 50)
        .await;
    markdown
        .iter()
        .all(|sentinel| has_message_on_page(&page, sentinel))
}

async fn open_profile(
    application: &std::path::Path,
    root: &std::path::Path,
    name: &str,
    core: Arc<CoreRuntime>,
    key: &PackageInstanceKey,
) -> Profile {
    let core_path = root.join(format!("{name}-core"));
    let private_path = root.join(format!("{name}-private"));
    let host = open_host(application, private_path.clone(), core.clone()).await;
    let mount = open_mount(&host, key).await;
    Profile {
        core,
        host,
        mount,
        core_path,
        private_path,
    }
}

async fn join_profile(
    application: &std::path::Path,
    root: &std::path::Path,
    name: &str,
    manager: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
    key: &PackageInstanceKey,
) -> Profile {
    let invitation = create_invitation(manager, workspace).await;
    let core = Arc::new(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(
            root.join(format!("{name}-core")),
        ))
        .await
        .unwrap(),
    );
    core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    open_profile(application, root, name, core, key).await
}
