use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use rusqlite::OptionalExtension;

use crate::MembershipOperationId;
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
}

impl fmt::Debug for CoreRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreRuntime")
            .field("public_identity", &self.public_identity)
            .finish_non_exhaustive()
    }
}

/// The Core side of every package synchronization session.
///
/// Every method re-checks current membership, so a revocation that lands while
/// a session is running stops that session serving or accepting anything more.
pub(crate) struct RuntimeSyncReceiver {
    pub(crate) state: Weak<Mutex<RuntimeState>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) demand: Arc<crate::membership_sync::MembershipDemand>,
    /// Isolating a scope changes what every subscriber of it reports, so the
    /// registry is asked to wake them.
    pub(crate) health: Arc<crate::synchronization::SyncHealthRegistry>,
}

impl crate::synchronization::SyncReceiver for RuntimeSyncReceiver {
    fn authorize_session(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        let state = self
            .state
            .upgrade()
            .ok_or(crate::synchronization::Refusal::Unavailable)?;
        let state = state
            .lock()
            .map_err(|_| crate::synchronization::Refusal::Unavailable)?;
        if state.poisoned {
            return Err(crate::synchronization::Refusal::Unavailable);
        }
        let local = PublicIdentity(state.secret_key.public());
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(crate::synchronization::Refusal::NotAuthorized)?;
        if !projection.contains_member(local) || !projection.contains_member(peer) {
            return Err(crate::synchronization::Refusal::NotAuthorized);
        }
        let membership_version = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package, workspace);
        // A scope isolated by a local integrity failure serves nothing and
        // accepts nothing, but the Installation stays usable.
        if crate::synchronization::quarantine::is_scope_quarantined(&state.connection, scope)
            .map_err(|_| crate::synchronization::Refusal::Unavailable)?
        {
            return Err(crate::synchronization::Refusal::Unavailable);
        }
        let content_key_id = state
            .connection
            .query_row(
                "SELECT content_key_id FROM sync_scopes WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|_| crate::synchronization::Refusal::Unavailable)?
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        Ok(crate::synchronization::SessionGrant {
            membership_version,
            content_key_id,
        })
    }

    fn protection_bundle_for(
        &self,
        workspace: WorkspaceId,
        package: &str,
        recipient: PublicIdentity,
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let mut state = state.lock().map_err(|_| ())?;
        let projection = state.memberships.get(&workspace).ok_or(())?;
        let local = PublicIdentity(state.secret_key.public());
        if !projection.contains_member(local) || !projection.contains_member(recipient) {
            return Err(());
        }
        let admission = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package, workspace);
        let origin: Option<Vec<u8>> = state
            .connection
            .query_row(
                "SELECT control_bytes FROM protection_control_operations
                 WHERE scope_id=?1 AND operation_kind='create' LIMIT 1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ())?;
        // Without the creator's origin this Installation cannot prove the key's
        // authority, so it offers nothing rather than an unusable grant.
        let Some(origin) = origin else {
            return Ok(None);
        };
        let secret_key = state.secret_key.clone();
        match crate::synchronization::export_protection_grant(
            &mut state.connection,
            &secret_key,
            workspace,
            package,
            recipient,
            admission,
        ) {
            Ok(grant) => Ok(Some(crate::synchronization::ProtectionBundle {
                origin,
                grant,
            })),
            Err(crate::SynchronizationError::WorkspaceNotFound) => Ok(None),
            Err(_) => Err(()),
        }
    }

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let mut state = state.lock().map_err(|_| ())?;
        if crate::synchronization::protection_control_author(&bundle.grant).map_err(|_| ())? != peer
        {
            return Err(());
        }
        let projection = state.memberships.get(&workspace).ok_or(())?;
        if !projection.contains_member(peer) {
            return Err(());
        }
        let creator = projection.creator();
        let secret = state.secret_key.clone();
        crate::synchronization::import_protection_grant(
            &mut state.connection,
            &secret,
            workspace,
            package,
            creator,
            &bundle.origin,
            &bundle.grant,
        )
        .map_err(|_| ())?;
        // The scope now exists, so a subscription that was waiting for history
        // access can proceed even when nothing was parked.
        crate::synchronization::record_subscription::publish_scope_exists(
            &state, workspace, package,
        );
        // Records that arrived before the key can now be applied.
        replication::promote_scope(&mut state, workspace, package).map_err(|_| ())
    }

    /// Two Installations that disagree about membership are exactly the case a
    /// membership session resolves, so the worker is asked to run one.
    fn note_membership_mismatch(&self, workspace: WorkspaceId, peer: PublicIdentity) {
        self.demand.note(workspace, peer);
    }

    fn session_seed(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionSeed, crate::synchronization::Refusal> {
        use crate::synchronization::Refusal;
        let state = self.state.upgrade().ok_or(Refusal::Unavailable)?;
        let state = state.lock().map_err(|_| Refusal::Unavailable)?;
        if state.poisoned {
            return Err(Refusal::Unavailable);
        }
        let local = PublicIdentity(state.secret_key.public());
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(Refusal::NotAuthorized)?;
        if !projection.contains_member(local) || !projection.contains_member(peer) {
            return Err(Refusal::NotAuthorized);
        }
        let scope = crate::synchronization::scope_digest(package, workspace);
        if crate::synchronization::quarantine::is_scope_quarantined(&state.connection, scope)
            .map_err(|_| Refusal::Unavailable)?
        {
            return Err(Refusal::Unavailable);
        }
        let mut authors = projection
            .snapshot()
            .members()
            .iter()
            .map(|member| member.identity())
            .collect::<std::collections::BTreeSet<_>>();
        let mut heights = std::collections::BTreeMap::new();
        {
            let mut statement = state
                .connection
                .prepare(
                    "SELECT author_public_identity, MAX(author_sequence)
                     FROM p2panda_operations WHERE scope_id=?1 AND status='accepted'
                     GROUP BY author_public_identity",
                )
                .map_err(|_| Refusal::Unavailable)?;
            let rows = statement
                .query_map([scope.as_slice()], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|_| Refusal::Unavailable)?;
            for row in rows {
                let (author, height) = row.map_err(|_| Refusal::Unavailable)?;
                let Ok(author) = <[u8; 32]>::try_from(author).map(PublicIdentity::from_bytes)
                else {
                    continue;
                };
                let Ok(author) = author else { continue };
                authors.insert(author);
                let Ok(height) = u32::try_from(height) else {
                    continue;
                };
                heights.insert(author, height);
            }
        }
        // A quarantined record ends the author's served log: a peer would
        // refuse anything past a missing sequence anyway.
        for (author, height) in &mut heights {
            let lowest = crate::synchronization::quarantine::lowest_quarantined_sequence(
                &state.connection,
                scope,
                *author,
            )
            .map_err(|_| Refusal::Unavailable)?;
            if let Some(lowest) = lowest.and_then(|value| u32::try_from(value).ok()) {
                *height = lowest.saturating_sub(1);
                if lowest == 0 {
                    *height = u32::MAX;
                }
            }
        }
        heights.retain(|_, height| *height != u32::MAX);
        Ok(crate::synchronization::SessionSeed {
            authors: authors.into_iter().collect(),
            heights,
        })
    }

    fn range_sizes(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
        limit: usize,
    ) -> Result<Vec<(u32, usize)>, ()> {
        let state = self.state.upgrade().ok_or(())?;
        let state = state.lock().map_err(|_| ())?;
        if !state.memberships.get(&workspace).is_some_and(|projection| {
            projection.contains_member(PublicIdentity(state.secret_key.public()))
                && projection.contains_member(peer)
        }) {
            return Err(());
        }
        let scope = crate::synchronization::scope_digest(package, workspace);
        let mut statement = state
            .connection
            .prepare(
                "SELECT author_sequence, length(header) + length(payload)
                 FROM p2panda_operations
                 WHERE scope_id=?1 AND author_public_identity=?2 AND status='accepted'
                   AND author_sequence > ?3 AND author_sequence <= ?4
                 ORDER BY author_sequence LIMIT ?5",
            )
            .map_err(|_| ())?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    scope.as_slice(),
                    author.as_bytes().as_slice(),
                    i64::from(
                        after.map_or(-1_i32, |value| i32::try_from(value).unwrap_or(i32::MAX))
                    ),
                    i64::from(until.unwrap_or(u32::MAX)),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|_| ())?;
        let mut sizes = Vec::new();
        for row in rows {
            let (sequence, bytes) = row.map_err(|_| ())?;
            sizes.push((
                u32::try_from(sequence).map_err(|_| ())?,
                usize::try_from(bytes).map_err(|_| ())?,
            ));
        }
        Ok(sizes)
    }

    fn load_range_page(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        author: PublicIdentity,
        from: u32,
        until: u32,
        rows: usize,
    ) -> Result<Vec<crate::synchronization::ReplicationEntry>, crate::synchronization::RangeLoadError>
    {
        use crate::synchronization::RangeLoadError;
        let state = self.state.upgrade().ok_or(RangeLoadError::Unavailable)?;
        let state = state.lock().map_err(|_| RangeLoadError::Unavailable)?;
        if !state.memberships.get(&workspace).is_some_and(|projection| {
            projection.contains_member(PublicIdentity(state.secret_key.public()))
                && projection.contains_member(peer)
        }) {
            return Err(RangeLoadError::Unavailable);
        }
        let scope = crate::synchronization::scope_digest(package, workspace);
        type ProjectionRow = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
        let page: Vec<ProjectionRow> = {
            let mut statement = state
                .connection
                .prepare(
                    "SELECT operation_id, header, payload, payload_hash
                     FROM p2panda_operations
                     WHERE scope_id=?1 AND author_public_identity=?2 AND status='accepted'
                       AND author_sequence >= ?3 AND author_sequence <= ?4
                     ORDER BY author_sequence LIMIT ?5",
                )
                .map_err(|_| RangeLoadError::Unavailable)?;
            let mapped = statement
                .query_map(
                    rusqlite::params![
                        scope.as_slice(),
                        author.as_bytes().as_slice(),
                        i64::from(from),
                        i64::from(until),
                        i64::try_from(rows).unwrap_or(i64::MAX),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, Vec<u8>>(3)?,
                        ))
                    },
                )
                .map_err(|_| RangeLoadError::Unavailable)?;
            mapped
                .collect::<Result<_, _>>()
                .map_err(|_| RangeLoadError::Unavailable)?
        };
        let now = self.clock.now_unix_ms();
        let mut entries = Vec::with_capacity(page.len());
        for (operation_id, header_bytes, payload, payload_hash) in page {
            // Every row is authenticated as it is read, so a tampered
            // projection is caught here rather than served to a peer.
            let header = p2panda_core::Header::<()>::decode(&header_bytes).ok();
            let sound = header.as_ref().is_some_and(|header| {
                header.hash().as_bytes().as_slice() == operation_id
                    && p2panda_core::Hash::digest(&payload).as_bytes().as_slice() == payload_hash
            });
            let Some(header) = header.filter(|_| sound) else {
                let _ = crate::synchronization::quarantine::quarantine_scope(
                    &state.connection,
                    scope,
                    crate::synchronization::quarantine::ScopeFault::Projection,
                    now,
                );
                drop(state);
                self.health.scope_changed(scope);
                return Err(RangeLoadError::ScopeCorrupt);
            };
            #[cfg(test)]
            crate::synchronization::note_record_loaded_for_test();
            entries.push((
                p2panda_core::Operation::from_parts(
                    header,
                    Some(p2panda_core::Body::from_bytes(&payload)),
                ),
                header_bytes,
            ));
        }
        Ok(entries)
    }

    /// Classifies one received operation in the order design §5.5 fixes.
    ///
    /// Only a revocation or a storage failure ends the session; anything else
    /// is applied, recognised as a duplicate, parked, or rejected.
    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        self.receive_operations(peer, workspace, package, vec![operation])?
            .pop()
            .ok_or(crate::synchronization::FatalSyncError::Storage)
    }

    /// Applies a batch of received operations in one lock hold.
    ///
    /// The peer's membership is re-checked once for the batch; each operation
    /// is then classified and committed in its own transaction, so a rejection
    /// isolates one operation rather than the batch.
    fn receive_operations(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operations: Vec<p2panda_core::Operation<()>>,
    ) -> Result<Vec<crate::synchronization::OperationOutcome>, crate::synchronization::FatalSyncError>
    {
        use crate::synchronization::FatalSyncError;
        let state = self.state.upgrade().ok_or(FatalSyncError::Storage)?;
        let mut state = state.lock().map_err(|_| FatalSyncError::Storage)?;
        let Some(projection) = state.memberships.get(&workspace) else {
            return Err(FatalSyncError::PeerRevoked);
        };
        if !projection.contains_member(peer) {
            return Err(FatalSyncError::PeerRevoked);
        }
        let now = self.clock.now_unix_ms();
        let mut outcomes = Vec::with_capacity(operations.len());
        for operation in operations {
            outcomes.push(classify_operation(
                &mut state, peer, workspace, package, &operation, now,
            )?);
        }
        Ok(outcomes)
    }
}

/// Classifies one received operation while the runtime state is already held.
fn classify_operation(
    state: &mut RuntimeState,
    peer: PublicIdentity,
    workspace: WorkspaceId,
    package: &str,
    operation: &p2panda_core::Operation<()>,
    now: i64,
) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError> {
    use crate::synchronization::{FatalSyncError, OperationOutcome, RejectReason};
    let Some(author) =
        crate::synchronization::key_bridge::identity(&operation.header.verifying_key)
    else {
        return Ok(OperationOutcome::Rejected(RejectReason::Invalid));
    };
    let known_author = state
        .memberships
        .get(&workspace)
        .ok_or(FatalSyncError::PeerRevoked)?
        .has_membership_history(author);
    if !known_author {
        return park_outcome(
            state,
            workspace,
            package,
            peer,
            crate::synchronization::parking::ParkReason::UnknownAuthor,
            operation,
            now,
        );
    }
    match crate::synchronization::insert_remote_operation(
        &mut state.connection,
        workspace,
        package,
        operation,
        now,
    ) {
        Ok(crate::synchronization::RemoteInsert::Applied {
            delivery_sequence, ..
        }) => {
            crate::synchronization::record_subscription::publish_commit(
                state,
                workspace,
                package,
                delivery_sequence,
            );
            Ok(OperationOutcome::Applied)
        }
        Ok(crate::synchronization::RemoteInsert::Duplicate(_)) => Ok(OperationOutcome::Duplicate),
        Err(crate::synchronization::RemoteInsertError::MissingScope) => park_outcome(
            state,
            workspace,
            package,
            peer,
            crate::synchronization::parking::ParkReason::MissingContentKey,
            operation,
            now,
        ),
        Err(crate::synchronization::RemoteInsertError::KeyMismatch) => {
            Ok(OperationOutcome::Rejected(RejectReason::KeyMismatch))
        }
        Err(crate::synchronization::RemoteInsertError::WrongScope) => {
            Ok(OperationOutcome::Rejected(RejectReason::WrongScope))
        }
        Err(crate::synchronization::RemoteInsertError::Conflict) => {
            Ok(OperationOutcome::Rejected(RejectReason::Conflict))
        }
        Err(crate::synchronization::RemoteInsertError::Invalid) => {
            Ok(OperationOutcome::Rejected(RejectReason::Invalid))
        }
        Err(crate::synchronization::RemoteInsertError::HeightOutOfRange) => {
            Ok(OperationOutcome::Rejected(RejectReason::HeightOutOfRange))
        }
        Err(crate::synchronization::RemoteInsertError::Storage) => Err(FatalSyncError::Storage),
    }
}

/// Parks one operation and reports the session outcome that follows.
#[allow(clippy::too_many_arguments)]
fn park_outcome(
    state: &mut RuntimeState,
    workspace: WorkspaceId,
    package: &str,
    peer: PublicIdentity,
    reason: crate::synchronization::parking::ParkReason,
    operation: &p2panda_core::Operation<()>,
    now_unix_ms: i64,
) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError> {
    use crate::synchronization::parking::{ParkResult, ParkingLimits};
    use crate::synchronization::{FatalSyncError, OperationOutcome, RejectReason};
    match crate::synchronization::parking::park(
        &mut state.connection,
        workspace,
        package,
        peer,
        reason,
        operation,
        now_unix_ms,
        ParkingLimits::CURRENT,
    ) {
        Ok(ParkResult::Parked | ParkResult::AlreadyParked) => Ok(OperationOutcome::Parked(reason)),
        Ok(ParkResult::QuotaExceeded) => Ok(OperationOutcome::ParkRefused),
        Err(crate::SynchronizationError::WrongScope) => {
            Ok(OperationOutcome::Rejected(RejectReason::WrongScope))
        }
        Err(crate::SynchronizationError::InvalidInput) => {
            Ok(OperationOutcome::Rejected(RejectReason::Invalid))
        }
        Err(_) => Err(FatalSyncError::Storage),
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
    /// Authenticates a canonical membership envelope without consulting workspace state.
    ///
    /// This verifies only version-1 structure, content identity, admission proof, and signatures.
    /// Trust anchoring, dependency resolution, timestamp policy, and authorization remain part of
    /// durable workspace processing. The canonical format's own tests are its
    /// only callers.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn authenticate_membership_operation(
        canonical_envelope: &[u8],
    ) -> Result<MembershipOperationId, MembershipError> {
        crate::membership::authenticate(canonical_envelope).map(|envelope| envelope.id)
    }

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
        #[cfg_attr(not(test), allow(unused_variables))] test_readiness: Option<bool>,
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
        .map_err(|_| CoreError::Storage(StorageError::Database))?;
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
            next_subscriber_id: 0,
            poisoned: false,
        }));
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
        let protocols = vec![(
            crate::membership_sync::MEMBERSHIP_SYNC_ALPN.to_vec(),
            Box::new(crate::membership_sync::MembershipSyncHandler::new(
                Arc::clone(&membership_replica),
            )) as Box<dyn iroh::protocol::DynProtocolHandler>,
        )];
        #[cfg(test)]
        let transport = if let Some(ready_immediately) = test_readiness {
            crate::onboarding::transport::Transport::bind_loopback_with_issuer_and_services(
                secret_key.clone(),
                std::time::Duration::from_millis(5),
                ready_immediately,
                issuer,
                services.clone(),
                protocols,
            )
            .await
        } else {
            crate::onboarding::transport::Transport::bind_production(
                secret_key.clone(),
                issuer,
                Some(services.clone()),
                protocols,
            )
            .await
        };
        #[cfg(not(test))]
        let transport = crate::onboarding::transport::Transport::bind_production(
            secret_key.clone(),
            issuer,
            Some(services.clone()),
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
        let runner = crate::synchronization::worker::NetworkSessionRunner::new(
            Arc::clone(&transport),
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
        })
    }

    /// Returns this installation's validated public identity.
    #[must_use]
    pub const fn public_identity(&self) -> PublicIdentity {
        self.public_identity
    }

    /// Returns a snapshot of this installation's validated display name.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .display_name
            .clone()
    }

    /// Updates this installation's display name.
    ///
    /// Names contain 1–64 Unicode alphanumeric characters, spaces, hyphens,
    /// underscores, apostrophes, or periods, with no surrounding whitespace.
    pub fn set_display_name(&self, display_name: &str) -> Result<(), IdentityError> {
        crate::identity::validate_display_name(display_name)?;
        let mut state = self.lock_identity_state()?;
        match crate::storage::update_display_name(&mut state.connection, display_name) {
            Ok(()) => {
                state.display_name = display_name.to_owned();
                Ok(())
            }
            Err(error) => {
                if error == StorageError::UncertainOutcome {
                    state.poisoned = true;
                }
                Err(IdentityError::Storage(error))
            }
        }
    }

    /// Signs bytes under a validated, versioned, length-delimited domain context.
    pub fn sign(
        &self,
        context: &SigningContext,
        payload: &[u8],
    ) -> Result<PublicSignature, IdentityError> {
        let state = self.lock_identity_state()?;
        crate::identity::sign(&state.secret_key, context.as_str(), payload)
            .map(|signature| PublicSignature(signature.to_bytes()))
    }

    fn lock_identity_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, IdentityError> {
        let state = self
            .state
            .lock()
            .map_err(|_| IdentityError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(IdentityError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Creates a named workspace with this installation as its first manager.
    ///
    /// Names contain 1–64 Unicode alphanumeric characters, spaces, hyphens,
    /// underscores, apostrophes, or periods, with no surrounding whitespace.
    pub fn create_workspace(&self, name: &str) -> Result<WorkspaceSummary, WorkspaceError> {
        crate::workspace::validate_name(name)?;
        let mut state = self.lock_workspace_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        let secret_key = state.secret_key.clone();
        let result =
            crate::workspace::create(&mut state.connection, &secret_key, name, now_unix_ms);
        match result {
            Ok(summary) => {
                match crate::membership::projection::recover(&mut state.connection, now_unix_ms) {
                    Ok(memberships) => state.memberships = memberships,
                    Err(error) => {
                        if error == StorageError::UncertainOutcome {
                            state.poisoned = true;
                        }
                        return Err(WorkspaceError::Storage(error));
                    }
                }
                Ok(summary)
            }
            Err(error) => {
                if matches!(
                    error,
                    WorkspaceError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Lists workspaces known to this installation.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        let state = self.lock_workspace_state()?;
        crate::workspace::list(&state.connection)
    }

    fn lock_workspace_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, WorkspaceError> {
        let state = self
            .state
            .lock()
            .map_err(|_| WorkspaceError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(WorkspaceError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Issues an expiring invitation as the local manager.
    #[allow(dead_code)]
    pub(crate) fn issue_invitation(
        &self,
        workspace_id: WorkspaceId,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        self.issue_invitation_at(workspace_id, self.clock.now_unix_ms(), expires_at_unix_ms)
    }

    fn issue_invitation_at(
        &self,
        workspace_id: WorkspaceId,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        let mut state = self.lock_invitation_state()?;
        self.maintain_and_publish(&mut state, issued_at_unix_ms)
            .map_err(InvitationError::Membership)?;
        let RuntimeState {
            secret_key,
            memberships,
            ..
        } = &mut *state;
        let projection = memberships
            .get(&workspace_id)
            .ok_or(InvitationError::WorkspaceNotFound)?;
        let local_is_manager = projection.snapshot().members().iter().any(|member| {
            member.identity() == self.public_identity && member.role() == MembershipRole::Manager
        });
        if !local_is_manager {
            return Err(InvitationError::NotManager);
        }
        if expires_at_unix_ms <= issued_at_unix_ms {
            return Err(InvitationError::Expired);
        }
        let nonce = SecretKey::generate().to_bytes();
        let token = crate::invitation::sign_token(
            secret_key,
            workspace_id,
            issued_at_unix_ms,
            expires_at_unix_ms,
            nonce,
        )?;
        Ok(InvitationToken::from_verified(&token))
    }

    /// Creates a fixed-lifetime opaque invitation after endpoint readiness succeeds.
    pub async fn create_workspace_invitation(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        const INVITATION_LIFETIME_MS: i64 = 15 * 60 * 1_000;

        let operation = self.transport.begin_invitation().await?;
        #[cfg(test)]
        self.transport.pause_after_invitation_readiness().await;
        let issued_at_unix_ms = self.clock.now_unix_ms();
        let token = self
            .issue_invitation_at(
                workspace_id,
                issued_at_unix_ms,
                issued_at_unix_ms.saturating_add(INVITATION_LIFETIME_MS),
            )
            .map_err(map_invitation_onboarding_error)?;
        let verified = token
            .verified()
            .map_err(|_| crate::OnboardingError::Authentication)?;
        let invitation = crate::WorkspaceInvitation::from_parts(&verified, operation.address())
            .map_err(|_| crate::OnboardingError::InvalidInput);
        drop(operation);
        invitation
    }

    /// Joins the invited Workspace after authenticated transport and atomic local import.
    pub async fn join_workspace(
        &self,
        invitation: &crate::WorkspaceInvitation,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        self.join_workspace_with_budget(
            invitation,
            crate::onboarding::transport::WHOLE_JOIN_TIMEOUT,
        )
        .await
    }

    async fn join_workspace_with_budget(
        &self,
        invitation: &crate::WorkspaceInvitation,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let deadline = std::time::Instant::now()
            .checked_add(budget)
            .ok_or(crate::OnboardingError::Timeout)?;
        let permit = self.transport.begin_outbound().await?;
        let (verified, claim, response) = self
            .transport
            .whole_join_until(
                self.join_network_phase(invitation, deadline, &permit),
                deadline,
            )
            .await?;
        // Validation commits nothing, so the whole-join timer still bounds it.
        Self::ensure_join_deadline(deadline)?;
        let local_identity = self.public_identity;
        #[cfg(test)]
        let transport = Arc::clone(&self.transport);
        let validating = async move {
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                transport.block_after_response();
                crate::membership::projection::validate_bootstrap(
                    response,
                    &verified.token,
                    &claim,
                    local_identity,
                )
            })
            .await
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)?
        };
        let bootstrap = self
            .transport
            .whole_join_until(validating, deadline)
            .await?;
        // The import runs outside the timer: a timer must never report
        // `Timeout` for an import that then commits. The closure enforces the
        // deadline itself, and the outbound permit is held until it finishes.
        Self::ensure_join_deadline(deadline)?;
        let state = Arc::clone(&self.state);
        let imported = tokio::task::spawn_blocking(move || {
            let state = match crate::blocking::lock_until(&state, deadline) {
                Ok(state) => state,
                Err(crate::blocking::LockUntilError::Timeout) => {
                    return Err(crate::OnboardingError::Timeout);
                }
                Err(crate::blocking::LockUntilError::Poisoned) => {
                    return Err(crate::OnboardingError::UncertainStorageOutcome);
                }
            };
            import_validated_bootstrap_locked(state, &bootstrap, Some(deadline), |_| Ok(()))
        })
        .await
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        drop(permit);
        imported
    }

    /// The network half of a join: the pre-network claim and the exchange.
    ///
    /// Everything here is bounded by the whole-join timer, because nothing here
    /// commits.
    async fn join_network_phase(
        &self,
        invitation: &crate::WorkspaceInvitation,
        deadline: std::time::Instant,
        permit: &crate::onboarding::transport::OutboundPermit,
    ) -> Result<
        (
            crate::onboarding::transfer::VerifiedWorkspaceInvitation,
            crate::invitation::VerifiedInvitationClaim,
            crate::onboarding::wire::SuccessResponse,
        ),
        crate::OnboardingError,
    > {
        let verified = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        Self::ensure_join_deadline(deadline)?;
        let state = Arc::clone(&self.state);
        let token = verified.token.clone();
        let claim = tokio::task::spawn_blocking(move || {
            let state = match crate::blocking::lock_until(&state, deadline) {
                Ok(state) => state,
                Err(crate::blocking::LockUntilError::Timeout) => {
                    return Err(crate::OnboardingError::Timeout);
                }
                Err(crate::blocking::LockUntilError::Poisoned) => {
                    return Err(crate::OnboardingError::UncertainStorageOutcome);
                }
            };
            if state.poisoned {
                return Err(crate::OnboardingError::UncertainStorageOutcome);
            }
            let local_creator: Option<Vec<u8>> = state
                .connection
                .query_row(
                    "SELECT creator_public_identity FROM workspaces WHERE workspace_id = ?1",
                    [token.workspace_id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| crate::OnboardingError::LocalConflict)?;
            if matches!(local_creator, Some(creator) if creator != token.inviter.as_bytes()) {
                return Err(crate::OnboardingError::LocalConflict);
            }
            crate::invitation::sign_claim(&state.secret_key, &token)
                .map_err(map_invitation_onboarding_error)
        })
        .await
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)??;
        Self::ensure_join_deadline(deadline)?;

        let response = crate::onboarding::client::join(
            &self.transport,
            permit,
            verified.clone(),
            claim.clone(),
        )
        .await?;
        #[cfg(test)]
        self.transport.pause_after_response().await;
        Ok((verified, claim, response))
    }

    fn ensure_join_deadline(deadline: std::time::Instant) -> Result<(), crate::OnboardingError> {
        if std::time::Instant::now() < deadline {
            Ok(())
        } else {
            Err(crate::OnboardingError::Timeout)
        }
    }

    #[cfg(test)]
    fn import_validated_bootstrap(
        &self,
        bootstrap: &crate::membership::projection::ValidatedBootstrap,
        deadline: Option<std::time::Instant>,
        fault: impl FnMut(crate::membership::projection::ImportBoundary) -> Result<(), StorageError>,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        if let Some(deadline) = deadline {
            Self::ensure_join_deadline(deadline)?;
        }
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        import_validated_bootstrap_locked(state, bootstrap, deadline, fault)
    }

    /// Appends one authenticated, protected immutable record to the workspace journal.
    pub fn append_record(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        purpose: &str,
        canonical: &[u8],
    ) -> Result<crate::ImmutableRecord, crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let standing = state.memberships.get(&workspace_id).map_or(
            crate::synchronization::MembershipStanding::None,
            |projection| {
                if projection.contains_member(self.public_identity) {
                    crate::synchronization::MembershipStanding::Current
                } else if projection.has_membership_history(self.public_identity) {
                    crate::synchronization::MembershipStanding::Revoked
                } else {
                    crate::synchronization::MembershipStanding::None
                }
            },
        );
        let is_creator = state
            .memberships
            .get(&workspace_id)
            .is_some_and(|projection| projection.creator() == self.public_identity);
        let secret_key = state.secret_key.clone();
        let now = self.clock.now_unix_ms();
        let (record, delivery_sequence) = crate::synchronization::append(
            &mut state.connection,
            &secret_key,
            workspace_id,
            package_identity,
            purpose,
            canonical,
            standing,
            is_creator,
            now,
        )?;
        crate::synchronization::record_subscription::publish_commit(
            &state,
            workspace_id,
            package_identity,
            delivery_sequence,
        );
        self.sync_worker.wake();
        Ok(record)
    }

    /// Imports a welcome bundle that traces to the Workspace creator's origin.
    #[cfg(test)]
    pub(crate) fn import_protection_grant(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        origin: &[u8],
        grant: &[u8],
    ) -> Result<usize, crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let author = crate::synchronization::protection_control_author(grant)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::SynchronizationError::WorkspaceNotFound)?;
        if !projection.contains_member(author) {
            return Err(crate::SynchronizationError::NotCurrentMember);
        }
        let creator = projection.creator();
        let secret = state.secret_key.clone();
        let imported = crate::synchronization::import_protection_grant(
            &mut state.connection,
            &secret,
            workspace_id,
            package_identity,
            creator,
            origin,
            grant,
        )?;
        crate::synchronization::record_subscription::publish_scope_exists(
            &state,
            workspace_id,
            package_identity,
        );
        Ok(imported)
    }

    /// Opens sealed records in request order after authenticating their scope and contents.
    pub fn open_records(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        sealed_records: &[Vec<u8>],
    ) -> Result<Vec<crate::OpenedRecord>, crate::SynchronizationError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let now_unix_ms = state.clock.now_unix_ms();
        crate::synchronization::open_records(
            &state.connection,
            workspace_id,
            package_identity,
            sealed_records,
            now_unix_ms,
        )
    }

    /// Replays the installation-local immutable-record journal from an optional cursor.
    ///
    /// The subscription holds no queue: it remembers a position and reads pages
    /// of committed records. Subscribing before the scope exists is allowed; the
    /// subscription waits for the content key to arrive.
    ///
    /// # Errors
    ///
    /// Returns [`crate::SynchronizationError::CursorScopeMismatch`] for a cursor
    /// from another delivery journal and
    /// [`crate::SynchronizationError::CursorAhead`] for one beyond the journal's
    /// head.
    pub fn subscribe_records(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        after: Option<crate::Cursor>,
    ) -> Result<crate::RecordSubscription, crate::SynchronizationError> {
        use crate::synchronization::record_subscription as records;
        let state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        let journal = records::journal_of(&state.connection, scope)?;
        let position = records::validate_cursor(&state.connection, scope, journal, after)?;
        let head = records::scope_head(&state, scope);
        Ok(records::RecordSubscription::new(
            Arc::downgrade(&self.state),
            workspace_id,
            package_identity.to_owned(),
            scope,
            journal,
            position,
            head,
            self.public_identity,
            Arc::clone(&self.health),
        ))
    }

    /// Watches one Workspace's local membership epoch.
    ///
    /// The Host uses it to open a membership subscription only when there is a
    /// transition to read, instead of polling. `tokio::sync::watch` appears here
    /// deliberately: Core already depends on Tokio, and the Host is its only
    /// consumer.
    ///
    /// # Errors
    ///
    /// Returns [`SubscriptionError::WorkspaceNotFound`] when the Workspace is
    /// not known locally.
    pub fn membership_epoch_watch(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<tokio::sync::watch::Receiver<u64>, SubscriptionError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubscriptionError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(SubscriptionError::Storage(StorageError::RuntimePoisoned));
        }
        let epoch = state
            .memberships
            .get(&workspace_id)
            .ok_or(SubscriptionError::WorkspaceNotFound)?
            .snapshot()
            .epoch()
            .value();
        Ok(state
            .membership_epochs
            .entry(workspace_id)
            .or_insert_with(|| tokio::sync::watch::Sender::new(epoch))
            .subscribe())
    }

    /// Stops accepting onboarding and synchronization work and awaits closure.
    ///
    /// Local identity, Workspace, and membership commands remain available.
    pub async fn shutdown(&self) -> Result<(), crate::OnboardingError> {
        // Stop the scheduler and await every session before Router/Endpoint
        // shutdown. This ordering prevents SQLite callbacks from racing the
        // shared network teardown.
        self.sync_worker.shutdown().await;
        self.transport.shutdown().await
    }

    /// Creates proof that this installation claims the supplied invitation.
    #[allow(dead_code)]
    pub(crate) fn create_invitation_claim(
        &self,
        token: &InvitationToken,
    ) -> Result<InvitationClaim, InvitationError> {
        let token = token.verified()?;
        let state = self.lock_invitation_state()?;
        let claim = crate::invitation::sign_claim(&state.secret_key, &token)?;
        Ok(InvitationClaim::from_verified(&claim))
    }

    /// Redeems a claim on the installation that issued its token.
    #[allow(dead_code)]
    pub(crate) fn redeem_invitation(
        &self,
        claim: &InvitationClaim,
    ) -> Result<MembershipTransition, InvitationError> {
        self.redeem_invitation_inner(claim, |_| Ok(()))
    }

    fn redeem_invitation_inner(
        &self,
        claim: &InvitationClaim,
        fault: impl FnMut(crate::membership::projection::RedemptionBoundary) -> Result<(), StorageError>,
    ) -> Result<MembershipTransition, InvitationError> {
        let claim = claim.verified()?;
        let mut state = self.lock_invitation_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)
            .map_err(InvitationError::Membership)?;
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let authenticated_peer = claim.invitee;
        let result = crate::membership::projection::redeem_invitation_with_fault(
            connection,
            memberships,
            secret_key,
            claim,
            authenticated_peer,
            now_unix_ms,
            fault,
        );
        match result {
            Ok(export) => {
                let accepted = export.applied.accepted;
                let transition = Self::transition_from_applied(export.applied).ok_or(
                    InvitationError::Membership(MembershipError::ConflictingGenesis),
                )?;
                if accepted {
                    replication::after_membership_accepted(&mut state, &transition);
                }
                Ok(transition)
            }
            Err(error) => {
                if matches!(
                    error,
                    InvitationError::Storage(StorageError::UncertainOutcome)
                        | InvitationError::Membership(MembershipError::Storage(
                            StorageError::UncertainOutcome
                        ))
                ) {
                    *poisoned = true;
                }
                Err(error)
            }
        }
    }

    fn lock_invitation_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, InvitationError> {
        let state = self
            .state
            .lock()
            .map_err(|_| InvitationError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(InvitationError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    /// Revokes a workspace member as the local manager.
    pub fn revoke_member(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
    ) -> Result<MembershipTransition, MembershipError> {
        self.revoke_member_with_limits(
            workspace_id,
            member,
            crate::membership::projection::BootstrapLimits::CURRENT,
        )
    }

    fn revoke_member_with_limits(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
    ) -> Result<MembershipTransition, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let current = memberships
            .get(&workspace_id)
            .cloned()
            .ok_or(MembershipError::WorkspaceNotFound)?;
        if current.creator() == member {
            return Err(MembershipError::CannotRevokeManager);
        }
        if !current.contains_member(member) {
            return Err(MembershipError::MemberNotFound);
        }
        let envelope = crate::membership::MembershipEnvelope::sign(
            secret_key,
            workspace_id,
            now_unix_ms,
            crate::membership::MembershipAction::Revoke { target: member },
            current.heads(),
        )?;
        match crate::membership::projection::apply_canonical_with_limits(
            connection,
            memberships,
            envelope.canonical_bytes(),
            now_unix_ms,
            limits,
        ) {
            Ok(applied) if applied.accepted => {
                let transition = Self::transition_from_applied(applied)
                    .ok_or(MembershipError::ConflictingGenesis)?;
                // A revoked Installation must stop being served immediately,
                // not at the end of whatever session is already running.
                replication::after_membership_accepted(&mut state, &transition);
                self.sync_worker.wake();
                Ok(transition)
            }
            Ok(_) => Err(MembershipError::AuthorizationRejected),
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    *poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Returns the current canonical membership projection.
    pub fn membership_snapshot(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MembershipSnapshot, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        state
            .memberships
            .get(&workspace_id)
            .map(crate::membership::WorkspaceProjection::snapshot)
            .ok_or(MembershipError::WorkspaceNotFound)
    }

    fn maintain_and_publish(
        &self,
        state: &mut RuntimeState,
        now_unix_ms: i64,
    ) -> Result<(), MembershipError> {
        replication::maintain_and_publish(state, now_unix_ms)
    }

    /// Returns `None` when the causing operation is a workspace `create`, which
    /// starts membership instead of transitioning it.
    fn transition_from_applied(
        applied: crate::membership::projection::AppliedMembership,
    ) -> Option<MembershipTransition> {
        Some(MembershipTransition::new(
            applied.snapshot.workspace_id(),
            applied.snapshot.epoch(),
            applied.snapshot.version(),
            applied.operation_id,
            applied.snapshot_digest,
            applied.change?,
            applied.author,
        ))
    }

    fn publish_transition(state: &mut RuntimeState, transition: MembershipTransition) {
        if let Some(listeners) = state.subscribers.get_mut(&transition.workspace_id()) {
            listeners.retain(|listener| listener.sender.send(transition.clone()).is_ok());
        }
    }

    fn lock_membership_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, MembershipError> {
        let state = self
            .state
            .lock()
            .map_err(|_| MembershipError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(MembershipError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }

    // Phase 6 invitation redemption will call this same production path.
    #[allow(dead_code)]
    pub(crate) fn apply_membership_envelope(
        &self,
        canonical_envelope: &[u8],
    ) -> Result<crate::membership::projection::AppliedMembership, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let result = {
            let RuntimeState {
                connection,
                memberships,
                ..
            } = &mut *state;
            crate::membership::projection::apply_canonical(
                connection,
                memberships,
                canonical_envelope,
                now_unix_ms,
            )
        };
        match result {
            Ok(applied) => {
                if applied.accepted
                    && let Some(change) = applied.change
                {
                    let transition = MembershipTransition::new(
                        applied.snapshot.workspace_id(),
                        applied.snapshot.epoch(),
                        applied.snapshot.version(),
                        applied.operation_id,
                        applied.snapshot_digest,
                        change,
                        applied.author,
                    );
                    replication::after_membership_accepted(&mut state, &transition);
                }
                Ok(applied)
            }
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Returns the current content-derived membership version.
    pub fn membership_version(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<MembershipVersion, MembershipError> {
        self.membership_snapshot(workspace_id)
            .map(|snapshot| snapshot.version())
    }

    /// Answers authenticated author-declared membership history.
    ///
    /// Admission is inclusive at its signed authored millisecond and revocation
    /// is exclusive at its signed authored millisecond. Late operations can
    /// revise an earlier answer. This informational query must never authorize
    /// a new write; authorization must use current membership and its version.
    pub fn was_member(
        &self,
        workspace_id: WorkspaceId,
        identity: PublicIdentity,
        at_unix_ms: i64,
    ) -> Result<bool, MembershipError> {
        let mut state = self.lock_membership_state()?;
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(MembershipError::WorkspaceNotFound)?;
        crate::membership::was_member(projection, workspace_id, identity, at_unix_ms)
    }

    /// Subscribes after a durable local epoch cursor.
    ///
    /// Durable catch-up is returned first in epoch order, followed by live
    /// post-commit delivery. Registering the listener and reading catch-up both
    /// happen while the runtime state lock is held, preventing a handoff gap.
    pub fn subscribe_membership(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        self.subscribe_membership_inner(workspace_id, after_epoch, None)
    }

    /// Subscribes with at most 32 durable catch-up transitions held in memory.
    ///
    /// Bounded consumers reopen after their last delivered epoch to continue
    /// catch-up without owning an unbounded live receiver or durable buffer.
    pub fn subscribe_membership_bounded(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
        max_catch_up: usize,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        self.subscribe_membership_inner(workspace_id, after_epoch, Some(max_catch_up.clamp(1, 32)))
    }

    fn subscribe_membership_inner(
        &self,
        workspace_id: WorkspaceId,
        after_epoch: MembershipEpoch,
        catch_up_limit: Option<usize>,
    ) -> Result<MembershipSubscription, SubscriptionError> {
        #[cfg(feature = "test-hooks")]
        if let Ok(mut counts) = MEMBERSHIP_SUBSCRIPTIONS_OPENED.lock() {
            *counts.entry(workspace_id).or_insert(0) += 1;
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| SubscriptionError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(SubscriptionError::Storage(StorageError::RuntimePoisoned));
        }
        let now_unix_ms = self.clock.now_unix_ms();
        self.maintain_and_publish(&mut state, now_unix_ms)
            .map_err(|error| match error {
                MembershipError::Storage(error) => SubscriptionError::Storage(error),
                _ => SubscriptionError::Storage(StorageError::CorruptWorkspace),
            })?;
        let (sender, receiver) = mpsc::channel();
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);
        let subscriber_id = state.next_subscriber_id;
        state
            .subscribers
            .entry(workspace_id)
            .or_default()
            .push(MembershipSubscriber {
                id: subscriber_id,
                sender,
            });
        let durable = match catch_up_limit {
            Some(limit) => crate::subscription::load_after_bounded(
                &state.connection,
                workspace_id,
                after_epoch,
                limit,
            ),
            None => crate::subscription::load_after(&state.connection, workspace_id, after_epoch),
        };
        let durable = match durable {
            Ok(durable) => durable,
            Err(error) => {
                if let Some(listeners) = state.subscribers.get_mut(&workspace_id) {
                    listeners.retain(|listener| listener.id != subscriber_id);
                    if listeners.is_empty() {
                        state.subscribers.remove(&workspace_id);
                    }
                }
                return Err(error);
            }
        };
        Ok(MembershipSubscription {
            state: crate::subscription::SubscriptionState::new(durable, receiver, after_epoch),
            runtime_state: Arc::downgrade(&self.state),
            workspace_id,
            subscriber_id,
        })
    }
}

/// Imports a validated bootstrap with the state lock already held.
///
/// A free function so the whole import can run inside one blocking closure,
/// without a `&CoreRuntime` reference crossing the thread boundary.
fn import_validated_bootstrap_locked(
    mut state: std::sync::MutexGuard<'_, RuntimeState>,
    bootstrap: &crate::membership::projection::ValidatedBootstrap,
    deadline: Option<std::time::Instant>,
    fault: impl FnMut(crate::membership::projection::ImportBoundary) -> Result<(), StorageError>,
) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
    if state.poisoned {
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    if let Some(deadline) = deadline {
        CoreRuntime::ensure_join_deadline(deadline)?;
    }
    let prior_busy_timeout_ms = state
        .connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .map_err(|_| crate::OnboardingError::LocalConflict)?;
    let prior_busy_timeout_ms =
        u64::try_from(prior_busy_timeout_ms).map_err(|_| crate::OnboardingError::LocalConflict)?;
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(crate::OnboardingError::Timeout)?;
        state
            .connection
            .busy_timeout(remaining)
            .map_err(|_| crate::OnboardingError::LocalConflict)?;
    }
    let result = crate::membership::projection::import_bootstrap_with_fault(
        &mut state.connection,
        bootstrap,
        deadline,
        fault,
    );
    if state
        .connection
        .busy_timeout(std::time::Duration::from_millis(prior_busy_timeout_ms))
        .is_err()
    {
        state.poisoned = true;
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    if !matches!(
        &result,
        Err(crate::OnboardingError::UncertainStorageOutcome)
    ) && result.is_err()
        && deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        return Err(crate::OnboardingError::Timeout);
    }
    match result {
        Ok(imported) => {
            let snapshot = imported.projection.snapshot();
            state
                .memberships
                .insert(snapshot.workspace_id(), imported.projection);
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                state.poisoned = true;
                return Err(crate::OnboardingError::UncertainStorageOutcome);
            }
            Ok((imported.summary, snapshot))
        }
        Err(error) => {
            if error == crate::OnboardingError::UncertainStorageOutcome {
                state.poisoned = true;
            }
            Err(error)
        }
    }
}

pub(crate) fn respond_to_join(
    state: &Arc<Mutex<RuntimeState>>,
    clock: &Arc<dyn Clock>,
    authenticated_peer: PublicIdentity,
    claim: crate::invitation::VerifiedInvitationClaim,
) -> Result<Vec<u8>, crate::OnboardingError> {
    if claim.invitee != authenticated_peer {
        return Err(crate::OnboardingError::Authentication);
    }
    let mut state = state
        .lock()
        .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
    if state.poisoned {
        return Err(crate::OnboardingError::UncertainStorageOutcome);
    }
    let now_unix_ms = clock.now_unix_ms();
    let applied = {
        let RuntimeState {
            connection,
            memberships,
            ..
        } = &mut *state;
        crate::membership::projection::maintain_quarantine(connection, memberships, now_unix_ms)
    }
    .map_err(|error| match error {
        MembershipError::Storage(StorageError::UncertainOutcome) => {
            state.poisoned = true;
            crate::OnboardingError::UncertainStorageOutcome
        }
        _ => crate::OnboardingError::InvalidInput,
    })?;
    for applied in applied {
        if let Some(transition) = CoreRuntime::transition_from_applied(applied) {
            replication::after_membership_accepted(&mut state, &transition);
        }
    }
    let RuntimeState {
        connection,
        secret_key,
        memberships,
        poisoned,
        ..
    } = &mut *state;
    match crate::membership::projection::redeem_invitation(
        connection,
        memberships,
        secret_key,
        claim,
        authenticated_peer,
        now_unix_ms,
    ) {
        Ok(export) => {
            if export.applied.accepted
                && let Some(transition) = CoreRuntime::transition_from_applied(export.applied)
            {
                replication::after_membership_accepted(&mut state, &transition);
            }
            Ok(export.response)
        }
        Err(error) => {
            if matches!(
                error,
                InvitationError::Storage(StorageError::UncertainOutcome)
                    | InvitationError::Membership(MembershipError::Storage(
                        StorageError::UncertainOutcome
                    ))
            ) {
                *poisoned = true;
            }
            Err(map_invitation_onboarding_error(error))
        }
    }
}

pub(crate) fn map_invitation_onboarding_error(error: InvitationError) -> crate::OnboardingError {
    match error {
        InvitationError::Expired => crate::OnboardingError::Expired,
        InvitationError::AlreadyUsed => crate::OnboardingError::AlreadyUsed,
        InvitationError::Storage(StorageError::UncertainOutcome)
        | InvitationError::Membership(MembershipError::Storage(StorageError::UncertainOutcome)) => {
            crate::OnboardingError::UncertainStorageOutcome
        }
        InvitationError::WorkspaceNotFound => crate::OnboardingError::LocalConflict,
        InvitationError::Membership(MembershipError::MemberNotFound) => {
            crate::OnboardingError::AdmissionNoLongerCurrent
        }
        InvitationError::Membership(MembershipError::DecodeLimit) => {
            crate::OnboardingError::Overload
        }
        InvitationError::NotManager
        | InvitationError::WrongIssuer
        | InvitationError::InvalidTokenSignature
        | InvitationError::InvalidClaimSignature
        | InvitationError::ClaimTargetMismatch => crate::OnboardingError::Authentication,
        _ => crate::OnboardingError::InvalidInput,
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        self.transport.start_abnormal_shutdown();
    }
}

mod replication;
mod subscription;
#[cfg(any(test, feature = "test-hooks"))]
mod test_hooks;
mod types;

pub use subscription::MembershipSubscription;
#[cfg(test)]
pub(crate) use test_hooks::ProbeOutcome;
#[cfg(any(test, feature = "test-hooks"))]
pub use test_hooks::RuntimeStateHold;
pub use types::*;

#[cfg(feature = "test-hooks")]
use subscription::MEMBERSHIP_SUBSCRIPTIONS_OPENED;
use subscription::MembershipSubscriber;
#[cfg(feature = "test-hooks")]
pub(crate) use subscription::membership_subscriptions_opened;

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;
