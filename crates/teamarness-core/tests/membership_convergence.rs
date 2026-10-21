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
