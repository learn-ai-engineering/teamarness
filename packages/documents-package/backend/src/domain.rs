//! Documents domain: records, validation, and the canonical codec.
//! Deliberately language-neutral; transport, key custody, and encryption stay in Core.
//!
//! A Collection, Document, or Comment is identified by the `RecordId` of the record that
//! created it, so a record that targets one carries that 64-character identity.

use std::fmt;

const MAGIC: &[u8] = b"TDOCS\x01";
pub(crate) const MAX_NAME: usize = 128;
pub(crate) const MAX_TITLE: usize = 256;
/// The largest Loro update one record carries. Base64 inside a JSON event (about 214 KB) still
/// fits one 256 KiB UI message, and the record stays under the 262,144-byte canonical bound.
pub(crate) const MAX_UPDATE: usize = 160 * 1024;
/// A Comment's Markdown is bounded like a Conversations Message.
pub(crate) const MAX_MARKDOWN: usize = 32 * 1024;
/// A quoted excerpt is short context, not a copy of the document.
pub(crate) const MAX_QUOTE: usize = 2 * 1024;
const TARGET_LEN: usize = 64;

/// The record vocabulary. Kind numbers are part of the canonical encoding and are never
/// reused: 10 is the retired plain-text `document.edit`, 11 the identified `document.edit`,
/// 20–22 are Comments, and 30 is reserved for `document.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainRecord {
    CollectionCreate {
        author: String,
        name: String,
    },
    CollectionRename {
        author: String,
        collection: String,
        name: String,
    },
    CollectionArchive {
        author: String,
        collection: String,
    },
    CollectionRestore {
        author: String,
        collection: String,
    },
    /// A Document and its body genesis: one Loro update, from a fresh peer, holding the root
    /// map, its attributes, the `children` list and one empty paragraph. Every Installation
    /// imports it before any edit, so no editor ever initialises an empty body.
    DocumentCreate {
        author: String,
        collection: String,
        title: String,
        genesis: Vec<u8>,
    },
    DocumentRename {
        author: String,
        document: String,
        title: String,
    },
    DocumentMove {
        author: String,
        document: String,
        collection: String,
    },
    DocumentArchive {
        author: String,
        document: String,
    },
    DocumentRestore {
        author: String,
        document: String,
    },
    /// One identified, bounded Loro update of a Document's body tree.
    IdentifiedDocumentEdit {
        author: String,
        document: String,
        request_id: String,
        payload_digest: [u8; 32],
        update: Vec<u8>,
    },
    /// A Comment on a Document, optionally replying to a parent Comment and quoting an excerpt.
    CommentAdd {
        author: String,
        document: String,
        parent: Option<String>,
        quote: Option<String>,
        markdown: String,
        /// Author-clock Unix milliseconds for display only; never an ordering authority.
        created_at: Option<String>,
    },
    CommentResolve {
        author: String,
        comment: String,
    },
    CommentReopen {
        author: String,
        comment: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    InvalidKind,
    InvalidLength,
    InvalidUtf8,
    InvalidIdentity,
    InvalidTarget,
    InvalidName,
    InvalidTitle,
    InvalidUpdate,
    /// A `document.create` without a body genesis, or with one that is not exactly the genesis.
    InvalidGenesis,
    InvalidRequestId,
    InvalidMarkdown,
    InvalidQuote,
    InvalidTimestamp,
}
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DomainError {}

impl DomainRecord {
    pub fn collection_create(author: &str, name: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_text(name, MAX_NAME, DomainError::InvalidName)?;
        Ok(Self::CollectionCreate {
            author: author.into(),
            name: name.into(),
        })
    }
    pub fn collection_rename(
        author: &str,
        collection: &str,
        name: &str,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(collection)?;
        validate_text(name, MAX_NAME, DomainError::InvalidName)?;
        Ok(Self::CollectionRename {
            author: author.into(),
            collection: collection.into(),
            name: name.into(),
        })
    }
    pub fn collection_archive(author: &str, collection: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(collection)?;
        Ok(Self::CollectionArchive {
            author: author.into(),
            collection: collection.into(),
        })
    }
    pub fn collection_restore(author: &str, collection: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(collection)?;
        Ok(Self::CollectionRestore {
            author: author.into(),
            collection: collection.into(),
        })
    }
    pub fn document_create(
        author: &str,
        collection: &str,
        title: &str,
        genesis: Vec<u8>,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(collection)?;
        validate_text(title, MAX_TITLE, DomainError::InvalidTitle)?;
        if genesis.is_empty()
            || genesis.len() > MAX_UPDATE
            || !crate::body::is_valid_genesis(&genesis)
        {
            return Err(DomainError::InvalidGenesis);
        }
        Ok(Self::DocumentCreate {
            author: author.into(),
            collection: collection.into(),
            title: title.into(),
            genesis,
        })
    }
    pub fn document_rename(author: &str, document: &str, title: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        validate_text(title, MAX_TITLE, DomainError::InvalidTitle)?;
        Ok(Self::DocumentRename {
            author: author.into(),
            document: document.into(),
            title: title.into(),
        })
    }
    pub fn document_move(
        author: &str,
        document: &str,
        collection: &str,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        validate_target(collection)?;
        Ok(Self::DocumentMove {
            author: author.into(),
            document: document.into(),
            collection: collection.into(),
        })
    }
    pub fn document_archive(author: &str, document: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        Ok(Self::DocumentArchive {
            author: author.into(),
            document: document.into(),
        })
    }
    pub fn document_restore(author: &str, document: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        Ok(Self::DocumentRestore {
            author: author.into(),
            document: document.into(),
        })
    }

    pub fn identified_document_edit(
        author: &str,
        document: &str,
        request_id: &str,
        payload_digest: &[u8],
        update: Vec<u8>,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        if request_id.len() != 32
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(DomainError::InvalidRequestId);
        }
        let digest: [u8; 32] = payload_digest
            .try_into()
            .map_err(|_| DomainError::InvalidUpdate)?;
        if update.is_empty() || update.len() > MAX_UPDATE {
            return Err(DomainError::InvalidUpdate);
        }
        Ok(Self::IdentifiedDocumentEdit {
            author: author.into(),
            document: document.into(),
            request_id: request_id.into(),
            payload_digest: digest,
            update,
        })
    }

    pub fn comment_add(
        author: &str,
        document: &str,
        parent: Option<&str>,
        quote: Option<&str>,
        markdown: &str,
    ) -> Result<Self, DomainError> {
        Self::comment_add_at(author, document, parent, quote, markdown, None)
    }

    pub fn comment_add_at(
        author: &str,
        document: &str,
        parent: Option<&str>,
        quote: Option<&str>,
        markdown: &str,
        created_at: Option<&str>,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        if let Some(time) = created_at {
            validate_timestamp(time)?;
        }
        if let Some(parent) = parent {
            validate_target(parent)?;
        }
        if let Some(quote) = quote
            && (quote.is_empty() || quote.len() > MAX_QUOTE)
        {
            return Err(DomainError::InvalidQuote);
        }
        if markdown.is_empty() || markdown.len() > MAX_MARKDOWN {
            return Err(DomainError::InvalidMarkdown);
        }
        Ok(Self::CommentAdd {
            author: author.into(),
            document: document.into(),
            parent: parent.map(str::to_owned),
            quote: quote.map(str::to_owned),
            markdown: markdown.into(),
            created_at: created_at.map(str::to_owned),
        })
    }
    pub fn comment_resolve(author: &str, comment: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(comment)?;
        Ok(Self::CommentResolve {
            author: author.into(),
            comment: comment.into(),
        })
    }
    pub fn comment_reopen(author: &str, comment: &str) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(comment)?;
        Ok(Self::CommentReopen {
            author: author.into(),
            comment: comment.into(),
        })
    }

    /// The signer every record claims; the projection refuses a record whose log author differs.
    pub fn author(&self) -> &str {
        match self {
            Self::CollectionCreate { author, .. }
            | Self::CollectionRename { author, .. }
            | Self::CollectionArchive { author, .. }
            | Self::CollectionRestore { author, .. }
            | Self::DocumentCreate { author, .. }
            | Self::DocumentRename { author, .. }
            | Self::DocumentMove { author, .. }
            | Self::DocumentArchive { author, .. }
            | Self::DocumentRestore { author, .. }
            | Self::IdentifiedDocumentEdit { author, .. }
            | Self::CommentAdd { author, .. }
            | Self::CommentResolve { author, .. }
            | Self::CommentReopen { author, .. } => author,
        }
    }

    /// The record this one must find before it can be projected, if any.
    ///
    /// A move names two targets; the Document is the one it waits on, and the destination
    /// Collection is checked when it is applied.
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::CollectionCreate { .. } => None,
            Self::CollectionRename { collection, .. }
            | Self::CollectionArchive { collection, .. }
            | Self::CollectionRestore { collection, .. }
            | Self::DocumentCreate { collection, .. } => Some(collection),
            Self::DocumentRename { document, .. }
            | Self::DocumentMove { document, .. }
            | Self::DocumentArchive { document, .. }
            | Self::DocumentRestore { document, .. }
            | Self::IdentifiedDocumentEdit { document, .. }
            | Self::CommentAdd { document, .. } => Some(document),
            Self::CommentResolve { comment, .. } | Self::CommentReopen { comment, .. } => {
                Some(comment)
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::CollectionCreate { .. } => "collection.create",
            Self::CollectionRename { .. } => "collection.rename",
            Self::CollectionArchive { .. } => "collection.archive",
            Self::CollectionRestore { .. } => "collection.restore",
            Self::DocumentCreate { .. } => "document.create",
            Self::DocumentRename { .. } => "document.rename",
            Self::DocumentMove { .. } => "document.move",
            Self::DocumentArchive { .. } => "document.archive",
            Self::DocumentRestore { .. } => "document.restore",
            Self::IdentifiedDocumentEdit { .. } => "document.edit",
            Self::CommentAdd { .. } => "comment.add",
            Self::CommentResolve { .. } => "comment.resolve",
            Self::CommentReopen { .. } => "comment.reopen",
        }
    }

    /// The purpose of body edit records, which the listing never needs to open.
    pub const EDIT_KIND: &'static str = "document.edit";
    /// The purpose of the record that creates a Document and carries its body genesis.
    pub const CREATE_KIND: &'static str = "document.create";

    /// Every purpose the listing and comments need opened after restart.
    pub const METADATA_KINDS: [&'static str; 12] = [
        "collection.create",
        "collection.rename",
        "collection.archive",
        "collection.restore",
        "document.create",
        "document.rename",
        "document.move",
        "document.archive",
        "document.restore",
        "comment.add",
        "comment.resolve",
        "comment.reopen",
    ];

    pub fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        match self {
            Self::CollectionCreate { author, name } => {
                out.push(1);
                put_text(&mut out, author);
                put_text(&mut out, name);
            }
            Self::CollectionRename {
                author,
                collection,
                name,
            } => {
                out.push(2);
                put_text(&mut out, author);
                put_text(&mut out, collection);
                put_text(&mut out, name);
            }
            Self::CollectionArchive { author, collection } => {
                out.push(3);
                put_text(&mut out, author);
                put_text(&mut out, collection);
            }
            Self::CollectionRestore { author, collection } => {
                out.push(4);
                put_text(&mut out, author);
                put_text(&mut out, collection);
            }
            Self::DocumentCreate {
                author,
                collection,
                title,
                genesis,
            } => {
                out.push(5);
                put_text(&mut out, author);
                put_text(&mut out, collection);
                put_text(&mut out, title);
                put_bytes(&mut out, genesis);
            }
            Self::DocumentRename {
                author,
                document,
                title,
            } => {
                out.push(6);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_text(&mut out, title);
            }
            Self::DocumentMove {
                author,
                document,
                collection,
            } => {
                out.push(7);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_text(&mut out, collection);
            }
            Self::DocumentArchive { author, document } => {
                out.push(8);
                put_text(&mut out, author);
                put_text(&mut out, document);
            }
            Self::DocumentRestore { author, document } => {
                out.push(9);
                put_text(&mut out, author);
                put_text(&mut out, document);
            }
            Self::IdentifiedDocumentEdit {
                author,
                document,
                request_id,
                payload_digest,
                update,
            } => {
                out.push(11);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_text(&mut out, request_id);
                out.extend_from_slice(payload_digest);
                put_bytes(&mut out, update);
            }
            Self::CommentAdd {
                author,
                document,
                parent,
                quote,
                markdown,
                created_at,
            } => {
                out.push(20);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_text(&mut out, parent.as_deref().unwrap_or(""));
                put_text(&mut out, quote.as_deref().unwrap_or(""));
                put_text(&mut out, markdown);
                if let Some(time) = created_at {
                    put_text(&mut out, time);
                }
            }
            Self::CommentResolve { author, comment } => {
                out.push(21);
                put_text(&mut out, author);
                put_text(&mut out, comment);
            }
            Self::CommentReopen { author, comment } => {
                out.push(22);
                put_text(&mut out, author);
                put_text(&mut out, comment);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DomainError> {
        if bytes.len() < MAGIC.len() + 1 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(DomainError::InvalidKind);
        }
        let mut p = MAGIC.len();
        let kind = bytes[p];
        p += 1;
        let author = take_text(bytes, &mut p)?;
        let record = match kind {
            1 => {
                let name = take_text(bytes, &mut p)?;
                Self::collection_create(&author, &name)?
            }
            2 => {
                let collection = take_text(bytes, &mut p)?;
                let name = take_text(bytes, &mut p)?;
                Self::collection_rename(&author, &collection, &name)?
            }
            3 => {
                let collection = take_text(bytes, &mut p)?;
                Self::collection_archive(&author, &collection)?
            }
            4 => {
                let collection = take_text(bytes, &mut p)?;
                Self::collection_restore(&author, &collection)?
            }
            5 => {
                let collection = take_text(bytes, &mut p)?;
                let title = take_text(bytes, &mut p)?;
                let genesis = take_bytes(bytes, &mut p).map_err(|_| DomainError::InvalidGenesis)?;
                Self::document_create(&author, &collection, &title, genesis)?
            }
            6 => {
                let document = take_text(bytes, &mut p)?;
                let title = take_text(bytes, &mut p)?;
                Self::document_rename(&author, &document, &title)?
            }
            7 => {
                let document = take_text(bytes, &mut p)?;
                let collection = take_text(bytes, &mut p)?;
                Self::document_move(&author, &document, &collection)?
            }
            8 => {
                let document = take_text(bytes, &mut p)?;
                Self::document_archive(&author, &document)?
            }
            9 => {
                let document = take_text(bytes, &mut p)?;
                Self::document_restore(&author, &document)?
            }
            // Kind 10 (the plain-text body) is retired: there are no such Documents to read.
            11 => {
                let document = take_text(bytes, &mut p)?;
                let request_id = take_text(bytes, &mut p)?;
                let digest = bytes.get(p..p + 32).ok_or(DomainError::InvalidLength)?;
                p += 32;
                let update = take_bytes(bytes, &mut p)?;
                Self::identified_document_edit(&author, &document, &request_id, digest, update)?
            }
            20 => {
                let document = take_text(bytes, &mut p)?;
                let parent = take_text(bytes, &mut p)?;
                let quote = take_text(bytes, &mut p)?;
                let markdown = take_text(bytes, &mut p)?;
                let created_at = if p == bytes.len() {
                    None
                } else {
                    Some(take_text(bytes, &mut p)?)
                };
                Self::comment_add_at(
                    &author,
                    &document,
                    (!parent.is_empty()).then_some(parent.as_str()),
                    (!quote.is_empty()).then_some(quote.as_str()),
                    &markdown,
                    created_at.as_deref(),
                )?
            }
            21 => {
                let comment = take_text(bytes, &mut p)?;
                Self::comment_resolve(&author, &comment)?
            }
            22 => {
                let comment = take_text(bytes, &mut p)?;
                Self::comment_reopen(&author, &comment)?
            }
            _ => return Err(DomainError::InvalidKind),
        };
        if p != bytes.len() {
            return Err(DomainError::InvalidLength);
        }
        Ok(record)
    }
}

pub(crate) fn validate_identity(v: &str) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > 256 || !v.is_ascii() {
        Err(DomainError::InvalidIdentity)
    } else {
        Ok(())
    }
}

/// A target is a record identity: 64 lowercase hexadecimal characters, the contract's grammar.
pub(crate) fn validate_target(v: &str) -> Result<(), DomainError> {
    if v.len() == TARGET_LEN
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(DomainError::InvalidTarget)
    }
}

/// A name or title: 1 to `max` Unicode scalar values, matching published character limits.
/// Control characters and leading or trailing whitespace are disallowed.
fn validate_text(v: &str, max: usize, error: DomainError) -> Result<(), DomainError> {
    if v.is_empty() || v.chars().count() > max || v.chars().any(char::is_control) || v.trim() != v {
        Err(error)
    } else {
        Ok(())
    }
}

fn validate_timestamp(time: &str) -> Result<(), DomainError> {
    // Canonical decimal Unix milliseconds; the year 9999 is the latest displayable date.
    if time.is_empty()
        || (time.len() > 1 && time.starts_with('0'))
        || !time.bytes().all(|b| b.is_ascii_digit())
        || !matches!(time.parse::<u64>(), Ok(value) if value <= 253_402_300_799_999)
    {
        return Err(DomainError::InvalidTimestamp);
    }
    Ok(())
}

fn put_text(out: &mut Vec<u8>, v: &str) {
    put_bytes(out, v.as_bytes());
}
fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    out.extend((u32::try_from(v.len()).unwrap()).to_be_bytes());
    out.extend(v);
}
fn take_bytes(b: &[u8], p: &mut usize) -> Result<Vec<u8>, DomainError> {
    if b.len().saturating_sub(*p) < 4 {
        return Err(DomainError::InvalidLength);
    };
    let n = u32::from_be_bytes(b[*p..*p + 4].try_into().unwrap()) as usize;
    *p += 4;
    if n > b.len().saturating_sub(*p) {
        return Err(DomainError::InvalidLength);
    };
    let out = b[*p..*p + n].to_vec();
    *p += n;
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn old_comment_fixture_and_new_author_clock_round_trip() {
        // Sealed TDOCS\x01 comment.add layout before creation time was appended.
        let mut old = b"TDOCS\x01\x14".to_vec();
        for field in ["alice", TARGET, "", "", "Old"] {
            put_text(&mut old, field);
        }
        assert_eq!(DomainRecord::decode(&old).unwrap().encode(), old);
        let new =
            DomainRecord::comment_add_at("alice", TARGET, None, None, "New", Some("1730000000000"))
                .unwrap();
        assert_eq!(DomainRecord::decode(&new.encode()).unwrap(), new);
        assert!(
            DomainRecord::comment_add_at("alice", TARGET, None, None, "x", Some("253402300800000"))
                .is_err()
        );
        assert!(
            DomainRecord::comment_add_at("alice", TARGET, None, None, "x", Some("01")).is_err()
        );
    }

    fn genesis() -> Vec<u8> {
        crate::body::genesis()
    }

    #[test]
    fn identified_edit_round_trips_and_kind_ten_is_refused() {
        let identified = DomainRecord::identified_document_edit(
            "alice",
            TARGET,
            &"a".repeat(32),
            &[3; 32],
            vec![1, 2],
        )
        .unwrap();
        assert_eq!(identified.encode()[6], 11);
        assert_eq!(
            DomainRecord::decode(&identified.encode()).unwrap(),
            identified
        );
        assert!(
            DomainRecord::identified_document_edit("alice", TARGET, "short", &[3], vec![1])
                .is_err()
        );
        // The retired plain-text layout: author, document, update.
        let mut kind_ten = MAGIC.to_vec();
        kind_ten.push(10);
        put_text(&mut kind_ten, "alice");
        put_text(&mut kind_ten, TARGET);
        put_bytes(&mut kind_ten, &[1, 2]);
        assert_eq!(
            DomainRecord::decode(&kind_ten).unwrap_err(),
            DomainError::InvalidKind
        );
    }

    // Verifies: TEAM.SYS.DOCS-R09 — one record carries at most 160 KiB of update.
    #[test]
    fn edit_updates_are_bounded_at_160_kib() {
        assert_eq!(MAX_UPDATE, 160 * 1024);
        let at_bound = DomainRecord::identified_document_edit(
            "alice",
            TARGET,
            &"a".repeat(32),
            &[3; 32],
            vec![7; MAX_UPDATE],
        )
        .unwrap();
        assert_eq!(DomainRecord::decode(&at_bound.encode()).unwrap(), at_bound);
        assert_eq!(
            DomainRecord::identified_document_edit(
                "alice",
                TARGET,
                &"a".repeat(32),
                &[3; 32],
                vec![7; MAX_UPDATE + 1],
            )
            .unwrap_err(),
            DomainError::InvalidUpdate
        );
        assert!(
            DomainRecord::identified_document_edit(
                "alice",
                TARGET,
                &"a".repeat(32),
                &[3; 32],
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn a_document_create_requires_exactly_a_body_genesis() {
        let create = DomainRecord::document_create("alice", TARGET, "Notes", genesis()).unwrap();
        assert_eq!(DomainRecord::decode(&create.encode()).unwrap(), create);
        // The layout before genesis: author, collection, title.
        let mut without = MAGIC.to_vec();
        without.push(5);
        for field in ["alice", TARGET, "Notes"] {
            put_text(&mut without, field);
        }
        assert_eq!(
            DomainRecord::decode(&without).unwrap_err(),
            DomainError::InvalidGenesis
        );
        for invalid in [Vec::new(), b"not loro".to_vec(), vec![1; MAX_UPDATE + 1]] {
            assert_eq!(
                DomainRecord::document_create("alice", TARGET, "Notes", invalid).unwrap_err(),
                DomainError::InvalidGenesis
            );
        }
        // A tree with content, or a schema-invalid tree, is not a genesis.
        let doc = loro::LoroDoc::new();
        crate::markdown::write_tree(
            &doc,
            &[crate::markdown::Block::Paragraph(vec![
                crate::markdown::Inline::Text("text".into(), crate::markdown::Marks::default()),
            ])],
        )
        .unwrap();
        let content = doc.export(loro::ExportMode::all_updates()).unwrap();
        let junk = loro::LoroDoc::new();
        junk.get_map("doc").insert("nodeName", "page").unwrap();
        junk.commit();
        let junk = junk.export(loro::ExportMode::all_updates()).unwrap();
        for invalid in [content, junk] {
            assert_eq!(
                DomainRecord::document_create("alice", TARGET, "Notes", invalid).unwrap_err(),
                DomainError::InvalidGenesis
            );
        }
        let mut swapped = create.encode();
        swapped.truncate(swapped.len() - 1);
        assert!(DomainRecord::decode(&swapped).is_err());
    }

    #[test]
    fn names_and_titles_count_unicode_characters() {
        assert!(DomainRecord::collection_create("alice", &"界".repeat(MAX_NAME)).is_ok());
        assert!(DomainRecord::collection_create("alice", &"界".repeat(MAX_NAME + 1)).is_err());
        assert!(
            DomainRecord::document_create("alice", TARGET, &"界".repeat(MAX_TITLE), genesis())
                .is_ok()
        );
        assert!(
            DomainRecord::document_create("alice", TARGET, &"界".repeat(MAX_TITLE + 1), genesis())
                .is_err()
        );
    }

    #[test]
    fn every_kind_round_trips() {
        let records = [
            DomainRecord::collection_create("alice", "Planning").unwrap(),
            DomainRecord::collection_rename("alice", TARGET, "Plans").unwrap(),
            DomainRecord::collection_archive("alice", TARGET).unwrap(),
            DomainRecord::collection_restore("alice", TARGET).unwrap(),
            DomainRecord::document_create("alice", TARGET, "Q4 plan", genesis()).unwrap(),
            DomainRecord::document_rename("alice", TARGET, "Q4 plan v2").unwrap(),
            DomainRecord::document_move("alice", TARGET, TARGET).unwrap(),
            DomainRecord::document_archive("alice", TARGET).unwrap(),
            DomainRecord::document_restore("alice", TARGET).unwrap(),
        ];
        for record in records {
            let bytes = record.encode();
            assert_eq!(DomainRecord::decode(&bytes).unwrap(), record);
            assert!(DomainRecord::METADATA_KINDS.contains(&record.kind()));
        }
        for record in [
            DomainRecord::comment_add("alice", TARGET, None, None, "Looks good").unwrap(),
            DomainRecord::comment_add("alice", TARGET, Some(TARGET), Some("quoted"), "Reply")
                .unwrap(),
            DomainRecord::comment_resolve("alice", TARGET).unwrap(),
            DomainRecord::comment_reopen("alice", TARGET).unwrap(),
        ] {
            assert_eq!(DomainRecord::decode(&record.encode()).unwrap(), record);
            assert!(DomainRecord::METADATA_KINDS.contains(&record.kind()));
        }
        assert!(DomainRecord::comment_add("alice", TARGET, None, None, "").is_err());
        assert!(DomainRecord::comment_add("alice", TARGET, None, Some(""), "x").is_err());
        assert!(
            DomainRecord::comment_add("alice", TARGET, None, Some(&"q".repeat(MAX_QUOTE + 1)), "x")
                .is_err()
        );
        assert!(DomainRecord::comment_add("alice", TARGET, Some("bad"), None, "x").is_err());
        assert!(
            DomainRecord::comment_add("alice", TARGET, None, None, &"m".repeat(MAX_MARKDOWN + 1))
                .is_err()
        );
        let edit = DomainRecord::identified_document_edit(
            "alice",
            TARGET,
            &"b".repeat(32),
            &[1; 32],
            vec![0, 255, 7],
        )
        .unwrap();
        assert_eq!(DomainRecord::decode(&edit.encode()).unwrap(), edit);
        assert_eq!(edit.kind(), DomainRecord::EDIT_KIND);
        assert!(!DomainRecord::METADATA_KINDS.contains(&edit.kind()));
    }

    #[test]
    fn bounds_and_grammar_are_enforced() {
        assert!(DomainRecord::collection_create("alice", "").is_err());
        assert!(DomainRecord::collection_create("alice", " padded").is_err());
        assert!(DomainRecord::collection_create("alice", "tab\there").is_err());
        assert!(DomainRecord::collection_create("alice", &"n".repeat(MAX_NAME + 1)).is_err());
        assert!(
            DomainRecord::document_create("alice", TARGET, &"t".repeat(MAX_TITLE + 1), genesis())
                .is_err()
        );
        assert!(DomainRecord::document_create("alice", "not-a-target", "t", genesis()).is_err());
        assert!(
            DomainRecord::document_create("alice", &TARGET.to_uppercase(), "t", genesis()).is_err()
        );
        assert!(DomainRecord::collection_create("", "x").is_err());
    }

    #[test]
    fn malformed_bytes_are_refused() {
        assert_eq!(
            DomainRecord::decode(b"TCONV\x01").unwrap_err(),
            DomainError::InvalidKind
        );
        let mut bytes = DomainRecord::collection_create("alice", "x")
            .unwrap()
            .encode();
        bytes.push(0);
        assert_eq!(
            DomainRecord::decode(&bytes).unwrap_err(),
            DomainError::InvalidLength
        );
        let mut unknown = MAGIC.to_vec();
        unknown.push(23);
        put_text(&mut unknown, "alice");
        assert_eq!(
            DomainRecord::decode(&unknown).unwrap_err(),
            DomainError::InvalidKind
        );
        let truncated = &DomainRecord::collection_create("alice", "x")
            .unwrap()
            .encode()[..9];
        assert!(DomainRecord::decode(truncated).is_err());
    }
}
