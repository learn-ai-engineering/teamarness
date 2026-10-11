//! The Core side of every package synchronization session.

use super::*;
use crate::diag::fault;
use crate::synchronization::SyncRejection;

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
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, SyncRejection> {
        let state = self.state.upgrade().ok_or(SyncRejection::RuntimeGone)?;
        let mut state = state.lock().map_err(|_| SyncRejection::RuntimeGone)?;
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(SyncRejection::UnknownWorkspace)?;
        let local = PublicIdentity(state.secret_key.public());
        if !projection.contains_member(local) || !projection.contains_member(recipient) {
            return Err(SyncRejection::NotMember);
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
            .map_err(fault!(SyncRejection::Storage))?;
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
            Err(error) => Err(fault!(SyncRejection::Grant)(error)),
        }
    }

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, SyncRejection> {
        let state = self.state.upgrade().ok_or(SyncRejection::RuntimeGone)?;
        let mut state = state.lock().map_err(|_| SyncRejection::RuntimeGone)?;
        if crate::synchronization::protection_control_author(&bundle.grant)
            .map_err(|_| SyncRejection::InvalidProof)?
            != peer
        {
            return Err(SyncRejection::InvalidProof);
        }
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(SyncRejection::UnknownWorkspace)?;
        if !projection.contains_member(peer) {
            return Err(SyncRejection::NotMember);
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
        .map_err(fault!(SyncRejection::Grant))?;
        // The scope now exists, so a subscription that was waiting for history
        // access can proceed even when nothing was parked.
        crate::synchronization::record_subscription::publish_scope_exists(
            &state, workspace, package,
        );
        state.sync_wake.notify_one();
        // Records that arrived before the key can now be applied.
        let promoted = replication::promote_scope(&mut state, workspace, package)
            .map_err(fault!(SyncRejection::Storage))?;
        if crate::synchronization::is_reserved_record_scope(package) {
            person_name_sync::reconcile(&mut state, workspace)
                .map_err(fault!(SyncRejection::Storage))?;
        }
        Ok(promoted)
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
    ) -> Result<Vec<(u32, usize)>, SyncRejection> {
        let state = self.state.upgrade().ok_or(SyncRejection::RuntimeGone)?;
        let state = state.lock().map_err(|_| SyncRejection::RuntimeGone)?;
        if !state.memberships.get(&workspace).is_some_and(|projection| {
            projection.contains_member(PublicIdentity(state.secret_key.public()))
                && projection.contains_member(peer)
        }) {
            return Err(SyncRejection::NotMember);
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
            .map_err(fault!(SyncRejection::Storage))?;
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
            .map_err(fault!(SyncRejection::Storage))?;
        let mut sizes = Vec::new();
        for row in rows {
            let (sequence, bytes) = row.map_err(fault!(SyncRejection::Storage))?;
            sizes.push((
                u32::try_from(sequence).map_err(|_| SyncRejection::Storage)?,
                usize::try_from(bytes).map_err(|_| SyncRejection::Storage)?,
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
            crate::synchronization::note_record_loaded_for_test(scope);
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
            if crate::synchronization::is_reserved_record_scope(package) {
                // Semantic errors are isolated from valid transport and subsequent records.
                let _ = person_name_sync::reconcile(state, workspace);
            }
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
