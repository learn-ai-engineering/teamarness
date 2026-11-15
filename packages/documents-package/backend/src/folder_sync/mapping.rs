//! Making a mapping: the root checks and the mapping rules of `documents.mapping.set`.
//!
//! A mapped root must be absolute, exist and be a folder. It must not be inside this Package's
//! data directory, and it must not equal, contain or sit inside another mapping's root. If the
//! folder holds no file folder sync would import, the Collection is exported to it; if the
//! Collection holds no Document, the folder is imported into it; if both hold something, the
//! mapping is refused. Mapping only stores the mapping: the folder sync session sees it as a
//! catalog change and reconciles it in full.

use std::path::{Path, PathBuf};

use serde_json::json;
use teamarness_package_sdk::BackendError;

use super::super::storage_failed;
use super::scan::scan;
use crate::store::mappings::{FolderMapping, MappingState};
use crate::store::{DocumentStore, StoreError};

/// What a new mapping does first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// The folder was empty: every Document of the Collection is written to it.
    Export,
    /// The Collection was empty: every file of the folder becomes a Document.
    Import,
    /// The Collection was already mapped to this folder.
    Unchanged,
}

impl Direction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Import => "import",
            Self::Unchanged => "unchanged",
        }
    }
}

/// A refused mapping. `reason` is one of `not_absolute`, `not_found`, `not_a_directory`,
/// `unreadable`, `inside_data_directory`, `overlapping_mapping` or `not_empty`.
fn refused(reason: &str) -> BackendError {
    BackendError::new(
        "documents.mapping_refused",
        false,
        json!({ "reason": reason }),
    )
}

/// Maps `collection` to the folder at `path`, and returns the canonical root and what the
/// mapping does first. Runs file and SQLite work: call it on the blocking pool.
pub(crate) fn set_mapping(
    store: &DocumentStore,
    data_directory: &Path,
    collection: &str,
    path: &str,
) -> Result<(String, Direction), BackendError> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(refused("not_absolute"));
    }
    let root = requested.canonicalize().map_err(|_| refused("not_found"))?;
    if !root.is_dir() {
        return Err(refused("not_a_directory"));
    }
    let root_path = root
        .to_str()
        .ok_or_else(|| refused("not_found"))?
        .to_owned();
    let data = data_directory
        .canonicalize()
        .unwrap_or_else(|_| data_directory.to_owned());
    if root.starts_with(&data) {
        return Err(refused("inside_data_directory"));
    }
    let mappings = store.folder_mappings().map_err(|_| storage_failed())?;
    if mappings
        .iter()
        .any(|mapping| mapping.collection_id == collection && mapping.root_path == root_path)
    {
        return Ok((root_path, Direction::Unchanged));
    }
    for other in mappings
        .iter()
        .filter(|mapping| mapping.collection_id != collection)
    {
        let other = PathBuf::from(&other.root_path);
        if root.starts_with(&other) || other.starts_with(&root) {
            return Err(refused("overlapping_mapping"));
        }
    }
    let folder_empty = scan(&root)
        .map_err(|_| refused("unreadable"))?
        .files
        .is_empty();
    let collection_empty = !store
        .documents()
        .map_err(|_| storage_failed())?
        .iter()
        .any(|document| document.collection_id == collection && !document.archived);
    let direction = match (folder_empty, collection_empty) {
        (true, _) => Direction::Export,
        (false, true) => Direction::Import,
        (false, false) => return Err(refused("not_empty")),
    };
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    store
        .set_folder_mapping(&FolderMapping {
            collection_id: collection.to_owned(),
            root_path: root_path.clone(),
            state: MappingState::Active,
            created_at,
        })
        .map_err(|cause| match cause {
            StoreError::Conflict => refused("overlapping_mapping"),
            _ => storage_failed(),
        })?;
    Ok((root_path, direction))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(directory: &Path) -> DocumentStore {
        DocumentStore::open(directory.join("documents.sqlite3")).unwrap()
    }

    fn reason(result: Result<(String, Direction), BackendError>) -> String {
        let cause = result.unwrap_err();
        assert_eq!(cause.code(), "documents.mapping_refused");
        cause.details()["reason"].as_str().unwrap().to_owned()
    }

    #[test]
    fn roots_must_be_absolute_existing_folders_outside_data_and_other_mappings() {
        let data = tempfile::tempdir().unwrap();
        let folders = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let a = folders.path().join("a");
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::write(folders.path().join("file.md"), "x").unwrap();

        assert_eq!(
            reason(set_mapping(&store, data.path(), "c1", "relative/path")),
            "not_absolute"
        );
        let missing = folders.path().join("missing");
        assert_eq!(
            reason(set_mapping(
                &store,
                data.path(),
                "c1",
                missing.to_str().unwrap()
            )),
            "not_found"
        );
        let file = folders.path().join("file.md");
        assert_eq!(
            reason(set_mapping(
                &store,
                data.path(),
                "c1",
                file.to_str().unwrap()
            )),
            "not_a_directory"
        );
        assert_eq!(
            reason(set_mapping(
                &store,
                data.path(),
                "c1",
                data.path().to_str().unwrap()
            )),
            "inside_data_directory"
        );

        let (root, direction) =
            set_mapping(&store, data.path(), "c1", a.to_str().unwrap()).unwrap();
        assert_eq!(direction, Direction::Export);
        assert_eq!(PathBuf::from(&root), a.canonicalize().unwrap());
        assert_eq!(
            set_mapping(&store, data.path(), "c1", a.to_str().unwrap())
                .unwrap()
                .1,
            Direction::Unchanged
        );
        for overlapping in [a.clone(), a.join("inner"), folders.path().to_owned()] {
            assert_eq!(
                reason(set_mapping(
                    &store,
                    data.path(),
                    "c2",
                    overlapping.to_str().unwrap()
                )),
                "overlapping_mapping"
            );
        }
        assert_eq!(store.folder_mappings().unwrap().len(), 1);
    }

    #[test]
    fn a_folder_with_files_imports_into_an_empty_collection() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = store(data.path());
        std::fs::write(folder.path().join("Plan.md"), "# Plan\n").unwrap();
        let (_, direction) =
            set_mapping(&store, data.path(), "c1", folder.path().to_str().unwrap()).unwrap();
        assert_eq!(direction, Direction::Import);
    }
}
