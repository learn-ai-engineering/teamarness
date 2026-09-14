mod common;

use std::collections::BTreeMap;

use teamarness_package_host::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiSubscription, DisposePackage, HostRevision,
    LifecycleState, OpenUiMount, OpenUiSubscription, PackageErrorCode, PackageHost,
    PackageHostCommand, PackageHostNotification, PackageHostOutcome,
    PackageHostSubscriptionSelector, PackageValue, RecoverPackage, ShutdownHost, SuspendPackage,
    UiRequest, UiRequestId, UiSubscriptionId,
};

fn empty() -> PackageValue {
    PackageValue::Object(BTreeMap::new())
}

#[tokio::test]
async fn direct_node_process_completes_the_real_host_lifecycle_and_protocol() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert_eq!(snapshot.packages.len(), 1);
    assert_eq!(snapshot.instances.len(), 1);
    assert_eq!(
        snapshot.instances[0].lifecycle,
        LifecycleState::Active,
        "diagnostics: {:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.instances[0].process_generation, Some(1));
    let instance = snapshot.instances[0].key.clone();
    let lease_database = fixture._private.join("packages/package-host.sqlite3");
    let lease_count = || {
        rusqlite::Connection::open(&lease_database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM process_leases", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(lease_count(), 1, "lease is durable before active use");

    let opened = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: instance.clone(),
        }))
        .await
        .unwrap();
    let (mount, opened_revision) = match opened {
        PackageHostOutcome::UiMountOpened {
            mount, revision, ..
        } => (mount, revision),
        other => panic!("unexpected outcome: {other:?}"),
    };

    let delayed = host.execute(PackageHostCommand::UiRequest(UiRequest {
        mount: mount.clone(),
        request: UiRequestId::new("delayed").unwrap(),
        operation: "fixture.delayed".into(),
        payload: empty(),
    }));
    let immediate = host.execute(PackageHostCommand::UiRequest(UiRequest {
        mount: mount.clone(),
        request: UiRequestId::new("immediate").unwrap(),
        operation: "fixture.immediate".into(),
        payload: empty(),
    }));
    let (delayed, immediate) = tokio::join!(delayed, immediate);
    assert!(matches!(
        delayed.unwrap(),
        PackageHostOutcome::UiResponse { .. }
    ));
    assert!(matches!(
        immediate.unwrap(),
        PackageHostOutcome::UiResponse { .. }
    ));

    assert!(matches!(
        host.execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("stderr").unwrap(),
            operation: "fixture.stderr".into(),
            payload: empty(),
        }))
        .await
        .unwrap(),
        PackageHostOutcome::UiResponse { .. }
    ));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        std::fs::read_dir(fixture._private.join("packages/logs"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with("log.1"))
    );

    assert!(matches!(
        host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new("events").unwrap(),
            operation: "fixture.events".into(),
            payload: empty(),
        }))
        .await
        .unwrap(),
        PackageHostOutcome::UiSubscriptionOpened
    ));
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new("events").unwrap(),
        })
        .await
        .unwrap();
    for sequence in ["1", "2"] {
        assert!(matches!(
            events.changed().await.unwrap(),
            PackageHostNotification::UiEvent(event) if event.sequence == sequence
        ));
    }
    assert!(matches!(
        host.execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: mount.clone(),
                subscription: UiSubscriptionId::new("events").unwrap(),
                through_sequence: 2,
            }
        ))
        .await
        .unwrap(),
        PackageHostOutcome::UiEventsAcknowledged
    ));
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        LifecycleState::Active
    );

    let cancellation_host = host.clone();
    let cancellation_mount = mount.clone();
    let cancelled = tokio::spawn(async move {
        cancellation_host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: cancellation_mount,
                request: UiRequestId::new("cancelled").unwrap(),
                operation: "fixture.delayed".into(),
                payload: empty(),
            }))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(matches!(
        host.execute(PackageHostCommand::CancelUiRequest(CancelUiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("cancelled").unwrap(),
        }))
        .await
        .unwrap(),
        PackageHostOutcome::UiRequestCancelled
    ));
    assert!(cancelled.await.unwrap().is_err());
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        LifecycleState::Active
    );

    assert!(matches!(
        host.execute(PackageHostCommand::Suspend(SuspendPackage {
            instance: instance.clone()
        }))
        .await
        .unwrap(),
        PackageHostOutcome::Suspended { .. }
    ));
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        LifecycleState::Suspended
    );
    assert!(matches!(
        host.execute(PackageHostCommand::Recover(RecoverPackage {
            instance: instance.clone()
        }))
        .await
        .unwrap(),
        PackageHostOutcome::Recovered { .. }
    ));
    assert_eq!(
        host.snapshot().await.instances[0].process_generation,
        Some(1)
    );

    let mut subscription = host
        .subscribe(PackageHostSubscriptionSelector::HostState {
            after_revision: HostRevision(opened_revision.0),
        })
        .await
        .unwrap();
    host.execute(PackageHostCommand::Dispose(DisposePackage { instance }))
        .await
        .unwrap();
    let PackageHostNotification::HostState(notice) = subscription.changed().await.unwrap() else {
        panic!("expected host state notification")
    };
    assert!(notice.revision > opened_revision);
    assert!(notice.revision_gap);
    assert_eq!(
        host.snapshot().await.instances[0].lifecycle,
        LifecycleState::Disposed
    );
    assert_eq!(
        lease_count(),
        0,
        "disposed process was reaped before lease deletion"
    );
    let stale_request = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("after-dispose").unwrap(),
            operation: "fixture.immediate".into(),
            payload: empty(),
        }))
        .await
        .unwrap_err();
    assert_eq!(stale_request.code, PackageErrorCode::StaleMount);
    let stale_subscription = host
        .execute(PackageHostCommand::CloseUiSubscription(
            CloseUiSubscription {
                mount,
                subscription: UiSubscriptionId::new("events").unwrap(),
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(stale_subscription.code, PackageErrorCode::StaleMount);

    assert!(matches!(
        host.execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap(),
        PackageHostOutcome::Shutdown
    ));
}
