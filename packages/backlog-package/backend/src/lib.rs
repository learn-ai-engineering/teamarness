#![forbid(unsafe_code)]

//! The Backlog Package: Epics that group Tickets, replicated on the immutable record log.
//!
//! Every change is a record; each Ticket field converges on its own by Order Key, and a
//! Ticket's version is the greatest Order Key applied to it (ADR 0039). Titles and bodies live
//! only in memory (ADR 0027). This crate depends only on the language-neutral Package SDK;
//! transport and key custody stay in Core.

mod backend;
pub(crate) mod domain;
pub(crate) mod store;

pub use backend::{BacklogActive, router};
pub use domain::{
    DEFAULT_PRIORITY, DEFAULT_STATUS, DomainError, DomainRecord, Field, Priority, Status,
    TicketChanges,
};
pub use store::projection::{Outcome, Projection, Touched};
pub use store::{
    BacklogStore, EpicView, Incoming, IngestResult, SealedRow, StoreError, TicketLocks, TicketView,
};
