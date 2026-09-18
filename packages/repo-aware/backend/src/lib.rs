#![forbid(unsafe_code)]

pub mod error;
pub mod registration;
pub mod snapshot;
pub mod state;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{Backend, BackendError, BackendRequest, EventSubscription, Package};
use tokio::sync::Mutex;

use error::ErrorInfo;
use registration::validate_git_checkout;
use snapshot::compute_snapshot_impl;
use state::StateStore;

#[derive(Debug)]
pub struct RepoAwareBackend {
    active: Arc<Mutex<Option<StateStore>>>,
}

impl Default for RepoAwareBackend {
    fn default() -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
        }
    }
}

impl RepoAwareBackend {
    pub async fn serve(self) -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(self)).serve().await
    }

    fn backend_error(info: ErrorInfo) -> BackendError {
        BackendError::new(info.code, info.retryable, empty_object())
    }

    async fn activate(&self, request: &BackendRequest) -> Result<JsonValue, BackendError> {
        let path = request.initialization.data_directory().path();
        let store = StateStore::open(path)?;
        *self.active.lock().await = Some(store);
        Ok(empty_object())
    }

    async fn list(&self) -> Result<JsonValue, BackendError> {
        let active = self.active.lock().await;
        let store = active
            .as_ref()
            .ok_or_else(|| Self::backend_error(error::STORAGE_FAILED))?;
        serde_json::to_value(serde_json::json!({
            "registrations": store.registrations(),
        }))
        .map_err(|_| Self::backend_error(error::STORAGE_FAILED))
    }

    async fn register(&self, payload: JsonValue) -> Result<JsonValue, BackendError> {
        let path = payload
            .as_object()
            .and_then(|obj| obj.get("path"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| Self::backend_error(error::INVALID_REQUEST))?;

        let canonical = registration::validate_path(path)?;
        validate_git_checkout(&canonical)?;
        let root = registration::git_root(&canonical)?;

        let mut active = self.active.lock().await;
        let store = active
            .as_mut()
            .ok_or_else(|| Self::backend_error(error::STORAGE_FAILED))?;
        let registration = store.register(&root)?;

        serde_json::to_value(serde_json::json!({ "registration": registration }))
            .map_err(|_| Self::backend_error(error::STORAGE_FAILED))
    }

    async fn remove(&self, payload: JsonValue) -> Result<JsonValue, BackendError> {
        let id = payload
            .as_object()
            .and_then(|obj| obj.get("registrationId"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| Self::backend_error(error::INVALID_REQUEST))?;

        let mut active = self.active.lock().await;
        let store = active
            .as_mut()
            .ok_or_else(|| Self::backend_error(error::STORAGE_FAILED))?;
        store.remove(id)?;

        Ok(serde_json::json!({ "removed": true }))
    }

    async fn snapshot(&self, payload: JsonValue) -> Result<JsonValue, BackendError> {
        let registration_id = payload
            .as_object()
            .and_then(|obj| obj.get("registrationId"))
            .and_then(JsonValue::as_str)
            .map(|s| s.to_owned())
            .ok_or_else(|| Self::backend_error(error::INVALID_REQUEST))?;

        let (path, cancel) = {
            let active = self.active.lock().await;
            let store = active
                .as_ref()
                .ok_or_else(|| Self::backend_error(error::STORAGE_FAILED))?;

            let registration = store
                .registrations()
                .into_iter()
                .find(|r| r.id == registration_id)
                .ok_or_else(|| Self::backend_error(error::REGISTRATION_NOT_FOUND))?;

            (
                registration.path.to_owned(),
                Arc::new(AtomicBool::new(false)),
            )
        };

        let cancel_clone = cancel.clone();
        let rid = registration_id.clone();
        let snapshot =
            tokio::task::spawn_blocking(move || compute_snapshot_impl(&rid, &path, cancel_clone))
                .await
                .map_err(|_| Self::backend_error(error::SNAPSHOT_INTERRUPTED))??;

        serde_json::to_value(serde_json::json!({
            "registrationId": registration_id,
            "snapshot": snapshot,
        }))
        .map_err(|_| Self::backend_error(error::STORAGE_FAILED))
    }
}

impl Backend for RepoAwareBackend {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" => self.activate(&request).await,
                "lifecycle.suspend" | "lifecycle.dispose" => {
                    *self.active.lock().await = None;
                    Ok(empty_object())
                }
                "lifecycle.health" => Ok(serde_json::json!({ "healthy": true })),
                "repo_aware.list" => self.list().await,
                "repo_aware.register" => self.register(request.payload).await,
                "repo_aware.snapshot" => self.snapshot(request.payload).await,
                "repo_aware.remove" => self.remove(request.payload).await,
                _ => Err(Self::backend_error(error::INVALID_REQUEST)),
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async { Err(Self::backend_error(error::INVALID_REQUEST)) })
    }
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Default::default())
}
