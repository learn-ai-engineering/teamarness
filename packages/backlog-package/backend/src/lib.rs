#![forbid(unsafe_code)]

//! The Backlog Package: Epics that group Tickets, replicated on the immutable record log.
//!
//! Every change is a record; each Ticket field converges on its own by Order Key, and a
//! Ticket's version is the greatest Order Key applied to it (ADR 0039). Titles and bodies live
//! only in memory (ADR 0027). This crate depends only on the language-neutral Package SDK;
//! transport and key custody stay in Core.

pub(crate) mod domain;
pub(crate) mod store;

use serde_json::json;
use teamarness_package_sdk::{BackendError, Router};

pub use domain::{
    DEFAULT_PRIORITY, DEFAULT_STATUS, DomainError, DomainRecord, Field, Priority, Status,
    TicketChanges,
};
pub use store::projection::{Outcome, Projection, Touched};
pub use store::{BacklogStore, EpicView, Incoming, IngestResult, StoreError, TicketView};

/// One activation of the Backlog Package. Requests, subscriptions and synchronization arrive
/// with the backend runtime; this activation opens the store.
pub struct BacklogActive {
    store: BacklogStore,
}

impl BacklogActive {
    pub fn store(&self) -> &BacklogStore {
        &self.store
    }
}

/// The Backlog Package Backend.
#[must_use]
pub fn router() -> Router<BacklogActive> {
    Router::new(|activation| {
        let path = activation
            .initialization()
            .data_directory()
            .path()
            .join("backlog.sqlite3");
        let cancellation = activation.cancellation().clone();
        async move {
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                BacklogStore::open(path).map_err(storage_error)
            })
            .await?;
            Ok(BacklogActive { store })
        }
    })
}

fn storage_error(_error: StoreError) -> BackendError {
    BackendError::new("backlog.unavailable", true, json!({}))
}
