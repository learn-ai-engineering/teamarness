//! Deleting a Workspace from this Installation (ADR 0050).
//!
//! One operation, under the Personal tier lock and the reconciliation lock so nothing creates an
//! Instance meanwhile:
//!
//! 1. Core confirms the Workspace may be deleted.
//! 2. The Host records the deletion, so a restart can finish it.
//! 3. Every Instance of the Workspace is disposed and removed.
//! 4. Core erases the Workspace, checking its rule again.
//! 5. The Package Data Directories and Host rows go, then the record.
//!
//! A refusal or failure before step 4 leaves the Workspace whole: the record is dropped and its
//! Instances are created again. Opening the Host finishes step 5 when Core no longer lists the
//! Workspace, or drops the record when it does (see [`PackageHost::settle_workspace_deletions`]).

use std::sync::Arc;

use crate::data_directory;
use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, host_error, instance_error,
};
use crate::host_store::HostStoreHandle;
use crate::values::{PackageInstanceKey, WorkspaceId};
use crate::{HostRevision, PackageHost, PackageHostConfig, ReconcileWorkspaces};

/// What a Workspace deletion did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDeletion {
    pub revision: HostRevision,
    /// The Instances that were disposed and removed, for callers that hold per-Instance state.
    pub removed: Vec<PackageInstanceKey>,
    /// The Workspace is gone, but some Package data could not be removed yet. Opening the Host
    /// removes it.
    pub data_removal_pending: bool,
}

/// Why a Workspace was not deleted. Nothing was erased in either case.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceDeletionFailure {
    /// Core's rule refused: unknown Workspace, not its creator, or other Members.
    Refused(teamarness_core::WorkspaceError),
    /// The Host could not stop the Workspace's Instances or record the deletion.
    Host(PackageErrorEnvelope),
}

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// Deletes a Workspace and all of its data on this Installation (ADR 0050).
    pub async fn delete_workspace(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<WorkspaceDeletion, WorkspaceDeletionFailure> {
        let core_workspace =
            <teamarness_core::WorkspaceId as std::str::FromStr>::from_str(workspace.as_str())
                .map_err(|_| {
                    WorkspaceDeletionFailure::Refused(
                        teamarness_core::WorkspaceError::UnknownWorkspace,
                    )
                })?;
        let mut tier = self.inner.personal.lock().await;
        let gate = self.inner.reconciliation.lock().await;
        self.ensure_running()
            .await
            .map_err(WorkspaceDeletionFailure::Host)?;
        let core = Arc::clone(&self.inner.core);
        blocking(move || core.check_workspace_deletable(core_workspace))
            .await
            .map_err(WorkspaceDeletionFailure::Host)?
            .map_err(WorkspaceDeletionFailure::Refused)?;

        let config = self.inner.config.clone();
        let marking = workspace.clone();
        let installation = config.installation_public_identity().clone();
        self.inner
            .store
            .run(move |store| {
                store.mark_workspace_deletion(installation.as_str(), marking.as_str())
            })
            .await
            .and_then(|result| result)
            .map_err(|failure| {
                WorkspaceDeletionFailure::Host(host_error(failure.code, PackageErrorStage::Storage))
            })?;

        let actors = self
            .inner
            .data
            .lock()
            .await
            .instances
            .iter()
            .filter(|(key, _)| key.workspace_id == *workspace)
            .map(|(key, actor)| (key.clone(), actor.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::new();
        let mut failure = None;
        for (key, actor) in actors {
            self.inner.port_broker.unregister(&key);
            let _mounts = self.inner.mount_management.lock().await;
            let disposed = actor.dispose().await;
            self.revoke_mounts_for_instance(&key).await;
            tier.record_disposed(&key, actor.generation().await);
            self.inner.data.lock().await.instances.remove(&key);
            if let Err(error) = disposed {
                failure.get_or_insert_with(|| instance_error(error, &key));
            }
            removed.push(key);
        }

        let erased = match failure {
            // A process that may still run keeps its data: nothing is erased.
            Some(error) => Err(WorkspaceDeletionFailure::Host(error)),
            None => {
                let core = Arc::clone(&self.inner.core);
                match blocking(move || core.delete_workspace(core_workspace)).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(refused)) => Err(WorkspaceDeletionFailure::Refused(refused)),
                    Err(error) => Err(WorkspaceDeletionFailure::Host(error)),
                }
            }
        };
        if let Err(failure) = erased {
            // Decide from Core whether the erase happened: an uncertain commit may have landed.
            let core = Arc::clone(&self.inner.core);
            let listed = blocking(move || {
                core.list_workspaces().map(|workspaces| {
                    workspaces
                        .iter()
                        .any(|summary| summary.id() == core_workspace)
                })
            })
            .await;
            match listed {
                Ok(Ok(true)) => {
                    // The Workspace is still whole: drop the record and give it its Instances back.
                    let clearing = workspace.clone();
                    let installation = config.installation_public_identity().clone();
                    let _ = self
                        .inner
                        .store
                        .run(move |store| {
                            store.clear_workspace_deletion(installation.as_str(), clearing.as_str())
                        })
                        .await;
                    drop(gate);
                    drop(tier);
                    let _ = self.reconcile_workspaces(ReconcileWorkspaces {}).await;
                    return Err(failure);
                }
                // Erased after all: finish like a success.
                Ok(Ok(false)) => {}
                // Unknown: the record stays, so opening the Host settles it either way.
                _ => return Err(failure),
            }
        }

        let data_removal_pending = remove_workspace_data(&self.inner.store, &config, workspace)
            .await
            .is_err();
        drop(gate);
        drop(tier);
        Ok(WorkspaceDeletion {
            revision: self.bump_revision().await,
            removed,
            data_removal_pending,
        })
    }

    /// Finishes or drops each recorded Workspace deletion when the Host opens, before any
    /// Instance is created. A Workspace Core still lists was not deleted, so its record goes and
    /// it keeps its data; one Core no longer lists has its Package data removed. A deletion whose
    /// data cannot be removed yet, for example because a process lease is unresolved, stays
    /// recorded for the next open.
    pub(crate) async fn settle_workspace_deletions(
        config: &PackageHostConfig,
        store: &HostStoreHandle,
        listed: &[teamarness_core::WorkspaceSummary],
    ) -> Result<(), PackageErrorEnvelope> {
        let installation = config.installation_public_identity().clone();
        let pending = store
            .run(move |store| store.pending_workspace_deletions(installation.as_str()))
            .await
            .and_then(|result| result)
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
        for workspace in pending {
            let Ok(workspace) = WorkspaceId::new(workspace) else {
                continue;
            };
            if listed
                .iter()
                .any(|summary| summary.id().to_string() == workspace.as_str())
            {
                let installation = config.installation_public_identity().clone();
                store
                    .run(move |store| {
                        store.clear_workspace_deletion(installation.as_str(), workspace.as_str())
                    })
                    .await
                    .and_then(|result| result)
                    .map_err(|failure| host_error(failure.code, PackageErrorStage::Storage))?;
            } else {
                let _ = remove_workspace_data(store, config, &workspace).await;
            }
        }
        Ok(())
    }
}

async fn remove_workspace_data(
    store: &HostStoreHandle,
    config: &PackageHostConfig,
    workspace: &WorkspaceId,
) -> Result<(), PackageErrorCode> {
    let private = config.private_data_directory().to_owned();
    let installation = config.installation_public_identity().clone();
    let workspace = workspace.clone();
    store
        .run(move |store| {
            data_directory::remove_workspace_data(store, &private, &installation, &workspace)
        })
        .await
        .map_err(|failure| failure.code)?
        .map_err(|failure| failure.code)
}

#[allow(clippy::result_large_err)]
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, PackageErrorEnvelope> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        host_error(
            PackageErrorCode::DataDirectoryUnavailable,
            PackageErrorStage::Storage,
        )
    })
}
