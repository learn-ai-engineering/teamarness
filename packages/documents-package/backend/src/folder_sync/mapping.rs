//! Making, listing and clearing mappings: the root checks, the mapping rules, the git rule and
//! the pre-mapping report of `documents.mapping.set`, and the bodies of `documents.mapping.list`
//! and `documents.mapping.clear`. Existing mappings opt in through a read-only incremental
//! preview and gate-protected `documents.mapping.enable-recursive` confirmation.
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
use super::codec::{EXTENSION, file_name, git_ancestor, name_key, preview_folder, resolve};
use super::reconcile::folder_of;
use super::reconcile::{Refusal, precheck, read_file};
use super::scan::{Scan, scan, subfolders};
use crate::markdown;
use crate::store::mappings::{FolderMapping, MappingState};
use crate::store::{DocumentStore, StoreError};

/// The most entries each list of a report or a listing carries. The counts are always whole.
pub(crate) const MAX_LISTED: usize = 200;

/// The most characters of a refusal's detail that a report or a listing carries.
const MAX_DETAIL_CHARS: usize = 200;

/// Leaves room for the response envelope within the UI contract's 256 KiB payload limit.
const MAX_REPORT_BYTES: usize = 128 * 1024;

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
    /// Folders excluded by an existing flat mapping.
    pub subfolders: Vec<String>,
    /// The nearest folder at or above the root that holds `.git`.
    pub git_root: Option<String>,
    pub relocations: Vec<(String, Option<String>, String)>,
    pub blocked: bool,
}

impl Report {
    pub(crate) fn to_json(&self) -> JsonValue {
        let mut value = json!({
            "counts": {
                "files": self.files.len(),
                "refused": self.refused.len(),
                "reformatted": self.reformatted.len(),
                "subfolders": self.subfolders.len(),
                "relocations": self.relocations.len(),
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
            "blocked": self.blocked,
            "relocations": self.relocations.iter().take(MAX_LISTED)
                .map(|(document, from, to)| json!({"documentId":document,"from":from,"to":to})).collect::<Vec<_>>(),
        });
        let mut bytes = serde_json::to_vec(&value)
            .expect("report contains only JSON values")
            .len();
        while bytes > MAX_REPORT_BYTES {
            for name in [
                "files",
                "refused",
                "reformatted",
                "relocations",
                "subfolders",
            ] {
                let list = value[name].as_array_mut().expect("report list");
                if let Some(last) = list.pop() {
                    bytes -= serde_json::to_vec(&last).expect("report entry").len()
                        + usize::from(!list.is_empty());
                }
                if bytes <= MAX_REPORT_BYTES {
                    break;
                }
            }
        }
        value
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
    let recursive = !unchanged
        || mappings
            .iter()
            .any(|mapping| Some(mapping.collection_id.as_str()) == collection && mapping.recursive);
    let scanned = scan(&root, recursive.into()).map_err(|_| refused("unreadable"))?;
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
            recursive: true,
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

/// Called only under the reconciler gate. Preview and validation perform no writes.
pub(super) fn enable_recursive(
    store: &DocumentStore,
    collection: &str,
    options: SetOptions,
) -> Result<JsonValue, BackendError> {
    let mapping = store
        .folder_mapping(collection)
        .map_err(|_| storage_failed())?
        .ok_or_else(|| refused("not_mapped"))?;
    if mapping.recursive {
        return Ok(
            json!({"collectionId":collection,"dryRun":options.dry_run,"recursive":true,"changed":false}),
        );
    }
    let root = Path::new(&mapping.root_path);
    let scanned =
        scan(root, super::scan::ScanMode::Recursive).map_err(|_| refused("unreadable"))?;
    let files = store
        .mapped_files(collection)
        .map_err(|_| storage_failed())?;
    let mut incremental = scanned.clone();
    incremental.files.retain(|candidate| {
        candidate.rel.contains('/')
            && !files
                .iter()
                .any(|file| file.name_key == name_key(&candidate.rel))
    });
    let mut report = report(
        root,
        &incremental,
        git_ancestor(root).map(|p| p.to_string_lossy().into_owned()),
    );
    let mut destinations = HashSet::<String>::new();
    let mut folders = HashMap::<String, String>::new();
    for document in store
        .documents()
        .map_err(|_| storage_failed())?
        .into_iter()
        .filter(|document| document.collection_id == collection && !document.archived)
    {
        let file = files.iter().find(|file| file.document_id == document.id);
        let current_folder = file.map(|file| folder_of(&file.rel_path)).unwrap_or("");
        if crate::domain::normalize_folder(current_folder)
            .ok()
            .as_deref()
            == Some(document.folder.as_str())
        {
            continue;
        }
        let planned = preview_folder(root, &document.folder);
        let folder = planned
            .as_ref()
            .cloned()
            .unwrap_or_else(|_| document.folder.clone());
        let name = file_name(document.title.as_deref().unwrap_or("Untitled"));
        let to = if folder.is_empty() {
            name
        } else {
            format!("{folder}/{name}")
        };
        report.relocations.push((
            document.id,
            file.map(|file| file.rel_path.clone()),
            to.clone(),
        ));
        let source_unsafe = file.is_some_and(|file| {
            scanned.protects(&file.rel_path)
                || resolve(root, Path::new(&file.rel_path)).is_err()
                || !read_file(&root.join(&file.rel_path)).is_ok_and(|content| content.is_some())
        });
        let key = name_key(&to);
        let mut collision = destinations.iter().any(|other| {
            other == &key
                || other.starts_with(&format!("{key}/"))
                || key.starts_with(&format!("{other}/"))
        }) || files.iter().any(|other| other.name_key == key);
        destinations.insert(key);
        let mut prefix = String::new();
        for part in document.folder.split('/').filter(|part| !part.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(previous) = folders.insert(name_key(&prefix), prefix.clone()) {
                collision |= previous != prefix;
            }
        }
        let target = root.join(&to);
        let occupied = planned.is_ok()
            && !scanned.protects(&to)
            && match target.parent().map(std::fs::read_dir) {
                Some(Ok(entries)) => {
                    entries
                        .collect::<Result<Vec<_>, _>>()
                        .map_or(true, |entries| {
                            entries.iter().any(|entry| {
                                name_key(&entry.file_name().to_string_lossy())
                                    == name_key(
                                        target
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .as_ref(),
                                    )
                            })
                        })
                }
                Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => false,
                _ => true,
            };
        if planned.is_err() || scanned.protects(&to) || source_unsafe || collision || occupied {
            report.blocked = true;
            report.refused.push(ReportRefusal {
                name: to,
                reason: if collision || occupied {
                    "name_collision_unresolved"
                } else {
                    "invalid_folder"
                },
                detail: Some("Required relocation is blocked; subfolders remain disabled".into()),
            });
        }
    }
    if options.dry_run {
        return Ok(
            json!({"collectionId":collection,"dryRun":true,"recursive":false,"changed":false,"report":report.to_json()}),
        );
    }
    if report.blocked {
        return Err(refused_with(
            "relocation_blocked",
            json!({"path":report.refused.last().map(|refused| &refused.name)}),
        ));
    }
    if report.git_root.is_some() && !options.confirm_git {
        return Err(refused_with(
            "git_working_tree",
            json!({"gitRoot":report.git_root}),
        ));
    }
    if !store
        .enable_recursive_mapping(collection, &mapping.root_path)
        .map_err(|_| storage_failed())?
    {
        return Err(refused("not_mapped"));
    }
    Ok(json!({"collectionId":collection,"dryRun":false,"recursive":true,"changed":true}))
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
    for name in scanned.unreadable.iter().chain(&scanned.evicted) {
        report.refused.push(ReportRefusal {
            name: name.clone(),
            reason: "unreadable",
            detail: None,
        });
    }
    for (name, reason) in &scanned.protected {
        if report.refused.iter().any(|refused| refused.name == *name) {
            continue;
        }
        report.refused.push(ReportRefusal {
            name: name.clone(),
            reason: match reason.as_str() {
                "symlink" => "symlink",
                "name_collision_unresolved" => "name_collision_unresolved",
                "invalid_folder" => "invalid_folder",
                _ => "unreadable",
            },
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
                    .map_err(|_| storage_failed())?;
                let pending = if mapping.recursive {
                    0
                } else {
                    store
                        .documents()
                        .map_err(|_| storage_failed())?
                        .iter()
                        .filter(|document| {
                            document.collection_id == collection.id && !document.archived
                        })
                        .filter(|document| {
                            let folder = files
                                .iter()
                                .find(|file| file.document_id == document.id)
                                .map(|file| folder_of(&file.rel_path))
                                .unwrap_or("");
                            crate::domain::normalize_folder(folder).ok().as_deref()
                                != Some(document.folder.as_str())
                        })
                        .count()
                };
                let refused = store
                    .refused_files(&collection.id)
                    .map_err(|_| storage_failed())?;
                let root = Path::new(&mapping.root_path);
                let folders = if mapping.recursive {
                    Vec::new()
                } else {
                    subfolders(root)
                };
                json!({
                    "path": mapping.root_path,
                    "recursive": mapping.recursive,
                    "pendingFolderChanges": pending,
                    "state": mapping.state.as_str(),
                    "counts": {
                        "files": files.len(),
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
        assert!(report.subfolders.is_empty());
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
    fn a_mapping_without_a_collection_is_not_listed_and_clears_once() {
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

    // Verifies: TEAM.SYS.DOCS-R14 — new mappings include nested files and legacy listings expose consent.
    #[tokio::test]
    async fn recursive_mapping_creation_and_legacy_listing() {
        use crate::store::mappings::{FolderMapping, MappingState};
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::create_dir(root.join("Nested")).unwrap();
        std::fs::write(root.join("Nested/Note.md"), "Nested.\n").unwrap();
        let mut session = started(data.path()).await;
        let collection = collection(&mut session, "Plans").await;
        let preview = request(
            &mut session,
            "documents.mapping.set",
            json!({"collectionId":collection,"path":root.to_str().unwrap(),"dryRun":true}),
        )
        .await;
        assert_eq!(preview["direction"], "import");
        assert_eq!(
            preview["report"]["files"],
            json!([{"name":"Nested/Note.md","bytes":8}])
        );
        assert_eq!(preview["report"]["subfolders"], json!([]));
        request(
            &mut session,
            "documents.mapping.set",
            json!({"collectionId":collection,"path":root.to_str().unwrap()}),
        )
        .await;
        assert!(
            session
                .handles
                .store
                .folder_mapping(&collection)
                .unwrap()
                .unwrap()
                .recursive
        );
        request(
            &mut session,
            "documents.document.create",
            json!({"collectionId":collection,"title":"Remote","folder":"Waiting"}),
        )
        .await;
        session
            .handles
            .store
            .set_folder_mapping(&FolderMapping {
                collection_id: collection.clone(),
                root_path: root.to_str().unwrap().into(),
                state: MappingState::Active,
                created_at: 1,
                recursive: false,
            })
            .unwrap();
        let listed = request(&mut session, "documents.mapping.list", json!({})).await;
        assert_eq!(listed["collections"][0]["mapping"]["recursive"], false);
        assert_eq!(
            listed["collections"][0]["mapping"]["pendingFolderChanges"],
            1
        );
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("Nested")).unwrap();
        std::fs::write(other.path().join("Nested/Other.md"), "Other.\n").unwrap();
        assert_eq!(refused(&mut session, json!({"collectionId":collection,"path":other.path().to_str().unwrap(),"dryRun":true})).await, "not_empty");
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14 — bounded reports keep full counts, including evictions.
    #[tokio::test]
    async fn enable_recursive_report_bounds_and_evictions() {
        use crate::store::mappings::{FolderMapping, MappingState};
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let nested = format!("{}/{}/{}", "A".repeat(200), "B".repeat(200), "C".repeat(80));
        std::fs::create_dir_all(root.join(&nested)).unwrap();
        for n in 0..205 {
            std::fs::write(root.join(format!("{nested}/{n}.md")), "Title\n=====\n").unwrap();
            std::fs::write(root.join(format!("{nested}/Bad-{n}.md")), [0xff]).unwrap();
        }
        std::fs::write(root.join(format!("{nested}/.AaEvicted.md.icloud")), "").unwrap();
        let mut session = started(data.path()).await;
        let collection = collection(&mut session, "Plans").await;
        session
            .handles
            .store
            .set_folder_mapping(&FolderMapping {
                collection_id: collection.clone(),
                root_path: root.to_str().unwrap().into(),
                state: MappingState::Active,
                created_at: 1,
                recursive: false,
            })
            .unwrap();
        for n in 0..205 {
            request(
                &mut session,
                "documents.document.create",
                json!({"collectionId":collection,"title":"Export","folder":format!("Future/{n}")}),
            )
            .await;
        }
        let preview = request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection,"dryRun":true}),
        )
        .await;
        let report = &preview["report"];
        assert_eq!(report["counts"]["files"], 205);
        assert_eq!(report["counts"]["reformatted"], 205);
        assert!((1..=200).contains(&report["files"].as_array().unwrap().len()));
        assert!((1..=200).contains(&report["reformatted"].as_array().unwrap().len()));
        assert_eq!(report["counts"]["refused"], 206);
        assert!((1..=200).contains(&report["refused"].as_array().unwrap().len()));
        assert!(
            report["refused"]
                .as_array()
                .unwrap()
                .iter()
                .any(|file| file["name"] == format!("{nested}/AaEvicted.md")
                    && file["reason"] == "unreadable")
        );
        assert_eq!(report["counts"]["relocations"], 205);
        assert!((1..=200).contains(&report["relocations"].as_array().unwrap().len()));
        let contract = teamarness_package_contract::ui_contract::compile_ui_contract(
            include_bytes!("../../../ui-contract.json"),
        )
        .unwrap();
        contract
            .validate_response("documents.mapping.enable-recursive", &preview)
            .unwrap();
        assert!(
            contract
                .validate_request(
                    "documents.mapping.enable-recursive",
                    &json!({"collectionId":collection,"extra":true})
                )
                .is_err()
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14 — planned paths cannot alias directories or become file parents.
    #[tokio::test]
    async fn enable_recursive_refuses_collisions_between_planned_relocations() {
        use crate::store::mappings::{FolderMapping, MappingState};
        for placements in [
            [("One", "A"), ("Two", "a")],
            [("Note", "A"), ("Two", "A/Note.md")],
        ] {
            let data = tempfile::tempdir().unwrap();
            let folder = tempfile::tempdir().unwrap();
            let mut session = started(data.path()).await;
            let collection = collection(&mut session, "Plans").await;
            for (title, path) in placements {
                request(
                    &mut session,
                    "documents.document.create",
                    json!({"collectionId":collection,"title":title,"folder":path}),
                )
                .await;
            }
            session
                .handles
                .store
                .set_folder_mapping(&FolderMapping {
                    collection_id: collection.clone(),
                    root_path: folder.path().to_str().unwrap().into(),
                    state: MappingState::Active,
                    created_at: 1,
                    recursive: false,
                })
                .unwrap();
            let preview = request(
                &mut session,
                "documents.mapping.enable-recursive",
                json!({"collectionId":collection,"dryRun":true}),
            )
            .await;
            assert_eq!(preview["report"]["blocked"], true, "{placements:?}");
            let error = session
                .harness
                .request(
                    "documents.mapping.enable-recursive",
                    json!({"collectionId":collection}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.details()["reason"], "relocation_blocked");
            assert!(
                !session
                    .handles
                    .store
                    .folder_mapping(&collection)
                    .unwrap()
                    .unwrap()
                    .recursive
            );
            assert!(std::fs::read_dir(folder.path()).unwrap().next().is_none());
            session.harness.shutdown().await.unwrap();
        }
    }

    // Verifies: TEAM.SYS.DOCS-R14 — confirmation rechecks current paths without partial activation.
    #[cfg(unix)]
    #[tokio::test]
    async fn enable_recursive_rechecks_collisions_aliases_symlinks_and_unreadable_paths() {
        use crate::store::mappings::{FolderMapping, MappingState};
        use std::os::unix::fs::{PermissionsExt, symlink};
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = folder.path();
        let mut session = started(data.path()).await;
        let collection = collection(&mut session, "Plans").await;
        request(
            &mut session,
            "documents.document.create",
            json!({"collectionId":collection,"title":"Remote","folder":"Target/deep"}),
        )
        .await;
        session
            .handles
            .store
            .set_folder_mapping(&FolderMapping {
                collection_id: collection.clone(),
                root_path: root.to_str().unwrap().into(),
                state: MappingState::Active,
                created_at: 1,
                recursive: false,
            })
            .unwrap();
        let preview = request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection,"dryRun":true}),
        )
        .await;
        assert_eq!(preview["report"]["blocked"], false);
        assert!(!root.join("Target").exists());
        std::fs::create_dir_all(root.join("Target/deep")).unwrap();
        std::fs::write(root.join("Target/deep/Remote.md"), "Unrelated.\n").unwrap();
        for kind in ["occupied", "symlink", "case_alias", "unreadable"] {
            if kind != "occupied" {
                std::fs::remove_dir_all(root.join("Target")).unwrap();
                if kind == "symlink" {
                    symlink(outside.path(), root.join("Target")).unwrap();
                }
                if kind == "case_alias" {
                    std::fs::create_dir(root.join("target")).unwrap();
                }
                if kind == "unreadable" {
                    std::fs::create_dir(root.join("Target")).unwrap();
                    std::fs::set_permissions(
                        root.join("Target"),
                        std::fs::Permissions::from_mode(0o000),
                    )
                    .unwrap();
                }
            }
            let can_read = kind == "unreadable" && std::fs::read_dir(root.join("Target")).is_ok();
            if !can_read {
                let refused = session
                    .harness
                    .request(
                        "documents.mapping.enable-recursive",
                        json!({"collectionId":collection}),
                    )
                    .await
                    .unwrap_err();
                assert_eq!(refused.details()["reason"], "relocation_blocked", "{kind}");
                assert!(
                    !session
                        .handles
                        .store
                        .folder_mapping(&collection)
                        .unwrap()
                        .unwrap()
                        .recursive
                );
            }
            if kind == "occupied" {
                assert_eq!(
                    std::fs::read(root.join("Target/deep/Remote.md")).unwrap(),
                    b"Unrelated.\n"
                );
            }
            if kind == "symlink" {
                std::fs::remove_file(root.join("Target")).unwrap();
                std::fs::create_dir(root.join("Target")).unwrap();
                assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
            }
            if kind == "case_alias" {
                std::fs::remove_dir(root.join("target")).unwrap();
                std::fs::create_dir(root.join("Target")).unwrap();
            }
            if kind == "unreadable" {
                std::fs::set_permissions(
                    root.join("Target"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
        }
        std::fs::create_dir(root.join(".git")).unwrap();
        let git = session
            .harness
            .request(
                "documents.mapping.enable-recursive",
                json!({"collectionId":collection}),
            )
            .await
            .unwrap_err();
        assert_eq!(git.details()["reason"], "git_working_tree");
        request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection,"confirmGit":true}),
        )
        .await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14 — incremental consent preserves durable root associations.
    #[tokio::test]
    async fn enable_recursive_preview_is_incremental_and_preserves_all_local_state() {
        use crate::store::mappings::{FolderMapping, MappedFile, MappingState, RefusedFile};
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let mut session = started(data.path()).await;
        let collection = collection(&mut session, "Plans").await;
        let document = request(
            &mut session,
            "documents.document.create",
            json!({"collectionId":collection,"title":"Root","folder":"Moved/deep"}),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let store = session.handles.store.clone();
        let mapping = FolderMapping {
            collection_id: collection.clone(),
            root_path: root.to_str().unwrap().into(),
            state: MappingState::Active,
            created_at: 1,
            recursive: false,
        };
        store.set_folder_mapping(&mapping).unwrap();
        std::fs::write(root.join("Root.md"), "Root bytes.\n").unwrap();
        std::fs::create_dir(root.join("Nested")).unwrap();
        std::fs::write(root.join("Nested/New.md"), "Title\n=====\n").unwrap();
        std::fs::write(root.join("Nested/Bad.md"), [0xff]).unwrap();
        let file = MappedFile {
            document_id: document.clone(),
            collection_id: collection.clone(),
            rel_path: "Root.md".into(),
            name_key: "root.md".into(),
            base_version: Some("base".into()),
            written_hash: Some([1; 32]),
            pending_write_hash: Some([2; 32]),
            pending_write_version: Some("pending".into()),
        };
        store.upsert_mapped_file(&file).unwrap();
        let refusal = RefusedFile {
            collection_id: collection.clone(),
            rel_path: "Old.md".into(),
            refused_hash: Some([3; 32]),
            reason: "lossy".into(),
            detail: Some("kept".into()),
        };
        store.set_refused_file(&refusal).unwrap();
        let calls = session.scripted.recorded_calls().len();
        let preview = request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection,"dryRun":true}),
        )
        .await;
        assert_eq!(
            preview["report"]["files"],
            json!([{"name":"Nested/New.md","bytes":12}])
        );
        assert_eq!(preview["report"]["reformatted"], json!(["Nested/New.md"]));
        assert_eq!(preview["report"]["refused"][0]["name"], "Nested/Bad.md");
        assert_eq!(
            preview["report"]["relocations"],
            json!([{"documentId":document,"from":"Root.md","to":"Moved/deep/Root.md"}])
        );
        assert_eq!(session.scripted.recorded_calls().len(), calls);
        assert_eq!(store.folder_mapping(&collection).unwrap(), Some(mapping));
        assert_eq!(store.mapped_files(&collection).unwrap(), vec![file.clone()]);
        assert_eq!(
            store.refused_files(&collection).unwrap(),
            vec![refusal.clone()]
        );
        assert!(!root.join("Moved").exists());
        assert_eq!(
            std::fs::read(root.join("Root.md")).unwrap(),
            b"Root bytes.\n"
        );
        let confirmed = request(
            &mut session,
            "documents.mapping.enable-recursive",
            json!({"collectionId":collection}),
        )
        .await;
        assert_eq!(confirmed["changed"], true);
        assert!(
            store
                .folder_mapping(&collection)
                .unwrap()
                .unwrap()
                .recursive
        );
        assert_eq!(store.mapped_files(&collection).unwrap(), vec![file]);
        assert_eq!(store.refused_files(&collection).unwrap(), vec![refusal]);
        assert!(!root.join("Moved").exists(), "activation only commits mode");
        assert_eq!(
            request(
                &mut session,
                "documents.mapping.enable-recursive",
                json!({"collectionId":collection})
            )
            .await["changed"],
            false
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.DOCS-R14 (Configuration, Mapping rules) — every refusal reason of `documents.mapping.set`.
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

    // Verifies: TEAM.SYS.DOCS-R14 (Pre-mapping report) — a dry run reports and changes nothing; (Configuration) — "Create Collection
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
        assert_eq!(report["subfolders"], json!([]));
        assert_eq!(report["gitRoot"], JsonValue::Null);
        assert_eq!(
            report["counts"],
            json!({ "files": 2, "refused": 1, "reformatted": 1, "subfolders": 0, "relocations": 0 })
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
                        "path": canonical.to_str().unwrap(), "state": "active", "recursive":true, "pendingFolderChanges":0,
                        "counts": { "files": 0, "refused": 1, "subfolders": 0 },
                        "refused": [{ "name": "Lossy.md", "reason": "lossy", "detail": "https example com" }],
                        "subfolders": [], "gitRoot": null,
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

    // Verifies: TEAM.SYS.DOCS-R14 (Configuration) — the settings form's draft survives a Package switch.
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
