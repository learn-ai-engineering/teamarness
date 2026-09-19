mod common;

use std::collections::BTreeMap;

use teamarness_package_host::{
    AcknowledgeUiEvents, CancelUiRequest, LifecycleState, MountState, OpenUiMount,
    OpenUiSubscription, PackageErrorCode, PackageHost, PackageHostCommand, PackageHostNotification,
    PackageHostOutcome, PackageHostSubscriptionSelector, PackageValue, UiRequest, UiRequestId,
    UiSubscriptionId,
};

fn empty() -> PackageValue {
    PackageValue::Object(BTreeMap::new())
}

async fn mount(host: &PackageHost, index: usize) -> teamarness_package_host::UiMountId {
    let instance = host.snapshot().await.instances[index].key.clone();
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        other => panic!("unexpected outcome: {other:?}"),
    }
}

async fn open(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    id: &str,
    operation: &str,
) -> teamarness_package_host::PackageHostSubscription {
    let subscription = UiSubscriptionId::new(id).unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: subscription.clone(),
        operation: operation.into(),
        payload: empty(),
    }))
    .await
    .unwrap();
    host.subscribe(PackageHostSubscriptionSelector::UiEvents {
        mount: mount.clone(),
        subscription,
    })
    .await
    .unwrap()
}

fn sequence(notification: PackageHostNotification) -> String {
    match notification {
        PackageHostNotification::UiEvent(event) => event.sequence,
        other => panic!("unexpected notification: {other:?}"),
    }
}

#[tokio::test]
async fn immediate_events_wait_for_the_claim_and_arrive_in_order() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let mut events = open(&host, &mount, "events", "fixture.events").await;
    let second_claim = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new("events").unwrap(),
        })
        .await;
    assert!(matches!(
        second_claim,
        Err(error) if error.code == PackageErrorCode::StaleMount
    ));

    assert_eq!(sequence(events.changed().await.unwrap()), "1");
    assert_eq!(sequence(events.changed().await.unwrap()), "2");
    assert!(matches!(
        host.execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount,
                subscription: UiSubscriptionId::new("events").unwrap(),
                through_sequence: 2,
            }
        ))
        .await
        .unwrap(),
        PackageHostOutcome::UiEventsAcknowledged
    ));
}

#[tokio::test]
async fn thirty_two_credit_window_replenishes_only_the_acknowledged_delta() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let mut events = open(&host, &mount, "saturation", "fixture.saturation").await;
    let premature = host
        .execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: mount.clone(),
                subscription: UiSubscriptionId::new("saturation").unwrap(),
                through_sequence: 1,
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(premature.code, PackageErrorCode::ProtocolViolation);
    for expected in 1..=32 {
        assert_eq!(
            sequence(events.changed().await.unwrap()),
            expected.to_string()
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.changed())
            .await
            .is_err()
    );

    host.execute(PackageHostCommand::AcknowledgeUiEvents(
        AcknowledgeUiEvents {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new("saturation").unwrap(),
            through_sequence: 1,
        },
    ))
    .await
    .unwrap();
    assert_eq!(sequence(events.changed().await.unwrap()), "33");

    for through in [1, 0, 34, 65] {
        let error = host
            .execute(PackageHostCommand::AcknowledgeUiEvents(
                AcknowledgeUiEvents {
                    mount: mount.clone(),
                    subscription: UiSubscriptionId::new("saturation").unwrap(),
                    through_sequence: through,
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code, PackageErrorCode::ProtocolViolation, "{through}");
    }
}

#[tokio::test]
async fn one_mount_accepts_at_most_eight_event_routes() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    for index in 0..8 {
        host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new(format!("stream-{index}")).unwrap(),
            operation: "fixture.events".into(),
            payload: empty(),
        }))
        .await
        .unwrap();
    }
    let error = host
        .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount,
            subscription: UiSubscriptionId::new("stream-8").unwrap(),
            operation: "fixture.events".into(),
            payload: empty(),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::Overloaded);
}

/// A UI event is classified by the one shared classifier, like every other
/// frame.
///
/// A duplicate sequence, a gap, and a 33rd event against the 32-event window
/// are `event_sequence` and `credit_violation`: out of contract, and fatal to
/// the process generation. A route that decided them itself reported its own
/// codes, kept the generation alive and failed only the mount, which is the
/// divergence this asserts is gone. Events the Package sent before the
/// violation still reach the subscriber in order, so nothing is dropped in
/// silence.
#[tokio::test]
async fn events_the_contract_forbids_fail_the_generation_and_not_only_the_mount() {
    for operation in ["fixture.duplicate", "fixture.gap", "fixture.overflow"] {
        let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let mount = mount(&host, 0).await;
        let mut events = open(&host, &mount, "failed-route", operation).await;
        let mut delivered = Vec::new();
        let error = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(2), events.changed())
                .await
                .unwrap()
            {
                Ok(notification) => delivered.push(sequence(notification)),
                Err(error) => break error,
            }
        };
        assert_eq!(
            error.code,
            PackageErrorCode::ProtocolViolation,
            "{operation}"
        );
        for (index, actual) in delivered.iter().enumerate() {
            assert_eq!(actual, &(index + 1).to_string(), "{operation}");
        }
        assert_eq!(
            host.snapshot().await.instances[0].lifecycle,
            LifecycleState::Failed,
            "{operation}"
        );
        // The generation failed, so its mounts went with it: a protocol
        // violation is not restartable and clears the process state.
        assert_eq!(
            host.snapshot().await.instances[0].mount_state,
            None,
            "{operation}"
        );
    }
}

#[tokio::test]
async fn backend_event_contract_failure_fails_only_the_mount() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await.instances[0].clone();
    let mount = mount(&host, 0).await;
    let subscription = UiSubscriptionId::new("invalid").unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: subscription.clone(),
        operation: "fixture.invalid_event".into(),
        payload: empty(),
    }))
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while host.snapshot().await.instances[0].mount_state != Some(MountState::Failed) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount,
            subscription,
        })
        .await
        .unwrap();
    assert_eq!(
        events.changed().await.unwrap_err().code,
        PackageErrorCode::UiContractViolation
    );
    let after = host.snapshot().await.instances[0].clone();
    assert_eq!(after.lifecycle, LifecycleState::Active);
    assert_eq!(after.process_generation, before.process_generation);
    assert_eq!(after.mount_state, Some(MountState::Failed));
}

#[tokio::test]
async fn request_contract_rejects_both_sides_and_backend_violation_fails_only_the_mount() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await.instances[0].clone();
    let mount = mount(&host, 0).await;
    for (operation, payload) in [
        ("unknown.operation", empty()),
        (
            "fixture.typed",
            PackageValue::Object(BTreeMap::from([(
                "value".to_owned(),
                PackageValue::Boolean(true),
            )])),
        ),
    ] {
        let error = host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: mount.clone(),
                request: UiRequestId::new(operation.replace('.', "-")).unwrap(),
                operation: operation.into(),
                payload,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code, PackageErrorCode::UiContractViolation);
        assert_eq!(
            host.snapshot().await.instances[0].mount_state,
            Some(MountState::Mounted)
        );
    }

    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new("invalid-response").unwrap(),
            operation: "fixture.invalid_response".into(),
            payload: empty(),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::UiContractViolation);
    let after = host.snapshot().await.instances[0].clone();
    assert_eq!(after.lifecycle, LifecycleState::Active);
    assert_eq!(after.process_generation, before.process_generation);
    assert_eq!(after.mount_state, Some(MountState::Failed));
}

#[tokio::test]
async fn package_closed_stream_completes_without_stopping_the_backend() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let generation = host.snapshot().await.instances[0].process_generation;
    let mount = mount(&host, 0).await;
    let mut events = open(&host, &mount, "closed", "fixture.close").await;
    assert!(matches!(
        events.changed().await.unwrap(),
        PackageHostNotification::UiStreamCompleted(_)
    ));
    assert_eq!(
        host.snapshot().await.instances[0].process_generation,
        generation
    );
}

/// V-01: a stream that completes normally is acknowledged for its last event.
///
/// The subscriber's acknowledgement is routinely still in flight when the
/// stream ends, so it names a subscription the contract has already closed.
/// That released credit for a closed stream and killed the generation, which
/// `ProtocolViolation` does not even restart.
#[tokio::test]
async fn acknowledging_the_last_event_of_a_completed_stream_keeps_the_generation() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let generation = host.snapshot().await.instances[0].process_generation;
    let mount = mount(&host, 0).await;
    let subscription = UiSubscriptionId::new("completing").unwrap();
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: subscription.clone(),
        operation: "fixture.complete".into(),
        payload: empty(),
    }))
    .await
    .unwrap();
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription: subscription.clone(),
        })
        .await
        .unwrap();
    assert_eq!(sequence(events.changed().await.unwrap()), "1");
    assert_eq!(sequence(events.changed().await.unwrap()), "2");
    assert!(matches!(
        events.changed().await.unwrap(),
        PackageHostNotification::UiStreamCompleted(_)
    ));

    // The acknowledgement the subscriber had already queued for event 2.
    let _ = host
        .execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: mount.clone(),
                subscription,
                through_sequence: 2,
            },
        ))
        .await;

    let after = host.snapshot().await.instances[0].clone();
    assert_eq!(after.lifecycle, LifecycleState::Active);
    assert_eq!(after.process_generation, generation);
    assert_eq!(after.mount_state, Some(MountState::Mounted));
}

/// A Package that rejects a subscription reports its own code to its own UI, and the mount
/// stays healthy: only a host-owned stream failure is a mount fault.
#[tokio::test]
async fn package_rejected_stream_carries_its_code_without_failing_the_mount() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let mount = mount(&host, 0).await;
    let mut events = open(&host, &mount, "rejected", "fixture.rejected_stream").await;
    let error = events.changed().await.unwrap_err();
    assert_eq!(error.code, PackageErrorCode::BackendError);
    let package_error = error.package_error.as_ref().expect("Package-owned error");
    assert_eq!(package_error.code, "fixture.stream_rejected");
    assert!(!package_error.retryable);
    assert!(package_error.details.is_empty());
    assert_eq!(
        host.snapshot().await.instances[0].mount_state,
        Some(MountState::Mounted)
    );
}

#[tokio::test]
async fn acknowledgements_reject_cross_mount_and_stale_generation_routes() {
    let fixture =
        common::Fixture::node(&[("one", "dev.teamarness.one"), ("two", "dev.teamarness.two")])
            .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let first = mount(&host, 0).await;
    let second = mount(&host, 1).await;
    let mut events = open(&host, &second, "only-second", "fixture.events").await;
    assert_eq!(sequence(events.changed().await.unwrap()), "1");
    let cross_mount = host
        .execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: first.clone(),
                subscription: UiSubscriptionId::new("only-second").unwrap(),
                through_sequence: 1,
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(cross_mount.code, PackageErrorCode::StaleMount);

    let mut old_events = open(&host, &first, "old-generation", "fixture.events").await;
    assert_eq!(sequence(old_events.changed().await.unwrap()), "1");
    let generation = host.snapshot().await.instances[0]
        .process_generation
        .unwrap();
    let _ = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: first.clone(),
            request: UiRequestId::new("abort-generation").unwrap(),
            operation: "fixture.abort".into(),
            payload: empty(),
        }))
        .await;
    assert!(
        host.snapshot().await.instances[0]
            .process_generation
            .unwrap()
            > generation
    );
    let stale = host
        .execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: first,
                subscription: UiSubscriptionId::new("old-generation").unwrap(),
                through_sequence: 1,
            },
        ))
        .await
        .unwrap_err();
    assert_eq!(stale.code, PackageErrorCode::StaleMount);
}

#[tokio::test]
async fn mount_scoped_client_ids_do_not_collide_or_cross_cancel() {
    let fixture =
        common::Fixture::node(&[("one", "dev.teamarness.one"), ("two", "dev.teamarness.two")])
            .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let first = mount(&host, 0).await;
    let second = mount(&host, 1).await;
    let first_host = host.clone();
    let first_mount = first.clone();
    let first_delayed = tokio::spawn(async move {
        first_host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: first_mount,
                request: UiRequestId::new("same").unwrap(),
                operation: "fixture.delayed".into(),
                payload: empty(),
            }))
            .await
    });
    let second_host = host.clone();
    let second_mount = second.clone();
    let second_delayed = tokio::spawn(async move {
        second_host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: second_mount,
                request: UiRequestId::new("same").unwrap(),
                operation: "fixture.delayed".into(),
                payload: empty(),
            }))
            .await
    });
    // settle: lets both requests reach the Backend before one is cancelled.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    host.execute(PackageHostCommand::CancelUiRequest(CancelUiRequest {
        mount: first.clone(),
        request: UiRequestId::new("same").unwrap(),
    }))
    .await
    .unwrap();
    host.execute(PackageHostCommand::UiRequest(UiRequest {
        mount: second.clone(),
        request: UiRequestId::new("release-second").unwrap(),
        operation: "fixture.immediate".into(),
        payload: empty(),
    }))
    .await
    .unwrap();
    assert!(first_delayed.await.unwrap().is_err());
    assert!(second_delayed.await.unwrap().is_ok());

    for current in [&first, &second] {
        assert!(matches!(
            host.execute(PackageHostCommand::UiRequest(UiRequest {
                mount: current.clone(),
                request: UiRequestId::new("reused").unwrap(),
                operation: "fixture.immediate".into(),
                payload: empty(),
            }))
            .await
            .unwrap(),
            PackageHostOutcome::UiResponse { .. }
        ));
    }

    let mut first_events = open(&host, &first, "same-stream", "fixture.events").await;
    let mut second_events = open(&host, &second, "same-stream", "fixture.events").await;
    assert_eq!(sequence(first_events.changed().await.unwrap()), "1");
    assert_eq!(sequence(second_events.changed().await.unwrap()), "1");

    let replacement = mount(&host, 0).await;
    assert_ne!(replacement, first);
    assert_eq!(
        host.execute(PackageHostCommand::AcknowledgeUiEvents(
            AcknowledgeUiEvents {
                mount: first.clone(),
                subscription: UiSubscriptionId::new("same-stream").unwrap(),
                through_sequence: 1,
            }
        ))
        .await
        .unwrap_err()
        .code,
        PackageErrorCode::StaleMount
    );
    let mut replacement_events = open(&host, &replacement, "same-stream", "fixture.events").await;
    assert_eq!(sequence(replacement_events.changed().await.unwrap()), "1");
    assert!(matches!(
        host.execute(PackageHostCommand::UiRequest(UiRequest {
            mount: replacement,
            request: UiRequestId::new("same").unwrap(),
            operation: "fixture.immediate".into(),
            payload: empty(),
        }))
        .await
        .unwrap(),
        PackageHostOutcome::UiResponse { .. }
    ));
    assert_eq!(
        host.execute(PackageHostCommand::UiRequest(UiRequest {
            mount: first,
            request: UiRequestId::new("same").unwrap(),
            operation: "fixture.immediate".into(),
            payload: empty(),
        }))
        .await
        .unwrap_err()
        .code,
        PackageErrorCode::StaleMount
    );
}
