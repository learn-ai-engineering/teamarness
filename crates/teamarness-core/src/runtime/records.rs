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
        let (record, delivery_sequence) = match crate::synchronization::append(
            &mut state.connection,
            &secret_key,
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
}
