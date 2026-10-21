use std::sync::{Arc, Mutex};

use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::capabilities::person_names::PersonNames;
use teamarness_package_sdk::capabilities::record_log::{Availability, RecordLog};
use teamarness_package_sdk::{BackendError, Context, Empty, Router, event_channel};
use tokio::sync::watch;

fn error(code: &str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, json!({}))
}

fn capability_error(operation: &str, cause: BackendError) -> BackendError {
    eprintln!(
        "documents capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    BackendError::new(
        "documents.capability_unavailable",
        cause.retryable(),
        json!({ "causeCode": cause.code() }),
    )
}

/// What the Package tells the UI about synchronization.
#[derive(Debug, Clone)]
pub(crate) struct SyncReport {
    pub(crate) availability: Availability,
    pub(crate) warnings: Vec<String>,
}

/// Everything one activation of Documents owns.
pub struct DocumentsActive {
    #[allow(dead_code)]
    log: RecordLog,
    #[allow(dead_code)]
    names: PersonNames,
    sync: Arc<Mutex<SyncReport>>,
    changes: watch::Sender<u64>,
    #[allow(dead_code)]
    installation: String,
}

/// The Documents Backend.
#[must_use]
pub fn router() -> Router<DocumentsActive> {
    Router::new(move |activation| {
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        async move {
            let log = RecordLog::from_initialization(&initialization, capabilities.clone())
                .map_err(|cause| capability_error("record-log", cause))?;
            let names = PersonNames::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("person-names", cause))?;
            let (changes, _) = watch::channel(0);
            Ok(DocumentsActive {
                log,
                names,
                sync: Arc::new(Mutex::new(SyncReport {
                    availability: Availability::CatchingUp,
                    warnings: Vec::new(),
                })),
                changes,
                installation: initialization.installation().identity.clone(),
            })
        }
    })
    .request(
        "documents.snapshot",
        |context: Context<DocumentsActive>, _: Empty| async move {
            Ok(context.state().snapshot_value())
        },
    )
    .subscription(
        "documents.changes",
        |context: Context<DocumentsActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(32);
            let mut revisions = context.state().changes.subscribe();
            tokio::spawn(async move {
                loop {
                    if sender.send(context.state().snapshot_value()).await.is_err() {
                        return;
                    }
                    if revisions.changed().await.is_err() {
                        return;
                    }
                }
            });
            Ok(subscription)
        },
    )
}

impl DocumentsActive {
    fn report(&self) -> SyncReport {
        self.sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The listing the UI renders. Phase 1 has no records yet, so it is empty.
    fn snapshot_value(&self) -> JsonValue {
        let report = self.report();
        json!({
            "collections": [],
            "documents": [],
            "availability": report.availability.as_str(),
            "warnings": report.warnings,
        })
    }
}

#[allow(dead_code)]
fn unused_error_helper() -> BackendError {
    error("documents.storage_failed", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamarness_package_sdk::testing::assert_router_matches_contract;

    #[test]
    fn router_matches_the_ui_contract() {
        assert_router_matches_contract(
            &router().operations(),
            include_str!("../../ui-contract.json"),
        );
    }

    /// Loro is linked and its text container round-trips an edit; the body model builds on it
    /// in a later phase.
    #[test]
    fn loro_text_round_trips() {
        let doc = loro::LoroDoc::new();
        let text = doc.get_text("body");
        text.insert(0, "# Plan").unwrap();
        doc.commit();
        assert_eq!(text.to_string(), "# Plan");
    }
}
