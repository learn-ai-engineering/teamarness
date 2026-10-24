//! The immutable record log: append, read, subscribe, and the
//! recipient-bound protection grant a scope travels under.

use super::*;

impl CoreRuntime {
    /// Appends one authenticated, protected immutable record to the workspace journal.
    pub fn append_record(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        purpose: &str,
        canonical: &[u8],
    ) -> Result<crate::ImmutableRecord, crate::SynchronizationError> {
        self.append_record_as_principal(workspace_id, package_identity, purpose, canonical, None)
    }

    /// Appends as a Workspace Agent when a Principal selector is supplied.
    /// The Agent signs for any Package Instance on this Installation while it
    /// is current, this Installation stewards it, and at least one Package is
    /// bound to it (ADR 0030); otherwise `PrincipalNotBound`.
    pub fn append_record_as_principal(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        purpose: &str,
        canonical: &[u8],
        principal: Option<crate::PrincipalId>,
    ) -> Result<crate::ImmutableRecord, crate::SynchronizationError> {
        if crate::synchronization::is_reserved_record_scope(package_identity) {
            return Err(crate::SynchronizationError::InvalidInput);
        }
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
        let agent_key = if let Some(id) = principal {
            let projection = state
                .memberships
                .get(&workspace_id)
                .ok_or(crate::SynchronizationError::PrincipalNotBound)?;
            let registry = crate::principals::registry(projection);
            let (_, agent_identity, kind, steward, _) = registry
                .iter()
                .copied()
                .find(|(candidate, _, _, _, _)| *candidate == id)
                .ok_or(crate::SynchronizationError::PrincipalNotBound)?;
            let stewarded_here = registry.iter().any(|(person, key, _, _, _)| {
                Some(*person) == steward && *key == self.public_identity
            });
            if kind != crate::PrincipalKind::Agent || !stewarded_here {
                return Err(crate::SynchronizationError::PrincipalNotBound);
            }
            let bound =
                crate::principals::agent_keys::has_binding(&state.connection, workspace_id, id)
                    .map_err(crate::SynchronizationError::Storage)?;
            if !bound {
                return Err(crate::SynchronizationError::PrincipalNotBound);
            }
            let key = crate::principals::agent_keys::load(
                &state.connection,
                workspace_id,
                id,
                agent_identity,
            )
            .map_err(crate::SynchronizationError::Storage)?
            .ok_or(crate::SynchronizationError::PrincipalNotBound)?;
            Some(key)
        } else {
            None
        };
        let now = self.clock.now_unix_ms();
        let (record, delivery_sequence) = match crate::synchronization::append_with_signer(
            &mut state.connection,
            &secret_key,
            agent_key.as_ref().unwrap_or(&secret_key),
            workspace_id,
            package_identity,
            purpose,
            canonical,
            standing,
            is_creator,
            now,
        ) {
            Err(crate::SynchronizationError::ContentKeyUnavailable) => {
                if crate::synchronization::declare_member_key_demand(
                    &state.connection,
                    workspace_id,
                    package_identity,
                )? {
                    self.sync_worker.wake();
                }
                return Err(crate::SynchronizationError::ContentKeyUnavailable);
            }
            other => other?,
        };
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
        self.sync_worker.wake();
        Ok(imported)
    }

    /// Opens sealed records in request order after authenticating their scope and contents.
    pub fn open_records(
        &self,
        workspace_id: WorkspaceId,
        package_identity: &str,
        sealed_records: &[Vec<u8>],
    ) -> Result<Vec<crate::OpenedRecord>, crate::SynchronizationError> {
        if crate::synchronization::is_reserved_record_scope(package_identity) {
            return Err(crate::SynchronizationError::InvalidInput);
        }
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
    /// of committed records. A current creator's subscription initializes an
    /// empty scope; a Member without its key waits for history access.
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
        if crate::synchronization::is_reserved_record_scope(package_identity) {
            return Err(crate::SynchronizationError::InvalidInput);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::SynchronizationError::RuntimeUnavailable)?;
        if state.poisoned {
            return Err(crate::SynchronizationError::RuntimeUnavailable);
        }
        let scope = crate::synchronization::scope_digest(package_identity, workspace_id);
        let journal = records::journal_of(&state.connection, scope)?;
        let position = records::validate_cursor(&state.connection, scope, journal, after)?;
        let is_current_creator = state
            .memberships
            .get(&workspace_id)
            .is_some_and(|projection| {
                projection.creator() == self.public_identity
                    && projection.contains_member(self.public_identity)
            });
        if is_current_creator {
            let secret = state.secret_key.clone();
            if crate::synchronization::declare_creator_scope(
                &mut state.connection,
                &secret,
                workspace_id,
                package_identity,
            )? {
                records::publish_scope_exists(&state, workspace_id, package_identity);
                self.sync_worker.wake();
            }
        } else if state
            .memberships
            .get(&workspace_id)
            .is_some_and(|projection| projection.contains_member(self.public_identity))
            && crate::synchronization::declare_member_key_demand(
                &state.connection,
                workspace_id,
                package_identity,
            )?
        {
            self.sync_worker.wake();
        }
        let journal = records::journal_of(&state.connection, scope)?;
        let head = records::scope_head(&state, scope);
        let initial_high_water = after.is_none().then(|| head.subscribe().borrow().sequence);
        let initial_gap_free = journal.is_some();
        Ok(records::RecordSubscription::new(
            Arc::downgrade(&self.state),
            workspace_id,
            package_identity.to_owned(),
            scope,
            journal,
            position,
            initial_high_water,
            initial_gap_free,
            head,
            self.public_identity,
            Arc::clone(&self.health),
        ))
    }
}
