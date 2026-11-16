//! Folder sync: a Collection mapped to a folder of Markdown files on this Installation.
//!
//! It holds the file codec (names, hashes, safe atomic writes and the splice writer), the flat
//! folder scan, the reconciler that applies the file-to-Document and Document-to-file rules
//! crash-safely ([`FolderSync`]), and the session that drives it from file watchers, body events
//! and catalog changes ([`watch::run`]).
//!
//! Folder sync never follows a symbolic link, never writes outside the mapped root, and never
//! overwrites a file whose bytes it has not seen.

mod codec;
mod mapping;
mod reconcile;
mod scan;
pub(crate) mod watch;

pub(crate) use mapping::{SetOptions, clear_mapping, list_mappings, set_mapping};
pub(crate) use reconcile::FolderSync;
