#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{Backend, BackendError, BackendRequest, Package};

const NAMESPACE: &str = "__REQUEST_NAMESPACE__";

#[derive(Default)]
pub struct GeneratedBackend;

impl GeneratedBackend {
    pub async fn serve() -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(Self)).serve().await
    }
}

impl Backend for GeneratedBackend {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" | "lifecycle.suspend" | "lifecycle.dispose" => Ok(object([])),
                operation if operation == format!("{NAMESPACE}.read") => Ok(object([
                    ("message", JsonValue::String(format!("Hello from {NAMESPACE}"))),
                    ("displayName", JsonValue::String("__PACKAGE_DISPLAY_NAME__".into())),
                ])),
                _ => Err(BackendError::new("package.unknown_operation", false, object([]))),
            }
        })
    }
}

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut value = JsonValue::Object(Default::default());
    let fields = value.as_object_mut().expect("new object");
    for (name, entry) in entries {
        fields.insert(name.to_owned(), entry);
    }
    value
}
