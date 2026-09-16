#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{Backend, BackendError, BackendRequest, EventSubscription, Package};

#[derive(Debug)]
pub struct RepoAwareBackend;

impl RepoAwareBackend {
    pub async fn serve(self) -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(self)).serve().await
    }
}

impl Backend for RepoAwareBackend {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                "lifecycle.activate"
                | "lifecycle.recover"
                | "lifecycle.suspend"
                | "lifecycle.dispose" => Ok(empty_object()),
                "repo_aware.register"
                | "repo_aware.list"
                | "repo_aware.snapshot"
                | "repo_aware.remove" => {
                    Err(backend_error("not_implemented", false))
                }
                _ => Err(backend_error("repo_aware.unknown_operation", false)),
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async { Err(backend_error("repo_aware.unknown_subscription", false)) })
    }
}

fn backend_error(code: &'static str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, empty_object())
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Default::default())
}