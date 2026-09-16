#![forbid(unsafe_code)]

//! The Conversations Package domain and durable projection.  This crate deliberately
//! depends only on the language-neutral Package SDK; transport and key custody stay in Core.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value as JsonValue};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, EventSubscription, Package, event_channel,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

pub const GENERAL_ID: [u8; 16] = [0; 16];
const MAGIC: &[u8] = b"TCONV\x01";
const MAX_MARKDOWN: usize = 32 * 1024;
const PAGE_SIZE: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainRecord {
    ChannelCreate {
        id: [u8; 16],
        creator: String,
        slug: String,
    },
    ChannelHide {
        id: [u8; 16],
        channel_id: [u8; 16],
        creator: String,
    },
    MessagePost {
        id: [u8; 16],
        channel_id: [u8; 16],
        author: String,
        markdown: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    InvalidKind,
    InvalidLength,
    InvalidUtf8,
    InvalidIdentity,
    InvalidSlug,
    InvalidMarkdown,
    InvalidId,
}
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DomainError {}

impl DomainRecord {
    pub fn channel_create(
        creator: impl Into<String>,
        slug: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let creator = creator.into();
        let slug = slug.into();
        validate_identity(&creator)?;
        validate_slug(&slug)?;
        Ok(Self::ChannelCreate {
            id: random_id(),
            creator,
            slug,
        })
    }
    pub fn channel_create_with_id(
        id: [u8; 16],
        creator: impl Into<String>,
        slug: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let creator = creator.into();
        let slug = slug.into();
        validate_id(id)?;
        validate_identity(&creator)?;
        validate_slug(&slug)?;
        Ok(Self::ChannelCreate { id, creator, slug })
    }
    pub fn channel_hide(
        channel_id: [u8; 16],
        creator: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let creator = creator.into();
        validate_id(channel_id)?;
        validate_identity(&creator)?;
        Ok(Self::ChannelHide {
            id: random_id(),
            channel_id,
            creator,
        })
    }
    pub fn message_post(
        channel_id: [u8; 16],
        author: impl Into<String>,
        markdown: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let author = author.into();
        let markdown = markdown.into();
        validate_identity(&author)?;
        validate_markdown(&markdown)?;
        Ok(Self::MessagePost {
            id: random_id(),
            channel_id,
            author,
            markdown,
        })
    }
    pub fn id(&self) -> [u8; 16] {
        match self {
            Self::ChannelCreate { id, .. }
            | Self::ChannelHide { id, .. }
            | Self::MessagePost { id, .. } => *id,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ChannelCreate { .. } => "channel.create",
            Self::ChannelHide { .. } => "channel.hide",
            Self::MessagePost { .. } => "message.post",
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        match self {
            Self::ChannelCreate { id, creator, slug } => {
                out.push(1);
                out.extend(id);
                put_text(&mut out, creator);
                put_text(&mut out, slug);
            }
            Self::ChannelHide {
                id,
                channel_id,
                creator,
            } => {
                out.push(2);
                out.extend(id);
                out.extend(channel_id);
                put_text(&mut out, creator);
            }
            Self::MessagePost {
                id,
                channel_id,
                author,
                markdown,
            } => {
                out.push(3);
                out.extend(id);
                out.extend(channel_id);
                put_text(&mut out, author);
                put_text(&mut out, markdown);
            }
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, DomainError> {
        if bytes.len() < MAGIC.len() + 1 + 16 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(DomainError::InvalidKind);
        }
        let mut p = MAGIC.len();
        let kind = bytes[p];
        p += 1;
        let id = take_id(bytes, &mut p)?;
        match kind {
            1 => {
                let creator = take_text(bytes, &mut p)?;
                let slug = take_text(bytes, &mut p)?;
                if p != bytes.len() {
                    return Err(DomainError::InvalidLength);
                };
                Self::channel_create_with_id(id, creator, slug)
            }
            2 => {
                let channel_id = take_id(bytes, &mut p)?;
                let creator = take_text(bytes, &mut p)?;
                if p != bytes.len() {
                    return Err(DomainError::InvalidLength);
                };
                validate_identity(&creator)?;
                Ok(Self::ChannelHide {
                    id,
                    channel_id,
                    creator,
                })
            }
            3 => {
                let channel_id = take_id(bytes, &mut p)?;
                let author = take_text(bytes, &mut p)?;
                let markdown = take_text(bytes, &mut p)?;
                if p != bytes.len() {
                    return Err(DomainError::InvalidLength);
                };
                Self::message_post_with_id(id, channel_id, author, markdown)
            }
            _ => Err(DomainError::InvalidKind),
        }
    }
    fn message_post_with_id(
        id: [u8; 16],
        channel_id: [u8; 16],
        author: String,
        markdown: String,
    ) -> Result<Self, DomainError> {
        validate_id(id)?;
        validate_identity(&author)?;
        validate_markdown(&markdown)?;
        Ok(Self::MessagePost {
            id,
            channel_id,
            author,
            markdown,
        })
    }
}

fn random_id() -> [u8; 16] {
    let mut id = [0; 16];
    getrandom::fill(&mut id).expect("OS randomness");
    id
}
fn validate_id(id: [u8; 16]) -> Result<(), DomainError> {
    if id == GENERAL_ID {
        Err(DomainError::InvalidId)
    } else {
        Ok(())
    }
}
fn validate_identity(v: &str) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > 256 || !v.is_ascii() {
        Err(DomainError::InvalidIdentity)
    } else {
        Ok(())
    }
}
fn validate_slug(v: &str) -> Result<(), DomainError> {
    if !(1..=48).contains(&v.len())
        || !v.is_ascii()
        || v.bytes()
            .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
        || v.starts_with('-')
        || v.ends_with('-')
    {
        Err(DomainError::InvalidSlug)
    } else {
        Ok(())
    }
}
fn validate_markdown(v: &str) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > MAX_MARKDOWN || !v.is_char_boundary(v.len()) {
        Err(DomainError::InvalidMarkdown)
    } else {
        Ok(())
    }
}
fn put_text(out: &mut Vec<u8>, v: &str) {
    out.extend((u32::try_from(v.len()).unwrap()).to_be_bytes());
    out.extend(v.as_bytes());
}
fn take_id(b: &[u8], p: &mut usize) -> Result<[u8; 16], DomainError> {
    if b.len().saturating_sub(*p) < 16 {
        return Err(DomainError::InvalidLength);
    };
    let mut x = [0; 16];
    x.copy_from_slice(&b[*p..*p + 16]);
    *p += 16;
    Ok(x)
}
fn take_text(b: &[u8], p: &mut usize) -> Result<String, DomainError> {
    if b.len().saturating_sub(*p) < 4 {
        return Err(DomainError::InvalidLength);
    };
    let n = u32::from_be_bytes(b[*p..*p + 4].try_into().unwrap()) as usize;
    *p += 4;
    if n > b.len().saturating_sub(*p) {
        return Err(DomainError::InvalidLength);
    };
    let s = std::str::from_utf8(&b[*p..*p + n])
        .map_err(|_| DomainError::InvalidUtf8)?
        .to_owned();
    *p += n;
    Ok(s)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestResult {
    Applied,
    Duplicate,
    Deferred(String),
    Quarantined(String),
}
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
pub struct MessageView {
    pub id: String,
    pub channel_id: String,
    pub order_key: String,
    pub author: String,
    pub markdown: String,
    pub ingestion_ordinal: u64,
}

#[derive(Clone)]
pub struct ConversationStore {
    inner: Arc<Mutex<StoreInner>>,
}
struct StoreInner {
    conn: Connection,
    slugs: BTreeMap<String, String>,
    markdown: HashMap<String, String>,
    deferred: HashMap<String, DomainRecord>,
}

struct ProjectionOutcome {
    result: IngestResult,
    slug: Option<(String, String)>,
    markdown: Option<(String, String)>,
    deferred: Option<DomainRecord>,
    retry_channel: Option<String>,
    removed_messages: Vec<String>,
}

impl ProjectionOutcome {
    fn applied() -> Self {
        Self {
            result: IngestResult::Applied,
            slug: None,
            markdown: None,
            deferred: None,
            retry_channel: None,
            removed_messages: Vec::new(),
        }
    }
}

fn project_domain(
    tx: &rusqlite::Transaction<'_>,
    record_id: &str,
    order_key: &str,
    author: &str,
    ordinal: i64,
    domain: &DomainRecord,
) -> Result<ProjectionOutcome, rusqlite::Error> {
    if domain.kind()
        != tx.query_row(
            "SELECT purpose FROM records WHERE record_id=?1",
            [record_id],
            |row| row.get::<_, String>(0),
        )?
    {
        quarantine(tx, record_id, "purpose_mismatch")?;
        return Ok(ProjectionOutcome {
            result: IngestResult::Quarantined("purpose_mismatch".into()),
            ..ProjectionOutcome::applied()
        });
    }
    let signer_matches = match domain {
        DomainRecord::ChannelCreate { creator, .. } | DomainRecord::ChannelHide { creator, .. } => {
            creator == author
        }
        DomainRecord::MessagePost {
            author: domain_author,
            ..
        } => domain_author == author,
    };
    if !signer_matches {
        quarantine(tx, record_id, "signer_mismatch")?;
        return Ok(ProjectionOutcome {
            result: IngestResult::Quarantined("signer_mismatch".into()),
            ..ProjectionOutcome::applied()
        });
    }

    match domain {
        DomainRecord::ChannelCreate { id, creator, slug } => {
            let channel_id = hex_id(*id);
            tx.execute(
                "INSERT OR IGNORE INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,?2,?3,NULL)",
                params![channel_id, creator, order_key],
            )?;
            tx.execute(
                "UPDATE records SET quarantine_code=NULL WHERE record_id=?1",
                [record_id],
            )?;
            Ok(ProjectionOutcome {
                slug: Some((channel_id.clone(), slug.clone())),
                retry_channel: Some(channel_id),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::ChannelHide {
            channel_id,
            creator,
            ..
        } => {
            let channel_id = hex_id(*channel_id);
            let owner: Option<String> = tx
                .query_row(
                    "SELECT creator FROM channels WHERE channel_id=?1",
                    [&channel_id],
                    |row| row.get(0),
                )
                .optional()?;
            if channel_id == hex_id(GENERAL_ID) {
                quarantine(tx, record_id, "general_cannot_hide")?;
                return Ok(ProjectionOutcome {
                    result: IngestResult::Quarantined("general_cannot_hide".into()),
                    ..ProjectionOutcome::applied()
                });
            }
            let Some(owner) = owner else {
                quarantine(tx, record_id, "waiting_for_channel")?;
                return Ok(ProjectionOutcome {
                    result: IngestResult::Deferred("waiting_for_channel".into()),
                    deferred: Some(domain.clone()),
                    ..ProjectionOutcome::applied()
                });
            };
            if owner != *creator {
                quarantine(tx, record_id, "hide_not_creator")?;
                return Ok(ProjectionOutcome {
                    result: IngestResult::Quarantined("hide_not_creator".into()),
                    ..ProjectionOutcome::applied()
                });
            }
            tx.execute(
                "UPDATE channels SET hidden_order=CASE WHEN hidden_order IS NULL OR hidden_order>?2 THEN ?2 ELSE hidden_order END WHERE channel_id=?1",
                params![channel_id, order_key],
            )?;
            tx.execute(
                "UPDATE records SET quarantine_code=NULL WHERE record_id=?1",
                [record_id],
            )?;
            let hidden_order: String = tx.query_row(
                "SELECT hidden_order FROM channels WHERE channel_id=?1",
                [&channel_id],
                |row| row.get(0),
            )?;
            let removed_messages = {
                let mut statement = tx.prepare(
                    "SELECT message_id,record_id FROM messages WHERE channel_id=?1 AND order_key>?2",
                )?;
                let rows = statement.query_map(params![channel_id, hidden_order], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for (_, message_record_id) in &removed_messages {
                quarantine(tx, message_record_id, "channel_hidden")?;
            }
            tx.execute(
                "DELETE FROM messages WHERE channel_id=?1 AND order_key>?2",
                params![channel_id, hidden_order],
            )?;
            Ok(ProjectionOutcome {
                removed_messages: removed_messages
                    .into_iter()
                    .map(|(message_id, _)| message_id)
                    .collect(),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::MessagePost {
            id,
            channel_id,
            author: domain_author,
            markdown,
        } => {
            let channel_id = hex_id(*channel_id);
            let hidden_order: Option<Option<String>> = tx
                .query_row(
                    "SELECT hidden_order FROM channels WHERE channel_id=?1",
                    [&channel_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(hidden_order) = hidden_order else {
                quarantine(tx, record_id, "waiting_for_channel")?;
                return Ok(ProjectionOutcome {
                    result: IngestResult::Deferred("waiting_for_channel".into()),
                    deferred: Some(domain.clone()),
                    ..ProjectionOutcome::applied()
                });
            };
            if hidden_order
                .as_deref()
                .is_some_and(|hidden_order| order_key > hidden_order)
            {
                quarantine(tx, record_id, "channel_hidden")?;
                return Ok(ProjectionOutcome {
                    result: IngestResult::Quarantined("channel_hidden".into()),
                    ..ProjectionOutcome::applied()
                });
            }
            let message_id = hex_id(*id);
            tx.execute(
                "INSERT OR IGNORE INTO messages(message_id,channel_id,order_key,author,record_id,ingestion_ordinal) VALUES(?1,?2,?3,?4,?5,?6)",
                params![message_id, channel_id, order_key, domain_author, record_id, ordinal],
            )?;
            tx.execute(
                "UPDATE records SET quarantine_code=NULL WHERE record_id=?1",
                [record_id],
            )?;
            Ok(ProjectionOutcome {
                markdown: Some((message_id, markdown.clone())),
                ..ProjectionOutcome::applied()
            })
        }
    }
}

impl ConversationStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let path = path.as_ref().to_owned();
        let c = Connection::open(&path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS records(record_id TEXT PRIMARY KEY, order_key TEXT NOT NULL, author TEXT NOT NULL, purpose TEXT NOT NULL, signature TEXT NOT NULL, sealed_record BLOB NOT NULL, cursor TEXT, ingestion_ordinal INTEGER NOT NULL, quarantine_code TEXT); CREATE INDEX IF NOT EXISTS records_purpose ON records(purpose); CREATE TABLE IF NOT EXISTS channels(channel_id TEXT PRIMARY KEY, creator TEXT NOT NULL, create_order TEXT NOT NULL, hidden_order TEXT); CREATE TABLE IF NOT EXISTS messages(message_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, order_key TEXT NOT NULL, author TEXT NOT NULL, record_id TEXT NOT NULL, ingestion_ordinal INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS messages_channel_order ON messages(channel_id, order_key DESC); CREATE TABLE IF NOT EXISTS unread(channel_id TEXT PRIMARY KEY, last_read_ingestion_ordinal INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1), cursor TEXT, next_ingestion_ordinal INTEGER NOT NULL); INSERT OR IGNORE INTO metadata(singleton,cursor,next_ingestion_ordinal) VALUES(1,NULL,0);")?;
        let s = Self {
            inner: Arc::new(Mutex::new(StoreInner {
                conn: c,
                slugs: BTreeMap::new(),
                markdown: HashMap::new(),
                deferred: HashMap::new(),
            })),
        };
        s.ensure_general()?;
        Ok(s)
    }
    fn ensure_general(&self) -> Result<(), rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        x.conn.execute("INSERT OR IGNORE INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,'system','',NULL)",[hex_id(GENERAL_ID)])?;
        Ok(())
    }
    pub fn cursor(&self) -> Result<Option<String>, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        x.conn
            .query_row("SELECT cursor FROM metadata WHERE singleton=1", [], |r| {
                r.get(0)
            })
    }
    pub fn record_count(&self) -> Result<u64, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        x.conn.query_row("SELECT COUNT(*) FROM records", [], |r| {
            r.get::<_, i64>(0).map(|v| v as u64)
        })
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
    ) -> Result<IngestResult, rusqlite::Error> {
        let parsed = DomainRecord::decode(canonical);
        let mut inner = self.inner.lock().unwrap();
        let transaction = inner.conn.transaction()?;
        let existing: Option<(String, String, String, String, Vec<u8>)> = transaction
            .query_row(
                "SELECT order_key,author,purpose,signature,sealed_record FROM records WHERE record_id=?1",
                [record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
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
                        .execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])?;
                }
                transaction.commit()?;
                return Ok(IngestResult::Duplicate);
            }
            quarantine(&transaction, record_id, "immutable_field_mismatch")?;
            transaction.commit()?;
            return Ok(IngestResult::Quarantined("immutable_field_mismatch".into()));
        }
        let ordinal: i64 = transaction.query_row(
            "SELECT next_ingestion_ordinal FROM metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let ordinal = ordinal + 1;
        transaction.execute(
            "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)",
            params![record_id, order_key, author, purpose, signature, sealed, cursor, ordinal],
        )?;
        transaction.execute(
            "UPDATE metadata SET cursor=?1,next_ingestion_ordinal=?2 WHERE singleton=1",
            params![cursor, ordinal],
        )?;
        let outcome = match parsed {
            Ok(domain) => {
                project_domain(&transaction, record_id, order_key, author, ordinal, &domain)?
            }
            Err(_) => {
                quarantine(&transaction, record_id, "malformed_record")?;
                ProjectionOutcome {
                    result: IngestResult::Quarantined("malformed_record".into()),
                    ..ProjectionOutcome::applied()
                }
            }
        };
        transaction.commit()?;
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

    fn retry_cached_for_channel(&self, channel_id: &str) -> Result<(), rusqlite::Error> {
        let mut inner = self.inner.lock().unwrap();
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
            let (order_key, author, ordinal): (String, String, i64) = inner.conn.query_row(
                "SELECT order_key,author,ingestion_ordinal FROM records WHERE record_id=?1",
                [&record_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let transaction = inner.conn.transaction()?;
            let outcome = project_domain(
                &transaction,
                &record_id,
                &order_key,
                &author,
                ordinal,
                &domain,
            )?;
            transaction.commit()?;
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

    pub fn deferred_records(&self) -> Result<Vec<StoredRecord>, rusqlite::Error> {
        let inner = self.inner.lock().unwrap();
        let mut statement = inner.conn.prepare(
            "SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal FROM records WHERE quarantine_code='waiting_for_channel' ORDER BY order_key,record_id",
        )?;
        let rows = statement.query_map([], |row| {
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
        })?;
        rows.collect()
    }

    pub fn retry_deferred(
        &self,
        record_id: &str,
        canonical: &[u8],
    ) -> Result<IngestResult, rusqlite::Error> {
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
            let mut inner = self.inner.lock().unwrap();
            inner.deferred.insert(record_id.to_owned(), domain);
        }
        self.retry_cached_for_channel(&channel_id)?;
        let inner = self.inner.lock().unwrap();
        let code: Option<String> = inner.conn.query_row(
            "SELECT quarantine_code FROM records WHERE record_id=?1",
            [record_id],
            |row| row.get(0),
        )?;
        Ok(match code.as_deref() {
            None => IngestResult::Applied,
            Some("waiting_for_channel") => IngestResult::Deferred("waiting_for_channel".into()),
            Some(code) => IngestResult::Quarantined(code.to_owned()),
        })
    }

    pub fn channel_is_active(&self, channel_id: &str) -> Result<bool, rusqlite::Error> {
        let inner = self.inner.lock().unwrap();
        inner
            .conn
            .query_row(
                "SELECT hidden_order IS NULL FROM channels WHERE channel_id=?1",
                [channel_id],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
    }

    pub fn channels(&self) -> Result<Vec<ChannelView>, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        let mut q = x.conn.prepare(
            "SELECT channel_id,creator,hidden_order FROM channels ORDER BY create_order,channel_id",
        )?;
        let rows = q.query_map([], |r| {
            let id: String = r.get(0)?;
            let creator: String = r.get(1)?;
            let hidden: Option<String> = r.get(2)?;
            Ok((id, creator, hidden.is_some()))
        })?;
        let mut out = Vec::new();
        let mut counts = HashMap::new();
        {
            let mut st=x.conn.prepare("SELECT channel_id,COUNT(*) FROM messages m WHERE ingestion_ordinal>COALESCE((SELECT last_read_ingestion_ordinal FROM unread u WHERE u.channel_id=m.channel_id),0) GROUP BY channel_id")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (id, n) = r?;
                counts.insert(id, n as u64);
            }
        }
        let channels = rows.collect::<Result<Vec<_>, _>>()?;
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
                let mut prefix = 8;
                loop {
                    let candidate = format!("{base}-{}", &id[..prefix]);
                    if !reserved.contains(&candidate) && !assigned.contains(&candidate) {
                        break candidate;
                    }
                    if prefix < id.len() {
                        prefix = (prefix + 8).min(id.len());
                    } else {
                        break format!("{base}-{id}-{id}");
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
    pub fn message_rows(
        &self,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MessageRow>, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        let sql = if before.is_some() {
            "SELECT m.message_id,m.order_key,m.author,m.record_id,r.sealed_record,m.ingestion_ordinal FROM messages m JOIN records r ON r.record_id=m.record_id WHERE m.channel_id=?1 AND m.order_key<?2 ORDER BY m.order_key DESC LIMIT ?3"
        } else {
            "SELECT m.message_id,m.order_key,m.author,m.record_id,r.sealed_record,m.ingestion_ordinal FROM messages m JOIN records r ON r.record_id=m.record_id WHERE m.channel_id=?1 ORDER BY m.order_key DESC LIMIT ?2"
        };
        let mut s = x.conn.prepare(sql)?;
        let mut out = Vec::new();
        if let Some(b) = before {
            for r in s.query_map(params![channel, b, limit as i64], row_message)? {
                out.push(r?)
            }
        } else {
            for r in s.query_map(params![channel, limit as i64], row_message)? {
                out.push(r?)
            }
        }
        Ok(out)
    }
    pub fn mark_read(&self, channel: &str) -> Result<(), rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        let n: i64 = x.conn.query_row(
            "SELECT COALESCE(MAX(ingestion_ordinal),0) FROM messages WHERE channel_id=?1",
            [channel],
            |r| r.get(0),
        )?;
        x.conn.execute("INSERT INTO unread(channel_id,last_read_ingestion_ordinal) VALUES(?1,?2) ON CONFLICT(channel_id) DO UPDATE SET last_read_ingestion_ordinal=excluded.last_read_ingestion_ordinal",params![channel,n])?;
        Ok(())
    }
    pub fn read_ordinal(&self, channel: &str) -> Result<u64, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        x.conn.query_row("SELECT COALESCE((SELECT last_read_ingestion_ordinal FROM unread WHERE channel_id=?1),0)",[channel],|r|r.get::<_,i64>(0).map(|v|v as u64))
    }
    pub fn control_records(&self) -> Result<Vec<StoredRecord>, rusqlite::Error> {
        let x = self.inner.lock().unwrap();
        let mut statement = x.conn.prepare("SELECT record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal FROM records WHERE purpose IN ('channel.create','channel.hide') AND quarantine_code IS NULL ORDER BY order_key")?;
        let rows = statement.query_map([], |r| {
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
        })?;
        rows.collect()
    }

    /// Rehydrates non-secret display data after restart. The bytes are never stored.
    pub fn remember_opened(&self, canonical: &[u8]) -> Result<(), DomainError> {
        let domain = DomainRecord::decode(canonical)?;
        let mut x = self.inner.lock().unwrap();
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
    ) -> Result<IngestResult, rusqlite::Error> {
        self.ingest(
            record_id, order_key, author, purpose, signature, canonical, sealed, None,
        )
    }
}
fn row_message(r: &rusqlite::Row<'_>) -> Result<MessageRow, rusqlite::Error> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get::<_, i64>(5)? as u64,
    ))
}
fn quarantine(tx: &rusqlite::Transaction<'_>, id: &str, code: &str) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE records SET quarantine_code=?2 WHERE record_id=?1",
        params![id, code],
    )
    .map(|_| ())
}
fn hex_id(id: [u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode_id(v: &str) -> Option<[u8; 16]> {
    if v.len() != 32 {
        return None;
    }
    let mut out = [0; 16];
    for (i, p) in v.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()?;
    }
    Some(out)
}

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut o = Map::new();
    for (k, v) in entries {
        o.insert(k.into(), v);
    }
    JsonValue::Object(o)
}
fn str_field(v: &JsonValue, k: &str) -> Result<String, BackendError> {
    v.get(k)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| error("conversations.invalid_request", false))
}
fn error(code: &str, retryable: bool) -> BackendError {
    BackendError::new(code, retryable, object([]))
}

fn capability_error(operation: &str, cause: BackendError) -> BackendError {
    eprintln!(
        "conversations capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    BackendError::new(
        "conversations.capability_unavailable",
        cause.retryable(),
        object([("causeCode", JsonValue::String(cause.code().to_owned()))]),
    )
}
fn grant(init: &teamarness_package_sdk::Initialization, id: &str) -> Result<String, BackendError> {
    init.capabilities()
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v.get("identity").and_then(JsonValue::as_str) == Some(id))
                .and_then(|v| v.get("grant"))
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| error("conversations.missing_capability", false))
}
fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
fn from_b64(v: &str) -> Result<Vec<u8>, BackendError> {
    URL_SAFE_NO_PAD
        .decode(v)
        .map_err(|_| error("conversations.invalid_record", false))
}

#[derive(Clone)]
pub struct ConversationsBackend {
    active: Arc<AsyncMutex<Option<Running>>>,
    lifecycle: Arc<AsyncMutex<()>>,
    changes: watch::Sender<u64>,
}
#[derive(Clone)]
struct Active {
    store: ConversationStore,
    record_grant: String,
}
struct Running {
    active: Active,
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Default for ConversationsBackend {
    fn default() -> Self {
        let (tx, _) = watch::channel(0);
        Self {
            active: Arc::new(AsyncMutex::new(None)),
            lifecycle: Arc::new(AsyncMutex::new(())),
            changes: tx,
        }
    }
}
impl ConversationsBackend {
    pub async fn serve() -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(Self::default())).serve().await
    }

    async fn active(&self) -> Result<Active, BackendError> {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|running| running.active.clone())
            .ok_or_else(|| error("conversations.not_active", true))
    }

    async fn stop_active(&self) {
        if let Some(running) = self.active.lock().await.take() {
            let _ = running.cancel.send(true);
            let _ = running.task.await;
        }
    }

    async fn deactivate(&self) -> Result<JsonValue, BackendError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_active().await;
        Ok(object([]))
    }

    async fn activate(&self, r: &BackendRequest) -> Result<JsonValue, BackendError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_active().await;
        let data = r
            .initialization
            .data_directory()
            .get("path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| error("conversations.invalid_initialization", false))?;
        let store = ConversationStore::open(Path::new(data).join("conversations.sqlite3"))
            .map_err(|_| error("conversations.storage_failed", true))?;
        let sync = grant(
            &r.initialization,
            "teamarness.synchronization.immutable-record-log",
        )?;
        let active = Active {
            store: store.clone(),
            record_grant: sync.clone(),
        };
        let cursor = store
            .cursor()
            .map_err(|_| error("conversations.storage_failed", true))?;
        let mut sub = r
            .capabilities
            .subscribe(
                &sync,
                object([
                    ("method", JsonValue::String("subscribe".into())),
                    (
                        "afterCursor",
                        cursor.map_or(JsonValue::Null, JsonValue::String),
                    ),
                ]),
            )
            .await
            .map_err(|cause| capability_error("subscribe", cause))?;
        // Slugs are domain plaintext and are intentionally rebuilt through bounded Core opens.
        // Message bodies are opened only when a page is requested.
        for batch in store
            .control_records()
            .map_err(|_| error("conversations.storage_failed", true))?
            .chunks(32)
        {
            let sealed = batch
                .iter()
                .map(|row| JsonValue::String(b64(&row.sealed_record)))
                .collect();
            let opened = r
                .capabilities
                .call(
                    &sync,
                    "open",
                    object([("sealedRecords", JsonValue::Array(sealed))]),
                )
                .await
                .map_err(|cause| capability_error("open-control-records", cause))?;
            if let Some(results) = opened.get("results").and_then(JsonValue::as_array) {
                for result in results {
                    if let Some(canonical) = result.get("canonical").and_then(JsonValue::as_str) {
                        let bytes = from_b64(canonical)?;
                        let _ = store.remember_opened(&bytes);
                    }
                }
            }
        }
        // Deferred records remain sealed at rest. Recovery opens at most one capability batch
        // at a time and retries projection only in memory after Channel controls are available.
        for batch in store
            .deferred_records()
            .map_err(|_| error("conversations.storage_failed", true))?
            .chunks(32)
        {
            let sealed = batch
                .iter()
                .map(|row| JsonValue::String(b64(&row.sealed_record)))
                .collect();
            let opened = r
                .capabilities
                .call(
                    &sync,
                    "open",
                    object([("sealedRecords", JsonValue::Array(sealed))]),
                )
                .await
                .map_err(|cause| capability_error("open-deferred-records", cause))?;
            if let Some(results) = opened.get("results").and_then(JsonValue::as_array) {
                for (row, result) in batch.iter().zip(results) {
                    if let Some(canonical) = result.get("canonical").and_then(JsonValue::as_str) {
                        let canonical = from_b64(canonical)?;
                        store
                            .retry_deferred(&row.record_id, &canonical)
                            .map_err(|_| error("conversations.storage_failed", true))?;
                    }
                }
            }
        }
        let backend = self.clone();
        let state = active.clone();
        let (cancel, mut cancelled) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = cancelled.changed() => {
                        if result.is_err() || *cancelled.borrow() {
                            break;
                        }
                    }
                    event = sub.recv() => match event {
                        Some(Ok(event)) => match ingest_event(&state.store, &event) {
                            Ok(_) => backend.changes.send_modify(|revision| *revision = revision.saturating_add(1)),
                            Err(error) => eprintln!(
                                "conversations record ingestion failure: code={} retryable={}",
                                error.code(),
                                error.retryable()
                            ),
                        },
                        Some(Err(error)) => {
                            eprintln!(
                                "conversations record subscription failure: code={} retryable={}",
                                error.code(),
                                error.retryable()
                            );
                            break;
                        }
                        None => {
                            eprintln!("conversations record subscription closed");
                            break;
                        }
                    }
                }
            }
            sub.close().await;
        });
        *self.active.lock().await = Some(Running {
            active,
            cancel,
            task,
        });
        Ok(object([]))
    }
    async fn snapshot(&self) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let channels = a
            .store
            .channels()
            .map_err(|_| error("conversations.storage_failed", true))?;
        Ok(object([
            (
                "channels",
                JsonValue::Array(channels.into_iter().map(channel_json).collect()),
            ),
            ("availability", JsonValue::String("current".into())),
        ]))
    }
    async fn append(
        &self,
        r: &BackendRequest,
        purpose: &str,
        domain: DomainRecord,
    ) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        let canonical = domain.encode();
        let response = r
            .capabilities
            .call(
                &a.record_grant,
                "append",
                object([
                    ("purpose", JsonValue::String(purpose.into())),
                    ("canonical", JsonValue::String(b64(&canonical))),
                ]),
            )
            .await
            .map_err(|cause| capability_error(purpose, cause))?;
        let record_id = str_field(&response, "recordId")?;
        let order = str_field(&response, "orderKey")?;
        let author = str_field(&response, "author")?;
        let signature = str_field(&response, "signature")?;
        let sealed = from_b64(&str_field(&response, "sealedRecord")?)?;
        a.store
            .ingest(
                &record_id, &order, &author, purpose, &signature, &canonical, &sealed, None,
            )
            .map_err(|_| error("conversations.storage_failed", true))?;
        self.changes.send_modify(|x| *x = x.saturating_add(1));
        Ok(response)
    }
    async fn page(
        &self,
        r: &BackendRequest,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<JsonValue, BackendError> {
        let a = self.active().await?;
        if decode_id(channel).is_none() {
            return Err(error("conversations.invalid_request", false));
        }
        let rows = a
            .store
            .message_rows(channel, before, limit.min(PAGE_SIZE))
            .map_err(|_| error("conversations.storage_failed", true))?;
        if rows.is_empty() {
            return Ok(object([
                ("messages", JsonValue::Array(Vec::new())),
                ("hasMore", JsonValue::Bool(false)),
            ]));
        }
        let sealed = rows.iter().map(|x| JsonValue::String(b64(&x.4))).collect();
        let opened = r
            .capabilities
            .call(
                &a.record_grant,
                "open",
                object([("sealedRecords", JsonValue::Array(sealed))]),
            )
            .await
            .map_err(|cause| capability_error("open-message-page", cause))?;
        let results = opened
            .get("results")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| error("conversations.invalid_record", false))?;
        let local_identity = r
            .initialization
            .installation()
            .get("identity")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let mut messages = Vec::new();
        for ((id, order, author, _rid, _sealed, ordinal), value) in rows.into_iter().zip(results) {
            let canon = value
                .get("canonical")
                .and_then(JsonValue::as_str)
                .and_then(|s| from_b64(s).ok())
                .and_then(|b| DomainRecord::decode(&b).ok());
            if let Some(DomainRecord::MessagePost { markdown, .. }) = canon {
                messages.push(object([
                    ("id", JsonValue::String(id)),
                    ("orderKey", JsonValue::String(order)),
                    ("author", JsonValue::String(author.clone())),
                    ("isLocal", JsonValue::Bool(author == local_identity)),
                    ("markdown", JsonValue::String(markdown)),
                    ("ingestionOrdinal", JsonValue::String(ordinal.to_string())),
                ]));
            }
        }
        Ok(object([
            ("messages", JsonValue::Array(messages)),
            (
                "hasMore",
                JsonValue::Bool(results.len() == limit.min(PAGE_SIZE)),
            ),
        ]))
    }
}
impl Backend for ConversationsBackend {
    fn handle(
        &self,
        r: BackendRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<JsonValue, BackendError>> + Send + '_>>
    {
        Box::pin(async move {
            match r.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" => self.activate(&r).await,
                "lifecycle.suspend" | "lifecycle.dispose" => self.deactivate().await,
                "conversations.snapshot" => self.snapshot().await,
                "conversations.channel.create" | "channel.create" => {
                    let slug = str_field(&r.payload, "slug")?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::channel_create(author, slug)
                        .map_err(|_| error("conversations.invalid_slug", false))?;
                    self.append(&r, "channel.create", d).await
                }
                "conversations.channel.hide" | "channel.hide" => {
                    if r.payload.get("confirm").and_then(JsonValue::as_bool) != Some(true) {
                        return Err(error("conversations.confirmation_required", false));
                    }
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::channel_hide(cid, author)
                        .map_err(|_| error("conversations.invalid_request", false))?;
                    self.append(&r, "channel.hide", d).await
                }
                "conversations.message.post" | "message.post" => {
                    let cid = decode_id(&str_field(&r.payload, "channelId")?)
                        .ok_or_else(|| error("conversations.invalid_request", false))?;
                    let active = self.active().await?;
                    if !active
                        .store
                        .channel_is_active(&hex_id(cid))
                        .map_err(|_| error("conversations.storage_failed", true))?
                    {
                        return Err(error("conversations.channel_hidden", false));
                    }
                    let md = str_field(&r.payload, "markdown")?;
                    let author = r
                        .initialization
                        .installation()
                        .get("identity")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown");
                    let d = DomainRecord::message_post(cid, author, md)
                        .map_err(|_| error("conversations.invalid_markdown", false))?;
                    self.append(&r, "message.post", d).await
                }
                "conversations.channel.open"
                | "conversations.channel.page"
                | "channel.open"
                | "channel.page" => {
                    let cid = str_field(&r.payload, "channelId")?;
                    let before = r.payload.get("beforeOrderKey").and_then(JsonValue::as_str);
                    let limit = r
                        .payload
                        .get("limit")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(PAGE_SIZE as u64) as usize;
                    let out = self.page(&r, &cid, before, limit).await?;
                    if r.operation.ends_with("open") {
                        self.active()
                            .await?
                            .store
                            .mark_read(&cid)
                            .map_err(|_| error("conversations.storage_failed", true))?;
                    }
                    Ok(out)
                }
                "conversations.channel.mark-read"
                | "conversations.channel.mark_read"
                | "channel.mark-read" => {
                    let a = self.active().await?;
                    a.store
                        .mark_read(&str_field(&r.payload, "channelId")?)
                        .map_err(|_| error("conversations.storage_failed", true))?;
                    Ok(object([]))
                }
                _ => Err(error("conversations.unknown_operation", false)),
            }
        })
    }
    fn subscribe(
        &self,
        r: BackendRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<EventSubscription, BackendError>> + Send + '_>,
    > {
        Box::pin(async move {
            if r.operation != "conversations.changes" && r.operation != "changes" {
                return Err(error("conversations.unknown_subscription", false));
            }
            let (tx, sub) = event_channel(32);
            let me = self.clone();
            let mut rev = self.changes.subscribe();
            tokio::spawn(async move {
                if let Ok(v) = me.snapshot().await
                    && tx.send(v).await.is_err()
                {
                    return;
                }
                while rev.changed().await.is_ok() {
                    if let Ok(v) = me.snapshot().await
                        && tx.send(v).await.is_err()
                    {
                        return;
                    }
                }
            });
            Ok(sub)
        })
    }
}
fn ingest_event(store: &ConversationStore, v: &JsonValue) -> Result<IngestResult, BackendError> {
    let id = str_field(v, "recordId")?;
    let order = str_field(v, "orderKey")?;
    let author = str_field(v, "author")?;
    let purpose = str_field(v, "purpose")?;
    let sig = str_field(v, "signature")?;
    let canonical = from_b64(&str_field(v, "canonical")?)?;
    let sealed = from_b64(&str_field(v, "sealedRecord")?)?;
    let cursor = v.get("cursor").and_then(JsonValue::as_str);
    store
        .ingest(
            &id, &order, &author, &purpose, &sig, &canonical, &sealed, cursor,
        )
        .map_err(|_| error("conversations.storage_failed", true))
}
fn channel_json(c: ChannelView) -> JsonValue {
    object([
        ("id", JsonValue::String(c.id)),
        ("slug", JsonValue::String(c.slug)),
        ("creator", JsonValue::String(c.creator)),
        ("hidden", JsonValue::Bool(c.hidden)),
        ("unreadCount", JsonValue::String(c.unread_count.to_string())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_roundtrip_and_bounds() {
        let x = DomainRecord::message_post([1; 16], "author", "**hello**").unwrap();
        assert_eq!(DomainRecord::decode(&x.encode()).unwrap(), x);
        assert!(DomainRecord::channel_create_with_id([2; 16], "a", "Bad Slug").is_err());
        assert!(DomainRecord::channel_create_with_id([2; 16], "a", "bad_slug").is_err());
        assert!(DomainRecord::message_post([1; 16], "a", "x".repeat(MAX_MARKDOWN + 1)).is_err());
    }
    #[test]
    fn general_channel_accepts_messages_but_cannot_be_hidden() {
        let message = DomainRecord::message_post(GENERAL_ID, "author", "hello").unwrap();
        assert_eq!(DomainRecord::decode(&message.encode()).unwrap(), message);
        assert!(DomainRecord::channel_hide(GENERAL_ID, "author").is_err());
    }

    #[test]
    fn general_and_duplicate_slug_order_are_stable() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let a = DomainRecord::channel_create_with_id([1; 16], "author", "team").unwrap();
        let b = DomainRecord::channel_create_with_id([2; 16], "author", "team").unwrap();
        let c = DomainRecord::channel_create_with_id([3; 16], "author", "team-02020202").unwrap();
        for (d, o) in [(&a, "01"), (&b, "02"), (&c, "03")] {
            let c = d.encode();
            assert_eq!(
                s.ingest(
                    &hex_id(d.id()),
                    o,
                    "author",
                    d.kind(),
                    "sig",
                    &c,
                    b"sealed",
                    None
                )
                .unwrap(),
                IngestResult::Applied
            )
        }
        let channels = s.channels().unwrap();
        assert!(channels.iter().any(|channel| channel.slug == "team"));
        assert!(
            channels
                .iter()
                .any(|channel| channel.slug == "team-0202020202020202")
        );
        assert!(
            channels
                .iter()
                .any(|channel| channel.slug == "team-02020202")
        );

        let reversed = ConversationStore::open(d.path().join("reversed.sqlite")).unwrap();
        for (record, order) in [(&c, "03"), (&b, "02"), (&a, "01")] {
            reversed
                .ingest(
                    &hex_id(record.id()),
                    order,
                    "author",
                    record.kind(),
                    "sig",
                    &record.encode(),
                    b"sealed",
                    None,
                )
                .unwrap();
        }
        let presentation = |store: &ConversationStore| {
            store
                .channels()
                .unwrap()
                .into_iter()
                .filter(|channel| channel.slug.starts_with("team"))
                .map(|channel| (channel.id, channel.slug))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(presentation(&s), presentation(&reversed));
    }
    #[test]
    fn persistence_contains_sealed_bytes_and_opaque_indexes_only() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.sqlite");
        let s = ConversationStore::open(&path).unwrap();
        let c = DomainRecord::channel_create_with_id([3; 16], "alice", "private").unwrap();
        let canonical = c.encode();
        s.ingest(
            "aabb",
            "0001",
            "alice",
            c.kind(),
            "signature",
            &canonical,
            b"ciphertext",
            Some("cursor"),
        )
        .unwrap();
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes
                .windows(b"ciphertext".len())
                .any(|w| w == b"ciphertext")
        );
        assert!(!bytes.windows(b"private".len()).any(|w| w == b"private"));
        assert!(!bytes.windows(b"TCONV".len()).any(|w| w == b"TCONV"));
        let reopened = ConversationStore::open(&path).unwrap();
        assert_eq!(reopened.cursor().unwrap().as_deref(), Some("cursor"));
        assert_eq!(
            reopened
                .channels()
                .unwrap()
                .iter()
                .find(|x| x.id == hex_id([3; 16]))
                .unwrap()
                .slug,
            "channel-03030303"
        );
    }

    #[test]
    fn message_before_channel_is_deferred_then_projected() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let message = DomainRecord::message_post([8; 16], "alice", "hello").unwrap();
        assert_eq!(
            store
                .ingest(
                    "message-record",
                    "01",
                    "alice",
                    message.kind(),
                    "s",
                    &message.encode(),
                    b"sealed-message",
                    None,
                )
                .unwrap(),
            IngestResult::Deferred("waiting_for_channel".into())
        );
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );

        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel-record",
                "02",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"sealed-channel",
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deferred_message_recovery_uses_reopened_sealed_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("x.sqlite");
        let message = DomainRecord::message_post([8; 16], "alice", "restart secret").unwrap();
        {
            let store = ConversationStore::open(&path).unwrap();
            store
                .ingest(
                    "message-record",
                    "01",
                    "alice",
                    message.kind(),
                    "s",
                    &message.encode(),
                    b"sealed-message",
                    None,
                )
                .unwrap();
        }
        let store = ConversationStore::open(&path).unwrap();
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel-record",
                "02",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"sealed-channel",
                None,
            )
            .unwrap();
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );
        let deferred = store.deferred_records().unwrap();
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].sealed_record, b"sealed-message");
        assert_eq!(
            store
                .retry_deferred("message-record", &message.encode())
                .unwrap(),
            IngestResult::Applied
        );
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        let bytes = std::fs::read(path).unwrap();
        assert!(
            !bytes
                .windows(b"restart secret".len())
                .any(|window| window == b"restart secret")
        );
    }

    #[test]
    fn earliest_hide_removes_later_messages_but_keeps_earlier_history() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel",
                "01",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"c",
                None,
            )
            .unwrap();
        let later = DomainRecord::message_post([8; 16], "bob", "later").unwrap();
        store
            .ingest(
                "later",
                "04",
                "bob",
                later.kind(),
                "s",
                &later.encode(),
                b"l",
                None,
            )
            .unwrap();
        let hide = DomainRecord::channel_hide([8; 16], "alice").unwrap();
        store
            .ingest(
                "hide",
                "03",
                "alice",
                hide.kind(),
                "s",
                &hide.encode(),
                b"h",
                None,
            )
            .unwrap();
        assert!(!store.channel_is_active(&hex_id([8; 16])).unwrap());
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );

        let earlier = DomainRecord::message_post([8; 16], "bob", "earlier").unwrap();
        assert_eq!(
            store
                .ingest(
                    "earlier",
                    "02",
                    "bob",
                    earlier.kind(),
                    "s",
                    &earlier.encode(),
                    b"e",
                    None
                )
                .unwrap(),
            IngestResult::Applied
        );
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
        let after = DomainRecord::message_post([8; 16], "bob", "after").unwrap();
        assert_eq!(
            store
                .ingest(
                    "after",
                    "05",
                    "bob",
                    after.kind(),
                    "s",
                    &after.encode(),
                    b"a",
                    None
                )
                .unwrap(),
            IngestResult::Quarantined("channel_hidden".into())
        );
    }

    #[test]
    fn deferred_hide_and_messages_replay_in_order_after_channel_arrives() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let earlier = DomainRecord::message_post([8; 16], "bob", "earlier").unwrap();
        let hide = DomainRecord::channel_hide([8; 16], "alice").unwrap();
        let later = DomainRecord::message_post([8; 16], "bob", "later").unwrap();
        for (record_id, order, author, record) in [
            ("later", "04", "bob", &later),
            ("hide", "03", "alice", &hide),
            ("earlier", "02", "bob", &earlier),
        ] {
            assert!(matches!(
                store
                    .ingest(
                        record_id,
                        order,
                        author,
                        record.kind(),
                        "s",
                        &record.encode(),
                        record_id.as_bytes(),
                        None,
                    )
                    .unwrap(),
                IngestResult::Deferred(_)
            ));
        }
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel",
                "05",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"channel",
                None,
            )
            .unwrap();

        let rows = store
            .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, hex_id(earlier.id()));
        assert!(!store.channel_is_active(&hex_id([8; 16])).unwrap());
    }

    #[test]
    fn scale_fixture_keeps_channel_and_page_queries_bounded() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("scale.sqlite")).unwrap();
        let mut inner = s.inner.lock().unwrap();
        let tx = inner.conn.transaction().unwrap();
        for channel in 1..=100_u32 {
            let channel_id = format!("{channel:032x}");
            tx.execute(
                "INSERT INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,'author',?2,NULL)",
                params![channel_id, format!("{channel:082x}")],
            ).unwrap();
        }
        for message in 0..100_000_u32 {
            let channel = (message % 100) + 1;
            let channel_id = format!("{channel:032x}");
            let message_id = format!("{message:032x}");
            let record_id = format!("{message:064x}");
            let order = format!("{message:082x}");
            tx.execute(
                "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,'author','message.post','signature',X'7365616c6564',NULL,?3,NULL)",
                params![record_id, order, i64::from(message) + 1],
            ).unwrap();
            tx.execute(
                "INSERT INTO messages(message_id,channel_id,order_key,author,record_id,ingestion_ordinal) VALUES(?1,?2,?3,'author',?4,?5)",
                params![message_id, channel_id, order, record_id, i64::from(message) + 1],
            ).unwrap();
        }
        tx.commit().unwrap();
        drop(inner);
        assert_eq!(s.channels().unwrap().len(), 101);
        for channel in 1..=100_u32 {
            let rows = s
                .message_rows(&format!("{channel:032x}"), None, PAGE_SIZE)
                .unwrap();
            assert_eq!(rows.len(), PAGE_SIZE);
        }
    }

    #[test]
    fn unread_defaults_to_all_messages_before_first_read() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let message = DomainRecord::message_post(GENERAL_ID, "alice", "one").unwrap();
        s.ingest(
            &hex_id(message.id()),
            "01",
            "alice",
            message.kind(),
            "s",
            &message.encode(),
            b"x",
            None,
        )
        .unwrap();

        assert_eq!(
            s.channels()
                .unwrap()
                .iter()
                .find(|channel| channel.id == hex_id(GENERAL_ID))
                .unwrap()
                .unread_count,
            1
        );
    }

    #[tokio::test]
    async fn lifecycle_deactivation_cancels_and_awaits_the_owned_task() {
        let directory = tempfile::tempdir().unwrap();
        let backend = ConversationsBackend::default();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let (cancel, mut cancelled) = watch::channel(false);
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_exited = Arc::clone(&exited);
        let task = tokio::spawn(async move {
            let _ = cancelled.changed().await;
            task_exited.store(true, std::sync::atomic::Ordering::Release);
        });
        *backend.active.lock().await = Some(Running {
            active: Active {
                store,
                record_grant: "grant".into(),
            },
            cancel,
            task,
        });

        backend.deactivate().await.unwrap();

        assert!(backend.active.lock().await.is_none());
        assert!(exited.load(std::sync::atomic::Ordering::Acquire));
        backend.deactivate().await.unwrap();
    }

    #[test]
    fn unread_uses_ingestion_not_sort_order() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let c = DomainRecord::channel_create_with_id([1; 16], "a", "team").unwrap();
        let cb = c.encode();
        s.ingest(&hex_id(c.id()), "10", "a", c.kind(), "s", &cb, b"x", None)
            .unwrap();
        let m = DomainRecord::message_post([1; 16], "a", "one").unwrap();
        let mb = m.encode();
        s.ingest(&hex_id(m.id()), "01", "a", m.kind(), "s", &mb, b"y", None)
            .unwrap();
        s.mark_read(&hex_id([1; 16])).unwrap();
        let m2 = DomainRecord::message_post([1; 16], "b", "two").unwrap();
        let mb = m2.encode();
        s.ingest(&hex_id(m2.id()), "00", "b", m2.kind(), "s", &mb, b"z", None)
            .unwrap();
        assert_eq!(
            s.channels()
                .unwrap()
                .iter()
                .find(|x| x.id == hex_id([1; 16]))
                .unwrap()
                .unread_count,
            1
        );
    }
}
