//! Blocking synchronization work participates in the ownership handoff fence.

use std::sync::Arc;
use std::time::Duration;

use super::cluster::cluster_root;
use crate::{CoreRuntime, CoreRuntimeConfig, StorageError};

/// Verifies: TEAM.SYS.TRANS-R07, TEAM.SYS.ID-R11
#[tokio::test(flavor = "current_thread")]
async fn cancelled_storage_callback_prevents_successful_shutdown_and_retirement() {
    let root = cluster_root("shutdown-session-work");
    let runtime = CoreRuntime::open_loopback_for_tests(
        CoreRuntimeConfig::new(&root).with_exclusive_application_ownership(),
    )
    .await
    .unwrap();
    let workspace = runtime.create_workspace("Cleanup").unwrap().id();
    let receiver = Arc::clone(&runtime.sync_services.receiver);
    let sessions = Arc::clone(&runtime.peer_sessions);
    let capacity = Arc::new(tokio::sync::Semaphore::new(1));
    let work = sessions
        .begin_work(capacity.clone().try_acquire_owned().unwrap())
        .unwrap();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (finished, finish) = tokio::sync::oneshot::channel();
    let peer = runtime.public_identity();
    let running = tokio::spawn(async move {
        work.receiver_call(&receiver, move |receiver| {
            let result = receiver.session_seed(peer, workspace, "teamarness.test");
            let _ = started.send(());
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            let _ = finished.send(());
            result
        })
        .await
    });
    start.await.unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert_eq!(capacity.available_permits(), 0);
    assert!(!sessions.work_is_idle());
    assert!(
        runtime.shutdown().await.is_err(),
        "cleanup must report surviving storage work"
    );
    assert!(!runtime.state.lock().unwrap().shutdown_complete);
    assert!(matches!(
        runtime.retire_application_ownership(),
        Err(StorageError::RuntimePoisoned)
    ));
    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(
            CoreRuntimeConfig::new(&root).with_exclusive_application_ownership(),
        )
        .await,
        Err(crate::CoreError::Storage(
            StorageError::ApplicationAlreadyOwned
        ))
    ));
    assert!(
        sessions.begin_work(()).is_none(),
        "shutdown closes new work admission"
    );
    release.send(()).unwrap();
    finish.await.unwrap();
    // Observe the final lease drop, not just the callback's last statement.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !sessions.work_is_idle() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(capacity.try_acquire().is_ok());
    runtime.shutdown().await.unwrap();
    runtime.retire_application_ownership().unwrap();
    drop(runtime);
    let reopened = CoreRuntime::open_loopback_for_tests(
        CoreRuntimeConfig::new(&root).with_exclusive_application_ownership(),
    )
    .await
    .unwrap();
    reopened.shutdown().await.unwrap();
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
