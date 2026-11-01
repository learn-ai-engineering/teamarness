//! The Draft store in the Package Data Directory.
//!
//! - `content/sha256/<hex>` holds complete, immutable files (mode `0400`), shared by every
//!   Draft, so an unchanged library is stored once.
//! - `content/partial/<hex>/<chunkIndex>` holds the chunks received for a file not yet complete.
//! - `content/tmp/` holds a file while it is assembled and verified.
//! - `exhibits.sqlite3` holds the Drafts, their file lists, and the bookkeeping garbage
//!   collection reads.
//!
//! Every operation runs under one lock: uploads are small and serial per file, and the lock is
//! what makes "the last chunk assembles once" and "garbage collection never races an upload"
//! true without more machinery. The filesystem is the truth for whether a file is complete.

pub(crate) mod migration;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fmt::Display;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::domain::{
    CHUNK_BYTES, ExhibitsError, GC_GRACE_MS, MAX_FILES, MAX_MISSING_QUERY, MAX_TOTAL_BYTES,
    READ_PAGE, chunk_count, chunk_length, is_sha256, validate_chunk_header, validate_id,
    validate_path, validate_title,
};

/// The current time in Unix milliseconds. Injected, so tests move time without sleeping.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The wall clock.
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            })
    })
}

/// The content store's directory, relative to the Package Data Directory.
pub const CONTENT_DIRECTORY: &str = "content/sha256";

/// The source path of a complete file, relative to the Package Data Directory.
#[must_use]
pub fn content_source_path(sha256: &str) -> String {
    format!("{CONTENT_DIRECTORY}/{sha256}")
}

/// What `files.missing` reports for one hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// Nothing of the file has been received.
    All,
    /// The chunk indices not yet received, in order.
    Chunks(Vec<u64>),
}

/// One file of a Draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Who made an update: the Host-derived Port actor, or the local Person through the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// `person` or `agent`.
    pub kind: &'static str,
    /// The actor's current signing key.
    pub identity: String,
}

/// A `draft.update` request after decoding.
#[derive(Debug, Clone)]
pub struct DraftUpdate {
    pub author: Author,
    pub id: String,
    pub title: String,
    pub entry: String,
    /// `(path, sha256)` pairs.
    pub files: Vec<(String, String)>,
    pub expected_revision: Option<u64>,
}

/// One row of the newest-first listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftSummary {
    pub id: String,
    pub title: String,
    pub revision: u64,
    pub updated_at: i64,
}

/// One page of the newest-first listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPage {
    pub drafts: Vec<DraftSummary>,
    /// The cursor for the next page, when there is one.
    pub next_before: Option<String>,
}

/// One page of a Draft's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftManifest {
    pub updated_by: Author,
    pub entry: String,
    pub title: String,
    pub revision: u64,
    pub updated_at: i64,
    /// The Draft's files from `offset`, sorted by path, at most [`READ_PAGE`].
    pub files: Vec<DraftFile>,
    pub file_count: usize,
    pub next_offset: Option<usize>,
}

/// What one garbage collection pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    pub partials_removed: usize,
    pub files_removed: usize,
}

/// The Draft store. Cloning shares it.
#[derive(Clone)]
pub struct ExhibitStore {
    connection: Arc<Mutex<Connection>>,
    root: PathBuf,
    clock: Clock,
}

fn fault<E: Display>(operation: &'static str) -> impl FnOnce(E) -> ExhibitsError {
    move |cause| {
        eprintln!("exhibits storage failure: operation={operation} cause={cause}");
        ExhibitsError::Storage
    }
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl ExhibitStore {
    /// Opens the store in the Package Data Directory `root`, creating its layout.
    ///
    /// # Errors
    /// `exhibits.storage_failed` when the directories or the database cannot be opened or
    /// migrated.
    pub fn open(root: impl Into<PathBuf>, clock: Clock) -> Result<Self, ExhibitsError> {
        let root = root.into();
        for directory in ["content/sha256", "content/partial", "content/tmp"] {
            fs::create_dir_all(root.join(directory)).map_err(fault("create_layout"))?;
        }
        let mut connection =
            Connection::open(root.join("exhibits.sqlite3")).map_err(fault("open_database"))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(fault("foreign_keys"))?;
        migration::apply(&mut connection).map_err(fault("migrate"))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            root,
            clock,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn complete_path(&self, sha256: &str) -> PathBuf {
        self.root.join(CONTENT_DIRECTORY).join(sha256)
    }

    fn partial_directory(&self, sha256: &str) -> PathBuf {
        self.root.join("content/partial").join(sha256)
    }

    /// The size of a complete file, or `None` when the store does not hold it completely.
    fn complete_size(&self, sha256: &str) -> Result<Option<u64>, ExhibitsError> {
        match fs::symlink_metadata(self.complete_path(sha256)) {
            Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(fault("stat_content")(error)),
        }
    }

    /// Stores a whole file the caller sent as bytes and returns its SHA-256 (lowercase hex).
    ///
    /// The file goes through the same chunk path as `file.put`, so it is verified, promoted and
    /// recorded exactly as an uploaded file is. Writing a file already stored changes nothing.
    ///
    /// # Errors
    /// `exhibits.too_large` (`file`) over 16 MiB, or `exhibits.storage_failed`.
    pub fn write_whole(&self, data: &[u8]) -> Result<String, ExhibitsError> {
        let sha256 = hex(&Sha256::digest(data));
        let size = data.len() as u64;
        let count = chunk_count(size);
        for index in 0..count {
            let start = usize::try_from(index * CHUNK_BYTES as u64)
                .map_err(|_| ExhibitsError::TooLarge { limit: "file" })?;
            let end = (start + CHUNK_BYTES).min(data.len());
            self.put_chunk(&sha256, size, index, count, &data[start..end])?;
        }
        Ok(sha256)
    }

    /// Receives one chunk of a file and returns whether the file is now complete.
    ///
    /// Chunks are idempotent by `(sha256, chunkIndex)`: a repeated chunk replaces the earlier
    /// one. When every chunk is present the file is assembled, its size and SHA-256 are
    /// verified, and it is promoted with an atomic rename. A file that fails verification is
    /// not promoted, and its partial upload is discarded. A put for a file already complete
    /// returns `true` without writing.
    ///
    /// # Errors
    /// The `file.put` refusals of [`validate_chunk_header`], `exhibits.invalid_chunk`
    /// (`chunk_size`) when the data is not exactly the chunk's length,
    /// `exhibits.content_mismatch`, or `exhibits.storage_failed`.
    pub fn put_chunk(
        &self,
        sha256: &str,
        size: u64,
        index: u64,
        count: u64,
        data: &[u8],
    ) -> Result<bool, ExhibitsError> {
        validate_chunk_header(sha256, size, index, count)?;
        if data.len() as u64 != chunk_length(size, index) {
            return Err(ExhibitsError::InvalidChunk {
                reason: "chunk_size",
            });
        }
        let connection = self.lock();
        if self.complete_size(sha256)?.is_some() {
            return Ok(true);
        }
        let now = (self.clock)();
        let directory = self.partial_directory(sha256);
        let existing: Option<(i64, i64)> = connection
            .query_row(
                "SELECT size, chunk_count FROM partials WHERE sha256 = ?1",
                [sha256],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(fault("read_partial"))?;
        if existing.is_some_and(|(known_size, known_count)| {
            to_u64(known_size) != size || to_u64(known_count) != count
        }) {
            // A different declared size for the same hash: one of them is wrong, and only the
            // newest declaration can still complete. Start that upload afresh.
            remove_tree(&directory)?;
            connection
                .execute("DELETE FROM partials WHERE sha256 = ?1", [sha256])
                .map_err(fault("reset_partial"))?;
        }
        fs::create_dir_all(&directory).map_err(fault("create_partial"))?;
        connection
            .execute(
                "INSERT INTO partials (sha256, size, chunk_count, started_at, touched_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (sha256) DO UPDATE SET touched_at = excluded.touched_at",
                params![sha256, to_i64(size), to_i64(count), now],
            )
            .map_err(fault("record_partial"))?;
        let chunk = directory.join(index.to_string());
        let temporary = directory.join(format!("{index}.tmp"));
        write_file(&temporary, data)?;
        fs::rename(&temporary, &chunk).map_err(fault("place_chunk"))?;

        for chunk in 0..count {
            match fs::symlink_metadata(directory.join(chunk.to_string())) {
                Ok(metadata) if metadata.len() == chunk_length(size, chunk) => {}
                _ => return Ok(false),
            }
        }
        self.finish_partial(&connection, sha256, size, count, now)
            .map(|()| true)
    }

    /// Assembles a partial upload whose chunks are all present, then removes the partial
    /// whatever the outcome.
    fn finish_partial(
        &self,
        connection: &Connection,
        sha256: &str,
        size: u64,
        count: u64,
        now: i64,
    ) -> Result<(), ExhibitsError> {
        let promoted = self.assemble(connection, sha256, size, count, now);
        remove_tree(&self.partial_directory(sha256))?;
        connection
            .execute("DELETE FROM partials WHERE sha256 = ?1", [sha256])
            .map_err(fault("finish_partial"))?;
        promoted
    }

    /// Concatenates the chunks into `content/tmp`, verifies the result and renames it into the
    /// content store. Nothing is promoted on any failure.
    fn assemble(
        &self,
        connection: &Connection,
        sha256: &str,
        size: u64,
        count: u64,
        now: i64,
    ) -> Result<(), ExhibitsError> {
        let directory = self.partial_directory(sha256);
        let temporary = self.root.join("content/tmp").join(sha256);
        let verified = (|| {
            let mut output = fs::File::create(&temporary).map_err(fault("create_assembly"))?;
            let mut hasher = Sha256::new();
            let mut total = 0_u64;
            for chunk in 0..count {
                let bytes =
                    fs::read(directory.join(chunk.to_string())).map_err(fault("read_chunk"))?;
                hasher.update(&bytes);
                total += bytes.len() as u64;
                output.write_all(&bytes).map_err(fault("write_assembly"))?;
            }
            output.sync_all().map_err(fault("sync_assembly"))?;
            Ok::<bool, ExhibitsError>(total == size && hex(&hasher.finalize()) == sha256)
        })();
        match verified {
            Ok(true) => {}
            Ok(false) => {
                let _ = fs::remove_file(&temporary);
                return Err(ExhibitsError::ContentMismatch);
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o400))
            .map_err(fault("seal_content"))?;
        fs::rename(&temporary, self.complete_path(sha256)).map_err(fault("promote_content"))?;
        connection
            .execute(
                "INSERT INTO content (sha256, size, stored_at, released_at)
                 VALUES (?1, ?2, ?3,
                         CASE WHEN EXISTS (SELECT 1 FROM draft_files WHERE sha256 = ?1)
                              THEN NULL ELSE ?3 END)
                 ON CONFLICT (sha256) DO UPDATE SET size = excluded.size",
                params![sha256, to_i64(size), now],
            )
            .map_err(fault("record_content"))?;
        Ok(())
    }

    /// For each hash, whether the store is missing all of the file, some chunks, or nothing.
    /// Hashes the store holds completely are left out; duplicates are reported once.
    ///
    /// This is a query and changes nothing. A partial upload whose chunks are all present but
    /// which was never assembled (the process ended during assembly) reports its last chunk,
    /// so re-sending that chunk with `file.put` finishes the upload.
    ///
    /// # Errors
    /// `exhibits.invalid_request` for a malformed hash or more than 256 hashes, or
    /// `exhibits.storage_failed`.
    pub fn missing(&self, hashes: &[String]) -> Result<Vec<(String, Missing)>, ExhibitsError> {
        if hashes.len() > MAX_MISSING_QUERY || !hashes.iter().all(|hash| is_sha256(hash)) {
            return Err(ExhibitsError::InvalidRequest);
        }
        let connection = self.lock();
        let mut seen = BTreeSet::new();
        let mut missing = Vec::new();
        for sha256 in hashes {
            if !seen.insert(sha256.as_str()) || self.complete_size(sha256)?.is_some() {
                continue;
            }
            let partial: Option<(i64, i64)> = connection
                .query_row(
                    "SELECT size, chunk_count FROM partials WHERE sha256 = ?1",
                    [sha256],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(fault("read_partial"))?;
            let Some((size, count)) = partial else {
                missing.push((sha256.clone(), Missing::All));
                continue;
            };
            let directory = self.partial_directory(sha256);
            let chunks: Vec<u64> = (0..to_u64(count))
                .filter(|chunk| {
                    !fs::symlink_metadata(directory.join(chunk.to_string()))
                        .is_ok_and(|metadata| metadata.len() == chunk_length(to_u64(size), *chunk))
                })
                .collect();
            let chunks = if chunks.is_empty() {
                vec![to_u64(count) - 1]
            } else {
                chunks
            };
            missing.push((sha256.clone(), Missing::Chunks(chunks)));
        }
        Ok(missing)
    }

    /// Reads chunk `index` of a complete file, and the file's chunk count.
    ///
    /// # Errors
    /// `exhibits.invalid_request` for a malformed hash, `exhibits.not_found` for a file the
    /// store does not hold completely, `exhibits.invalid_chunk` for an index out of range, or
    /// `exhibits.storage_failed`.
    pub fn read_chunk(&self, sha256: &str, index: u64) -> Result<(Vec<u8>, u64), ExhibitsError> {
        if !is_sha256(sha256) {
            return Err(ExhibitsError::InvalidRequest);
        }
        let _connection = self.lock();
        let size = self.complete_size(sha256)?.ok_or(ExhibitsError::NotFound)?;
        let count = chunk_count(size);
        if index >= count {
            return Err(ExhibitsError::InvalidChunk {
                reason: "chunk_index",
            });
        }
        let mut file = fs::File::open(self.complete_path(sha256)).map_err(fault("open_content"))?;
        file.seek(SeekFrom::Start(index * crate::domain::CHUNK_BYTES as u64))
            .map_err(fault("seek_content"))?;
        let mut bytes = vec![0; usize::try_from(chunk_length(size, index)).unwrap_or_default()];
        file.read_exact(&mut bytes).map_err(fault("read_content"))?;
        Ok((bytes, count))
    }

    /// Reads at most `length` bytes of a complete file from `offset`, and the file's size.
    ///
    /// # Errors
    /// `exhibits.invalid_request` for a malformed hash or an offset past the end,
    /// `exhibits.not_found` for a file the store does not hold completely, or
    /// `exhibits.storage_failed`.
    pub fn read_range(
        &self,
        sha256: &str,
        offset: u64,
        length: u64,
    ) -> Result<(Vec<u8>, u64), ExhibitsError> {
        if !is_sha256(sha256) {
            return Err(ExhibitsError::InvalidRequest);
        }
        let _connection = self.lock();
        let size = self.complete_size(sha256)?.ok_or(ExhibitsError::NotFound)?;
        if offset > size {
            return Err(ExhibitsError::InvalidRequest);
        }
        let mut file = fs::File::open(self.complete_path(sha256)).map_err(fault("open_content"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(fault("seek_content"))?;
        let mut bytes = vec![0; usize::try_from(length.min(size - offset)).unwrap_or_default()];
        file.read_exact(&mut bytes).map_err(fault("read_content"))?;
        Ok((bytes, size))
    }

    /// Creates a Draft, or atomically replaces its title, entry and file list, and returns the
    /// new revision.
    ///
    /// # Errors
    /// `exhibits.invalid_id`, `exhibits.invalid_title`, `exhibits.too_large` (`count` or
    /// `total`), `exhibits.invalid_path` (also for a duplicate path or an entry that is not
    /// listed), `exhibits.type_refused`, `exhibits.invalid_request` (a malformed hash),
    /// `exhibits.missing_files`, `exhibits.revision_conflict`, or `exhibits.storage_failed`.
    pub fn update_draft(&self, update: &DraftUpdate) -> Result<u64, ExhibitsError> {
        validate_id(&update.id)?;
        validate_title(&update.title)?;
        if update.files.len() > MAX_FILES {
            return Err(ExhibitsError::TooLarge { limit: "count" });
        }
        let mut paths = BTreeSet::new();
        for (path, sha256) in &update.files {
            validate_path(path)?;
            if !paths.insert(path.as_str()) {
                return Err(ExhibitsError::InvalidPath {
                    path: path.clone(),
                    reason: "duplicate",
                });
            }
            if !is_sha256(sha256) {
                return Err(ExhibitsError::InvalidRequest);
            }
        }
        if !paths.contains(update.entry.as_str()) {
            return Err(ExhibitsError::InvalidPath {
                path: update.entry.chars().take(128).collect(),
                reason: "entry_not_listed",
            });
        }
        let mut connection = self.lock();
        let mut files = Vec::with_capacity(update.files.len());
        let mut missing = Vec::new();
        let mut reported = BTreeSet::new();
        let mut total = 0_u64;
        for (path, sha256) in &update.files {
            match self.complete_size(sha256)? {
                Some(size) => {
                    total += size;
                    files.push(DraftFile {
                        path: path.clone(),
                        sha256: sha256.clone(),
                        size,
                    });
                }
                None => {
                    if reported.insert(sha256.as_str()) {
                        missing.push(sha256.clone());
                    }
                }
            }
        }
        if !missing.is_empty() {
            return Err(ExhibitsError::MissingFiles(missing));
        }
        if total > MAX_TOTAL_BYTES {
            return Err(ExhibitsError::TooLarge { limit: "total" });
        }
        let transaction = connection.transaction().map_err(fault("begin_update"))?;
        let current = current_revision(&transaction, &update.id)?;
        if update
            .expected_revision
            .is_some_and(|expected| expected != current)
        {
            return Err(ExhibitsError::RevisionConflict { current });
        }
        let now = next_timestamp(&transaction, (self.clock)())?;
        let revision = current + 1;
        transaction
            .execute(
                "INSERT INTO drafts
                     (id, title, entry, revision, created_at, updated_at, updated_by_kind, updated_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET title = excluded.title, entry = excluded.entry,
                     revision = excluded.revision, updated_at = excluded.updated_at,
                     updated_by_kind = excluded.updated_by_kind, updated_by = excluded.updated_by",
                params![
                    update.id,
                    update.title,
                    update.entry,
                    to_i64(revision),
                    now,
                    update.author.kind,
                    update.author.identity
                ],
            )
            .map_err(fault("write_draft"))?;
        transaction
            .execute("DELETE FROM draft_files WHERE draft_id = ?1", [&update.id])
            .map_err(fault("clear_files"))?;
        {
            let mut insert = transaction
                .prepare("INSERT INTO draft_files (draft_id, path, sha256, size) VALUES (?1, ?2, ?3, ?4)")
                .map_err(fault("prepare_files"))?;
            for file in &files {
                insert
                    .execute(params![
                        update.id,
                        file.path,
                        file.sha256,
                        to_i64(file.size)
                    ])
                    .map_err(fault("write_files"))?;
            }
        }
        reconcile_references(&transaction, now)?;
        transaction.commit().map_err(fault("commit_update"))?;
        Ok(revision)
    }

    /// The Drafts, newest first by update time, `limit` at a time, after the `before` cursor.
    ///
    /// # Errors
    /// `exhibits.invalid_request` for a malformed cursor, or `exhibits.storage_failed`.
    pub fn list(&self, before: Option<&str>, limit: usize) -> Result<DraftPage, ExhibitsError> {
        let cursor = before.map(parse_cursor).transpose()?;
        let (before_at, before_id) = cursor.unwrap_or((i64::MAX, String::new()));
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT id, title, revision, updated_at FROM drafts
                 WHERE ?3 OR updated_at < ?1 OR (updated_at = ?1 AND id < ?2)
                 ORDER BY updated_at DESC, id DESC LIMIT ?4",
            )
            .map_err(fault("prepare_list"))?;
        let mut drafts = statement
            .query_map(
                params![
                    before_at,
                    before_id,
                    before.is_none(),
                    i64::try_from(limit + 1).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok(DraftSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        revision: to_u64(row.get(2)?),
                        updated_at: row.get(3)?,
                    })
                },
            )
            .map_err(fault("list"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(fault("list_rows"))?;
        let next_before = if drafts.len() > limit {
            drafts.truncate(limit);
            drafts
                .last()
                .map(|last| format!("{}:{}", last.updated_at, last.id))
        } else {
            None
        };
        Ok(DraftPage {
            drafts,
            next_before,
        })
    }

    /// One page of a Draft's manifest, files sorted by path from `offset`.
    ///
    /// # Errors
    /// `exhibits.invalid_id`, `exhibits.not_found`, or `exhibits.storage_failed`.
    pub fn read(&self, id: &str, offset: usize) -> Result<DraftManifest, ExhibitsError> {
        validate_id(id)?;
        let connection = self.lock();
        let (title, entry, revision, updated_at, updated_by) = draft_row(&connection, id)?;
        let all = draft_files(&connection, id)?;
        let file_count = all.len();
        let files: Vec<DraftFile> = all.into_iter().skip(offset).take(READ_PAGE).collect();
        let end = offset.saturating_add(files.len());
        Ok(DraftManifest {
            updated_by,
            entry,
            title,
            revision,
            updated_at,
            files,
            file_count,
            next_offset: (end < file_count).then_some(end),
        })
    }

    /// Every file of a Draft with its entry and revision, for opening a content view.
    ///
    /// # Errors
    /// `exhibits.invalid_id`, `exhibits.not_found`, or `exhibits.storage_failed`.
    pub fn view_files(&self, id: &str) -> Result<(String, u64, Vec<DraftFile>), ExhibitsError> {
        validate_id(id)?;
        let connection = self.lock();
        let (_, entry, revision, _, _) = draft_row(&connection, id)?;
        Ok((entry, revision, draft_files(&connection, id)?))
    }

    /// Deletes a Draft and returns whether it existed. Its files stay in the content store
    /// until garbage collection, so a Draft deleted by mistake can be recreated without an
    /// upload within the grace period.
    ///
    /// # Errors
    /// `exhibits.invalid_id`, `exhibits.revision_conflict`, or `exhibits.storage_failed`.
    pub fn delete(&self, id: &str, expected_revision: Option<u64>) -> Result<bool, ExhibitsError> {
        validate_id(id)?;
        let mut connection = self.lock();
        let transaction = connection.transaction().map_err(fault("begin_delete"))?;
        let current = current_revision(&transaction, id)?;
        if current == 0 {
            return Ok(false);
        }
        if expected_revision.is_some_and(|expected| expected != current) {
            return Err(ExhibitsError::RevisionConflict { current });
        }
        transaction
            .execute("DELETE FROM draft_files WHERE draft_id = ?1", [id])
            .map_err(fault("delete_files"))?;
        transaction
            .execute("DELETE FROM drafts WHERE id = ?1", [id])
            .map_err(fault("delete_draft"))?;
        reconcile_references(&transaction, (self.clock)())?;
        transaction.commit().map_err(fault("commit_delete"))?;
        Ok(true)
    }

    /// Deletes partial uploads idle for longer than the grace period, complete files no Draft
    /// has referenced for longer than it, and leftovers of interrupted work.
    ///
    /// # Errors
    /// `exhibits.storage_failed`.
    pub fn collect_garbage(&self) -> Result<GcReport, ExhibitsError> {
        let mut connection = self.lock();
        let now = (self.clock)();
        let cutoff = now.saturating_sub(GC_GRACE_MS);
        let mut report = GcReport::default();

        // Assembly leftovers: nothing assembles outside this lock.
        for entry in read_directory(&self.root.join("content/tmp"))? {
            let _ = fs::remove_file(entry);
        }

        let transaction = connection.transaction().map_err(fault("begin_gc"))?;
        let stale: Vec<String> = query_strings(
            &transaction,
            "SELECT sha256 FROM partials WHERE touched_at < ?1",
            cutoff,
        )?;
        for sha256 in &stale {
            remove_tree(&self.partial_directory(sha256))?;
            transaction
                .execute("DELETE FROM partials WHERE sha256 = ?1", [sha256])
                .map_err(fault("gc_partial"))?;
            report.partials_removed += 1;
        }
        let known: BTreeSet<String> = query_strings(
            &transaction,
            "SELECT sha256 FROM partials WHERE touched_at >= ?1",
            cutoff,
        )?
        .into_iter()
        .collect();
        for directory in read_directory(&self.root.join("content/partial"))? {
            let name = directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            if !known.contains(&name) {
                remove_tree(&directory)?;
                report.partials_removed += 1;
            }
        }

        // Complete files on disk that the database does not know start their grace period now.
        for file in read_directory(&self.root.join(CONTENT_DIRECTORY))? {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let size = fs::symlink_metadata(&file).map_or(0, |metadata| metadata.len());
            transaction
                .execute(
                    "INSERT INTO content (sha256, size, stored_at, released_at)
                     VALUES (?1, ?2, ?3, ?3) ON CONFLICT (sha256) DO NOTHING",
                    params![name, to_i64(size), now],
                )
                .map_err(fault("gc_adopt"))?;
        }
        reconcile_references(&transaction, now)?;
        let released: Vec<String> = query_strings(
            &transaction,
            "SELECT sha256 FROM content WHERE released_at IS NOT NULL AND released_at < ?1",
            cutoff,
        )?;
        for sha256 in &released {
            match fs::remove_file(self.complete_path(sha256)) {
                Ok(()) => report.files_removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(fault("gc_remove_content")(error)),
            }
            transaction
                .execute("DELETE FROM content WHERE sha256 = ?1", [sha256])
                .map_err(fault("gc_content"))?;
        }
        transaction.commit().map_err(fault("commit_gc"))?;
        Ok(report)
    }
}

/// Marks content referenced by a Draft as held, and content no Draft references any more as
/// released at `now`, keeping the earlier release time of content already released.
fn reconcile_references(transaction: &Transaction<'_>, now: i64) -> Result<(), ExhibitsError> {
    transaction
        .execute(
            "UPDATE content SET released_at = NULL
             WHERE released_at IS NOT NULL AND sha256 IN (SELECT sha256 FROM draft_files)",
            [],
        )
        .map_err(fault("hold_content"))?;
    transaction
        .execute(
            "UPDATE content SET released_at = ?1
             WHERE released_at IS NULL AND sha256 NOT IN (SELECT sha256 FROM draft_files)",
            [now],
        )
        .map_err(fault("release_content"))?;
    Ok(())
}

fn current_revision(connection: &Connection, id: &str) -> Result<u64, ExhibitsError> {
    connection
        .query_row("SELECT revision FROM drafts WHERE id = ?1", [id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .map_err(fault("read_revision"))
        .map(|revision| revision.map_or(0, to_u64))
}

/// A strictly increasing update time, so newest first is a total order even within one
/// millisecond.
fn next_timestamp(connection: &Connection, now: i64) -> Result<i64, ExhibitsError> {
    let latest: Option<i64> = connection
        .query_row("SELECT MAX(updated_at) FROM drafts", [], |row| row.get(0))
        .map_err(fault("read_latest"))?;
    Ok(latest.map_or(now, |latest| now.max(latest.saturating_add(1))))
}

fn draft_row(
    connection: &Connection,
    id: &str,
) -> Result<(String, String, u64, i64, Author), ExhibitsError> {
    connection
        .query_row(
            "SELECT title, entry, revision, updated_at, updated_by_kind, updated_by
             FROM drafts WHERE id = ?1",
            [id],
            |row| {
                let kind: String = row.get(4)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    to_u64(row.get(2)?),
                    row.get(3)?,
                    Author {
                        kind: if kind == "agent" { "agent" } else { "person" },
                        identity: row.get(5)?,
                    },
                ))
            },
        )
        .optional()
        .map_err(fault("read_draft"))?
        .ok_or(ExhibitsError::NotFound)
}

fn draft_files(connection: &Connection, id: &str) -> Result<Vec<DraftFile>, ExhibitsError> {
    let mut statement = connection
        .prepare("SELECT path, sha256, size FROM draft_files WHERE draft_id = ?1 ORDER BY path")
        .map_err(fault("prepare_files"))?;
    statement
        .query_map([id], |row| {
            Ok(DraftFile {
                path: row.get(0)?,
                sha256: row.get(1)?,
                size: to_u64(row.get(2)?),
            })
        })
        .map_err(fault("read_files"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(fault("read_file_rows"))
}

fn query_strings(
    connection: &Connection,
    sql: &str,
    parameter: i64,
) -> Result<Vec<String>, ExhibitsError> {
    let mut statement = connection.prepare(sql).map_err(fault("prepare_query"))?;
    statement
        .query_map([parameter], |row| row.get(0))
        .map_err(fault("query"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(fault("query_rows"))
}

/// Parses a `before` cursor, `<updatedAtMs>:<exhibitId>`.
fn parse_cursor(cursor: &str) -> Result<(i64, String), ExhibitsError> {
    let (at, id) = cursor
        .split_once(':')
        .ok_or(ExhibitsError::InvalidRequest)?;
    let at = at
        .parse::<i64>()
        .map_err(|_| ExhibitsError::InvalidRequest)?;
    if validate_id(id).is_err() {
        return Err(ExhibitsError::InvalidRequest);
    }
    Ok((at, id.to_owned()))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ExhibitsError> {
    let mut file = fs::File::create(path).map_err(fault("create_chunk"))?;
    file.write_all(bytes).map_err(fault("write_chunk"))?;
    file.sync_all().map_err(fault("sync_chunk"))
}

fn remove_tree(path: &Path) -> Result<(), ExhibitsError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(fault("remove_partial")(error)),
    }
}

fn read_directory(path: &Path) -> Result<Vec<PathBuf>, ExhibitsError> {
    fs::read_dir(path)
        .map_err(fault("read_directory"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(fault("read_directory_entry"))
}

/// Lowercase hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}
