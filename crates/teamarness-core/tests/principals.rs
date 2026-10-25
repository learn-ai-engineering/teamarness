use std::sync::atomic::{AtomicU64, Ordering};
use teamarness_core::{
    CoreRuntime, CoreRuntimeConfig, PersonNameError, PrincipalKind, SynchronizationError,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "principals-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
async fn open(path: &std::path::Path) -> CoreRuntime {
    CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(path))
        .await
        .unwrap()
}

#[tokio::test]
async fn agent_custody_name_and_removal_survive_restart_without_resurrection() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Team", "Steward")
        .unwrap()
        .id();
    let person = runtime
        .workspace_person(workspace, runtime.public_identity())
        .unwrap()
        .unwrap();
    let first = runtime.add_agent(workspace, "First").unwrap();
    let second = runtime.add_agent(workspace, "Second").unwrap();
    assert_ne!(first, second);
    let (_, rows) = runtime.principals_snapshot(workspace).unwrap();
    assert_eq!(rows.len(), 3);
    let agent = runtime.principal(workspace, first).unwrap().unwrap();
    assert_eq!(agent.kind(), PrincipalKind::Agent);
    assert_eq!(agent.steward_principal_id(), Some(person.id()));
    assert_eq!(agent.name(), Some("First"));
    runtime
        .rename_principal(workspace, first, "Renamed")
        .unwrap();
    runtime.remove_agent(workspace, first).unwrap();
    assert!(runtime.principal(workspace, first).unwrap().is_none());
    assert_eq!(
        runtime
            .principal(workspace, second)
            .unwrap()
            .unwrap()
            .name(),
        Some("Second")
    );
    assert_eq!(
        runtime.remove_agent(workspace, first),
        Err(PersonNameError::AgentNotFound)
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = open(&path).await;
    assert!(reopened.principal(workspace, first).unwrap().is_none());
    assert!(reopened.principal(workspace, second).unwrap().is_some());
    // A retained local custody row must never put a remotely removed key back
    // in a current registry: the registry is exclusively membership-derived.
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn non_steward_cannot_rename_or_bind_remote_agent_but_manager_can_remove_one() {
    let manager_path = directory();
    let steward_path = directory();
    let manager = open(&manager_path).await;
    let steward = open(&steward_path).await;
    let workspace = manager.create_workspace("Team").unwrap().id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let invitation = loop {
        match manager.create_workspace_invitation(workspace).await {
            Ok(invitation) => break invitation,
            Err(_) if std::time::Instant::now() < deadline => {
                // negative-wait: bounded backoff while the invitation transport becomes ready.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(error) => panic!("invitation readiness: {error}"),
        }
    };
    steward.join_workspace(&invitation).await.unwrap();
    manager.remember_loopback_peer_for_tests(&steward);
    steward.remember_loopback_peer_for_tests(&manager);
    let mut manager_principals = manager.subscribe_principals(workspace).unwrap();
    let first = steward.add_agent(workspace, "One").unwrap();
    let second = steward.add_agent(workspace, "Two").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while manager.principal(workspace, first).unwrap().is_none()
        && std::time::Instant::now() < deadline
    {
        // poll-interval: bounded retry until the peer has the Agent membership.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(manager.principal(workspace, first).unwrap().is_some());
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        manager_principals.changed(),
    )
    .await
    .unwrap()
    .unwrap();
    let (added_cursor, added_rows) = manager.principals_snapshot(workspace).unwrap();
    assert!(added_rows.iter().any(|row| row.id() == first));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    while manager.principal(workspace, first).unwrap().unwrap().name() != Some("One")
        && std::time::Instant::now() < deadline
    {
        // poll-interval: bounded retry until the private name edit replicates.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        manager.principal(workspace, first).unwrap().unwrap().name(),
        Some("One")
    );
    assert_eq!(
        manager.rename_principal(workspace, first, "No"),
        Err(PersonNameError::NotOwnPerson)
    );
    assert_eq!(
        manager.bind_agent_package(workspace, first, "pkg"),
        Err(PersonNameError::NotOwnPerson)
    );
    let mut steward_principals = steward.subscribe_principals(workspace).unwrap();
    steward
        .bind_agent_package(workspace, first, "agent.runtime")
        .unwrap();
    assert_eq!(
        steward.bound_agent(workspace, "agent.runtime").unwrap(),
        Some(first)
    );
    manager.remove_agent(workspace, first).unwrap();
    assert!(manager.principal(workspace, first).unwrap().is_none());
    let (removed_cursor, removed_rows) = manager.principals_snapshot(workspace).unwrap();
    assert!(removed_cursor > added_cursor);
    assert!(!removed_rows.iter().any(|row| row.id() == first));
    assert!(manager.principal(workspace, second).unwrap().is_some());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while steward.principal(workspace, first).unwrap().is_some()
        && std::time::Instant::now() < deadline
    {
        // poll-interval: bounded retry until the Manager removal replicates.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(steward.principal(workspace, first).unwrap().is_none());
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        steward_principals.changed(),
    )
    .await
    .unwrap()
    .unwrap();
    let (_, steward_rows) = steward.principals_snapshot(workspace).unwrap();
    assert!(!steward_rows.iter().any(|row| row.id() == first));
    // A peer-removed Agent has left the registry: the binding read is None and
    // the key never signs again, whatever local rows remain.
    assert_eq!(
        steward.bound_agent(workspace, "agent.runtime").unwrap(),
        None
    );
    assert_eq!(
        steward
            .append_record_as_principal(
                workspace,
                "agent.runtime",
                "message.post",
                b"late",
                Some(first)
            )
            .err(),
        Some(SynchronizationError::PrincipalNotBound)
    );
    assert_eq!(
        steward
            .principal(workspace, second)
            .unwrap()
            .unwrap()
            .kind(),
        PrincipalKind::Agent
    );
    assert_eq!(
        steward.rename_principal(workspace, first, "Resurrect"),
        Err(PersonNameError::NotOwnPerson)
    );
    assert_eq!(
        steward.bind_agent_package(workspace, second, "internal:teamarness.principals"),
        Err(PersonNameError::InvalidPackageIdentity)
    );
    assert_eq!(
        steward.bind_agent_package(workspace, second, "not-ascii-🦀"),
        Err(PersonNameError::InvalidPackageIdentity)
    );
    steward
        .bind_agent_package(workspace, second, "agent.runtime")
        .unwrap();
    assert_eq!(
        steward.bound_agent(workspace, "agent.runtime").unwrap(),
        Some(second)
    );
    manager
        .revoke_member(workspace, steward.public_identity())
        .unwrap();
    assert!(manager.principal(workspace, second).unwrap().is_none());
    manager.shutdown().await.unwrap();
    steward.shutdown().await.unwrap();
    std::fs::remove_dir_all(manager_path).unwrap();
    std::fs::remove_dir_all(steward_path).unwrap();
}

fn admission_rows(path: &std::path::Path) -> (i64, i64, i64, i64) {
    let db = rusqlite::Connection::open(path.join("core.sqlite3")).unwrap();
    db.query_row(
        "SELECT (SELECT COUNT(*) FROM membership_operations WHERE action=5),
                (SELECT COUNT(*) FROM agent_keys),
                (SELECT COUNT(*) FROM principal_names n JOIN workspace_principals p USING(workspace_id,principal_id) WHERE p.kind='agent'),
                (SELECT COUNT(*) FROM pending_principal_name_edits e JOIN workspace_principals p USING(workspace_id,principal_id) WHERE p.kind='agent')",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap()
}

#[tokio::test]
async fn failed_named_agent_add_commits_neither_membership_nor_custody() {
    use teamarness_core::AgentAddFaultForTests as Fault;
    for boundary in [
        Fault::BeforeName,
        Fault::BeforeCommit,
        Fault::BootstrapBound,
    ] {
        let path = directory();
        let runtime = open(&path).await;
        let workspace = runtime
            .create_workspace_with_person_name("Team", "Owner")
            .unwrap()
            .id();
        let before = runtime.membership_snapshot(workspace).unwrap();
        assert!(
            runtime
                .add_agent_with_fault_for_tests(workspace, "Agent", boundary)
                .is_err()
        );
        assert_eq!(runtime.membership_snapshot(workspace).unwrap(), before);
        assert_eq!(admission_rows(&path), (0, 0, 0, 0));
        let id = runtime.add_agent(workspace, "Agent").unwrap();
        assert_eq!(
            runtime.principal(workspace, id).unwrap().unwrap().name(),
            Some("Agent")
        );
        assert_eq!(admission_rows(&path).0, 1);
        runtime.shutdown().await.unwrap();
        drop(runtime);
        let reopened = open(&path).await;
        assert_eq!(
            reopened.principal(workspace, id).unwrap().unwrap().name(),
            Some("Agent")
        );
        reopened.shutdown().await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[tokio::test]
async fn uncertain_named_agent_commit_poisoned_until_roster_reconciliation() {
    use teamarness_core::{AgentAddFaultForTests as Fault, MembershipError, StorageError};
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Team", "Owner")
        .unwrap()
        .id();
    assert_eq!(
        runtime.add_agent_with_fault_for_tests(workspace, "Agent", Fault::AfterCommit),
        Err(PersonNameError::Membership(MembershipError::Storage(
            StorageError::UncertainOutcome
        )))
    );
    assert_eq!(
        runtime.add_agent(workspace, "Agent"),
        Err(PersonNameError::Storage(StorageError::RuntimePoisoned))
    );
    assert_eq!(admission_rows(&path), (1, 1, 1, 1));
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = open(&path).await;
    let (membership, principals) = reopened.principal_roster(workspace).unwrap();
    assert_eq!(membership.members().len(), 2);
    assert_eq!(
        principals
            .iter()
            .filter(|p| p.kind() == PrincipalKind::Agent && p.name() == Some("Agent"))
            .count(),
        1
    );
    assert_eq!(admission_rows(&path).0, 1);
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn committed_agent_name_survives_deferred_sync_initialization() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Team", "Owner")
        .unwrap()
        .id();
    let id = runtime
        .add_agent_with_fault_for_tests(
            workspace,
            "Agent",
            teamarness_core::AgentAddFaultForTests::PostCommitInitialization,
        )
        .unwrap();
    assert_eq!(
        runtime.principal(workspace, id).unwrap().unwrap().name(),
        Some("Agent")
    );
    assert_eq!(admission_rows(&path).0, 1);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = open(&path).await;
    assert_eq!(
        reopened.principal(workspace, id).unwrap().unwrap().name(),
        Some("Agent")
    );
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn principal_roster_reads_one_transition_not_two_separate_epochs() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Team", "Owner")
        .unwrap()
        .id();
    let old = runtime.membership_snapshot(workspace).unwrap();
    let id = runtime.add_agent(workspace, "Agent").unwrap();
    let (_, new_rows) = runtime.principals_snapshot(workspace).unwrap();
    assert!(
        old.members()
            .iter()
            .all(|m| new_rows.iter().any(|p| p.keys().contains(&m.identity())))
    );
    let (snapshot, rows) = runtime.principal_roster(workspace).unwrap();
    assert_eq!(snapshot.members().len(), 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|p| p.id() == id));
    runtime.remove_agent(workspace, id).unwrap();
    let (snapshot, rows) = runtime.principal_roster(workspace).unwrap();
    assert_eq!(snapshot.members().len(), 1);
    assert_eq!(rows.len(), 1);
    runtime.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

fn binding_rows(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let db = rusqlite::Connection::open(path.join("core.sqlite3")).unwrap();
    let mut statement = db
        .prepare(
            "SELECT package_identity, agent_principal_id FROM agent_package_bindings ORDER BY package_identity",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[tokio::test]
// Verifies: TEAM.SYS.ID-R10, TEAM.SYS.PKG-R10
async fn bound_agent_signs_for_any_local_package_until_unbound_or_removed() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Team", "Steward")
        .unwrap()
        .id();
    let person = runtime
        .workspace_person(workspace, runtime.public_identity())
        .unwrap()
        .unwrap()
        .id();
    let first = runtime.add_agent(workspace, "First").unwrap();
    let second = runtime.add_agent(workspace, "Second").unwrap();
    let second_key = runtime
        .principal(workspace, second)
        .unwrap()
        .unwrap()
        .keys()[0];
    let sign = |package: &str, principal| {
        runtime
            .append_record_as_principal(
                workspace,
                package,
                "message.post",
                b"hello",
                Some(principal),
            )
            .map(|record| record.author())
    };

    assert_eq!(runtime.bound_agent(workspace, "pkg.one").unwrap(), None);
    assert_eq!(
        runtime.bound_agent(workspace, "not-ascii-\u{1F980}"),
        Err(PersonNameError::InvalidPackageIdentity)
    );
    assert_eq!(
        runtime.unbind_agent_package(workspace, "internal:teamarness.principals"),
        Err(PersonNameError::InvalidPackageIdentity)
    );
    assert_eq!(
        sign("pkg.one", first),
        Err(SynchronizationError::PrincipalNotBound)
    );

    // A Package maps to one Agent; a later bind replaces the earlier one.
    runtime
        .bind_agent_package(workspace, first, "pkg.one")
        .unwrap();
    runtime
        .bind_agent_package(workspace, second, "pkg.one")
        .unwrap();
    assert_eq!(
        runtime.bound_agent(workspace, "pkg.one").unwrap(),
        Some(second)
    );
    assert_eq!(
        sign("pkg.one", first),
        Err(SynchronizationError::PrincipalNotBound)
    );

    // Two Packages share one Agent; the signing rule ignores the caller.
    runtime
        .bind_agent_package(workspace, second, "pkg.two")
        .unwrap();
    assert_eq!(
        runtime.bound_agent(workspace, "pkg.two").unwrap(),
        Some(second)
    );
    assert_eq!(sign("pkg.one", second), Ok(second_key));
    assert_eq!(sign("pkg.two", second), Ok(second_key));
    assert_eq!(sign("pkg.three", second), Ok(second_key));
    assert_eq!(
        sign("pkg.one", person),
        Err(SynchronizationError::PrincipalNotBound)
    );
    assert_eq!(
        binding_rows(&path),
        vec![
            ("pkg.one".to_string(), second.as_bytes().to_vec()),
            ("pkg.two".to_string(), second.as_bytes().to_vec()),
        ]
    );

    // Unbinding one Package leaves the other binding, and the authority, in place.
    runtime.unbind_agent_package(workspace, "pkg.two").unwrap();
    runtime.unbind_agent_package(workspace, "pkg.two").unwrap();
    assert_eq!(runtime.bound_agent(workspace, "pkg.two").unwrap(), None);
    assert_eq!(sign("pkg.two", second), Ok(second_key));
    runtime.unbind_agent_package(workspace, "pkg.one").unwrap();
    assert_eq!(
        sign("pkg.one", second),
        Err(SynchronizationError::PrincipalNotBound)
    );
    assert!(binding_rows(&path).is_empty());

    // Own removal clears this Installation's rows for the Agent.
    runtime
        .bind_agent_package(workspace, second, "pkg.one")
        .unwrap();
    runtime
        .bind_agent_package(workspace, first, "pkg.two")
        .unwrap();
    runtime.remove_agent(workspace, second).unwrap();
    assert_eq!(runtime.bound_agent(workspace, "pkg.one").unwrap(), None);
    assert_eq!(
        runtime.bound_agent(workspace, "pkg.two").unwrap(),
        Some(first)
    );
    assert_eq!(
        binding_rows(&path),
        vec![("pkg.two".to_string(), first.as_bytes().to_vec())]
    );
    assert_eq!(
        sign("pkg.one", second),
        Err(SynchronizationError::PrincipalNotBound)
    );
    assert_eq!(
        runtime.bind_agent_package(workspace, second, "pkg.one"),
        Err(PersonNameError::AgentNotFound)
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);

    // Bindings are durable local configuration.
    let reopened = open(&path).await;
    assert_eq!(
        reopened.bound_agent(workspace, "pkg.two").unwrap(),
        Some(first)
    );
    assert_eq!(reopened.bound_agent(workspace, "pkg.one").unwrap(), None);
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}
