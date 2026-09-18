//! What to synchronize next, and with whom.
//!
//! This is a pure decision: no I/O, no clock of its own, no storage. The driver
//! supplies the jobs a durable load found, the time, and each session's
//! outcome; this module decides which job is dispatched next and what durable
//! progress and health that implies.
//!
//! Backoff belongs to a peer, not to the schedule: a peer that has gone away
//! waits longer between attempts, and every other peer keeps its own pace.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

// The scheduler's clock is the runtime's, so a test with paused time sees
// exactly the deadlines the driver waits on.
use tokio::time::Instant;

use super::limits::SchedulerLimits;
use crate::{PublicIdentity, WorkspaceId};

/// One unit of synchronization work with one peer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum JobKey {
    Membership {
        workspace: WorkspaceId,
        peer: PublicIdentity,
    },
    Records {
        scope: [u8; 32],
        workspace: WorkspaceId,
        package: String,
        peer: PublicIdentity,
    },
}

impl JobKey {
    pub(crate) const fn peer(&self) -> PublicIdentity {
        match self {
            Self::Membership { peer, .. } | Self::Records { peer, .. } => *peer,
        }
    }

    pub(crate) const fn workspace(&self) -> WorkspaceId {
        match self {
            Self::Membership { workspace, .. } | Self::Records { workspace, .. } => *workspace,
        }
    }

    const fn is_membership(&self) -> bool {
        matches!(self, Self::Membership { .. })
    }

    /// What this job is, for a log line that names no peer and no payload.
    pub(crate) const fn kind(&self) -> &'static str {
        if self.is_membership() {
            "membership"
        } else {
            "records"
        }
    }
}

/// A job the durable load found due, with the generation it was due for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DueJob {
    pub(crate) key: JobKey,
    pub(crate) generation: i64,
}

/// How one session ended, as the scheduler sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionOutcome {
    /// The session finished. `truncated` means one side had more to send.
    Completed {
        truncated: bool,
    },
    /// The peer could not be reached at all.
    Unreachable,
    /// The peer answered, and the session failed anyway.
    Failed,
    /// The peer does not consider this Installation a Member.
    RefusedNotAuthorized,
    /// The local gate refused before anything was dialled.
    LocalNotAuthorized,
    Cancelled,
}

/// What one peer's last session says about reaching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerOutcome {
    Reachable,
    Unreachable,
    Failed,
}

/// What the driver must do after a session completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchedulerAction {
    /// Record durable progress for this (job, peer) pair.
    Consume { key: JobKey, generation: i64 },
    /// Run a membership session with this peer next.
    ForceMembership {
        workspace: WorkspaceId,
        peer: PublicIdentity,
    },
    /// Report what this session says about reaching the peer.
    Health { key: JobKey, outcome: PeerOutcome },
}

#[derive(Debug, Clone)]
struct JobState {
    generation: i64,
    /// The generation the job was dispatched for, so a wake that lands while a
    /// session is running is not consumed by that session.
    dispatched_generation: i64,
    in_flight: bool,
    /// A job the scheduler added itself, which a durable load must not drop
    /// before it has been dispatched.
    forced: bool,
}

#[derive(Debug, Clone, Default)]
struct PeerState {
    failures: u32,
    not_before: Option<Instant>,
    in_flight: bool,
}

/// What a test sees of the scheduler's state.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerSnapshot {
    pub(crate) peer: PublicIdentity,
    pub(crate) failures: u32,
    pub(crate) jobs: usize,
    pub(crate) in_flight: bool,
}

/// The scheduling decision, as a pure state machine.
#[derive(Debug)]
pub(crate) struct SchedulerCore {
    jobs: BTreeMap<JobKey, JobState>,
    peers: BTreeMap<PublicIdentity, PeerState>,
    order: VecDeque<PublicIdentity>,
    last_attempt: BTreeMap<JobKey, Instant>,
    in_flight: usize,
    limits: SchedulerLimits,
    jitter: fn(Duration) -> Duration,
}

impl SchedulerCore {
    pub(crate) fn new(limits: SchedulerLimits, jitter: fn(Duration) -> Duration) -> Self {
        Self {
            jobs: BTreeMap::new(),
            peers: BTreeMap::new(),
            order: VecDeque::new(),
            last_attempt: BTreeMap::new(),
            in_flight: 0,
            limits,
            jitter,
        }
    }

    /// Replaces the durable job set, keeping what this scheduler still owns.
    ///
    /// In-flight jobs are kept, because their session is still running, and so
    /// are jobs the scheduler forced on itself and has not dispatched yet.
    pub(crate) fn replace_due(&mut self, due: Vec<DueJob>, _now: Instant) {
        let mut kept = self
            .jobs
            .iter()
            .filter(|(_, state)| state.in_flight || state.forced)
            .map(|(key, state)| (key.clone(), state.clone()))
            .collect::<BTreeMap<_, _>>();
        for job in due {
            if kept.len() >= self.limits.max_scheduled_jobs && !kept.contains_key(&job.key) {
                break;
            }
            match kept.get_mut(&job.key) {
                Some(state) => state.generation = state.generation.max(job.generation),
                None => {
                    kept.insert(
                        job.key.clone(),
                        JobState {
                            generation: job.generation,
                            dispatched_generation: 0,
                            in_flight: false,
                            forced: false,
                        },
                    );
                }
            }
        }
        self.jobs = kept;
        self.reconcile_peers();
    }

    /// Adds a membership job that survives the next durable load.
    pub(crate) fn force_membership(&mut self, workspace: WorkspaceId, peer: PublicIdentity) {
        let key = JobKey::Membership { workspace, peer };
        let entry = self.jobs.entry(key).or_insert(JobState {
            generation: 0,
            dispatched_generation: 0,
            in_flight: false,
            forced: true,
        });
        entry.forced = true;
        self.reconcile_peers();
    }

    /// Drops peers with no jobs left and admits peers that gained one.
    fn reconcile_peers(&mut self) {
        let with_jobs = self.jobs.keys().map(JobKey::peer).collect::<BTreeSet<_>>();
        for peer in &with_jobs {
            if !self.peers.contains_key(peer) {
                self.peers.insert(*peer, PeerState::default());
                self.order.push_back(*peer);
            }
        }
        // A peer that is still running a session keeps its state until it ends.
        self.peers
            .retain(|peer, state| with_jobs.contains(peer) || state.in_flight);
        let live = self.peers.keys().copied().collect::<BTreeSet<_>>();
        self.order.retain(|peer| live.contains(peer));
    }

    fn is_failing(&self, peer: PublicIdentity) -> bool {
        self.peers
            .get(&peer)
            .is_some_and(|state| state.failures >= self.limits.failing_threshold)
    }

    /// The next job to dispatch, if any slot and any peer is eligible.
    ///
    /// At most one session per peer, at most `outbound_sessions` in all, and at
    /// most `failing_peer_slots` of those for peers that keep failing. Peers are
    /// served round-robin; for one peer, membership goes before records.
    pub(crate) fn next_dispatch(&mut self, now: Instant) -> Option<DueJob> {
        if self.in_flight >= self.limits.outbound_sessions {
            return None;
        }
        let failing_in_flight = self
            .peers
            .iter()
            .filter(|(peer, state)| state.in_flight && self.is_failing(**peer))
            .count();
        for index in 0..self.order.len() {
            let peer = self.order[index];
            let Some(state) = self.peers.get(&peer) else {
                continue;
            };
            if state.in_flight || state.not_before.is_some_and(|due| due > now) {
                continue;
            }
            if self.is_failing(peer) && failing_in_flight >= self.limits.failing_peer_slots {
                continue;
            }
            let Some(key) = self.next_job_for(peer) else {
                continue;
            };
            let generation = {
                let job = self.jobs.get_mut(&key).expect("the job was just chosen");
                job.in_flight = true;
                job.dispatched_generation = job.generation;
                job.forced = false;
                job.generation
            };
            self.peers
                .get_mut(&peer)
                .expect("the peer was just chosen")
                .in_flight = true;
            self.in_flight += 1;
            self.order.remove(index);
            self.order.push_back(peer);
            self.last_attempt.insert(key.clone(), now);
            return Some(DueJob { key, generation });
        }
        None
    }

    fn next_job_for(&self, peer: PublicIdentity) -> Option<JobKey> {
        let mut records = None;
        for key in self.jobs.keys() {
            if key.peer() != peer || self.jobs.get(key).is_some_and(|state| state.in_flight) {
                continue;
            }
            if key.is_membership() {
                return Some(key.clone());
            }
            if records.is_none() {
                records = Some(key.clone());
            }
        }
        records
    }

    /// Records one session's outcome and reports what the driver must do.
    pub(crate) fn complete(
        &mut self,
        key: &JobKey,
        outcome: SessionOutcome,
        now: Instant,
    ) -> Vec<SchedulerAction> {
        let peer = key.peer();
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(state) = self.peers.get_mut(&peer) {
            state.in_flight = false;
        }
        let dispatched = self
            .jobs
            .get(key)
            .map_or(0, |job| job.dispatched_generation);
        if let Some(job) = self.jobs.get_mut(key) {
            job.in_flight = false;
        }
        let mut actions = Vec::new();
        match outcome {
            SessionOutcome::Completed { truncated } => {
                self.succeed(peer);
                actions.push(SchedulerAction::Health {
                    key: key.clone(),
                    outcome: PeerOutcome::Reachable,
                });
                if truncated {
                    // There is more to exchange, so the job stays due and the
                    // generation is not consumed.
                } else {
                    actions.push(SchedulerAction::Consume {
                        key: key.clone(),
                        generation: dispatched,
                    });
                    self.remove_if_unchanged(key, dispatched);
                }
            }
            SessionOutcome::Unreachable => {
                self.fail(peer, now);
                actions.push(SchedulerAction::Health {
                    key: key.clone(),
                    outcome: PeerOutcome::Unreachable,
                });
            }
            SessionOutcome::Failed => {
                self.fail(peer, now);
                actions.push(SchedulerAction::Health {
                    key: key.clone(),
                    outcome: PeerOutcome::Failed,
                });
            }
            SessionOutcome::RefusedNotAuthorized => {
                self.fail(peer, now);
                actions.push(SchedulerAction::Health {
                    key: key.clone(),
                    outcome: PeerOutcome::Failed,
                });
                // A peer that does not know this Installation as a Member is
                // exactly the case a membership session resolves.
                actions.push(SchedulerAction::ForceMembership {
                    workspace: key.workspace(),
                    peer,
                });
            }
            SessionOutcome::LocalNotAuthorized | SessionOutcome::Cancelled => {
                self.jobs.remove(key);
            }
        }
        self.reconcile_peers();
        actions
    }

    /// Removes a job whose generation has not moved since it was dispatched.
    ///
    /// A wake that landed while the session ran leaves the job due again.
    fn remove_if_unchanged(&mut self, key: &JobKey, dispatched: i64) {
        if self
            .jobs
            .get(key)
            .is_some_and(|job| job.generation <= dispatched && !job.forced)
        {
            self.jobs.remove(key);
        }
    }

    fn succeed(&mut self, peer: PublicIdentity) {
        if let Some(state) = self.peers.get_mut(&peer) {
            state.failures = 0;
            state.not_before = None;
        }
    }

    fn fail(&mut self, peer: PublicIdentity, now: Instant) {
        let limits = self.limits;
        let jitter = self.jitter;
        if let Some(state) = self.peers.get_mut(&peer) {
            state.failures = state.failures.saturating_add(1);
            let shift = state.failures.saturating_sub(1).min(20);
            let delay = limits
                .retry_base
                .saturating_mul(1_u32.checked_shl(shift).unwrap_or(u32::MAX))
                .min(limits.retry_cap);
            state.not_before = Some(now + jitter(delay).min(limits.retry_cap));
        }
    }

    /// When the next peer leaves backoff, if any is waiting.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.peers
            .iter()
            .filter(|(peer, state)| {
                !state.in_flight && self.jobs.keys().any(|key| key.peer() == **peer)
            })
            .filter_map(|(_, state)| state.not_before)
            .min()
    }

    pub(crate) const fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// When a (job, peer) pair was last attempted, so an idle reload can tell
    /// which pairs have gone quiet.
    pub(crate) fn last_attempts(&self) -> BTreeMap<JobKey, Instant> {
        self.last_attempt.clone()
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<PeerSnapshot> {
        self.peers
            .iter()
            .map(|(peer, state)| PeerSnapshot {
                peer: *peer,
                failures: state.failures,
                jobs: self.jobs.keys().filter(|key| key.peer() == *peer).count(),
                in_flight: state.in_flight,
            })
            .collect()
    }
}

/// Spreads retries of peers that failed together, by up to ±20 %.
pub(crate) fn production_jitter(delay: Duration) -> Duration {
    let mut sample = [0_u8; 2];
    let _ = getrandom::fill(&mut sample);
    let ratio = u64::from(SchedulerLimits::CURRENT.jitter_ratio_percent);
    let span = delay.as_millis().min(u128::from(u64::MAX)) as u64 * ratio / 50;
    let offset = if span == 0 {
        0
    } else {
        u64::from(u16::from_be_bytes(sample)) % (span + 1)
    };
    let base = delay.as_millis().min(u128::from(u64::MAX)) as u64;
    let lower = base.saturating_sub(span / 2);
    Duration::from_millis(lower.saturating_add(offset))
}

/// The same delay every time, so a test asserts a deadline rather than a range.
pub(crate) const fn no_jitter(delay: Duration) -> Duration {
    delay
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn identity(seed: u8) -> PublicIdentity {
        PublicIdentity::from_bytes(*SecretKey::from_bytes(&[seed; 32]).public().as_bytes()).unwrap()
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_bytes([7; 16])
    }

    fn records(peer: u8, package: &str) -> JobKey {
        JobKey::Records {
            scope: [peer; 32],
            workspace: workspace(),
            package: package.to_owned(),
            peer: identity(peer),
        }
    }

    fn membership(peer: u8) -> JobKey {
        JobKey::Membership {
            workspace: workspace(),
            peer: identity(peer),
        }
    }

    fn due(key: JobKey, generation: i64) -> DueJob {
        DueJob { key, generation }
    }

    fn core() -> SchedulerCore {
        SchedulerCore::new(SchedulerLimits::CURRENT, no_jitter)
    }

    #[test]
    fn backoff_applies_only_after_a_failure() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(vec![due(records(1, "teamarness.test"), 1)], base);
        let job = core.next_dispatch(base).expect("a new job dispatches now");
        assert_eq!(core.next_deadline(), None, "nothing is waiting yet");

        core.complete(&job.key, SessionOutcome::Unreachable, base);
        assert_eq!(core.next_dispatch(base), None);
        assert_eq!(
            core.next_deadline(),
            Some(base + Duration::from_millis(500))
        );

        let second = base + Duration::from_millis(500);
        let job = core.next_dispatch(second).expect("backoff elapsed");
        core.complete(&job.key, SessionOutcome::Unreachable, second);
        assert_eq!(core.next_deadline(), Some(second + Duration::from_secs(1)));

        let mut later = second;
        for _ in 0..8 {
            later += Duration::from_secs(120);
            let job = core.next_dispatch(later).expect("backoff elapsed");
            core.complete(&job.key, SessionOutcome::Unreachable, later);
        }
        assert_eq!(core.next_deadline(), Some(later + Duration::from_secs(60)));
    }

    #[test]
    fn success_resets_backoff() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(vec![due(records(1, "teamarness.test"), 1)], base);
        let job = core.next_dispatch(base).unwrap();
        core.complete(&job.key, SessionOutcome::Unreachable, base);
        assert!(core.next_deadline().is_some());

        core.replace_due(vec![due(records(1, "teamarness.test"), 2)], base);
        let later = base + Duration::from_secs(1);
        let job = core.next_dispatch(later).unwrap();
        core.complete(
            &job.key,
            SessionOutcome::Completed { truncated: false },
            later,
        );
        assert_eq!(core.next_deadline(), None);
    }

    #[test]
    fn backoff_of_one_peer_does_not_delay_another() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(
            vec![
                due(records(1, "teamarness.test"), 1),
                due(records(2, "teamarness.test"), 1),
            ],
            base,
        );
        let first = core.next_dispatch(base).unwrap();
        core.complete(&first.key, SessionOutcome::Unreachable, base);
        let second = core
            .next_dispatch(base)
            .expect("the other peer is eligible");
        assert_ne!(second.key.peer(), first.key.peer());
    }

    #[test]
    fn at_most_four_in_flight_and_one_per_peer() {
        let base = Instant::now();
        let mut core = core();
        let jobs = (1..=6)
            .flat_map(|peer| {
                [
                    due(records(peer, "teamarness.one"), 1),
                    due(records(peer, "teamarness.two"), 1),
                ]
            })
            .collect::<Vec<_>>();
        core.replace_due(jobs, base);
        let mut dispatched = Vec::new();
        while let Some(job) = core.next_dispatch(base) {
            dispatched.push(job);
        }
        assert_eq!(dispatched.len(), 4);
        assert_eq!(core.in_flight(), 4);
        let peers = dispatched
            .iter()
            .map(|job| job.key.peer())
            .collect::<BTreeSet<_>>();
        assert_eq!(peers.len(), 4, "one session per peer");
    }

    #[test]
    fn failing_peers_use_at_most_two_slots() {
        let base = Instant::now();
        let mut core = core();
        let jobs = (1..=6)
            .map(|peer| due(records(peer, "teamarness.test"), 1))
            .collect::<Vec<_>>();
        let failing = (1..=4).map(identity).collect::<BTreeSet<_>>();
        // Peers 1..=4 keep failing; 5 and 6 keep succeeding. Each round starts
        // far enough ahead that every backoff has elapsed.
        for round in 0..6 {
            let now = base + Duration::from_secs(300 * round);
            core.replace_due(jobs.clone(), now);
            let mut dispatched = Vec::new();
            while let Some(job) = core.next_dispatch(now) {
                dispatched.push(job);
            }
            for job in dispatched {
                let outcome = if failing.contains(&job.key.peer()) {
                    SessionOutcome::Unreachable
                } else {
                    SessionOutcome::Completed { truncated: false }
                };
                core.complete(&job.key, outcome, now);
            }
        }

        let elapsed = base + Duration::from_secs(3_600);
        core.replace_due(jobs, elapsed);
        let mut dispatched = Vec::new();
        while let Some(job) = core.next_dispatch(elapsed) {
            dispatched.push(job.key.peer());
        }
        assert_eq!(dispatched.len(), 4, "every slot is used");
        assert_eq!(
            dispatched
                .iter()
                .filter(|peer| failing.contains(peer))
                .count(),
            2,
            "failing peers cannot occupy every slot"
        );
    }

    #[test]
    fn membership_job_for_a_peer_dispatches_before_its_record_jobs() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(
            vec![due(records(1, "teamarness.test"), 1), due(membership(1), 1)],
            base,
        );
        let first = core.next_dispatch(base).unwrap();
        assert_eq!(first.key, membership(1));
    }

    #[test]
    fn round_robin_serves_every_peer_before_repeating() {
        let base = Instant::now();
        let mut core = core();
        let jobs = (1..=25)
            .map(|peer| due(records(peer, "teamarness.test"), 1))
            .collect::<Vec<_>>();
        core.replace_due(jobs, base);
        let mut served = Vec::new();
        while let Some(job) = core.next_dispatch(base) {
            served.push(job.key.peer());
            core.complete(
                &job.key,
                SessionOutcome::Completed { truncated: false },
                base,
            );
        }
        assert_eq!(served.len(), 25);
        assert_eq!(
            served.iter().copied().collect::<BTreeSet<_>>().len(),
            25,
            "every peer is served exactly once before any repeats"
        );
    }

    #[test]
    fn truncated_completion_requeues_behind_other_eligible_peers_without_consuming() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(
            vec![
                due(records(1, "teamarness.test"), 5),
                due(records(2, "teamarness.test"), 5),
            ],
            base,
        );
        let first = core.next_dispatch(base).unwrap();
        let actions = core.complete(
            &first.key,
            SessionOutcome::Completed { truncated: true },
            base,
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, SchedulerAction::Consume { .. })),
            "a truncated session has more to send, so nothing is consumed"
        );
        let next = core.next_dispatch(base).unwrap();
        assert_ne!(
            next.key.peer(),
            first.key.peer(),
            "the other peer is served before the truncated one repeats"
        );
    }

    #[test]
    fn completion_consumes_the_generation_seen_at_dispatch() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(vec![due(records(1, "teamarness.test"), 5)], base);
        let job = core.next_dispatch(base).unwrap();
        // A wake lands while the session is running.
        core.replace_due(vec![due(records(1, "teamarness.test"), 9)], base);
        let actions = core.complete(
            &job.key,
            SessionOutcome::Completed { truncated: false },
            base,
        );
        assert_eq!(
            actions.iter().find_map(|action| match action {
                SchedulerAction::Consume { generation, .. } => Some(*generation),
                _ => None,
            }),
            Some(5),
            "only the generation this session saw is consumed"
        );
        assert!(
            core.next_dispatch(base).is_some(),
            "the later wake leaves the job due again"
        );
    }

    #[test]
    fn refused_not_authorized_forces_a_membership_job() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(vec![due(records(1, "teamarness.test"), 1)], base);
        let job = core.next_dispatch(base).unwrap();
        let actions = core.complete(&job.key, SessionOutcome::RefusedNotAuthorized, base);
        let forced = actions.iter().find_map(|action| match action {
            SchedulerAction::ForceMembership { workspace, peer } => Some((*workspace, *peer)),
            _ => None,
        });
        assert_eq!(forced, Some((workspace(), identity(1))));

        core.force_membership(workspace(), identity(1));
        // A durable load that knows nothing of the forced job must not drop it.
        core.replace_due(Vec::new(), base);
        let elapsed = base + Duration::from_secs(120);
        assert_eq!(
            core.next_dispatch(elapsed).map(|job| job.key),
            Some(membership(1))
        );
    }

    #[test]
    fn local_not_authorized_and_cancelled_drop_the_job_without_backoff() {
        let base = Instant::now();
        let mut core = core();
        core.replace_due(
            vec![
                due(records(1, "teamarness.test"), 1),
                due(records(2, "teamarness.test"), 1),
            ],
            base,
        );
        let first = core.next_dispatch(base).unwrap();
        assert!(
            core.complete(&first.key, SessionOutcome::LocalNotAuthorized, base)
                .is_empty()
        );
        let second = core.next_dispatch(base).unwrap();
        assert!(
            core.complete(&second.key, SessionOutcome::Cancelled, base)
                .is_empty()
        );
        assert_eq!(core.next_deadline(), None, "neither peer is in backoff");
        assert_eq!(core.next_dispatch(base), None, "both jobs are gone");
    }

    #[test]
    fn replace_due_caps_scheduled_jobs() {
        let base = Instant::now();
        let mut core = SchedulerCore::new(
            SchedulerLimits {
                max_scheduled_jobs: 3,
                ..SchedulerLimits::CURRENT
            },
            no_jitter,
        );
        let jobs = (1..=10)
            .map(|peer| due(records(peer, "teamarness.test"), 1))
            .collect::<Vec<_>>();
        core.replace_due(jobs, base);
        let mut dispatched = 0;
        while core.next_dispatch(base).is_some() {
            dispatched += 1;
        }
        assert_eq!(dispatched, 3);
    }

    #[test]
    fn production_jitter_stays_within_the_ratio_and_the_cap() {
        for _ in 0..64 {
            let delay = production_jitter(Duration::from_millis(1_000));
            assert!(delay >= Duration::from_millis(800), "{delay:?}");
            assert!(delay <= Duration::from_millis(1_200), "{delay:?}");
        }
        assert_eq!(production_jitter(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn the_outbound_session_bound_matches_the_network_budget() {
        assert_eq!(
            SchedulerLimits::CURRENT.outbound_sessions,
            crate::network::MAX_OUTBOUND_PEERS
        );
    }
}
