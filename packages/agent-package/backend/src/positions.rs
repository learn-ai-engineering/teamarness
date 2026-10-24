//! The high-water mark of every Channel the harness has looked at: `positions.json` in the
//! Package Data Directory.
//!
//! A mark is the Conversations `position` of the newest Message the harness has dealt with.
//! Positions are fixed-width lowercase hex order keys, so string order is Channel order; the
//! comparison still guards on length so a key of another width can never read as older. A mark
//! is written before anything depends on it: on first sight of a Channel it is set to the head
//! so history is never answered, and it moves past a trigger before the reply is requested, so a
//! crash or an uncertain post never yields a second reply.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use teamarness_package_sdk::storage::{JsonDocumentStore, StorageError};

const POSITIONS_FILE: &str = "positions.json";
const POSITIONS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u32,
    /// Channel ID → the position of the newest Message handled in that Channel; an empty string
    /// marks a Channel first seen while empty, so its every Message is new.
    channels: BTreeMap<String, String>,
}

/// Whether `candidate` lies after `mark` in Channel order.
#[must_use]
pub fn is_after(candidate: &str, mark: &str) -> bool {
    (candidate.len(), candidate) > (mark.len(), mark)
}

/// The persisted marks of one Package Instance.
#[derive(Debug)]
pub struct Positions {
    document: JsonDocumentStore<Document>,
}

impl Positions {
    /// Opens the document, creating an empty one on first use.
    ///
    /// # Errors
    ///
    /// Returns the storage failure; a document from an unknown schema version is `Corrupt`.
    pub fn open(directory: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(directory)?;
        let document =
            JsonDocumentStore::open_or_create(directory.join(POSITIONS_FILE), || Document {
                schema_version: POSITIONS_SCHEMA_VERSION,
                channels: BTreeMap::new(),
            })?;
        if document.get().schema_version != POSITIONS_SCHEMA_VERSION {
            return Err(StorageError::Corrupt);
        }
        Ok(Self { document })
    }

    /// The mark of `channel`, or `None` when the harness has never seen it.
    #[must_use]
    pub fn mark(&self, channel: &str) -> Option<&str> {
        self.document
            .get()
            .channels
            .get(channel)
            .map(String::as_str)
    }

    /// Persists `position` as the mark of `channel`; the in-memory mark changes only after the
    /// write succeeded.
    ///
    /// # Errors
    ///
    /// Returns the write failure; the previous mark is kept.
    pub fn advance(&mut self, channel: &str, position: &str) -> Result<(), StorageError> {
        if self.mark(channel) == Some(position) {
            return Ok(());
        }
        self.document.update(|document| {
            document
                .channels
                .insert(channel.to_owned(), position.to_owned());
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_persist_and_compare_in_channel_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut positions = Positions::open(directory.path()).unwrap();
        assert_eq!(positions.mark("c1"), None);
        positions.advance("c1", "").unwrap();
        assert_eq!(positions.mark("c1"), Some(""));
        positions
            .advance("c1", &"0".repeat(81).replace('0', "1"))
            .unwrap();
        let reopened = Positions::open(directory.path()).unwrap();
        assert_eq!(reopened.mark("c1"), Some("1".repeat(81).as_str()));
        assert!(is_after("1", ""));
        assert!(is_after("10", "09"));
        assert!(is_after("100", "99"));
        assert!(!is_after("09", "10"));
        assert!(!is_after("", ""));
        assert!(!is_after("a", "a"));
    }

    #[test]
    fn unknown_schema_version_is_corrupt() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(POSITIONS_FILE),
            br#"{"schemaVersion":9,"channels":{}}"#,
        )
        .unwrap();
        assert_eq!(
            Positions::open(directory.path()).err(),
            Some(StorageError::Corrupt)
        );
    }
}
