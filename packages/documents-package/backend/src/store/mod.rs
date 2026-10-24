pub(crate) mod migration;
pub(crate) mod projection;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{DomainRecord, quarantine};
use projection::WAITING;

pub(crate) type ImportClassifier<'a> =
    dyn Fn(&str, &str, &str, Option<&str>, usize, &[u8]) -> Option<&'static str> + 'a;

#[derive(Debug)]
pub enum StoreError {
    Storage,
    Migration,
    Lock,
    UnsupportedSchema,
    UncertainOutcome,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StoreError {}

#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub record_id: String,
    pub order_key: String,
    pub author: String,
    pub purpose: String,
    pub signature: String,
    pub sealed_record: Vec<u8>,
    pub cursor: Option<String>,
    pub ingestion_ordinal: u64,
    /// The Document a body edit belongs to; `None` for metadata records.
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionView {
    pub id: String,
    pub creator: String,
    pub archived: bool,
    /// `None` until the create or latest rename record has been opened.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentView {
    pub id: String,
    pub creator: String,
    pub collection_id: String,
    pub archived: bool,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentView {
    pub id: String,
    pub document_id: String,
    pub parent_id: Option<String>,
    pub author: String,
    pub order_key: String,
    pub resolved: bool,
    pub created_at: Option<String>,
    /// `None` until the add record has been opened.
    pub markdown: Option<String>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestResult {
    Applied,
    Duplicate,
    Deferred(String),
    Quarantined(String),
    IdentityConflict(String),
    ImportFailure(String),
}

/// What one ingest committed, plus every body update it made applicable: the record's own
/// edit, and any deferred edits a newly created Document unblocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub result: IngestResult,
    pub edits: Vec<(String, Vec<u8>)>,
}

/// The Package's durable projection: sealed records, ordering, and archive state in SQLite;
/// names and titles only in memory, remembered from opened records.
#[derive(Debug, Clone)]
pub struct DocumentStore {
    pub(crate) inner: Arc<Mutex<StoreInner>>,
    ingest_gate: Arc<Mutex<()>>,
}

#[derive(Debug)]
pub(crate) struct StoreInner {
    pub(crate) conn: Connection,
    /// Collection name or Document title by id, with the order key that set it.
    pub(crate) texts: HashMap<String, (String, String)>,
    /// Comment Markdown and optional quote by Comment id; a Comment never changes.
    pub(crate) comments: HashMap<String, (String, Option<String>)>,
    pub(crate) deferred: HashMap<String, DomainRecord>,
}

fn lock(inner: &Mutex<StoreInner>) -> Result<std::sync::MutexGuard<'_, StoreInner>, StoreError> {
    inner.lock().map_err(|_| StoreError::Lock)
}
fn rerr(e: rusqlite::Error) -> StoreError {
    let _ = e;
    StoreError::Storage
}

/// Remembers `text` for `id` unless a later-ordered record already set it.
fn remember_text(
    texts: &mut HashMap<String, (String, String)>,
    id: String,
    order: String,
    text: String,
) {
    match texts.get(&id) {
        Some((existing, _)) if *existing >= order => {}
        _ => {
            texts.insert(id, (order, text));
        }
    }
}

impl DocumentStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut conn = Connection::open(path.as_ref()).map_err(|_| StoreError::Storage)?;
        migration::apply(&mut conn)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(StoreInner {
                conn,
                texts: HashMap::new(),
                comments: HashMap::new(),
                deferred: HashMap::new(),
            })),
            ingest_gate: Arc::new(Mutex::new(())),
        })
    }

    pub fn cursor(&self) -> Result<Option<String>, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row("SELECT cursor FROM metadata WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .map_err(rerr)
    }

    /// Clears the committed delivery cursor; the session then replays from the beginning and
    /// ingestion deduplicates by record identity.
    pub fn reset_cursor(&self) -> Result<(), StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .execute("UPDATE metadata SET cursor=NULL WHERE singleton=1", [])
            .map(|_| ())
            .map_err(rerr)
    }

    /// Tracks sealed records already projected before this activation. A from-start
    /// marker cannot certify the old projection unless Core replayed every one.
    pub fn begin_replay_qualification(&self) -> Result<(), StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS replay_expected (record_id TEXT PRIMARY KEY);
             DELETE FROM replay_expected;
             INSERT INTO replay_expected SELECT record_id FROM records;",
            )
            .map_err(rerr)
    }

    pub fn note_replayed(&self, id: &str) -> Result<(), StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .execute("DELETE FROM replay_expected WHERE record_id=?1", [id])
            .map_err(rerr)?;
        Ok(())
    }

    pub fn replayed_previous_projection(&self) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM replay_expected)",
                [],
                |row| row.get(0),
            )
            .map_err(rerr)
    }

    /// Exact synchronous lookup; absence is meaningful only after qualified replay/open.
    pub fn edit_identity(&self, id: &str) -> Result<Option<(String, String, String)>, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT record_id,document_id,author FROM edit_identities WHERE request_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(rerr)
    }

    pub fn identity_conflicted(&self, document: &str) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM document_identity_conflicts WHERE document_id=?1)",
                [document],
                |row| row.get(0),
            )
            .map_err(rerr)
    }

    pub fn import_failure(&self, document: &str) -> Result<Option<(String, String)>, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT record_id,category FROM document_import_failures WHERE document_id=?1",
                [document],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(rerr)
    }

    pub fn import_failure_ordinal(&self, document: &str) -> Result<Option<u64>, StoreError> {
        let inner = lock(&self.inner)?;
        inner.conn.query_row(
            "SELECT r.ingestion_ordinal FROM document_import_failures f JOIN records r ON r.record_id=f.record_id WHERE f.document_id=?1",
            [document], |row| row.get::<_, i64>(0).map(|value| value as u64),
        ).optional().map_err(rerr)
    }

    /// For pre-v6 history discovered during activation, before the body is published.
    pub fn mark_import_failure(
        &self,
        document: &str,
        record_id: &str,
        category: &str,
    ) -> Result<(), StoreError> {
        let inner = lock(&self.inner)?;
        inner.conn.execute(
            "INSERT OR IGNORE INTO document_import_failures(document_id,record_id,category) VALUES(?1,?2,?3)",
            params![document, record_id, category],
        ).map_err(rerr)?;
        Ok(())
    }

    pub fn conflicted_documents(&self) -> Result<Vec<String>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut query = inner
            .conn
            .prepare("SELECT document_id FROM document_identity_conflicts ORDER BY document_id")
            .map_err(rerr)?;
        let rows = query.query_map([], |row| row.get(0)).map_err(rerr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(rerr)
    }

    /// Inspect applied and deferred edits before importing any body on restart.
    pub fn identity_candidates(&self) -> Result<Vec<StoredRecord>, StoreError> {
        self.records_where(&format!(
            "purpose='{}' AND (quarantine_code IS NULL OR quarantine_code='{WAITING}')",
            DomainRecord::EDIT_KIND
        ))
    }

    pub fn indexed_edit_record(&self, id: &str) -> Result<Option<StoredRecord>, StoreError> {
        let Some((record_id, _, _)) = self.edit_identity(id)? else {
            return Ok(None);
        };
        let inner = lock(&self.inner)?;
        let mut statement = inner.conn.prepare(
            "SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,target
             FROM records WHERE record_id=?1 AND quarantine_code IS NULL",
        ).map_err(rerr)?;
        statement
            .query_row([record_id], |row| {
                Ok(StoredRecord {
                    record_id: row.get(0)?,
                    order_key: row.get(1)?,
                    author: row.get(2)?,
                    purpose: row.get(3)?,
                    signature: row.get(4)?,
                    sealed_record: row.get(5)?,
                    cursor: row.get(6)?,
                    ingestion_ordinal: row.get::<_, i64>(7)? as u64,
                    target: row.get(8)?,
                })
            })
            .optional()
            .map_err(rerr)
    }

    /// An opened stored row is indexed only after its authenticated bytes agree with its
    /// immutable metadata. Deferred v4 rows are indexed before retrying their projection;
    /// a collision is quarantined before its Loro update can be imported.
    pub fn index_opened_edit(
        &self,
        row: &StoredRecord,
        canonical: &[u8],
    ) -> Result<bool, StoreError> {
        let domain = DomainRecord::decode(canonical).map_err(|_| StoreError::Storage)?;
        let DomainRecord::IdentifiedDocumentEdit {
            author,
            document,
            request_id,
            ..
        } = &domain
        else {
            return Ok(true);
        };
        if row.purpose != domain.kind()
            || row.author != *author
            || row.target.as_deref() != Some(document.as_str())
            || row.record_id.len() != 64
            || !row
                .record_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(StoreError::Storage);
        }
        let mut inner = lock(&self.inner)?;
        let tx = inner.conn.transaction().map_err(rerr)?;
        let valid: Option<Option<String>> = tx.query_row(
            "SELECT quarantine_code FROM records WHERE record_id=?1 AND sealed_record=?2 AND author=?3 AND purpose=?4 AND target=?5",
            params![row.record_id, row.sealed_record, row.author, row.purpose, document], |r| r.get(0),
        ).optional().map_err(rerr)?;
        if valid != Some(None) && valid.as_ref().and_then(|v| v.as_deref()) != Some(WAITING) {
            return Err(StoreError::Storage);
        }
        let accepted = index_identity(&tx, request_id, &row.record_id, document, author)?;
        if !accepted {
            quarantine(&tx, &row.record_id, "request_id_conflict").map_err(rerr)?;
        }
        tx.commit().map_err(rerr)?;
        if !accepted {
            inner.deferred.remove(&row.record_id);
        }
        Ok(accepted)
    }

    pub fn record_count(&self) -> Result<u64, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })
            .map_err(rerr)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ingest(
        &self,
        record_id: &str,
        order_key: &str,
        author: &str,
        purpose: &str,
        signature: &str,
        canonical: &[u8],
        sealed: &[u8],
        cursor: Option<&str>,
    ) -> Result<IngestResult, StoreError> {
        self.ingest_with_commit(
            record_id,
            order_key,
            author,
            purpose,
            signature,
            canonical,
            sealed,
            cursor,
            &|| {},
        )
        .map(|ingested| ingested.result)
    }

    /// Commits the sealed row, its projection, and the delivery cursor in one transaction
    /// (ADR 0008), then publishes through `on_commit`.
    ///
    /// A create record may unblock deferred dependants that commit in their own transactions;
    /// the cursor stays at its predecessor until every retry has committed, so a failure in
    /// between redelivers the create and repeats the retries.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ingest_with_commit(
        &self,
        record_id: &str,
        order_key: &str,
        author: &str,
        purpose: &str,
        signature: &str,
        canonical: &[u8],
        sealed: &[u8],
        cursor: Option<&str>,
        on_commit: &dyn Fn(),
    ) -> Result<Ingested, StoreError> {
        self.ingest_with_import(
            record_id,
            order_key,
            author,
            purpose,
            signature,
            canonical,
            sealed,
            cursor,
            on_commit,
            &|_, _, _, _, _, _| None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ingest_with_import(
        &self,
        record_id: &str,
        order_key: &str,
        author: &str,
        purpose: &str,
        signature: &str,
        canonical: &[u8],
        sealed: &[u8],
        cursor: Option<&str>,
        on_commit: &dyn Fn(),
        classify: &ImportClassifier<'_>,
    ) -> Result<Ingested, StoreError> {
        let _ingest = self.ingest_gate.lock().map_err(|_| StoreError::Lock)?;
        let parsed = DomainRecord::decode(canonical);
        let mut inner = lock(&self.inner)?;
        let transaction = inner.conn.transaction().map_err(rerr)?;
        let existing: Option<(String, String, String, String, Vec<u8>)> = transaction
            .query_row(
                "SELECT order_key,author,purpose,signature,sealed_record FROM records WHERE record_id=?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(rerr)?;
        if let Some((
            stored_order,
            stored_author,
            stored_purpose,
            stored_signature,
            stored_sealed,
        )) = existing
        {
            if stored_order == order_key
                && stored_author == author
                && stored_purpose == purpose
                && stored_signature == signature
                && stored_sealed == sealed
            {
                let created = match &parsed {
                    Ok(
                        DomainRecord::CollectionCreate { .. } | DomainRecord::DocumentCreate { .. },
                    ) => Some(record_id.to_owned()),
                    _ => None,
                };
                let code: Option<String> = transaction
                    .query_row(
                        "SELECT quarantine_code FROM records WHERE record_id=?1",
                        [record_id],
                        |r| r.get(0),
                    )
                    .map_err(rerr)?;
                if let Some(code) = code {
                    if let Some(cursor) = cursor {
                        transaction
                            .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                            .map_err(rerr)?;
                    }
                    transaction.commit().map_err(rerr)?;
                    on_commit();
                    let result = if code == "request_id_conflict" {
                        IngestResult::IdentityConflict(
                            parsed
                                .as_ref()
                                .ok()
                                .and_then(DomainRecord::target)
                                .unwrap_or("")
                                .to_owned(),
                        )
                    } else {
                        IngestResult::Quarantined(code)
                    };
                    return Ok(Ingested {
                        result,
                        edits: Vec::new(),
                    });
                }
                if let Ok(DomainRecord::IdentifiedDocumentEdit {
                    author: owner,
                    document,
                    request_id,
                    ..
                }) = &parsed
                    && owner == author
                    && purpose == DomainRecord::EDIT_KIND
                    && !index_identity(&transaction, request_id, record_id, document, owner)?
                {
                    quarantine(&transaction, record_id, "request_id_conflict").map_err(rerr)?;
                    if let Some(cursor) = cursor {
                        transaction
                            .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                            .map_err(rerr)?;
                    }
                    transaction.commit().map_err(rerr)?;
                    on_commit();
                    return Ok(Ingested {
                        result: IngestResult::IdentityConflict(document.clone()),
                        edits: Vec::new(),
                    });
                }
                if created.is_none()
                    && let Some(cursor) = cursor
                {
                    transaction
                        .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                        .map_err(rerr)?;
                }
                transaction.commit().map_err(rerr)?;
                on_commit();
                drop(inner);
                let mut edits = Vec::new();
                if let Some(target) = created {
                    self.retry_for_target(&target, on_commit, &mut edits, classify)?;
                    self.commit_delivery_cursor(cursor)?;
                }
                return Ok(Ingested {
                    result: IngestResult::Duplicate,
                    edits,
                });
            }
            quarantine(&transaction, record_id, "immutable_field_mismatch").map_err(rerr)?;
            transaction.commit().map_err(rerr)?;
            on_commit();
            return Ok(Ingested {
                result: IngestResult::Quarantined("immutable_field_mismatch".into()),
                edits: Vec::new(),
            });
        }
        let previous: i64 = transaction
            .query_row(
                "SELECT next_ingestion_ordinal FROM metadata WHERE singleton=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(rerr)?;
        let ordinal = previous + 1;
        transaction
            .execute(
                "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)",
                params![record_id, order_key, author, purpose, signature, sealed, cursor, ordinal],
            )
            .map_err(rerr)?;
        let mut outcome = match parsed {
            Ok(domain) => {
                if let DomainRecord::IdentifiedDocumentEdit {
                    author: owner,
                    document,
                    request_id,
                    ..
                } = &domain
                    && owner == author
                    && purpose == DomainRecord::EDIT_KIND
                    && !index_identity(&transaction, request_id, record_id, document, owner)?
                {
                    quarantine(&transaction, record_id, "request_id_conflict").map_err(rerr)?;
                    transaction.execute(
                        "UPDATE metadata SET cursor=?1,next_ingestion_ordinal=?2 WHERE singleton=1",
                        params![cursor, ordinal],
                    ).map_err(rerr)?;
                    transaction.commit().map_err(rerr)?;
                    on_commit();
                    return Ok(Ingested {
                        result: IngestResult::IdentityConflict(document.clone()),
                        edits: Vec::new(),
                    });
                }
                let outcome =
                    projection::project_domain(&transaction, record_id, order_key, author, &domain)
                        .map_err(rerr)?;
                if matches!(outcome.result, IngestResult::Quarantined(ref code) if code != "import_failure")
                    && let DomainRecord::IdentifiedDocumentEdit { request_id, .. } = &domain
                {
                    transaction
                        .execute(
                            "DELETE FROM edit_identities WHERE request_id=?1 AND record_id=?2",
                            params![request_id, record_id],
                        )
                        .map_err(rerr)?;
                }
                outcome
            }
            Err(_) => {
                quarantine(&transaction, record_id, "malformed_record").map_err(rerr)?;
                projection::ProjectionOutcome {
                    result: IngestResult::Quarantined("malformed_record".into()),
                    ..projection::ProjectionOutcome::applied()
                }
            }
        };
        if let Some((document, update)) = &outcome.edit
            && let Some(category) = classify(
                record_id,
                document,
                author,
                match DomainRecord::decode(canonical) {
                    Ok(DomainRecord::IdentifiedDocumentEdit { request_id, .. }) => Some(request_id),
                    _ => None,
                }
                .as_deref(),
                sealed.len(),
                update,
            )
        {
            transaction.execute(
                    "INSERT OR IGNORE INTO document_import_failures(document_id,record_id,category) VALUES(?1,?2,?3)",
                    params![document, record_id, category],
                ).map_err(rerr)?;
            outcome.result = IngestResult::ImportFailure(document.clone());
            outcome.edit = None;
        }
        if outcome.created.is_some() {
            transaction
                .execute(
                    "UPDATE metadata SET next_ingestion_ordinal=?1 WHERE singleton=1",
                    [ordinal],
                )
                .map_err(rerr)?;
        } else {
            transaction
                .execute(
                    "UPDATE metadata SET cursor=?1,next_ingestion_ordinal=?2 WHERE singleton=1",
                    params![cursor, ordinal],
                )
                .map_err(rerr)?;
        }
        transaction.commit().map_err(rerr)?;
        on_commit();
        if let Some((id, order, text)) = outcome.text {
            remember_text(&mut inner.texts, id, order, text);
        }
        if let Some((id, markdown, quote)) = outcome.comment {
            inner.comments.insert(id, (markdown, quote));
        }
        if let Some(domain) = outcome.deferred {
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        let created = outcome.created;
        let result = outcome.result;
        let mut edits: Vec<(String, Vec<u8>)> = outcome.edit.into_iter().collect();
        drop(inner);
        if let Some(target) = created {
            self.retry_for_target(&target, on_commit, &mut edits, classify)?;
            self.commit_delivery_cursor(cursor)?;
        }
        Ok(Ingested { result, edits })
    }

    fn commit_delivery_cursor(&self, cursor: Option<&str>) -> Result<(), StoreError> {
        if let Some(cursor) = cursor {
            let inner = lock(&self.inner)?;
            inner
                .conn
                .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                .map_err(rerr)?;
        }
        Ok(())
    }

    /// Re-projects every deferred record that waits on `target`, in order, collecting the body
    /// updates that became applicable.
    fn retry_for_target(
        &self,
        target: &str,
        on_commit: &dyn Fn(),
        edits: &mut Vec<(String, Vec<u8>)>,
        classify: &ImportClassifier<'_>,
    ) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner)?;
        let mut waiting: Vec<(String, DomainRecord)> = inner
            .deferred
            .iter()
            .filter(|(_, domain)| waits_on(domain, target))
            .map(|(id, domain)| (id.clone(), domain.clone()))
            .collect();
        let order_of =
            |inner: &StoreInner, record_id: &str| -> Result<(String, String), StoreError> {
                inner
                    .conn
                    .query_row(
                        "SELECT order_key,author FROM records WHERE record_id=?1",
                        [record_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(rerr)
            };
        let mut keyed = Vec::new();
        for (record_id, domain) in waiting.drain(..) {
            let (order, author) = order_of(&inner, &record_id)?;
            keyed.push((order, record_id, author, domain));
        }
        keyed.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        for (order_key, record_id, author, domain) in keyed {
            let transaction = inner.conn.transaction().map_err(rerr)?;
            let mut outcome =
                projection::project_domain(&transaction, &record_id, &order_key, &author, &domain)
                    .map_err(rerr)?;
            if let Some((document, update)) = &outcome.edit
                && let Some(category) = classify(
                    &record_id,
                    document,
                    &author,
                    match &domain {
                        DomainRecord::IdentifiedDocumentEdit { request_id, .. } => {
                            Some(request_id.as_str())
                        }
                        _ => None,
                    },
                    transaction
                        .query_row(
                            "SELECT length(sealed_record) FROM records WHERE record_id=?1",
                            [&record_id],
                            |row| row.get(0),
                        )
                        .map_err(rerr)?,
                    update,
                )
            {
                transaction.execute(
                        "INSERT OR IGNORE INTO document_import_failures(document_id,record_id,category) VALUES(?1,?2,?3)",
                        params![document, record_id, category],
                    ).map_err(rerr)?;
                outcome.result = IngestResult::ImportFailure(document.clone());
                outcome.edit = None;
            }
            transaction.commit().map_err(rerr)?;
            on_commit();
            if let Some((id, order, text)) = outcome.text {
                remember_text(&mut inner.texts, id, order, text);
            }
            if let Some((id, markdown, quote)) = outcome.comment {
                inner.comments.insert(id, (markdown, quote));
            }
            if let Some(edit) = outcome.edit {
                edits.push(edit);
            }
            if !matches!(outcome.result, IngestResult::Deferred(_)) {
                inner.deferred.remove(&record_id);
            }
            if let Some(created) = outcome.created {
                // A deferred Document create can itself unblock its dependants.
                drop(inner);
                self.retry_for_target(&created, on_commit, edits, classify)?;
                inner = lock(&self.inner)?;
            }
        }
        Ok(())
    }

    pub fn has_deferred_records(&self) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE quarantine_code=?1)",
                [WAITING],
                |row| row.get(0),
            )
            .map_err(rerr)
    }

    pub fn deferred_records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        self.records_where(&format!("quarantine_code='{WAITING}'"))
    }

    /// Every projected metadata record, oldest first, for rehydrating names after restart.
    pub fn control_records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        let kinds = DomainRecord::METADATA_KINDS
            .iter()
            .map(|kind| format!("'{kind}'"))
            .collect::<Vec<_>>()
            .join(",");
        self.records_where(&format!("purpose IN ({kinds}) AND quarantine_code IS NULL"))
    }

    fn records_where(&self, filter: &str) -> Result<Vec<StoredRecord>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare(&format!(
                "SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,target FROM records WHERE {filter} ORDER BY order_key,record_id"
            ))
            .map_err(rerr)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredRecord {
                    record_id: row.get(0)?,
                    order_key: row.get(1)?,
                    author: row.get(2)?,
                    purpose: row.get(3)?,
                    signature: row.get(4)?,
                    sealed_record: row.get(5)?,
                    cursor: row.get(6)?,
                    ingestion_ordinal: row.get::<_, i64>(7)? as u64,
                    target: row.get(8)?,
                })
            })
            .map_err(rerr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(rerr)
    }

    /// Rehydrates a name or title from an opened record after restart. The bytes are never stored.
    pub fn remember_opened(
        &self,
        record_id: &str,
        order_key: &str,
        canonical: &[u8],
    ) -> Result<(), StoreError> {
        let domain = DomainRecord::decode(canonical).map_err(|_| StoreError::Storage)?;
        let (id, text) = match &domain {
            DomainRecord::CollectionCreate { name, .. } => (record_id.to_owned(), name.clone()),
            DomainRecord::DocumentCreate { title, .. } => (record_id.to_owned(), title.clone()),
            DomainRecord::CollectionRename {
                collection, name, ..
            } => (collection.clone(), name.clone()),
            DomainRecord::DocumentRename {
                document, title, ..
            } => (document.clone(), title.clone()),
            DomainRecord::CommentAdd {
                markdown, quote, ..
            } => {
                let mut inner = lock(&self.inner)?;
                inner
                    .comments
                    .insert(record_id.to_owned(), (markdown.clone(), quote.clone()));
                return Ok(());
            }
            _ => return Ok(()),
        };
        let mut inner = lock(&self.inner)?;
        remember_text(&mut inner.texts, id, order_key.to_owned(), text);
        Ok(())
    }

    /// Retries one deferred record whose sealed bytes were re-opened after restart.
    pub fn retry_deferred(
        &self,
        record_id: &str,
        canonical: &[u8],
    ) -> Result<Ingested, StoreError> {
        self.retry_deferred_with_import(record_id, canonical, &|| {}, &|_, _, _, _, _, _| None)
    }

    pub(crate) fn retry_deferred_with_import(
        &self,
        record_id: &str,
        canonical: &[u8],
        on_commit: &dyn Fn(),
        classify: &ImportClassifier<'_>,
    ) -> Result<Ingested, StoreError> {
        let _ingest = self.ingest_gate.lock().map_err(|_| StoreError::Lock)?;
        let Ok(domain) = DomainRecord::decode(canonical) else {
            return Ok(Ingested {
                result: IngestResult::Quarantined("malformed_record".into()),
                edits: Vec::new(),
            });
        };
        let Some(target) = domain.target().map(str::to_owned) else {
            return Ok(Ingested {
                result: IngestResult::Duplicate,
                edits: Vec::new(),
            });
        };
        {
            let mut inner = lock(&self.inner)?;
            let code: Option<String> = inner
                .conn
                .query_row(
                    "SELECT quarantine_code FROM records WHERE record_id=?1",
                    [record_id],
                    |r| r.get(0),
                )
                .map_err(rerr)?;
            if code.as_deref() != Some(WAITING) {
                return Ok(Ingested {
                    result: code
                        .map(IngestResult::Quarantined)
                        .unwrap_or(IngestResult::Duplicate),
                    edits: Vec::new(),
                });
            }
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        let mut edits = Vec::new();
        self.retry_for_target(&target, on_commit, &mut edits, classify)?;
        let inner = lock(&self.inner)?;
        let code: Option<String> = inner
            .conn
            .query_row(
                "SELECT quarantine_code FROM records WHERE record_id=?1",
                [record_id],
                |row| row.get(0),
            )
            .map_err(rerr)?;
        drop(inner);
        let result = match code.as_deref() {
            None if self.import_failure(&target)?.is_some() => IngestResult::ImportFailure(target),
            None => IngestResult::Applied,
            Some(WAITING) => IngestResult::Deferred(WAITING.into()),
            Some(code) => IngestResult::Quarantined(code.to_owned()),
        };
        Ok(Ingested { result, edits })
    }

    /// Every applied record a body is built from, oldest first, optionally for one Document:
    /// the `document.create` that carries its genesis, then its `document.edit` records.
    /// [`StoredRecord::target`] is the Document for both.
    pub fn body_records(&self, document: Option<&str>) -> Result<Vec<StoredRecord>, StoreError> {
        let filter = match document {
            Some(id) => {
                validate_sql_identity(id)?;
                format!(
                    "((purpose='{create}' AND record_id='{id}') OR (purpose='{edit}' AND target='{id}'))",
                    create = DomainRecord::CREATE_KIND,
                    edit = DomainRecord::EDIT_KIND,
                )
            }
            None => format!(
                "purpose IN ('{}','{}')",
                DomainRecord::CREATE_KIND,
                DomainRecord::EDIT_KIND
            ),
        };
        let mut rows = self.records_where(&format!(
            "{filter} AND quarantine_code IS NULL AND NOT EXISTS (SELECT 1 FROM document_identity_conflicts WHERE document_id=COALESCE(target,record_id))"
        ))?;
        for row in &mut rows {
            if row.purpose == DomainRecord::CREATE_KIND {
                row.target = Some(row.record_id.clone());
            }
        }
        Ok(rows)
    }

    /// Every applied body edit record, oldest first, optionally for one Document.
    pub fn edit_records(&self, document: Option<&str>) -> Result<Vec<StoredRecord>, StoreError> {
        match document {
            Some(id) => {
                validate_sql_identity(id)?;
                self.records_where(&format!(
                    "purpose='{}' AND quarantine_code IS NULL AND target='{id}' AND NOT EXISTS (SELECT 1 FROM document_identity_conflicts WHERE document_id=target)",
                    DomainRecord::EDIT_KIND
                ))
            }
            None => self.records_where(&format!(
                "purpose='{}' AND quarantine_code IS NULL AND NOT EXISTS (SELECT 1 FROM document_identity_conflicts WHERE document_id=target)",
                DomainRecord::EDIT_KIND
            )),
        }
    }

    pub(crate) fn has_projection_warnings(&self) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM records WHERE quarantine_code IS NOT NULL AND quarantine_code!='{WAITING}')"),
                [],
                |row| row.get(0),
            )
            .map_err(rerr)
    }

    pub fn comment_exists(&self, id: &str) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row("SELECT 1 FROM comments WHERE comment_id=?1", [id], |_| {
                Ok(())
            })
            .optional()
            .map(|found| found.is_some())
            .map_err(rerr)
    }

    /// The Comments of one Document in creation order, threads included.
    pub fn comments(&self, document: &str) -> Result<Vec<CommentView>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare("SELECT comment_id,parent_id,author,create_order,resolved,created_at FROM comments WHERE document_id=?1 ORDER BY create_order,comment_id")
            .map_err(rerr)?;
        let rows = statement
            .query_map([document], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(rerr)?;
        rows.map(|row| {
            row.map(|(id, parent_id, author, order_key, resolved, created_at)| {
                let text = inner.comments.get(&id);
                CommentView {
                    markdown: text.map(|(markdown, _)| markdown.clone()),
                    quote: text.and_then(|(_, quote)| quote.clone()),
                    id,
                    document_id: document.to_owned(),
                    parent_id,
                    author,
                    order_key,
                    resolved,
                    created_at,
                }
            })
            .map_err(rerr)
        })
        .collect()
    }

    pub fn collection_exists(&self, id: &str) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT 1 FROM collections WHERE collection_id=?1",
                [id],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .map_err(rerr)
    }
    pub fn document_exists(&self, id: &str) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row("SELECT 1 FROM documents WHERE document_id=?1", [id], |_| {
                Ok(())
            })
            .optional()
            .map(|found| found.is_some())
            .map_err(rerr)
    }

    pub fn collections(&self) -> Result<Vec<CollectionView>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare("SELECT collection_id,creator,archived FROM collections ORDER BY create_order,collection_id")
            .map_err(rerr)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(rerr)?;
        rows.map(|row| {
            row.map(|(id, creator, archived)| CollectionView {
                name: inner.texts.get(&id).map(|(_, text)| text.clone()),
                id,
                creator,
                archived,
            })
            .map_err(rerr)
        })
        .collect()
    }

    pub fn documents(&self) -> Result<Vec<DocumentView>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner
            .conn
            .prepare("SELECT document_id,creator,collection_id,archived FROM documents ORDER BY create_order,document_id")
            .map_err(rerr)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })
            .map_err(rerr)?;
        rows.map(|row| {
            row.map(|(id, creator, collection_id, archived)| DocumentView {
                title: inner.texts.get(&id).map(|(_, text)| text.clone()),
                id,
                creator,
                collection_id,
                archived,
            })
            .map_err(rerr)
        })
        .collect()
    }
}

/// Insert only a provenance-consistent identity. Both duplicate and conflicting record IDs
/// are decided in the same transaction that projects the sealed record and cursor.
fn index_identity(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    record_id: &str,
    document: &str,
    author: &str,
) -> Result<bool, StoreError> {
    let prior: Option<(String, String, String)> = tx
        .query_row(
            "SELECT record_id,document_id,author FROM edit_identities WHERE request_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(rerr)?;
    if let Some(prior) = prior {
        if prior == (record_id.to_owned(), document.to_owned(), author.to_owned()) {
            return Ok(true);
        }
        for target in [&prior.1, document] {
            tx.execute(
                "INSERT OR IGNORE INTO document_identity_conflicts(document_id) VALUES(?1)",
                [target],
            )
            .map_err(rerr)?;
        }
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO edit_identities(request_id,record_id,document_id,author) VALUES(?1,?2,?3,?4)",
        params![id, record_id, document, author],
    )
    .map_err(rerr)?;
    Ok(true)
}

/// A record identity is 64 lowercase hex characters, so it is safe to splice into a filter.
fn validate_sql_identity(id: &str) -> Result<(), StoreError> {
    if id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(StoreError::Storage)
    }
}

/// Whether a deferred record can be retried once `target` exists.
fn waits_on(domain: &DomainRecord, target: &str) -> bool {
    domain.target() == Some(target)
        || matches!(domain, DomainRecord::DocumentMove { collection, .. } if collection == target)
        || matches!(domain, DomainRecord::CommentAdd { parent: Some(parent), .. } if parent == target)
}
