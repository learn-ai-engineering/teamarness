pub(crate) mod migration;
pub(crate) mod projection;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{DomainRecord, quarantine};
use projection::WAITING;

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
pub enum IngestResult {
    Applied,
    Duplicate,
    Deferred(String),
    Quarantined(String),
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
    ) -> Result<IngestResult, StoreError> {
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
                if let Some(target) = created {
                    self.retry_for_target(&target, on_commit)?;
                    self.commit_delivery_cursor(cursor)?;
                }
                return Ok(IngestResult::Duplicate);
            }
            quarantine(&transaction, record_id, "immutable_field_mismatch").map_err(rerr)?;
            transaction.commit().map_err(rerr)?;
            on_commit();
            return Ok(IngestResult::Quarantined("immutable_field_mismatch".into()));
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
        let outcome = match parsed {
            Ok(domain) => {
                projection::project_domain(&transaction, record_id, order_key, author, &domain)
                    .map_err(rerr)?
            }
            Err(_) => {
                quarantine(&transaction, record_id, "malformed_record").map_err(rerr)?;
                projection::ProjectionOutcome {
                    result: IngestResult::Quarantined("malformed_record".into()),
                    ..projection::ProjectionOutcome::applied()
                }
            }
        };
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
        if let Some(domain) = outcome.deferred {
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        let created = outcome.created;
        let result = outcome.result;
        drop(inner);
        if let Some(target) = created {
            self.retry_for_target(&target, on_commit)?;
            self.commit_delivery_cursor(cursor)?;
        }
        Ok(result)
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

    /// Re-projects every deferred record that waits on `target`, in order.
    fn retry_for_target(&self, target: &str, on_commit: &dyn Fn()) -> Result<(), StoreError> {
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
            let outcome =
                projection::project_domain(&transaction, &record_id, &order_key, &author, &domain)
                    .map_err(rerr)?;
            transaction.commit().map_err(rerr)?;
            on_commit();
            if let Some((id, order, text)) = outcome.text {
                remember_text(&mut inner.texts, id, order, text);
            }
            if !matches!(outcome.result, IngestResult::Deferred(_)) {
                inner.deferred.remove(&record_id);
            }
            if let Some(created) = outcome.created {
                // A deferred Document create can itself unblock its dependants.
                drop(inner);
                self.retry_for_target(&created, on_commit)?;
                inner = lock(&self.inner)?;
            }
        }
        Ok(())
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
                "SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal FROM records WHERE {filter} ORDER BY order_key,record_id"
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
    ) -> Result<IngestResult, StoreError> {
        let _ingest = self.ingest_gate.lock().map_err(|_| StoreError::Lock)?;
        let Ok(domain) = DomainRecord::decode(canonical) else {
            return Ok(IngestResult::Quarantined("malformed_record".into()));
        };
        let Some(target) = domain.target().map(str::to_owned) else {
            return Ok(IngestResult::Duplicate);
        };
        {
            let mut inner = lock(&self.inner)?;
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        self.retry_for_target(&target, &|| {})?;
        let inner = lock(&self.inner)?;
        let code: Option<String> = inner
            .conn
            .query_row(
                "SELECT quarantine_code FROM records WHERE record_id=?1",
                [record_id],
                |row| row.get(0),
            )
            .map_err(rerr)?;
        Ok(match code.as_deref() {
            None => IngestResult::Applied,
            Some(WAITING) => IngestResult::Deferred(WAITING.into()),
            Some(code) => IngestResult::Quarantined(code.to_owned()),
        })
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

/// Whether a deferred record can be retried once `target` exists.
fn waits_on(domain: &DomainRecord, target: &str) -> bool {
    domain.target() == Some(target)
        || matches!(domain, DomainRecord::DocumentMove { collection, .. } if collection == target)
}
