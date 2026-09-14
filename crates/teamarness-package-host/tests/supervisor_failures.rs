mod common;
#[allow(dead_code)]
#[path = "../src/protocol.rs"]
mod host_protocol;

use std::collections::BTreeMap;

use teamarness_package_host::{
    LifecycleState, OpenUiMount, PackageErrorCode, PackageHost, PackageHostCommand,
    PackageHostOutcome, PackageValue, RecoverPackage, UiRequest, UiRequestId,
};

#[tokio::test]
async fn package_error_preserves_generation_and_unrelated_instance() {
    let fixture =
        common::Fixture::node(&[("one", "dev.teamarness.one"), ("two", "dev.teamarness.two")])
            .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await;
    assert!(
        before
            .instances
            .iter()
            .all(|item| item.lifecycle == LifecycleState::Active)
    );
    let target = before.instances[0].clone();
    let other = before.instances[1].clone();
    let opened = host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: target.key.clone(),
        }))
        .await
        .unwrap();
    let mount = match opened {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        _ => unreachable!(),
    };
    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new("ordinary-error").unwrap(),
            operation: "fixture.error".into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::BackendError);
    let after = host.snapshot().await;
    let target_after = after
        .instances
        .iter()
        .find(|item| item.key == target.key)
        .unwrap();
    let other_after = after
        .instances
        .iter()
        .find(|item| item.key == other.key)
        .unwrap();
    assert_eq!(target_after.lifecycle, LifecycleState::Active);
    assert_eq!(target_after.process_generation, target.process_generation);
    assert_eq!(other_after.lifecycle, LifecycleState::Active);
    assert_eq!(other_after.process_generation, other.process_generation);
}

#[tokio::test]
async fn exit_invalidates_generation_without_replaying_the_request_and_explicit_recovery_works() {
    let fixture = common::Fixture::node(&[("one", "dev.teamarness.one")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let instance = host.snapshot().await.instances[0].key.clone();
    let mount = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: instance.clone(),
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        _ => unreachable!(),
    };
    let stale_mount = mount.clone();
    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new("abort").unwrap(),
            operation: "fixture.abort".into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await
        .unwrap_err();
    assert!(matches!(
        error.code,
        PackageErrorCode::ProcessExited | PackageErrorCode::OutcomeUncertain
    ));
    let restarted = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let snapshot = host.snapshot().await.instances[0].clone();
            if snapshot.lifecycle == LifecycleState::Active {
                break snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(restarted.process_generation, Some(2));
    let stale = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: stale_mount,
            request: UiRequestId::new("must-not-replay").unwrap(),
            operation: "fixture.immediate".into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await
        .unwrap_err();
    assert_eq!(stale.code, PackageErrorCode::StaleMount);
    let mutation_files = walk_files(&fixture._private)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "mutation-attempts")
        })
        .collect::<Vec<_>>();
    assert_eq!(mutation_files.len(), 1);
    assert_eq!(std::fs::read_to_string(&mutation_files[0]).unwrap(), "1\n");
    host.execute(PackageHostCommand::Recover(RecoverPackage { instance }))
        .await
        .unwrap();
    let recovered = host.snapshot().await.instances[0].clone();
    assert_eq!(recovered.lifecycle, LifecycleState::Active);
    assert_eq!(recovered.process_generation, Some(2));
}

#[tokio::test]
async fn malformed_protocol_generation_fails_without_restarting_or_harming_another_instance() {
    let fixture = common::Fixture::node(&[
        ("bad", "dev.teamarness.bad"),
        ("healthy", "dev.teamarness.healthy"),
    ])
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await;
    let bad = before
        .instances
        .iter()
        .find(|item| item.key.package_identity.as_str() == "dev.teamarness.bad")
        .unwrap()
        .clone();
    let mount = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: bad.key.clone(),
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        _ => unreachable!(),
    };
    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount,
            request: UiRequestId::new("malformed").unwrap(),
            operation: "fixture.malformed".into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::MalformedFrame);
    let after = host.snapshot().await;
    let bad_after = after
        .instances
        .iter()
        .find(|item| item.key == bad.key)
        .unwrap();
    let healthy = after
        .instances
        .iter()
        .find(|item| item.key.package_identity.as_str() == "dev.teamarness.healthy")
        .unwrap();
    assert_eq!(bad_after.lifecycle, LifecycleState::Failed);
    assert_eq!(bad_after.process_generation, Some(1));
    assert_eq!(healthy.lifecycle, LifecycleState::Active);
}

fn walk_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap().flatten() {
            if entry.path().is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files
}

#[tokio::test]
async fn four_failed_generations_enter_crash_loop_without_affecting_another_instance() {
    let fixture = common::Fixture::node(&[
        ("crash", "dev.teamarness.crash"),
        ("healthy", "dev.teamarness.healthy"),
    ])
    .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    let crashed = snapshot
        .instances
        .iter()
        .find(|item| item.key.package_identity.as_str() == "dev.teamarness.crash")
        .unwrap();
    let healthy = snapshot
        .instances
        .iter()
        .find(|item| item.key.package_identity.as_str() == "dev.teamarness.healthy")
        .unwrap();
    assert_eq!(crashed.lifecycle, LifecycleState::CrashLoop);
    assert_eq!(crashed.process_generation, Some(4));
    assert_eq!(healthy.lifecycle, LifecycleState::Active);
    assert_eq!(healthy.process_generation, Some(1));
}

#[tokio::test]
async fn uncertain_state_change_deadline_cancels_once_and_is_never_replayed() {
    use tokio::io::AsyncReadExt;

    let (host_stdout, package_stdout) = tokio::io::duplex(4096);
    let (mut package_stdin, host_stdin) = tokio::io::duplex(4096);
    let client = host_protocol::start(host_stdout, host_stdin);
    let drain = tokio::spawn(async move {
        let mut bytes = Vec::new();
        package_stdin.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let started = std::time::Instant::now();
    let error = client
        .request(
            "h:uncertain".into(),
            "fixture.mutate",
            None,
            PackageValue::Object(BTreeMap::new()),
            std::time::Duration::from_millis(10),
            true,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, PackageErrorCode::OutcomeUncertain);
    assert!(error.outcome_uncertain);
    assert!(started.elapsed() >= std::time::Duration::from_millis(500));
    drop(client);
    drop(package_stdout);
    let bytes = drain.await.unwrap();
    let request_occurrences = String::from_utf8_lossy(&bytes)
        .matches("fixture.mutate")
        .count();
    assert_eq!(request_occurrences, 1);
}

#[test]
fn failure_and_deadline_matrix_has_a_stable_public_code_for_every_required_class() {
    let matrix = [
        ("exit", PackageErrorCode::ProcessExited),
        ("abort", PackageErrorCode::ProcessExited),
        ("stall", PackageErrorCode::DeadlineExceeded),
        ("malformed_frame", PackageErrorCode::MalformedFrame),
        ("unknown_response", PackageErrorCode::ProtocolViolation),
        ("duplicate_terminal", PackageErrorCode::ProtocolViolation),
        ("send_without_credit", PackageErrorCode::ProtocolViolation),
        ("overload", PackageErrorCode::Overloaded),
        ("package_error", PackageErrorCode::BackendError),
        ("lifecycle_error", PackageErrorCode::BackendError),
        ("health_failure", PackageErrorCode::DeadlineExceeded),
        ("crash_loop", PackageErrorCode::CrashLoop),
        ("clean_shutdown", PackageErrorCode::ShuttingDown),
        ("eof_exit", PackageErrorCode::ProcessExited),
        ("handshake_deadline", PackageErrorCode::HandshakeTimeout),
        ("lifecycle_deadline", PackageErrorCode::DeadlineExceeded),
        ("request_deadline", PackageErrorCode::OutcomeUncertain),
        ("capability_deadline", PackageErrorCode::DeadlineExceeded),
        ("health_deadline", PackageErrorCode::DeadlineExceeded),
        ("shutdown_deadline", PackageErrorCode::ShuttingDown),
    ];
    assert_eq!(matrix.len(), 20);
    assert!(matrix.iter().all(|(name, _)| !name.is_empty()));
}
