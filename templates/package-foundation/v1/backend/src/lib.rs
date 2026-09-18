#![forbid(unsafe_code)]

//! The Package Backend: one typed request, one persisted document, one activation state.
//!
//! The router owns the protocol. This file owns the product: what an operation means, what is
//! stored, and which error code an author-visible failure carries.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use teamarness_package_sdk::storage::JsonDocumentStore;
use teamarness_package_sdk::{BackendError, Context, Empty, Router};

const NAMESPACE: &str = "__REQUEST_NAMESPACE__";
// Token substitution changes the literal width, so rustfmt must leave this declaration stable.
#[rustfmt::skip]
const DISPLAY_NAME: &str = __PACKAGE_DISPLAY_NAME_RUST__;

const DOCUMENT_NAME: &str = "__REQUEST_NAMESPACE__.v1.json";
/// The one declared request, named once so the contract and the router cannot drift.
pub const READ: &str = "__REQUEST_NAMESPACE__.read";
/// This Package's own error code for a failed read or write.
pub const STORAGE_FAILED: &str = "__REQUEST_NAMESPACE__.storage_failed";
const DOCUMENT_SCHEMA_VERSION: u32 = 1;

/// What the Package keeps in its own Package Data Directory.
///
/// The schema version is written with the data so a later change can migrate it deliberately.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub schema_version: u32,
    pub read_count: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: DOCUMENT_SCHEMA_VERSION,
            read_count: 0,
        }
    }
}

/// What one activation holds: the open document, shared by every handler of that activation.
pub struct State {
    store: Mutex<JsonDocumentStore<Document>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadResponse {
    message: String,
    display_name: &'static str,
    read_count: u64,
}

/// Storage failed. It is retryable: the same request may succeed once the cause clears.
fn storage_failed() -> BackendError {
    BackendError::new(STORAGE_FAILED, true, serde_json::json!({}))
}

/// The Package Backend.
#[must_use]
pub fn router() -> Router<State> {
    Router::new(|activation| {
        let path = activation.data_directory().join(DOCUMENT_NAME);
        let cancellation = activation.cancellation().clone();
        async move {
            // Opening a file blocks, so it runs off the async runtime.
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                JsonDocumentStore::open_or_create(path, Document::default)
                    .map_err(|_| storage_failed())
            })
            .await?;
            Ok(State {
                store: Mutex::new(store),
            })
        }
    })
    .request(READ, |context: Context<State>, _: Empty| async move {
        let state = context.state().clone();
        context
            .blocking(move |_| {
                let mut store = state.store.lock().map_err(|_| storage_failed())?;
                let read_count = store
                    .update(|document| {
                        document.read_count += 1;
                        Ok(document.read_count)
                    })
                    .map_err(|_| storage_failed())?;
                Ok(ReadResponse {
                    message: format!("Hello from {NAMESPACE}"),
                    display_name: DISPLAY_NAME,
                    read_count,
                })
            })
            .await
    })
}
