//! Folder sync: a Collection mapped to a folder of Markdown files on this Installation.
//!
//! This module is pure so far. It holds the file codec (names, hashes, safe atomic writes and
//! the splice writer) and the flat folder scan. It has no Backend state. The reconciler
//! (slice B Phase 4) and the watcher (Phase 5) build on it.
//!
//! Folder sync never follows a symbolic link, never writes outside the mapped root, and never
//! overwrites a file whose bytes it has not seen.

// Phase 4 of slice B (the reconciler) is the first caller outside tests and removes this allow.
// Until then the codec and the scan, and their re-exports below, are only used by their tests.
#![allow(dead_code, unused_imports)]

mod codec;
mod scan;

pub(crate) use codec::{
    TEMP_SUFFIX, WriteRefused, file_name, git_ancestor, name_key, sha256, splice, unique_name,
    write_atomic,
};
pub(crate) use scan::{Scan, ScannedFile, scan};
