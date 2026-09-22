//! Invitations, claims, redemption, and joining a Workspace.

use super::*;
use crate::diag::fault;

impl CoreRuntime {
    /// Issues an expiring invitation as the local manager.
    #[allow(dead_code)]
    pub(crate) fn issue_invitation(
        &self,
        workspace_id: WorkspaceId,
        expires_at_unix_ms: i64,
    ) -> Result<InvitationToken, InvitationError> {
        self.issue_invitation_at(workspace_id, self.clock.now_unix_ms(), expires_at_unix_ms)
    }

    pub(super) fn issue_invitation_at(
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
            .map_err(fault!(crate::OnboardingError::Authentication))?;
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

    pub(super) async fn join_workspace_with_budget(
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
    pub(super) fn import_validated_bootstrap(
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

    pub(super) fn redeem_invitation_inner(
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

    pub(super) fn lock_invitation_state(
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
