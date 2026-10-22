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
                principal_sync::initialize(&mut state, summary.id())
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
        crate::principals::validate(person_name)?;
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
        principal_sync::initialize(&mut state, workspace.id()).map_err(fault!(
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
        crate::principals::lookup(&state.connection, projection, identity)
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
            crate::principals::edit::profile_generation(&state.connection, workspace_id).map_err(
                fault!(crate::PersonNameError::Storage(StorageError::Database)),
            )?;
        // Keep the Person-only view for existing Host/Shell consumers until
        // they switch to principals in their assigned phases.
        let members = projection
            .installation_members()
            .into_iter()
            .map(|identity| {
                crate::principals::lookup(&state.connection, projection, identity)
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
            crate::principals::edit::profile_generation(&state.connection, workspace_id).map_err(
                fault!(crate::PersonNameError::Storage(StorageError::Database)),
            )?;
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
        crate::principals::validate(name)?;
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
        let result = crate::principals::set(&mut state.connection, &projection, identity, name);
        if result
            == Err(crate::PersonNameError::Storage(
                StorageError::UncertainOutcome,
            ))
        {
            state.poisoned = true;
        }
        if result.is_ok() {
            // The name and generation have committed even if private replay fails.
            principal_sync::publish_generation(&state, workspace_id);
            principal_sync::initialize(&mut state, workspace_id).map_err(fault!(
                crate::PersonNameError::Storage(StorageError::Database)
            ))?;
        }
        result
    }
}

impl CoreRuntime {
    /// Adds a locally custodied Agent to the caller's admitted Person group.
    /// An uncertain commit poisons this runtime. Reopen and inspect the roster
    /// before choosing whether to issue a new add: names are non-unique, so a
    /// new name-only call cannot identify the outcome of an earlier attempt.
    pub fn add_agent(
        &self,
        workspace: WorkspaceId,
        name: &str,
    ) -> Result<crate::PrincipalId, crate::PersonNameError> {
        self.add_agent_inner(
            workspace,
            name,
            crate::membership::projection::BootstrapLimits::CURRENT,
            |_| Ok(()),
            false,
        )
    }

    fn add_agent_inner(
        &self,
        workspace: WorkspaceId,
        name: &str,
        limits: crate::membership::projection::BootstrapLimits,
        fault: impl FnMut(
            crate::membership::projection::LocalAgentAddBoundary,
        ) -> Result<(), StorageError>,
        defer_initialization: bool,
    ) -> Result<crate::PrincipalId, crate::PersonNameError> {
        crate::principals::validate(name)?;
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?
            .clone();
        let group = projection
            .person_group(self.public_identity)
            .ok_or(crate::PersonNameError::NotOwnPerson)?;
        let secret = iroh_base::SecretKey::generate();
        let agent = PublicIdentity::from_bytes(*secret.public().as_bytes()).map_err(fault!(
            crate::PersonNameError::Storage(StorageError::CorruptIdentity)
        ))?;
        let proof =
            crate::membership::AgentKeyProof::sign(&secret, workspace, group, self.public_identity)
                .map_err(crate::PersonNameError::Membership)?;
        let now_unix_ms = self.clock.now_unix_ms();
        let envelope = crate::membership::MembershipEnvelope::sign(
            &state.secret_key,
            workspace,
            now_unix_ms,
            crate::membership::MembershipAction::AddAgentKey {
                group,
                agent,
                proof,
            },
            projection.heads(),
        )
        .map_err(crate::PersonNameError::Membership)?;
        let id = crate::principals::derive_agent(workspace, envelope.id);
        let applied = {
            let RuntimeState {
                connection,
                memberships,
                ..
            } = &mut *state;
            crate::membership::projection::apply_local_agent_add(
                connection,
                memberships,
                envelope,
                now_unix_ms,
                crate::membership::projection::LocalAgentAdd {
                    id,
                    author: self.public_identity,
                    secret: &secret,
                    name,
                },
                limits,
                fault,
            )
        };
        match applied {
            Ok(applied) if applied.accepted => {
                #[cfg(any(test, feature = "test-hooks"))]
                if defer_initialization {
                    state.principal_sync_init_faults = 2;
                }
                #[cfg(not(any(test, feature = "test-hooks")))]
                let _ = defer_initialization;
                if let Some(transition) = Self::transition_from_applied(applied) {
                    replication::after_membership_accepted(&mut state, &transition);
                }
                principal_sync::publish_generation(&state, workspace);
                // A durable intent/wake recovers initialization independently of this add.
                if let Err(error) = principal_sync::initialize(&mut state, workspace) {
                    if matches!(
                        error,
                        crate::SynchronizationError::Storage(StorageError::UncertainOutcome)
                    ) {
                        state.poisoned = true;
                    }
                    state.sync_wake.notify_one();
                }
                self.sync_worker.wake();
                Ok(id)
            }
            Ok(_) => Err(crate::PersonNameError::Membership(
                MembershipError::AuthorizationRejected,
            )),
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(crate::PersonNameError::Membership(error))
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn add_agent_with_fault_for_tests(
        &self,
        workspace: WorkspaceId,
        name: &str,
        boundary: crate::runtime::AgentAddFaultForTests,
    ) -> Result<crate::PrincipalId, crate::PersonNameError> {
        use crate::membership::projection::{BootstrapLimits, LocalAgentAddBoundary};
        let limits = if boundary == crate::runtime::AgentAddFaultForTests::BootstrapBound {
            BootstrapLimits {
                max_operations: 2,
                ..BootstrapLimits::CURRENT
            }
        } else {
            BootstrapLimits::CURRENT
        };
        self.add_agent_inner(
            workspace,
            name,
            limits,
            |current| {
                let matched = matches!(
                    (boundary, current),
                    (
                        crate::runtime::AgentAddFaultForTests::BeforeName,
                        LocalAgentAddBoundary::BeforeName
                    ) | (
                        crate::runtime::AgentAddFaultForTests::BeforeCommit,
                        LocalAgentAddBoundary::BeforeCommit
                    ) | (
                        crate::runtime::AgentAddFaultForTests::AfterCommit,
                        LocalAgentAddBoundary::AfterCommit
                    )
                );
                if matched {
                    Err(StorageError::Database)
                } else {
                    Ok(())
                }
            },
            boundary == crate::runtime::AgentAddFaultForTests::PostCommitInitialization,
        )
    }

    /// Removes exactly one current Agent. A creator may remove a remote Agent
    /// without access to the Steward's Installation or Agent secret.
    pub fn remove_agent(
        &self,
        workspace: WorkspaceId,
        id: crate::PrincipalId,
    ) -> Result<(), crate::PersonNameError> {
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?
            .clone();
        let (_, agent, kind, steward, _) = crate::principals::registry(&projection)
            .into_iter()
            .find(|(candidate, _, _, _, _)| *candidate == id)
            .ok_or(crate::PersonNameError::AgentNotFound)?;
        if kind != crate::PrincipalKind::Agent {
            return Err(crate::PersonNameError::AgentNotFound);
        }
        let steward_id = steward.ok_or(crate::PersonNameError::AgentNotFound)?;
        let (group, _, _, _, _) = crate::principals::registry(&projection)
            .into_iter()
            .find(|(candidate, _, _, _, _)| *candidate == steward_id)
            .ok_or(crate::PersonNameError::AgentNotFound)?;
        let group = projection
            .principal_anchors()
            .into_iter()
            .find(|(anchor, _, _)| crate::principals::derive(workspace, *anchor) == group)
            .map(|(anchor, _, _)| anchor)
            .ok_or(crate::PersonNameError::AgentNotFound)?;
        if self.public_identity != projection.creator()
            && projection.person_group(self.public_identity) != Some(group)
        {
            return Err(crate::PersonNameError::NotOwnPerson);
        }
        let envelope = crate::membership::MembershipEnvelope::sign(
            &state.secret_key,
            workspace,
            self.clock.now_unix_ms(),
            crate::membership::MembershipAction::RemoveAgentKey { group, agent },
            projection.heads(),
        )
        .map_err(crate::PersonNameError::Membership)?;
        let applied = {
            let RuntimeState {
                connection,
                memberships,
                ..
            } = &mut *state;
            crate::membership::projection::apply_canonical(
                connection,
                memberships,
                envelope.canonical_bytes(),
                self.clock.now_unix_ms(),
            )
        };
        match applied {
            Ok(applied) if applied.accepted => {
                if let Some(transition) = Self::transition_from_applied(applied) {
                    replication::after_membership_accepted(&mut state, &transition);
                }
                self.sync_worker.wake();
                Ok(())
            }
            Ok(_) => Err(crate::PersonNameError::Membership(
                MembershipError::AuthorizationRejected,
            )),
            Err(error) => {
                if matches!(
                    error,
                    MembershipError::Storage(StorageError::UncertainOutcome)
                ) {
                    state.poisoned = true;
                }
                Err(crate::PersonNameError::Membership(error))
            }
        }
    }

    pub fn rename_principal(
        &self,
        workspace: WorkspaceId,
        id: crate::PrincipalId,
        name: &str,
    ) -> Result<(), crate::PersonNameError> {
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?
            .clone();
        crate::principals::set_id(
            &mut state.connection,
            &projection,
            self.public_identity,
            id,
            name,
        )?;
        principal_sync::publish_generation(&state, workspace);
        principal_sync::initialize(&mut state, workspace).map_err(fault!(
            crate::PersonNameError::Storage(StorageError::Database)
        ))
    }

    pub fn workspace_principal(
        &self,
        workspace: WorkspaceId,
        identity: PublicIdentity,
    ) -> Result<Option<crate::WorkspacePrincipal>, crate::PersonNameError> {
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        crate::principals::lookup(&state.connection, projection, identity)
    }

    pub fn principal(
        &self,
        workspace: WorkspaceId,
        id: crate::PrincipalId,
    ) -> Result<Option<crate::WorkspacePrincipal>, crate::PersonNameError> {
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        crate::principals::lookup_id(&state.connection, projection, id)
    }

    /// Membership and Principal presentation from one runtime state, after pending
    /// membership transitions have been applied. A Shell roster must use this pair.
    pub fn principal_roster(
        &self,
        workspace: WorkspaceId,
    ) -> Result<(crate::MembershipSnapshot, Vec<crate::WorkspacePrincipal>), crate::PersonNameError>
    {
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
        self.maintain_and_publish(&mut state, self.clock.now_unix_ms())
            .map_err(crate::PersonNameError::Membership)?;
        let projection = state
            .memberships
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        let rows = crate::principals::registry(projection)
            .into_iter()
            .map(|(id, _, _, _, _)| {
                crate::principals::lookup_id(&state.connection, projection, id)
                    .map(|row| row.expect("current Principal has an anchor"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((projection.snapshot(), rows))
    }

    pub fn principals_snapshot(
        &self,
        workspace: WorkspaceId,
    ) -> Result<(u64, Vec<crate::WorkspacePrincipal>), crate::PersonNameError> {
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        let generation = crate::principals::edit::profile_generation(&state.connection, workspace)
            .map_err(fault!(crate::PersonNameError::Storage(
                StorageError::Database
            )))?;
        let rows = crate::principals::registry(projection)
            .into_iter()
            .map(|(id, _, _, _, _)| {
                crate::principals::lookup_id(&state.connection, projection, id)
                    .map(|row| row.expect("current Principal has an anchor"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((generation, rows))
    }

    pub fn subscribe_principals(
        &self,
        workspace: WorkspaceId,
    ) -> Result<tokio::sync::watch::Receiver<u64>, crate::PersonNameError> {
        self.subscribe_person_names(workspace)
    }

    /// Local-only binding; authorization requires this Installation to steward
    /// the Agent. The Package Host consumes this in Phase 4.
    pub fn bind_agent_package(
        &self,
        workspace: WorkspaceId,
        id: crate::PrincipalId,
        package: &str,
    ) -> Result<(), crate::PersonNameError> {
        if package.is_empty()
            || package.len() > u16::MAX as usize
            || !package.is_ascii()
            || crate::synchronization::is_reserved_record_scope(package)
        {
            return Err(crate::PersonNameError::InvalidPackageIdentity);
        }
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
            .get(&workspace)
            .ok_or(crate::PersonNameError::WorkspaceNotFound)?;
        let row = crate::principals::registry(projection)
            .into_iter()
            .find(|(candidate, _, _, _, _)| *candidate == id)
            .ok_or(crate::PersonNameError::AgentNotFound)?;
        if row.2 != crate::PrincipalKind::Agent
            || !crate::principals::registry(projection)
                .iter()
                .any(|(person_id, key, _, _, _)| {
                    Some(*person_id) == row.3 && *key == self.public_identity
                })
        {
            return Err(crate::PersonNameError::NotOwnPerson);
        }
        crate::principals::agent_keys::bind(&state.connection, workspace, id, package)
            .map_err(crate::PersonNameError::Storage)
    }
}
