use super::*;

#[tokio::test]
async fn test_clock_is_injected_without_entering_the_public_configuration() {
    let calls = Arc::new(AtomicU64::new(0));
    let directory =
        std::env::temp_dir().join(format!("teamarness-runtime-clock-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);

    let runtime = CoreRuntime::open_with_clock(
        CoreRuntimeConfig::new(&directory),
        Box::new(RecordingClock {
            calls: Arc::clone(&calls),
        }),
    )
    .await
    .unwrap();

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn dropping_a_membership_subscription_deregisters_its_listener() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-subscription-drop-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let runtime = CoreRuntime::open_with_clock(
        CoreRuntimeConfig::new(&directory),
        Box::new(RecordingClock {
            calls: Arc::new(AtomicU64::new(0)),
        }),
    )
    .await
    .unwrap();
    let workspace = runtime.create_workspace("Subscription drop").unwrap();

    let subscription = runtime
        .subscribe_membership_bounded(workspace.id(), MembershipEpoch::new(0), 1)
        .unwrap();
    assert_eq!(
        runtime
            .state
            .lock()
            .unwrap()
            .subscribers
            .get(&workspace.id())
            .map_or(0, Vec::len),
        1
    );
    drop(subscription);
    assert_eq!(
        runtime
            .state
            .lock()
            .unwrap()
            .subscribers
            .get(&workspace.id())
            .map_or(0, Vec::len),
        0
    );

    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn endpoint_identity_persists_and_invitation_lifetime_starts_after_readiness() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-transport-identity-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let now = Arc::new(AtomicI64::new(42_000));
    let config = CoreRuntimeConfig::new(&directory);
    let runtime =
        CoreRuntime::open_with_clock(config.clone(), Box::new(AdjustableClock(Arc::clone(&now))))
            .await
            .unwrap();
    assert_eq!(
        runtime.transport.endpoint_id(),
        runtime.public_identity.as_public_key()
    );
    let workspace = runtime.create_workspace("Transport identity").unwrap();
    let invitation = runtime
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let verified = invitation.verified().unwrap();
    assert_eq!(verified.token.issued_at_unix_ms, 42_000);
    assert_eq!(verified.token.expires_at_unix_ms, 42_000 + 15 * 60 * 1_000);
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened =
        CoreRuntime::open_with_clock(config, Box::new(AdjustableClock(Arc::clone(&now))))
            .await
            .unwrap();
    assert_eq!(
        reopened.transport.endpoint_id(),
        reopened.public_identity.as_public_key()
    );
    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_invitation_creation_after_readiness() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-invitation-shutdown-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let runtime = Arc::new(
        CoreRuntime::open_with_clock(
            CoreRuntimeConfig::new(&directory),
            Box::new(AdjustableClock(Arc::new(AtomicI64::new(42_000)))),
        )
        .await
        .unwrap(),
    );
    let workspace = runtime.create_workspace("Invitation shutdown").unwrap();

    runtime.transport.hold_next_invitation_after_readiness();
    let inviting_runtime = Arc::clone(&runtime);
    let invitation = tokio::spawn(async move {
        inviting_runtime
            .create_workspace_invitation(workspace.id())
            .await
    });
    runtime.transport.wait_for_invitation_readiness_hold().await;
    assert_eq!(runtime.transport.active_operations(), 1);

    let closing_runtime = Arc::clone(&runtime);
    let mut shutdown = tokio::spawn(async move { closing_runtime.shutdown().await });
    tokio::select! {
        result = &mut shutdown => {
            panic!("shutdown completed before invitation creation: {result:?}");
        }
        () = runtime.transport.wait_for_idle_waiter() => {}
    }

    runtime.transport.release_invitation_readiness_hold();
    invitation.await.unwrap().unwrap();
    shutdown.await.unwrap().unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn readiness_failure_does_not_sign_or_start_an_invitation_lifetime() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-readiness-clock-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let calls = Arc::new(AtomicU64::new(0));
    let runtime = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(&directory),
        Box::new(RecordingClock {
            calls: Arc::clone(&calls),
        }),
        false,
    )
    .await
    .unwrap();
    let workspace = runtime.create_workspace("No readiness").unwrap();
    let before = calls.load(Ordering::Relaxed);
    assert_eq!(
        runtime
            .create_workspace_invitation(workspace.id())
            .await
            .unwrap_err(),
        crate::OnboardingError::Readiness
    );
    assert_eq!(calls.load(Ordering::Relaxed), before);
    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn stopping_transport_rejects_onboarding_but_preserves_local_commands() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-local-during-shutdown-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let runtime = Arc::new(
        CoreRuntime::open_with_clock(
            CoreRuntimeConfig::new(&directory),
            Box::new(AdjustableClock(Arc::new(AtomicI64::new(7_000)))),
        )
        .await
        .unwrap(),
    );
    let workspace = runtime.create_workspace("Local during shutdown").unwrap();
    let active = runtime.transport.begin_outbound().await.unwrap();
    let closing = Arc::clone(&runtime);
    let shutdown = tokio::spawn(async move { closing.shutdown().await });
    while runtime.transport.lifecycle_state() == 0 {
        tokio::task::yield_now().await;
    }

    assert_eq!(runtime.display_name(), "This device");
    assert_eq!(runtime.list_workspaces().unwrap(), vec![workspace.clone()]);
    assert_eq!(
        runtime
            .membership_snapshot(workspace.id())
            .unwrap()
            .members()
            .len(),
        1
    );
    assert_eq!(
        runtime
            .create_workspace_invitation(workspace.id())
            .await
            .unwrap_err(),
        crate::OnboardingError::Shutdown
    );
    assert!(matches!(
        runtime.transport.begin_outbound().await,
        Err(crate::OnboardingError::Shutdown)
    ));
    drop(active);
    shutdown.await.unwrap().unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}
