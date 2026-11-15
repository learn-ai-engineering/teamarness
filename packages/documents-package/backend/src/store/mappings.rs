//! Folder sync state: which folder each Collection maps to, the file of each mapped Document,
//! and the files folder sync refused.
//!
//! This state belongs to this Installation and is never replicated. Every write is one SQLite
//! transaction, so a crash leaves either the old state or the new one. The folder sync
//! reconciler and the `documents.mapping.*` requests use it.

use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

use super::{DocumentStore, StoreError, lock, rerr};

/// A SHA-256 digest of a file's bytes.
pub type FileHash = [u8; 32];

/// Whether folder sync works on a mapping now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingState {
    /// Folder sync reconciles the folder.
    Active,
    /// The root is missing or unreadable. Nothing is archived until it returns.
    Unavailable,
    /// The Collection is archived. The files stay, and the mapping resumes on restore.
    Paused,
}

impl MappingState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Unavailable => "unavailable",
            Self::Paused => "paused",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "active" => Ok(Self::Active),
            "unavailable" => Ok(Self::Unavailable),
            "paused" => Ok(Self::Paused),
            _ => Err(StoreError::Storage),
        }
    }
}

/// One Collection's mapped folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderMapping {
    pub collection_id: String,
    /// The absolute path of the mapped folder.
    pub root_path: String,
    pub state: MappingState,
    /// When the mapping was made, in Unix milliseconds.
    pub created_at: i64,
}

/// The file of one mapped Document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedFile {
    pub document_id: String,
    pub collection_id: String,
    /// The file's path relative to the mapping's root.
    pub rel_path: String,
    /// The case-folded file name, unique within one mapping.
    pub name_key: String,
    /// The version token the file was last synced at.
    pub base_version: Option<String>,
    /// The hash of the bytes last read from or written to the file.
    pub written_hash: Option<FileHash>,
    /// Set by [`DocumentStore::begin_write`] before a write-back renames the file.
    pub pending_write_hash: Option<FileHash>,
    pub pending_write_version: Option<String>,
}

/// A file folder sync did not import, with the reason the settings view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedFile {
    pub collection_id: String,
    pub rel_path: String,
    /// The hash of the refused bytes, so the same bytes are not retried until the file changes.
    pub refused_hash: Option<FileHash>,
    /// One of `lossy`, `too_large`, `not_utf8`, `schema_violation`, `merge_refused`, `symlink`,
    /// `title_too_long` or `name_collision_unresolved`.
    pub reason: String,
    pub detail: Option<String>,
}

const MAPPING_COLUMNS: &str = "collection_id,root_path,state,created_at";
const FILE_COLUMNS: &str = "document_id,collection_id,rel_path,name_key,base_version,written_hash,pending_write_hash,pending_write_version";

fn hash(value: Option<Vec<u8>>) -> Result<Option<FileHash>, StoreError> {
    value
        .map(|bytes| FileHash::try_from(bytes.as_slice()).map_err(|_| StoreError::Storage))
        .transpose()
}

fn mapping_row(row: &Row<'_>) -> rusqlite::Result<(String, String, String, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn mapping(
    (collection_id, root_path, state, created_at): (String, String, String, i64),
) -> Result<FolderMapping, StoreError> {
    Ok(FolderMapping {
        collection_id,
        root_path,
        state: MappingState::parse(&state)?,
        created_at,
    })
}

type FileRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<String>,
);

fn file_row(row: &Row<'_>) -> rusqlite::Result<FileRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn file(
    (
        document_id,
        collection_id,
        rel_path,
        name_key,
        base_version,
        written_hash,
        pending_write_hash,
        pending_write_version,
    ): FileRow,
) -> Result<MappedFile, StoreError> {
    Ok(MappedFile {
        document_id,
        collection_id,
        rel_path,
        name_key,
        base_version,
        written_hash: hash(written_hash)?,
        pending_write_hash: hash(pending_write_hash)?,
        pending_write_version,
    })
}

/// A unique or missing-parent violation is [`StoreError::Conflict`]; any other failure is
/// [`StoreError::Storage`].
fn write_error(error: rusqlite::Error) -> StoreError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::ConstraintViolation) => StoreError::Conflict,
        _ => StoreError::Storage,
    }
}

impl DocumentStore {
    /// The mapping of `collection_id`, if it has one.
    pub fn folder_mapping(&self, collection_id: &str) -> Result<Option<FolderMapping>, StoreError> {
        let inner = lock(&self.inner)?;
        let row = inner
            .conn
            .query_row(
                &format!("SELECT {MAPPING_COLUMNS} FROM folder_mappings WHERE collection_id=?1"),
                [collection_id],
                mapping_row,
            )
            .optional()
            .map_err(rerr)?;
        row.map(mapping).transpose()
    }

    /// Every mapping, oldest first.
    pub fn folder_mappings(&self) -> Result<Vec<FolderMapping>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare(&format!(
                "SELECT {MAPPING_COLUMNS} FROM folder_mappings ORDER BY created_at,collection_id"
            ))
            .map_err(rerr)?;
        let rows = statement
            .query_map([], mapping_row)
            .map_err(rerr)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(rerr)?;
        rows.into_iter().map(mapping).collect()
    }

    /// Saves `mapping`, keeping its file state unless its root changed. A new root starts with
    /// no file state and no refusals, because they describe another folder.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when another mapping already has this root.
    pub fn set_folder_mapping(&self, mapping: &FolderMapping) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let root: Option<String> = transaction
            .query_row(
                "SELECT root_path FROM folder_mappings WHERE collection_id=?1",
                [&mapping.collection_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(rerr)?;
        if root.is_some_and(|root| root != mapping.root_path) {
            clear_children(&transaction, &mapping.collection_id)?;
        }
        transaction
            .execute(
                "INSERT INTO folder_mappings(collection_id,root_path,state,created_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(collection_id) DO UPDATE SET root_path=excluded.root_path,state=excluded.state,created_at=excluded.created_at",
                params![
                    mapping.collection_id,
                    mapping.root_path,
                    mapping.state.as_str(),
                    mapping.created_at
                ],
            )
            .map_err(write_error)?;
        transaction.commit().map_err(rerr)
    }

    /// Sets the state of the mapping of `collection_id`, only while its root is still
    /// `root_path`. Nothing else changes. Returns whether that mapping was found.
    ///
    /// A reconcile that read a mapping saves its state with this, so a root that was changed or
    /// cleared while it ran is never written back.
    pub fn set_folder_mapping_state(
        &self,
        collection_id: &str,
        root_path: &str,
        state: MappingState,
    ) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        let updated = inner
            .conn
            .execute(
                "UPDATE folder_mappings SET state=?3 WHERE collection_id=?1 AND root_path=?2",
                params![collection_id, root_path, state.as_str()],
            )
            .map_err(rerr)?;
        Ok(updated > 0)
    }

    /// Drops the mapping of `collection_id` with its file state and refusals. The files on disk
    /// are not touched. Returns whether a mapping existed.
    pub fn clear_folder_mapping(&self, collection_id: &str) -> Result<bool, StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        // The schema declares the cascade, but this connection does not enforce foreign keys, so
        // the children are deleted here.
        clear_children(&transaction, collection_id)?;
        let removed = transaction
            .execute(
                "DELETE FROM folder_mappings WHERE collection_id=?1",
                [collection_id],
            )
            .map_err(rerr)?;
        transaction.commit().map_err(rerr)?;
        Ok(removed > 0)
    }

    /// Saves the file state of `file.document_id`, replacing any earlier state.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when the Collection has no mapping, or another Document of the
    /// mapping already has `name_key`.
    pub fn upsert_mapped_file(&self, file: &MappedFile) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let mapped: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folder_mappings WHERE collection_id=?1)",
                [&file.collection_id],
                |row| row.get(0),
            )
            .map_err(rerr)?;
        if !mapped {
            return Err(StoreError::Conflict);
        }
        // An upsert by Document, not `INSERT OR REPLACE`: a `name_key` another Document holds
        // must fail, not silently replace that Document's row.
        transaction
            .execute(
                &format!(
                    "INSERT INTO mapped_files({FILE_COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(document_id) DO UPDATE SET collection_id=excluded.collection_id,
                     rel_path=excluded.rel_path,name_key=excluded.name_key,
                     base_version=excluded.base_version,written_hash=excluded.written_hash,
                     pending_write_hash=excluded.pending_write_hash,
                     pending_write_version=excluded.pending_write_version"
                ),
                params![
                    file.document_id,
                    file.collection_id,
                    file.rel_path,
                    file.name_key,
                    file.base_version,
                    file.written_hash.as_ref().map(<[u8; 32]>::as_slice),
                    file.pending_write_hash.as_ref().map(<[u8; 32]>::as_slice),
                    file.pending_write_version,
                ],
            )
            .map_err(write_error)?;
        transaction.commit().map_err(rerr)
    }

    /// The file state of `document_id`, if the Document is mapped.
    pub fn mapped_file(&self, document_id: &str) -> Result<Option<MappedFile>, StoreError> {
        self.mapped_file_where("document_id=?1", &[document_id])
    }

    /// The file state in `collection_id` whose name key is `name_key`.
    pub fn mapped_file_by_name_key(
        &self,
        collection_id: &str,
        name_key: &str,
    ) -> Result<Option<MappedFile>, StoreError> {
        self.mapped_file_where(
            "collection_id=?1 AND name_key=?2",
            &[collection_id, name_key],
        )
    }

    /// Every file state of `collection_id`, by relative path.
    pub fn mapped_files(&self, collection_id: &str) -> Result<Vec<MappedFile>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM mapped_files WHERE collection_id=?1 ORDER BY rel_path,document_id"
            ))
            .map_err(rerr)?;
        let rows = statement
            .query_map([collection_id], file_row)
            .map_err(rerr)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(rerr)?;
        rows.into_iter().map(file).collect()
    }

    fn mapped_file_where(
        &self,
        filter: &str,
        values: &[&str],
    ) -> Result<Option<MappedFile>, StoreError> {
        let inner = lock(&self.inner)?;
        let row = inner
            .conn
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM mapped_files WHERE {filter}"),
                rusqlite::params_from_iter(values),
                file_row,
            )
            .optional()
            .map_err(rerr)?;
        row.map(file).transpose()
    }

    /// Drops the file state of `document_id`. Returns whether it existed.
    pub fn delete_mapped_file(&self, document_id: &str) -> Result<bool, StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let removed = transaction
            .execute(
                "DELETE FROM mapped_files WHERE document_id=?1",
                [document_id],
            )
            .map_err(rerr)?;
        transaction.commit().map_err(rerr)?;
        Ok(removed > 0)
    }

    /// The first phase of a write-back: records the hash and version of the bytes about to
    /// replace the file. A file found with `hash` later adopts `version` instead of being
    /// imported. Returns whether the Document is mapped.
    pub fn begin_write(
        &self,
        document_id: &str,
        hash: &FileHash,
        version: &str,
    ) -> Result<bool, StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let changed = transaction
            .execute(
                "UPDATE mapped_files SET pending_write_hash=?2,pending_write_version=?3 WHERE document_id=?1",
                params![document_id, hash.as_slice(), version],
            )
            .map_err(rerr)?;
        transaction.commit().map_err(rerr)?;
        Ok(changed > 0)
    }

    /// The last phase of a write-back, or the adoption of a pending write found on disk: the
    /// pending hash and version become `written_hash` and `base_version`. Returns whether a
    /// pending write existed.
    pub fn finish_write(&self, document_id: &str) -> Result<bool, StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let changed = transaction
            .execute(
                "UPDATE mapped_files SET written_hash=pending_write_hash,base_version=pending_write_version,
                 pending_write_hash=NULL,pending_write_version=NULL
                 WHERE document_id=?1 AND pending_write_hash IS NOT NULL",
                [document_id],
            )
            .map_err(rerr)?;
        transaction.commit().map_err(rerr)?;
        Ok(changed > 0)
    }

    /// Records that `refused.rel_path` was refused, replacing any earlier refusal of that path.
    ///
    /// # Errors
    ///
    /// [`StoreError::Conflict`] when the Collection has no mapping.
    pub fn set_refused_file(&self, refused: &RefusedFile) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let mapped: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folder_mappings WHERE collection_id=?1)",
                [&refused.collection_id],
                |row| row.get(0),
            )
            .map_err(rerr)?;
        if !mapped {
            return Err(StoreError::Conflict);
        }
        transaction
            .execute(
                "INSERT OR REPLACE INTO refused_files(collection_id,rel_path,refused_hash,reason,detail) VALUES(?1,?2,?3,?4,?5)",
                params![
                    refused.collection_id,
                    refused.rel_path,
                    refused.refused_hash.as_ref().map(<[u8; 32]>::as_slice),
                    refused.reason,
                    refused.detail,
                ],
            )
            .map_err(write_error)?;
        transaction.commit().map_err(rerr)
    }

    /// Drops the refusal of `rel_path` in `collection_id`. Returns whether it existed.
    pub fn clear_refused_file(
        &self,
        collection_id: &str,
        rel_path: &str,
    ) -> Result<bool, StoreError> {
        let mut inner = lock(&self.inner)?;
        let transaction = inner
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rerr)?;
        let removed = transaction
            .execute(
                "DELETE FROM refused_files WHERE collection_id=?1 AND rel_path=?2",
                [collection_id, rel_path],
            )
            .map_err(rerr)?;
        transaction.commit().map_err(rerr)?;
        Ok(removed > 0)
    }

    /// Every refusal of `collection_id`, by relative path.
    pub fn refused_files(&self, collection_id: &str) -> Result<Vec<RefusedFile>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare(
                "SELECT collection_id,rel_path,refused_hash,reason,detail FROM refused_files WHERE collection_id=?1 ORDER BY rel_path",
            )
            .map_err(rerr)?;
        let rows = statement
            .query_map([collection_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(rerr)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(rerr)?;
        rows.into_iter()
            .map(|(collection_id, rel_path, refused_hash, reason, detail)| {
                Ok(RefusedFile {
                    collection_id,
                    rel_path,
                    refused_hash: hash(refused_hash)?,
                    reason,
                    detail,
                })
            })
            .collect()
    }
}

fn clear_children(
    transaction: &rusqlite::Transaction<'_>,
    collection_id: &str,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "DELETE FROM mapped_files WHERE collection_id=?1",
            [collection_id],
        )
        .map_err(rerr)?;
    transaction
        .execute(
            "DELETE FROM refused_files WHERE collection_id=?1",
            [collection_id],
        )
        .map_err(rerr)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        store: DocumentStore,
        path: std::path::PathBuf,
        _directory: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        Fixture {
            store: DocumentStore::open(&path).unwrap(),
            path,
            _directory: directory,
        }
    }

    fn mapping(collection: &str, root: &str, created_at: i64) -> FolderMapping {
        FolderMapping {
            collection_id: collection.to_owned(),
            root_path: root.to_owned(),
            state: MappingState::Active,
            created_at,
        }
    }

    fn file(document: &str, collection: &str, name: &str) -> MappedFile {
        MappedFile {
            document_id: document.to_owned(),
            collection_id: collection.to_owned(),
            rel_path: format!("{name}.md"),
            name_key: format!("{}.md", name.to_lowercase()),
            base_version: Some(format!("v-{document}")),
            written_hash: Some([1; 32]),
            pending_write_hash: None,
            pending_write_version: None,
        }
    }

    fn refusal(collection: &str, rel_path: &str, reason: &str) -> RefusedFile {
        RefusedFile {
            collection_id: collection.to_owned(),
            rel_path: rel_path.to_owned(),
            refused_hash: Some([7; 32]),
            reason: reason.to_owned(),
            detail: Some("copy".to_owned()),
        }
    }

    #[test]
    fn a_mapping_is_saved_listed_and_updated() {
        let f = fixture();
        assert_eq!(f.store.folder_mapping("c1").unwrap(), None);
        assert!(f.store.folder_mappings().unwrap().is_empty());
        f.store.set_folder_mapping(&mapping("c2", "/b", 2)).unwrap();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        assert_eq!(
            f.store.folder_mappings().unwrap(),
            vec![mapping("c1", "/a", 1), mapping("c2", "/b", 2)],
            "oldest first"
        );
        let mut paused = mapping("c1", "/a", 1);
        paused.state = MappingState::Paused;
        f.store.set_folder_mapping(&paused).unwrap();
        assert_eq!(f.store.folder_mapping("c1").unwrap(), Some(paused));
    }

    #[test]
    fn a_state_update_never_reverts_a_changed_or_cleared_root() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        f.store
            .upsert_mapped_file(&file("d1", "c1", "Plan"))
            .unwrap();
        assert!(
            f.store
                .set_folder_mapping_state("c1", "/a", MappingState::Unavailable)
                .unwrap()
        );
        let saved = f.store.folder_mapping("c1").unwrap().unwrap();
        assert_eq!(saved.state, MappingState::Unavailable);
        assert_eq!(saved.root_path, "/a");
        assert_eq!(
            f.store.mapped_files("c1").unwrap().len(),
            1,
            "children kept"
        );

        // A reconcile that read `/a` must not touch the mapping once it moved to `/b`.
        f.store.set_folder_mapping(&mapping("c1", "/b", 1)).unwrap();
        assert!(
            !f.store
                .set_folder_mapping_state("c1", "/a", MappingState::Paused)
                .unwrap()
        );
        assert_eq!(
            f.store.folder_mapping("c1").unwrap(),
            Some(mapping("c1", "/b", 1))
        );

        // Nor recreate it once cleared.
        f.store.clear_folder_mapping("c1").unwrap();
        assert!(
            !f.store
                .set_folder_mapping_state("c1", "/b", MappingState::Active)
                .unwrap()
        );
        assert_eq!(f.store.folder_mapping("c1").unwrap(), None);
    }

    #[test]
    fn every_mapping_state_round_trips() {
        let f = fixture();
        for state in [
            MappingState::Active,
            MappingState::Unavailable,
            MappingState::Paused,
        ] {
            let mut saved = mapping("c1", "/a", 1);
            saved.state = state;
            f.store.set_folder_mapping(&saved).unwrap();
            assert_eq!(f.store.folder_mapping("c1").unwrap().unwrap().state, state);
        }
    }

    #[test]
    fn a_root_maps_to_one_collection() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        assert!(matches!(
            f.store.set_folder_mapping(&mapping("c2", "/a", 2)),
            Err(StoreError::Conflict)
        ));
        assert_eq!(f.store.folder_mapping("c2").unwrap(), None);
    }

    #[test]
    fn a_new_root_drops_the_old_file_state() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        f.store
            .upsert_mapped_file(&file("d1", "c1", "One"))
            .unwrap();
        f.store
            .set_refused_file(&refusal("c1", "x.md", "lossy"))
            .unwrap();
        let mut same = mapping("c1", "/a", 1);
        same.state = MappingState::Unavailable;
        f.store.set_folder_mapping(&same).unwrap();
        assert_eq!(
            f.store.mapped_files("c1").unwrap().len(),
            1,
            "same root keeps it"
        );
        f.store.set_folder_mapping(&mapping("c1", "/b", 1)).unwrap();
        assert!(f.store.mapped_files("c1").unwrap().is_empty());
        assert!(f.store.refused_files("c1").unwrap().is_empty());
    }

    #[test]
    fn clearing_a_mapping_cascades_to_its_files_and_refusals() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        f.store.set_folder_mapping(&mapping("c2", "/b", 2)).unwrap();
        f.store
            .upsert_mapped_file(&file("d1", "c1", "One"))
            .unwrap();
        f.store
            .upsert_mapped_file(&file("d2", "c2", "Two"))
            .unwrap();
        f.store
            .set_refused_file(&refusal("c1", "x.md", "lossy"))
            .unwrap();
        f.store
            .set_refused_file(&refusal("c2", "y.md", "symlink"))
            .unwrap();
        assert!(f.store.clear_folder_mapping("c1").unwrap());
        assert!(!f.store.clear_folder_mapping("c1").unwrap());
        assert_eq!(f.store.folder_mapping("c1").unwrap(), None);
        assert_eq!(f.store.mapped_file("d1").unwrap(), None);
        assert!(f.store.refused_files("c1").unwrap().is_empty());
        assert_eq!(
            f.store.mapped_file("d2").unwrap(),
            Some(file("d2", "c2", "Two")),
            "another mapping keeps its state"
        );
        assert_eq!(f.store.refused_files("c2").unwrap().len(), 1);
        // A new mapping of the same Collection starts empty.
        f.store.set_folder_mapping(&mapping("c1", "/a", 3)).unwrap();
        assert!(f.store.mapped_files("c1").unwrap().is_empty());
    }

    #[test]
    fn file_state_is_upserted_read_and_deleted() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        assert_eq!(f.store.mapped_file("d1").unwrap(), None);
        let first = file("d1", "c1", "One");
        f.store.upsert_mapped_file(&first).unwrap();
        f.store
            .upsert_mapped_file(&file("d2", "c1", "Two"))
            .unwrap();
        assert_eq!(f.store.mapped_file("d1").unwrap(), Some(first.clone()));
        assert_eq!(
            f.store.mapped_file_by_name_key("c1", "one.md").unwrap(),
            Some(first)
        );
        assert_eq!(
            f.store.mapped_file_by_name_key("c2", "one.md").unwrap(),
            None
        );
        // A rename replaces the path and the key.
        let mut renamed = file("d1", "c1", "Uno");
        renamed.base_version = None;
        renamed.written_hash = None;
        f.store.upsert_mapped_file(&renamed).unwrap();
        assert_eq!(f.store.mapped_file("d1").unwrap(), Some(renamed.clone()));
        assert_eq!(
            f.store.mapped_file_by_name_key("c1", "one.md").unwrap(),
            None
        );
        assert_eq!(
            f.store
                .mapped_files("c1")
                .unwrap()
                .iter()
                .map(|file| file.rel_path.as_str())
                .collect::<Vec<_>>(),
            ["Two.md", "Uno.md"]
        );
        assert!(f.store.delete_mapped_file("d1").unwrap());
        assert!(!f.store.delete_mapped_file("d1").unwrap());
        assert_eq!(f.store.mapped_file("d1").unwrap(), None);
    }

    #[test]
    fn a_name_key_is_unique_per_mapping() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        f.store.set_folder_mapping(&mapping("c2", "/b", 2)).unwrap();
        f.store
            .upsert_mapped_file(&file("d1", "c1", "Notes"))
            .unwrap();
        // Differs only in case: same key.
        let collision = file("d2", "c1", "NOTES");
        assert!(matches!(
            f.store.upsert_mapped_file(&collision),
            Err(StoreError::Conflict)
        ));
        assert_eq!(f.store.mapped_file("d2").unwrap(), None);
        assert_eq!(
            f.store.mapped_file("d1").unwrap(),
            Some(file("d1", "c1", "Notes")),
            "the holder is untouched"
        );
        // Another mapping may use the same key.
        f.store
            .upsert_mapped_file(&file("d3", "c2", "Notes"))
            .unwrap();
    }

    #[test]
    fn file_state_needs_a_mapping() {
        let f = fixture();
        assert!(matches!(
            f.store.upsert_mapped_file(&file("d1", "c1", "One")),
            Err(StoreError::Conflict)
        ));
        assert!(matches!(
            f.store.set_refused_file(&refusal("c1", "x.md", "lossy")),
            Err(StoreError::Conflict)
        ));
    }

    #[test]
    fn a_write_back_is_two_phase() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        assert!(!f.store.begin_write("d1", &[2; 32], "v2").unwrap());
        f.store
            .upsert_mapped_file(&file("d1", "c1", "One"))
            .unwrap();
        assert!(!f.store.finish_write("d1").unwrap(), "nothing pending");
        assert!(f.store.begin_write("d1", &[2; 32], "v2").unwrap());
        let pending = f.store.mapped_file("d1").unwrap().unwrap();
        assert_eq!(pending.written_hash, Some([1; 32]));
        assert_eq!(pending.base_version.as_deref(), Some("v-d1"));
        assert_eq!(pending.pending_write_hash, Some([2; 32]));
        assert_eq!(pending.pending_write_version.as_deref(), Some("v2"));
        assert!(f.store.finish_write("d1").unwrap());
        let written = f.store.mapped_file("d1").unwrap().unwrap();
        assert_eq!(written.written_hash, Some([2; 32]));
        assert_eq!(written.base_version.as_deref(), Some("v2"));
        assert_eq!(written.pending_write_hash, None);
        assert_eq!(written.pending_write_version, None);
        assert!(!f.store.finish_write("d1").unwrap());
    }

    #[test]
    fn a_pending_write_survives_a_reopen() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        f.store
            .upsert_mapped_file(&file("d1", "c1", "One"))
            .unwrap();
        f.store.begin_write("d1", &[2; 32], "v2").unwrap();
        drop(f.store);
        let store = DocumentStore::open(&f.path).unwrap();
        let pending = store.mapped_file("d1").unwrap().unwrap();
        assert_eq!(pending.pending_write_hash, Some([2; 32]));
        assert_eq!(pending.pending_write_version.as_deref(), Some("v2"));
        assert_eq!(pending.written_hash, Some([1; 32]));
        assert!(store.finish_write("d1").unwrap());
        assert_eq!(
            store
                .mapped_file("d1")
                .unwrap()
                .unwrap()
                .base_version
                .as_deref(),
            Some("v2")
        );
    }

    #[test]
    fn refusals_are_set_replaced_listed_and_cleared() {
        let f = fixture();
        f.store.set_folder_mapping(&mapping("c1", "/a", 1)).unwrap();
        assert!(f.store.refused_files("c1").unwrap().is_empty());
        f.store
            .set_refused_file(&refusal("c1", "z.md", "too_large"))
            .unwrap();
        f.store
            .set_refused_file(&refusal("c1", "a.md", "lossy"))
            .unwrap();
        let mut again = refusal("c1", "a.md", "not_utf8");
        again.refused_hash = None;
        again.detail = None;
        f.store.set_refused_file(&again).unwrap();
        assert_eq!(
            f.store.refused_files("c1").unwrap(),
            vec![again, refusal("c1", "z.md", "too_large")]
        );
        assert!(f.store.clear_refused_file("c1", "a.md").unwrap());
        assert!(!f.store.clear_refused_file("c1", "a.md").unwrap());
        assert_eq!(f.store.refused_files("c1").unwrap().len(), 1);
    }
}
