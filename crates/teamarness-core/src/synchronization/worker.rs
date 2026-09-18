//! Serialized ownership of synchronization scheduling and lifecycle.
//!
//! The worker only holds the runtime state lock while reading or committing a
//! small SQLite transaction. Network sessions always run after that guard has
//! been dropped, and are cancelled before the shared Network is shut down.
//!
//! Membership replication and record replication run as two independent loops
//! over the one shared Endpoint. They are separate because a record session
//! with an unreachable peer occupies its whole session budget, and canonical
//! membership must keep moving while that happens: an Installation that cannot
//! learn who is a Member cannot decide who to serve.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::membership_sync::{MembershipDemand, MembershipReplica};
use crate::onboarding::transport::Transport;
use crate::runtime::{RuntimeState, SystemClock};
use crate::synchronization::peer_sessions::{self, PeerSessions};
use crate::synchronization::scheduler::RetryState;
use crate::synchronization::{LogSessionError, Refusal};
use crate::{PublicIdentity, SynchronizationError, WorkspaceId};

const IDLE_RECONCILIATION: Duration = Duration::from_secs(5);
const MAX_DIALS: usize = 4;
/// A full membership sweep every sixth idle tick, so offline peers and missed
/// wakes converge in about thirty seconds without a dial every five.
const MEMBERSHIP_IDLE_EVERY: u32 = 6;
/// A membership session is small, so an unreachable peer is retried sooner than
/// a record session would be. This is the ceiling on that backoff.
const MEMBERSHIP_RETRY_CEILING: Duration = Duration::from_secs(5);

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

#[derive(Debug, Clone)]
struct MembershipJob {
    workspace: WorkspaceId,
    peer: PublicIdentity,
    generation: i64,
}

/// Everything one worker pass needs. Bundled so the pass functions stay
/// readable as the worker grows a second protocol.
struct WorkerContext {
    state: Weak<Mutex<RuntimeState>>,
    network: Arc<Transport>,
    local: PublicIdentity,
    sessions: Arc<PeerSessions>,
    replica: Arc<dyn MembershipReplica>,
    demand: Arc<MembershipDemand>,
    stopped: Arc<AtomicBool>,
    cancel: Arc<Notify>,
}

/// Owns the background synchronization command loop for one Core runtime.
///
/// A worker is started only after storage and projection recovery have
/// completed. Its join handle is awaited by [`CoreRuntime::shutdown`].
pub(crate) struct SyncWorker {
    stopped: Arc<AtomicBool>,
    commands: mpsc::UnboundedSender<WorkerCommand>,
    wake: Arc<Notify>,
    membership_wake: Arc<Notify>,
    cancel: Arc<Notify>,
    join: Mutex<Vec<JoinHandle<()>>>,
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
        replica: Arc<dyn MembershipReplica>,
        demand: Arc<MembershipDemand>,
    ) -> Arc<Self> {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let worker = Arc::new(Self {
            stopped: Arc::new(AtomicBool::new(false)),
            commands,
            wake: Arc::new(Notify::new()),
            membership_wake: Arc::new(Notify::new()),
            cancel: Arc::new(Notify::new()),
            join: Mutex::new(Vec::new()),
        });
        let context = Arc::new(WorkerContext {
            state,
            network,
            local,
            sessions,
            replica,
            demand,
            stopped: Arc::clone(&worker.stopped),
            cancel: Arc::clone(&worker.cancel),
        });
        let wake = Arc::clone(&worker.wake);
        let records = tokio::spawn({
            let context = Arc::clone(&context);
            async move {
                run_records(&context, wake, command_rx).await;
            }
        });
        let membership_wake = Arc::clone(&worker.membership_wake);
        let membership = tokio::spawn(async move {
            run_membership(&context, membership_wake).await;
        });
        *worker
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = vec![records, membership];
        worker
    }

    /// Requests reconciliation after a committed local or remote write.
    pub(crate) fn wake(&self) {
        if !self.stopped.load(Ordering::Acquire) {
            let _ = self.commands.send(WorkerCommand::Wake);
            self.wake.notify_one();
            self.membership_wake.notify_one();
        }
    }

    pub(crate) async fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.commands.send(WorkerCommand::Cancel);
        self.cancel.notify_waiters();
        self.wake.notify_waiters();
        self.membership_wake.notify_waiters();
        let join = std::mem::take(
            &mut *self
                .join
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for handle in join {
            let _ = handle.await;
        }
    }
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.cancel.notify_waiters();
        self.wake.notify_waiters();
        self.membership_wake.notify_waiters();
        for handle in std::mem::take(
            self.join
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ) {
            handle.abort();
        }
    }
}

/// Per-peer membership backoff: when a peer's next attempt is due, and how far
/// its retry delay has grown.
type MembershipRetry = BTreeMap<(WorkspaceId, PublicIdentity), (Instant, RetryState)>;

async fn run_records(
    context: &WorkerContext,
    wake: Arc<Notify>,
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
) {
    let mut idle = tokio::time::interval(IDLE_RECONCILIATION);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry = BTreeMap::new();
    let mut first = true;
    while !context.stopped.load(Ordering::Acquire) {
        if first {
            first = false;
            record_pass(context, &mut retry, true).await;
            continue;
        }
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(WorkerCommand::Wake) => record_pass(context, &mut retry, false).await,
                    Some(WorkerCommand::Cancel) => context.cancel.notify_waiters(),
                    None => return,
                }
            }
            _ = wake.notified() => record_pass(context, &mut retry, false).await,
            _ = idle.tick() => record_pass(context, &mut retry, true).await,
            _ = context.cancel.notified() => {}
        }
    }
}

async fn run_membership(context: &WorkerContext, wake: Arc<Notify>) {
    let mut idle = tokio::time::interval(IDLE_RECONCILIATION);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry = MembershipRetry::new();
    let mut idle_ticks: u32 = 0;
    let mut first = true;
    while !context.stopped.load(Ordering::Acquire) {
        if first {
            first = false;
            // The first pass after start is a full sweep, so an Installation
            // that was offline catches up with every Member it knows.
            membership_pass(context, &mut retry, true).await;
            continue;
        }
        tokio::select! {
            _ = wake.notified() => membership_pass(context, &mut retry, false).await,
            () = context.demand.changed() => membership_pass(context, &mut retry, false).await,
            _ = idle.tick() => {
                idle_ticks = idle_ticks.wrapping_add(1);
                membership_pass(context, &mut retry, idle_ticks.is_multiple_of(MEMBERSHIP_IDLE_EVERY)).await;
            }
            _ = context.cancel.notified() => {}
        }
    }
}

async fn record_pass(
    context: &WorkerContext,
    retry: &mut BTreeMap<(WorkspaceId, String, PublicIdentity), RetryState>,
    reconcile_idle: bool,
) {
    let Some(state_arc) = context.state.upgrade() else {
        return;
    };
    run_record_pass(context, &state_arc, retry, reconcile_idle).await;
}

async fn membership_pass(context: &WorkerContext, retry: &mut MembershipRetry, idle_due: bool) {
    let Some(state_arc) = context.state.upgrade() else {
        return;
    };
    run_membership_pass(context, &state_arc, retry, idle_due).await;
}

/// One membership pass: reconcile every due Workspace with its Members.
async fn run_membership_pass(
    context: &WorkerContext,
    state_arc: &Arc<Mutex<RuntimeState>>,
    retry: &mut MembershipRetry,
    idle_due: bool,
) {
    let now = Instant::now();
    let due_now =
        |key: &(WorkspaceId, PublicIdentity)| retry.get(key).is_none_or(|(due, _)| *due <= now);
    let forced = context.demand.take_ready(due_now);
    // A Workspace's wake is consumed only when every current Member was dialled
    // and answered, so one unreachable Member never hides the work from the
    // others, and a peer waiting out its backoff keeps the wake alive.
    let mut failed = BTreeSet::new();
    let mut jobs = Vec::new();
    for job in load_membership_jobs(state_arc, context.local, idle_due, &forced) {
        // A peer that just failed is left alone until its backoff elapses, so
        // an Installation that has gone away is not dialled on every wake.
        if due_now(&(job.workspace, job.peer)) {
            jobs.push(job);
        } else {
            failed.insert(job.workspace);
        }
    }
    if jobs.is_empty() {
        return;
    }
    let mut attempted = BTreeMap::<WorkspaceId, i64>::new();
    for chunk in jobs.chunks(MAX_DIALS) {
        if context.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut running = FuturesUnordered::new();
        for job in chunk {
            attempted.insert(job.workspace, job.generation);
            let state = Arc::clone(state_arc);
            let network = Arc::clone(&context.network);
            let replica = Arc::clone(&context.replica);
            let cancel = context.cancel.clone();
            let local = context.local;
            running.push(async move {
                // Re-check right before dialling: a revocation committed while
                // this pass was queued must not produce a dial.
                if !still_current(&state, local, job.peer, job.workspace) {
                    return (job.clone(), false);
                }
                let session = network.run_membership_session(
                    iroh_base::EndpointAddr::new(job.peer.as_public_key()),
                    replica,
                    job.workspace,
                    job.peer,
                );
                tokio::select! {
                    result = session => (job.clone(), result.is_ok()),
                    _ = cancel.notified() => (job.clone(), false),
                }
            });
        }
        while let Some((job, success)) = running.next().await {
            let key = (job.workspace, job.peer);
            if success {
                if let Some((due, state)) = retry.get_mut(&key) {
                    state.succeeded();
                    *due = Instant::now();
                }
            } else {
                // The Workspace wake stays unconsumed, so a later pass retries.
                // Re-recording the demand here would spin this loop instead.
                failed.insert(job.workspace);
                let entry = retry
                    .entry(key)
                    .or_insert((Instant::now(), RetryState::new()));
                let delay = entry.1.next_delay_jittered().min(MEMBERSHIP_RETRY_CEILING);
                entry.0 = Instant::now() + delay;
            }
        }
    }
    for (workspace, generation) in attempted {
        if !failed.contains(&workspace) {
            let _ = consume_membership_wake(state_arc, workspace, generation);
        }
    }
}

async fn run_record_pass(
    context: &WorkerContext,
    state_arc: &Arc<Mutex<RuntimeState>>,
    retry: &mut BTreeMap<(WorkspaceId, String, PublicIdentity), RetryState>,
    reconcile_idle: bool,
) {
    let jobs = load_jobs(state_arc, context.local, reconcile_idle);
    for chunk in jobs.chunks(MAX_DIALS) {
        if context.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut running = FuturesUnordered::new();
        for job in chunk {
            let key = (job.workspace, job.package.clone(), job.peer);
            let state = Arc::clone(state_arc);
            let network = Arc::clone(&context.network);
            let cancel = context.cancel.clone();
            let local = context.local;
            let demand = Arc::clone(&context.demand);
            // Subscribe before the retry sleep, so a revocation committed while
            // this job waits is observed instead of being slept through.
            let mut cancelled_peer = context.sessions.subscribe(job.peer);
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
                    return (job.clone(), None);
                }
                let receiver: Arc<dyn crate::synchronization::SyncReceiver> =
                    Arc::new(crate::runtime::RuntimeSyncReceiver {
                        state: Arc::downgrade(&state),
                        clock: Arc::new(SystemClock),
                        demand,
                    });
                let session = network.send_log_sync(
                    iroh_base::EndpointAddr::new(job.peer.as_public_key()),
                    receiver,
                    job.workspace,
                    job.package.clone(),
                    job.peer,
                );
                tokio::select! {
                    result = session => (job.clone(), Some(result)),
                    _ = cancel.notified() => (job.clone(), None),
                    () = peer_sessions::cancelled(&mut cancelled_peer) => (job.clone(), None),
                }
            });
        }
        while let Some((job, result)) = running.next().await {
            let key = (job.workspace, job.package.clone(), job.peer);
            match result {
                Some(Ok(_)) => {
                    retry.entry(key).or_insert_with(RetryState::new).succeeded();
                    let _ = consume_wake(state_arc, &job);
                }
                // A peer that does not know this Installation as a Member is
                // exactly the case a membership session resolves.
                Some(Err(LogSessionError::Refused(Refusal::NotAuthorized))) => {
                    context.demand.note(job.workspace, job.peer);
                }
                Some(Err(_)) | None => {}
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

/// Builds the membership jobs for this pass.
///
/// An Installation that is no longer a current Member of a Workspace schedules
/// nothing for it, in either pass.
fn load_membership_jobs(
    state: &Arc<Mutex<RuntimeState>>,
    local: PublicIdentity,
    idle_due: bool,
    forced: &BTreeSet<(WorkspaceId, PublicIdentity)>,
) -> Vec<MembershipJob> {
    let Ok(state) = state.try_lock() else {
        return Vec::new();
    };
    if state.poisoned {
        return Vec::new();
    }
    let mut jobs = Vec::new();
    let Ok(mut statement) = state.connection.prepare(
        "SELECT workspace_id, wake_generation, consumed_generation
         FROM membership_sync_wakes ORDER BY workspace_id",
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
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    let Ok(rows) = rows else {
        return jobs;
    };
    for (workspace, generation, consumed) in rows.flatten() {
        let Some(projection) = state.memberships.get(&workspace) else {
            continue;
        };
        if !projection.contains_member(local) {
            continue;
        }
        let mut peers = forced
            .iter()
            .filter(|(candidate, _)| *candidate == workspace)
            .map(|(_, peer)| *peer)
            .collect::<BTreeSet<_>>();
        if generation > consumed || idle_due {
            peers.extend(
                projection
                    .snapshot()
                    .members()
                    .iter()
                    .map(crate::MemberSnapshot::identity),
            );
        }
        for peer in peers {
            if peer != local && projection.contains_member(peer) {
                jobs.push(MembershipJob {
                    workspace,
                    peer,
                    generation,
                });
            }
        }
    }
    jobs
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
        if !projection.contains_member(local) {
            continue;
        }
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

fn consume_membership_wake(
    state: &Arc<Mutex<RuntimeState>>,
    workspace: WorkspaceId,
    generation: i64,
) -> Result<(), SynchronizationError> {
    let mut state = state
        .try_lock()
        .map_err(|_| SynchronizationError::RuntimeUnavailable)?;
    let tx = state
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::Database))?;
    tx.execute(
        "UPDATE membership_sync_wakes SET consumed_generation=CASE
             WHEN consumed_generation < ?2 THEN ?2 ELSE consumed_generation END
         WHERE workspace_id=?1",
        rusqlite::params![workspace.as_bytes().as_slice(), generation],
    )
    .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::Database))?;
    tx.commit()
        .map_err(|_| SynchronizationError::Storage(crate::error::StorageError::UncertainOutcome))
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

    #[test]
    fn a_full_membership_sweep_runs_about_every_thirty_seconds() {
        assert_eq!(
            IDLE_RECONCILIATION * MEMBERSHIP_IDLE_EVERY,
            Duration::from_secs(30)
        );
    }
}
