//! Making, listing and clearing mappings: the root checks, the mapping rules, the git rule and
//! the pre-mapping report of `documents.mapping.set`, and the bodies of `documents.mapping.list`
//! and `documents.mapping.clear`.
//!
//! A mapped root must be absolute, exist and be a folder. It must not be inside this Package's
//! data directory, and it must not equal, contain or sit inside another mapping's root. If the
//! folder holds no file folder sync would import, the Collection is exported to it; if the
//! Collection holds no Document, the folder is imported into it; if both hold something, the
//! mapping is refused. A root inside a git working tree is mapped only when the Member confirms
//! it. Mapping only stores the mapping: the folder sync session sees it as a catalog change and
//! reconciles it in full. Clearing drops the state and leaves every file on disk.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::BackendError;

use super::super::storage_failed;
use super::codec::{EXTENSION, git_ancestor, name_key};
use super::reconcile::{Refusal, precheck, read_file};
use super::scan::{Scan, scan, subfolders};
use crate::markdown;
use crate::store::mappings::{FolderMapping, MappingState};
use crate::store::{DocumentStore, StoreError};

/// The most entries each list of a report or a listing carries. The counts are always whole.
pub(crate) const MAX_LISTED: usize = 200;

/// The most characters of a refusal's detail that a report or a listing carries.
const MAX_DETAIL_CHARS: usize = 200;

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

/// How `documents.mapping.set` treats a request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SetOptions {
    /// Check and report only: nothing is stored, and a git working tree is reported, not
    /// refused.
    pub dry_run: bool,
    /// The Member confirmed a root inside a git working tree.
    pub confirm_git: bool,
}

/// One file the pre-mapping report says will not be imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportRefusal {
    pub name: String,
    /// A [`Refusal`] reason, or `unreadable` for a file that cannot be read now.
    pub reason: &'static str,
    pub detail: Option<String>,
}

/// The pre-mapping report: what mapping the folder would do to each of its entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Files that will be imported, with their length in bytes.
    pub files: Vec<(String, u64)>,
    pub refused: Vec<ReportRefusal>,
    /// Imported files whose bytes are not Canonical Markdown: the first write-back after an
    /// edit reformats them.
    pub reformatted: Vec<String>,
    /// Folders that are not synced yet.
    pub subfolders: Vec<String>,
    /// The nearest folder at or above the root that holds `.git`.
    pub git_root: Option<String>,
}

impl Report {
    pub(crate) fn to_json(&self) -> JsonValue {
        json!({
            "counts": {
                "files": self.files.len(),
                "refused": self.refused.len(),
                "reformatted": self.reformatted.len(),
                "subfolders": self.subfolders.len(),
            },
            "files": self.files.iter().take(MAX_LISTED)
                .map(|(name, bytes)| json!({ "name": name, "bytes": bytes }))
                .collect::<Vec<_>>(),
            "refused": self.refused.iter().take(MAX_LISTED)
                .map(|refused| refusal_json(&refused.name, refused.reason, refused.detail.as_deref()))
                .collect::<Vec<_>>(),
            "reformatted": self.reformatted.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "subfolders": self.subfolders.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "gitRoot": self.git_root,
        })
    }
}

/// What `documents.mapping.set` decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    /// The canonical root path.
    pub root: String,
    pub direction: Direction,
    /// The pre-mapping report, made only for a dry run.
    pub report: Option<Report>,
}

/// A refused mapping. `reason` is one of `not_absolute`, `not_found`, `not_a_directory`,
/// `unreadable`, `inside_data_directory`, `overlapping_mapping`, `not_empty` or
/// `git_working_tree`.
fn refused(reason: &str) -> BackendError {
    refused_with(reason, json!({}))
}

fn refused_with(reason: &str, mut details: JsonValue) -> BackendError {
    details["reason"] = json!(reason);
    BackendError::new("documents.mapping_refused", false, details)
}

/// Checks a mapping of `collection` to the folder at `path`, and stores it unless
/// `options.dry_run`. `collection` is `None` for a Collection that does not exist yet ("Create
/// Collection from folder"): it counts as empty, and nothing is stored. Runs file and SQLite
/// work: call it on the blocking pool.
pub(crate) fn set_mapping(
    store: &DocumentStore,
    data_directory: &Path,
    collection: Option<&str>,
    path: &str,
    options: SetOptions,
) -> Result<Planned, BackendError> {
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
    let unchanged = mappings.iter().any(|mapping| {
        Some(mapping.collection_id.as_str()) == collection && mapping.root_path == root_path
    });
    if unchanged && !options.dry_run {
        return Ok(Planned {
            root: root_path,
            direction: Direction::Unchanged,
            report: None,
        });
    }
    for other in mappings
        .iter()
        .filter(|mapping| Some(mapping.collection_id.as_str()) != collection)
    {
        let other = PathBuf::from(&other.root_path);
        if root.starts_with(&other) || other.starts_with(&root) {
            return Err(refused("overlapping_mapping"));
        }
    }
    let scanned = scan(&root).map_err(|_| refused("unreadable"))?;
    let direction = if unchanged {
        Direction::Unchanged
    } else {
        let folder_empty = scanned.files.is_empty();
        let collection_empty = match collection {
            None => true,
            Some(collection) => !store
                .documents()
                .map_err(|_| storage_failed())?
                .iter()
                .any(|document| document.collection_id == collection && !document.archived),
        };
        match (folder_empty, collection_empty) {
            (true, _) => Direction::Export,
            (false, true) => Direction::Import,
            (false, false) => return Err(refused("not_empty")),
        }
    };
    let git_root = git_ancestor(&root).map(|found| found.to_string_lossy().into_owned());
    if options.dry_run {
        return Ok(Planned {
            root: root_path,
            direction,
            report: Some(report(&root, &scanned, git_root)),
        });
    }
    if let Some(git_root) = git_root
        && !options.confirm_git
    {
        return Err(refused_with(
            "git_working_tree",
            json!({ "gitRoot": git_root }),
        ));
    }
    let Some(collection) = collection else {
        return Ok(Planned {
            root: root_path,
            direction,
            report: None,
        });
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
    Ok(Planned {
        root: root_path,
        direction,
        report: None,
    })
}

/// The pre-mapping report of the folder at `root`, which `scanned` describes. Each file gets
/// the checks the reconciler runs before it creates a Document, and nothing is written.
fn report(root: &Path, scanned: &Scan, git_root: Option<String>) -> Report {
    let genesis = crate::body::genesis();
    let mut report = Report {
        subfolders: scanned.subfolders.clone(),
        git_root,
        ..Report::default()
    };
    let mut taken = HashSet::new();
    for file in &scanned.files {
        let refuse = |reason: &'static str, detail: Option<String>| ReportRefusal {
            name: file.rel.clone(),
            reason,
            detail: detail.map(|detail| detail.chars().take(MAX_DETAIL_CHARS).collect()),
        };
        if !taken.insert(name_key(&file.rel)) {
            report
                .refused
                .push(refuse(Refusal::NameCollisionUnresolved.as_str(), None));
            continue;
        }
        let Ok(Some(content)) = read_file(&root.join(&file.rel)) else {
            report.refused.push(refuse("unreadable", None));
            continue;
        };
        match precheck(&file.rel, content.bytes, &genesis) {
            Ok((_, text)) => {
                if markdown::canonical(&text).is_ok_and(|canonical| canonical != text) {
                    report.reformatted.push(file.rel.clone());
                }
                report.files.push((file.rel.clone(), file.len));
            }
            Err((reason, detail)) => report.refused.push(refuse(reason.as_str(), detail)),
        }
    }
    for link in scanned
        .symlinks
        .iter()
        .filter(|link| link.ends_with(EXTENSION))
    {
        report.refused.push(ReportRefusal {
            name: link.clone(),
            reason: Refusal::Symlink.as_str(),
            detail: None,
        });
    }
    for name in &scanned.unreadable {
        report.refused.push(ReportRefusal {
            name: name.clone(),
            reason: "unreadable",
            detail: None,
        });
    }
    report.refused.sort_by(|a, b| a.name.cmp(&b.name));
    report
}

fn refusal_json(name: &str, reason: &str, detail: Option<&str>) -> JsonValue {
    let mut value = json!({ "name": name, "reason": reason });
    if let Some(detail) = detail {
        value["detail"] = json!(detail.chars().take(MAX_DETAIL_CHARS).collect::<String>());
    }
    value
}

/// Drops the mapping of `collection` and its state. Every file stays on disk. Returns whether a
/// mapping existed.
pub(crate) fn clear_mapping(store: &DocumentStore, collection: &str) -> Result<bool, BackendError> {
    store
        .clear_folder_mapping(collection)
        .map_err(|_| storage_failed())
}

/// The body of `documents.mapping.list`: every Collection with its mapping, if any.
pub(crate) fn list_mappings(store: &DocumentStore) -> Result<JsonValue, BackendError> {
    let collections = store.collections().map_err(|_| storage_failed())?;
    let mut mappings: HashMap<String, FolderMapping> = store
        .folder_mappings()
        .map_err(|_| storage_failed())?
        .into_iter()
        .map(|mapping| (mapping.collection_id.clone(), mapping))
        .collect();
    let mut listed = Vec::with_capacity(collections.len());
    for collection in collections {
        let mapping = match mappings.remove(&collection.id) {
            None => JsonValue::Null,
            Some(mapping) => {
                let files = store
                    .mapped_files(&collection.id)
                    .map_err(|_| storage_failed())?
                    .len();
                let refused = store
                    .refused_files(&collection.id)
                    .map_err(|_| storage_failed())?;
                let root = Path::new(&mapping.root_path);
                let folders = subfolders(root);
                json!({
                    "path": mapping.root_path,
                    "state": mapping.state.as_str(),
                    "counts": {
                        "files": files,
                        "refused": refused.len(),
                        "subfolders": folders.len(),
                    },
                    "refused": refused.iter().take(MAX_LISTED)
                        .map(|file| refusal_json(&file.rel_path, &file.reason, file.detail.as_deref()))
                        .collect::<Vec<_>>(),
                    "subfolders": folders.iter().take(MAX_LISTED).collect::<Vec<_>>(),
                    "gitRoot": git_ancestor(root).map(|found| found.to_string_lossy().into_owned()),
                })
            }
        };
        listed.push(json!({
            "collectionId": collection.id,
            "name": collection.name.unwrap_or_else(|| "Untitled".into()),
            "archived": collection.archived,
            "mapping": mapping,
        }));
    }
    Ok(json!({ "collections": listed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(directory: &Path) -> DocumentStore {
        DocumentStore::open(directory.join("documents.sqlite3")).unwrap()
    }

    fn reason(result: Result<Planned, BackendError>) -> String {
        let cause = result.unwrap_err();
        assert_eq!(cause.code(), "documents.mapping_refused");
        cause.details()["reason"].as_str().unwrap().to_owned()
    }

    const SET: SetOptions = SetOptions {
        dry_run: false,
        confirm_git: false,
    };
    const DRY: SetOptions = SetOptions {
        dry_run: true,
        confirm_git: false,
    };

    #[test]
    fn roots_must_be_absolute_existing_folders_outside_data_and_other_mappings() {
        let data = tempfile::tempdir().unwrap();
        let folders = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let a = folders.path().join("a");
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::write(folders.path().join("file.md"), "x").unwrap();
        let at = |path: &Path| path.to_str().unwrap().to_owned();

        for options in [SET, DRY] {
            let set = |path: &str| set_mapping(&store, data.path(), Some("c1"), path, options);
            assert_eq!(reason(set("relative/path")), "not_absolute");
            assert_eq!(
                reason(set(&at(&folders.path().join("missing")))),
                "not_found"
            );
            assert_eq!(
                reason(set(&at(&folders.path().join("file.md")))),
                "not_a_directory"
            );
            assert_eq!(reason(set(&at(data.path()))), "inside_data_directory");
            assert!(store.folder_mappings().unwrap().is_empty());
        }

        let planned = set_mapping(&store, data.path(), Some("c1"), &at(&a), SET).unwrap();
        assert_eq!(planned.direction, Direction::Export);
        assert_eq!(planned.report, None);
        assert_eq!(PathBuf::from(&planned.root), a.canonicalize().unwrap());
        assert_eq!(
            set_mapping(&store, data.path(), Some("c1"), &at(&a), SET)
                .unwrap()
                .direction,
            Direction::Unchanged
        );
        for overlapping in [a.clone(), a.join("inner"), folders.path().to_owned()] {
            for (collection, options) in [(Some("c2"), SET), (Some("c2"), DRY), (None, DRY)] {
                assert_eq!(
                    reason(set_mapping(
                        &store,
                        data.path(),
                        collection,
                        &at(&overlapping),
                        options
                    )),
                    "overlapping_mapping"
                );
            }
        }
        assert_eq!(store.folder_mappings().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_root_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let locked = folder.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok();
        let result = set_mapping(
            &store(data.path()),
            data.path(),
            Some("c1"),
            locked.to_str().unwrap(),
            SET,
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            // Root can read any folder, so the refusal is asserted only where it applies.
            assert_eq!(reason(result), "unreadable");
        }
    }

    #[test]
    fn a_folder_with_files_imports_into_an_empty_collection() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = store(data.path());
        std::fs::write(folder.path().join("Plan.md"), "# Plan\n").unwrap();
        let planned = set_mapping(
            &store,
            data.path(),
            Some("c1"),
            folder.path().to_str().unwrap(),
            SET,
        )
        .unwrap();
        assert_eq!(planned.direction, Direction::Import);
    }

    #[test]
    fn a_dry_run_reports_and_stores_nothing() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let root = folder.path();
        std::fs::write(root.join("Plan.md"), "# Plan\n\nText.\n").unwrap();
        std::fs::write(root.join("Wrapped.md"), "Title\n=====\n\nOne\nline.\n").unwrap();
        std::fs::write(
            root.join("Lossy.md"),
            "Text.\n\n[unused]: https://example.com\n",
        )
        .unwrap();
        std::fs::write(root.join("Binary.md"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(root.join("notes.txt"), "skipped").unwrap();
        std::fs::create_dir(root.join("archive")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Plan.md"), root.join("Link.md")).unwrap();

        let planned = set_mapping(&store, data.path(), None, root.to_str().unwrap(), DRY).unwrap();
        assert_eq!(planned.direction, Direction::Import);
        let report = planned.report.unwrap();
        assert_eq!(
            report
                .files
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["Plan.md", "Wrapped.md"]
        );
        assert_eq!(report.files[0].1, 14);
        assert_eq!(report.reformatted, ["Wrapped.md"]);
        assert_eq!(report.subfolders, ["archive"]);
        assert_eq!(report.git_root, None);
        let refused: Vec<(&str, &str)> = report
            .refused
            .iter()
            .map(|refused| (refused.name.as_str(), refused.reason))
            .collect();
        let mut expected = vec![("Binary.md", "not_utf8"), ("Lossy.md", "lossy")];
        if cfg!(unix) {
            expected.insert(1, ("Link.md", "symlink"));
        }
        assert_eq!(refused, expected);
        let lossy = report
            .refused
            .iter()
            .find(|refused| refused.reason == "lossy")
            .unwrap();
        assert!(lossy.detail.as_deref().unwrap().contains("example"));

        let value = report.to_json();
        assert_eq!(value["counts"]["files"], 2);
        assert_eq!(value["refused"][0]["name"], "Binary.md");
        assert_eq!(value["gitRoot"], JsonValue::Null);
        assert!(
            store.folder_mappings().unwrap().is_empty(),
            "nothing stored"
        );
        assert_eq!(
            std::fs::read(root.join("Lossy.md")).unwrap(),
            b"Text.\n\n[unused]: https://example.com\n",
            "nothing written"
        );
    }

    #[test]
    fn a_git_working_tree_needs_a_confirmation() {
        let data = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let store = store(data.path());
        std::fs::create_dir(repository.path().join(".git")).unwrap();
        let notes = repository.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let path = notes.to_str().unwrap();
        let git_root = repository
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let report = set_mapping(&store, data.path(), Some("c1"), path, DRY)
            .unwrap()
            .report
            .unwrap();
        assert_eq!(report.git_root.as_deref(), Some(git_root.as_str()));

        let cause = set_mapping(&store, data.path(), Some("c1"), path, SET).unwrap_err();
        assert_eq!(cause.details()["reason"], "git_working_tree");
        assert_eq!(cause.details()["gitRoot"], git_root.as_str());
        assert!(store.folder_mappings().unwrap().is_empty());

        let confirmed = SetOptions {
            dry_run: false,
            confirm_git: true,
        };
        set_mapping(&store, data.path(), Some("c1"), path, confirmed).unwrap();
        assert_eq!(store.folder_mappings().unwrap().len(), 1);
    }

    #[test]
    fn a_new_collection_is_checked_but_not_stored() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = store(data.path());
        std::fs::write(folder.path().join("Plan.md"), "# Plan\n").unwrap();
        let planned = set_mapping(
            &store,
            data.path(),
            None,
            folder.path().to_str().unwrap(),
            SET,
        )
        .unwrap();
        assert_eq!(planned.direction, Direction::Import);
        assert!(store.folder_mappings().unwrap().is_empty());
    }

    #[test]
    fn case_only_duplicates_are_reported_as_collisions() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("Plan.md"), "one\n").unwrap();
        // On a case-insensitive file system this replaces the first file.
        std::fs::write(folder.path().join("plan.md"), "two\n").unwrap();
        let report = set_mapping(
            &store(data.path()),
            data.path(),
            None,
            folder.path().to_str().unwrap(),
            DRY,
        )
        .unwrap()
        .report
        .unwrap();
        let names = std::fs::read_dir(folder.path()).unwrap().count();
        if names == 2 {
            assert_eq!(report.files.len(), 1);
            assert_eq!(report.refused[0].reason, "name_collision_unresolved");
        } else {
            assert_eq!(report.files.len(), 1);
            assert!(report.refused.is_empty());
        }
    }

    #[test]
    fn the_listing_shows_every_collection_and_its_mapping() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder.path().join("sub")).unwrap();
        let store = store(data.path());
        assert_eq!(list_mappings(&store).unwrap()["collections"], json!([]));
        set_mapping(
            &store,
            data.path(),
            Some("c1"),
            folder.path().to_str().unwrap(),
            SET,
        )
        .unwrap();
        store
            .set_refused_file(&crate::store::mappings::RefusedFile {
                collection_id: "c1".to_owned(),
                rel_path: "Lossy.md".to_owned(),
                refused_hash: Some([1; 32]),
                reason: "lossy".to_owned(),
                detail: Some("unused".to_owned()),
            })
            .unwrap();
        // Without a Collection record, the mapping is not listed: the listing follows
        // Collections. The Backend request tests cover the whole listing.
        assert_eq!(list_mappings(&store).unwrap()["collections"], json!([]));
        assert!(clear_mapping(&store, "c1").unwrap());
        assert!(!clear_mapping(&store, "c1").unwrap());
        assert!(store.refused_files("c1").unwrap().is_empty());
    }
}

/// The `documents.mapping.*` requests through the Backend.
#[cfg(test)]
mod request_tests {
    use serde_json::{Value as JsonValue, json};

    use super::super::reconcile::tests::remembering_log;
    use crate::backend::tests::{Session, qualify_empty_history, start_without_folder_sync};

    async fn request(session: &mut Session, operation: &str, payload: JsonValue) -> JsonValue {
        session
            .harness
            .request(operation, payload)
            .await
            .unwrap_or_else(|cause| panic!("{operation}: {}", cause.code()))
    }

    async fn refused(session: &mut Session, payload: JsonValue) -> String {
        let cause = session
            .harness
            .request("documents.mapping.set", payload)
            .await
            .unwrap_err();
        assert_eq!(cause.code(), "documents.mapping_refused");
        cause.details()["reason"].as_str().unwrap().to_owned()
    }

    async fn collection(session: &mut Session, name: &str) -> String {
        request(
            session,
            "documents.collection.create",
            json!({ "name": name }),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn started(data: &std::path::Path) -> Session {
        let session = start_without_folder_sync(data, remembering_log()).await;
        qualify_empty_history(&session).await;
        session
    }

    // Verifies: slice B B-1, B-3 — every refusal reason of `documents.mapping.set`.
    #[tokio::test]
    async fn set_refuses_each_unsafe_or_conflicting_folder() {
        let data = tempfile::tempdir().unwrap();
        let folders = tempfile::tempdir().unwrap();
        let mut session = started(data.path()).await;
        let plans = collection(&mut session, "Plans").await;
        let other = collection(&mut session, "Other").await;
        let at = |path: &std::path::Path| path.to_str().unwrap().to_owned();
        let mapped = folders.path().join("mapped");
        let full = folders.path().join("full");
        let repository = folders.path().join("repository");
        for folder in [&mapped, &full, &repository.join("notes")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(full.join("Plan.md"), "# Plan\n").unwrap();
        std::fs::create_dir(repository.join(".git")).unwrap();
        std::fs::write(folders.path().join("file.md"), "x").unwrap();
        request(
            &mut session,
            "documents.mapping.set",
            json!({ "collectionId": other, "path": at(&mapped) }),
        )
        .await;
        request(
            &mut session,
            "documents.document.create",
            json!({ "collectionId": plans, "title": "Plan" }),
        )
        .await;

        for (path, reason) in [
            ("relative/folder".to_owned(), "not_absolute"),
            (at(&folders.path().join("missing")), "not_found"),
            (at(&folders.path().join("file.md")), "not_a_directory"),
            (at(data.path()), "inside_data_directory"),
            (at(&mapped), "overlapping_mapping"),
            (at(folders.path()), "overlapping_mapping"),
            (at(&full), "not_empty"),
            (at(&repository.join("notes")), "git_working_tree"),
        ] {
            assert_eq!(
                refused(&mut session, json!({ "collectionId": plans, "path": path })).await,
                reason,
                "{path}"
            );
        }
        // The git rule gives the working tree's root, and a confirmation maps the folder.
        let cause = session
            .harness
            .request(
                "documents.mapping.set",
                json!({ "collectionId": plans, "path": at(&repository.join("notes")) }),
            )
            .await
            .unwrap_err();
        assert_eq!(
            cause.details()["gitRoot"],
            at(&repository.canonicalize().unwrap())
        );
        let confirmed = request(
            &mut session,
            "documents.mapping.set",
            json!({ "collectionId": plans, "path": at(&repository.join("notes")), "confirmGit": true }),
        )
        .await;
        assert_eq!(confirmed["direction"], "export");
        assert_eq!(confirmed["dryRun"], false);

        // A request names exactly one Collection, and only a known one.
        for payload in [
            json!({ "path": at(&mapped) }),
            json!({ "collectionId": plans, "newCollectionName": "Both", "path": at(&mapped) }),
        ] {
            let cause = session
                .harness
                .request("documents.mapping.set", payload)
                .await
                .unwrap_err();
            assert_eq!(cause.code(), "documents.invalid_request");
        }
        let cause = session
            .harness
            .request(
                "documents.mapping.set",
                json!({ "collectionId": "f".repeat(64), "path": at(&mapped) }),
            )
            .await
            .unwrap_err();
        assert_eq!(cause.code(), "documents.unknown_collection");
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: slice B B-2 — a dry run reports and changes nothing; B-1 — "Create Collection
    // from folder"; the listing and Unmap.
    #[tokio::test]
    async fn preview_create_list_and_clear() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let mut session = started(data.path()).await;
        std::fs::write(root.join("Plan.md"), "# Plan\n\nText.\n").unwrap();
        std::fs::write(root.join("Wrapped.md"), "Title\n=====\n").unwrap();
        std::fs::write(
            root.join("Lossy.md"),
            "Text.\n\n[unused]: https://example.com\n",
        )
        .unwrap();
        std::fs::create_dir(root.join("archive")).unwrap();
        let path = root.to_str().unwrap();

        let preview = request(
            &mut session,
            "documents.mapping.set",
            json!({ "newCollectionName": "Planning", "path": path, "dryRun": true }),
        )
        .await;
        assert_eq!(preview["collectionId"], JsonValue::Null);
        assert_eq!(preview["direction"], "import");
        assert_eq!(preview["dryRun"], true);
        let report = &preview["report"];
        assert_eq!(
            report["files"],
            json!([{ "name": "Plan.md", "bytes": 14 }, { "name": "Wrapped.md", "bytes": 12 }])
        );
        assert_eq!(report["refused"][0]["name"], "Lossy.md");
        assert_eq!(report["refused"][0]["reason"], "lossy");
        assert_eq!(report["reformatted"], json!(["Wrapped.md"]));
        assert_eq!(report["subfolders"], json!(["archive"]));
        assert_eq!(report["gitRoot"], JsonValue::Null);
        assert_eq!(
            report["counts"],
            json!({ "files": 2, "refused": 1, "reformatted": 1, "subfolders": 1 })
        );
        assert!(session.handles.store.collections().unwrap().is_empty());
        assert!(session.handles.store.folder_mappings().unwrap().is_empty());

        let created = request(
            &mut session,
            "documents.mapping.set",
            json!({ "newCollectionName": "Planning", "path": path }),
        )
        .await;
        assert_eq!(created["direction"], "import");
        let collection = created["collectionId"].as_str().unwrap().to_owned();
        assert_eq!(
            session.handles.store.collections().unwrap()[0]
                .name
                .as_deref(),
            Some("Planning")
        );
        session
            .handles
            .store
            .set_refused_file(&crate::store::mappings::RefusedFile {
                collection_id: collection.clone(),
                rel_path: "Lossy.md".to_owned(),
                refused_hash: Some([1; 32]),
                reason: "lossy".to_owned(),
                detail: Some("https example com".to_owned()),
            })
            .unwrap();

        let empty = self::collection(&mut session, "Empty").await;
        let listed = request(&mut session, "documents.mapping.list", json!({})).await;
        let canonical = root.canonicalize().unwrap();
        assert_eq!(
            listed["collections"],
            json!([
                {
                    "collectionId": collection, "name": "Planning", "archived": false,
                    "mapping": {
                        "path": canonical.to_str().unwrap(), "state": "active",
                        "counts": { "files": 0, "refused": 1, "subfolders": 1 },
                        "refused": [{ "name": "Lossy.md", "reason": "lossy", "detail": "https example com" }],
                        "subfolders": ["archive"], "gitRoot": null,
                    },
                },
                { "collectionId": empty, "name": "Empty", "archived": false, "mapping": null },
            ])
        );

        // A new Collection with a refused folder is not created.
        let cause = session
            .harness
            .request(
                "documents.mapping.set",
                json!({ "newCollectionName": "Again", "path": path }),
            )
            .await
            .unwrap_err();
        assert_eq!(cause.details()["reason"], "overlapping_mapping");
        assert_eq!(session.handles.store.collections().unwrap().len(), 2);

        let cleared = request(
            &mut session,
            "documents.mapping.clear",
            json!({ "collectionId": collection }),
        )
        .await;
        assert_eq!(
            cleared,
            json!({ "collectionId": collection, "cleared": true })
        );
        assert!(session.handles.store.folder_mappings().unwrap().is_empty());
        assert!(root.join("Plan.md").is_file(), "Unmap leaves the files");
        let again = request(
            &mut session,
            "documents.mapping.clear",
            json!({ "collectionId": collection }),
        )
        .await;
        assert_eq!(again["cleared"], false);
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: slice B B-1 — the settings form's draft survives a Package switch.
    #[tokio::test]
    async fn the_mapping_draft_is_remembered_in_memory() {
        let data = tempfile::tempdir().unwrap();
        let mut session = started(data.path()).await;
        assert_eq!(
            request(&mut session, "documents.mapping.draft.get", json!({})).await,
            json!({})
        );
        let draft = json!({ "path": "/Users/me/notes", "name": "Notes" });
        request(&mut session, "documents.mapping.draft.set", draft.clone()).await;
        assert_eq!(
            request(&mut session, "documents.mapping.draft.get", json!({})).await,
            draft
        );
        session.harness.shutdown().await.unwrap();
    }
}
