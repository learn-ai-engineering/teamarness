//! The reconciler: it compares a mapped folder with its Collection and applies the
//! file-to-Document and Document-to-file rules, crash-safely.
//!
//! Folder to Collection ([`FolderSync::reconcile_folder`]), for each `.md` file:
//!
//! - its hash is `written_hash`: nothing;
//! - its hash is `pending_write_hash`: the write-back that made it is adopted, not imported;
//! - it changed: loss check, then import as the local Person's revision of `base_version`;
//! - it is new: pre-check, then a Document is created and given the file's text;
//! - it is missing, and its hash is on a new path: the Document is renamed;
//! - it is missing otherwise: the Document is archived.
//!
//! Collection to folder ([`FolderSync::reconcile_document`]): a Document that moved past its
//! file's `base_version` is written back with the splice writer, after any unimported edit of
//! the file is imported. A rename renames the file, an archive removes it, and a restore or a
//! new Document writes one.
//!
//! Crash safety:
//!
//! - An import's request ID is derived from the Document, the base version and the file hash.
//!   An ID already in the record log advances the base instead of appending again.
//! - A write-back records the hash and version it is about to write before it renames the file
//!   ([`DocumentStore::begin_write`]). A file found with that hash adopts that version.
//!
//! A missing or unreadable root archives nothing: the mapping is `unavailable` until it
//! returns. The first sync of a file never rewrites it. A refused file is never touched, and the
//! same bytes are not retried until they change.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use teamarness_package_sdk::capabilities::record_log::RecordLog;
use teamarness_package_sdk::{BackendError, CancellationToken};
use tokio::sync::watch;

use super::super::ports::{BodyState, body_availability, decode_version, encode_version};
use super::super::{
    BodyHub, LocalWrite, append_domain_record, drain_local_writes, error, lookup_identities,
    reopen_document, stage_local_write, storage_failed,
};
use super::codec::{
    EXTENSION, WriteRefused, file_name, name_key, sha256, sha256_reader, splice, unique_name,
    write_atomic,
};
use super::scan::{ScannedFile, scan};
use crate::body::{BodyError, DocumentBody, MAX_TEXT_BYTES};
use crate::domain::{DomainRecord, MAX_TITLE, MAX_UPDATE};
use crate::markdown;
use crate::store::mappings::{FileHash, FolderMapping, MappedFile, MappingState, RefusedFile};
use crate::store::{DocumentStore, DocumentView, StoreError};

/// The most bytes folder sync reads from one file. Canonical text is bounded by
/// [`MAX_TEXT_BYTES`]; a file four times larger is refused without being read whole.
const MAX_FILE_BYTES: u64 = 4 * MAX_TEXT_BYTES as u64;

/// How often one write-back imports a file that changed under it before it gives up until
/// the next reconcile.
const WRITE_ATTEMPTS: usize = 3;

/// Why a file was not imported. The settings view shows the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The import would lose text: the loss check found tokens the export drops.
    Lossy,
    /// The text, or the one record its import needs, is over the Documents bounds.
    TooLarge,
    NotUtf8,
    /// The Markdown cannot be held by the Documents schema.
    SchemaViolation,
    /// The body refused the merge.
    MergeRefused,
    /// The file is a symbolic link, which folder sync never follows.
    Symlink,
    /// The file name does not make a valid Document title.
    TitleTooLong,
    /// Another file of the mapping already has this name on a case-insensitive file system.
    NameCollisionUnresolved,
}

impl Refusal {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Lossy => "lossy",
            Self::TooLarge => "too_large",
            Self::NotUtf8 => "not_utf8",
            Self::SchemaViolation => "schema_violation",
            Self::MergeRefused => "merge_refused",
            Self::Symlink => "symlink",
            Self::TitleTooLong => "title_too_long",
            Self::NameCollisionUnresolved => "name_collision_unresolved",
        }
    }
}

/// What one reconcile did, for logging and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Changed files imported as revisions, or found already included.
    pub imported: usize,
    /// New files that became Documents.
    pub created: usize,
    /// Files whose rename renamed their Document.
    pub renamed: usize,
    /// Missing files whose Document was archived.
    pub archived: usize,
    /// Pending write-backs adopted without an import.
    pub adopted: usize,
    /// Files written: write-backs and new files.
    pub written: usize,
    /// Files renamed after their Document was renamed.
    pub moved: usize,
    /// Files removed after their Document was archived or moved away.
    pub removed: usize,
    /// Files newly refused.
    pub refused: usize,
    /// Files or Documents left for a later reconcile, because a body is not available yet.
    pub pending: usize,
}

/// Where a test stops a reconcile, as a crash would. Outside tests, nothing stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Crash {
    /// After an import's record reached the record log, before its file state is saved.
    AfterImportAppend,
    /// After a write-back renamed its file into place, before `finish_write`.
    AfterWriteRename,
}

/// What an import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Imported {
    /// The file's bytes are now the file state's `written_hash`.
    Done,
    /// The file was refused, now or before.
    Refused,
    /// The body is not available, or the record log did not answer: try again later.
    Pending,
    /// The file went away.
    Missing,
}

/// A file's bytes as read now. `bytes` is `None` when the file is over [`MAX_FILE_BYTES`].
struct FileContent {
    hash: FileHash,
    bytes: Option<Vec<u8>>,
}

/// The pieces folder sync needs from one activation. Cheap to clone; every clone shares one
/// gate, so reconciles never interleave.
#[derive(Clone)]
pub(crate) struct FolderSync {
    log: RecordLog,
    hub: BodyHub,
    changes: watch::Sender<u64>,
    /// This Installation's Person, who signs every record folder sync writes.
    author: String,
    cancellation: CancellationToken,
    gate: Arc<tokio::sync::Mutex<()>>,
    /// Documents whose file is known, since this activation began, to say what the Document
    /// said at the file's `base_version`. Any other file is compared with its Document before
    /// the write-back skips it, so a crash between an import and its write-back leaves no file
    /// behind its Document.
    verified: Arc<std::sync::Mutex<HashSet<String>>>,
    #[cfg(test)]
    crash: Arc<std::sync::Mutex<Option<Crash>>>,
}

impl FolderSync {
    pub(crate) fn new(
        log: &RecordLog,
        hub: &BodyHub,
        changes: &watch::Sender<u64>,
        author: String,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            log: log.clone(),
            hub: hub.clone(),
            changes: changes.clone(),
            author,
            cancellation,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            verified: Arc::new(std::sync::Mutex::new(HashSet::new())),
            #[cfg(test)]
            crash: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Makes the next reconcile stop at `point`, as a crash would.
    #[cfg(test)]
    pub(crate) fn crash_at(&self, point: Crash) {
        *self.crash.lock().unwrap() = Some(point);
    }

    #[cfg(test)]
    fn crash_point(&self, point: Crash) -> Result<(), BackendError> {
        let mut armed = self.crash.lock().unwrap();
        if *armed == Some(point) {
            *armed = None;
            return Err(error("documents.test_crash", true));
        }
        Ok(())
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    fn crash_point(&self, _point: Crash) -> Result<(), BackendError> {
        Ok(())
    }

    /// Reconciles the whole mapping of `collection`: every file of its folder, then every
    /// Document of the Collection. An export mapping (empty folder) writes every Document; an
    /// import mapping (empty Collection) creates a Document for every file. A Collection
    /// without a mapping, or with a paused one, is left alone.
    pub(crate) async fn reconcile_folder(&self, collection: &str) -> Result<Outcome, BackendError> {
        let _gate = self.gate.lock().await;
        let mut outcome = Outcome::default();
        self.folder(collection, &mut outcome).await?;
        Ok(outcome)
    }

    /// Writes `document` back to its mapped file, or renames, removes or creates that file, as
    /// the Document now requires. A Document outside any active mapping is left alone.
    pub(crate) async fn reconcile_document(&self, document: &str) -> Result<Outcome, BackendError> {
        let _gate = self.gate.lock().await;
        let mut outcome = Outcome::default();
        self.document(document, &mut outcome).await?;
        Ok(outcome)
    }

    // ------------------------------------------------------------ folder to Collection

    async fn folder(&self, collection: &str, outcome: &mut Outcome) -> Result<(), BackendError> {
        let Some(mapping) = self.mapping(collection).await? else {
            return Ok(());
        };
        let root = PathBuf::from(&mapping.root_path);
        let scanned = {
            let root = root.clone();
            self.blocking(move || Ok(scan(&root))).await?
        };
        let Ok(scanned) = scanned else {
            // An unmounted drive or a removed folder archives nothing.
            self.set_state(&mapping, MappingState::Unavailable).await?;
            return Ok(());
        };
        self.set_state(&mapping, MappingState::Active).await?;

        let on_disk: HashMap<&str, &ScannedFile> = scanned
            .files
            .iter()
            .map(|file| (file.rel.as_str(), file))
            .collect();
        let unreadable: HashSet<&str> = scanned.unreadable.iter().map(String::as_str).collect();
        let files = self.mapped_files(collection).await?;
        let mapped: HashSet<&str> = files.iter().map(|file| file.rel_path.as_str()).collect();
        let mut new_files: Vec<&ScannedFile> = scanned
            .files
            .iter()
            .filter(|file| !mapped.contains(file.rel.as_str()))
            .collect();

        for link in &scanned.symlinks {
            if link.ends_with(EXTENSION) {
                self.refuse(collection, link, None, Refusal::Symlink, None, outcome)
                    .await?;
            }
        }

        for file in &files {
            if let Some(found) = on_disk.get(file.rel_path.as_str()) {
                if Some(found.hash) == file.written_hash {
                    continue;
                }
                if Some(found.hash) == file.pending_write_hash {
                    self.adopt(&file.document_id, outcome).await?;
                    continue;
                }
                self.import(&root, file, outcome).await?;
                continue;
            }
            if unreadable.contains(file.rel_path.as_str()) {
                continue;
            }
            if file.written_hash.is_none() {
                // A write of a new file that never reached the disk: the Document is written
                // again below, under a fresh name.
                let document = file.document_id.clone();
                self.store(move |store| store.delete_mapped_file(&document))
                    .await?;
                continue;
            }
            let moved = new_files.iter().position(|candidate| {
                Some(candidate.hash) == file.written_hash
                    || Some(candidate.hash) == file.pending_write_hash
            });
            match moved {
                Some(index) => {
                    let target = new_files.remove(index);
                    self.renamed(file, target, outcome).await?;
                }
                None => self.archive(file, outcome).await?,
            }
        }

        // Name keys held now, after archives freed theirs.
        let mut taken: HashSet<String> = self
            .mapped_files(collection)
            .await?
            .into_iter()
            .map(|file| file.name_key)
            .collect();
        for file in new_files {
            self.create(&root, collection, file, &mut taken, outcome)
                .await?;
        }

        // A refusal of a file that is gone no longer applies.
        let present: HashSet<&str> = scanned
            .files
            .iter()
            .map(|file| file.rel.as_str())
            .chain(scanned.symlinks.iter().map(String::as_str))
            .collect();
        let refused = self.refusals(collection).await?;
        for refusal in refused {
            if !present.contains(refusal.rel_path.as_str()) {
                let collection = collection.to_owned();
                self.store(move |store| store.clear_refused_file(&collection, &refusal.rel_path))
                    .await?;
            }
        }

        // Collection to folder: every Document of the Collection, and every Document whose file
        // is still in this folder.
        let mut documents: Vec<String> = self
            .documents()
            .await?
            .into_iter()
            .filter(|document| document.collection_id == collection)
            .map(|document| document.id)
            .collect();
        for file in self.mapped_files(collection).await? {
            if !documents.contains(&file.document_id) {
                documents.push(file.document_id);
            }
        }
        for document in documents {
            self.document(&document, outcome).await?;
        }
        Ok(())
    }

    /// Adopts the pending write-back of `document`: the file holds what it wrote.
    async fn adopt(&self, document: &str, outcome: &mut Outcome) -> Result<(), BackendError> {
        let id = document.to_owned();
        if self.store(move |store| store.finish_write(&id)).await? {
            self.verify(document);
            outcome.adopted += 1;
        }
        Ok(())
    }

    /// Imports the file of `file` as the local Person's revision of its `base_version`.
    async fn import(
        &self,
        root: &Path,
        file: &MappedFile,
        outcome: &mut Outcome,
    ) -> Result<Imported, BackendError> {
        let Some(content) = self.read(root, &file.rel_path).await? else {
            return Ok(Imported::Missing);
        };
        if Some(content.hash) == file.written_hash {
            return Ok(Imported::Done);
        }
        if Some(content.hash) == file.pending_write_hash {
            self.adopt(&file.document_id, outcome).await?;
            return Ok(Imported::Done);
        }
        if self
            .refused_before(&file.collection_id, &file.rel_path, &content.hash)
            .await?
        {
            return Ok(Imported::Refused);
        }
        let text = match checked_text(content.bytes) {
            Ok(text) => text,
            Err((reason, detail)) => {
                self.refuse(
                    &file.collection_id,
                    &file.rel_path,
                    Some(content.hash),
                    reason,
                    detail,
                    outcome,
                )
                .await?;
                return Ok(Imported::Refused);
            }
        };
        let document = file.document_id.as_str();
        if !self.available(document).await? {
            outcome.pending += 1;
            return Ok(Imported::Pending);
        }
        let current = self.version(document);
        let Some(base_token) = file.base_version.clone().or(current.clone()) else {
            outcome.pending += 1;
            return Ok(Imported::Pending);
        };
        let Some(base) = decode_version(&base_token) else {
            self.refuse(
                &file.collection_id,
                &file.rel_path,
                Some(content.hash),
                Refusal::MergeRefused,
                Some("base_unavailable".to_owned()),
                outcome,
            )
            .await?;
            return Ok(Imported::Refused);
        };
        let id = request_id(document, &base_token, &content.hash);

        // The inclusion check: a crash after this import's append and before its file state
        // was saved left the revision in the record log. It is not appended again.
        let included = match lookup_identities(
            &self.log,
            &self.hub.store,
            &self.hub,
            std::slice::from_ref(&id),
        )
        .await
        {
            Ok(found) => found.contains_key(&id),
            Err(_) => {
                outcome.pending += 1;
                return Ok(Imported::Pending);
            }
        };
        let base_version = if included {
            // The Document holds this file's revision already: it is the base now. The file is
            // not yet known to say everything the Document says, so the write-back that follows
            // compares them.
            current
                .clone()
                .ok_or_else(|| error("documents.document_pending", true))?
        } else {
            let staged = stage_local_write(
                &self.log,
                &self.hub,
                &self.author,
                document,
                &id,
                LocalWrite::Revision {
                    base: &base,
                    markdown: &text,
                    append_first: false,
                },
            )
            .await;
            let staged = match staged {
                Ok(staged) => staged,
                Err(cause) => {
                    let Some((reason, detail)) = merge_refusal(&cause) else {
                        outcome.pending += 1;
                        return Ok(Imported::Pending);
                    };
                    self.refuse(
                        &file.collection_id,
                        &file.rel_path,
                        Some(content.hash),
                        reason,
                        Some(detail),
                        outcome,
                    )
                    .await?;
                    return Ok(Imported::Refused);
                }
            };
            if staged.changed {
                // The staged record is retried by the flusher if this fails; the next reconcile
                // then finds its ID included.
                drain_local_writes(&self.log, &self.hub, &self.changes, &self.author, document)
                    .await?;
                self.crash_point(Crash::AfterImportAppend)?;
                // The merge's own version: it holds no edit adopted after the merge forked the
                // body, so a concurrent edit is still written back.
                encode_version(&staged.version)
                    .or(self.version(document))
                    .ok_or_else(|| error("documents.document_pending", true))?
            } else {
                // A revision that changes nothing keeps the base: the Document may hold edits
                // the file does not, and the write-back writes them.
                base_token
            }
        };

        let state = MappedFile {
            base_version: Some(base_version),
            written_hash: Some(content.hash),
            pending_write_hash: None,
            pending_write_version: None,
            ..file.clone()
        };
        self.store(move |store| {
            store.upsert_mapped_file(&state)?;
            store.clear_refused_file(&state.collection_id, &state.rel_path)?;
            Ok(())
        })
        .await?;
        // A concurrent edit merged into the Document is not in the file yet.
        self.unverify(document);
        outcome.imported += 1;
        Ok(Imported::Done)
    }

    /// A new file becomes a Document: pre-check, `document.create` with the file stem as the
    /// title, then the file's text as the first revision of the genesis.
    async fn create(
        &self,
        root: &Path,
        collection: &str,
        file: &ScannedFile,
        taken: &mut HashSet<String>,
        outcome: &mut Outcome,
    ) -> Result<(), BackendError> {
        if self
            .refused_before(collection, &file.rel, &file.hash)
            .await?
        {
            return Ok(());
        }
        let key = name_key(&file.rel);
        if taken.contains(&key) {
            self.refuse(
                collection,
                &file.rel,
                Some(file.hash),
                Refusal::NameCollisionUnresolved,
                None,
                outcome,
            )
            .await?;
            return Ok(());
        }
        let Some(content) = self.read(root, &file.rel).await? else {
            return Ok(());
        };
        let genesis = crate::body::genesis();
        let checked = {
            let bytes = content.bytes;
            let genesis = genesis.clone();
            let rel = file.rel.clone();
            self.blocking(move || Ok(precheck(&rel, bytes, &genesis)))
                .await?
        };
        let (title, text) = match checked {
            Ok(checked) => checked,
            Err((reason, detail)) => {
                self.refuse(
                    collection,
                    &file.rel,
                    Some(content.hash),
                    reason,
                    detail,
                    outcome,
                )
                .await?;
                return Ok(());
            }
        };
        let base = encode_version(&crate::body::genesis_frontiers(&genesis))
            .ok_or_else(|| error("documents.edit_failed", true))?;
        let record = match DomainRecord::document_create(&self.author, collection, &title, genesis)
        {
            Ok(record) => record,
            Err(_) => {
                self.refuse(
                    collection,
                    &file.rel,
                    Some(content.hash),
                    Refusal::TitleTooLong,
                    None,
                    outcome,
                )
                .await?;
                return Ok(());
            }
        };
        let created = self.append(record).await?;
        let document = created["id"]
            .as_str()
            .ok_or_else(storage_failed)?
            .to_owned();
        // The file state names the genesis as its base and no written hash, so a crash before
        // the first revision imports the file into this Document on the next reconcile.
        let state = MappedFile {
            document_id: document,
            collection_id: collection.to_owned(),
            rel_path: file.rel.clone(),
            name_key: key.clone(),
            base_version: Some(base),
            written_hash: None,
            pending_write_hash: None,
            pending_write_version: None,
        };
        {
            let state = state.clone();
            self.store(move |store| {
                store.upsert_mapped_file(&state)?;
                store.clear_refused_file(&state.collection_id, &state.rel_path)?;
                Ok(())
            })
            .await?;
        }
        taken.insert(key);
        outcome.created += 1;
        if text.is_empty() {
            // Nothing to revise: the file and the genesis agree.
            let state = MappedFile {
                written_hash: Some(content.hash),
                ..state
            };
            self.store(move |store| store.upsert_mapped_file(&state))
                .await?;
            return Ok(());
        }
        self.import(root, &state, outcome).await?;
        Ok(())
    }

    /// A missing file whose bytes are on `target` now: the Document takes the new file stem as
    /// its title.
    async fn renamed(
        &self,
        file: &MappedFile,
        target: &ScannedFile,
        outcome: &mut Outcome,
    ) -> Result<(), BackendError> {
        let title = title_of(&target.rel);
        let current = self
            .documents()
            .await?
            .into_iter()
            .find(|document| document.id == file.document_id);
        if let Some(current) = current
            && current.title.as_deref() != Some(title.as_str())
            && let Ok(record) =
                DomainRecord::document_rename(&self.author, &file.document_id, &title)
        {
            self.append(record).await?;
        }
        let state = MappedFile {
            rel_path: target.rel.clone(),
            name_key: name_key(&target.rel),
            ..file.clone()
        };
        self.store(move |store| {
            store.upsert_mapped_file(&state)?;
            store.clear_refused_file(&state.collection_id, &state.rel_path)?;
            Ok(())
        })
        .await?;
        outcome.renamed += 1;
        // The renamed file may also have been edited since; the write-back below imports it.
        Ok(())
    }

    /// A deleted file archives its Document for the team.
    async fn archive(&self, file: &MappedFile, outcome: &mut Outcome) -> Result<(), BackendError> {
        let current = self
            .documents()
            .await?
            .into_iter()
            .find(|document| document.id == file.document_id);
        if current.is_some_and(|document| !document.archived) {
            let record = DomainRecord::document_archive(&self.author, &file.document_id)
                .map_err(|_| storage_failed())?;
            self.append(record).await?;
        }
        let document = file.document_id.clone();
        self.store(move |store| store.delete_mapped_file(&document))
            .await?;
        outcome.archived += 1;
        Ok(())
    }

    // ------------------------------------------------------------ Collection to folder

    async fn document(&self, document: &str, outcome: &mut Outcome) -> Result<(), BackendError> {
        let Some(current) = self
            .documents()
            .await?
            .into_iter()
            .find(|known| known.id == document)
        else {
            return Ok(());
        };
        let mut state = {
            let document = document.to_owned();
            self.store(move |store| store.mapped_file(&document))
                .await?
        };
        // A Document moved to another Collection leaves this folder.
        if let Some(file) = state.as_ref()
            && file.collection_id != current.collection_id
        {
            if let Some(mapping) = self.mapping(&file.collection_id).await?
                && mapping.state == MappingState::Active
            {
                let root = PathBuf::from(&mapping.root_path);
                if !self.remove(&root, file, outcome).await? {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
            state = None;
        }
        let Some(mapping) = self.mapping(&current.collection_id).await? else {
            return Ok(());
        };
        if mapping.state != MappingState::Active {
            return Ok(());
        }
        let root = PathBuf::from(&mapping.root_path);
        let present = {
            let root = root.clone();
            self.blocking(move || Ok(fs::metadata(&root).is_ok_and(|meta| meta.is_dir())))
                .await?
        };
        if !present {
            self.set_state(&mapping, MappingState::Unavailable).await?;
            return Ok(());
        }
        if current.archived {
            if let Some(file) = state {
                self.remove(&root, &file, outcome).await?;
            }
            return Ok(());
        }
        let Some(title) = current.title.clone() else {
            outcome.pending += 1;
            return Ok(());
        };
        match state {
            None => self.write_new(&root, &current, &title, outcome).await,
            Some(file) => self.write_back(&root, file, &title, outcome).await,
        }
    }

    /// Writes a Document that has no file yet: a new or restored Document, or an export.
    async fn write_new(
        &self,
        root: &Path,
        document: &DocumentView,
        title: &str,
        outcome: &mut Outcome,
    ) -> Result<(), BackendError> {
        if !self.available(&document.id).await? {
            outcome.pending += 1;
            return Ok(());
        }
        let Some((text, version)) = self.body(&document.id) else {
            outcome.pending += 1;
            return Ok(());
        };
        let taken = self.taken(root, &document.collection_id, None).await?;
        let name = unique_name(title, &taken);
        let hash = sha256(text.as_bytes());
        // Phase one: the file state names what is about to be written, so a crash after the
        // write adopts it.
        let state = MappedFile {
            document_id: document.id.clone(),
            collection_id: document.collection_id.clone(),
            rel_path: name.clone(),
            name_key: name_key(&name),
            base_version: None,
            written_hash: None,
            pending_write_hash: Some(hash),
            pending_write_version: Some(version),
        };
        self.store(move |store| store.upsert_mapped_file(&state))
            .await?;
        let written = {
            let root = root.to_path_buf();
            let name = name.clone();
            self.blocking(move || Ok(write_atomic(&root, Path::new(&name), &text, None)))
                .await?
        };
        match written {
            Ok(_) => {
                self.crash_point(Crash::AfterWriteRename)?;
                let id = document.id.clone();
                self.store(move |store| store.finish_write(&id)).await?;
                self.verify(&document.id);
                outcome.written += 1;
            }
            Err(refused) => {
                // A file appeared under the name: the next reconcile picks another one.
                log_refused_write(&document.id, &refused);
                let id = document.id.clone();
                self.store(move |store| store.delete_mapped_file(&id))
                    .await?;
            }
        }
        Ok(())
    }

    /// Brings the file of a mapped Document up to date: imports an edit of the file first,
    /// renames it after a Document rename, then splices the Document's export into it.
    async fn write_back(
        &self,
        root: &Path,
        mut file: MappedFile,
        title: &str,
        outcome: &mut Outcome,
    ) -> Result<(), BackendError> {
        for _ in 0..WRITE_ATTEMPTS {
            let Some(content) = self.read(root, &file.rel_path).await? else {
                // A deleted file is the folder reconcile's to archive.
                return Ok(());
            };
            if Some(content.hash) == file.pending_write_hash {
                self.adopt(&file.document_id, outcome).await?;
                file = self.refetch(&file).await?;
                continue;
            }
            if Some(content.hash) != file.written_hash {
                // An external edit is never overwritten: it is imported first.
                if self.import(root, &file, outcome).await? != Imported::Done {
                    return Ok(());
                }
                file = self.refetch(&file).await?;
                continue;
            }
            if !self.available(&file.document_id).await? {
                outcome.pending += 1;
                return Ok(());
            }
            file = self.rename_after_title(root, file, title, outcome).await?;
            let Some((text, version)) = self.body(&file.document_id) else {
                outcome.pending += 1;
                return Ok(());
            };
            if file.base_version.as_deref() == Some(version.as_str())
                && self.is_verified(&file.document_id)
            {
                return Ok(());
            }
            let previous = content
                .bytes
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            let (same, spliced) = {
                let text = text.clone();
                self.blocking(move || {
                    // The file already says what the Document says: it is never rewritten
                    // just to change its bytes.
                    if markdown::canonical(&previous).is_ok_and(|canonical| canonical == text) {
                        return Ok((true, previous));
                    }
                    let spliced = splice(&previous, &text);
                    Ok((false, spliced))
                })
                .await?
            };
            if same {
                if file.base_version.as_deref() != Some(version.as_str()) {
                    let state = MappedFile {
                        base_version: Some(version),
                        ..file.clone()
                    };
                    self.store(move |store| store.upsert_mapped_file(&state))
                        .await?;
                }
                self.verify(&file.document_id);
                return Ok(());
            }
            let hash = sha256(spliced.as_bytes());
            {
                let id = file.document_id.clone();
                let version = version.clone();
                self.store(move |store| store.begin_write(&id, &hash, &version))
                    .await?;
            }
            let written = {
                let root = root.to_path_buf();
                let rel = file.rel_path.clone();
                let expected = content.hash;
                self.blocking(move || {
                    Ok(write_atomic(
                        &root,
                        Path::new(&rel),
                        &spliced,
                        Some(expected),
                    ))
                })
                .await?
            };
            match written {
                Ok(_) => {
                    self.crash_point(Crash::AfterWriteRename)?;
                    let id = file.document_id.clone();
                    self.store(move |store| store.finish_write(&id)).await?;
                    self.verify(&file.document_id);
                    outcome.written += 1;
                    return Ok(());
                }
                Err(WriteRefused::Changed { .. }) => {
                    // Edited between the read and the write: import it, then try again.
                    file = self.refetch(&file).await?;
                }
                Err(refused) => {
                    log_refused_write(&file.document_id, &refused);
                    return Ok(());
                }
            }
        }
        outcome.pending += 1;
        Ok(())
    }

    /// Renames the file of `file` when its name no longer renders the Document's title.
    async fn rename_after_title(
        &self,
        root: &Path,
        file: MappedFile,
        title: &str,
        outcome: &mut Outcome,
    ) -> Result<MappedFile, BackendError> {
        let stem = file
            .rel_path
            .strip_suffix(EXTENSION)
            .unwrap_or(&file.rel_path);
        if file_name(stem) == file_name(title) {
            return Ok(file);
        }
        let taken = self
            .taken(root, &file.collection_id, Some(&file.name_key))
            .await?;
        let name = unique_name(title, &taken);
        if name == file.rel_path {
            return Ok(file);
        }
        let moved = {
            let root = root.to_path_buf();
            let from = file.rel_path.clone();
            let to = name.clone();
            self.blocking(move || Ok(rename_file(&root, &from, &to)))
                .await?
        };
        if let Err(cause) = moved {
            eprintln!(
                "documents folder sync rename failure: document={} kind={:?}",
                file.document_id,
                cause.kind()
            );
            return Ok(file);
        }
        let state = MappedFile {
            rel_path: name.clone(),
            name_key: name_key(&name),
            ..file
        };
        {
            let state = state.clone();
            self.store(move |store| store.upsert_mapped_file(&state))
                .await?;
        }
        outcome.moved += 1;
        Ok(state)
    }

    /// Removes the file of an archived or moved Document, importing any unimported edit first.
    /// Returns whether the file is gone; a file whose edit cannot be imported stays.
    async fn remove(
        &self,
        root: &Path,
        file: &MappedFile,
        outcome: &mut Outcome,
    ) -> Result<bool, BackendError> {
        let mut file = file.clone();
        for _ in 0..WRITE_ATTEMPTS {
            let Some(content) = self.read(root, &file.rel_path).await? else {
                let id = file.document_id.clone();
                self.store(move |store| store.delete_mapped_file(&id))
                    .await?;
                return Ok(true);
            };
            if Some(content.hash) != file.written_hash
                && Some(content.hash) != file.pending_write_hash
            {
                if self.import(root, &file, outcome).await? != Imported::Done {
                    return Ok(false);
                }
                file = self.refetch(&file).await?;
                continue;
            }
            let removed = {
                let path = root.join(&file.rel_path);
                self.blocking(move || {
                    Ok(match fs::symlink_metadata(&path) {
                        Ok(meta) if meta.is_file() => fs::remove_file(&path),
                        Ok(_) => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
                        Err(cause) => Err(cause),
                    })
                })
                .await?
            };
            if let Err(cause) = removed {
                eprintln!(
                    "documents folder sync remove failure: document={} kind={:?}",
                    file.document_id,
                    cause.kind()
                );
                return Ok(false);
            }
            let id = file.document_id.clone();
            self.store(move |store| store.delete_mapped_file(&id))
                .await?;
            outcome.removed += 1;
            return Ok(true);
        }
        Ok(false)
    }

    // ------------------------------------------------------------ helpers

    /// The mapping of `collection`, unless it has none. An archived Collection pauses its
    /// mapping, and a restored one resumes it.
    async fn mapping(&self, collection: &str) -> Result<Option<FolderMapping>, BackendError> {
        let id = collection.to_owned();
        let (mapping, archived) = self
            .store(move |store| {
                let mapping = store.folder_mapping(&id)?;
                let archived = store
                    .collections()?
                    .into_iter()
                    .find(|known| known.id == id)
                    .is_some_and(|known| known.archived);
                Ok((mapping, archived))
            })
            .await?;
        let Some(mapping) = mapping else {
            return Ok(None);
        };
        if archived {
            self.set_state(&mapping, MappingState::Paused).await?;
            return Ok(None);
        }
        if mapping.state == MappingState::Paused {
            self.set_state(&mapping, MappingState::Active).await?;
            return Ok(Some(FolderMapping {
                state: MappingState::Active,
                ..mapping
            }));
        }
        Ok(Some(mapping))
    }

    async fn set_state(
        &self,
        mapping: &FolderMapping,
        state: MappingState,
    ) -> Result<(), BackendError> {
        if mapping.state == state {
            return Ok(());
        }
        let next = FolderMapping {
            state,
            ..mapping.clone()
        };
        self.store(move |store| store.set_folder_mapping(&next))
            .await
    }

    async fn mapped_files(&self, collection: &str) -> Result<Vec<MappedFile>, BackendError> {
        let id = collection.to_owned();
        self.store(move |store| store.mapped_files(&id)).await
    }

    async fn refetch(&self, file: &MappedFile) -> Result<MappedFile, BackendError> {
        let id = file.document_id.clone();
        self.store(move |store| store.mapped_file(&id))
            .await?
            .ok_or_else(storage_failed)
    }

    async fn refusals(&self, collection: &str) -> Result<Vec<RefusedFile>, BackendError> {
        let id = collection.to_owned();
        self.store(move |store| store.refused_files(&id)).await
    }

    async fn documents(&self) -> Result<Vec<DocumentView>, BackendError> {
        self.store(DocumentStore::documents).await
    }

    /// Whether these exact bytes of `rel` were refused before: they are not retried.
    async fn refused_before(
        &self,
        collection: &str,
        rel: &str,
        hash: &FileHash,
    ) -> Result<bool, BackendError> {
        Ok(self
            .refusals(collection)
            .await?
            .iter()
            .any(|refusal| refusal.rel_path == rel && refusal.refused_hash.as_ref() == Some(hash)))
    }

    async fn refuse(
        &self,
        collection: &str,
        rel: &str,
        hash: Option<FileHash>,
        reason: Refusal,
        detail: Option<String>,
        outcome: &mut Outcome,
    ) -> Result<(), BackendError> {
        let refusal = RefusedFile {
            collection_id: collection.to_owned(),
            rel_path: rel.to_owned(),
            refused_hash: hash,
            reason: reason.as_str().to_owned(),
            detail,
        };
        let known = self.refusals(collection).await?.contains(&refusal);
        if known {
            return Ok(());
        }
        self.store(move |store| store.set_refused_file(&refusal))
            .await?;
        outcome.refused += 1;
        Ok(())
    }

    /// The name keys a new file name must avoid: every entry of the folder and every file state
    /// of the mapping, except `own`.
    async fn taken(
        &self,
        root: &Path,
        collection: &str,
        own: Option<&str>,
    ) -> Result<HashSet<String>, BackendError> {
        let mut taken: HashSet<String> = self
            .mapped_files(collection)
            .await?
            .into_iter()
            .map(|file| file.name_key)
            .collect();
        let root = root.to_path_buf();
        let entries = self
            .blocking(move || {
                Ok(fs::read_dir(&root)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| name_key(&entry.file_name().to_string_lossy()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default())
            })
            .await?;
        taken.extend(entries);
        if let Some(own) = own {
            taken.remove(own);
        }
        Ok(taken)
    }

    /// Reads `root/rel` without following a link. `None` when it is missing or not a file.
    async fn read(&self, root: &Path, rel: &str) -> Result<Option<FileContent>, BackendError> {
        let path = root.join(rel);
        let read = self.blocking(move || Ok(read_file(&path))).await?;
        match read {
            Ok(content) => Ok(content),
            Err(cause) => {
                eprintln!(
                    "documents folder sync read failure: kind={:?}",
                    cause.kind()
                );
                Ok(None)
            }
        }
    }

    /// Opens what the body of `document` still needs, then says whether it can be revised.
    async fn available(&self, document: &str) -> Result<bool, BackendError> {
        if reopen_document(&self.log, &self.hub, &self.cancellation, document)
            .await
            .is_err()
        {
            return Ok(false);
        }
        Ok(body_availability(&self.hub, document)? == BodyState::Available)
    }

    fn is_verified(&self, document: &str) -> bool {
        self.verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(document)
    }

    fn verify(&self, document: &str) {
        self.verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(document.to_owned());
    }

    fn unverify(&self, document: &str) {
        self.verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(document);
    }

    /// The Canonical Markdown and version token of the body of `document`.
    fn body(&self, document: &str) -> Option<(String, String)> {
        self.hub.with_body(document, |body| {
            encode_version(&body.frontiers()).map(|version| (body.text().to_owned(), version))
        })
    }

    fn version(&self, document: &str) -> Option<String> {
        self.hub
            .with_body(document, |body| encode_version(&body.frontiers()))
    }

    async fn append(&self, record: DomainRecord) -> Result<serde_json::Value, BackendError> {
        append_domain_record(
            &self.log,
            &self.hub,
            &self.changes,
            &self.cancellation,
            record,
            None,
        )
        .await
    }

    /// Runs `work` on the store, on the blocking pool.
    async fn store<T, F>(&self, work: F) -> Result<T, BackendError>
    where
        T: Send + 'static,
        F: FnOnce(&DocumentStore) -> Result<T, StoreError> + Send + 'static,
    {
        let store = self.hub.store.clone();
        self.blocking(move || work(&store).map_err(|_| storage_failed()))
            .await
    }

    /// Runs file or CPU work on the blocking pool, raced against the cancellation.
    async fn blocking<T, F>(&self, work: F) -> Result<T, BackendError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, BackendError> + Send + 'static,
    {
        teamarness_package_sdk::blocking(&self.cancellation, move |_| work()).await
    }
}

/// The request ID of the import of a file whose bytes hash to `file` into `document`, as a
/// revision of the version `base`: the first 32 hex characters of SHA-256 over
/// `"teamarness-folder-sync\0" ‖ document ‖ "\0" ‖ base ‖ "\0" ‖ hex(file)`.
pub(crate) fn request_id(document: &str, base: &str, file: &FileHash) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"teamarness-folder-sync\0");
    hasher.update(document.as_bytes());
    hasher.update(b"\0");
    hasher.update(base.as_bytes());
    hasher.update(b"\0");
    hasher.update(hex(file).as_bytes());
    let mut id = hex(&hasher.finalize());
    id.truncate(32);
    id
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The Document title a file name gives: its stem, with control characters replaced by `-`
/// and the ends trimmed, or `Untitled`.
pub(crate) fn title_of(rel: &str) -> String {
    let stem = rel.strip_suffix(EXTENSION).unwrap_or(rel);
    let title: String = stem
        .chars()
        .map(|c| if c.is_control() { '-' } else { c })
        .collect();
    let title = title.trim();
    if title.is_empty() {
        "Untitled".to_owned()
    } else {
        title.to_owned()
    }
}

/// The text of a file whose import may go ahead: UTF-8, within the bounds, held by the schema
/// and losing no token. Otherwise the refusal and its detail.
fn checked_text(bytes: Option<Vec<u8>>) -> Result<String, (Refusal, Option<String>)> {
    let Some(bytes) = bytes else {
        return Err((Refusal::TooLarge, None));
    };
    let text = String::from_utf8(bytes).map_err(|_| (Refusal::NotUtf8, None))?;
    match markdown::lost_tokens(&text) {
        Ok(lost) if lost.is_empty() => Ok(text),
        Ok(lost) => Err((Refusal::Lossy, Some(lost.join(" ")))),
        Err(_) => Err((Refusal::SchemaViolation, None)),
    }
}

/// The pre-check of a new file, before any record is written: [`checked_text`], canonical
/// text within [`MAX_TEXT_BYTES`], a title within [`MAX_TITLE`], and a dry-run merge against
/// `genesis` whose one record fits [`MAX_UPDATE`]. Returns the title and the text.
fn precheck(
    rel: &str,
    bytes: Option<Vec<u8>>,
    genesis: &[u8],
) -> Result<(String, String), (Refusal, Option<String>)> {
    let text = checked_text(bytes)?;
    let canonical = markdown::canonical(&text).map_err(|_| (Refusal::SchemaViolation, None))?;
    if canonical.len() > MAX_TEXT_BYTES {
        return Err((Refusal::TooLarge, None));
    }
    let title = title_of(rel);
    if title.chars().count() > MAX_TITLE {
        return Err((Refusal::TitleTooLong, None));
    }
    if !text.is_empty() {
        let refused = |cause: BodyError| match cause {
            BodyError::RecordTooLarge | BodyError::TooLarge => (Refusal::TooLarge, None),
            BodyError::Schema(_) => (Refusal::SchemaViolation, None),
            _ => (Refusal::MergeRefused, None),
        };
        let (body, _) = DocumentBody::new()
            .import_candidate(genesis)
            .map_err(|_| (Refusal::MergeRefused, None))?;
        let (source, peer) = body.merge_source().map_err(refused)?;
        let base = crate::body::genesis_frontiers(genesis);
        let merged = crate::body::merge_revision(&source, &base, &text, peer).map_err(refused)?;
        if merged.update.len() > MAX_UPDATE {
            return Err((Refusal::TooLarge, None));
        }
        body.check_merged(peer, "0".repeat(32), &merged)
            .map_err(refused)?;
    }
    Ok((title, text))
}

/// The refusal a failed staging of an import becomes, or `None` when it may succeed later.
fn merge_refusal(cause: &BackendError) -> Option<(Refusal, String)> {
    let reason = match cause.code() {
        "documents.edit_request_too_large" | "documents.document_too_large" => Refusal::TooLarge,
        "documents.schema_violation" => Refusal::SchemaViolation,
        "documents.base_unavailable"
        | "documents.invalid_update"
        | "documents.invalid_request"
        | "documents.foreign_peer" => Refusal::MergeRefused,
        _ => return None,
    };
    Some((reason, cause.code().to_owned()))
}

/// Reads `path` without following a link: `None` when it is missing or not a regular file. A
/// file over [`MAX_FILE_BYTES`] is hashed but not kept.
fn read_file(path: &Path) -> io::Result<Option<FileContent>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(cause),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    if metadata.len() > MAX_FILE_BYTES {
        let (hash, _) = sha256_reader(file)?;
        return Ok(Some(FileContent { hash, bytes: None }));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        let (hash, _) = sha256_reader(File::open(path)?)?;
        return Ok(Some(FileContent { hash, bytes: None }));
    }
    Ok(Some(FileContent {
        hash: sha256(&bytes),
        bytes: Some(bytes),
    }))
}

/// Renames `root/from` to `root/to`, never over another file. A rename that changes only case
/// is the same file on a case-insensitive file system, so it is allowed.
fn rename_file(root: &Path, from: &str, to: &str) -> io::Result<()> {
    let source = root.join(from);
    let target = root.join(to);
    if !fs::symlink_metadata(&source)?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    if name_key(from) != name_key(to) && fs::symlink_metadata(&target).is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    fs::rename(source, target)
}

fn log_refused_write(document: &str, refused: &WriteRefused) {
    let cause = match refused {
        WriteRefused::Changed { found } => format!("changed:present={}", found.is_some()),
        WriteRefused::OutsideRoot => "outside_root".to_owned(),
        WriteRefused::NotADirectory => "not_a_directory".to_owned(),
        WriteRefused::Symlink => "symlink".to_owned(),
        WriteRefused::NotAFile => "not_a_file".to_owned(),
        WriteRefused::Io(error) => format!("io:{:?}", error.kind()),
    };
    eprintln!("documents folder sync write refused: document={document} cause={cause}");
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
pub(super) mod tests;
