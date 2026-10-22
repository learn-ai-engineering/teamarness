use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

/// Verifies: TEAM.SYS.MEM-R07
#[tokio::test]
async fn accepted_set_version_is_content_derived_and_restart_stable() {
    let directory = private_data_directory("membership-version");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Convergent").unwrap();
    let before = runtime.membership_snapshot(workspace.id()).unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let after = reopened.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(after.version(), before.version());
    assert_eq!(after.members(), before.members());

    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

/// Verifies: TEAM.SYS.MEM-R02, MEM-R07, MEM-R10. Both independently signed
/// Agent claims are accepted facts, but neither can remain an effective Member.
#[tokio::test]
async fn concurrent_signed_cross_group_key_claims_fail_closed_in_both_orders() {
    use std::time::{SystemTime, UNIX_EPOCH};
    use teamarness_core::test_authoring::{
        TestAction, identity_of, operation_id, sign_admission, sign_operation,
    };
    use teamarness_core::{MembershipApplyOutcomeForTests, MembershipChange};

    let baseline = private_data_directory("conflicting-claims-baseline");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&baseline))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Conflicting claims").unwrap().id();
    let creator = runtime.installation_secret_for_tests();
    let genesis = runtime.genesis_id_for_tests(workspace);
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let stewards = [[31; 32], [32; 32]];
    let agent = [33; 32];
    let mut claims = Vec::new();
    for (index, steward) in stewards.iter().enumerate() {
        let (create, admit) = sign_admission(
            &creator,
            steward,
            workspace,
            genesis,
            now,
            now + 100_000,
            [u8::try_from(index + 1).unwrap(); 32],
            now,
            &runtime.membership_heads_for_tests(workspace),
        );
        assert_eq!(
            runtime
                .apply_membership_operation_for_tests(&create)
                .unwrap(),
            MembershipApplyOutcomeForTests::Quarantined
        );
        assert!(matches!(
            runtime
                .apply_membership_operation_for_tests(&admit)
                .unwrap(),
            MembershipApplyOutcomeForTests::Accepted(_)
        ));
        claims.push(sign_operation(
            steward,
            workspace,
            now + 1,
            TestAction::AddAgentKey {
                group: operation_id(&create),
                agent_secret: agent,
            },
            &[operation_id(&admit)],
        ));
    }
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let mut results = Vec::new();
    for (index, order) in [[0, 1], [1, 0]].into_iter().enumerate() {
        let directory = private_data_directory(&format!("conflicting-claims-peer-{index}"));
        fs::create_dir_all(&directory).unwrap();
        fs::copy(
            baseline.join("core.sqlite3"),
            directory.join("core.sqlite3"),
        )
        .unwrap();
        let peer = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        assert!(matches!(
            peer.apply_membership_operation_for_tests(&claims[order[0]])
                .unwrap(),
            MembershipApplyOutcomeForTests::Accepted(_)
        ));
        let before_conflict = peer.membership_snapshot(workspace).unwrap();
        let mut live = peer
            .subscribe_membership(workspace, before_conflict.epoch())
            .unwrap();
        assert!(matches!(
            peer.apply_membership_operation_for_tests(&claims[order[1]])
                .unwrap(),
            MembershipApplyOutcomeForTests::Accepted(_)
        ));
        let conflict_event = live
            .next_transition_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            conflict_event.change(),
            MembershipChange::Revoked {
                member: identity_of(&agent)
            }
        );
        let snapshot = peer.membership_snapshot(workspace).unwrap();
        assert!(
            !snapshot
                .members()
                .iter()
                .any(|member| member.identity() == identity_of(&agent))
        );
        assert!(
            peer.workspace_principal(workspace, identity_of(&agent))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            peer.export_membership_operations_for_tests(workspace).len(),
            9
        );
        let principal_rows: i64 = rusqlite::Connection::open(directory.join("core.sqlite3"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM workspace_principals WHERE lower(hex(workspace_id)) = ?1 AND lower(hex(public_identity)) = ?2",
                rusqlite::params![workspace.to_string(), identity_of(&agent).to_string()],
                |row| row.get(0),
            ).unwrap();
        assert_eq!(principal_rows, 0);
        assert!(matches!(
            peer.apply_membership_operation_for_tests(&claims[order[0]])
                .unwrap(),
            MembershipApplyOutcomeForTests::Duplicate
        ));
        let removal = sign_operation(
            &stewards[order[0]],
            workspace,
            now + 2,
            TestAction::RemoveAgentKey {
                group: peer
                    .person_group_for_tests(workspace, identity_of(&stewards[order[0]]))
                    .unwrap(),
                agent: identity_of(&agent),
            },
            &peer.membership_heads_for_tests(workspace),
        );
        assert!(matches!(
            peer.apply_membership_operation_for_tests(&removal).unwrap(),
            MembershipApplyOutcomeForTests::Accepted(_)
        ));
        assert!(
            peer.workspace_principal(workspace, identity_of(&agent))
                .unwrap()
                .is_none()
        );
        let readd = sign_operation(
            &stewards[order[0]],
            workspace,
            now + 3,
            TestAction::AddAgentKey {
                group: peer
                    .person_group_for_tests(workspace, identity_of(&stewards[order[0]]))
                    .unwrap(),
                agent_secret: agent,
            },
            &peer.membership_heads_for_tests(workspace),
        );
        assert!(peer.apply_membership_operation_for_tests(&readd).is_err());
        results.push(snapshot);
        peer.shutdown().await.unwrap();
        drop(peer);
        let reopened = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let mut durable = reopened
            .subscribe_membership(workspace, before_conflict.epoch())
            .unwrap();
        assert_eq!(
            durable
                .next_transition_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .change(),
            conflict_event.change()
        );
        assert!(
            !reopened
                .membership_snapshot(workspace)
                .unwrap()
                .members()
                .iter()
                .any(|member| member.identity() == identity_of(&agent))
        );
        assert!(
            reopened
                .workspace_principal(workspace, identity_of(&agent))
                .unwrap()
                .is_none()
        );
        reopened.shutdown().await.unwrap();
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }
    assert_eq!(results[0], results[1]);
    fs::remove_dir_all(baseline).unwrap();
}

/// Verifies: TEAM.SYS.MEM-R02, MEM-R07. A key claimed as both a Person and
/// an Agent is not an effective Steward or Agent after either delivery order.
#[tokio::test]
async fn concurrent_person_and_agent_claims_exclude_the_key_and_its_agents() {
    use std::time::{SystemTime, UNIX_EPOCH};
    use teamarness_core::MembershipApplyOutcomeForTests;
    use teamarness_core::test_authoring::{
        TestAction, identity_of, operation_id, sign_admission, sign_operation,
    };

    let baseline = private_data_directory("cross-kind-baseline");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&baseline))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Cross-kind").unwrap().id();
    let creator = runtime.installation_secret_for_tests();
    let genesis = runtime.genesis_id_for_tests(workspace);
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let steward = [41; 32];
    let shared = [42; 32];
    let child = [43; 32];
    let (create_steward, admit_steward) = sign_admission(
        &creator,
        &steward,
        workspace,
        genesis,
        now,
        now + 100_000,
        [1; 32],
        now,
        &runtime.membership_heads_for_tests(workspace),
    );
    runtime
        .apply_membership_operation_for_tests(&create_steward)
        .unwrap();
    runtime
        .apply_membership_operation_for_tests(&admit_steward)
        .unwrap();
    let heads = runtime.membership_heads_for_tests(workspace);
    let add_shared = sign_operation(
        &steward,
        workspace,
        now + 1,
        TestAction::AddAgentKey {
            group: operation_id(&create_steward),
            agent_secret: shared,
        },
        &[operation_id(&admit_steward)],
    );
    let (create_shared, admit_shared) = sign_admission(
        &creator,
        &shared,
        workspace,
        genesis,
        now,
        now + 100_000,
        [2; 32],
        now + 1,
        &heads,
    );
    let add_child = sign_operation(
        &shared,
        workspace,
        now + 2,
        TestAction::AddAgentKey {
            group: operation_id(&create_shared),
            agent_secret: child,
        },
        &[operation_id(&admit_shared)],
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let mut snapshots = Vec::new();
    for (index, order) in [[0, 1], [1, 0]].into_iter().enumerate() {
        let directory = private_data_directory(&format!("cross-kind-peer-{index}"));
        fs::create_dir_all(&directory).unwrap();
        fs::copy(
            baseline.join("core.sqlite3"),
            directory.join("core.sqlite3"),
        )
        .unwrap();
        let peer = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let claims = [&add_shared, &admit_shared];
        peer.apply_membership_operation_for_tests(&create_shared)
            .unwrap();
        for claim in order {
            assert!(matches!(
                peer.apply_membership_operation_for_tests(claims[claim])
                    .unwrap(),
                MembershipApplyOutcomeForTests::Accepted(_)
            ));
        }
        assert!(matches!(
            peer.apply_membership_operation_for_tests(&add_child)
                .unwrap(),
            MembershipApplyOutcomeForTests::Accepted(_)
        ));
        let snapshot = peer.membership_snapshot(workspace).unwrap();
        assert!(
            !peer
                .installation_members_for_tests(workspace)
                .contains(&identity_of(&shared))
        );
        for key in [shared, child] {
            assert!(
                !snapshot
                    .members()
                    .iter()
                    .any(|member| member.identity() == identity_of(&key))
            );
            assert!(
                peer.workspace_principal(workspace, identity_of(&key))
                    .unwrap()
                    .is_none()
            );
        }
        let attempted = sign_operation(
            &shared,
            workspace,
            now + 3,
            TestAction::AddAgentKey {
                group: operation_id(&create_shared),
                agent_secret: [44; 32],
            },
            &peer.membership_heads_for_tests(workspace),
        );
        assert!(
            peer.apply_membership_operation_for_tests(&attempted)
                .is_err()
        );
        snapshots.push(snapshot);
        peer.shutdown().await.unwrap();
        drop(peer);
        fs::remove_dir_all(directory).unwrap();
    }
    assert_eq!(snapshots[0], snapshots[1]);
    fs::remove_dir_all(baseline).unwrap();
}

/// Verifies: TEAM.SYS.MEM-R04, MEM-R06, MEM-R07, MEM-R10. The Manager revokes a
/// Person while that Person, cut off, adds an Agent key. After the two
/// Installations exchange, both agree the Person and its Agent are out, with
/// no gap and no readmission (strong remove).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_revocation_and_agent_add_converge_with_both_keys_out() {
    use std::time::{SystemTime, UNIX_EPOCH};

    use teamarness_core::test_authoring::{TestAction, identity_of, sign_operation};
    use teamarness_core::{MembershipApplyOutcomeForTests, MembershipRole};

    const AGENT: [u8; 32] = [3; 32];

    let manager_directory = private_data_directory("concurrency-manager");
    let person_directory = private_data_directory("concurrency-person");
    let manager = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&manager_directory))
        .await
        .unwrap();
    let person = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&person_directory))
        .await
        .unwrap();
    manager.remember_loopback_peer_for_tests(&person);
    person.remember_loopback_peer_for_tests(&manager);
    let workspace = manager.create_workspace("Concurrent").unwrap().id();
    let invitation = manager
        .create_workspace_invitation(workspace)
        .await
        .unwrap();
    person.join_workspace(&invitation).await.unwrap();
    assert_eq!(
        manager.membership_version(workspace).unwrap(),
        person.membership_version(workspace).unwrap()
    );

    // Restart both loopback endpoints without reintroducing their new addresses.
    // Pausing scheduling alone does not cancel an already-running sync session,
    // which could otherwise deliver the revoke before the concurrent add.
    manager.shutdown().await.unwrap();
    person.shutdown().await.unwrap();
    drop(manager);
    drop(person);
    let manager = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&manager_directory))
        .await
        .unwrap();
    let person = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&person_directory))
        .await
        .unwrap();
    manager.pause_sync_scheduler_for_tests(true);
    person.pause_sync_scheduler_for_tests(true);

    let person_identity = person.public_identity();
    let agent = identity_of(&AGENT);
    manager.revoke_member(workspace, person_identity).unwrap();
    let group = person
        .person_group_for_tests(workspace, person_identity)
        .expect("the Person still holds its group in its own view");
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let add_agent = sign_operation(
        &person.installation_secret_for_tests(),
        workspace,
        now,
        TestAction::AddAgentKey {
            group,
            agent_secret: AGENT,
        },
        &person.membership_heads_for_tests(workspace),
    );
    assert!(matches!(
        person
            .apply_membership_operation_for_tests(&add_agent)
            .unwrap(),
        MembershipApplyOutcomeForTests::Accepted(_)
    ));
    assert!(
        person
            .membership_snapshot(workspace)
            .unwrap()
            .members()
            .iter()
            .any(|member| member.identity() == agent),
        "in its own view the Person added its Agent"
    );

    // Exchange the concurrent envelopes through the production membership
    // acceptance path. A revoked Installation is deliberately no longer a
    // session peer, so network sync cannot deliver its last concurrent edit.
    // Causal export gives each side the same operation set in either order.
    let manager_operations = manager.export_membership_operations_for_tests(workspace);
    let person_operations = person.export_membership_operations_for_tests(workspace);
    for canonical in person_operations {
        manager
            .apply_membership_operation_for_tests(&canonical)
            .unwrap();
    }
    for canonical in manager_operations {
        person
            .apply_membership_operation_for_tests(&canonical)
            .unwrap();
    }
    let manager_snapshot = manager.membership_snapshot(workspace).unwrap();
    let person_snapshot = person.membership_snapshot(workspace).unwrap();
    assert_eq!(manager_snapshot.version(), person_snapshot.version());
    assert_eq!(manager_snapshot.members(), person_snapshot.members());
    assert_eq!(manager_snapshot.members().len(), 1);
    assert_eq!(
        manager_snapshot.members()[0].role(),
        MembershipRole::Manager
    );
    assert_eq!(
        manager_snapshot.members()[0].identity(),
        manager.public_identity()
    );
    // No readmission: the Agent's add is in the set yet grants nothing, and the
    // Person's interval is closed on both sides.
    assert!(!manager.was_member(workspace, agent, i64::MAX).unwrap());
    assert!(!person.was_member(workspace, agent, i64::MAX).unwrap());
    assert!(
        !person
            .was_member(workspace, person_identity, i64::MAX)
            .unwrap()
    );

    manager.shutdown().await.unwrap();
    person.shutdown().await.unwrap();
    drop(manager);
    drop(person);
    fs::remove_dir_all(manager_directory).unwrap();
    fs::remove_dir_all(person_directory).unwrap();
}
