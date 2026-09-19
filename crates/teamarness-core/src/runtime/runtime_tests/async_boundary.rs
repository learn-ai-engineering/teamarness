//! Core state work never occupies an asynchronous executor thread.
//!
//! Real components: real [`CoreRuntime`]s on temporary directories, real Iroh
//! endpoints in loopback mode with the in-memory address book, real SQLite. The
//! only injected fault is a held runtime state lock, which is what makes the
//! difference between blocking and not blocking observable.
//!
//! Every test here runs on a `current_thread` runtime: one executor thread, so
//! a single blocking call starves everything else on it.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use super::super::{CoreRuntime, CoreRuntimeConfig, RuntimeStateHold, SystemClock};
use super::cluster::cluster_root;

const PACKAGE: &str = "teamarness.test";

/// Spawns `work`, then a sibling task on `executor` that sleeps 20 ms ten
/// times, and reports whether the sibling finished while `hold` still held the
/// runtime state lock.
///
/// The five-second wait is a failure bound, not a timing assertion: a sibling
/// that needs 200 ms of ticks either gets them from a free executor or never
/// gets them at all.
async fn sibling_progresses_while_locked<F>(
    hold: RuntimeStateHold,
    executor: &tokio::runtime::Handle,
    work: F,
) -> (bool, F::Output)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let work = tokio::spawn(work);
    let (finished, sibling) = tokio::sync::oneshot::channel();
    executor.spawn(async move {
        for _ in 0..10 {
            // simulated-delay: the sibling task's own work, which is what the executor must keep running.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = finished.send(());
    });
    let progressed = tokio::time::timeout(Duration::from_secs(5), sibling)
        .await
        .is_ok_and(|delivered| delivered.is_ok());
    hold.release();
    let outcome = work.await.expect("the spawned work did not panic");
    (progressed, outcome)
}

/// A second Installation with its own single-threaded executor.
///
/// The responder's work has to be observed on the thread that runs it, so this
/// Installation owns a runtime the test can spawn a sibling task onto.
struct RemoteInstallation {
    runtime: Arc<CoreRuntime>,
    executor: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RemoteInstallation {
    fn open(directory: std::path::PathBuf) -> Self {
        let (ready, opened) = std::sync::mpsc::channel();
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a single-threaded executor builds");
            executor.block_on(async move {
                let runtime = Arc::new(
                    CoreRuntime::open_with_clock_and_readiness(
                        CoreRuntimeConfig::new(&directory),
                        Box::new(SystemClock),
                        true,
                    )
                    .await
                    .expect("the remote Installation opens"),
                );
                ready
                    .send((Arc::clone(&runtime), tokio::runtime::Handle::current()))
                    .expect("the test is still waiting for the remote Installation");
                let _ = stopping.await;
                let _ = runtime.shutdown().await;
            });
        });
        let (runtime, executor) = opened.recv().expect("the remote Installation opened");
        Self {
            runtime,
            executor,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn finish(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RemoteInstallation {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Verifies: TEAM.SYS.TRANS-R03, TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "current_thread")]
async fn inbound_log_sync_receiver_calls_do_not_block_the_executor() {
    let root = cluster_root("inbound-receiver-off-executor");
    let _ = std::fs::remove_dir_all(&root);
    let holder = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("a")),
        Box::new(SystemClock),
        true,
    )
    .await
    .expect("the record holder opens");
    let remote = RemoteInstallation::open(root.join("b"));
    holder.remember_peer_address_for_test(remote.runtime.endpoint_address_for_test());
    remote
        .runtime
        .remember_peer_address_for_test(holder.endpoint_address_for_test());

    let workspace = holder.create_workspace("Async boundary").unwrap();
    let id = workspace.id();
    let now = crate::runtime::Clock::now_unix_ms(&SystemClock);
    let invitation = holder
        .create_workspace_invitation_for_test(id, now, now.saturating_add(600_000))
        .expect("the creator issues an invitation");
    remote
        .runtime
        .join_workspace(&invitation)
        .await
        .expect("the remote Installation joins");
    holder
        .append_record(id, PACKAGE, "message.post", b"one")
        .expect("the creator appends a record");

    // With the remote Installation's state held, its responder cannot finish a
    // single receiver call. Its executor must stay free anyway.
    let hold = remote.runtime.hold_runtime_state_for_test();
    let address = remote.runtime.endpoint_address_for_test();
    let peer = remote.runtime.public_identity();
    let session = {
        let holder = Arc::new(holder);
        let dialling = Arc::clone(&holder);
        let (progressed, session) =
            sibling_progresses_while_locked(hold, &remote.executor, async move {
                dialling
                    .replicate_records_to_address_for_test(id, PACKAGE, address, peer)
                    .await
            })
            .await;
        assert!(
            progressed,
            "the responder's executor was blocked by a receiver call"
        );
        assert!(
            session.is_ok(),
            "the session completes once the lock is released: {session:?}"
        );
        holder
    };

    let mut delivered = remote
        .runtime
        .subscribe_records(id, PACKAGE, None)
        .expect("the remote Installation holds the scope");
    let mut received = 0;
    while let Ok(Some(_)) = delivered.next_event_timeout(Duration::ZERO) {
        received += 1;
    }
    assert_eq!(received, 1, "the record arrived once the lock was released");
    session.shutdown().await.unwrap();
    remote.finish();
    let _ = std::fs::remove_dir_all(&root);
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "current_thread")]
async fn worker_job_load_does_not_block_the_executor() {
    let root = cluster_root("worker-load-off-executor");
    let _ = std::fs::remove_dir_all(&root);
    let runtime = Arc::new(
        CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("a")),
            Box::new(SystemClock),
            true,
        )
        .await
        .expect("the Installation opens"),
    );
    let workspace = runtime.create_workspace("Worker load").unwrap();
    runtime
        .append_record(workspace.id(), PACKAGE, "message.post", b"one")
        .expect("a record is appended");

    let hold = runtime.hold_runtime_state_for_test();
    let waking = Arc::clone(&runtime);
    let (progressed, ()) =
        sibling_progresses_while_locked(hold, &tokio::runtime::Handle::current(), async move {
            waking.sync_now_for_test();
        })
        .await;
    assert!(
        progressed,
        "the worker held the executor while waiting for runtime state"
    );

    runtime.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test(flavor = "current_thread")]
async fn join_waits_for_the_runtime_lock_off_the_executor() {
    let root = cluster_root("join-lock-off-executor");
    let _ = std::fs::remove_dir_all(&root);
    let issuer = CoreRuntime::open_with_clock_and_readiness(
        CoreRuntimeConfig::new(root.join("issuer")),
        Box::new(SystemClock),
        true,
    )
    .await
    .expect("the issuer opens");
    let joiner = Arc::new(
        CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join("joiner")),
            Box::new(SystemClock),
            true,
        )
        .await
        .expect("the joiner opens"),
    );
    issuer.remember_peer_address_for_test(joiner.endpoint_address_for_test());
    joiner.remember_peer_address_for_test(issuer.endpoint_address_for_test());
    let workspace = issuer.create_workspace("Join under contention").unwrap();
    let now = crate::runtime::Clock::now_unix_ms(&SystemClock);
    let invitation = issuer
        .create_workspace_invitation_for_test(workspace.id(), now, now.saturating_add(600_000))
        .expect("the creator issues an invitation");

    // The pre-network claim needs the state lock, which is held for longer than
    // the join budget.
    let hold = joiner.hold_runtime_state_for_test();
    let joining = Arc::clone(&joiner);
    let (progressed, result) =
        sibling_progresses_while_locked(hold, &tokio::runtime::Handle::current(), async move {
            joining
                .join_workspace_with_budget_for_test(&invitation, Duration::from_millis(100))
                .await
        })
        .await;
    assert!(
        progressed,
        "the join held the executor while waiting for runtime state"
    );
    assert_eq!(result.unwrap_err(), crate::OnboardingError::Timeout);
    assert!(
        joiner.list_workspaces().unwrap().is_empty(),
        "a timed-out join imports nothing"
    );

    issuer.shutdown().await.unwrap();
    joiner.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
