//! Observation and fault-injection seams.
//!
//! Every item here exists for a test. Production never reaches any of them:
//! each is `#[cfg(test)]`, or gated on the `test-hooks` feature that only a
//! `[dev-dependencies]` entry may enable. A demoted production method never
//! reappears here — `scripts/check-core-module-graph.mjs` holds that.

use super::*;

/// What a raw package-synchronization probe observed.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    Refused(crate::synchronization::Refusal),
    Welcomed { operations_received: usize },
    Failed(String),
}

/// Serves this Installation's real operations plus a fixed set of extras.
///
/// Only the sending side is altered: the peer applies everything through its
/// own unmodified gate and classification.
#[cfg(test)]
struct InjectingReceiver {
    inner: RuntimeSyncReceiver,
    extra: Vec<([u8; 32], p2panda_core::Operation<()>)>,
}

#[cfg(test)]
impl InjectingReceiver {
    fn extra_operations(&self) -> impl Iterator<Item = &p2panda_core::Operation<()>> {
        self.extra.iter().map(|(_, operation)| operation)
    }

    fn extra_for(
        &self,
        author: PublicIdentity,
        after: Option<u32>,
        until: Option<u32>,
    ) -> Vec<p2panda_core::Operation<()>> {
        self.extra_operations()
            .filter(|operation| {
                *operation.header.verifying_key.as_bytes() == *author.as_bytes()
                    && after.is_none_or(|value| operation.header.seq_num > value)
                    && until.is_none_or(|value| operation.header.seq_num <= value)
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
impl crate::synchronization::SyncReceiver for InjectingReceiver {
    fn authorize_session(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        self.inner.authorize_session(peer, workspace, package)
    }

    fn protection_bundle_for(
        &self,
        workspace: WorkspaceId,
        package: &str,
        recipient: PublicIdentity,
    ) -> Result<
        Option<crate::synchronization::ProtectionBundle>,
        crate::synchronization::SyncRejection,
    > {
        self.inner
            .protection_bundle_for(workspace, package, recipient)
    }

    fn import_protection_bundle(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, crate::synchronization::SyncRejection> {
        self.inner
            .import_protection_bundle(peer, workspace, package, bundle)
    }

    fn session_seed(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
    ) -> Result<crate::synchronization::SessionSeed, crate::synchronization::Refusal> {
        let mut seed = self.inner.session_seed(peer, workspace, package)?;
        for operation in self.extra_operations() {
            let Some(author) =
                crate::synchronization::key_bridge::identity(&operation.header.verifying_key)
            else {
                continue;
            };
            seed.authors.push(author);
            let height = seed
                .heights
                .entry(author)
                .or_insert(operation.header.seq_num);
            *height = (*height).max(operation.header.seq_num);
        }
        seed.authors.sort_unstable();
        seed.authors.dedup();
        Ok(seed)
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
    ) -> Result<Vec<(u32, usize)>, crate::synchronization::SyncRejection> {
        let mut sizes = self
            .inner
            .range_sizes(peer, workspace, package, author, after, until, limit)?;
        for operation in self.extra_for(author, after, until) {
            let size = operation.header.encode().len()
                + operation
                    .body
                    .as_ref()
                    .map_or(0, |body| body.to_bytes().len());
            sizes.push((operation.header.seq_num, size));
        }
        sizes.sort_unstable_by_key(|(sequence, _)| *sequence);
        sizes.dedup_by_key(|(sequence, _)| *sequence);
        sizes.truncate(limit);
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
        let mut page = self
            .inner
            .load_range_page(peer, workspace, package, author, from, until, rows)?;
        // An injected operation replaces the stored one at its sequence, which
        // is how a test delivers a forged record beside genuine ones.
        for operation in self.extra_for(author, from.checked_sub(1), Some(until)) {
            page.retain(|(stored, _)| stored.header.seq_num != operation.header.seq_num);
            let header = operation.header.encode();
            page.push((operation, header));
        }
        page.sort_unstable_by_key(|(operation, _)| operation.header.seq_num);
        page.truncate(rows);
        Ok(page)
    }

    fn receive_operation(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        package: &str,
        operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        self.inner
            .receive_operation(peer, workspace, package, operation)
    }
}

/// A probe's Core seam: it authorizes its own dial, offers nothing, and counts
/// what the peer sent. It is a fake only for the probe's own side.
#[cfg(test)]
struct ProbeReceiver;

#[cfg(test)]
impl crate::synchronization::SyncReceiver for ProbeReceiver {
    fn authorize_session(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
    ) -> Result<crate::synchronization::SessionGrant, crate::synchronization::Refusal> {
        Ok(crate::synchronization::SessionGrant {
            membership_version: [0; 32],
            content_key_id: None,
        })
    }

    fn protection_bundle_for(
        &self,
        _workspace: WorkspaceId,
        _package: &str,
        _recipient: PublicIdentity,
    ) -> Result<
        Option<crate::synchronization::ProtectionBundle>,
        crate::synchronization::SyncRejection,
    > {
        Ok(None)
    }

    fn import_protection_bundle(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, crate::synchronization::SyncRejection> {
        Ok(0)
    }

    fn session_seed(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
    ) -> Result<crate::synchronization::SessionSeed, crate::synchronization::Refusal> {
        Ok(crate::synchronization::SessionSeed::default())
    }

    fn range_sizes(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _author: PublicIdentity,
        _after: Option<u32>,
        _until: Option<u32>,
        _limit: usize,
    ) -> Result<Vec<(u32, usize)>, crate::synchronization::SyncRejection> {
        Ok(Vec::new())
    }

    fn load_range_page(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _author: PublicIdentity,
        _from: u32,
        _until: u32,
        _rows: usize,
    ) -> Result<Vec<crate::synchronization::ReplicationEntry>, crate::synchronization::RangeLoadError>
    {
        Ok(Vec::new())
    }

    fn receive_operation(
        &self,
        _peer: PublicIdentity,
        _workspace: WorkspaceId,
        _package: &str,
        _operation: p2panda_core::Operation<()>,
    ) -> Result<crate::synchronization::OperationOutcome, crate::synchronization::FatalSyncError>
    {
        Ok(crate::synchronization::OperationOutcome::Applied)
    }
}

/// Holds the runtime state lock on a separate thread until released.
///
/// A test uses this to make "Core is busy" observable; production never
/// constructs one.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub struct RuntimeStateHold {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl RuntimeStateHold {
    pub fn release(mut self) {
        self.release.take().unwrap().send(()).unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for RuntimeStateHold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
impl CoreRuntimeConfig {
    /// Replaces the synchronization bounds this Installation runs sessions
    /// under.
    pub(crate) fn with_session_limits_for_test(
        mut self,
        limits: crate::synchronization::limits::SessionLimits,
    ) -> Self {
        self.session_limits = limits;
        self
    }
}

#[cfg(test)]
impl RuntimeState {
    /// Builds a runtime state with no networking, for scheduler driver tests.
    ///
    /// Storage and the membership projection are the real ones; only the
    /// Endpoint and the session runner are absent.
    pub(crate) fn for_scheduler_tests(
        connection: rusqlite::Connection,
        secret_key: iroh_base::SecretKey,
        memberships: BTreeMap<WorkspaceId, crate::membership::WorkspaceProjection>,
    ) -> Self {
        Self {
            connection,
            clock: Arc::new(SystemClock),
            peer_sessions: crate::synchronization::PeerSessions::new(),
            sync_wake: Arc::new(tokio::sync::Notify::new()),
            secret_key,
            display_name: crate::identity::DEFAULT_DISPLAY_NAME.to_owned(),
            memberships,
            subscribers: BTreeMap::new(),
            record_heads: Arc::default(),
            membership_epochs: BTreeMap::new(),
            next_subscriber_id: 0,
            poisoned: false,
        }
    }

    /// Replaces the membership projection of one Workspace, as a revocation
    /// that arrived from elsewhere would.
    pub(crate) fn replace_membership_for_test(
        &mut self,
        workspace: WorkspaceId,
        projection: Option<crate::membership::WorkspaceProjection>,
    ) {
        match projection {
            Some(projection) => {
                self.memberships.insert(workspace, projection);
            }
            None => {
                self.memberships.remove(&workspace);
            }
        }
    }
}

impl CoreRuntime {
    /// How many inbound package synchronization sessions hold a slot.
    #[cfg(test)]
    pub(crate) fn inbound_package_sessions_for_test(&self) -> usize {
        self.log_sync.active_for_test()
    }

    /// How many records one scope's delivery journal holds.
    ///
    /// Counting rows is cheap; a test that polls until convergence must not
    /// decrypt the whole journal on every pass.
    #[cfg(test)]
    pub(crate) fn committed_record_count_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
    ) -> usize {
        let state = self.state.lock().expect("test runtime state");
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM record_delivery WHERE scope_id=?1",
                [scope.as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| usize::try_from(count).unwrap_or(0))
            .unwrap_or(0)
    }

    /// Opens a package synchronization connection and stops before the Hello,
    /// or reports that the responder refused the stream.
    #[cfg(test)]
    pub(crate) async fn try_hold_package_sync_before_hello_for_test(
        &self,
        address: iroh_base::EndpointAddr,
    ) -> Option<(iroh::endpoint::Connection, iroh::endpoint::SendStream)> {
        let connection = self
            .transport
            .endpoint()
            .connect(
                address,
                crate::synchronization::network_handler::LOG_SYNC_ALPN,
            )
            .await
            .ok()?;
        let (mut send, _receive) = connection.open_bi().await.ok()?;
        send.write_all(&64_u32.to_be_bytes()).await.ok()?;
        Some((connection, send))
    }

    /// How many inbound sessions one peer has ever been given a slot for.
    #[cfg(test)]
    pub(crate) fn inbound_sessions_from_for_test(&self, peer: PublicIdentity) -> usize {
        self.log_sync.accepted_from_for_test(peer)
    }

    /// The last record-session reports this Installation ran, newest last.
    #[cfg(test)]
    pub(crate) fn session_reports_for_test(
        &self,
    ) -> Vec<(PublicIdentity, crate::synchronization::ExchangeReport)> {
        self.session_reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// What the restart that opened this runtime validated and isolated.
    #[cfg(test)]
    pub(crate) fn recovery_report_for_test(&self) -> crate::synchronization::RecoveryReport {
        self.recovery_report
    }

    #[cfg(test)]
    pub(crate) fn create_workspace_invitation_for_test(
        &self,
        workspace_id: WorkspaceId,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        let token = self
            .issue_invitation_at(workspace_id, issued_at_unix_ms, expires_at_unix_ms)
            .map_err(map_invitation_onboarding_error)?;
        crate::WorkspaceInvitation::from_parts(
            &token
                .verified()
                .map_err(|_| crate::OnboardingError::Authentication)?,
            self.transport.endpoint_address(),
        )
        .map_err(|_| crate::OnboardingError::InvalidInput)
    }

    #[cfg(test)]
    pub(crate) fn create_claim_for_invitation_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
    ) -> Result<InvitationClaim, crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let claim = crate::invitation::sign_claim(&state.secret_key, &invitation.token)
            .map_err(map_invitation_onboarding_error)?;
        Ok(InvitationClaim::from_verified(&claim))
    }

    #[cfg(test)]
    pub(crate) fn export_current_bootstrap_for_test(
        &self,
        workspace_id: WorkspaceId,
        admission_id: MembershipOperationId,
    ) -> Result<crate::onboarding::wire::SuccessResponse, crate::OnboardingError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::OnboardingError::LocalConflict)?;
        let frame = crate::membership::projection::encode_current_bootstrap_with_limits(
            &state.connection,
            projection,
            admission_id,
            crate::membership::projection::BootstrapLimits::CURRENT,
        )
        .map_err(|_| crate::OnboardingError::InvalidBootstrap)?;
        crate::onboarding::wire::decode_success(&frame)
            .map_err(|_| crate::OnboardingError::InvalidBootstrap)
    }

    #[cfg(test)]
    pub(crate) fn import_response_with_deadline_delay_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        response: crate::onboarding::wire::SuccessResponse,
        boundary: crate::membership::projection::ImportBoundary,
        delay: std::time::Duration,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let claim = {
            let state = self
                .state
                .lock()
                .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
            crate::invitation::sign_claim(&state.secret_key, &invitation.token)
                .map_err(map_invitation_onboarding_error)?
        };
        let bootstrap = crate::membership::projection::validate_bootstrap(
            response,
            &invitation.token,
            &claim,
            self.public_identity,
        )?;
        let deadline = std::time::Instant::now()
            .checked_add(budget)
            .ok_or(crate::OnboardingError::Timeout)?;
        self.import_validated_bootstrap(&bootstrap, Some(deadline), |current| {
            if current == boundary {
                std::thread::sleep(delay);
            }
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn import_response_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        response: crate::onboarding::wire::SuccessResponse,
        boundary: Option<crate::membership::projection::ImportBoundary>,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        let invitation = invitation
            .verified()
            .map_err(|_| crate::OnboardingError::InvalidInput)?;
        let claim = {
            let state = self
                .state
                .lock()
                .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
            crate::invitation::sign_claim(&state.secret_key, &invitation.token)
                .map_err(map_invitation_onboarding_error)?
        };
        let bootstrap = crate::membership::projection::validate_bootstrap(
            response,
            &invitation.token,
            &claim,
            self.public_identity,
        )?;
        self.import_validated_bootstrap(&bootstrap, None, |current| {
            if boundary == Some(current) {
                Err(
                    if current == crate::membership::projection::ImportBoundary::AfterCommit {
                        StorageError::UncertainOutcome
                    } else {
                        StorageError::Database
                    },
                )
            } else {
                Ok(())
            }
        })
    }

    /// Creates the welcome bundle a current recipient needs for one scope.
    ///
    /// The grant is HPKE-sealed to `recipient` and signed by this Installation,
    /// so only the recipient's private key recovers the content key; the origin
    /// is the Workspace creator's `create` control, which authorizes the key.
    #[cfg(test)]
    pub(crate) fn export_protection_bundle_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        recipient: PublicIdentity,
    ) -> Result<(Vec<u8>, Vec<u8>), crate::SynchronizationError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::SynchronizationError::WorkspaceNotFound)?;
        if !projection.contains_member(self.public_identity)
            || !projection.contains_member(recipient)
        {
            return Err(crate::SynchronizationError::NotCurrentMember);
        }
        let admission = *projection.snapshot().version().as_bytes();
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        let origin: Vec<u8> = state
            .connection
            .query_row(
                "SELECT control_bytes FROM protection_control_operations
                 WHERE scope_id=?1 AND operation_kind='create' LIMIT 1",
                [scope.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| crate::SynchronizationError::Storage(StorageError::Database))?
            .ok_or(crate::SynchronizationError::ContentKeyUnavailable)?;
        let secret_key = state.secret_key.clone();
        let grant = crate::synchronization::export_protection_grant(
            &mut state.connection,
            &secret_key,
            workspace_id,
            package_identity,
            recipient,
            admission,
        )?;
        Ok((origin, grant))
    }

    /// Runs one outbound session, retrying while the peer is already running
    /// as many sessions with this Installation as it allows.
    ///
    /// The scheduler dials on its own, so an explicit session in a test can
    /// cross one of its own; the deadline only bounds failure.
    #[cfg(test)]
    pub(super) async fn replicate_records_to_address_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        address: iroh_base::EndpointAddr,
        peer: PublicIdentity,
    ) -> Result<crate::synchronization::ExchangeReport, crate::synchronization::LogSessionError>
    {
        let expiry = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let result = self
                .replicate_records_to_address_once_for_test(
                    workspace_id,
                    package_identity,
                    address.clone(),
                    peer,
                )
                .await;
            // An explicit session competes with the responder's two inbound
            // slots per peer. Losing that race is not the behaviour any caller
            // is testing, so retry until the deadline. `Cancelled` is NOT
            // retried: `local_revocation_cancels_a_live_session` asserts it, and
            // a caller that must not race the scheduler pauses it instead.
            if !matches!(
                result,
                Err(crate::synchronization::LogSessionError::PeerBusy)
            ) || std::time::Instant::now() >= expiry
            {
                return result;
            }
            // negative-wait: backoff between retries; the peer offers no event
            // saying it has freed a slot.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[cfg(test)]
    pub(super) async fn replicate_records_to_address_once_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        address: iroh_base::EndpointAddr,
        peer: PublicIdentity,
    ) -> Result<crate::synchronization::ExchangeReport, crate::synchronization::LogSessionError>
    {
        let local = self.public_identity;
        let state = Arc::downgrade(&self.state);
        let current = crate::blocking::run_with_state(&state, move |state| {
            state
                .memberships
                .get(&workspace_id)
                .is_some_and(|projection| projection.contains_member(local))
        })
        .await
        .unwrap_or(false);
        if !current {
            return Err(crate::synchronization::LogSessionError::Refused(
                crate::synchronization::Refusal::NotAuthorized,
            ));
        }
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> =
            Arc::new(RuntimeSyncReceiver {
                state: Arc::downgrade(&self.state),
                clock: Arc::clone(&self.clock),
                demand: Arc::clone(&self.membership_demand),
                health: Arc::clone(&self.health),
            });
        crate::synchronization::client::send_log_sync(
            &self.network,
            &self.sync_services,
            address,
            receiver,
            workspace_id,
            package_identity.to_owned(),
            peer,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn open_for_onboarding_test(
        config: CoreRuntimeConfig,
    ) -> Result<Self, CoreError> {
        Self::open_with_clock(config, Box::new(SystemClock)).await
    }

    #[cfg(test)]
    pub(crate) fn drop_next_onboarding_response_for_test(&self) {
        self.transport.handler().drop_next_response();
    }

    #[cfg(test)]
    pub(crate) fn endpoint_address_for_test(&self) -> iroh_base::EndpointAddr {
        self.transport.endpoint_address()
    }

    /// Teaches this loopback runtime how to reach a peer it has never dialled.
    #[cfg(test)]
    pub(crate) fn remember_peer_address_for_test(&self, address: iroh_base::EndpointAddr) {
        self.transport.remember_peer_address(address);
    }

    /// Every accepted membership operation identifier this Installation holds.
    #[cfg(test)]
    pub(crate) fn membership_operation_ids_for_test(
        &self,
        workspace_id: WorkspaceId,
    ) -> BTreeSet<MembershipOperationId> {
        let state = self.state.lock().expect("test runtime state");
        state
            .memberships
            .get(&workspace_id)
            .map(|projection| {
                crate::membership::replication::operation_ids(projection)
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stops this Installation replicating membership in either direction.
    ///
    /// Every replica call reports the runtime as unavailable, which is what an
    /// Installation that cannot currently answer looks like on the wire.
    #[cfg(test)]
    pub(crate) fn pause_membership_replication_for_test(&self, paused: bool) {
        self.membership_paused
            .store(paused, std::sync::atomic::Ordering::Release);
    }

    /// What the scheduler currently knows about each peer.
    #[cfg(test)]
    pub(crate) fn scheduler_snapshot_for_test(
        &self,
    ) -> Vec<crate::synchronization::scheduler::PeerSnapshot> {
        self.sync_worker.snapshot_for_test()
    }

    /// Stops and resumes synchronization scheduling.
    #[cfg(test)]
    pub(crate) fn pause_sync_scheduler_for_test(&self, paused: bool) {
        self.sync_worker.pause_for_test(paused);
    }

    /// Isolates one stored record, as a failed read would.
    #[cfg(test)]
    pub(crate) fn quarantine_record_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        record_id: crate::RecordId,
    ) {
        let state = self.state.lock().expect("test runtime state");
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        crate::synchronization::quarantine::quarantine_record(
            &state.connection,
            scope,
            record_id.as_bytes(),
            crate::synchronization::quarantine::RecordFault::Open,
            self.clock.now_unix_ms(),
        )
        .expect("the record is isolated");
    }

    /// Whether a scope has been isolated by a local integrity failure.
    #[cfg(test)]
    pub(crate) fn scope_is_quarantined_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
    ) -> bool {
        let state = self.state.lock().expect("test runtime state");
        crate::synchronization::quarantine::is_scope_quarantined(
            &state.connection,
            crate::synchronization::scope_digest(package_identity, workspace_id),
        )
        .expect("the quarantine table is readable")
    }

    /// Breaks one projection row's payload hash, as on-disk tampering would.
    #[cfg(test)]
    pub(crate) fn tamper_payload_hash_for_test(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
    ) {
        let state = self.state.lock().expect("test runtime state");
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        state
            .connection
            .execute_batch("DROP TRIGGER IF EXISTS p2panda_operations_no_update;")
            .expect("the append-only guard is dropped for this test");
        state
            .connection
            .execute(
                "UPDATE p2panda_operations SET payload_hash = zeroblob(32)
                 WHERE scope_id = ?1 AND author_sequence = (
                     SELECT MAX(author_sequence) FROM p2panda_operations WHERE scope_id = ?1)",
                [scope.as_slice()],
            )
            .expect("the projection row is rewritten");
    }

    /// Runs one real membership session against `address`, as the worker would.
    #[cfg(test)]
    pub(crate) async fn membership_session_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        peer: PublicIdentity,
    ) -> Result<
        crate::membership_sync::MembershipSessionReport,
        crate::membership_sync::MembershipSessionError,
    > {
        crate::membership_sync::run_membership_session_on(
            &self.network,
            address,
            Arc::clone(&self.membership_replica),
            workspace_id,
            peer,
        )
        .await
    }

    /// Dials the membership protocol pushing nothing, and reports what the
    /// responder served and whether it refused.
    #[cfg(test)]
    pub(crate) async fn probe_membership_sync_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        operation_ids: Vec<[u8; 32]>,
    ) -> (Vec<Vec<u8>>, bool) {
        crate::membership_sync::session::probe_membership_sync(
            self.transport.endpoint(),
            address,
            workspace_id,
            operation_ids,
        )
        .await
    }

    /// How many operations one scope currently holds parked.
    #[cfg(test)]
    pub(crate) fn parked_count_for_test(&self, workspace_id: WorkspaceId, package: &str) -> usize {
        let state = self.state.lock().expect("test runtime state");
        crate::synchronization::parking::parked_count(&state.connection, workspace_id, package)
            .expect("a parked count")
    }

    /// Runs one synchronization pass now instead of waiting for the idle tick.
    #[cfg(test)]
    pub(crate) fn sync_now_for_test(&self) {
        self.sync_worker.sweep_for_test();
    }

    /// Dials a peer's package synchronization ALPN as this Installation and
    /// reports whether the peer's gate served it anything.
    #[cfg(test)]
    pub(crate) async fn probe_package_sync_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
    ) -> ProbeOutcome {
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> = Arc::new(ProbeReceiver);
        let sessions = Arc::clone(&self.state.lock().expect("test runtime state").peer_sessions);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::synchronization::network_handler::outbound_log_session(
                self.transport.endpoint(),
                address,
                receiver,
                sessions,
                Arc::clone(&self.health),
                workspace_id,
                package_identity.to_owned(),
                peer,
                self.session_limits,
            ),
        )
        .await;
        match result {
            Ok(Ok(report)) => ProbeOutcome::Welcomed {
                operations_received: report.outcomes.applied,
            },
            Ok(Err(crate::synchronization::LogSessionError::Refused(refusal))) => {
                ProbeOutcome::Refused(refusal)
            }
            Ok(Err(error)) => ProbeOutcome::Failed(error.to_string()),
            Err(_) => ProbeOutcome::Failed("probe timed out".to_owned()),
        }
    }

    /// Runs one real outbound session that also pushes `extra` operations.
    ///
    /// The extra operations join this Installation's own, so a test can deliver
    /// a forged record beside genuine ones in a single session.
    #[cfg(test)]
    pub(crate) async fn deliver_operations_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
        extra: Vec<p2panda_core::Operation<()>>,
    ) -> Result<crate::synchronization::ExchangeReport, crate::synchronization::LogSessionError>
    {
        let log_id = crate::synchronization::operation_log_id(
            crate::synchronization::scope_digest(package_identity, workspace_id),
        );
        let sessions = Arc::clone(&self.state.lock().expect("test runtime state").peer_sessions);
        let receiver: Arc<dyn crate::synchronization::SyncReceiver> = Arc::new(InjectingReceiver {
            inner: RuntimeSyncReceiver {
                state: Arc::downgrade(&self.state),
                clock: Arc::clone(&self.clock),
                demand: Arc::clone(&self.membership_demand),
                health: Arc::clone(&self.health),
            },
            extra: extra
                .into_iter()
                .map(|operation| (log_id, operation))
                .collect(),
        });
        crate::synchronization::network_handler::outbound_log_session(
            self.transport.endpoint(),
            address,
            receiver,
            sessions,
            Arc::clone(&self.health),
            workspace_id,
            package_identity.to_owned(),
            peer,
            self.session_limits,
        )
        .await
    }

    /// Opens a package synchronization session and then stops talking.
    ///
    /// The responder is left waiting for the rest of the first frame, which is
    /// where a test can observe what ends an inbound session that is doing
    /// nothing wrong. Returns the application close code the responder used.
    #[cfg(test)]
    pub(crate) async fn stalled_package_sync_close_code_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        wait: std::time::Duration,
    ) -> Option<u64> {
        let connection = self
            .transport
            .endpoint()
            .connect(
                address,
                crate::synchronization::network_handler::LOG_SYNC_ALPN,
            )
            .await
            .ok()?;
        let (mut send, _receive) = connection.open_bi().await.ok()?;
        // A length header with no body: the stream exists, so the responder
        // accepts it, and then waits for bytes that never arrive.
        send.write_all(&64_u32.to_be_bytes()).await.ok()?;
        let closed = tokio::time::timeout(wait, connection.closed()).await.ok()?;
        match closed {
            iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
                Some(u64::from(close.error_code))
            }
            _ => None,
        }
    }

    /// Opens a package synchronization stream and then waits, holding the
    /// responder's inbound slot until the connection is dropped or its Hello
    /// deadline passes.
    #[cfg(test)]
    pub(crate) async fn hold_package_sync_before_hello_for_test(
        &self,
        address: iroh_base::EndpointAddr,
    ) -> (iroh::endpoint::Connection, iroh::endpoint::SendStream) {
        let connection = self
            .transport
            .endpoint()
            .connect(
                address,
                crate::synchronization::network_handler::LOG_SYNC_ALPN,
            )
            .await
            .expect("the responder accepts the connection");
        let (mut send, _receive) = connection
            .open_bi()
            .await
            .expect("the responder accepts the stream");
        // A length header with no body: the responder now waits for a Hello.
        send.write_all(&64_u32.to_be_bytes())
            .await
            .expect("the header is written");
        // The stream is returned with the connection: dropping it would reset
        // the stream and release the responder's slot.
        (connection, send)
    }

    /// Connects on the package synchronization ALPN and reports the close code
    /// the responder used, without sending anything.
    #[cfg(test)]
    pub(crate) async fn package_sync_close_code_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        wait: std::time::Duration,
    ) -> Option<u64> {
        let connection = self
            .transport
            .endpoint()
            .connect(
                address,
                crate::synchronization::network_handler::LOG_SYNC_ALPN,
            )
            .await
            .ok()?;
        match tokio::time::timeout(wait, connection.closed()).await.ok()? {
            iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
                Some(u64::from(close.error_code))
            }
            _ => None,
        }
    }

    /// Reports whether a peer's Router still offers `alpn` at all.
    #[cfg(test)]
    pub(crate) async fn can_connect_alpn_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        alpn: &[u8],
    ) -> bool {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.transport.endpoint().connect(address, alpn),
        )
        .await
        .is_ok_and(|connection| connection.is_ok())
    }

    #[cfg(test)]
    pub(crate) fn timeout_next_join_for_test(&self) {
        self.transport.force_next_join_timeout();
    }

    #[cfg(test)]
    pub(crate) fn invalidate_next_bootstrap_for_test(&self) {
        self.transport.force_next_invalid_bootstrap();
    }

    #[cfg(test)]
    pub(crate) fn pause_next_join_after_response_for_test(&self) {
        self.transport.force_next_post_response_pause();
    }

    #[cfg(test)]
    pub(crate) fn hold_next_join_after_response_for_test(&self) {
        self.transport.hold_next_join_after_response();
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_join_held_after_response_for_test(&self) {
        self.transport.wait_for_post_response_hold().await;
    }

    #[cfg(test)]
    pub(crate) fn release_join_held_after_response_for_test(&self) {
        self.transport.release_post_response_hold();
    }

    #[cfg(test)]
    pub(crate) fn block_next_join_after_response_for_test(&self, duration: std::time::Duration) {
        self.transport.force_next_post_response_block(duration);
    }

    #[cfg(test)]
    pub(crate) async fn join_workspace_with_budget_for_test(
        &self,
        invitation: &crate::WorkspaceInvitation,
        budget: std::time::Duration,
    ) -> Result<(WorkspaceSummary, MembershipSnapshot), crate::OnboardingError> {
        self.join_workspace_with_budget(invitation, budget).await
    }

    #[cfg(test)]
    pub(crate) fn hold_runtime_state_for_test(&self) -> RuntimeStateHold {
        self.hold_state_inner()
    }

    /// Stops and resumes synchronization scheduling from another crate.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn pause_sync_scheduler_for_tests(&self, paused: bool) {
        self.sync_worker.pause_for_test(paused);
    }

    /// Admits a freshly generated Installation through the ordinary admission
    /// path, without networking, so a test has a Member that is never reachable.
    ///
    /// # Panics
    ///
    /// Panics when the Workspace is not one this Installation manages.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn admit_unreachable_member_for_tests(&self, workspace_id: WorkspaceId) -> PublicIdentity {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("a test identity is generated");
        let secret = SecretKey::from_bytes(&bytes);
        let member = PublicIdentity::from_bytes(*secret.public().as_bytes())
            .expect("a generated identity is valid");
        let token = self
            .issue_invitation(
                workspace_id,
                self.clock.now_unix_ms().saturating_add(600_000),
            )
            .expect("the local Installation manages the workspace");
        let verified = token.verified().expect("a freshly issued token verifies");
        let claim = crate::invitation::sign_claim(&secret, &verified)
            .expect("the generated identity can claim");
        self.admit_and_export_with_limits_for_test(
            &InvitationClaim::from_verified(&claim),
            member,
            crate::membership::projection::BootstrapLimits::CURRENT,
            None,
        )
        .expect("the admission commits");
        member
    }

    /// Injects what the last session with `peer` saw, so a test can reach an
    /// availability row without waiting for a real session to fail.
    ///
    /// # Panics
    ///
    /// Panics on an outcome name that is not one of the four accepted values.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_peer_outcome_for_tests(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        peer: PublicIdentity,
        outcome: &str,
    ) {
        let reported = match outcome {
            "reachable" => Some(crate::synchronization::PeerOutcome::Reachable),
            "unreachable" => Some(crate::synchronization::PeerOutcome::Unreachable),
            "failed" => Some(crate::synchronization::PeerOutcome::Failed),
            "clear" => None,
            other => panic!("unknown peer outcome {other}"),
        };
        self.health.record_outcome(
            crate::synchronization::scope_digest(package_identity, workspace_id),
            peer,
            reported,
            None,
        );
    }

    /// Isolates one scope, as a local integrity failure would.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn quarantine_scope_for_tests(&self, workspace_id: WorkspaceId, package_identity: &str) {
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        {
            let state = self.state.lock().expect("test runtime state");
            crate::synchronization::quarantine::quarantine_scope(
                &state.connection,
                scope,
                crate::synchronization::quarantine::ScopeFault::Projection,
                self.clock.now_unix_ms(),
            )
            .expect("the scope is isolated");
        }
        self.health.scope_changed(scope);
    }

    /// Holds the runtime state lock until the returned value is released, so a
    /// test in another crate can observe the Host while Core is busy.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn hold_state_for_tests(&self) -> RuntimeStateHold {
        self.hold_state_inner()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn hold_state_inner(&self) -> RuntimeStateHold {
        let state = Arc::clone(&self.state);
        let (acquired_sender, acquired_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _state = state.lock().unwrap();
            acquired_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
        });
        acquired_receiver.recv().unwrap();
        RuntimeStateHold {
            release: Some(release_sender),
            thread: Some(thread),
        }
    }

    #[cfg(test)]
    pub(crate) async fn exhaust_outbound_joins_for_test(
        &self,
    ) -> Vec<crate::onboarding::transport::OutboundPermit> {
        let mut permits = Vec::new();
        for _ in 0..crate::onboarding::transport::MAX_OUTBOUND_JOINS {
            permits.push(self.transport.begin_outbound().await.expect("test permit"));
        }
        permits
    }

    #[cfg(test)]
    pub(crate) async fn hold_outbound_join_for_test(
        &self,
    ) -> Result<crate::onboarding::transport::OutboundPermit, crate::OnboardingError> {
        self.transport.begin_outbound().await
    }

    #[cfg(test)]
    pub(crate) fn onboarding_lifecycle_state_for_test(&self) -> u8 {
        self.transport.lifecycle_state()
    }

    #[cfg(test)]
    pub(crate) fn active_onboarding_operations_for_test(&self) -> usize {
        self.transport.active_operations()
    }

    #[cfg(test)]
    pub(crate) fn available_outbound_joins_for_test(&self) -> usize {
        self.transport.available_outbound_permits()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_shutdown_idle_waiter_for_test(&self) {
        self.transport.wait_for_idle_waiter().await;
    }

    #[cfg(test)]
    pub(crate) async fn send_raw_onboarding_frame_for_test(
        &self,
        address: iroh_base::EndpointAddr,
        frame: &[u8],
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        let permit = self.transport.begin_outbound().await?;
        let connection = self.transport.connect(address, &permit).await?;
        let (mut send, mut receive) = connection
            .open_bi()
            .await
            .map_err(|_| crate::OnboardingError::Authentication)?;
        send.write_all(frame)
            .await
            .map_err(|_| crate::OnboardingError::Authentication)?;
        send.finish()
            .map_err(|_| crate::OnboardingError::Authentication)?;
        receive
            .read_to_end(1_024 + 4)
            .await
            .map_err(|_| crate::OnboardingError::Overload)
    }

    #[cfg(test)]
    pub(crate) fn create_workspace_invitation_for_workspace_id_for_test(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<crate::WorkspaceInvitation, crate::OnboardingError> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::OnboardingError::UncertainStorageOutcome)?;
        let issued_at_unix_ms = self.clock.now_unix_ms();
        let token = crate::invitation::sign_token(
            &state.secret_key,
            workspace_id,
            issued_at_unix_ms,
            issued_at_unix_ms.saturating_add(60_000),
            SecretKey::generate().to_bytes(),
        )
        .map_err(map_invitation_onboarding_error)?;
        crate::WorkspaceInvitation::from_parts(&token, self.transport.endpoint_address())
            .map_err(|_| crate::OnboardingError::InvalidInput)
    }

    #[cfg(test)]
    pub(crate) fn admit_and_export_for_test(
        &self,
        claim: &InvitationClaim,
        authenticated_peer: PublicIdentity,
        boundary: Option<crate::membership::projection::RedemptionBoundary>,
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        self.admit_and_export_with_limits_for_test(
            claim,
            authenticated_peer,
            crate::membership::projection::BootstrapLimits::CURRENT,
            boundary,
        )
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn admit_and_export_with_limits_for_test(
        &self,
        claim: &InvitationClaim,
        authenticated_peer: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
        boundary: Option<crate::membership::projection::RedemptionBoundary>,
    ) -> Result<Vec<u8>, crate::OnboardingError> {
        let claim = claim.verified().map_err(map_invitation_onboarding_error)?;
        let mut state = self
            .lock_invitation_state()
            .map_err(map_invitation_onboarding_error)?;
        let now_unix_ms = self.clock.now_unix_ms();
        let RuntimeState {
            connection,
            secret_key,
            memberships,
            poisoned,
            ..
        } = &mut *state;
        let result = crate::membership::projection::redeem_invitation_with_fault_and_limits(
            connection,
            memberships,
            secret_key,
            claim,
            crate::membership::projection::AdmissionContext {
                authenticated_peer,
                now_unix_ms,
                limits,
            },
            |current| {
                if boundary == Some(current) {
                    Err(
                        if current == crate::membership::projection::RedemptionBoundary::AfterCommit
                        {
                            StorageError::UncertainOutcome
                        } else {
                            StorageError::Database
                        },
                    )
                } else {
                    Ok(())
                }
            },
        );
        match result {
            Ok(export) => Ok(export.response),
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

    #[cfg(test)]
    pub(super) fn redeem_invitation_with_test_fault(
        &self,
        claim: &InvitationClaim,
        boundary: crate::membership::projection::RedemptionBoundary,
    ) -> Result<MembershipTransition, InvitationError> {
        self.redeem_invitation_inner(claim, |current| {
            if current == boundary {
                Err(
                    if current == crate::membership::projection::RedemptionBoundary::AfterCommit {
                        StorageError::UncertainOutcome
                    } else {
                        StorageError::Database
                    },
                )
            } else {
                Ok(())
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn revoke_member_with_limits_for_test(
        &self,
        workspace_id: WorkspaceId,
        member: PublicIdentity,
        limits: crate::membership::projection::BootstrapLimits,
    ) -> Result<MembershipTransition, MembershipError> {
        self.revoke_member_with_limits(workspace_id, member, limits)
    }
}
