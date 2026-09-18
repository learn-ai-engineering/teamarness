//! The asynchronous driver around the synchronization scheduler.
//!
//! The driver owns no policy: it loads what is durably due, asks
//! [`SchedulerCore`] what to run next, runs it, and applies the actions the
//! scheduler returns. Everything it does with the runtime state crosses to a
//! blocking thread, so nothing is silently skipped because a lock was busy.
//!
//! Membership and record replication share one schedule, and stay independent
//! where it matters: at most one session per peer, with that peer's membership
//! job dispatched before its record jobs, and backoff that belongs to the peer
//! rather than to the protocol. A peer that has gone away therefore never
//! delays another peer's membership convergence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::time::Instant;

use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;

use crate::blocking::StateUnavailable;
use crate::membership_sync::{MembershipDemand, MembershipReplica, MembershipSessionError};
use crate::onboarding::transport::Transport;
use crate::runtime::RuntimeState;
use crate::synchronization::limits::SchedulerLimits;
use crate::synchronization::scheduler::{
    DueJob, JobKey, SchedulerAction, SchedulerCore, SessionOutcome,
};
use crate::synchronization::{LogSessionError, Refusal, SyncServices};
use crate::{PublicIdentity, WorkspaceId};

/// Why the driver reloaded the durable job set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadKind {
    /// Something committed, or a peer asked for a membership session.
    Wake,
    /// Record scopes that have gone quiet are reconciled anyway.
    RecordIdle,
    /// The same, for canonical membership.
    MembershipIdle,
    /// Everything, whether or not it has gone quiet. A test asks for this
    /// instead of waiting out an idle interval.
    #[cfg(test)]
    FullSweep,
}

/// Runs one scheduled session. Production dials; tests script the outcome.
pub(crate) trait SessionRunner: Send + Sync + 'static {
    fn run(
        &self,
        job: DueJob,
        stop: watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SessionOutcome> + Send>>;
}

/// The production runner: real sessions over the shared Endpoint.
pub(crate) struct NetworkSessionRunner {
    network: Arc<Transport>,
    services: SyncServices,
    replica: Arc<dyn MembershipReplica>,
    demand: Arc<MembershipDemand>,
    state: Weak<Mutex<RuntimeState>>,
    local: PublicIdentity,
}

impl NetworkSessionRunner {
    pub(crate) fn new(
        network: Arc<Transport>,
        services: SyncServices,
        replica: Arc<dyn MembershipReplica>,
        demand: Arc<MembershipDemand>,
        state: Weak<Mutex<RuntimeState>>,
        local: PublicIdentity,
    ) -> Self {
        Self {
            network,
            services,
            replica,
            demand,
            state,
            local,
        }
    }

    /// Re-checks the gate before dialling, so a job queued before a revocation
    /// never reaches the network.
    async fn still_current(&self, peer: PublicIdentity, workspace: WorkspaceId) -> bool {
        let local = self.local;
        crate::blocking::run_with_state(&self.state, move |state| {
            state.memberships.get(&workspace).is_some_and(|projection| {
                projection.contains_member(local) && projection.contains_member(peer)
            })
        })
        .await
        .unwrap_or(false)
    }
}

impl SessionRunner for NetworkSessionRunner {
    fn run(
        &self,
        job: DueJob,
        mut stop: watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SessionOutcome> + Send>> {
        let network = Arc::clone(&self.network);
        let services = self.services.clone();
        let replica = Arc::clone(&self.replica);
        let demand = Arc::clone(&self.demand);
        let state = Weak::clone(&self.state);
        let local = self.local;
        Box::pin(async move {
            let runner = Self {
                network,
                services,
                replica,
                demand,
                state,
                local,
            };
            let peer = job.key.peer();
            let workspace = job.key.workspace();
            if !runner.still_current(peer, workspace).await {
                return SessionOutcome::LocalNotAuthorized;
            }
            let address = iroh_base::EndpointAddr::new(peer.as_public_key());
            let outcome = match &job.key {
                JobKey::Membership { workspace, peer } => {
                    let session = runner.network.run_membership_session(
                        address,
                        Arc::clone(&runner.replica),
                        *workspace,
                        *peer,
                    );
                    tokio::select! {
                        result = session => membership_outcome(&job.key, result),
                        _ = stop.changed() => SessionOutcome::Cancelled,
                    }
                }
                JobKey::Records {
                    workspace,
                    package,
                    peer,
                    ..
                } => {
                    let session = runner.network.send_log_sync(
                        address,
                        Arc::clone(&runner.services.receiver),
                        *workspace,
                        package.clone(),
                        *peer,
                    );
                    tokio::select! {
                        result = session => record_outcome(&job.key, result),
                        _ = stop.changed() => SessionOutcome::Cancelled,
                    }
                }
            };
            if matches!(outcome, SessionOutcome::RefusedNotAuthorized) {
                runner.demand.note(workspace, peer);
            }
            // Only record sessions say anything about a scope's availability;
            // membership sessions belong to the Workspace, not to one scope.
            if let JobKey::Records {
                workspace, package, ..
            } = &job.key
            {
                report_health(
                    &runner.services.health,
                    crate::synchronization::scope_digest(package, *workspace),
                    peer,
                    outcome,
                );
            }
            outcome
        })
    }
}

/// Tells the health registry what one record session saw of its peer.
///
/// A cancelled session, and one the local gate refused, say nothing about the
/// peer, so they remove its entry rather than blaming it.
fn report_health(
    health: &Arc<crate::synchronization::SyncHealthRegistry>,
    scope: [u8; 32],
    peer: PublicIdentity,
    outcome: SessionOutcome,
) {
    let (reported, truncated) = match outcome {
        SessionOutcome::Completed { truncated } => (
            Some(crate::synchronization::PeerOutcome::Reachable),
            Some(truncated),
        ),
        SessionOutcome::Unreachable => {
            (Some(crate::synchronization::PeerOutcome::Unreachable), None)
        }
        SessionOutcome::Failed | SessionOutcome::RefusedNotAuthorized => {
            (Some(crate::synchronization::PeerOutcome::Failed), None)
        }
        SessionOutcome::LocalNotAuthorized | SessionOutcome::Cancelled => (None, None),
    };
    health.record_outcome(scope, peer, reported, truncated);
}

/// Classifies a record session, naming the failure kind once in the log.
fn record_outcome(
    key: &JobKey,
    result: Result<crate::synchronization::ExchangeReport, LogSessionError>,
) -> SessionOutcome {
    let error = match result {
        Ok(report) => {
            return SessionOutcome::Completed {
                truncated: report.sent_truncated || report.peer_truncated,
            };
        }
        Err(error) => error,
    };
    let outcome = match &error {
        LogSessionError::Refused(Refusal::NotAuthorized) => SessionOutcome::RefusedNotAuthorized,
        LogSessionError::LocalNotAuthorized => SessionOutcome::LocalNotAuthorized,
        LogSessionError::Cancelled => SessionOutcome::Cancelled,
        // A peer that could not be reached at all is a different thing from one
        // that answered and then failed.
        LogSessionError::Transport(_) => SessionOutcome::Unreachable,
        _ => SessionOutcome::Failed,
    };
    if !matches!(
        outcome,
        SessionOutcome::Cancelled | SessionOutcome::LocalNotAuthorized
    ) {
        report_failure(key, &error.to_string());
    }
    outcome
}

fn membership_outcome(
    key: &JobKey,
    result: Result<crate::membership_sync::MembershipSessionReport, MembershipSessionError>,
) -> SessionOutcome {
    let error = match result {
        Ok(_) => return SessionOutcome::Completed { truncated: false },
        Err(error) => error,
    };
    let outcome = match &error {
        MembershipSessionError::Transport(_) => SessionOutcome::Unreachable,
        MembershipSessionError::Protocol(_)
        | MembershipSessionError::Invalid
        | MembershipSessionError::Timeout => SessionOutcome::Failed,
    };
    report_failure(key, &error.to_string());
    outcome
}

/// Names a failure once, with no peer identity and no payload.
fn report_failure(key: &JobKey, error: &str) {
    eprintln!("sync session {} failed: {error}", key.kind());
}

/// Owns the background synchronization schedule for one Core runtime.
pub(crate) struct SyncWorker {
    wake: Arc<Notify>,
    stop: watch::Sender<bool>,
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    paused: Arc<AtomicBool>,
    /// Asks the next reload to be a full sweep.
    #[cfg(test)]
    sweep: Arc<AtomicBool>,
    /// How many durable loads this driver has run. A burst of wakes must cost
    /// one follow-up load, not one per wake.
    #[cfg(test)]
    loads: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    snapshot: Arc<Mutex<Vec<crate::synchronization::scheduler::PeerSnapshot>>>,
}

impl std::fmt::Debug for SyncWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyncWorker")
            .field("stopped", &*self.stop.borrow())
            .finish_non_exhaustive()
    }
}

impl SyncWorker {
    pub(crate) fn start(
        wake: Arc<Notify>,
        state: Weak<Mutex<RuntimeState>>,
        local: PublicIdentity,
        runner: Arc<dyn SessionRunner>,
        demand: Arc<MembershipDemand>,
        limits: SchedulerLimits,
        jitter: fn(Duration) -> Duration,
    ) -> Arc<Self> {
        let (stop, stop_rx) = watch::channel(false);
        let worker = Arc::new(Self {
            wake,
            stop,
            join: Mutex::new(None),
            paused: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            sweep: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            loads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            snapshot: Arc::new(Mutex::new(Vec::new())),
        });
        let driver = Driver {
            state,
            local,
            runner,
            demand,
            limits,
            wake: Arc::clone(&worker.wake),
            paused: Arc::clone(&worker.paused),
            stop: stop_rx,
            #[cfg(test)]
            sweep: Arc::clone(&worker.sweep),
            #[cfg(test)]
            loads: Arc::clone(&worker.loads),
            #[cfg(test)]
            snapshot: Arc::clone(&worker.snapshot),
        };
        let join = tokio::spawn(async move { driver.run(jitter).await });
        *worker
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(join);
        worker
    }

    /// Requests a reload. Any number of wakes leave exactly one stored permit,
    /// so a burst costs one follow-up load, not one per wake.
    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Requests the reconciliation the idle intervals would eventually do,
    /// without waiting one out.
    #[cfg(test)]
    pub(crate) fn sweep_for_test(&self) {
        self.sweep.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    /// How many durable loads this driver has run.
    #[cfg(test)]
    pub(crate) fn loads_for_test(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }

    /// What the scheduler currently knows about each peer.
    #[cfg(test)]
    pub(crate) fn snapshot_for_test(&self) -> Vec<crate::synchronization::scheduler::PeerSnapshot> {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Stops scheduling and dispatching. Durable loads still run.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn pause_for_test(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        if !paused {
            self.wake.notify_one();
        }
    }

    pub(crate) async fn shutdown(&self) {
        if self.stop.send_replace(true) {
            return;
        }
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
        let _ = self.stop.send_replace(true);
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

struct Driver {
    state: Weak<Mutex<RuntimeState>>,
    local: PublicIdentity,
    runner: Arc<dyn SessionRunner>,
    demand: Arc<MembershipDemand>,
    limits: SchedulerLimits,
    wake: Arc<Notify>,
    paused: Arc<AtomicBool>,
    stop: watch::Receiver<bool>,
    #[cfg(test)]
    sweep: Arc<AtomicBool>,
    #[cfg(test)]
    loads: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    snapshot: Arc<Mutex<Vec<crate::synchronization::scheduler::PeerSnapshot>>>,
}

impl Driver {
    async fn run(mut self, jitter: fn(Duration) -> Duration) {
        let mut core = SchedulerCore::new(self.limits, jitter);
        let mut sessions: JoinSet<(JobKey, SessionOutcome)> = JoinSet::new();
        // The first tick is one period away: the sweep below already covers
        // the start, and an immediate tick would be a second load for nothing.
        let mut record_idle = tokio::time::interval_at(
            Instant::now() + self.limits.record_idle,
            self.limits.record_idle,
        );
        record_idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut membership_idle = tokio::time::interval_at(
            Instant::now() + self.limits.membership_idle,
            self.limits.membership_idle,
        );
        membership_idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first pass after start is a full sweep, so an Installation that
        // was away catches up with every peer it knows.
        let mut reload = Some(ReloadKind::RecordIdle);

        loop {
            if reload.is_none() {
                let deadline = core.next_deadline();
                tokio::select! {
                    biased;
                    changed = self.stop.changed() => {
                        if changed.is_err() || *self.stop.borrow() {
                            break;
                        }
                    }
                    Some(finished) = sessions.join_next(), if !sessions.is_empty() => {
                        if let Ok((key, outcome)) = finished {
                            let actions = core.complete(&key, outcome, Instant::now());
                            if self.apply(&mut core, actions).await.is_err() {
                                return;
                            }
                        }
                    }
                    () = self.wake.notified() => reload = Some(ReloadKind::Wake),
                    () = self.demand.changed() => reload = Some(ReloadKind::Wake),
                    _ = record_idle.tick() => reload = Some(ReloadKind::RecordIdle),
                    _ = membership_idle.tick() => reload = Some(ReloadKind::MembershipIdle),
                    () = sleep_until(deadline) => {}
                }
            }
            if *self.stop.borrow() {
                break;
            }
            #[cfg(test)]
            if self.sweep.swap(false, Ordering::AcqRel) {
                reload = Some(ReloadKind::FullSweep);
            }
            if let Some(kind) = reload.take() {
                let attempts = core.last_attempts();
                match self.load(kind, attempts).await {
                    Ok(due) => {
                        #[cfg(test)]
                        self.loads.fetch_add(1, Ordering::Relaxed);
                        core.replace_due(due, Instant::now());
                    }
                    // A dropped runtime has nothing left to schedule, and a
                    // poisoned one must not schedule anything more.
                    Err(_) => return,
                }
            }
            if self.paused.load(Ordering::Acquire) {
                continue;
            }
            #[cfg(test)]
            {
                *self
                    .snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = core.snapshot();
            }
            let now = Instant::now();
            while let Some(job) = core.next_dispatch(now) {
                let runner = Arc::clone(&self.runner);
                let stop = self.stop.clone();
                let key = job.key.clone();
                sessions.spawn(async move {
                    let outcome = runner.run(job, stop).await;
                    (key, outcome)
                });
            }
        }
        self.drain(sessions).await;
    }

    /// Waits at most the shutdown grace for sessions, then abandons them.
    async fn drain(&self, mut sessions: JoinSet<(JobKey, SessionOutcome)>) {
        if sessions.is_empty() {
            return;
        }
        let _ = tokio::time::timeout(self.limits.shutdown_grace, async {
            while sessions.join_next().await.is_some() {}
        })
        .await;
        sessions.abort_all();
    }

    async fn apply(
        &self,
        core: &mut SchedulerCore,
        actions: Vec<SchedulerAction>,
    ) -> Result<(), StateUnavailable> {
        for action in actions {
            match action {
                SchedulerAction::Consume { key, generation } => {
                    consume(&self.state, &key, generation).await?;
                }
                SchedulerAction::ForceMembership { workspace, peer } => {
                    core.force_membership(workspace, peer);
                }
                // Session health is reported to subscriptions by the health
                // registry; the scheduler only says what it saw.
                SchedulerAction::Health { .. } => {}
            }
        }
        Ok(())
    }

    async fn load(
        &self,
        kind: ReloadKind,
        attempts: BTreeMap<JobKey, Instant>,
    ) -> Result<Vec<DueJob>, StateUnavailable> {
        let local = self.local;
        let limits = self.limits;
        let now = Instant::now();
        let forced = self.demand.take_ready(|_| true);
        crate::blocking::run_with_state(&self.state, move |state| {
            load_due_jobs(state, local, kind, limits, &attempts, now, &forced)
        })
        .await
    }
}

/// Completes when `deadline` passes, and otherwise never.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Everything that is durably due right now, for every peer this Installation
/// shares a Workspace with.
fn load_due_jobs(
    state: &mut RuntimeState,
    local: PublicIdentity,
    kind: ReloadKind,
    limits: SchedulerLimits,
    attempts: &BTreeMap<JobKey, Instant>,
    now: Instant,
    forced: &BTreeSet<(WorkspaceId, PublicIdentity)>,
) -> Vec<DueJob> {
    let mut jobs = Vec::new();
    #[cfg(test)]
    let sweeping = kind == ReloadKind::FullSweep;
    #[cfg(not(test))]
    let sweeping = false;
    let idle_due = |key: &JobKey, interval: Duration| {
        sweeping
            || attempts
                .get(key)
                .is_none_or(|attempt| now.duration_since(*attempt) >= interval)
    };
    for (workspace, peer) in forced {
        jobs.push(DueJob {
            key: JobKey::Membership {
                workspace: *workspace,
                peer: *peer,
            },
            generation: 0,
        });
    }
    let membership_progress = peer_progress(
        state,
        "SELECT workspace_id, peer_public_identity, consumed_generation
         FROM membership_peer_progress",
    );
    for (workspace, generation) in membership_wakes(state) {
        for peer in peers_of(state, local, workspace) {
            let key = JobKey::Membership { workspace, peer };
            let consumed = membership_progress
                .get(&(workspace.as_bytes().to_vec(), peer))
                .copied()
                .unwrap_or(0);
            if generation > consumed
                || (kind == ReloadKind::MembershipIdle && idle_due(&key, limits.membership_idle))
            {
                jobs.push(DueJob { key, generation });
            }
        }
    }
    let record_progress = peer_progress(
        state,
        "SELECT scope_id, peer_public_identity, consumed_generation FROM sync_peer_progress",
    );
    for (scope, workspace, package, generation) in record_wakes(state) {
        for peer in peers_of(state, local, workspace) {
            let key = JobKey::Records {
                scope,
                workspace,
                package: package.clone(),
                peer,
            };
            let consumed = record_progress
                .get(&(scope.to_vec(), peer))
                .copied()
                .unwrap_or(0);
            if generation > consumed
                || ((sweeping || kind == ReloadKind::RecordIdle)
                    && idle_due(&key, limits.record_idle))
            {
                jobs.push(DueJob { key, generation });
            }
            if jobs.len() >= limits.max_scheduled_jobs {
                return jobs;
            }
        }
    }
    jobs
}

type ProgressKey = (Vec<u8>, PublicIdentity);

fn peer_progress(state: &RuntimeState, query: &str) -> BTreeMap<ProgressKey, i64> {
    let mut progress = BTreeMap::new();
    let Ok(mut statement) = state.connection.prepare(query) else {
        return progress;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }) else {
        return progress;
    };
    for (owner, peer, consumed) in rows.flatten() {
        let Ok(peer) = <[u8; 32]>::try_from(peer).map(PublicIdentity::from_bytes) else {
            continue;
        };
        let Ok(peer) = peer else { continue };
        progress.insert((owner, peer), consumed);
    }
    progress
}

fn membership_wakes(state: &RuntimeState) -> Vec<(WorkspaceId, i64)> {
    let Ok(mut statement) = state.connection.prepare(
        "SELECT workspace_id, wake_generation FROM membership_sync_wakes ORDER BY workspace_id",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|(workspace, generation)| {
            <[u8; 16]>::try_from(workspace)
                .ok()
                .map(|bytes| (WorkspaceId::from_bytes(bytes), generation))
        })
        .collect()
}

type RecordWake = ([u8; 32], WorkspaceId, String, i64);

fn record_wakes(state: &RuntimeState) -> Vec<RecordWake> {
    // A scope isolated by a local integrity failure is never scheduled.
    let Ok(mut statement) = state.connection.prepare(
        "SELECT s.scope_id, s.workspace_id, s.package_identity, w.wake_generation
         FROM sync_scopes s JOIN durable_wakes w USING(scope_id)
         LEFT JOIN quarantined_scopes q USING(scope_id)
         WHERE q.scope_id IS NULL
         ORDER BY s.workspace_id, s.package_identity",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|(scope, workspace, package, generation)| {
            let scope = <[u8; 32]>::try_from(scope).ok()?;
            let workspace = <[u8; 16]>::try_from(workspace).ok()?;
            Some((
                scope,
                WorkspaceId::from_bytes(workspace),
                package,
                generation,
            ))
        })
        .collect()
}

/// The current Members of a Workspace this Installation is still a Member of.
fn peers_of(
    state: &RuntimeState,
    local: PublicIdentity,
    workspace: WorkspaceId,
) -> Vec<PublicIdentity> {
    let Some(projection) = state.memberships.get(&workspace) else {
        return Vec::new();
    };
    if !projection.contains_member(local) {
        return Vec::new();
    }
    projection
        .snapshot()
        .members()
        .iter()
        .map(crate::MemberSnapshot::identity)
        .filter(|peer| *peer != local)
        .collect()
}

async fn consume(
    state: &Weak<Mutex<RuntimeState>>,
    key: &JobKey,
    generation: i64,
) -> Result<(), StateUnavailable> {
    let key = key.clone();
    crate::blocking::run_with_state(state, move |state| {
        consume_locked(state, &key, generation);
    })
    .await
}

fn consume_locked(state: &mut RuntimeState, key: &JobKey, generation: i64) {
    let result = match key {
        JobKey::Membership { workspace, peer } => state.connection.execute(
            "INSERT INTO membership_peer_progress
                 (workspace_id, peer_public_identity, consumed_generation)
             VALUES (?1, ?2, ?3)
             ON CONFLICT DO UPDATE SET
                 consumed_generation = MAX(consumed_generation, excluded.consumed_generation)",
            rusqlite::params![
                workspace.as_bytes().as_slice(),
                peer.as_bytes().as_slice(),
                generation
            ],
        ),
        JobKey::Records { scope, peer, .. } => state.connection.execute(
            "INSERT INTO sync_peer_progress
                 (scope_id, peer_public_identity, consumed_generation)
             VALUES (?1, ?2, ?3)
             ON CONFLICT DO UPDATE SET
                 consumed_generation = MAX(consumed_generation, excluded.consumed_generation)",
            rusqlite::params![scope.as_slice(), peer.as_bytes().as_slice(), generation],
        ),
    };
    if result.is_err() {
        // Progress that cannot be recorded only means the job is due again.
        eprintln!("sync progress for a {} job was not recorded", key.kind());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::synchronization::scheduler::no_jitter;

    const PACKAGE: &str = "teamarness.test";

    /// A scripted runner. Only the session is scripted: the scheduler, the
    /// driver, the durable loads and the real storage under them are real.
    struct FakeRunner {
        dispatches: Mutex<Vec<(JobKey, Instant)>>,
        outcome: SessionOutcome,
        /// When set, a session waits for this before returning.
        release: Arc<Notify>,
        hold: AtomicBool,
        /// When set, a session never returns and ignores the stop signal.
        wedge: AtomicBool,
        started: Arc<Notify>,
        running: AtomicUsize,
    }

    impl FakeRunner {
        fn new(outcome: SessionOutcome) -> Arc<Self> {
            Arc::new(Self {
                dispatches: Mutex::new(Vec::new()),
                outcome,
                release: Arc::new(Notify::new()),
                hold: AtomicBool::new(false),
                wedge: AtomicBool::new(false),
                started: Arc::new(Notify::new()),
                running: AtomicUsize::new(0),
            })
        }

        fn dispatches(&self) -> Vec<(JobKey, Instant)> {
            self.dispatches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn dispatches_for(&self, peer: PublicIdentity) -> usize {
            self.dispatches()
                .iter()
                .filter(|(key, _)| key.peer() == peer)
                .count()
        }
    }

    impl SessionRunner for FakeRunner {
        fn run(
            &self,
            job: DueJob,
            _stop: watch::Receiver<bool>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SessionOutcome> + Send>> {
            self.dispatches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((job.key, Instant::now()));
            let outcome = self.outcome;
            let release = Arc::clone(&self.release);
            let started = Arc::clone(&self.started);
            let hold = self.hold.load(Ordering::Acquire);
            let wedge = self.wedge.load(Ordering::Acquire);
            self.running.fetch_add(1, Ordering::AcqRel);
            Box::pin(async move {
                started.notify_waiters();
                if wedge {
                    std::future::pending::<()>().await;
                }
                if hold {
                    release.notified().await;
                }
                outcome
            })
        }
    }

    /// A Workspace with one record scope and one other Member, in real storage.
    ///
    /// Storage, the durable wakes and the record scope are the real ones; the
    /// second Member's projection is built directly, because this fixture has
    /// no networking to admit one through.
    struct Fixture {
        state: Arc<Mutex<RuntimeState>>,
        local: PublicIdentity,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        root: std::path::PathBuf,
    }

    impl Fixture {
        fn open(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "teamarness-scheduler-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("the test root is created");
            let stored = crate::storage::open(&root, 0).expect("a migrated database");
            let mut connection = stored.connection;
            let secret = stored.secret_key;
            let local = PublicIdentity::from_bytes(*secret.public().as_bytes())
                .expect("the local identity is valid");
            let summary = crate::workspace::create(&mut connection, &secret, "Scheduling", 0)
                .expect("the Workspace is created");
            let workspace = summary.id();
            crate::synchronization::append(
                &mut connection,
                &secret,
                workspace,
                PACKAGE,
                "message.post",
                b"seed",
                crate::synchronization::MembershipStanding::Current,
                true,
                0,
            )
            .expect("the scope exists");
            let peer = PublicIdentity::from_bytes(
                *iroh_base::SecretKey::from_bytes(&[9; 32])
                    .public()
                    .as_bytes(),
            )
            .expect("the peer identity is valid");
            let projection = crate::membership::WorkspaceProjection::for_replication_test(
                workspace,
                local,
                crate::MembershipOperationId::digest([1]),
                std::collections::BTreeMap::new(),
                vec![local, peer],
            );
            let memberships = BTreeMap::from([(workspace, projection)]);
            let state = Arc::new(Mutex::new(RuntimeState::for_scheduler_tests(
                connection,
                secret,
                memberships,
            )));
            Self {
                state,
                local,
                peer,
                workspace,
                root,
            }
        }

        fn finish(self) {
            drop(self.state);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn start(
        fixture: &Fixture,
        runner: Arc<FakeRunner>,
        limits: SchedulerLimits,
    ) -> Arc<SyncWorker> {
        SyncWorker::start(
            Arc::clone(
                &fixture
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .sync_wake,
            ),
            Arc::downgrade(&fixture.state),
            fixture.local,
            runner,
            MembershipDemand::new(),
            limits,
            no_jitter,
        )
    }

    /// Waits for `check` on paused time, letting the driver run between polls.
    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..2_000 {
            if check() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("timed out waiting until {what}");
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn wake_burst_during_a_session_causes_one_follow_up_reload() {
        let fixture = Fixture::open("wake-burst");
        let runner = FakeRunner::new(SessionOutcome::Completed { truncated: false });
        runner.hold.store(true, Ordering::Release);
        let worker = start(&fixture, Arc::clone(&runner), SchedulerLimits::CURRENT);

        until("the first session is dispatched", || {
            !runner.dispatches().is_empty()
        })
        .await;
        let after_first = worker.loads_for_test();
        for _ in 0..1_000 {
            worker.wake();
        }
        runner.hold.store(false, Ordering::Release);
        runner.release.notify_waiters();
        until("the follow-up load has run", || {
            worker.loads_for_test() > after_first
        })
        .await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }

        // A thousand wakes cost at most two follow-up loads: one for the
        // waiter that was already parked, and one stored permit. Without
        // coalescing this would be a thousand.
        let follow_ups = worker.loads_for_test() - after_first;
        assert!(
            (1..=2).contains(&follow_ups),
            "a thousand wakes coalesced into {follow_ups} follow-up loads"
        );
        assert_eq!(
            runner.dispatches_for(fixture.peer),
            1,
            "the wakes carried no new work, so nothing is dialled again"
        );
        worker.shutdown().await;
        drop(worker);
        fixture.finish();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_during_backoff_returns_without_advancing_time() {
        let fixture = Fixture::open("shutdown-backoff");
        let runner = FakeRunner::new(SessionOutcome::Unreachable);
        let worker = start(&fixture, Arc::clone(&runner), SchedulerLimits::CURRENT);
        for expected in 1..=3 {
            until("the peer has failed again", || {
                runner.dispatches_for(fixture.peer) >= expected
            })
            .await;
            // Step past the backoff this failure created, so the next attempt
            // is due. Shutdown must not do the same.
            tokio::time::advance(Duration::from_secs(5)).await;
        }

        let before = Instant::now();
        worker.shutdown().await;
        assert!(
            Instant::now().duration_since(before) < Duration::from_secs(1),
            "shutdown does not wait out a backoff"
        );
        drop(worker);
        fixture.finish();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_waits_at_most_the_grace_for_sessions_that_ignore_stop() {
        let fixture = Fixture::open("shutdown-grace");
        let runner = FakeRunner::new(SessionOutcome::Completed { truncated: false });
        runner.wedge.store(true, Ordering::Release);
        let worker = start(&fixture, Arc::clone(&runner), SchedulerLimits::CURRENT);
        until("a session is running", || !runner.dispatches().is_empty()).await;

        let before = Instant::now();
        worker.shutdown().await;
        assert_eq!(
            Instant::now().duration_since(before),
            SchedulerLimits::CURRENT.shutdown_grace,
            "shutdown waits exactly the grace for a session that ignores it"
        );
        drop(worker);
        fixture.finish();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn revoked_peer_in_backoff_is_never_dialled() {
        let fixture = Fixture::open("revoked-backoff");
        let runner = FakeRunner::new(SessionOutcome::Unreachable);
        let worker = start(&fixture, Arc::clone(&runner), SchedulerLimits::CURRENT);
        until("the peer has failed once", || {
            runner.dispatches_for(fixture.peer) >= 1
        })
        .await;
        let dispatched = runner.dispatches_for(fixture.peer);

        // The peer leaves the Workspace while it is waiting out its backoff.
        fixture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace_membership_for_test(fixture.workspace, None);
        worker.wake();
        tokio::time::advance(Duration::from_secs(2)).await;
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            runner.dispatches_for(fixture.peer),
            dispatched,
            "a peer that is no longer a Member is never dialled again"
        );
        worker.shutdown().await;
        drop(worker);
        fixture.finish();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn busy_runtime_lock_delays_but_does_not_drop_a_wake() {
        let fixture = Fixture::open("busy-lock");
        let runner = FakeRunner::new(SessionOutcome::Completed { truncated: false });
        let worker = start(&fixture, Arc::clone(&runner), SchedulerLimits::CURRENT);
        until("the first pass has run", || worker.loads_for_test() >= 1).await;
        let before = worker.loads_for_test();

        let (release, released) = std::sync::mpsc::channel::<()>();
        let held = Arc::clone(&fixture.state);
        let holding = std::thread::spawn(move || {
            let _guard = held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = released.recv();
        });
        // Let the holding thread take the lock before the wake lands.
        while fixture.state.try_lock().is_ok() {
            std::thread::yield_now();
        }

        worker.wake();
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            worker.loads_for_test(),
            before,
            "a busy lock delays the load instead of skipping it"
        );

        let _ = release.send(());
        holding.join().expect("the holding thread finishes");
        until("the delayed wake is loaded", || {
            worker.loads_for_test() > before
        })
        .await;

        worker.shutdown().await;
        drop(worker);
        fixture.finish();
    }
}
