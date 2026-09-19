//! Canonical membership: the snapshot, the transition stream, revocation,
//! and applying an envelope that arrived from elsewhere.

use super::*;

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

    pub(super) fn revoke_member_with_limits(
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

    pub(super) fn maintain_and_publish(
        &self,
        state: &mut RuntimeState,
        now_unix_ms: i64,
    ) -> Result<(), MembershipError> {
        replication::maintain_and_publish(state, now_unix_ms)
    }

    /// Returns `None` when the causing operation is a workspace `create`, which
    /// starts membership instead of transitioning it.
    pub(super) fn transition_from_applied(
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

    pub(super) fn publish_transition(state: &mut RuntimeState, transition: MembershipTransition) {
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
}
