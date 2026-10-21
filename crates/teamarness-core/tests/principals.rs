use std::sync::atomic::{AtomicU64, Ordering};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, PersonNameError, PrincipalKind};

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
    manager
        .revoke_member(workspace, steward.public_identity())
        .unwrap();
    assert!(manager.principal(workspace, second).unwrap().is_none());
    manager.shutdown().await.unwrap();
    steward.shutdown().await.unwrap();
    std::fs::remove_dir_all(manager_path).unwrap();
    std::fs::remove_dir_all(steward_path).unwrap();
}
