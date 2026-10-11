use teamarness_core::{CoreRuntime, CoreRuntimeConfig, SynchronizationError};

const RESERVED: &str = "internal:teamarness.person-names";
const LEGACY_VALID_PACKAGE: &str = "internal.teamarness.person-names";

#[tokio::test]
async fn public_record_operations_cannot_use_person_name_scope() {
    let root = std::env::temp_dir().join(format!(
        "teamarness-reserved-scope-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let creator = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&root))
        .await
        .unwrap();
    let workspace = creator.create_workspace("Names").unwrap().id();
    assert!(matches!(
        creator.append_record(workspace, RESERVED, "name.edit", b"forged"),
        Err(SynchronizationError::InvalidInput)
    ));
    assert!(matches!(
        creator.subscribe_records(workspace, RESERVED, None),
        Err(SynchronizationError::InvalidInput)
    ));
    assert!(matches!(
        creator.open_records(workspace, RESERVED, &[]),
        Err(SynchronizationError::InvalidInput)
    ));
    let legacy = creator
        .append_record(
            workspace,
            LEGACY_VALID_PACKAGE,
            "message.post",
            b"previous data",
        )
        .unwrap();
    assert_eq!(
        creator
            .open_records(
                workspace,
                LEGACY_VALID_PACKAGE,
                &[legacy.sealed_record().to_vec()]
            )
            .unwrap()[0]
            .canonical(),
        b"previous data"
    );
    creator.shutdown().await.unwrap();
    drop(creator);
    let reopened = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&root))
        .await
        .unwrap();
    let mut replay = reopened
        .subscribe_records(workspace, LEGACY_VALID_PACKAGE, None)
        .unwrap();
    assert_eq!(
        replay
            .next_event_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .canonical(),
        b"previous data"
    );
    reopened.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
