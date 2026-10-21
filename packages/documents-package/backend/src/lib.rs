#![forbid(unsafe_code)]

//! The Documents Package: collaborative Markdown documents, Collections, and Comments.
//!
//! Every change is a record on the immutable record log; document text is rebuilt with Loro.
//! This crate depends only on the language-neutral Package SDK; transport and key custody
//! stay in Core.

mod backend;

pub use backend::{DocumentsActive, router};
