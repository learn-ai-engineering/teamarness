pub(crate) mod migration;
pub(crate) mod projection;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{DomainRecord, GENERAL_ID, hex_id, quarantine, row_message, slug_with_suffix};

#[derive(Debug)]
pub enum StoreError {
    Storage,
    Migration,
    Lock,
    UnsupportedSchema,
    LegacyValidation(String),
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
#[derive(Debug, Clone)]
pub struct ChannelView {
    pub id: String,
    pub slug: String,
    pub creator: String,
    pub hidden: bool,
    pub unread_count: u64,
}
pub type MessageRow = (String, String, String, String, Vec<u8>, u64);

#[derive(Debug, Clone)]
pub struct HistoryChange {
    pub revision: u64,
    pub channel_id: String,
    pub message_id: String,
    pub order_key: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct HistoryBatch {
    pub changes: Vec<HistoryChange>,
    pub high_water: u64,
    pub continuation: u64,
}

#[derive(Debug, Clone)]
pub struct MessageView {
    pub id: String,
    pub channel_id: String,
    pub order_key: String,
    pub author: String,
    pub markdown: String,
    pub ingestion_ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestResult {
    Applied,
    Duplicate,
    Deferred(String),
    Quarantined(String),
}

#[derive(Debug, Clone)]
pub struct ConversationStore {
    pub(crate) inner: Arc<Mutex<StoreInner>>,
    /// A one-shot ingest failure, armed only by a test.
    fail_next_ingest: Arc<AtomicBool>,
}
#[derive(Debug)]
pub(crate) struct StoreInner {
    pub(crate) conn: Connection,
    pub(crate) slugs: BTreeMap<String, String>,
    pub(crate) markdown: HashMap<String, String>,
    pub(crate) deferred: HashMap<String, DomainRecord>,
}

fn lock(inner: &Mutex<StoreInner>) -> Result<std::sync::MutexGuard<'_, StoreInner>, StoreError> {
    inner.lock().map_err(|_| StoreError::Lock)
}

fn rerr(e: rusqlite::Error) -> StoreError {
    let _ = e;
    StoreError::Storage
}

impl ConversationStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_owned();
        let mut c = Connection::open(&path).map_err(|_| StoreError::Storage)?;
        migration::apply(&mut c)?;
        let s = Self {
            inner: Arc::new(Mutex::new(StoreInner {
                conn: c,
                slugs: BTreeMap::new(),
                markdown: HashMap::new(),
                deferred: HashMap::new(),
            })),
            fail_next_ingest: Arc::new(AtomicBool::new(false)),
        };
        s.ensure_general()?;
        Ok(s)
    }
    fn ensure_general(&self) -> Result<(), StoreError> {
        let x = lock(&self.inner)?;
        x.conn
            .execute(
                "INSERT OR IGNORE INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,'system','',NULL)",
                [hex_id(GENERAL_ID)],
            )
            .map_err(rerr)?;
        Ok(())
    }
    pub fn cursor(&self) -> Result<Option<String>, StoreError> {
        let x = lock(&self.inner)?;
        x.conn
            .query_row("SELECT cursor FROM metadata WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .map_err(rerr)
    }
    /// Clears the committed delivery cursor.
    ///
    /// The Host refuses a cursor that does not belong to this journal; the
    /// Package then replays from the beginning, and ingestion deduplicates by
    /// record identity.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the metadata row cannot be written.
    pub fn reset_cursor(&self) -> Result<(), StoreError> {
        let x = lock(&self.inner)?;
        x.conn
            .execute("UPDATE metadata SET cursor = NULL WHERE singleton = 1", [])
            .map(|_| ())
            .map_err(rerr)
    }

    pub fn record_count(&self) -> Result<u64, StoreError> {
        let x = lock(&self.inner)?;
        x.conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })
            .map_err(rerr)
    }
    /// Fails the next `ingest` once, and only once.
    ///
    /// The redelivery decision belongs to the SDK's record log session and can only be observed
    /// through a real one, so the failure is injected here rather than simulated.
    pub fn fail_next_ingest_for_test(&self) {
        self.fail_next_ingest.store(true, Ordering::SeqCst);
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
        if self.fail_next_ingest.swap(false, Ordering::SeqCst) {
            return Err(StoreError::Storage);
        }
        let parsed = DomainRecord::decode(canonical);
        let mut inner = lock(&self.inner)?;
        let transaction = inner.conn.transaction().map_err(rerr)?;
        let existing: Option<(String, String, String, String, Vec<u8>)> = transaction
            .query_row(
                "SELECT order_key,author,purpose,signature,sealed_record FROM records WHERE record_id=?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional().map_err(rerr)?;
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
                if let Some(cursor) = cursor {
                    transaction
                        .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                        .map_err(rerr)?;
                }
                transaction.commit().map_err(rerr)?;
                return Ok(IngestResult::Duplicate);
            }
            quarantine(&transaction, record_id, "immutable_field_mismatch").map_err(rerr)?;
            transaction.commit().map_err(rerr)?;
            return Ok(IngestResult::Quarantined("immutable_field_mismatch".into()));
        }
        let ordinal: i64 = transaction
            .query_row(
                "SELECT next_ingestion_ordinal FROM metadata WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(rerr)?;
        let ordinal = ordinal + 1;
        transaction.execute(
            "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)",
            params![record_id, order_key, author, purpose, signature, sealed, cursor, ordinal],
        ).map_err(rerr)?;
        transaction
            .execute(
                "UPDATE metadata SET cursor=?1,next_ingestion_ordinal=?2 WHERE singleton=1",
                params![cursor, ordinal],
            )
            .map_err(rerr)?;
        let outcome = match parsed {
            Ok(domain) => projection::project_domain(
                &transaction,
                record_id,
                order_key,
                author,
                ordinal,
                &domain,
            )
            .map_err(rerr)?,
            Err(_) => {
                quarantine(&transaction, record_id, "malformed_record").map_err(rerr)?;
                projection::ProjectionOutcome {
                    result: IngestResult::Quarantined("malformed_record".into()),
                    ..projection::ProjectionOutcome::applied()
                }
            }
        };
        transaction.commit().map_err(rerr)?;
        if let Some((id, slug)) = outcome.slug {
            inner.slugs.insert(id, slug);
        }
        if let Some((id, markdown)) = outcome.markdown {
            inner.markdown.insert(id, markdown);
        }
        for id in outcome.removed_messages {
            inner.markdown.remove(&id);
        }
        if let Some(domain) = outcome.deferred {
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        let retry_channel = outcome.retry_channel;
        let result = outcome.result;
        drop(inner);
        if let Some(channel_id) = retry_channel {
            self.retry_cached_for_channel(&channel_id)?;
        }
        Ok(result)
    }

    fn retry_cached_for_channel(&self, channel_id: &str) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner)?;
        let mut deferred = inner
            .deferred
            .iter()
            .filter_map(|(record_id, domain)| {
                let target = match domain {
                    DomainRecord::ChannelHide { channel_id, .. }
                    | DomainRecord::MessagePost { channel_id, .. } => hex_id(*channel_id),
                    DomainRecord::ChannelCreate { .. } => return None,
                };
                (target == channel_id).then(|| (record_id.clone(), domain.clone()))
            })
            .collect::<Vec<_>>();
        deferred.sort_by(|(left, _), (right, _)| {
            let order = |record_id: &str| {
                inner
                    .conn
                    .query_row(
                        "SELECT order_key FROM records WHERE record_id=?1",
                        [record_id],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap_or_default()
            };
            order(left).cmp(&order(right)).then_with(|| left.cmp(right))
        });
        for (record_id, domain) in deferred {
            let (order_key, author, ordinal): (String, String, i64) = inner
                .conn
                .query_row(
                    "SELECT order_key,author,ingestion_ordinal FROM records WHERE record_id=?1",
                    [&record_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(rerr)?;
            let transaction = inner.conn.transaction().map_err(rerr)?;
            let outcome = projection::project_domain(
                &transaction,
                &record_id,
                &order_key,
                &author,
                ordinal,
                &domain,
            )
            .map_err(rerr)?;
            transaction.commit().map_err(rerr)?;
            if let Some((id, markdown)) = outcome.markdown {
                inner.markdown.insert(id, markdown);
            }
            for id in outcome.removed_messages {
                inner.markdown.remove(&id);
            }
            if !matches!(outcome.result, IngestResult::Deferred(_)) {
                inner.deferred.remove(&record_id);
            }
        }
        Ok(())
    }

    pub fn deferred_records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        let inner = lock(&self.inner)?;
        let mut statement = inner.conn.prepare(
            "SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal FROM records WHERE quarantine_code='waiting_for_channel' ORDER BY order_key,record_id",
        ).map_err(rerr)?;
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

    pub fn retry_deferred(
        &self,
        record_id: &str,
        canonical: &[u8],
    ) -> Result<IngestResult, StoreError> {
        let domain = match DomainRecord::decode(canonical) {
            Ok(domain) => domain,
            Err(_) => return Ok(IngestResult::Quarantined("malformed_record".into())),
        };
        let channel_id = match &domain {
            DomainRecord::ChannelHide { channel_id, .. }
            | DomainRecord::MessagePost { channel_id, .. } => hex_id(*channel_id),
            DomainRecord::ChannelCreate { .. } => return Ok(IngestResult::Duplicate),
        };
        {
            let mut inner = lock(&self.inner)?;
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        self.retry_cached_for_channel(&channel_id)?;
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
            Some("waiting_for_channel") => IngestResult::Deferred("waiting_for_channel".into()),
            Some(code) => IngestResult::Quarantined(code.to_owned()),
        })
    }

    pub fn channel_is_active(&self, channel_id: &str) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT hidden_order IS NULL FROM channels WHERE channel_id=?1",
                [channel_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(rerr)
            .map(|value| value.unwrap_or(false))
    }

    pub(crate) fn has_projection_warnings(&self) -> Result<bool, StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE quarantine_code IS NOT NULL AND quarantine_code!='waiting_for_channel')",
                [],
                |row| row.get(0),
            )
            .map_err(rerr)
    }

    pub fn channels(&self) -> Result<Vec<ChannelView>, StoreError> {
        let x = lock(&self.inner)?;
        let mut q = x
            .conn
            .prepare(
                "SELECT channel_id,creator,hidden_order FROM channels ORDER BY create_order,channel_id",
            )
            .map_err(rerr)?;
        let rows = q
            .query_map([], |r| {
                let id: String = r.get(0)?;
                let creator: String = r.get(1)?;
                let hidden: Option<String> = r.get(2)?;
                Ok((id, creator, hidden.is_some()))
            })
            .map_err(rerr)?;
        let mut out = Vec::new();
        let mut counts = HashMap::new();
        {
            let mut st = x
                .conn
                .prepare("SELECT channel_id,COUNT(*) FROM messages m WHERE ingestion_ordinal>COALESCE((SELECT last_read_ingestion_ordinal FROM unread u WHERE u.channel_id=m.channel_id),0) GROUP BY channel_id")
                .map_err(rerr)?;
            for r in st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map_err(rerr)?
            {
                let (id, n) = r.map_err(rerr)?;
                counts.insert(id, n as u64);
            }
        }
        let channels = rows.collect::<Result<Vec<_>, _>>().map_err(rerr)?;
        let base_slug = |id: &str| {
            if id == hex_id(GENERAL_ID) {
                "general".into()
            } else {
                x.slugs
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("channel-{}", &id[..8]))
            }
        };
        let mut base_counts = HashMap::<String, usize>::new();
        for (id, _, _) in &channels {
            *base_counts.entry(base_slug(id)).or_default() += 1;
        }
        let reserved = base_counts.keys().cloned().collect::<HashSet<_>>();
        let mut used = HashMap::<String, usize>::new();
        let mut assigned = HashSet::new();
        for (id, creator, hidden) in channels {
            let base = base_slug(&id);
            let occurrence = used.entry(base.clone()).or_default();
            *occurrence += 1;
            let slug = if base_counts[&base] == 1 || *occurrence == 1 {
                base
            } else {
                let mut prefix = 8.min(id.len());
                'candidate: loop {
                    let candidate = slug_with_suffix(&base, &id[..prefix]);
                    if !reserved.contains(&candidate) && !assigned.contains(&candidate) {
                        break candidate;
                    }
                    if prefix < id.len() {
                        prefix = (prefix + 8).min(id.len());
                        continue;
                    }
                    let mut discriminator = 2_u64;
                    loop {
                        let suffix = format!("{id}-{discriminator}");
                        let candidate = slug_with_suffix(&base, &suffix);
                        if !reserved.contains(&candidate) && !assigned.contains(&candidate) {
                            break 'candidate candidate;
                        }
                        discriminator += 1;
                    }
                }
            };
            assigned.insert(slug.clone());
            out.push(ChannelView {
                id: id.clone(),
                slug,
                creator,
                hidden,
                unread_count: counts.get(&id).copied().unwrap_or(0),
            });
        }
        Ok(out)
    }
    pub fn history_head(&self) -> Result<(String, u64), StoreError> {
        let inner = lock(&self.inner)?;
        inner
            .conn
            .query_row(
                "SELECT epoch,revision FROM history_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .map_err(rerr)
    }

    /// The epoch and the ordered, bounded changes are sampled under the same store lock.
    /// `None` means the caller must reopen and rebuild its loaded window.
    pub fn history_changes(
        &self,
        channel: &str,
        epoch: &str,
        after: u64,
        limit: usize,
    ) -> Result<Option<HistoryBatch>, StoreError> {
        let inner = lock(&self.inner)?;
        let (current_epoch, head): (String, i64) = inner
            .conn
            .query_row(
                "SELECT epoch,revision FROM history_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(rerr)?;
        let Ok(after) = i64::try_from(after) else {
            return Ok(None);
        };
        if current_epoch != epoch || after > head {
            return Ok(None);
        }
        let limit = limit.clamp(1, 32);
        let mut statement = inner
            .conn
            .prepare(
                "SELECT revision,channel_id,message_id,order_key,kind FROM history_changes \
             WHERE channel_id=?1 AND revision>?2 AND revision<=?3 ORDER BY revision LIMIT ?4",
            )
            .map_err(rerr)?;
        let mut changes = statement
            .query_map(params![channel, after, head, (limit + 1) as i64], |row| {
                Ok(HistoryChange {
                    revision: row.get::<_, i64>(0)? as u64,
                    channel_id: row.get(1)?,
                    message_id: row.get(2)?,
                    order_key: row.get(3)?,
                    kind: row.get(4)?,
                })
            })
            .map_err(rerr)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(rerr)?;
        let more = changes.len() > limit;
        changes.truncate(limit);
        let continuation = if more {
            changes
                .last()
                .map_or(after as u64, |change| change.revision)
        } else {
            head as u64
        };
        Ok(Some(HistoryBatch {
            changes,
            high_water: head as u64,
            continuation,
        }))
    }

    pub fn message_rows(
        &self,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MessageRow>, StoreError> {
        self.message_rows_with_head(channel, before, limit)
            .map(|(_, rows)| rows)
    }

    /// Capture raw rows and projection watermark atomically before asynchronous `open`.
    pub fn message_rows_with_head(
        &self,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<((String, u64), Vec<MessageRow>), StoreError> {
        let x = lock(&self.inner)?;
        let head: (String, i64) = x
            .conn
            .query_row(
                "SELECT epoch,revision FROM history_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(rerr)?;
        let sql = if before.is_some() {
            "SELECT m.message_id,m.order_key,m.author,m.record_id,r.sealed_record,m.ingestion_ordinal FROM messages m JOIN records r ON r.record_id=m.record_id WHERE m.channel_id=?1 AND m.order_key<?2 ORDER BY m.order_key DESC LIMIT ?3"
        } else {
            "SELECT m.message_id,m.order_key,m.author,m.record_id,r.sealed_record,m.ingestion_ordinal FROM messages m JOIN records r ON r.record_id=m.record_id WHERE m.channel_id=?1 ORDER BY m.order_key DESC LIMIT ?2"
        };
        let mut s = x.conn.prepare(sql).map_err(rerr)?;
        let mut out = Vec::new();
        if let Some(b) = before {
            for r in s
                .query_map(params![channel, b, limit as i64], row_message)
                .map_err(rerr)?
            {
                out.push(r.map_err(rerr)?)
            }
        } else {
            for r in s
                .query_map(params![channel, limit as i64], row_message)
                .map_err(rerr)?
            {
                out.push(r.map_err(rerr)?)
            }
        }
        Ok(((head.0, head.1 as u64), out))
    }

    pub fn message_row_by_id(
        &self,
        channel: &str,
        id: &str,
    ) -> Result<Option<MessageRow>, StoreError> {
        let x = lock(&self.inner)?;
        x.conn.query_row(
            "SELECT m.message_id,m.order_key,m.author,m.record_id,r.sealed_record,m.ingestion_ordinal FROM messages m JOIN records r ON r.record_id=m.record_id WHERE m.channel_id=?1 AND m.message_id=?2",
            params![channel, id], row_message,
        ).optional().map_err(rerr)
    }

    pub fn mark_read(&self, channel: &str) -> Result<(), StoreError> {
        let x = lock(&self.inner)?;
        let n: i64 = x
            .conn
            .query_row(
                "SELECT COALESCE(MAX(ingestion_ordinal),0) FROM messages WHERE channel_id=?1",
                [channel],
                |r| r.get(0),
            )
            .map_err(rerr)?;
        x.conn.execute("INSERT INTO unread(channel_id,last_read_ingestion_ordinal) VALUES(?1,?2) ON CONFLICT(channel_id) DO UPDATE SET last_read_ingestion_ordinal=excluded.last_read_ingestion_ordinal",params![channel,n]).map_err(rerr)?;
        Ok(())
    }
    pub fn read_ordinal(&self, channel: &str) -> Result<u64, StoreError> {
        let x = lock(&self.inner)?;
        x.conn
            .query_row(
                "SELECT COALESCE((SELECT last_read_ingestion_ordinal FROM unread WHERE channel_id=?1),0)",
                [channel],
                |r| r.get::<_, i64>(0).map(|v| v as u64),
            )
            .map_err(rerr)
    }
    pub fn control_records(&self) -> Result<Vec<StoredRecord>, StoreError> {
        let x = lock(&self.inner)?;
        let mut statement = x.conn.prepare("SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal FROM records WHERE purpose IN ('channel.create','channel.hide') AND quarantine_code IS NULL ORDER BY order_key").map_err(rerr)?;
        let rows = statement
            .query_map([], |r| {
                Ok(StoredRecord {
                    record_id: r.get(0)?,
                    order_key: r.get(1)?,
                    author: r.get(2)?,
                    purpose: r.get(3)?,
                    signature: r.get(4)?,
                    sealed_record: r.get(5)?,
                    cursor: r.get(6)?,
                    ingestion_ordinal: r.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(rerr)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(rerr)
    }

    /// Rehydrates non-secret display data after restart. The bytes are never stored.
    pub fn remember_opened(&self, canonical: &[u8]) -> Result<(), StoreError> {
        let domain = DomainRecord::decode(canonical).map_err(|_| StoreError::Storage)?;
        let mut x = lock(&self.inner)?;
        match domain {
            DomainRecord::ChannelCreate { id, slug, .. } => {
                x.slugs.insert(hex_id(id), slug);
            }
            DomainRecord::MessagePost { id, markdown, .. } => {
                x.markdown.insert(hex_id(id), markdown);
            }
            DomainRecord::ChannelHide { .. } => {}
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_control(
        &self,
        record_id: &str,
        order_key: &str,
        author: &str,
        purpose: &str,
        signature: &str,
        canonical: &[u8],
        sealed: &[u8],
    ) -> Result<IngestResult, StoreError> {
        self.ingest(
            record_id, order_key, author, purpose, signature, canonical, sealed, None,
        )
    }
}
