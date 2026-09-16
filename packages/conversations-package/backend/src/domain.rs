//! Conversations domain: records, validation, canonical codec, and display helpers.
//! Deliberately language-neutral; transport, key custody, and encryption stay in Core.

use std::fmt;

use crate::store;

pub const GENERAL_ID: [u8; 16] = [0; 16];
const MAGIC: &[u8] = b"TCONV\x01";
pub(crate) const MAX_MARKDOWN: usize = 32 * 1024;
pub(crate) const MAX_SLUG_LEN: usize = 48;
pub const PAGE_SIZE: usize = 32;

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
pub(crate) fn validate_identity(v: &str) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > 256 || !v.is_ascii() {
        Err(DomainError::InvalidIdentity)
    } else {
        Ok(())
    }
}
pub(crate) fn validate_slug(v: &str) -> Result<(), DomainError> {
    if !(1..=MAX_SLUG_LEN).contains(&v.len())
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
pub(crate) fn slug_with_suffix(base: &str, suffix: &str) -> String {
    let base_len = base
        .len()
        .min(MAX_SLUG_LEN.saturating_sub(suffix.len() + 1));
    format!("{}-{suffix}", &base[..base_len])
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

pub(crate) fn hex_id(id: [u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn decode_id(v: &str) -> Option<[u8; 16]> {
    if v.len() != 32 {
        return None;
    }
    let mut out = [0; 16];
    for (i, p) in v.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()?;
    }
    Some(out)
}

pub(crate) fn quarantine(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    code: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE records SET quarantine_code=?2 WHERE record_id=?1",
        rusqlite::params![id, code],
    )
    .map(|_| ())
}

pub fn row_message(r: &rusqlite::Row<'_>) -> Result<store::MessageRow, rusqlite::Error> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get::<_, i64>(5)? as u64,
    ))
}
