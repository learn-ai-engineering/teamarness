//! Folder sync: a Collection mapped to a folder of Markdown files on this Installation.
//!
//! It holds the file codec (names, hashes, safe atomic writes and the splice writer), the flat
//! folder scan, and the reconciler that applies the file-to-Document and Document-to-file rules
//! crash-safely ([`FolderSync`]). The watcher (slice B Phase 5) drives the reconciler.
//!
//! Folder sync never follows a symbolic link, never writes outside the mapped root, and never
//! overwrites a file whose bytes it has not seen.

// The reconciler's first caller outside tests is the watcher and lifecycle task (slice B Phase 5),
// and the settings requests (Phase 6) are the first callers of `git_ancestor`. Until then only
// the tests reach them, so the shipped build allows what it does not use yet. The test build
// allows nothing.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

mod codec;
mod reconcile;
mod scan;

pub(crate) use reconcile::{FolderSync, Outcome, Refusal};
