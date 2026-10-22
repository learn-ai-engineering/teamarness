use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use rusqlite::OptionalExtension;

use crate::MembershipOperationId;
use crate::diag::fault;
use crate::error::{
    CoreError, IdentityError, InvitationError, MembershipError, StorageError, SubscriptionError,
    WorkspaceError,
};
use crate::invitation::{InvitationClaim, InvitationToken};

/// The sole public command and query seam for durable core state.
///
/// The runtime owns its SQLite connection and installation secret. Neither
/// those values nor private test dependencies can be obtained by callers.
pub struct CoreRuntime {
    public_identity: PublicIdentity,
    state: Arc<Mutex<RuntimeState>>,
    clock: Arc<dyn Clock>,
    transport: Arc<crate::onboarding::transport::Transport>,
    sync_worker: Arc<crate::synchronization::SyncWorker>,
    /// The worker owns the demand it acts on; this handle exists so a test that
    /// drives a session by hand builds a receiver with the same demand.
    #[cfg_attr(not(test), allow(dead_code))]
    membership_demand: Arc<crate::membership_sync::MembershipDemand>,
    /// What recent sessions said about reaching each peer, per scope.
    health: Arc<crate::synchronization::SyncHealthRegistry>,
    #[cfg(test)]
    session_reports: crate::synchronization::worker::SessionReports,
    #[cfg(test)]
    membership_replica: Arc<dyn crate::membership_sync::MembershipReplica>,
    #[cfg(test)]
    membership_paused: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    recovery_report: crate::synchronization::RecoveryReport,
    #[cfg(test)]
    session_limits: crate::synchronization::limits::SessionLimits,
    /// The shared Endpoint, so a test can drive one session by hand.
    #[cfg(test)]
    network: Arc<crate::network::Network>,
    /// The package-sync services and inbound handler, for the same reason.
    #[cfg(test)]
    sync_services: crate::synchronization::SyncServices,
    #[cfg(test)]
    log_sync: Arc<crate::synchronization::LogSyncHandler>,
}

impl fmt::Debug for CoreRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreRuntime")
            .field("public_identity", &self.public_identity)
            .finish_non_exhaustive()
    }
}

pub(crate) struct RuntimeState {
    pub(crate) connection: rusqlite::Connection,
    /// The one clock every commit time is read from, injectable in tests.
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) peer_sessions: Arc<crate::synchronization::PeerSessions>,
    /// Asks the synchronization scheduler to reload what is due. Every commit
    /// path can reach it, so nothing waits for an idle tick to be noticed.
    pub(crate) sync_wake: Arc<tokio::sync::Notify>,
    secret_key: iroh_base::SecretKey,
    display_name: String,
    pub(crate) memberships: BTreeMap<WorkspaceId, crate::membership::WorkspaceProjection>,
    subscribers: BTreeMap<WorkspaceId, Vec<MembershipSubscriber>>,
    /// The per-scope head notifiers record subscriptions wait on. Shared rather
    /// than owned, so quarantine and session health can wake a subscriber
    /// without taking this lock.
    pub(crate) record_heads: crate::synchronization::record_subscription::SharedRecordHeads,
    /// One watch per Workspace carrying the local membership epoch, so a
    /// membership stream waits instead of polling.
    membership_epochs: BTreeMap<WorkspaceId, tokio::sync::watch::Sender<u64>>,
    profile_generations: BTreeMap<WorkspaceId, tokio::sync::watch::Sender<u64>>,
    #[cfg(test)]
    person_name_open_error_once: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    principal_sync_init_faults: u8,
    next_subscriber_id: u64,
    pub(crate) poisoned: bool,
}

pub(crate) trait Clock: Send + Sync {
    fn now_unix_ms(&self) -> i64;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
    }
}

impl CoreRuntime {
    /// Opens storage and starts the authenticated onboarding transport.
    ///
    /// Opening does not wait for public relay readiness, so local commands remain
    /// available while offline. Call [`Self::shutdown`] and await it before exit.
    pub async fn open(config: CoreRuntimeConfig) -> Result<Self, CoreError> {
        Self::open_inner(config, Box::new(SystemClock), None).await
    }

    #[cfg(test)]
    async fn open_with_clock(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
    ) -> Result<Self, CoreError> {
        Self::open_inner(config, clock, Some(true)).await
    }

    #[cfg(test)]
    async fn open_with_clock_and_readiness(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
        ready_immediately: bool,
    ) -> Result<Self, CoreError> {
        Self::open_inner(config, clock, Some(ready_immediately)).await
    }

    async fn open_inner(
        config: CoreRuntimeConfig,
        clock: Box<dyn Clock>,
        #[cfg_attr(
            not(any(test, feature = "test-hooks")),
            allow(unused_variables)
        )]
        test_readiness: Option<bool>,
    ) -> Result<Self, CoreError> {
        let clock: Arc<dyn Clock> = Arc::from(clock);
        let opened_at_unix_ms = clock.now_unix_ms();
        // Opening storage and replaying the membership projection are both
        // blocking SQLite and signature work, so they run together on a
        // blocking thread and never on an asynchronous worker.
        let directory = config.private_data_directory().to_path_buf();
        let opened: Result<_, CoreError> = tokio::task::spawn_blocking(move || {
            let crate::storage::StoredRuntime {
                mut connection,
                secret_key,
                display_name,
                recovery,
            } = crate::storage::open(&directory, opened_at_unix_ms)?;
            let memberships =
                crate::membership::projection::recover(&mut connection, opened_at_unix_ms)?;
            // Parking is bounded in time as well as in size: a peer that went
            // away never leaves work here for longer than the time-to-live.
            let _ = crate::synchronization::parking::purge_expired(
                &connection,
                opened_at_unix_ms,
                crate::synchronization::parking::ParkingLimits::CURRENT,
            );
            Ok((connection, secret_key, display_name, memberships, recovery))
        })
        .await
        .map_err(fault!(CoreError::Storage(StorageError::Database)))?;
        let (connection, secret_key, display_name, memberships, recovery) = opened?;
        #[cfg(not(test))]
        let _ = recovery;
        let public_identity = PublicIdentity(secret_key.public());
        let peer_sessions = crate::synchronization::PeerSessions::new();
        let sync_wake = Arc::new(tokio::sync::Notify::new());
        // The head map is shared so session health and quarantine can wake a
        // subscriber without taking the runtime state lock.
        let record_heads: crate::synchronization::record_subscription::SharedRecordHeads =
            Arc::default();
        let health = crate::synchronization::SyncHealthRegistry::new(Arc::clone(&record_heads));
        let state = Arc::new(Mutex::new(RuntimeState {
            connection,
            clock: Arc::clone(&clock),
            peer_sessions: Arc::clone(&peer_sessions),
            sync_wake: Arc::clone(&sync_wake),
            secret_key: secret_key.clone(),
            display_name,
            memberships,
            subscribers: BTreeMap::new(),
            record_heads,
            membership_epochs: BTreeMap::new(),
            profile_generations: BTreeMap::new(),
            #[cfg(test)]
            person_name_open_error_once: false,
            #[cfg(any(test, feature = "test-hooks"))]
            principal_sync_init_faults: 0,
            next_subscriber_id: 0,
            poisoned: false,
        }));
        {
            let state = Arc::clone(&state);
            tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
                let mut locked = state
                    .lock()
                    .map_err(fault!(CoreError::Storage(StorageError::Database)))?;
                let workspaces: Vec<_> = locked.memberships.keys().copied().collect();
                for workspace in workspaces {
                    principal_sync::initialize(&mut locked, workspace)
                        .map_err(fault!(CoreError::Storage(StorageError::Database)))?;
                }
                Ok(())
            })
            .await
            .map_err(|_| CoreError::Storage(StorageError::Database))??;
        }
        let issuer = crate::onboarding::handler::IssuerService::new(
            Arc::downgrade(&state),
            Arc::clone(&clock),
        );
        let demand = crate::membership_sync::MembershipDemand::new();
        #[cfg(test)]
        let session_limits = config.session_limits;
        #[cfg(not(test))]
        let session_limits = crate::synchronization::limits::SessionLimits::CURRENT;
        let services = crate::synchronization::SyncServices {
            receiver: Arc::new(RuntimeSyncReceiver {
                state: Arc::downgrade(&state),
                clock: Arc::clone(&clock),
                demand: Arc::clone(&demand),
                health: Arc::clone(&health),
            }),
            sessions: Arc::clone(&peer_sessions),
            health: Arc::clone(&health),
            limits: session_limits,
        };
        // The composition point owns the membership protocol: `network` keeps
        // the Endpoint and Router and names no membership type.
        #[cfg(test)]
        let membership_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let membership_replica: Arc<dyn crate::membership_sync::MembershipReplica> =
            Arc::new(replication::RuntimeMembershipReplica::new(
                Arc::downgrade(&state),
                Arc::clone(&clock),
                #[cfg(test)]
                Arc::clone(&membership_paused),
            ));
        let (package_sync, log_sync) =
            crate::synchronization::client::package_sync_registration(services.clone());
        // Only a test reads the inbound handler back; production just routes it.
        #[cfg(not(test))]
        let _ = log_sync;
        let protocols = vec![
            crate::membership_sync::membership_sync_registration(Arc::clone(&membership_replica)),
            package_sync,
        ];
        #[cfg(any(test, feature = "test-hooks"))]
        let transport = if let Some(ready_immediately) = test_readiness {
            crate::onboarding::transport::Transport::bind_loopback_with_issuer_and_protocols(
                secret_key.clone(),
                std::time::Duration::from_millis(5),
                ready_immediately,
                issuer,
                protocols,
            )
            .await
        } else {
            crate::onboarding::transport::Transport::bind_production(
                secret_key.clone(),
                issuer,
                protocols,
            )
            .await
        };
        #[cfg(not(any(test, feature = "test-hooks")))]
        let transport = crate::onboarding::transport::Transport::bind_production(
            secret_key.clone(),
            issuer,
            protocols,
        )
        .await;
        let transport = transport.map_err(CoreError::Onboarding)?;
        debug_assert_eq!(transport.endpoint_id(), secret_key.public());
        // Recovery has completed before the worker is started. The worker owns
        // wake consumption and scheduling, while all network awaits happen
        // outside the runtime state mutex.
        #[cfg(test)]
        let session_reports: crate::synchronization::worker::SessionReports = Arc::default();
        let network = Arc::clone(transport.network());
        #[cfg(test)]
        let sync_services = services.clone();
        let runner = crate::synchronization::worker::NetworkSessionRunner::new(
            Arc::clone(&network),
            services,
            Arc::clone(&membership_replica),
            Arc::clone(&demand),
            Arc::downgrade(&state),
            public_identity,
            #[cfg(test)]
            Arc::clone(&session_reports),
        );
        let sync_worker = crate::synchronization::SyncWorker::start(
            sync_wake,
            Arc::downgrade(&state),
            public_identity,
            Arc::new(runner),
            Arc::clone(&demand),
            crate::synchronization::limits::SchedulerLimits::CURRENT,
            crate::synchronization::scheduler::production_jitter,
        );
        Ok(Self {
            public_identity,
            state,
            clock,
            transport,
            sync_worker,
            membership_demand: demand,
            health,
            #[cfg(test)]
            session_reports,
            #[cfg(test)]
            membership_replica,
            #[cfg(test)]
            membership_paused,
            #[cfg(test)]
            recovery_report: recovery,
            #[cfg(test)]
            session_limits,
            #[cfg(test)]
            network,
            #[cfg(test)]
            sync_services,
            #[cfg(test)]
            log_sync,
        })
    }

    /// Stops accepting onboarding and synchronization work and awaits closure.
    ///
    /// Local identity, Workspace, and membership commands remain available.
    pub async fn shutdown(&self) -> Result<(), crate::ShutdownError> {
        // Stop the scheduler and await every session before Router/Endpoint
        // shutdown. This ordering prevents SQLite callbacks from racing the
        // shared network teardown.
        self.sync_worker.shutdown().await;
        self.transport
            .shutdown()
            .await
            .map_err(|_| crate::ShutdownError::Transport)
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.transport.start_abnormal_shutdown();
    }
}

mod identity;
mod membership;
mod onboarding;
pub(crate) mod principal_sync;
mod records;
mod replication;
mod subscription;
mod sync_receiver;
#[cfg(any(test, feature = "test-hooks"))]
mod test_hooks;
mod types;
mod workspaces;

pub use subscription::MembershipSubscription;
#[cfg(test)]
pub(crate) use test_hooks::ProbeOutcome;
#[cfg(any(test, feature = "test-hooks"))]
pub use test_hooks::RuntimeStateHold;
#[cfg(feature = "test-hooks")]
pub use test_hooks::{AgentAddFaultForTests, MembershipApplyOutcomeForTests};
pub use types::*;

// `test_hooks` reaches this through `use super::*`; nothing else in `runtime` needs it.
#[cfg(any(test, feature = "test-hooks"))]
use onboarding::map_invitation_onboarding_error;
pub(crate) use onboarding::respond_to_join;
#[cfg(feature = "test-hooks")]
use subscription::MEMBERSHIP_SUBSCRIPTIONS_OPENED;
use subscription::MembershipSubscriber;
#[cfg(feature = "test-hooks")]
pub(crate) use subscription::membership_subscriptions_opened;
pub(crate) use sync_receiver::RuntimeSyncReceiver;

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;
