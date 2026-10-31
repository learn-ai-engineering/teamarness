//! The Markdown core: the shared body schema, Canonical Markdown export, and base-versioned
//! Markdown merge.
//!
//! This module depends only on `loro` and `comrak`, never on Documents domain or store types.
//! The normative contract is `context/02-system/09-documents/markdown-contract.md`; in short:
//!
//! - [`read_tree`] reads the `loro-prosemirror` tree of the shared schema as [`Block`]s, or
//!   returns a [`SchemaViolation`]. It is total and bounded by [`MAX_NESTING`].
//! - [`write_tree`] writes a body tree (one empty paragraph when empty) into a document that
//!   has none, with the binding's mark expand configuration: `after` for every mark, `none`
//!   for link.
//! - [`export`] gives Canonical Markdown. Export, import and export again gives the same
//!   bytes, and no content character is lost; only formatting Markdown cannot express, line-edge
//!   whitespace and empty paragraphs are normalised.
//! - [`canonical`] gives the Canonical Markdown a revision becomes, so a caller can tell whether
//!   a revision was already canonical.
//! - [`merge`] merges a Markdown revision made against a base version as one Loro update from a
//!   fresh peer. Unchanged blocks are untouched, changed blocks of the same kind are edited in
//!   place, and merging back an unchanged export produces no ops. It does not change the
//!   document; the caller appends and imports the update.

mod export;
mod merge;
mod schema;

pub use export::{canonical, export};
pub use merge::{DIFF_BUDGET, MergeError, Merged, merge, merge_within};
pub use schema::{
    Block, Inline, Link, MAX_NESTING, MAX_ORDER, Marks, SchemaViolation, WriteError, read_tree,
    write_tree,
};

/// Model-level functions for the property suites in `tests/`. Not part of the interface.
#[doc(hidden)]
pub mod testing {
    pub use super::export::{canonicalize, parse};
}
