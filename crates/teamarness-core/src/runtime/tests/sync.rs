use super::*;

#[tokio::test]
async fn production_log_sync_grants_scope_converges_and_survives_restart() {
    let root = std::env::temp_dir().join(format!(
        "teamarness-production-log-sync-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let a_dir = root.join("a");
    let b_dir = root.join("b");
    let clock = || {
        Box::new(RecordingClock {
            calls: Arc::new(AtomicU64::new(0)),
        }) as Box<dyn Clock>
    };
    let a_config = CoreRuntimeConfig::new(&a_dir);
    let b_config = CoreRuntimeConfig::new(&b_dir);
    let a = CoreRuntime::open_with_clock_and_readiness(a_config.clone(), clock(), true)
        .await
        .unwrap();
    let workspace = a.create_workspace("Production sync").unwrap();
    let package = "teamarness.test";
    let invitation = a
        .create_workspace_invitation_for_test(
            workspace.id(),
            workspace.created_at_unix_ms(),
            60_000,
        )
        .unwrap();
    let b = CoreRuntime::open_with_clock_and_readiness(b_config.clone(), clock(), true)
        .await
        .unwrap();
    b.join_workspace(&invitation).await.unwrap();

    let first = a
        .append_record(workspace.id(), package, "message.post", b"first")
        .unwrap();
    let received = wait_for_record(&b, workspace.id(), package, first.record_id()).await;
    assert_eq!(received.record_id(), first.record_id());

    let a_local = a
        .append_record(workspace.id(), package, "message.post", b"a-local")
        .unwrap();
    let received_a_local = wait_for_record(&b, workspace.id(), package, a_local.record_id()).await;
    assert_eq!(received_a_local.record_id(), a_local.record_id());

    let b_local = b
        .append_record(workspace.id(), package, "message.post", b"b-local")
        .unwrap();
    let received_b_local = wait_for_record(&a, workspace.id(), package, b_local.record_id()).await;
    let mut a_records = a.subscribe_records(workspace.id(), package, None).unwrap();
    let mut a_seen = std::collections::BTreeSet::new();
    while let Some(event) = a_records
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        a_seen.insert((event.record_id(), event.order_key()));
    }
    assert_eq!(received_b_local.record_id(), b_local.record_id());
    let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
    let mut b_seen = std::collections::BTreeSet::new();
    while let Some(event) = b_records
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        b_seen.insert((event.record_id(), event.order_key()));
    }
    for expected in [
        (first.record_id(), first.order_key()),
        (a_local.record_id(), a_local.order_key()),
        (b_local.record_id(), b_local.order_key()),
    ] {
        assert!(a_seen.contains(&expected));
        assert!(b_seen.contains(&expected));
    }

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    drop(a);
    drop(b);

    let a = CoreRuntime::open_with_clock_and_readiness(a_config, clock(), true)
        .await
        .unwrap();
    let b = CoreRuntime::open_with_clock_and_readiness(b_config, clock(), true)
        .await
        .unwrap();
    let after_restart = a
        .append_record(workspace.id(), package, "message.post", b"after-restart")
        .unwrap();
    a.replicate_records_to_address_for_test(
        workspace.id(),
        package,
        b.transport.endpoint_address(),
        b.public_identity(),
    )
    .await
    .unwrap();
    let received_after_restart =
        wait_for_record(&b, workspace.id(), package, after_restart.record_id()).await;
    assert_eq!(
        received_after_restart.record_id(),
        after_restart.record_id()
    );

    let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
    let mut b_seen = std::collections::BTreeSet::new();
    while let Some(event) = b_records
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        assert!(b_seen.insert((event.record_id(), event.order_key())));
    }
    assert_eq!(b_seen.len(), 4);
    assert!(b_seen.contains(&(after_restart.record_id(), after_restart.order_key())));

    b.state
        .lock()
        .unwrap()
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_remote_record_insert
             BEFORE INSERT ON immutable_records
             BEGIN SELECT RAISE(ABORT, 'injected remote apply failure'); END;",
        )
        .unwrap();
    let after_failure = a
        .append_record(workspace.id(), package, "message.post", b"after-failure")
        .unwrap();
    // The injected row failure is one peer's conflict, not a local fault:
    // the session finishes, the record is simply not applied yet.
    a.replicate_records_to_address_for_test(
        workspace.id(),
        package,
        b.transport.endpoint_address(),
        b.public_identity(),
    )
    .await
    .unwrap();
    let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
    let mut b_seen = std::collections::BTreeSet::new();
    while let Some(event) = b_records
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        b_seen.insert(event.record_id());
    }
    assert!(!b_seen.contains(&after_failure.record_id()));
    b.state
        .lock()
        .unwrap()
        .connection
        .execute_batch("DROP TRIGGER fail_remote_record_insert;")
        .unwrap();
    a.replicate_records_to_address_for_test(
        workspace.id(),
        package,
        b.transport.endpoint_address(),
        b.public_identity(),
    )
    .await
    .unwrap();
    let received_after_failure =
        wait_for_record(&b, workspace.id(), package, after_failure.record_id()).await;
    assert_eq!(
        received_after_failure.record_id(),
        after_failure.record_id()
    );

    let mut b_records = b.subscribe_records(workspace.id(), package, None).unwrap();
    let mut b_seen = std::collections::BTreeSet::new();
    while let Some(event) = b_records
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        assert!(b_seen.insert((event.record_id(), event.order_key())));
    }
    assert_eq!(b_seen.len(), 5);

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    drop(a);
    drop(b);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn current_peer_forwards_record_from_historical_member() {
    let root = std::env::temp_dir().join(format!(
        "teamarness-historical-record-forwarding-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let alice = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("alice")),
        Box::new(SystemClock),
        true,
    )
    .await
    .unwrap();
    let workspace = alice.create_workspace("Historical forwarding").unwrap();
    let bob_invitation = alice
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let bob = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("bob")),
        Box::new(SystemClock),
        true,
    )
    .await
    .unwrap();
    bob.join_workspace(&bob_invitation).await.unwrap();

    let package = "teamarness.test";
    alice
        .append_record(workspace.id(), package, "message.post", b"seed")
        .unwrap();
    let (bob_origin, bob_grant) = alice
        .export_protection_bundle_for_test(workspace.id(), package, bob.public_identity())
        .unwrap();
    bob.import_protection_grant(workspace.id(), package, &bob_origin, &bob_grant)
        .unwrap();
    let historical = bob
        .append_record(workspace.id(), package, "message.post", b"historical")
        .unwrap();
    // Bob's own operation is read while Bob is still a current Member: the
    // gate now refuses a revoked Installation on its own sessions too.
    let bob_receiver = RuntimeSyncReceiver {
        state: Arc::downgrade(&bob.state),
        clock: Arc::new(SystemClock),
        demand: Arc::clone(&bob.membership_demand),
        health: Arc::clone(&bob.health),
    };
    let bob_identity = bob.public_identity();
    let seed = crate::synchronization::SyncReceiver::session_seed(
        &bob_receiver,
        alice.public_identity(),
        workspace.id(),
        package,
    )
    .unwrap();
    let height = *seed
        .heights
        .get(&bob_identity)
        .expect("bob has authored one operation");
    let operation = crate::synchronization::SyncReceiver::load_range_page(
        &bob_receiver,
        alice.public_identity(),
        workspace.id(),
        package,
        bob_identity,
        height,
        height,
        1,
    )
    .unwrap()
    .into_iter()
    .map(|(operation, _)| operation)
    .next()
    .unwrap();
    alice
        .revoke_member(workspace.id(), bob.public_identity())
        .unwrap();

    let carol_invitation = alice
        .create_workspace_invitation(workspace.id())
        .await
        .unwrap();
    let carol = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("carol")),
        Box::new(SystemClock),
        true,
    )
    .await
    .unwrap();
    carol.join_workspace(&carol_invitation).await.unwrap();
    let (carol_origin, carol_grant) = alice
        .export_protection_bundle_for_test(workspace.id(), package, carol.public_identity())
        .unwrap();
    carol
        .import_protection_grant(workspace.id(), package, &carol_origin, &carol_grant)
        .unwrap();

    let carol_receiver = RuntimeSyncReceiver {
        state: Arc::downgrade(&carol.state),
        clock: Arc::new(SystemClock),
        demand: Arc::clone(&carol.membership_demand),
        health: Arc::clone(&carol.health),
    };
    crate::synchronization::SyncReceiver::receive_operation(
        &carol_receiver,
        alice.public_identity(),
        workspace.id(),
        package,
        operation,
    )
    .expect("a current peer may forward a historically eligible author's record");

    let mut subscription = carol
        .subscribe_records(workspace.id(), package, None)
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(event) = subscription
        .next_event_timeout(std::time::Duration::ZERO)
        .unwrap()
    {
        seen.insert(event.record_id());
    }
    assert!(seen.contains(&historical.record_id()));

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    carol.shutdown().await.unwrap();
    drop(alice);
    drop(bob);
    drop(carol);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalid_remote_sequence_isolated_before_journal_commit() {
    let root = std::env::temp_dir().join(format!(
        "teamarness-invalid-remote-operation-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let a = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("a")),
        Box::new(SystemClock),
        true,
    )
    .await
    .unwrap();
    let workspace = a.create_workspace("Invalid remote").unwrap();
    let invitation = a
        .create_workspace_invitation_for_test(
            workspace.id(),
            workspace.created_at_unix_ms(),
            SystemClock.now_unix_ms().saturating_add(60_000),
        )
        .unwrap();
    let b = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("b")),
        Box::new(SystemClock),
        true,
    )
    .await
    .unwrap();
    b.join_workspace(&invitation).await.unwrap();
    let package = "teamarness.test";
    a.append_record(workspace.id(), package, "message.post", b"seed")
        .unwrap();
    let (origin, grant) = a
        .export_protection_bundle_for_test(workspace.id(), package, b.public_identity())
        .unwrap();
    b.import_protection_grant(workspace.id(), package, &origin, &grant)
        .unwrap();
    let record = b
        .append_record(workspace.id(), package, "message.post", b"remote")
        .unwrap();
    let secret = b.state.lock().unwrap().secret_key.to_bytes();
    let key = p2panda_core::SigningKey::from_bytes(&secret);
    let operation = p2panda_core::Operation::from_parts(
        p2panda_core::Header::<()>::builder()
            .body(record.sealed_record())
            .seq_num(1)
            .backlink(Some(p2panda_core::Hash::from_bytes([7; 32])))
            .build(&key, ()),
        Some(p2panda_core::Body::from_bytes(record.sealed_record())),
    );
    let before = {
        let state = a.state.lock().unwrap();
        (
            state
                .connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM immutable_operations", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            state
                .connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM p2panda_operations", [], |row| {
                    row.get(0)
                })
                .unwrap(),
        )
    };
    let receiver = RuntimeSyncReceiver {
        state: Arc::downgrade(&a.state),
        clock: Arc::new(SystemClock),
        demand: Arc::clone(&a.membership_demand),
        health: Arc::clone(&a.health),
    };
    // One unacceptable operation is classified, not fatal: the session
    // continues and the author is skipped for the rest of it.
    assert_eq!(
        crate::synchronization::SyncReceiver::receive_operation(
            &receiver,
            b.public_identity(),
            workspace.id(),
            package,
            operation,
        ),
        Ok(crate::synchronization::OperationOutcome::Rejected(
            crate::synchronization::RejectReason::Conflict
        ))
    );
    let after = {
        let state = a.state.lock().unwrap();
        (
            state
                .connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM immutable_operations", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            state
                .connection
                .query_row::<i64, _, _>("SELECT COUNT(*) FROM p2panda_operations", [], |row| {
                    row.get(0)
                })
                .unwrap(),
        )
    };
    assert_eq!(before, after);
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    drop(a);
    drop(b);
    fs::remove_dir_all(root).unwrap();
}
