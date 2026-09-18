//! Serialized ownership of synchronization scheduling and lifecycle.
//!
//! The worker only holds the runtime state lock while reading or committing a
//! small SQLite transaction. Network sessions always run after that guard has
//! been dropped, and are cancelled before the shared Network is shut down.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::onboarding::transport::Transport;
use crate::runtime::RuntimeState;
use crate::synchronization::peer_sessions::{self, PeerSessions};
use crate::synchronization::scheduler::RetryState;
use crate::{PublicIdentity, SynchronizationError, WorkspaceId};

const IDLE_RECONCILIATION: Duration = Duration::from_secs(5);
const MAX_DIALS: usize = 4;

enum WorkerCommand {
    Wake,
    Cancel,
}

#[derive(Debug, Clone)]
struct Job {
    workspace: WorkspaceId,
    package: String,
    peer: PublicIdentity,
    generation: i64,
}

/// Owns the background synchronization command loop for one Core runtime.
///
/// A worker is started only after storage and projection recovery have
/// completed. Its join handle is awaited by [`CoreRuntime::shutdown`].
pub(crate) struct SyncWorker {
    stopped: Arc<AtomicBool>,
    commands: mpsc::UnboundedSender<WorkerCommand>,
    wake: Arc<Notify>,
    cancel: Arc<Notify>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SyncWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyncWorker")
            .field("stopped", &self.stopped.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl SyncWorker {
    pub(crate) fn start(
        state: Weak<Mutex<RuntimeState>>,
        network: Arc<Transport>,
        local: PublicIdentity,
        sessions: Arc<PeerSessions>,
    ) -> Arc<Self> {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let worker = Arc::new(Self {
            stopped: Arc::new(AtomicBool::new(false)),
            commands,
            wake: Arc::new(Notify::new()),
            cancel: Arc::new(Notify::new()),
            join: Mutex::new(None),
        });
        let stopped = Arc::clone(&worker.stopped);
        let wake = Arc::clone(&worker.wake);
        let cancel = Arc::clone(&worker.cancel);
        let join = tokio::spawn(async move {
            run(
                state, network, local, sessions, stopped, wake, cancel, command_rx,
            )
            .await;
        });
        *worker
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(join);
        worker
    }

    /// Requests reconciliation after a committed local or remote write.
    pub(crate) fn wake(&self) {
        if !self.stopped.load(Ordering::Acquire) {
            let _ = self.commands.send(WorkerCommand::Wake);
            self.wake.notify_one();
        }
    }

    pub(crate) async fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.commands.send(WorkerCommand::Cancel);
        self.cancel.notify_waiters();
        self.wake.notify_waiters();
        let join = self
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(join) = join {
            let _ = join.await;
        }
    }
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.cancel.notify_waiters();
        self.wake.notify_waiters();
        if let Some(join) = self
            .join
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            join.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    state: Weak<Mutex<RuntimeState>>,
    network: Arc<Transport>,
    local: PublicIdentity,
    sessions: Arc<PeerSessions>,
    stopped: Arc<AtomicBool>,
    wake: Arc<Notify>,
    cancel: Arc<Notify>,
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
) {
    let mut idle = tokio::time::interval(IDLE_RECONCILIATION);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry = BTreeMap::<(WorkspaceId, String, PublicIdentity), RetryState>::new();
    let mut first = true;
    while !stopped.load(Ordering::Acquire) {
        if first {
            first = false;
            run_once(
                &state, &network, local, &sessions, &stopped, &cancel, &mut retry, true,
            )
            .await;
            continue;
        }
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(WorkerCommand::Wake) => run_once(&state, &network, local, &sessions, &stopped, &cancel, &mut retry, false).await,
                    Some(WorkerCommand::Cancel) => cancel.notify_waiters(),
                    None => return,
                }
            }
            _ = wake.notified() => {
                run_once(&state, &network, local, &sessions, &stopped, &cancel, &mut retry, false).await;
            }
            _ = idle.tick() => {
                run_once(&state, &network, local, &sessions, &stopped, &cancel, &mut retry, true).await;
            }
            _ = cancel.notified() => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    state: &Weak<Mutex<RuntimeState>>,
    network: &Arc<Transport>,
    local: PublicIdentity,
    sessions: &Arc<PeerSessions>,
    stopped: &Arc<AtomicBool>,
    cancel: &Arc<Notify>,
    retry: &mut BTreeMap<(WorkspaceId, String, PublicIdentity), RetryState>,
    reconcile_idle: bool,
) {
    let Some(state_arc) = state.upgrade() else {
        return;
    };
    let jobs = load_jobs(&state_arc, local, reconcile_idle);
    for chunk in jobs.chunks(MAX_DIALS) {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let mut running = FuturesUnordered::new();
        for job in chunk {
            let key = (job.workspace, job.package.clone(), job.peer);
            let state = Arc::clone(&state_arc);
            let network = Arc::clone(network);
            let cancel = cancel.clone();
            // Subscribe before the retry sleep, so a revocation committed while
            // this job waits is observed instead of being slept through.
            let mut cancelled_peer = sessions.subscribe(job.peer);
            let delay = retry
                .entry(key.clone())
                .or_insert_with(RetryState::new)
                .next_delay_jittered();
            running.push(async move {
                // The first retry is immediate; subsequent retries are capped
                // and jittered to avoid synchronized member storms.
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
                if !still_current(&state, local, job.peer, job.workspace) {
                    return (job.clone(), false);
                }
                let receiver: Arc<dyn crate::synchronization::SyncReceiver> =
                    Arc::new(crate::runtime::RuntimeSyncReceiver {
                        state: Arc::downgrade(&state),
                    });
                let session = network.send_log_sync(
                    iroh_base::EndpointAddr::new(job.peer.as_public_key()),
                    receiver,
                    job.workspace,
                    job.package.clone(),
                    job.peer,
                );
                tokio::select! {
                    result = session => (job.clone(), result.is_ok()),
                    _ = cancel.notified() => (job.clone(), false),
                    () = peer_sessions::cancelled(&mut cancelled_peer) => (job.clone(), false),
                }
            });
        }
        while let Some((job, success)) = running.next().await {
            let key = (job.workspace, job.package.clone(), job.peer);
            if success {
                retry.entry(key).or_insert_with(RetryState::new).succeeded();
                let _ = consume_wake(&state_arc, &job);
            }
        }
    }
}

/// Re-checks the gate after the retry sleep, so a job queued before a
/// revocation never dials. A busy lock skips the job until the next pass.
fn still_current(
    state: &Arc<Mutex<RuntimeState>>,
    local: PublicIdentity,
    peer: PublicIdentity,
    workspace: WorkspaceId,
) -> bool {
    let Ok(state) = state.try_lock() else {
        return false;
    };
    !state.poisoned
        && state.memberships.get(&workspace).is_some_and(|projection| {
            projection.contains_member(local) && projection.contains_member(peer)
        })
}

fn load_jobs(state: &Arc<Mutex<RuntimeState>>, local: PublicIdentity, idle: bool) -> Vec<Job> {
    let Ok(state) = state.try_lock() else {
        // Never block the async executor behind a synchronous runtime mutex.
        return Vec::new();
    };
    if state.poisoned {
        return Vec::new();
    }
    let mut jobs = Vec::new();
    let Ok(mut statement) = state.connection.prepare(
        "SELECT s.workspace_id,s.package_identity,w.wake_generation,w.consumed_generation
         FROM sync_scopes s JOIN durable_wakes w USING(scope_id)
         ORDER BY s.workspace_id,s.package_identity",
    ) else {
        return jobs;
    };
    let rows = statement.query_map([], |row| {
        let workspace: Vec<u8> = row.get(0)?;
        Ok((
            WorkspaceId::from_bytes(
                workspace
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            ),
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    });
    let Ok(rows) = rows else {
        return jobs;
    };
    for row in rows.flatten() {
        let (workspace, package, generation, consumed) = row;
        if !idle && generation <= consumed {
            continue;
        }
        let Some(projection) = state.memberships.get(&workspace) else {
            continue;
        };
        for member in projection.snapshot().members() {
            let peer = member.identity();
            if peer != local {
                jobs.push(Job {
                    workspace,
                    package: package.clone(),
                    peer,
                    generation,
                });
            }
        }
    }
    jobs
}

fn consume_wake(state: &Arc<Mutex<RuntimeState>>, job: &Job) -> Result<(), SynchronizationError> {
    let mut state = state
        .try_lock()
        .map_err(|_| SynchronizationError::RuntimeUnavailable)?;
    let tx = state
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::Database))?;
    tx.execute(
        "UPDATE durable_wakes SET consumed_generation=CASE
             WHEN consumed_generation < ?2 THEN ?2 ELSE consumed_generation END
         WHERE scope_id=(SELECT scope_id FROM sync_scopes
                         WHERE workspace_id=?1 AND package_identity=?3)",
        rusqlite::params![
            job.workspace.as_bytes().as_slice(),
            job.generation,
            job.package
        ],
    )
    .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::Database))?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::UncertainOutcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_bounds_dial_batch() {
        assert_eq!(MAX_DIALS, 4);
        assert_eq!(MAX_DIALS, crate::network::MAX_OUTBOUND_PEERS);
        assert!(IDLE_RECONCILIATION > Duration::from_secs(1));
    }
}
