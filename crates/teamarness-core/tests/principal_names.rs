use std::sync::atomic::{AtomicU64, Ordering};
use teamarness_core::{
    CoreRuntime, CoreRuntimeConfig, OnboardingError, PersonNameError, WorkspaceId,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "principal-names-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn durable_name(path: &std::path::Path, workspace: WorkspaceId) -> String {
    rusqlite::Connection::open(path.join("core.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT name FROM principal_names n WHERE hex(n.workspace_id) = ?1
             AND (EXISTS (SELECT 1 FROM pending_principal_name_edits p WHERE p.workspace_id=n.workspace_id AND p.principal_id=n.principal_id)
               OR EXISTS (SELECT 1 FROM principal_name_winners w WHERE w.workspace_id=n.workspace_id AND w.principal_id=n.principal_id))",
            [workspace.to_string().to_uppercase()],
            |row| row.get(0),
        )
        .unwrap()
}

async fn open(path: &std::path::Path) -> CoreRuntime {
    CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(path))
        .await
        .unwrap()
}

#[tokio::test]
async fn person_anchor_and_name_are_workspace_scoped_and_durable() {
    let path = directory();
    let runtime = open(&path).await;
    let a = runtime
        .create_workspace_with_person_name("First", "Alex 🧑‍💻")
        .unwrap();
    let b = runtime
        .create_workspace_with_person_name("Second", "Alex 🧑‍💻")
        .unwrap();
    let identity = runtime.public_identity();
    let a_person = runtime.workspace_person(a.id(), identity).unwrap().unwrap();
    let b_person = runtime.workspace_person(b.id(), identity).unwrap().unwrap();
    assert_ne!(a_person.id(), b_person.id());
    assert_eq!(a_person.name(), Some("Alex 🧑‍💻"));
    assert_eq!(durable_name(&path, a.id()), "Alex 🧑‍💻");
    let before = runtime.membership_snapshot(a.id()).unwrap();
    runtime.set_own_person_name(a.id(), "Another").unwrap();
    assert_eq!(durable_name(&path, a.id()), "Another");
    assert_eq!(runtime.membership_snapshot(a.id()).unwrap(), before);
    assert_eq!(
        runtime
            .workspace_person(b.id(), identity)
            .unwrap()
            .unwrap()
            .name(),
        Some("Alex 🧑‍💻")
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let reopened = open(&path).await;
    assert_eq!(
        reopened
            .workspace_person(a.id(), identity)
            .unwrap()
            .unwrap()
            .id(),
        a_person.id()
    );
    assert_eq!(
        reopened
            .workspace_person(a.id(), identity)
            .unwrap()
            .unwrap()
            .name(),
        Some("Another")
    );
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn unicode_validation_legacy_fallback_and_ownership() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime.create_workspace("Legacy").unwrap();
    let identity = runtime.public_identity();
    let old = runtime
        .workspace_person(workspace.id(), identity)
        .unwrap()
        .unwrap();
    assert_eq!(old.name(), None);
    assert!(!old.fingerprint().is_empty());
    for name in [
        "",
        " \t",
        "x\n",
        "x\u{0000}",
        "x\u{200b}y",
        &"👩‍🚀".repeat(65),
        &"界".repeat(86),
    ] {
        assert_eq!(
            runtime.set_own_person_name(workspace.id(), name),
            Err(PersonNameError::InvalidName)
        );
    }
    runtime
        .set_own_person_name(workspace.id(), &"👩‍🚀".repeat(64))
        .unwrap_err(); // exceeds byte limit
    runtime
        .set_own_person_name(workspace.id(), &"😀".repeat(64))
        .unwrap(); // exactly 64 graphemes and 256 bytes
    runtime
        .set_own_person_name(workspace.id(), "a\u{0301}")
        .unwrap();
    runtime
        .set_own_person_name(workspace.id(), &"a".repeat(64))
        .unwrap();
    assert_eq!(
        runtime.set_own_person_name_for(workspace.id(), identity, "allowed"),
        Ok(())
    );
    let stranger = open(&directory()).await;
    assert_eq!(
        runtime.set_own_person_name_for(workspace.id(), stranger.public_identity(), "forged"),
        Err(PersonNameError::NotOwnPerson)
    );
    stranger.shutdown().await.unwrap();
    runtime.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}

async fn invite(
    runtime: &CoreRuntime,
    workspace: WorkspaceId,
) -> teamarness_core::WorkspaceInvitation {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match runtime.create_workspace_invitation(workspace).await {
            Ok(invitation) => return invitation,
            Err(OnboardingError::Readiness) if std::time::Instant::now() < deadline => {
                // poll-interval: bounded retry until invitation transport is ready.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(error) => panic!("invitation: {error}"),
        }
    }
}

#[tokio::test]
async fn named_join_retry_preserves_one_person_and_allows_later_rename() {
    let creator_path = directory();
    let joiner_path = directory();
    let creator = open(&creator_path).await;
    let joiner = open(&joiner_path).await;
    let workspace = creator
        .create_workspace_with_person_name("Team", "Shared")
        .unwrap();
    let invitation = invite(&creator, workspace.id()).await;
    let before = creator.membership_snapshot(workspace.id()).unwrap();
    let (joined, first) = joiner
        .join_workspace_with_person_name(&invitation, "Shared")
        .await
        .unwrap();
    assert_eq!(joined.id(), workspace.id());
    assert_eq!(first.members().len(), 2);
    assert_eq!(
        creator
            .membership_snapshot(workspace.id())
            .unwrap()
            .members()
            .len(),
        before.members().len() + 1
    );
    let person = joiner
        .workspace_person(workspace.id(), joiner.public_identity())
        .unwrap()
        .unwrap();
    assert_eq!(person.name(), Some("Shared")); // duplicates allowed
    assert_eq!(durable_name(&joiner_path, workspace.id()), "Shared");
    let issuer_person = creator
        .workspace_person(workspace.id(), joiner.public_identity())
        .unwrap()
        .unwrap();
    assert_eq!(issuer_person.id(), person.id());
    assert!(matches!(issuer_person.name(), None | Some("Shared"))); // Delivery may still be pending.
    let (_, retry) = joiner
        .join_workspace_with_person_name(&invitation, "Renamed")
        .await
        .unwrap();
    assert_eq!(retry, first);
    assert_eq!(durable_name(&joiner_path, workspace.id()), "Renamed");
    assert_eq!(
        joiner
            .workspace_person(workspace.id(), joiner.public_identity())
            .unwrap()
            .unwrap()
            .id(),
        person.id()
    );
    assert_eq!(
        joiner
            .workspace_person(workspace.id(), joiner.public_identity())
            .unwrap()
            .unwrap()
            .name(),
        Some("Renamed")
    );
    joiner.shutdown().await.unwrap();
    drop(joiner);
    let reopened = open(&joiner_path).await;
    assert_eq!(
        reopened
            .workspace_person(workspace.id(), reopened.public_identity())
            .unwrap()
            .unwrap()
            .name(),
        Some("Renamed")
    );
    reopened.shutdown().await.unwrap();
    creator.shutdown().await.unwrap();
    std::fs::remove_dir_all(joiner_path).unwrap();
    std::fs::remove_dir_all(creator_path).unwrap();
}

#[tokio::test]
async fn forged_person_anchor_is_rejected_without_changing_membership() {
    let path = directory();
    let runtime = open(&path).await;
    let workspace = runtime
        .create_workspace_with_person_name("Guarded", "Right")
        .unwrap();
    let identity = runtime.public_identity();
    let person = runtime
        .workspace_person(workspace.id(), identity)
        .unwrap()
        .unwrap();
    let before = runtime.membership_snapshot(workspace.id()).unwrap();
    let connection = rusqlite::Connection::open(path.join("core.sqlite3")).unwrap();
    // The public IDs are text; use hex decoding in SQLite to target the row.
    connection
        .execute(
            "UPDATE principal_names SET anchor_operation_id = ?1 WHERE hex(principal_id) = ?2",
            rusqlite::params![
                [99_u8; 32].as_slice(),
                person.id().to_string().to_uppercase()
            ],
        )
        .unwrap();
    assert!(runtime.workspace_person(workspace.id(), identity).is_err());
    assert_eq!(runtime.membership_snapshot(workspace.id()).unwrap(), before);
    runtime.shutdown().await.unwrap();
    std::fs::remove_dir_all(path).unwrap();
}
