//! Workspace creation and the local Workspace list.

use super::*;

impl CoreRuntime {
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
                person_name_sync::initialize(&mut state, summary.id())
                    .map_err(fault!(WorkspaceError::Storage(StorageError::Database)))?;
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
        let state = self.state.lock().map_err(fault!(WorkspaceError::Storage(
            StorageError::RuntimePoisoned
        )))?;
        if state.poisoned {
            return Err(WorkspaceError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }
}

impl CoreRuntime {
    /// Creates a Workspace and durably names its initial Person before reporting success.
    pub fn create_workspace_with_person_name(
        &self,
        workspace_name: &str,
        person_name: &str,
    ) -> Result<WorkspaceSummary, crate::PersonNameError> {
        crate::person_names::validate(person_name)?;
        crate::workspace::validate_name(workspace_name)
            .map_err(|_| crate::PersonNameError::InvalidName)?;
        let mut state = self
            .state
            .lock()
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned
            )))?;
        if state.poisoned {
            return Err(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned,
            ));
        }
        let secret_key = state.secret_key.clone();
        let created = crate::workspace::create_named(
            &mut state.connection,
            &secret_key,
            workspace_name,
            person_name,
            self.clock.now_unix_ms(),
        );
        let workspace = match created {
            Ok(workspace) => workspace,
            Err(error) => {
                if matches!(
                    error,
                    WorkspaceError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                return Err(match error {
                    WorkspaceError::Storage(storage) => crate::PersonNameError::Storage(storage),
                    _ => crate::PersonNameError::InvalidName,
                });
            }
        };
        match crate::membership::projection::recover(
            &mut state.connection,
            self.clock.now_unix_ms(),
        ) {
            Ok(memberships) => state.memberships = memberships,
            Err(error) => {
                if error == StorageError::UncertainOutcome {
                    state.poisoned = true;
                }
                return Err(crate::PersonNameError::Storage(error));
            }
        }
        person_name_sync::initialize(&mut state, workspace.id()).map_err(fault!(
            crate::PersonNameError::Storage(StorageError::Database)
        ))?;
        Ok(workspace)
    }

    /// Returns current presentation for a canonical member, or None for a nonmember.
    pub fn workspace_person(
        &self,
        workspace_id: WorkspaceId,
        identity: PublicIdentity,
    ) -> Result<Option<crate::WorkspacePerson>, crate::PersonNameError> {
        let state = self
            .state
            .lock()
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned
            )))?;
        if state.poisoned {
            return Err(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned,
            ));
        }
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        crate::person_names::lookup(&state.connection, projection, identity)
    }

    /// Atomically reads the durable name generation and current-member presentation.
    /// The generation is independent of membership epoch and record cursors.
    pub fn person_name_snapshot(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(u64, Vec<(PublicIdentity, crate::WorkspacePerson)>), crate::PersonNameError> {
        let state = self
            .state
            .lock()
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned
            )))?;
        if state.poisoned {
            return Err(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned,
            ));
        }
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        let generation =
            crate::person_names::edit::profile_generation(&state.connection, workspace_id)
                .map_err(fault!(crate::PersonNameError::Storage(
                    StorageError::Database
                )))?;
        let members = projection
            .snapshot()
            .members()
            .iter()
            .map(|member| {
                let identity = member.identity();
                crate::person_names::lookup(&state.connection, projection, identity)
                    .map(|person| (identity, person.expect("current member has an anchor")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((generation, members))
    }

    /// A separate wake for snapshot invalidation, including membership changes.
    /// Subscribe first, then read the snapshot; on a gap or restart, refetch it.
    pub fn subscribe_person_names(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<tokio::sync::watch::Receiver<u64>, crate::PersonNameError> {
        let mut state = self
            .state
            .lock()
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned
            )))?;
        if state.poisoned {
            return Err(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned,
            ));
        }
        if !state.memberships.contains_key(&workspace_id) {
            return Err(crate::PersonNameError::WorkspaceNotFound);
        }
        let generation =
            crate::person_names::edit::profile_generation(&state.connection, workspace_id)
                .map_err(fault!(crate::PersonNameError::Storage(
                    StorageError::Database
                )))?;
        Ok(state
            .profile_generations
            .entry(workspace_id)
            .or_insert_with(|| tokio::sync::watch::channel(generation).0)
            .subscribe())
    }

    /// Sets only this Installation's own current Workspace Person name.
    pub fn set_own_person_name(
        &self,
        workspace_id: WorkspaceId,
        name: &str,
    ) -> Result<(), crate::PersonNameError> {
        self.set_own_person_name_for(workspace_id, self.public_identity, name)
    }

    /// The supplied identity must match this runtime's Installation and its canonical member anchor.
    pub fn set_own_person_name_for(
        &self,
        workspace_id: WorkspaceId,
        identity: PublicIdentity,
        name: &str,
    ) -> Result<(), crate::PersonNameError> {
        crate::person_names::validate(name)?;
        if identity != self.public_identity {
            return Err(crate::PersonNameError::NotOwnPerson);
        }
        let mut state = self
            .state
            .lock()
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned
            )))?;
        if state.poisoned {
            return Err(crate::PersonNameError::Storage(
                StorageError::RuntimePoisoned,
            ));
        }
        let projection = state
            .memberships
            .get(&workspace_id)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?
            .clone();
        let result = crate::person_names::set(&mut state.connection, &projection, identity, name);
        if result
            == Err(crate::PersonNameError::Storage(
                StorageError::UncertainOutcome,
            ))
        {
            state.poisoned = true;
        }
        if result.is_ok() {
            // The name and generation have committed even if private replay fails.
            person_name_sync::publish_generation(&state, workspace_id);
            person_name_sync::initialize(&mut state, workspace_id).map_err(fault!(
                crate::PersonNameError::Storage(StorageError::Database)
            ))?;
        }
        result
    }
}
