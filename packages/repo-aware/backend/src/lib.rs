#![forbid(unsafe_code)]

pub mod error;
pub mod registration;
pub mod snapshot;
pub mod state;

use std::sync::Mutex;

use serde::Deserialize;
use teamarness_package_sdk::{BackendError, Context, Empty, Router};

use registration::resolve_registration;
use snapshot::compute_snapshot_impl;
use state::StateStore;

/// Everything one activation of Repo-aware owns.
///
/// The store is a plain synchronous mutex because every handler touches it inside
/// `Context::blocking`: registration, snapshots and state writes are filesystem work, and none of
/// it may hold an async worker.
pub struct RepoState {
    store: Mutex<StateStore>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisterRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationRequest {
    registration_id: String,
}

/// The Repo-aware Backend.
#[must_use]
pub fn router() -> Router<RepoState> {
    Router::new(|activation| {
        let directory = activation.data_directory().to_owned();
        let cancellation = activation.cancellation().clone();
        async move {
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                StateStore::open(&directory).map_err(BackendError::from)
            })
            .await?;
            Ok(RepoState {
                store: Mutex::new(store),
            })
        }
    })
    .request(
        "repo_aware.list",
        |context: Context<RepoState>, _: Empty| async move {
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    let store = state.store.lock().map_err(|_| storage_failed())?;
                    serde_json::to_value(serde_json::json!({
                        "registrations": store.registrations(),
                    }))
                    .map_err(|_| storage_failed())
                })
                .await
        },
    )
    .request(
        "repo_aware.register",
        |context: Context<RepoState>, request: RegisterRequest| async move {
            let state = context.state().clone();
            context
                .blocking(move |cancel| {
                    let root = resolve_registration(&request.path, cancel)?;
                    if cancel.is_cancelled() {
                        return Err(error::SNAPSHOT_INTERRUPTED.into());
                    }
                    let mut store = state.store.lock().map_err(|_| storage_failed())?;
                    let registration = store.register(&root)?;
                    serde_json::to_value(serde_json::json!({ "registration": registration }))
                        .map_err(|_| storage_failed())
                })
                .await
        },
    )
    .request(
        "repo_aware.remove",
        |context: Context<RepoState>, request: RegistrationRequest| async move {
            let state = context.state().clone();
            context
                .blocking(move |_| {
                    let mut store = state.store.lock().map_err(|_| storage_failed())?;
                    store.remove(&request.registration_id)?;
                    Ok(serde_json::json!({ "removed": true }))
                })
                .await
        },
    )
    .request(
        "repo_aware.snapshot",
        |context: Context<RepoState>, request: RegistrationRequest| async move {
            let registration_id = request.registration_id;
            let path = {
                let state = context.state();
                let store = state.store.lock().map_err(|_| storage_failed())?;
                store
                    .registrations()
                    .into_iter()
                    .find(|entry| entry.id == registration_id)
                    .map(|entry| entry.path)
                    .ok_or_else(|| BackendError::from(error::REGISTRATION_NOT_FOUND))?
            };
            let id = registration_id.clone();
            // The walk polls the request's own interrupt flag, so a cancelled request stops the
            // `gix` traversal instead of leaving it to finish unread.
            let snapshot = context
                .blocking(move |cancel| {
                    compute_snapshot_impl(&id, &path, cancel.interrupt_flag())
                        .map_err(BackendError::from)
                })
                .await?;
            serde_json::to_value(serde_json::json!({
                "registrationId": registration_id,
                "snapshot": snapshot,
            }))
            .map_err(|_| storage_failed())
        },
    )
}

fn storage_failed() -> BackendError {
    error::STORAGE_FAILED.into()
}
