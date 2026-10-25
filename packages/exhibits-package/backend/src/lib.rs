#![forbid(unsafe_code)]

//! The Exhibits Package: Agent-authored, multi-page interactive Exhibits that a Person views
//! inside Teamarness.
//!
//! An Agent (or the local Person) writes a Draft through the `dev.teamarness.exhibits` Port
//! Contract: it uploads files in chunks into a content store keyed by SHA-256, then names them
//! in `draft.update`. The Exhibits UI opens a Draft through `teamarness.content-views`, so the
//! Host serves its verified bytes in a nested, sandboxed content frame. Drafts are local to this
//! Installation; they are never replicated.

mod backend;
pub mod domain;
pub mod guide;
pub mod store;

pub use backend::{CONTRACT, DraftChange, ExhibitsActive, router, router_with_clock};
