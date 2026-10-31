//! Package synchronization is gated on current membership.
//!
//! Real components: real `CoreRuntime`s on temporary directories, real Iroh
//! endpoints in loopback mode with the in-memory address book, real SQLite.

use std::collections::BTreeSet;
use std::time::Duration;

use super::super::ProbeOutcome;
use super::cluster::{DEFAULT_DEADLINE, SyncCluster};
use crate::synchronization::{LogSessionError, Refusal};

const PACKAGE: &str = "teamarness.test";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn names_converge_without_package_instance_after_restart() {
    let mut cluster = SyncCluster::start("names-three-peers", &["creator", "alice", "bob"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace_with_person_name("Names", "Creator")
        .unwrap()
        .id();
    cluster.admit("creator", "alice", workspace).await;
    cluster.admit("creator", "bob", workspace).await;
    let version = cluster
        .node("creator")
        .membership_version(workspace)
        .unwrap();
    let mut wake = cluster
        .node("creator")
        .subscribe_person_names(workspace)
        .unwrap();
    cluster.stop("bob").await;
    cluster
        .node("alice")
        .set_own_person_name(workspace, "Alice")
        .unwrap();
    cluster
        .node("alice")
        .set_own_person_name(workspace, "Alice twice")
        .unwrap();
    cluster.stop("alice").await;
    cluster.restart("alice").await;
    cluster.restart("bob").await;
    let identity = cluster.identity("alice");
    cluster
        .observe_until("name converges on peers", DEFAULT_DEADLINE, |cluster| {
            ["creator", "bob"].iter().all(|peer| {
                cluster
                    .node(peer)
                    .workspace_person(workspace, identity)
                    .unwrap()
                    .and_then(|person| person.name().map(str::to_owned))
                    .as_deref()
                    == Some("Alice twice")
            })
        })
        .await;
    tokio::time::timeout(DEFAULT_DEADLINE, wake.changed())
        .await
        .unwrap()
        .unwrap();
    let (generation, people) = cluster.node("bob").person_name_snapshot(workspace).unwrap();
    assert!(generation > 0);
    assert_eq!(
        people
            .iter()
            .find(|(id, _)| *id == identity)
            .unwrap()
            .1
            .name(),
        Some("Alice twice")
    );
    assert_eq!(
        cluster
            .node("creator")
            .membership_version(workspace)
            .unwrap(),
        version
    );
    cluster.stop("creator").await;
    cluster.stop("alice").await;
    cluster.stop("bob").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_name_delivered_before_membership_converges_after_restart() {
    let mut cluster = SyncCluster::start("early-agent-name", &["steward", "peer"]).await;
    let workspace = cluster
        .node("steward")
        .create_workspace("Names")
        .unwrap()
        .id();
    cluster.admit("steward", "peer", workspace).await;
    cluster
        .node("peer")
        .pause_membership_replication_for_test(true);
    cluster
        .node("steward")
        .pause_membership_replication_for_test(true);
    let agent = cluster
        .node("steward")
        .add_agent(workspace, "Early")
        .unwrap();
    cluster
        .node("steward")
        .rename_principal(workspace, agent, "Early again")
        .unwrap();
    cluster
        .node("steward")
        .set_own_person_name(workspace, "Still progressing")
        .unwrap();
    cluster
        .node("steward")
        .replicate_records_to_address_for_test(
            workspace,
            crate::synchronization::reserved_principals_scope(),
            cluster.address("peer"),
            cluster.identity("peer"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .node("peer")
            .principal(workspace, agent)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        cluster
            .node("peer")
            .workspace_person(workspace, cluster.identity("steward"))
            .unwrap()
            .unwrap()
            .name(),
        Some("Still progressing")
    );
    let deferred_count = |cluster: &SyncCluster| {
        let state = cluster.node("peer").state.lock().unwrap();
        state.connection.query_row(
            "SELECT COUNT(*) FROM deferred_principal_names WHERE workspace_id=?1 AND principal_id=?2",
            rusqlite::params![workspace.as_bytes().as_slice(), agent.as_bytes().as_slice()],
            |row| row.get::<_, i64>(0),
        ).unwrap()
    };
    assert_eq!(deferred_count(&cluster), 2);
    cluster.stop("peer").await;
    cluster.restart("peer").await;
    assert!(
        cluster
            .node("peer")
            .principal(workspace, agent)
            .unwrap()
            .is_none()
    );
    assert_eq!(deferred_count(&cluster), 2);
    // An authenticated current Person who is not the Agent's Steward cannot
    // override the deferred name when membership becomes known.
    {
        let peer = cluster.node("peer");
        let mut state = peer.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let wrong = crate::principals::edit::PrincipalNameEdit::sign(
            &secret,
            workspace,
            agent,
            3,
            "Wrong steward",
        )
        .unwrap();
        crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            "principal-name.edit",
            wrong.canonical_bytes(),
            crate::synchronization::MembershipStanding::Current,
            false,
            peer.clock.now_unix_ms(),
        )
        .unwrap();
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
    }
    assert!(
        cluster
            .node("peer")
            .principal(workspace, agent)
            .unwrap()
            .is_none()
    );
    assert_eq!(deferred_count(&cluster), 3);
    cluster
        .node("peer")
        .pause_membership_replication_for_test(false);
    cluster
        .node("steward")
        .pause_membership_replication_for_test(false);
    cluster
        .replicate_membership("steward", "peer", workspace)
        .await;
    cluster
        .wait_until("early agent name appears", DEFAULT_DEADLINE, |c| {
            c.node("peer")
                .principal(workspace, agent)
                .unwrap()
                .is_some_and(|principal| principal.name() == Some("Early again"))
        })
        .await;
    cluster.stop("peer").await;
    cluster.restart("peer").await;
    assert_eq!(
        cluster
            .node("peer")
            .principal(workspace, agent)
            .unwrap()
            .unwrap()
            .name(),
        Some("Early again")
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn early_name_reference_failure_does_not_advance_consumption() {
    let cluster = SyncCluster::start("early-name-fault", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    creator.sync_worker.shutdown().await;
    {
        let mut state = creator.state.lock().unwrap();
        let target = crate::principals::derive_agent(
            workspace,
            crate::MembershipOperationId::digest(b"never-admitted-agent"),
        );
        let secret = state.secret_key.clone();
        let edit = crate::principals::edit::PrincipalNameEdit::sign(
            &secret, workspace, target, 1, "Deferred",
        )
        .unwrap();
        crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            "principal-name.edit",
            edit.canonical_bytes(),
            crate::synchronization::MembershipStanding::Current,
            true,
            creator.clock.now_unix_ms(),
        )
        .unwrap();
        state.connection.execute_batch("CREATE TRIGGER fail_name_cursor BEFORE INSERT ON principal_name_consumption BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
        assert!(matches!(
            super::super::principal_sync::reconcile(&mut state, workspace),
            Err(crate::SynchronizationError::Storage(_))
        ));
        let cursor: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM principal_name_consumption WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let references: i64 = state
            .connection
            .query_row("SELECT COUNT(*) FROM deferred_principal_names", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!((cursor, references), (0, 0));
        state
            .connection
            .execute_batch("DROP TRIGGER fail_name_cursor")
            .unwrap();
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
        let references: i64 = state
            .connection
            .query_row("SELECT COUNT(*) FROM deferred_principal_names", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(references, 1);
    }
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_name_replay_does_not_probe_historical_principals_on_scheduler_reload() {
    let cluster = SyncCluster::start("name-empty-replay", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator
        .create_workspace_with_person_name("Names", "Creator")
        .unwrap()
        .id();
    creator.sync_worker.shutdown().await;
    for index in 0..64 {
        creator
            .add_agent(workspace, &format!("Agent {index}"))
            .unwrap();
    }
    {
        let mut state = creator.state.lock().unwrap();
        assert_eq!(state.memberships[&workspace].principal_history().len(), 65);
        let principal = crate::principals::derive(
            workspace,
            crate::principals::anchor(&state.memberships[&workspace], creator.public_identity())
                .unwrap(),
        );
        state.connection.execute(
            "INSERT INTO pending_principal_name_edits (workspace_id, principal_id, name) VALUES (?1, ?2, 'Pending')",
            rusqlite::params![workspace.as_bytes().as_slice(), principal.as_bytes().as_slice()],
        ).unwrap();
        for _ in 0..3 {
            super::super::principal_sync::retry_pending_initialization(&mut state);
            super::super::principal_sync::continue_replay(&mut state);
            assert!(!state.name_replay_scans.contains_key(&workspace));
        }
    }
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn appended_unknown_names_do_not_rewind_active_eligible_replay() {
    let cluster = SyncCluster::start("name-interleaved-replay", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    let agent = creator.add_agent(workspace, "Initial").unwrap();
    creator.sync_worker.shutdown().await;
    {
        let mut state = creator.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let scope = crate::synchronization::reserved_principals_scope();
        let unknown = crate::principals::derive_agent(
            workspace,
            crate::MembershipOperationId::digest(b"never-added-interleaved"),
        );
        let append = |state: &mut super::super::RuntimeState, target, counter, name: &str| {
            let edit = crate::principals::edit::PrincipalNameEdit::sign(
                &secret, workspace, target, counter, name,
            )
            .unwrap();
            crate::synchronization::append(
                &mut state.connection,
                &secret,
                workspace,
                scope,
                "principal-name.edit",
                edit.canonical_bytes(),
                crate::synchronization::MembershipStanding::Current,
                true,
                creator.clock.now_unix_ms(),
            )
            .unwrap()
            .1
        };
        for counter in 1..=80 {
            let sequence = append(&mut state, unknown, counter, "Unclaimed");
            state.connection.execute(
                "INSERT INTO deferred_principal_names (workspace_id, delivery_sequence, principal_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![workspace.as_bytes().as_slice(), sequence, unknown.as_bytes().as_slice()],
            ).unwrap();
        }
        let sequence = append(&mut state, agent, 2, "Reached");
        state.connection.execute(
            "INSERT INTO deferred_principal_names (workspace_id, delivery_sequence, principal_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![workspace.as_bytes().as_slice(), sequence, agent.as_bytes().as_slice()],
        ).unwrap();
        state.connection.execute(
            "INSERT INTO principal_name_consumption (workspace_id, delivery_sequence) VALUES (?1, ?2) ON CONFLICT(workspace_id) DO UPDATE SET delivery_sequence=excluded.delivery_sequence",
            rusqlite::params![workspace.as_bytes().as_slice(), sequence],
        ).unwrap();
        super::super::principal_sync::initialize(&mut state, workspace).unwrap();
        for counter in 81..=84 {
            append(&mut state, unknown, counter, "Unclaimed");
            super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
        }
    }
    assert_eq!(
        creator.principal(workspace, agent).unwrap().unwrap().name(),
        Some("Reached"),
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_name_intent_retry_keeps_active_replay_position() {
    let cluster = SyncCluster::start("name-idle-replay", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator
        .create_workspace_with_person_name("Names", "Creator")
        .unwrap()
        .id();
    let agent = creator.add_agent(workspace, "Initial").unwrap();
    creator.sync_worker.shutdown().await;
    {
        let mut state = creator.state.lock().unwrap();
        let principal = crate::principals::derive(
            workspace,
            crate::principals::anchor(&state.memberships[&workspace], creator.public_identity())
                .unwrap(),
        );
        let unknown = crate::principals::derive_agent(
            workspace,
            crate::MembershipOperationId::digest(b"never-added-idle"),
        );
        let secret = state.secret_key.clone();
        for counter in 1..=80 {
            let (target, name) = if counter == 60 {
                (agent, "Reached")
            } else {
                (unknown, "Unclaimed")
            };
            let edit = crate::principals::edit::PrincipalNameEdit::sign(
                &secret,
                workspace,
                target,
                if counter == 60 { 2 } else { counter },
                name,
            )
            .unwrap();
            let (_, sequence) = crate::synchronization::append(
                &mut state.connection,
                &secret,
                workspace,
                crate::synchronization::reserved_principals_scope(),
                "principal-name.edit",
                edit.canonical_bytes(),
                crate::synchronization::MembershipStanding::Current,
                true,
                creator.clock.now_unix_ms(),
            )
            .unwrap();
            state.connection.execute(
                "INSERT INTO deferred_principal_names (workspace_id, delivery_sequence, principal_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![workspace.as_bytes().as_slice(), sequence, target.as_bytes().as_slice()],
            ).unwrap();
            if counter == 80 {
                state.connection.execute(
                    "INSERT INTO principal_name_consumption (workspace_id, delivery_sequence) VALUES (?1, ?2) ON CONFLICT(workspace_id) DO UPDATE SET delivery_sequence=excluded.delivery_sequence",
                    rusqlite::params![workspace.as_bytes().as_slice(), sequence],
                ).unwrap();
            }
        }
        super::super::principal_sync::initialize(&mut state, workspace).unwrap();
        state.connection.execute(
            "INSERT INTO pending_principal_name_edits (workspace_id, principal_id, name) VALUES (?1, ?2, 'Pending')",
            rusqlite::params![workspace.as_bytes().as_slice(), principal.as_bytes().as_slice()],
        ).unwrap();
        super::super::principal_sync::retry_pending_initialization(&mut state);
    }
    assert_eq!(
        creator.principal(workspace, agent).unwrap().unwrap().name(),
        Some("Reached")
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eligible_name_replay_is_bounded_without_starvation_by_unknown_targets() {
    let cluster = SyncCluster::start("name-eligible-page", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    let agent = creator.add_agent(workspace, "Initial").unwrap();
    creator.sync_worker.shutdown().await;
    {
        let mut state = creator.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let unknown = crate::principals::derive_agent(
            workspace,
            crate::MembershipOperationId::digest(b"never-added"),
        );
        let scope = crate::synchronization::reserved_principals_scope();
        for (target, start) in [(unknown, 1_u64), (agent, 2_u64)] {
            for offset in 0..40 {
                let edit = crate::principals::edit::PrincipalNameEdit::sign(
                    &secret,
                    workspace,
                    target,
                    start + offset,
                    &format!("Name {offset}"),
                )
                .unwrap();
                let (_, sequence) = crate::synchronization::append(
                    &mut state.connection,
                    &secret,
                    workspace,
                    scope,
                    "principal-name.edit",
                    edit.canonical_bytes(),
                    crate::synchronization::MembershipStanding::Current,
                    true,
                    creator.clock.now_unix_ms(),
                )
                .unwrap();
                state.connection.execute(
                    "INSERT INTO deferred_principal_names (workspace_id, delivery_sequence, principal_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![workspace.as_bytes().as_slice(), sequence, target.as_bytes().as_slice()],
                ).unwrap();
            }
        }
        let max_sequence: i64 = state
            .connection
            .query_row(
                "SELECT MAX(delivery_sequence) FROM record_delivery",
                [],
                |row| row.get(0),
            )
            .unwrap();
        state.connection.execute("INSERT INTO principal_name_consumption (workspace_id, delivery_sequence) VALUES (?1, ?2) ON CONFLICT(workspace_id) DO UPDATE SET delivery_sequence=excluded.delivery_sequence", rusqlite::params![workspace.as_bytes().as_slice(), max_sequence]).unwrap();
        super::super::principal_sync::initialize(&mut state, workspace).unwrap();
        let remaining: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM deferred_principal_names WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 80,
            "one lock hold inspects at most 32 references"
        );
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
        let remaining: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM deferred_principal_names WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 56,
            "the next bounded scan reaches eligible deliveries"
        );
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
        let remaining: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM deferred_principal_names WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 40,
            "never-known deliveries remain but do not block the eligible target"
        );
    }
    assert_eq!(
        creator.principal(workspace, agent).unwrap().unwrap().name(),
        Some("Name 39")
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_name_replay_has_bounded_passes_and_continues_after_restart() {
    let mut cluster = SyncCluster::start("names-replay-backlog", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    creator.sync_worker.shutdown().await;
    {
        let mut state = creator.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        let person = crate::principals::derive(
            workspace,
            crate::principals::anchor(projection, creator.public_identity()).unwrap(),
        );
        let secret = state.secret_key.clone();
        for counter in 1..=80 {
            let name = format!("Name {counter}");
            let edit = crate::principals::edit::PrincipalNameEdit::sign(
                &secret, workspace, person, counter, &name,
            )
            .unwrap();
            crate::synchronization::append(
                &mut state.connection,
                &secret,
                workspace,
                crate::synchronization::reserved_principals_scope(),
                "principal-name.edit",
                edit.canonical_bytes(),
                crate::synchronization::MembershipStanding::Current,
                true,
                creator.clock.now_unix_ms(),
            )
            .unwrap();
        }
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
        let consumed: i64 = state
            .connection
            .query_row(
                "SELECT delivery_sequence FROM principal_name_consumption WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            consumed, 32,
            "one pass must not hold the runtime lock over the backlog"
        );
    }
    cluster.stop("creator").await;
    cluster.restart("creator").await;
    cluster
        .observe_until("bounded replay continues", DEFAULT_DEADLINE, |cluster| {
            cluster
                .node("creator")
                .workspace_person(workspace, cluster.identity("creator"))
                .unwrap()
                .and_then(|person| person.name().map(str::to_owned))
                .as_deref()
                == Some("Name 80")
        })
        .await;
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_edit_storage_open_failure_keeps_delivery_for_retry() {
    let cluster = SyncCluster::start("names-open-retry", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    {
        let mut state = creator.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        let person = crate::principals::derive(
            workspace,
            crate::principals::anchor(projection, creator.public_identity()).unwrap(),
        );
        let signed = crate::principals::edit::PrincipalNameEdit::sign(
            &state.secret_key,
            workspace,
            person,
            1,
            "Recovered",
        )
        .unwrap();
        let secret = state.secret_key.clone();
        crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            "principal-name.edit",
            signed.canonical_bytes(),
            crate::synchronization::MembershipStanding::Current,
            true,
            creator.clock.now_unix_ms(),
        )
        .unwrap();
        state.person_name_open_error_once = true;
        assert!(matches!(
            super::super::principal_sync::reconcile(&mut state, workspace),
            Err(crate::SynchronizationError::Storage(_))
        ));
        let count: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM principal_name_consumption",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "a storage error must not consume the valid record"
        );
        super::super::principal_sync::reconcile(&mut state, workspace).unwrap();
    }
    assert_eq!(
        creator
            .workspace_person(workspace, creator.public_identity())
            .unwrap()
            .unwrap()
            .name(),
        Some("Recovered")
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_join_retry_wakes_subscriber_while_key_is_unavailable() {
    let cluster = SyncCluster::start("names-retry-wake", &["creator", "joiner"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    cluster.node("creator").sync_worker.shutdown().await;
    cluster.node("joiner").sync_worker.shutdown().await;
    let now = crate::runtime::Clock::now_unix_ms(&super::super::SystemClock);
    let invitation = cluster
        .node("creator")
        .create_workspace_invitation_for_test(workspace, now, now + 600_000)
        .unwrap();
    cluster
        .node("joiner")
        .join_workspace_with_person_name(&invitation, "First")
        .await
        .unwrap();
    let mut wake = cluster
        .node("joiner")
        .subscribe_person_names(workspace)
        .unwrap();
    let (before, _) = cluster
        .node("joiner")
        .person_name_snapshot(workspace)
        .unwrap();
    cluster
        .node("joiner")
        .join_workspace_with_person_name(&invitation, "Second")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), wake.changed())
        .await
        .unwrap()
        .unwrap();
    let (after, people) = cluster
        .node("joiner")
        .person_name_snapshot(workspace)
        .unwrap();
    assert!(after > before);
    assert_eq!(
        people
            .iter()
            .find(|(id, _)| *id == cluster.identity("joiner"))
            .unwrap()
            .1
            .name(),
        Some("Second")
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_name_survives_restart_before_private_key_arrives() {
    let mut cluster = SyncCluster::start("names-delayed-key", &["creator", "joiner"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    // Onboarding transport remains live, but no LogSync session can supply a key yet.
    cluster.node("creator").sync_worker.shutdown().await;
    cluster.node("joiner").sync_worker.shutdown().await;
    cluster.admit("creator", "joiner", workspace).await;
    cluster
        .node("joiner")
        .set_own_person_name(workspace, "Delayed")
        .unwrap();
    {
        let state = cluster.node("joiner").state.lock().unwrap();
        let scope = crate::synchronization::scope_digest(
            crate::synchronization::reserved_principals_scope(),
            workspace,
        );
        let key_count: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let pending_count: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM pending_principal_name_edits WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((key_count, pending_count), (0, 1));
    }
    cluster.stop("joiner").await;
    cluster.restart("joiner").await;
    cluster
        .observe_until(
            "pending edit flushes after verified key",
            DEFAULT_DEADLINE,
            |cluster| {
                cluster
                    .node("creator")
                    .workspace_person(workspace, cluster.identity("joiner"))
                    .unwrap()
                    .and_then(|person| person.name().map(str::to_owned))
                    .as_deref()
                    == Some("Delayed")
            },
        )
        .await;
    {
        let state = cluster.node("joiner").state.lock().unwrap();
        let pending: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM pending_principal_name_edits WHERE workspace_id=?1",
                [workspace.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_private_edit_does_not_block_valid_name_and_revoked_person_is_hidden() {
    let cluster = SyncCluster::start("names-malformed-revoked", &["creator", "member"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    cluster.admit("creator", "member", workspace).await;
    let malformed = {
        let creator = cluster.node("creator");
        let state = creator.state.lock().unwrap();
        let anchor = crate::principals::anchor(
            state.memberships.get(&workspace).unwrap(),
            creator.public_identity(),
        )
        .unwrap();
        let wrong_workspace = crate::WorkspaceId::from_bytes([99; 16]);
        crate::principals::edit::PrincipalNameEdit::sign(
            &state.secret_key,
            wrong_workspace,
            crate::principals::derive(workspace, anchor),
            100,
            "Forged",
        )
        .unwrap()
        .canonical_bytes()
        .to_vec()
    };
    {
        let creator = cluster.node("creator");
        let mut state = creator.state.lock().unwrap();
        let secret = state.secret_key.clone();
        let (record, sequence) = crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            "principal-name.edit",
            &malformed,
            crate::synchronization::MembershipStanding::Current,
            true,
            creator.clock.now_unix_ms(),
        )
        .unwrap();
        assert!(!record.sealed_record().is_empty());
        crate::synchronization::record_subscription::publish_commit(
            &state,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            sequence,
        );
        creator.sync_worker.wake();
    }
    cluster
        .node("creator")
        .set_own_person_name(workspace, "Valid")
        .unwrap();
    cluster
        .observe_until(
            "valid name after malformed edit",
            DEFAULT_DEADLINE,
            |cluster| {
                cluster
                    .node("member")
                    .workspace_person(workspace, cluster.identity("creator"))
                    .unwrap()
                    .and_then(|person| person.name().map(str::to_owned))
                    .as_deref()
                    == Some("Valid")
            },
        )
        .await;
    let mut wake = cluster
        .node("creator")
        .subscribe_person_names(workspace)
        .unwrap();
    let (before, _) = cluster
        .node("creator")
        .person_name_snapshot(workspace)
        .unwrap();
    cluster
        .node("creator")
        .revoke_member(workspace, cluster.identity("member"))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), wake.changed())
        .await
        .unwrap()
        .unwrap();
    let (after, _) = cluster
        .node("creator")
        .person_name_snapshot(workspace)
        .unwrap();
    assert!(
        after > before,
        "revocation invalidates the durable name snapshot cursor"
    );
    assert!(
        cluster
            .node("creator")
            .workspace_person(workspace, cluster.identity("member"))
            .unwrap()
            .is_none()
    );
    assert!(
        !cluster
            .node("creator")
            .person_name_snapshot(workspace)
            .unwrap()
            .1
            .iter()
            .any(|(id, _)| *id == cluster.identity("member"))
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_workspace_bootstrap_import_advances_generation_atomically_and_wakes_subscriber() {
    let mut cluster =
        SyncCluster::start("names-bootstrap-retry", &["creator", "joiner", "newcomer"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    cluster.node("creator").sync_worker.shutdown().await;
    cluster.node("joiner").sync_worker.shutdown().await;
    cluster.node("newcomer").sync_worker.shutdown().await;
    let now = crate::runtime::Clock::now_unix_ms(&super::super::SystemClock);
    let invitation = cluster
        .node("creator")
        .create_workspace_invitation_for_test(workspace, now, now + 600_000)
        .unwrap();
    cluster
        .node("joiner")
        .join_workspace(&invitation)
        .await
        .unwrap();
    let joiner = cluster.node("joiner");
    let wake = joiner.subscribe_person_names(workspace).unwrap();
    let (before, members) = joiner.person_name_snapshot(workspace).unwrap();
    assert!(
        !members
            .iter()
            .any(|(id, _)| *id == cluster.identity("newcomer"))
    );
    cluster.admit("creator", "newcomer", workspace).await;
    {
        let state = joiner.state.lock().unwrap();
        state.connection.execute_batch("CREATE TRIGGER fail_name_generation_insert BEFORE INSERT ON principal_name_generations BEGIN SELECT RAISE(FAIL, 'injected generation failure'); END; CREATE TRIGGER fail_name_generation_update BEFORE UPDATE ON principal_name_generations BEGIN SELECT RAISE(FAIL, 'injected generation failure'); END;").unwrap();
    }
    assert!(joiner.join_workspace(&invitation).await.is_err());
    assert_eq!(joiner.person_name_snapshot(workspace).unwrap().0, before);
    assert!(!wake.has_changed().unwrap());
    assert!(
        !cluster
            .members("joiner", workspace)
            .contains(&cluster.identity("newcomer"))
    );
    cluster.stop("joiner").await;
    cluster.restart("joiner").await;
    let joiner = cluster.node("joiner");
    assert_eq!(joiner.person_name_snapshot(workspace).unwrap().0, before);
    assert!(
        !cluster
            .members("joiner", workspace)
            .contains(&cluster.identity("newcomer"))
    );
    {
        let state = joiner.state.lock().unwrap();
        state.connection.execute_batch("DROP TRIGGER fail_name_generation_insert; DROP TRIGGER fail_name_generation_update;").unwrap();
    }
    let mut wake = joiner.subscribe_person_names(workspace).unwrap();
    joiner.join_workspace(&invitation).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), wake.changed())
        .await
        .unwrap()
        .unwrap();
    let (after, people) = joiner.person_name_snapshot(workspace).unwrap();
    assert!(after > before);
    assert!(
        people
            .iter()
            .any(|(id, _)| *id == cluster.identity("newcomer"))
    );
    cluster.stop("joiner").await;
    cluster.restart("joiner").await;
    let (durable, people) = cluster
        .node("joiner")
        .person_name_snapshot(workspace)
        .unwrap();
    assert_eq!(durable, after);
    assert!(
        people
            .iter()
            .any(|(id, _)| *id == cluster.identity("newcomer"))
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn membership_generation_write_failure_rolls_back_local_and_remote_revocations() {
    use crate::membership_sync::ReplicaError;

    let mut cluster = SyncCluster::start("names-membership-atomic", &["creator", "member"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    cluster.admit("creator", "member", workspace).await;
    cluster.node("creator").sync_worker.shutdown().await;
    cluster.node("member").sync_worker.shutdown().await;
    let creator = cluster.node("creator");
    let member = cluster.identity("member");
    let (before, _) = creator.person_name_snapshot(workspace).unwrap();
    let version = creator.membership_version(workspace).unwrap();
    let envelope = {
        let state = creator.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        crate::membership::MembershipEnvelope::sign(
            &state.secret_key,
            workspace,
            creator.clock.now_unix_ms(),
            crate::membership::MembershipAction::Revoke {
                target: member,
                group: projection.person_group(member).unwrap(),
            },
            projection.heads(),
        )
        .unwrap()
    };
    {
        let state = creator.state.lock().unwrap();
        state.connection.execute_batch("CREATE TRIGGER fail_name_generation_insert BEFORE INSERT ON principal_name_generations BEGIN SELECT RAISE(FAIL, 'injected generation failure'); END; CREATE TRIGGER fail_name_generation_update BEFORE UPDATE ON principal_name_generations BEGIN SELECT RAISE(FAIL, 'injected generation failure'); END;").unwrap();
    }
    assert!(creator.revoke_member(workspace, member).is_err());
    assert_eq!(creator.person_name_snapshot(workspace).unwrap().0, before);
    assert_eq!(creator.membership_version(workspace).unwrap(), version);
    // Exercise the independent remote replica apply entry point with the same failure.
    let replica = creator.membership_replica.clone();
    assert_eq!(
        replica.apply(member, workspace, envelope.canonical_bytes().to_vec()),
        Err(ReplicaError::Storage)
    );
    assert_eq!(creator.membership_version(workspace).unwrap(), version);
    cluster.stop("creator").await;
    cluster.restart("creator").await;
    let creator = cluster.node("creator");
    assert_eq!(creator.membership_version(workspace).unwrap(), version);
    assert_eq!(creator.person_name_snapshot(workspace).unwrap().0, before);
    {
        let state = creator.state.lock().unwrap();
        state
            .connection
            .execute_batch("DROP TRIGGER fail_name_generation_insert; DROP TRIGGER fail_name_generation_update;")
            .unwrap();
    }
    assert_eq!(
        replica.apply(member, workspace, envelope.canonical_bytes().to_vec()),
        Err(ReplicaError::Unavailable)
    );
    creator
        .apply_membership_envelope(envelope.canonical_bytes())
        .unwrap();
    let (after, members) = creator.person_name_snapshot(workspace).unwrap();
    assert!(after > before);
    assert!(!members.iter().any(|(id, _)| *id == member));
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_self_rename_wakes_subscriber_when_private_initialization_fails() {
    let cluster = SyncCluster::start("names-rename-init-failure", &["creator"]).await;
    let creator = cluster.node("creator");
    let workspace = creator.create_workspace("Names").unwrap().id();
    creator.sync_worker.shutdown().await;
    let mut wake = creator.subscribe_person_names(workspace).unwrap();
    let (before, _) = creator.person_name_snapshot(workspace).unwrap();
    {
        let mut state = creator.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        let person = crate::principals::derive(
            workspace,
            crate::principals::anchor(projection, creator.public_identity()).unwrap(),
        );
        let secret = state.secret_key.clone();
        let signed = crate::principals::edit::PrincipalNameEdit::sign(
            &secret, workspace, person, 1, "Older",
        )
        .unwrap();
        crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            crate::synchronization::reserved_principals_scope(),
            "principal-name.edit",
            signed.canonical_bytes(),
            crate::synchronization::MembershipStanding::Current,
            true,
            creator.clock.now_unix_ms(),
        )
        .unwrap();
        state.person_name_open_error_once = true;
    }
    assert!(creator.set_own_person_name(workspace, "Committed").is_err());
    tokio::time::timeout(Duration::from_secs(1), wake.changed())
        .await
        .unwrap()
        .unwrap();
    let (after, people) = creator.person_name_snapshot(workspace).unwrap();
    assert!(after > before);
    assert_eq!(people[0].1.name(), Some("Committed"));
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_principals_scope_can_be_initialized_and_demanded_privately() {
    let cluster = SyncCluster::start("private-person-names-scope", &["creator", "joiner"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Names")
        .unwrap()
        .id();
    let reserved = crate::synchronization::reserved_principals_scope();
    assert_eq!(reserved, "internal:teamarness.principals");
    let private_record = {
        let creator = cluster.node("creator");
        let mut state = creator.state.lock().unwrap();
        let secret = state.secret_key.clone();
        assert!(
            !crate::synchronization::declare_creator_scope(
                &mut state.connection,
                &secret,
                workspace,
                reserved,
            )
            .unwrap(),
            "creation already initialized the private scope"
        );
        crate::synchronization::append(
            &mut state.connection,
            &secret,
            workspace,
            reserved,
            "name.edit",
            b"private codec probe",
            crate::synchronization::MembershipStanding::Current,
            true,
            creator.clock.now_unix_ms(),
        )
        .unwrap()
        .0
    };
    cluster.node("creator").sync_worker.wake();
    cluster.admit("creator", "joiner", workspace).await;
    let joiner = cluster.node("joiner");
    for node in [cluster.node("creator"), joiner] {
        assert!(matches!(
            node.append_record(workspace, reserved, "name.edit", b"forged"),
            Err(crate::SynchronizationError::InvalidInput)
        ));
        assert!(matches!(
            node.subscribe_records(workspace, reserved, None),
            Err(crate::SynchronizationError::InvalidInput)
        ));
        assert!(matches!(
            node.open_records(workspace, reserved, &[]),
            Err(crate::SynchronizationError::InvalidInput)
        ));
    }
    {
        let joiner = cluster.node("joiner");
        let state = joiner.state.lock().unwrap();
        assert!(
            !crate::synchronization::declare_member_key_demand(
                &state.connection,
                workspace,
                reserved,
            )
            .unwrap(),
            "join import already demanded the private key"
        );
        joiner.sync_worker.wake();
    }
    cluster
        .observe_until(
            "the joiner receives the private scope key",
            Duration::from_secs(20),
            |cluster| {
                let state = cluster.node("joiner").state.lock().unwrap();
                let scope = crate::synchronization::scope_digest(reserved, workspace);
                state
                    .connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sync_scopes WHERE scope_id=?1)",
                        [scope.as_slice()],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap()
            },
        )
        .await;
    cluster
        .observe_until(
            "the joiner receives the protected private record",
            Duration::from_secs(20),
            |cluster| cluster.record_count("joiner", workspace, reserved) == 1,
        )
        .await;
    {
        let joiner = cluster.node("joiner");
        let state = joiner.state.lock().unwrap();
        assert_eq!(
            crate::synchronization::open_records(
                &state.connection,
                workspace,
                reserved,
                &[private_record.sealed_record().to_vec()],
                joiner.clock.now_unix_ms(),
            )
            .unwrap()[0]
                .canonical(),
            b"private codec probe"
        );
    }
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prompt_scope_key_acquisition_without_holder_sweep() {
    const SCOPE: &str = "teamarness.example.empty-scope";
    let cluster = SyncCluster::start("prompt-scope-key-demand", &["holder", "member"]).await;
    let workspace = cluster
        .node("holder")
        .create_workspace("Empty scope")
        .unwrap();
    let id = workspace.id();
    let _creator = cluster
        .node("holder")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    assert_eq!(cluster.record_count("holder", id, SCOPE), 0);
    // Only the requester can initiate after admission; the holder's outbound
    // scheduler cannot accidentally mask a missing demand wake.
    cluster.node("holder").pause_sync_scheduler_for_test(true);
    cluster.admit("holder", "member", id).await;
    let mut member = cluster
        .node("member")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    {
        let state = cluster.node("member").state.lock().unwrap();
        let pending: i64 = state.connection.query_row(
            "SELECT COUNT(*) FROM sync_scope_key_demands WHERE workspace_id=?1 AND package_identity=?2",
            rusqlite::params![id.as_bytes().as_slice(), SCOPE], |row| row.get(0)
        ).unwrap();
        assert_eq!(pending, 1, "subscription declares genuine pending demand");
    }
    cluster
        .observe_until(
            "the member receives a verified key before record idle",
            Duration::from_secs(20),
            |_| member.availability().unwrap().availability == crate::SyncAvailability::Current,
        )
        .await;
    {
        let state = cluster.node("member").state.lock().unwrap();
        let scope = crate::synchronization::scope_digest(SCOPE, id);
        let held: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let pending: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scope_key_demands WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            (held, pending),
            (1, 0),
            "only verified import replaces demand with a genuine scope"
        );
    }
    assert_eq!(
        cluster.record_count("holder", id, SCOPE),
        0,
        "the holder authored no Package record before the Member's first append"
    );
    let first = cluster
        .node("member")
        .append_record(id, SCOPE, "message.post", b"first")
        .unwrap();
    assert!(
        cluster
            .record_ids("member", id, SCOPE)
            .contains(&first.record_id())
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R08, TEAM.SYS.CONV-R12
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prompt_scope_key_acquisition_holder_push_reaches_current() {
    const SCOPE: &str = "teamarness.example.holder-push";
    let cluster = SyncCluster::start("holder-push-health", &["holder", "member"]).await;
    let id = cluster
        .node("holder")
        .create_workspace("Empty scope")
        .unwrap()
        .id();
    let _creator = cluster
        .node("holder")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    cluster.node("member").pause_sync_scheduler_for_test(true);
    cluster.admit("holder", "member", id).await;
    let mut member = cluster
        .node("member")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    cluster
        .observe_until(
            "the holder grants the key in an authenticated outbound session",
            Duration::from_secs(10),
            |c| {
                let state = c.node("member").state.lock().unwrap();
                let scope = crate::synchronization::scope_digest(SCOPE, id);
                let held: i64 = state
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM sync_scopes WHERE scope_id=?1",
                        [scope.as_slice()],
                        |row| row.get(0),
                    )
                    .unwrap();
                held == 1 && !c.node("holder").session_reports_for_test().is_empty()
            },
        )
        .await;
    cluster
        .observe_until(
            "the completed authenticated inbound session reports reachable and current",
            Duration::from_secs(10),
            |c| {
                let scope = crate::synchronization::scope_digest(SCOPE, id);
                let reachable = c
                    .node("member")
                    .health
                    .snapshot(scope)
                    .last_outcome
                    .get(&c.identity("holder"))
                    == Some(&crate::synchronization::PeerOutcome::Reachable);
                reachable
                    && member.availability().unwrap().availability
                        == crate::SyncAvailability::Current
            },
        )
        .await;
    assert_eq!(cluster.record_count("holder", id, SCOPE), 0);
    cluster.finish().await;
}

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scope_key_demand_restart_load_acquires_without_new_subscription() {
    const SCOPE: &str = "teamarness.example.restart-key";
    let mut cluster = SyncCluster::start("demand-restart-load", &["a", "b"]).await;
    let workspace = cluster
        .node("a")
        .create_workspace("Restart demand")
        .unwrap();
    let id = workspace.id();
    cluster.node("a").pause_sync_scheduler_for_test(true);
    cluster.admit("a", "b", id).await;
    cluster.node("b").pause_sync_scheduler_for_test(true);
    let _waiting = cluster
        .node("b")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    cluster.stop("b").await;
    let _creator = cluster
        .node("a")
        .subscribe_records(id, SCOPE, None)
        .unwrap();
    cluster.restart("b").await;
    // No second subscription, explicit replication, or forced sweep: startup
    // must load the durable pending row, and the paused holder cannot dial.
    cluster
        .observe_until(
            "restarted requester acquires key",
            Duration::from_secs(20),
            |c| {
                let state = c.node("b").state.lock().unwrap();
                let scope = crate::synchronization::scope_digest(SCOPE, id);
                let held: i64 = state
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM sync_scopes WHERE scope_id=?1",
                        [scope.as_slice()],
                        |row| row.get(0),
                    )
                    .unwrap();
                held == 1
            },
        )
        .await;
    cluster.finish().await;
}

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scope_key_demand_only_verified_matching_grant_retires_pending() {
    const OTHER: &str = "teamarness.example.other";
    let cluster = SyncCluster::start("verified-demand", &["a", "b", "c"]).await;
    for name in ["a", "b", "c"] {
        cluster.node(name).pause_sync_scheduler_for_test(true);
    }
    let workspace = cluster
        .node("a")
        .create_workspace("Verified grants")
        .unwrap();
    let id = workspace.id();
    let _creator = cluster
        .node("a")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let _other = cluster
        .node("a")
        .subscribe_records(id, OTHER, None)
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let _waiting = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let scope = crate::synchronization::scope_digest(PACKAGE, id);
    let pending = || {
        let state = cluster.node("b").state.lock().unwrap();
        state
            .connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM sync_scope_key_demands WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(pending(), 1);
    let (origin, grant) = cluster
        .node("a")
        .export_protection_bundle_for_test(id, PACKAGE, cluster.identity("b"))
        .unwrap();
    let (_, other_grant) = cluster
        .node("a")
        .export_protection_bundle_for_test(id, OTHER, cluster.identity("b"))
        .unwrap();
    assert!(
        cluster
            .node("b")
            .import_protection_grant(id, PACKAGE, &origin, &other_grant)
            .is_err()
    );
    assert_eq!(pending(), 1, "wrong-scope grant retains demand");
    let (_, wrong_recipient) = cluster
        .node("a")
        .export_protection_bundle_for_test(id, PACKAGE, cluster.identity("c"))
        .unwrap();
    assert!(
        cluster
            .node("b")
            .import_protection_grant(id, PACKAGE, &origin, &wrong_recipient)
            .is_err()
    );
    assert_eq!(pending(), 1, "wrong-recipient grant retains demand");
    let b_secret = cluster.node("b").state.lock().unwrap().secret_key.clone();
    let (forged_origin, forged_grant) = crate::synchronization::forge_protection_bundle_for_test(
        &b_secret,
        id,
        PACKAGE,
        cluster.identity("b"),
    );
    assert_eq!(
        cluster
            .node("b")
            .import_protection_grant(id, PACKAGE, &forged_origin, &forged_grant)
            .unwrap_err(),
        crate::SynchronizationError::KeyOriginRejected
    );
    assert_eq!(pending(), 1, "wrong-origin grant retains demand");
    cluster
        .node("b")
        .import_protection_grant(id, PACKAGE, &origin, &grant)
        .unwrap();
    assert_eq!(pending(), 0, "verified grant retires demand at commit");
    {
        let state = cluster.node("b").state.lock().unwrap();
        state.connection.execute(
            "INSERT INTO sync_scope_key_demands(scope_id,workspace_id,package_identity,demand_generation) VALUES(?1,?2,?3,1)",
            rusqlite::params![scope.as_slice(), id.as_bytes().as_slice(), PACKAGE],
        ).unwrap();
    }
    cluster
        .node("b")
        .import_protection_grant(id, PACKAGE, &origin, &grant)
        .unwrap();
    assert_eq!(
        pending(),
        0,
        "verified reimport cleans an already-held scope demand"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_member_is_refused_on_every_sync_alpn() {
    let cluster = SyncCluster::start("non-member-refused", &["a", "x"]).await;
    let workspace = cluster.node("a").create_workspace("Gate").unwrap();
    let id = workspace.id();
    let seed = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();

    // Package records.
    let outcome = cluster
        .node("x")
        .probe_package_sync_for_test(cluster.address("a"), id, PACKAGE, cluster.identity("a"))
        .await;
    assert_eq!(outcome, ProbeOutcome::Refused(Refusal::NotAuthorized));

    // Canonical membership.
    let (operations, refused) = cluster
        .node("x")
        .probe_membership_sync_for_test(cluster.address("a"), id, Vec::new())
        .await;
    assert!(
        operations.is_empty(),
        "a non-member is served no membership"
    );
    assert!(refused, "a non-member is refused on membership sync");

    // The retired batch ingest path.
    assert!(
        !cluster
            .node("x")
            .can_connect_alpn_for_test(cluster.address("a"), b"teamarness/package-sync-batch/1")
            .await,
        "the retired batch ingest ALPN must not be routed"
    );
    assert!(
        cluster
            .node("x")
            .can_connect_alpn_for_test(cluster.address("a"), b"teamarness/membership-sync/1")
            .await,
        "membership sync is offered; the refusal is inside the protocol"
    );

    assert_eq!(
        cluster.record_ids("a", id, PACKAGE),
        BTreeSet::from([seed.record_id()]),
        "a refused session must not change the serving installation's scope"
    );
    assert!(
        cluster.record_ids("x", id, PACKAGE).is_empty(),
        "a non-member must receive nothing"
    );
    assert!(
        cluster.members("x", id).is_empty(),
        "a non-member must learn no roster"
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_receives_content_key_in_the_handshake_from_either_side() {
    let cluster = SyncCluster::start("handshake-key", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Handshake").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();

    // The joiner dials: the key arrives in the responder's Welcome.
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    let b1 = cluster
        .node("b")
        .append_record(id, PACKAGE, "message.post", b"b1")
        .unwrap();
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("b", id, PACKAGE)
            .contains(&a1.record_id())
    );
    assert!(
        cluster
            .record_ids("a", id, PACKAGE)
            .contains(&b1.record_id())
    );

    // The holder dials: the key arrives in the initiator's Ready.
    cluster.admit("a", "c", id).await;
    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("c"),
            cluster.identity("c"),
        )
        .await
        .unwrap();
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("c", id, PACKAGE)
            .contains(&a1.record_id())
    );
    assert!(
        cluster
            .record_ids("a", id, PACKAGE)
            .contains(&c1.record_id())
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_revocation_cancels_a_live_session() {
    let cluster = SyncCluster::start("revocation-cancels", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Cancel").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;

    // The responder stalls inside its own gate, so the session is live but
    // cannot finish while the revocation is committed.
    let hold = cluster.node("b").hold_runtime_state_for_test();
    let result = {
        let session = cluster.node("a").replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        );
        tokio::pin!(session);
        let settled = tokio::select! {
            result = &mut session => Some(result),
            // negative-wait: a bounded window proving the session has not finished.
            () = tokio::time::sleep(Duration::from_millis(300)) => None,
        };
        assert!(settled.is_none(), "the session must still be running");

        cluster
            .node("a")
            .revoke_member(id, cluster.identity("b"))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), session)
            .await
            .expect("a revocation cancels the live session within two seconds")
    };
    assert_eq!(result.unwrap_err(), LogSessionError::Cancelled);
    hold.release();
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_member_is_refused_by_the_gate() {
    let cluster = SyncCluster::start("revoked-refused", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Revoked").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();

    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    let after = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"after-revocation")
        .unwrap();

    // B has not yet learned of its own revocation, so its local gate still
    // lets it dial. A's gate is the one that must refuse.
    let refused = cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await;
    assert_eq!(
        refused.unwrap_err(),
        LogSessionError::Refused(Refusal::NotAuthorized)
    );
    assert!(
        !cluster
            .record_ids("b", id, PACKAGE)
            .contains(&after.record_id()),
        "a revoked member must not receive records written after its revocation"
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_worker_converges_two_members_and_catches_up_after_a_restart() {
    let mut cluster = SyncCluster::start("worker-converges", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Worker").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    assert_eq!(
        cluster.members("a", id),
        cluster.members("b", id),
        "the bootstrap import gives the joiner the manager's roster"
    );

    cluster
        .wait_until("the joiner has the first record", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&a1.record_id())
        })
        .await;

    cluster.stop("b").await;
    let a2 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a2")
        .unwrap();
    cluster.restart("b").await;
    cluster
        .wait_until("the restarted member catches up", DEFAULT_DEADLINE, |c| {
            c.record_ids("b", id, PACKAGE).contains(&a2.record_id())
        })
        .await;
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_with_a_member_minted_origin_is_rejected() {
    let cluster = SyncCluster::start("minted-origin", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Origin").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;

    // B invents its own key and signs both halves of a bundle for C. Every
    // signature is genuine; only the creator-origin rule refuses it.
    let b_secret = cluster.node("b").state.lock().unwrap().secret_key.clone();
    let (forged_origin, forged_grant) = crate::synchronization::forge_protection_bundle_for_test(
        &b_secret,
        id,
        PACKAGE,
        cluster.identity("c"),
    );
    assert_eq!(
        cluster
            .node("c")
            .import_protection_grant(id, PACKAGE, &forged_origin, &forged_grant)
            .unwrap_err(),
        crate::SynchronizationError::KeyOriginRejected
    );

    let (origin, grant) = cluster
        .node("a")
        .export_protection_bundle_for_test(id, PACKAGE, cluster.identity("c"))
        .unwrap();
    cluster
        .node("c")
        .import_protection_grant(id, PACKAGE, &origin, &grant)
        .unwrap();
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("c", id, PACKAGE)
            .contains(&a1.record_id())
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_creator_holder_welcomes_a_new_member() {
    let mut cluster = SyncCluster::start("non-creator-holder", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Holder").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    cluster.admit("a", "c", id).await;
    cluster.replicate_membership("a", "b", id).await;
    cluster.stop("a").await;

    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .unwrap();
    assert!(
        cluster
            .record_ids("c", id, PACKAGE)
            .contains(&a1.record_id())
    );
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    cluster
        .wait_until(
            "the holder receives the newcomer's record",
            DEFAULT_DEADLINE,
            |c| c.record_ids("b", id, PACKAGE).contains(&c1.record_id()),
        )
        .await;
    cluster.finish().await;
}

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_key_demand_subscription_and_append_coalesce_across_restart() {
    let mut cluster = SyncCluster::start("member-key-demand-restart", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Demand").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("a").await;

    let count = |cluster: &SyncCluster| {
        let state = cluster.node("b").state.lock().unwrap();
        let demands: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scope_key_demands WHERE workspace_id=?1 AND package_identity=?2",
                rusqlite::params![id.as_bytes().as_slice(), PACKAGE],
                |row| row.get(0),
            )
            .unwrap();
        let scopes: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scopes WHERE workspace_id=?1 AND package_identity=?2",
                rusqlite::params![id.as_bytes().as_slice(), PACKAGE],
                |row| row.get(0),
            )
            .unwrap();
        (demands, scopes)
    };
    assert_eq!(
        count(&cluster),
        (0, 0),
        "a Package that does not subscribe has no demand"
    );
    let _first = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let _second = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert_eq!(
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", b"blocked")
            .unwrap_err(),
        crate::SynchronizationError::ContentKeyUnavailable
    );
    assert_eq!(
        count(&cluster),
        (1, 0),
        "subscription and failed append share one key-less row"
    );
    cluster.stop("b").await;
    cluster.restart("b").await;
    assert_eq!(
        count(&cluster),
        (1, 0),
        "demand survives a restart without creating a key"
    );
    cluster.finish().await;
}

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_key_demand_append_fallback_does_not_depend_on_subscription() {
    let mut cluster = SyncCluster::start("member-key-demand-fallback", &["a", "b", "x"]).await;
    let workspace = cluster
        .node("a")
        .create_workspace("Append fallback")
        .unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("a").await;

    // An unknown Installation can still open a waiting stream, but cannot
    // create a demand. It cannot use an append as a back door either.
    let _unknown = cluster
        .node("x")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert_eq!(
        cluster
            .node("x")
            .append_record(id, PACKAGE, "message.post", b"no")
            .unwrap_err(),
        crate::SynchronizationError::NotCurrentMember
    );
    {
        let unknown_state = cluster.node("x").state.lock().unwrap();
        let unknown: i64 = unknown_state
            .connection
            .query_row("SELECT COUNT(*) FROM sync_scope_key_demands", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(unknown, 0);
    }

    for _ in 0..2 {
        assert_eq!(
            cluster
                .node("b")
                .append_record(id, PACKAGE, "message.post", b"blocked")
                .unwrap_err(),
            crate::SynchronizationError::ContentKeyUnavailable
        );
    }
    {
        let state = cluster.node("b").state.lock().unwrap();
        let demand: (i64, i64) = state.connection.query_row(
            "SELECT COUNT(*), MIN(demand_generation) FROM sync_scope_key_demands WHERE workspace_id=?1 AND package_identity=?2",
            rusqlite::params![id.as_bytes().as_slice(), PACKAGE],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        let scopes: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scopes WHERE package_identity=?1",
                [PACKAGE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            demand,
            (1, 1),
            "blocked appends share an initially due demand"
        );
        assert_eq!(
            scopes, 0,
            "a Member does not mint a key or a placeholder scope"
        );
    }
    cluster.finish().await;
}

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R08
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_key_demand_local_revocation_clears_pending_and_prevents_recreation() {
    let cluster = SyncCluster::start("member-key-demand-revoke", &["a", "b"]).await;
    let workspace = cluster
        .node("a")
        .create_workspace("Demand revocation")
        .unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    let _subscription = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    let count = || {
        let state = cluster.node("b").state.lock().unwrap();
        state
            .connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM sync_scope_key_demands WHERE workspace_id=?1 AND package_identity=?2",
                rusqlite::params![id.as_bytes().as_slice(), PACKAGE],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(count(), 1);
    {
        let state = cluster.node("b").state.lock().unwrap();
        let scope = crate::synchronization::scope_digest(PACKAGE, id);
        state.connection.execute(
            "INSERT INTO sync_scope_key_demand_peer_progress(scope_id,peer_public_identity,consumed_generation) VALUES(?1,?2,0)",
            rusqlite::params![scope.as_slice(), cluster.identity("a").as_bytes().as_slice()],
        ).unwrap();
    }
    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    cluster.replicate_membership("b", "a", id).await;
    assert_eq!(count(), 0, "accepted local revocation removes demand");
    {
        let state = cluster.node("b").state.lock().unwrap();
        let progress: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_scope_key_demand_peer_progress",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(progress, 0, "revocation cascades peer progress");
    }
    let _waiting = cluster
        .node("b")
        .subscribe_records(id, PACKAGE, None)
        .unwrap();
    assert_eq!(
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", b"blocked")
            .unwrap_err(),
        crate::SynchronizationError::RevokedMember
    );
    assert_eq!(
        count(),
        0,
        "revoked subscription and append cannot recreate demand"
    );
    cluster.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_without_key_gets_content_key_unavailable() {
    let mut cluster = SyncCluster::start("no-key-yet", &["a", "b"]).await;
    let workspace = cluster.node("a").create_workspace("No key").unwrap();
    let id = workspace.id();
    cluster.admit("a", "b", id).await;
    cluster.stop("a").await;

    assert_eq!(
        cluster
            .node("b")
            .append_record(id, PACKAGE, "message.post", b"b1")
            .unwrap_err(),
        crate::SynchronizationError::ContentKeyUnavailable
    );
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.MEM-R10
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_from_an_unknown_author_are_applied_after_membership_replicates() {
    let mut cluster = SyncCluster::start("parked-author", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Parking").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    cluster.stop("b").await;

    // C is admitted while B is away, so B has never heard of C.
    cluster.admit("a", "c", id).await;
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();
    let c2 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c2")
        .unwrap();
    cluster
        .node("c")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();
    cluster.stop("c").await;
    cluster.restart("b").await;
    // Hold B's membership replication so the forwarded records arrive before
    // the admission that makes them applicable.
    cluster
        .node("b")
        .pause_membership_replication_for_test(true);

    cluster
        .node("a")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("b"),
            cluster.identity("b"),
        )
        .await
        .unwrap();
    assert_eq!(
        cluster.node("b").parked_count_for_test(id, PACKAGE),
        2,
        "both of the unknown author's records wait instead of aborting the session"
    );
    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(
        seen.contains(&a1.record_id()),
        "the known author still applies"
    );
    assert!(!seen.contains(&c1.record_id()));

    // Replication delivers the admission, which promotes what was parked.
    cluster
        .node("b")
        .pause_membership_replication_for_test(false);
    cluster
        .wait_until(
            "the parked records are applied after membership replicates",
            DEFAULT_DEADLINE,
            |c| {
                let seen = c.record_ids("b", id, PACKAGE);
                seen.contains(&c1.record_id()) && seen.contains(&c2.record_id())
            },
        )
        .await;
    assert!(cluster.members("b", id).contains(&cluster.identity("c")));
    assert_eq!(cluster.node("b").parked_count_for_test(id, PACKAGE), 0);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_scope_record_is_rejected_without_ending_the_session() {
    const OTHER: &str = "teamarness.other";
    let cluster = SyncCluster::start("forged-scope", &["a", "m", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Forgery").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, OTHER, "message.post", b"a-other")
        .unwrap();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "m", id).await;
    cluster.admit("a", "b", id).await;
    // Each joiner learned the roster as it stood when it joined; one membership
    // session each gives both the manager's current view, so either may dial
    // the other.
    cluster.replicate_membership("a", "m", id).await;
    cluster.replicate_membership("a", "b", id).await;
    for package in [OTHER, PACKAGE] {
        cluster
            .node("m")
            .replicate_records_to_address_for_test(
                id,
                package,
                cluster.address("a"),
                cluster.identity("a"),
            )
            .await
            .unwrap();
    }
    cluster
        .node("b")
        .replicate_records_to_address_for_test(
            id,
            PACKAGE,
            cluster.address("a"),
            cluster.identity("a"),
        )
        .await
        .unwrap();

    // M writes to one scope and re-seals that record into another it also holds.
    let m_other = cluster
        .node("m")
        .append_record(id, OTHER, "message.post", b"m-other")
        .unwrap();
    let forged = {
        let state = cluster.node("m").state.lock().unwrap();
        crate::synchronization::forge_cross_scope_operation_for_test(
            &state.connection,
            &state.secret_key,
            id,
            OTHER,
            PACKAGE,
            m_other.record_id(),
        )
    };

    // This test injects one raw session, not a scheduler job with Busy backoff.
    // Establish ownership of the responder slots before testing record handling.
    for name in ["a", "m", "b"] {
        cluster.node(name).pause_sync_scheduler_for_test(true);
    }
    cluster
        .wait_until(
            "raw injection has no competing sessions",
            DEFAULT_DEADLINE,
            |c| {
                ["a", "m", "b"].iter().all(|name| {
                    c.node(name)
                        .scheduler_snapshot_for_test()
                        .iter()
                        .all(|peer| !peer.in_flight)
                        && c.node(name).inbound_package_sessions_for_test() == 0
                })
            },
        )
        .await;
    // Deterministically distinguish pre-Hello Busy from a record-handling refusal.
    let first = cluster
        .node("m")
        .hold_package_sync_before_hello_for_test(cluster.address("b"))
        .await;
    let second = cluster
        .node("m")
        .hold_package_sync_before_hello_for_test(cluster.address("b"))
        .await;
    cluster
        .wait_until(
            "test owns both raw injection responder slots",
            DEFAULT_DEADLINE,
            |c| c.node("b").inbound_package_sessions_for_test() == 2,
        )
        .await;
    let refused = cluster
        .node("m")
        .deliver_operations_for_test(
            cluster.address("b"),
            id,
            PACKAGE,
            cluster.identity("b"),
            vec![forged.clone()],
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(crate::synchronization::LogSessionError::PeerBusy)
        ),
        "full responder slots refuse before record handling: {refused:?}"
    );
    drop((first, second));
    cluster
        .wait_until(
            "raw injection responder slots are released",
            DEFAULT_DEADLINE,
            |c| c.node("b").inbound_package_sessions_for_test() == 0,
        )
        .await;

    let applied = cluster
        .node("m")
        .deliver_operations_for_test(
            cluster.address("b"),
            id,
            PACKAGE,
            cluster.identity("b"),
            vec![forged],
        )
        .await
        .expect("one forged record must not end the session");
    assert_eq!(
        applied.outcomes.applied, 0,
        "the forging peer had nothing new to receive"
    );

    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(
        seen.contains(&a1.record_id()),
        "the genuine record in the same session still applies"
    );
    assert!(
        !seen.contains(&m_other.record_id()),
        "a record signed for another package never opens in this scope"
    );
    assert_eq!(cluster.node("b").parked_count_for_test(id, PACKAGE), 0);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.MEM-R06, TEAM.SYS.TRANS-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_member_cannot_pull_new_records_from_any_member() {
    let cluster = SyncCluster::start("revoked-pulls-nothing", &["a", "b", "c"]).await;
    let workspace = cluster.node("a").create_workspace("Revoked pull").unwrap();
    let id = workspace.id();
    let a0 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a0")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "c", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("c"),
    ]);
    cluster
        .wait_until(
            "all three converge on the seed record",
            DEFAULT_DEADLINE,
            |c| {
                c.members("b", id) == everyone
                    && c.members("c", id) == everyone
                    && c.record_ids("b", id, PACKAGE).contains(&a0.record_id())
                    && c.record_ids("c", id, PACKAGE).contains(&a0.record_id())
            },
        )
        .await;

    // B is held at its pre-revocation view, so its own gate still lets it dial.
    cluster
        .node("b")
        .pause_membership_replication_for_test(true);
    cluster
        .node("a")
        .revoke_member(id, cluster.identity("b"))
        .unwrap();
    let without_bob = BTreeSet::from([cluster.identity("a"), cluster.identity("c")]);
    cluster
        .wait_until(
            "the revocation reaches the third member",
            DEFAULT_DEADLINE,
            |c| c.members("c", id) == without_bob,
        )
        .await;

    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    let c1 = cluster
        .node("c")
        .append_record(id, PACKAGE, "message.post", b"c1")
        .unwrap();

    for _ in 0..10 {
        cluster.node("b").sync_now_for_test();
        // negative-wait: bounded rounds proving a revoked peer is served nothing.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(!seen.contains(&a1.record_id()));
    assert!(!seen.contains(&c1.record_id()));
    for holder in ["a", "c"] {
        assert_eq!(
            cluster
                .node("b")
                .probe_package_sync_for_test(
                    cluster.address(holder),
                    id,
                    PACKAGE,
                    cluster.identity(holder),
                )
                .await,
            ProbeOutcome::Refused(Refusal::NotAuthorized),
            "every remaining member refuses the revoked installation"
        );
    }

    // Once it can replicate again it learns its own revocation, and still gains
    // nothing written after it.
    cluster
        .node("b")
        .pause_membership_replication_for_test(false);
    cluster
        .wait_until(
            "the revoked member learns its own revocation",
            DEFAULT_DEADLINE,
            |c| !c.members("b", id).contains(&c.identity("b")),
        )
        .await;
    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(seen.contains(&a0.record_id()), "past data is retained");
    assert!(!seen.contains(&a1.record_id()));
    assert!(!seen.contains(&c1.record_id()));
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.MEM-R06
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_cancels_inbound_sessions_on_the_responder() {
    let cluster = SyncCluster::start("inbound-cancelled", &["a", "b", "x"]).await;
    let workspace = cluster.node("a").create_workspace("Inbound").unwrap();
    let id = workspace.id();
    cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("a", "b", id).await;
    cluster.admit("a", "x", id).await;
    let everyone = BTreeSet::from([
        cluster.identity("a"),
        cluster.identity("b"),
        cluster.identity("x"),
    ]);
    cluster
        .wait_until("both joiners hold the roster", DEFAULT_DEADLINE, |c| {
            c.members("b", id) == everyone && c.members("x", id) == everyone
        })
        .await;

    // X opens a session with B and then stops sending, so B is serving a live
    // session that nothing else would end. The revoking Installation is A, and
    // B learns of the revocation only through replication.
    let address = cluster.address("b");
    let revoked = cluster.identity("x");
    let close_code = {
        let stalled = cluster
            .node("x")
            .stalled_package_sync_close_code_for_test(address, DEFAULT_DEADLINE);
        tokio::pin!(stalled);
        let settled = tokio::select! {
            result = &mut stalled => Some(result),
            // negative-wait: a bounded window proving the inbound session is still open.
            () = tokio::time::sleep(Duration::from_millis(300)) => None,
        };
        assert!(settled.is_none(), "the inbound session must still be open");

        cluster.node("a").revoke_member(id, revoked).unwrap();
        stalled.await
    };

    assert_eq!(
        close_code,
        Some(4),
        "the responder closes the cancelled session with the invalid-or-cancelled code"
    );
    assert!(!cluster.members("b", id).contains(&cluster.identity("x")));
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_order_key_does_not_end_the_session() {
    let cluster = SyncCluster::start("forged-order-key", &["a", "m", "b"]).await;
    let workspace = cluster.node("a").create_workspace("Order keys").unwrap();
    let id = workspace.id();
    let a1 = cluster
        .node("a")
        .append_record(id, PACKAGE, "message.post", b"a1")
        .unwrap();
    cluster.admit("a", "m", id).await;
    cluster.admit("a", "b", id).await;
    cluster.replicate_membership("a", "m", id).await;
    cluster.replicate_membership("a", "b", id).await;
    for node in ["m", "b"] {
        cluster
            .node(node)
            .replicate_records_to_address_for_test(
                id,
                PACKAGE,
                cluster.address("a"),
                cluster.identity("a"),
            )
            .await
            .unwrap();
    }

    // M mints two operations whose order keys it has no right to: one naming
    // another record, one claiming a height far beyond B's own.
    let forged = {
        let state = cluster.node("m").state.lock().unwrap();
        let secret = state.secret_key.clone();
        let misbound = crate::synchronization::forge_order_key_operation_for_test(
            &state.connection,
            &secret,
            id,
            PACKAGE,
            b"m-misbound",
            |_| {
                crate::synchronization::OrderKey::from_height_and_id(
                    1,
                    crate::synchronization::RecordId::from_bytes_for_test([9; 32]),
                )
                .as_bytes()
            },
        );
        let too_high = crate::synchronization::forge_order_key_operation_for_test(
            &state.connection,
            &secret,
            id,
            PACKAGE,
            b"m-too-high",
            |record| {
                crate::synchronization::OrderKey::from_height_and_id(
                    crate::synchronization::limits::MAX_HEIGHT_ADVANCE + 2,
                    record,
                )
                .as_bytes()
            },
        );
        vec![misbound, too_high]
    };

    let deadline = std::time::Instant::now() + DEFAULT_DEADLINE;
    loop {
        match cluster
            .node("m")
            .deliver_operations_for_test(
                cluster.address("b"),
                id,
                PACKAGE,
                cluster.identity("b"),
                forged.clone(),
            )
            .await
        {
            Ok(_) => break,
            Err(LogSessionError::PeerBusy) if std::time::Instant::now() < deadline => {
                // poll-interval: bounded retry while the peer finishes its active session.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("two forged order keys must not end the session: {error}"),
        }
    }

    let seen = cluster.record_ids("b", id, PACKAGE);
    assert!(
        seen.contains(&a1.record_id()),
        "the genuine record still applies"
    );
    assert_eq!(seen.len(), 1, "neither forged record entered the journal");
    assert_eq!(cluster.node("b").parked_count_for_test(id, PACKAGE), 0);
    cluster.finish().await;
}

/// Verifies: TEAM.SYS.ID-R10, MEM-R02. An Agent key is a Member, but it lives on
/// its Steward's Installation: Protection Grants, key demands and sessions
/// address Installation keys only. Recipients and scheduled peers are subsets
/// of current Installations (a grant or session is not required for every key).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protection_grants_and_sessions_address_installation_keys_only() {
    use crate::membership::{AgentKeyProof, MembershipAction, MembershipEnvelope};

    let cluster = SyncCluster::start("no-grants-to-agents", &["creator", "member"]).await;
    let workspace = cluster
        .node("creator")
        .create_workspace("Agents")
        .unwrap()
        .id();
    cluster
        .node("creator")
        .append_record(workspace, PACKAGE, "message.post", b"seed")
        .unwrap();
    cluster.admit("creator", "member", workspace).await;

    // The Member adds an Agent key to its own key group.
    let agent_secret = iroh_base::SecretKey::from_bytes(&[77; 32]);
    let agent = crate::PublicIdentity::from_bytes(*agent_secret.public().as_bytes()).unwrap();
    let member_identity = cluster.identity("member");
    let add_agent = {
        let member = cluster.node("member");
        let state = member.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        let group = projection.person_group(member_identity).unwrap();
        MembershipEnvelope::sign(
            &state.secret_key,
            workspace,
            member.clock.now_unix_ms(),
            MembershipAction::AddAgentKey {
                group,
                agent,
                proof: AgentKeyProof::sign(&agent_secret, workspace, group, member_identity)
                    .unwrap(),
            },
            projection.heads(),
        )
        .unwrap()
    };
    assert!(
        cluster
            .node("member")
            .apply_membership_envelope(add_agent.canonical_bytes())
            .unwrap()
            .accepted
    );
    cluster
        .replicate_membership("member", "creator", workspace)
        .await;
    // A real endpoint authenticated with the Agent key must not be treated as
    // an Installation, even though its key is in the effective Member roster.
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(agent_secret.clone())
        .alpns(Vec::new())
        .relay_mode(iroh::RelayMode::Disabled)
        .clear_address_lookup()
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .bind()
        .await
        .unwrap();
    let creator = cluster.node("creator");
    let outcome = crate::runtime::test_hooks::probe_package_sync_as_key_for_test(
        &endpoint,
        cluster.address("creator"),
        creator,
        workspace,
        PACKAGE,
    )
    .await;
    assert_eq!(outcome, ProbeOutcome::Refused(Refusal::NotAuthorized));
    let (operations, refused) = crate::membership_sync::session::probe_membership_sync(
        &endpoint,
        cluster.address("creator"),
        workspace,
        Vec::new(),
    )
    .await;
    assert!(operations.is_empty());
    assert!(refused);
    let receiver = super::super::RuntimeSyncReceiver {
        state: std::sync::Arc::downgrade(&creator.state),
        clock: std::sync::Arc::clone(&creator.clock),
        demand: std::sync::Arc::clone(&creator.membership_demand),
        health: std::sync::Arc::clone(&creator.health),
    };
    use crate::synchronization::SyncReceiver;
    assert!(matches!(
        receiver.protection_bundle_for(workspace, PACKAGE, agent),
        Err(crate::synchronization::SyncRejection::NotMember)
    ));
    // Records flow both ways, so each side issues the other a Protection Grant.
    cluster
        .node("member")
        .replicate_records_to_address_for_test(
            workspace,
            PACKAGE,
            cluster.address("creator"),
            cluster.identity("creator"),
        )
        .await
        .unwrap();
    cluster
        .node("member")
        .append_record(workspace, PACKAGE, "message.post", b"from member")
        .unwrap();
    cluster
        .node("creator")
        .replicate_records_to_address_for_test(
            workspace,
            PACKAGE,
            cluster.address("member"),
            cluster.identity("member"),
        )
        .await
        .unwrap();

    let installations = BTreeSet::from([cluster.identity("creator"), member_identity]);
    for name in ["creator", "member"] {
        let node = cluster.node(name);
        let recipients = {
            let state = node.state.lock().unwrap();
            let projection = state.memberships.get(&workspace).unwrap();
            assert!(
                projection.contains_member(agent),
                "{name} counts the Agent as a Member"
            );
            assert_eq!(
                projection
                    .installation_members()
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                installations,
                "{name} addresses exactly the Installation keys"
            );
            let mut statement = state
                .connection
                .prepare(
                    "SELECT DISTINCT recipient_public_identity FROM protection_control_operations
                     WHERE operation_kind = 'grant'",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .unwrap()
                .map(|row| {
                    crate::PublicIdentity::from_bytes(row.unwrap().try_into().unwrap()).unwrap()
                })
                .collect::<BTreeSet<_>>()
        };
        assert!(!recipients.is_empty(), "{name} holds at least one grant");
        assert!(
            recipients.is_subset(&installations),
            "{name}: every grant recipient is an Installation key"
        );
        assert!(
            !recipients.contains(&agent),
            "{name}: no grant targets the Agent key"
        );
        let peers = node
            .scheduler_snapshot_for_test()
            .into_iter()
            .map(|peer| peer.peer)
            .collect::<BTreeSet<_>>();
        assert!(
            peers.is_subset(&installations),
            "{name}: the scheduler dials Installation keys only"
        );
        assert!(
            !peers.contains(&agent),
            "{name}: the Agent key is never a peer"
        );
    }

    // The creator's own Person group independently claims the same key. It
    // depends only on its group attachment, not on the member's Agent add.
    // The conflict must cancel any subscribed session for the effective key
    // rather than promote it as a second admission.
    let creator = cluster.node("creator");
    let attachment = creator
        .export_membership_operations_for_tests(workspace)
        .iter()
        .map(|bytes| crate::membership::authenticate(bytes).unwrap())
        .find(|operation| matches!(operation.action, MembershipAction::AttachPerson { .. }))
        .unwrap()
        .id;
    let session = creator.state.lock().unwrap().peer_sessions.subscribe(agent);
    let competing = {
        let state = creator.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        let group = projection
            .person_group(cluster.identity("creator"))
            .unwrap();
        MembershipEnvelope::sign(
            &state.secret_key,
            workspace,
            creator.clock.now_unix_ms(),
            MembershipAction::AddAgentKey {
                group,
                agent,
                proof: AgentKeyProof::sign(
                    &agent_secret,
                    workspace,
                    group,
                    cluster.identity("creator"),
                )
                .unwrap(),
            },
            vec![attachment],
        )
        .unwrap()
    };
    assert!(
        creator
            .apply_membership_envelope(competing.canonical_bytes())
            .unwrap()
            .accepted
    );
    assert!(
        session.has_changed().unwrap(),
        "conflict cancels the effective key's sessions"
    );
    cluster
        .replicate_membership("creator", "member", workspace)
        .await;
    for name in ["creator", "member"] {
        let node = cluster.node(name);
        let state = node.state.lock().unwrap();
        let projection = state.memberships.get(&workspace).unwrap();
        assert!(!projection.contains_member(agent));
        assert!(!projection.installation_members().contains(&agent));
        let grants: i64 = state.connection.query_row(
            "SELECT COUNT(*) FROM protection_control_operations WHERE operation_kind = 'grant' AND recipient_public_identity = ?1",
            [agent.as_bytes().as_slice()], |row| row.get(0),
        ).unwrap();
        assert_eq!(grants, 0, "no grant to the conflicted key");
        assert!(
            !node
                .scheduler_snapshot_for_test()
                .iter()
                .any(|peer| peer.peer == agent)
        );
    }
    let (operations, refused) = crate::membership_sync::session::probe_membership_sync(
        &endpoint,
        cluster.address("creator"),
        workspace,
        Vec::new(),
    )
    .await;
    assert!(
        operations.is_empty(),
        "a former Agent-only key receives no membership"
    );
    assert!(refused);
    endpoint.close().await;
    cluster.finish().await;
}
