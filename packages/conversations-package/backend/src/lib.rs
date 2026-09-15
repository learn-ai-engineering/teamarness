#![forbid(unsafe_code)]

//! The Conversations Package domain and durable projection.  This crate deliberately
//! depends only on the language-neutral Package SDK; transport and key custody stay in Core.

use std::collections::{BTreeMap, HashMap};
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
const PAGE_SIZE: usize = 50;

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
        validate_id(channel_id)?;
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
        validate_id(channel_id)?;
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
            .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
        || v.starts_with('-')
        || v.starts_with('_')
        || v.ends_with('-')
        || v.ends_with('_')
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
        let mut x = self.inner.lock().unwrap();
        let tx = x.conn.transaction()?;
        let old:Option<(String,String,String,String,Vec<u8>)>=tx.query_row("SELECT order_key,author,purpose,signature,sealed_record FROM records WHERE record_id=?1",[record_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if let Some((o, a, p, s, se)) = old {
            if o == order_key && a == author && p == purpose && s == signature && se == sealed {
                if let Some(c) = cursor {
                    tx.execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [c])?;
                }
                tx.commit()?;
                return Ok(IngestResult::Duplicate);
            }
            tx.execute(
                "UPDATE records SET quarantine_code='immutable_field_mismatch' WHERE record_id=?1",
                [record_id],
            )?;
            tx.commit()?;
            return Ok(IngestResult::Quarantined("immutable_field_mismatch".into()));
        }
        let ordinal: i64 = tx.query_row(
            "SELECT next_ingestion_ordinal FROM metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.execute("INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)",params![record_id,order_key,author,purpose,signature,sealed,cursor,ordinal+1])?;
        tx.execute(
            "UPDATE metadata SET cursor=?1,next_ingestion_ordinal=?2 WHERE singleton=1",
            params![cursor, ordinal + 1],
        )?;
        let mut result = IngestResult::Applied;
        let mut slug_cache = None;
        let mut markdown_cache = None;
        if let Ok(domain) = parsed {
            if domain.kind() != purpose {
                tx.execute(
                    "UPDATE records SET quarantine_code='purpose_mismatch' WHERE record_id=?1",
                    [record_id],
                )?;
                result = IngestResult::Quarantined("purpose_mismatch".into());
            } else {
                let signer_matches = match &domain {
                    DomainRecord::ChannelCreate { creator, .. }
                    | DomainRecord::ChannelHide { creator, .. } => creator == author,
                    DomainRecord::MessagePost {
                        author: domain_author,
                        ..
                    } => domain_author == author,
                };
                if !signer_matches {
                    quarantine(&tx, record_id, "signer_mismatch")?;
                    result = IngestResult::Quarantined("signer_mismatch".into());
                } else {
                    match &domain {
                        DomainRecord::ChannelCreate { id, creator, slug } => {
                            let cid = hex_id(*id);
                            let exists: Option<String> = tx
                                .query_row(
                                    "SELECT channel_id FROM channels WHERE channel_id=?1",
                                    [&cid],
                                    |r| r.get(0),
                                )
                                .optional()?;
                            if exists.is_none() {
                                tx.execute("INSERT INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,?2,?3,NULL)",params![cid,creator,order_key])?;
                            }
                            slug_cache = Some((cid, slug.clone()));
                        }
                        DomainRecord::ChannelHide {
                            id: _,
                            channel_id,
                            creator,
                        } => {
                            let cid = hex_id(*channel_id);
                            let owner: Option<String> = tx
                                .query_row(
                                    "SELECT creator FROM channels WHERE channel_id=?1",
                                    [&cid],
                                    |r| r.get(0),
                                )
                                .optional()?;
                            if cid == hex_id(GENERAL_ID) {
                                quarantine(&tx, record_id, "general_cannot_hide")?;
                                result = IngestResult::Quarantined("general_cannot_hide".into());
                            } else if owner.as_deref() != Some(creator) {
                                quarantine(&tx, record_id, "hide_not_creator")?;
                                result = IngestResult::Quarantined("hide_not_creator".into());
                            } else {
                                tx.execute("UPDATE channels SET hidden_order=CASE WHEN hidden_order IS NULL OR hidden_order>?2 THEN ?2 ELSE hidden_order END WHERE channel_id=?1",params![cid,order_key])?;
                            }
                        }
                        DomainRecord::MessagePost {
                            id,
                            channel_id,
                            author: domain_author,
                            markdown,
                        } => {
                            let cid = hex_id(*channel_id);
                            let exists: Option<String> = tx
                                .query_row(
                                    "SELECT channel_id FROM channels WHERE channel_id=?1",
                                    [&cid],
                                    |r| r.get(0),
                                )
                                .optional()?;
                            if exists.is_none() {
                                quarantine(&tx, record_id, "unknown_channel")?;
                                result = IngestResult::Quarantined("unknown_channel".into());
                            } else {
                                tx.execute("INSERT OR IGNORE INTO messages(message_id,channel_id,order_key,author,record_id,ingestion_ordinal) VALUES(?1,?2,?3,?4,?5,?6)",params![hex_id(*id),cid,order_key,domain_author,record_id,ordinal+1])?;
                                markdown_cache = Some((hex_id(*id), markdown.clone()));
                            }
                        }
                    }
                }
            }
        } else {
            quarantine(&tx, record_id, "malformed_record")?;
            result = IngestResult::Quarantined("malformed_record".into());
        }
        tx.commit()?;
        if let Some((id, slug)) = slug_cache {
            x.slugs.insert(id, slug);
        }
        if let Some((id, markdown)) = markdown_cache {
            x.markdown.insert(id, markdown);
        }
        Ok(result)
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
            let mut st=x.conn.prepare("SELECT channel_id,COUNT(*) FROM messages m WHERE ingestion_ordinal>(SELECT COALESCE(last_read_ingestion_ordinal,0) FROM unread u WHERE u.channel_id=m.channel_id) GROUP BY channel_id")?;
            for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (id, n) = r?;
                counts.insert(id, n as u64);
            }
        }
        let mut used = HashMap::<String, u32>::new();
        for r in rows {
            let (id, creator, hidden) = r?;
            let base = if id == hex_id(GENERAL_ID) {
                "general".into()
            } else {
                x.slugs
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("channel-{}", &id[..8]))
            };
            let count = used.entry(base.clone()).or_insert(0);
            *count += 1;
            let slug = if *count == 1 {
                base
            } else {
                format!("{base}-{}", *count)
            };
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
    active: Arc<AsyncMutex<Option<Active>>>,
    changes: watch::Sender<u64>,
}
#[derive(Clone)]
struct Active {
    store: ConversationStore,
    record_grant: String,
}
impl Default for ConversationsBackend {
    fn default() -> Self {
        let (tx, _) = watch::channel(0);
        Self {
            active: Arc::new(AsyncMutex::new(None)),
            changes: tx,
        }
    }
}
impl ConversationsBackend {
    pub async fn serve() -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(Self::default())).serve().await
    }
    async fn activate(&self, r: &BackendRequest) -> Result<JsonValue, BackendError> {
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
                object([(
                    "afterCursor",
                    cursor.map_or(JsonValue::Null, JsonValue::String),
                )]),
            )
            .await
            .map_err(|_| error("conversations.capability_unavailable", true))?;
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
                .map_err(|_| error("conversations.capability_unavailable", true))?;
            if let Some(results) = opened.get("results").and_then(JsonValue::as_array) {
                for result in results {
                    if let Some(canonical) = result.get("canonical").and_then(JsonValue::as_str) {
                        let bytes = from_b64(canonical)?;
                        let _ = store.remember_opened(&bytes);
                    }
                }
            }
        }
        let backend = self.clone();
        let state = active.clone();
        tokio::spawn(async move {
            while let Some(Ok(event)) = sub.recv().await {
                if ingest_event(&state.store, &event).is_ok() {
                    backend.changes.send_modify(|x| *x = x.saturating_add(1));
                }
            }
        });
        *self.active.lock().await = Some(active);
        Ok(object([]))
    }
    async fn snapshot(&self) -> Result<JsonValue, BackendError> {
        let a = self
            .active
            .lock()
            .await
            .clone()
            .ok_or_else(|| error("conversations.not_active", true))?;
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
        let a = self
            .active
            .lock()
            .await
            .clone()
            .ok_or_else(|| error("conversations.not_active", true))?;
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
            .map_err(|_| error("conversations.capability_unavailable", true))?;
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
        let a = self
            .active
            .lock()
            .await
            .clone()
            .ok_or_else(|| error("conversations.not_active", true))?;
        if decode_id(channel).is_none() {
            return Err(error("conversations.invalid_request", false));
        }
        let rows = a
            .store
            .message_rows(channel, before, limit.min(PAGE_SIZE))
            .map_err(|_| error("conversations.storage_failed", true))?;
        let sealed = rows.iter().map(|x| JsonValue::String(b64(&x.4))).collect();
        let opened = r
            .capabilities
            .call(
                &a.record_grant,
                "open",
                object([("sealedRecords", JsonValue::Array(sealed))]),
            )
            .await
            .map_err(|_| error("conversations.capability_unavailable", true))?;
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
                "lifecycle.suspend" => {
                    *self.active.lock().await = None;
                    Ok(object([]))
                }
                "lifecycle.dispose" => {
                    *self.active.lock().await = None;
                    Ok(object([]))
                }
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
                    if r.operation.ends_with("open")
                        && let Some(a) = self.active.lock().await.clone()
                    {
                        a.store
                            .mark_read(&cid)
                            .map_err(|_| error("conversations.storage_failed", true))?;
                    }
                    Ok(out)
                }
                "conversations.channel.mark-read"
                | "conversations.channel.mark_read"
                | "channel.mark-read" => {
                    let a = self
                        .active
                        .lock()
                        .await
                        .clone()
                        .ok_or_else(|| error("conversations.not_active", true))?;
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
        assert!(DomainRecord::message_post([1; 16], "a", "x".repeat(MAX_MARKDOWN + 1)).is_err());
    }
    #[test]
    fn general_and_duplicate_slug_order_are_stable() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let a = DomainRecord::channel_create_with_id([1; 16], "author", "team").unwrap();
        let b = DomainRecord::channel_create_with_id([2; 16], "author", "team").unwrap();
        for (d, o) in [(&a, "01"), (&b, "02")] {
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
        assert!(channels.iter().any(|c| c.slug == "team"));
        assert!(channels.iter().any(|c| c.slug == "team-2"));
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
    fn malformed_and_unknown_channel_records_are_quarantined_without_blocking_delivery() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let unknown = DomainRecord::message_post([9; 16], "alice", "hello").unwrap();
        assert_eq!(
            s.ingest(
                "one",
                "01",
                "alice",
                unknown.kind(),
                "s",
                &unknown.encode(),
                b"x",
                None
            )
            .unwrap(),
            IngestResult::Quarantined("unknown_channel".into())
        );
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        assert_eq!(
            s.ingest(
                "two",
                "02",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"y",
                None
            )
            .unwrap(),
            IngestResult::Applied
        );
        assert_eq!(
            s.channels()
                .unwrap()
                .iter()
                .filter(|x| x.slug == "known")
                .count(),
            1
        );
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
