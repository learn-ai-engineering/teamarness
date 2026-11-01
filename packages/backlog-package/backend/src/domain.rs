//! Backlog domain: records, validation, and the canonical codec.
//! Deliberately language-neutral; transport, key custody, and encryption stay in Core.
//!
//! An Epic or Ticket is identified by the `RecordId` of the record that created it, so a
//! record that targets one carries that 64-character identity. Every record carries its
//! `author`, which must equal the record's signer, and `at`, the author's clock in Unix
//! milliseconds, shown to people and never used for ordering.

use std::fmt;

const MAGIC: &[u8] = b"TBLOG\x01";
/// A title holds 1 to 256 Unicode scalar values.
pub(crate) const MAX_TITLE: usize = 256;
/// A body holds at most 64 KiB of UTF-8 Markdown.
pub(crate) const MAX_BODY: usize = 65_536;
/// The year 9999 is the latest displayable author-clock time.
pub(crate) const MAX_AT: u64 = 253_402_300_799_999;
const TARGET_LEN: usize = 64;

/// What a Ticket's lifecycle stage is. The order here is the listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    NotReady,
    Ready,
    InProgress,
    Done,
}

impl Status {
    pub const ALL: [Self; 4] = [Self::NotReady, Self::Ready, Self::InProgress, Self::Done];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotReady => "not_ready",
            Self::Ready => "ready",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }

    /// Parses the wire name.
    ///
    /// # Errors
    ///
    /// `Invalid(Status)` for any other string.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
            .ok_or(DomainError::Invalid(Field::Status))
    }

    fn code(self) -> u8 {
        match self {
            Self::NotReady => 1,
            Self::Ready => 2,
            Self::InProgress => 3,
            Self::Done => 4,
        }
    }

    fn from_code(code: u8) -> Result<Self, DomainError> {
        Self::ALL
            .into_iter()
            .find(|status| status.code() == code)
            .ok_or(DomainError::Invalid(Field::Status))
    }
}

/// How urgent a Ticket is. The order here is the listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl Priority {
    pub const ALL: [Self; 3] = [Self::High, Self::Medium, Self::Low];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// Parses the wire name.
    ///
    /// # Errors
    ///
    /// `Invalid(Priority)` for any other string.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .into_iter()
            .find(|priority| priority.as_str() == value)
            .ok_or(DomainError::Invalid(Field::Priority))
    }

    fn code(self) -> u8 {
        match self {
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }

    fn from_code(code: u8) -> Result<Self, DomainError> {
        Self::ALL
            .into_iter()
            .find(|priority| priority.code() == code)
            .ok_or(DomainError::Invalid(Field::Priority))
    }
}

/// A new Ticket's status when the creator gives none.
pub const DEFAULT_STATUS: Status = Status::NotReady;
/// A new Ticket's priority when the creator gives none.
pub const DEFAULT_PRIORITY: Priority = Priority::Medium;

/// The fields one `ticket.update` changes: at least one is present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketChanges {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<Status>,
    pub priority: Option<Priority>,
}

impl TicketChanges {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.status.is_none()
            && self.priority.is_none()
    }
}

const CHANGE_TITLE: u8 = 1;
const CHANGE_BODY: u8 = 2;
const CHANGE_STATUS: u8 = 4;
const CHANGE_PRIORITY: u8 = 8;
const CHANGE_ALL: u8 = CHANGE_TITLE | CHANGE_BODY | CHANGE_STATUS | CHANGE_PRIORITY;

/// The record vocabulary. Kind numbers are part of the canonical encoding and are never
/// reused: 1–4 are Epic records and 10–15 are Ticket records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainRecord {
    EpicCreate {
        author: String,
        at: u64,
        title: String,
    },
    EpicRename {
        author: String,
        at: u64,
        epic: String,
        title: String,
    },
    EpicArchive {
        author: String,
        at: u64,
        epic: String,
    },
    EpicRestore {
        author: String,
        at: u64,
        epic: String,
    },
    TicketCreate {
        author: String,
        at: u64,
        epic: String,
        title: String,
        body: String,
        status: Status,
        priority: Priority,
    },
    /// Only the fields the saver changed, so concurrent saves of different fields all survive.
    TicketUpdate {
        author: String,
        at: u64,
        ticket: String,
        changes: TicketChanges,
    },
    /// The assignee is a Workspace Principal ID, or `None` to unassign.
    TicketAssign {
        author: String,
        at: u64,
        ticket: String,
        assignee: Option<String>,
    },
    TicketMove {
        author: String,
        at: u64,
        ticket: String,
        epic: String,
    },
    TicketArchive {
        author: String,
        at: u64,
        ticket: String,
    },
    TicketRestore {
        author: String,
        at: u64,
        ticket: String,
    },
}

/// The record field a validation failure concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Author,
    At,
    Epic,
    Ticket,
    Title,
    Body,
    Status,
    Priority,
    Assignee,
    /// A `ticket.update` that changes nothing.
    Changes,
}

impl Field {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::At => "at",
            Self::Epic => "epicId",
            Self::Ticket => "ticketId",
            Self::Title => "title",
            Self::Body => "body",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Assignee => "assignee",
            Self::Changes => "changes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// Not a Backlog record, or an unknown kind byte.
    InvalidKind,
    /// Truncated or trailing bytes.
    InvalidLength,
    InvalidUtf8,
    /// A field breaks its grammar.
    Invalid(Field),
    /// A title over 256 characters or a body over 65,536 bytes.
    TooLarge(Field),
}

impl DomainError {
    /// The published limit a `TooLarge` field exceeded.
    pub fn limit(&self) -> Option<usize> {
        match self {
            Self::TooLarge(Field::Title) => Some(MAX_TITLE),
            Self::TooLarge(Field::Body) => Some(MAX_BODY),
            _ => None,
        }
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DomainError {}

impl DomainRecord {
    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn epic_create(author: &str, at: u64, title: &str) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_title(title)?;
        Ok(Self::EpicCreate {
            author: author.into(),
            at,
            title: title.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn epic_rename(
        author: &str,
        at: u64,
        epic: &str,
        title: &str,
    ) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(epic, Field::Epic)?;
        validate_title(title)?;
        Ok(Self::EpicRename {
            author: author.into(),
            at,
            epic: epic.into(),
            title: title.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn epic_archive(author: &str, at: u64, epic: &str) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(epic, Field::Epic)?;
        Ok(Self::EpicArchive {
            author: author.into(),
            at,
            epic: epic.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn epic_restore(author: &str, at: u64, epic: &str) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(epic, Field::Epic)?;
        Ok(Self::EpicRestore {
            author: author.into(),
            at,
            epic: epic.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn ticket_create(
        author: &str,
        at: u64,
        epic: &str,
        title: &str,
        body: &str,
        status: Status,
        priority: Priority,
    ) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(epic, Field::Epic)?;
        validate_title(title)?;
        validate_body(body)?;
        Ok(Self::TicketCreate {
            author: author.into(),
            at,
            epic: epic.into(),
            title: title.into(),
            body: body.into(),
            status,
            priority,
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar, or `Invalid(Changes)` when nothing changes.
    pub fn ticket_update(
        author: &str,
        at: u64,
        ticket: &str,
        changes: TicketChanges,
    ) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(ticket, Field::Ticket)?;
        if changes.is_empty() {
            return Err(DomainError::Invalid(Field::Changes));
        }
        if let Some(title) = &changes.title {
            validate_title(title)?;
        }
        if let Some(body) = &changes.body {
            validate_body(body)?;
        }
        Ok(Self::TicketUpdate {
            author: author.into(),
            at,
            ticket: ticket.into(),
            changes,
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn ticket_assign(
        author: &str,
        at: u64,
        ticket: &str,
        assignee: Option<&str>,
    ) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(ticket, Field::Ticket)?;
        if let Some(assignee) = assignee {
            validate_target(assignee, Field::Assignee)?;
        }
        Ok(Self::TicketAssign {
            author: author.into(),
            at,
            ticket: ticket.into(),
            assignee: assignee.map(str::to_owned),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn ticket_move(
        author: &str,
        at: u64,
        ticket: &str,
        epic: &str,
    ) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(ticket, Field::Ticket)?;
        validate_target(epic, Field::Epic)?;
        Ok(Self::TicketMove {
            author: author.into(),
            at,
            ticket: ticket.into(),
            epic: epic.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn ticket_archive(author: &str, at: u64, ticket: &str) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(ticket, Field::Ticket)?;
        Ok(Self::TicketArchive {
            author: author.into(),
            at,
            ticket: ticket.into(),
        })
    }

    /// # Errors
    ///
    /// The first field that breaks its grammar.
    pub fn ticket_restore(author: &str, at: u64, ticket: &str) -> Result<Self, DomainError> {
        validate_common(author, at)?;
        validate_target(ticket, Field::Ticket)?;
        Ok(Self::TicketRestore {
            author: author.into(),
            at,
            ticket: ticket.into(),
        })
    }

    pub fn author(&self) -> &str {
        match self {
            Self::EpicCreate { author, .. }
            | Self::EpicRename { author, .. }
            | Self::EpicArchive { author, .. }
            | Self::EpicRestore { author, .. }
            | Self::TicketCreate { author, .. }
            | Self::TicketUpdate { author, .. }
            | Self::TicketAssign { author, .. }
            | Self::TicketMove { author, .. }
            | Self::TicketArchive { author, .. }
            | Self::TicketRestore { author, .. } => author,
        }
    }

    /// The author-clock time in Unix milliseconds, for display only.
    pub fn at(&self) -> u64 {
        match self {
            Self::EpicCreate { at, .. }
            | Self::EpicRename { at, .. }
            | Self::EpicArchive { at, .. }
            | Self::EpicRestore { at, .. }
            | Self::TicketCreate { at, .. }
            | Self::TicketUpdate { at, .. }
            | Self::TicketAssign { at, .. }
            | Self::TicketMove { at, .. }
            | Self::TicketArchive { at, .. }
            | Self::TicketRestore { at, .. } => *at,
        }
    }

    /// The record's purpose on the record log, one per kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::EpicCreate { .. } => "epic.create",
            Self::EpicRename { .. } => "epic.rename",
            Self::EpicArchive { .. } => "epic.archive",
            Self::EpicRestore { .. } => "epic.restore",
            Self::TicketCreate { .. } => "ticket.create",
            Self::TicketUpdate { .. } => "ticket.update",
            Self::TicketAssign { .. } => "ticket.assign",
            Self::TicketMove { .. } => "ticket.move",
            Self::TicketArchive { .. } => "ticket.archive",
            Self::TicketRestore { .. } => "ticket.restore",
        }
    }

    /// Every purpose a Backlog record carries; all of them must be opened after restart.
    pub const KINDS: [&'static str; 10] = [
        "epic.create",
        "epic.rename",
        "epic.archive",
        "epic.restore",
        "ticket.create",
        "ticket.update",
        "ticket.assign",
        "ticket.move",
        "ticket.archive",
        "ticket.restore",
    ];

    fn kind_byte(&self) -> u8 {
        match self {
            Self::EpicCreate { .. } => 1,
            Self::EpicRename { .. } => 2,
            Self::EpicArchive { .. } => 3,
            Self::EpicRestore { .. } => 4,
            Self::TicketCreate { .. } => 10,
            Self::TicketUpdate { .. } => 11,
            Self::TicketAssign { .. } => 12,
            Self::TicketMove { .. } => 13,
            Self::TicketArchive { .. } => 14,
            Self::TicketRestore { .. } => 15,
        }
    }

    /// The canonical bytes: magic, kind byte, author, `at`, then the kind's fields.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.push(self.kind_byte());
        put_text(&mut out, self.author());
        out.extend(self.at().to_be_bytes());
        match self {
            Self::EpicCreate { title, .. } => put_text(&mut out, title),
            Self::EpicRename { epic, title, .. } => {
                put_text(&mut out, epic);
                put_text(&mut out, title);
            }
            Self::EpicArchive { epic, .. } | Self::EpicRestore { epic, .. } => {
                put_text(&mut out, epic);
            }
            Self::TicketCreate {
                epic,
                title,
                body,
                status,
                priority,
                ..
            } => {
                put_text(&mut out, epic);
                put_text(&mut out, title);
                put_text(&mut out, body);
                out.push(status.code());
                out.push(priority.code());
            }
            Self::TicketUpdate {
                ticket, changes, ..
            } => {
                put_text(&mut out, ticket);
                let mut mask = 0;
                if changes.title.is_some() {
                    mask |= CHANGE_TITLE;
                }
                if changes.body.is_some() {
                    mask |= CHANGE_BODY;
                }
                if changes.status.is_some() {
                    mask |= CHANGE_STATUS;
                }
                if changes.priority.is_some() {
                    mask |= CHANGE_PRIORITY;
                }
                out.push(mask);
                if let Some(title) = &changes.title {
                    put_text(&mut out, title);
                }
                if let Some(body) = &changes.body {
                    put_text(&mut out, body);
                }
                if let Some(status) = changes.status {
                    out.push(status.code());
                }
                if let Some(priority) = changes.priority {
                    out.push(priority.code());
                }
            }
            Self::TicketAssign {
                ticket, assignee, ..
            } => {
                put_text(&mut out, ticket);
                match assignee {
                    Some(assignee) => {
                        out.push(1);
                        put_text(&mut out, assignee);
                    }
                    None => out.push(0),
                }
            }
            Self::TicketMove { ticket, epic, .. } => {
                put_text(&mut out, ticket);
                put_text(&mut out, epic);
            }
            Self::TicketArchive { ticket, .. } | Self::TicketRestore { ticket, .. } => {
                put_text(&mut out, ticket);
            }
        }
        out
    }

    /// Decodes and validates canonical bytes.
    ///
    /// # Errors
    ///
    /// A foreign or unknown kind, malformed bytes, or the first field that breaks its grammar.
    pub fn decode(bytes: &[u8]) -> Result<Self, DomainError> {
        if bytes.len() < MAGIC.len() + 1 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(DomainError::InvalidKind);
        }
        let mut p = MAGIC.len();
        let kind = bytes[p];
        p += 1;
        if !matches!(kind, 1..=4 | 10..=15) {
            return Err(DomainError::InvalidKind);
        }
        let author = take_text(bytes, &mut p)?;
        let at = take_u64(bytes, &mut p)?;
        let record = match kind {
            1 => {
                let title = take_text(bytes, &mut p)?;
                Self::epic_create(&author, at, &title)?
            }
            2 => {
                let epic = take_text(bytes, &mut p)?;
                let title = take_text(bytes, &mut p)?;
                Self::epic_rename(&author, at, &epic, &title)?
            }
            3 => {
                let epic = take_text(bytes, &mut p)?;
                Self::epic_archive(&author, at, &epic)?
            }
            4 => {
                let epic = take_text(bytes, &mut p)?;
                Self::epic_restore(&author, at, &epic)?
            }
            10 => {
                let epic = take_text(bytes, &mut p)?;
                let title = take_text(bytes, &mut p)?;
                let body = take_text(bytes, &mut p)?;
                let status = Status::from_code(take_u8(bytes, &mut p)?)?;
                let priority = Priority::from_code(take_u8(bytes, &mut p)?)?;
                Self::ticket_create(&author, at, &epic, &title, &body, status, priority)?
            }
            11 => {
                let ticket = take_text(bytes, &mut p)?;
                let mask = take_u8(bytes, &mut p)?;
                if mask == 0 || mask & !CHANGE_ALL != 0 {
                    return Err(DomainError::Invalid(Field::Changes));
                }
                let mut changes = TicketChanges::default();
                if mask & CHANGE_TITLE != 0 {
                    changes.title = Some(take_text(bytes, &mut p)?);
                }
                if mask & CHANGE_BODY != 0 {
                    changes.body = Some(take_text(bytes, &mut p)?);
                }
                if mask & CHANGE_STATUS != 0 {
                    changes.status = Some(Status::from_code(take_u8(bytes, &mut p)?)?);
                }
                if mask & CHANGE_PRIORITY != 0 {
                    changes.priority = Some(Priority::from_code(take_u8(bytes, &mut p)?)?);
                }
                Self::ticket_update(&author, at, &ticket, changes)?
            }
            12 => {
                let ticket = take_text(bytes, &mut p)?;
                let assignee = match take_u8(bytes, &mut p)? {
                    0 => None,
                    1 => Some(take_text(bytes, &mut p)?),
                    _ => return Err(DomainError::Invalid(Field::Assignee)),
                };
                Self::ticket_assign(&author, at, &ticket, assignee.as_deref())?
            }
            13 => {
                let ticket = take_text(bytes, &mut p)?;
                let epic = take_text(bytes, &mut p)?;
                Self::ticket_move(&author, at, &ticket, &epic)?
            }
            14 => {
                let ticket = take_text(bytes, &mut p)?;
                Self::ticket_archive(&author, at, &ticket)?
            }
            15 => {
                let ticket = take_text(bytes, &mut p)?;
                Self::ticket_restore(&author, at, &ticket)?
            }
            _ => return Err(DomainError::InvalidKind),
        };
        if p != bytes.len() {
            return Err(DomainError::InvalidLength);
        }
        Ok(record)
    }
}

fn validate_common(author: &str, at: u64) -> Result<(), DomainError> {
    validate_identity(author)?;
    if at > MAX_AT {
        return Err(DomainError::Invalid(Field::At));
    }
    Ok(())
}

/// A signer identity: non-empty printable ASCII of at most 256 bytes.
pub(crate) fn validate_identity(v: &str) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > 256 || !v.bytes().all(|b| b.is_ascii_graphic()) {
        Err(DomainError::Invalid(Field::Author))
    } else {
        Ok(())
    }
}

/// A target is a record identity or Principal ID: 64 lowercase hexadecimal characters.
pub(crate) fn validate_target(v: &str, field: Field) -> Result<(), DomainError> {
    if v.len() == TARGET_LEN
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(DomainError::Invalid(field))
    }
}

/// A title: 1 to 256 Unicode scalar values, no control characters, and no leading or
/// trailing whitespace, matching the Documents title grammar.
pub(crate) fn validate_title(v: &str) -> Result<(), DomainError> {
    if v.chars().count() > MAX_TITLE {
        return Err(DomainError::TooLarge(Field::Title));
    }
    if v.is_empty() || v.chars().any(char::is_control) || v.trim() != v {
        return Err(DomainError::Invalid(Field::Title));
    }
    Ok(())
}

/// A body: at most 65,536 bytes of UTF-8 Markdown. It may be empty and may hold tabs and line
/// breaks, but no other control characters.
pub(crate) fn validate_body(v: &str) -> Result<(), DomainError> {
    if v.len() > MAX_BODY {
        return Err(DomainError::TooLarge(Field::Body));
    }
    if v.chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(DomainError::Invalid(Field::Body));
    }
    Ok(())
}

fn put_text(out: &mut Vec<u8>, v: &str) {
    out.extend(
        u32::try_from(v.len())
            .expect("validated fields fit a u32 length")
            .to_be_bytes(),
    );
    out.extend(v.as_bytes());
}

fn take_u8(b: &[u8], p: &mut usize) -> Result<u8, DomainError> {
    let value = *b.get(*p).ok_or(DomainError::InvalidLength)?;
    *p += 1;
    Ok(value)
}

fn take_u64(b: &[u8], p: &mut usize) -> Result<u64, DomainError> {
    let bytes: [u8; 8] = b
        .get(*p..*p + 8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(DomainError::InvalidLength)?;
    *p += 8;
    Ok(u64::from_be_bytes(bytes))
}

fn take_text(b: &[u8], p: &mut usize) -> Result<String, DomainError> {
    let length: [u8; 4] = b
        .get(*p..*p + 4)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(DomainError::InvalidLength)?;
    *p += 4;
    let n = u32::from_be_bytes(length) as usize;
    if n > b.len().saturating_sub(*p) {
        return Err(DomainError::InvalidLength);
    }
    let s = std::str::from_utf8(&b[*p..*p + n])
        .map_err(|_| DomainError::InvalidUtf8)?
        .to_owned();
    *p += n;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTHOR: &str = "alice";
    const AT: u64 = 1_761_000_000_000;
    const EPIC: &str = "e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1";
    const TICKET: &str = "7171717171717171717171717171717171717171717171717171717171717171";
    const PERSON: &str = "9090909090909090909090909090909090909090909090909090909090909090";

    /// Hex of a length-prefixed text field, written independently of the codec.
    fn text(value: &str) -> String {
        let mut out = format!("{:08x}", value.len());
        for byte in value.as_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The header every golden shares: magic, kind byte, author, `at`.
    fn header(kind: u8) -> String {
        format!("54424c4f4701{kind:02x}{}{AT:016x}", text(AUTHOR))
    }

    fn every_kind() -> Vec<(DomainRecord, String)> {
        vec![
            (
                DomainRecord::epic_create(AUTHOR, AT, "Launch").unwrap(),
                format!("{}{}", header(1), text("Launch")),
            ),
            (
                DomainRecord::epic_rename(AUTHOR, AT, EPIC, "Relaunch").unwrap(),
                format!("{}{}{}", header(2), text(EPIC), text("Relaunch")),
            ),
            (
                DomainRecord::epic_archive(AUTHOR, AT, EPIC).unwrap(),
                format!("{}{}", header(3), text(EPIC)),
            ),
            (
                DomainRecord::epic_restore(AUTHOR, AT, EPIC).unwrap(),
                format!("{}{}", header(4), text(EPIC)),
            ),
            (
                DomainRecord::ticket_create(
                    AUTHOR,
                    AT,
                    EPIC,
                    "Ship it",
                    "- [ ] task",
                    Status::Ready,
                    Priority::High,
                )
                .unwrap(),
                format!(
                    "{}{}{}{}0201",
                    header(10),
                    text(EPIC),
                    text("Ship it"),
                    text("- [ ] task")
                ),
            ),
            (
                DomainRecord::ticket_update(
                    AUTHOR,
                    AT,
                    TICKET,
                    TicketChanges {
                        title: Some("New".into()),
                        body: Some("Body".into()),
                        status: Some(Status::Done),
                        priority: Some(Priority::Low),
                    },
                )
                .unwrap(),
                format!(
                    "{}{}0f{}{}0403",
                    header(11),
                    text(TICKET),
                    text("New"),
                    text("Body")
                ),
            ),
            (
                DomainRecord::ticket_update(
                    AUTHOR,
                    AT,
                    TICKET,
                    TicketChanges {
                        status: Some(Status::InProgress),
                        ..TicketChanges::default()
                    },
                )
                .unwrap(),
                format!("{}{}0403", header(11), text(TICKET)),
            ),
            (
                DomainRecord::ticket_assign(AUTHOR, AT, TICKET, Some(PERSON)).unwrap(),
                format!("{}{}01{}", header(12), text(TICKET), text(PERSON)),
            ),
            (
                DomainRecord::ticket_assign(AUTHOR, AT, TICKET, None).unwrap(),
                format!("{}{}00", header(12), text(TICKET)),
            ),
            (
                DomainRecord::ticket_move(AUTHOR, AT, TICKET, EPIC).unwrap(),
                format!("{}{}{}", header(13), text(TICKET), text(EPIC)),
            ),
            (
                DomainRecord::ticket_archive(AUTHOR, AT, TICKET).unwrap(),
                format!("{}{}", header(14), text(TICKET)),
            ),
            (
                DomainRecord::ticket_restore(AUTHOR, AT, TICKET).unwrap(),
                format!("{}{}", header(15), text(TICKET)),
            ),
        ]
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — the canonical encoding of every kind is pinned.
    #[test]
    fn every_kind_matches_its_golden_bytes_and_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for (record, golden) in every_kind() {
            let bytes = record.encode();
            assert_eq!(hex(&bytes), golden, "{}", record.kind());
            assert_eq!(DomainRecord::decode(&bytes).unwrap(), record);
            assert!(DomainRecord::KINDS.contains(&record.kind()));
            seen.insert(record.kind());
        }
        assert_eq!(seen.len(), DomainRecord::KINDS.len());
    }

    #[test]
    fn the_golden_header_spells_the_magic_and_big_endian_time() {
        let bytes = DomainRecord::epic_create("a", 0x0102, "T")
            .unwrap()
            .encode();
        assert_eq!(
            bytes,
            [
                b"TBLOG\x01".as_slice(),
                &[1],
                &[0, 0, 0, 1, b'a'],
                &[0, 0, 0, 0, 0, 0, 1, 2],
                &[0, 0, 0, 1, b'T'],
            ]
            .concat()
        );
    }

    // Verifies: TEAM.SYS.BACKLOG-R05 — title limits count Unicode characters.
    #[test]
    fn titles_hold_1_to_256_characters_without_control_characters() {
        assert!(DomainRecord::epic_create(AUTHOR, AT, &"界".repeat(MAX_TITLE)).is_ok());
        assert_eq!(
            DomainRecord::epic_create(AUTHOR, AT, &"界".repeat(MAX_TITLE + 1)).unwrap_err(),
            DomainError::TooLarge(Field::Title)
        );
        assert_eq!(DomainError::TooLarge(Field::Title).limit(), Some(MAX_TITLE));
        for invalid in [
            "",
            "tab\there",
            "line\nbreak",
            " padded",
            "padded ",
            "bell\u{7}",
        ] {
            assert_eq!(
                DomainRecord::epic_create(AUTHOR, AT, invalid).unwrap_err(),
                DomainError::Invalid(Field::Title),
                "{invalid:?}"
            );
        }
    }

    // Verifies: TEAM.SYS.BACKLOG-R05 — the body bound is 65,536 bytes; tabs and line breaks pass.
    #[test]
    fn bodies_hold_at_most_65536_bytes_and_allow_tabs_and_line_breaks() {
        let create = |body: &str| {
            DomainRecord::ticket_create(AUTHOR, AT, EPIC, "T", body, Status::Ready, Priority::Low)
        };
        let at_bound = create(&"b".repeat(MAX_BODY)).unwrap();
        assert_eq!(DomainRecord::decode(&at_bound.encode()).unwrap(), at_bound);
        assert_eq!(
            create(&"b".repeat(MAX_BODY + 1)).unwrap_err(),
            DomainError::TooLarge(Field::Body)
        );
        // Multi-byte characters count by bytes, not characters.
        assert!(create(&"界".repeat(MAX_BODY / 3 + 1)).is_err());
        assert_eq!(DomainError::TooLarge(Field::Body).limit(), Some(MAX_BODY));
        assert!(create("").is_ok());
        assert!(create("a\tb\nc\r\nd").is_ok());
        for invalid in ["nul\0", "bell\u{7}", "esc\u{1b}[0m", "del\u{7f}"] {
            assert_eq!(
                create(invalid).unwrap_err(),
                DomainError::Invalid(Field::Body),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn statuses_and_priorities_parse_only_their_wire_names() {
        for status in Status::ALL {
            assert_eq!(Status::parse(status.as_str()).unwrap(), status);
        }
        for priority in Priority::ALL {
            assert_eq!(Priority::parse(priority.as_str()).unwrap(), priority);
        }
        for bad in ["", "Ready", "blocked", "not-ready"] {
            assert_eq!(
                Status::parse(bad).unwrap_err(),
                DomainError::Invalid(Field::Status)
            );
        }
        for bad in ["", "High", "urgent"] {
            assert_eq!(
                Priority::parse(bad).unwrap_err(),
                DomainError::Invalid(Field::Priority)
            );
        }
        assert_eq!(DEFAULT_STATUS, Status::NotReady);
        assert_eq!(DEFAULT_PRIORITY, Priority::Medium);
    }

    #[test]
    fn bad_enum_codes_are_refused_at_decode() {
        let mut bad_status =
            DomainRecord::ticket_create(AUTHOR, AT, EPIC, "T", "", Status::Ready, Priority::Low)
                .unwrap()
                .encode();
        let status_at = bad_status.len() - 2;
        bad_status[status_at] = 5;
        assert_eq!(
            DomainRecord::decode(&bad_status).unwrap_err(),
            DomainError::Invalid(Field::Status)
        );
        let mut bad_priority = bad_status.clone();
        bad_priority[status_at] = 1;
        let priority_at = bad_priority.len() - 1;
        bad_priority[priority_at] = 0;
        assert_eq!(
            DomainRecord::decode(&bad_priority).unwrap_err(),
            DomainError::Invalid(Field::Priority)
        );
        let mut bad_assign = DomainRecord::ticket_assign(AUTHOR, AT, TICKET, None)
            .unwrap()
            .encode();
        *bad_assign.last_mut().unwrap() = 2;
        assert_eq!(
            DomainRecord::decode(&bad_assign).unwrap_err(),
            DomainError::Invalid(Field::Assignee)
        );
    }

    #[test]
    fn an_update_must_change_at_least_one_field() {
        assert_eq!(
            DomainRecord::ticket_update(AUTHOR, AT, TICKET, TicketChanges::default()).unwrap_err(),
            DomainError::Invalid(Field::Changes)
        );
        let mut empty = MAGIC.to_vec();
        empty.push(11);
        put_text(&mut empty, AUTHOR);
        empty.extend(AT.to_be_bytes());
        put_text(&mut empty, TICKET);
        let mut zero = empty.clone();
        zero.push(0);
        assert_eq!(
            DomainRecord::decode(&zero).unwrap_err(),
            DomainError::Invalid(Field::Changes)
        );
        let mut unknown_bit = empty;
        unknown_bit.extend([0x10, 1]);
        assert_eq!(
            DomainRecord::decode(&unknown_bit).unwrap_err(),
            DomainError::Invalid(Field::Changes)
        );
        // A present field is validated like any other.
        assert_eq!(
            DomainRecord::ticket_update(
                AUTHOR,
                AT,
                TICKET,
                TicketChanges {
                    title: Some(String::new()),
                    ..TicketChanges::default()
                }
            )
            .unwrap_err(),
            DomainError::Invalid(Field::Title)
        );
    }

    #[test]
    fn targets_assignees_authors_and_times_are_checked() {
        assert_eq!(
            DomainRecord::epic_archive(AUTHOR, AT, "short").unwrap_err(),
            DomainError::Invalid(Field::Epic)
        );
        assert_eq!(
            DomainRecord::ticket_archive(AUTHOR, AT, &EPIC.to_uppercase()).unwrap_err(),
            DomainError::Invalid(Field::Ticket)
        );
        assert_eq!(
            DomainRecord::ticket_assign(AUTHOR, AT, TICKET, Some("nobody")).unwrap_err(),
            DomainError::Invalid(Field::Assignee)
        );
        assert_eq!(
            DomainRecord::epic_create("", AT, "T").unwrap_err(),
            DomainError::Invalid(Field::Author)
        );
        assert_eq!(
            DomainRecord::epic_create("a b", AT, "T").unwrap_err(),
            DomainError::Invalid(Field::Author)
        );
        assert!(DomainRecord::epic_create(AUTHOR, MAX_AT, "T").is_ok());
        assert_eq!(
            DomainRecord::epic_create(AUTHOR, MAX_AT + 1, "T").unwrap_err(),
            DomainError::Invalid(Field::At)
        );
    }

    #[test]
    fn malformed_bytes_are_refused() {
        assert_eq!(
            DomainRecord::decode(b"TDOCS\x01\x01").unwrap_err(),
            DomainError::InvalidKind
        );
        assert_eq!(
            DomainRecord::decode(b"TBLOG\x01").unwrap_err(),
            DomainError::InvalidKind
        );
        for kind in [0u8, 5, 9, 16, 255] {
            let mut unknown = MAGIC.to_vec();
            unknown.push(kind);
            put_text(&mut unknown, AUTHOR);
            assert_eq!(
                DomainRecord::decode(&unknown).unwrap_err(),
                DomainError::InvalidKind,
                "kind {kind}"
            );
        }
        let bytes = DomainRecord::epic_create(AUTHOR, AT, "T").unwrap().encode();
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            DomainRecord::decode(&trailing).unwrap_err(),
            DomainError::InvalidLength
        );
        for cut in 7..bytes.len() {
            assert!(DomainRecord::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut not_utf8 = MAGIC.to_vec();
        not_utf8.push(1);
        put_text(&mut not_utf8, AUTHOR);
        not_utf8.extend(AT.to_be_bytes());
        not_utf8.extend([0, 0, 0, 1, 0xff]);
        assert_eq!(
            DomainRecord::decode(&not_utf8).unwrap_err(),
            DomainError::InvalidUtf8
        );
    }
}
