//! Documents domain: records, validation, and the canonical codec.
//! Deliberately language-neutral; transport, key custody, and encryption stay in Core.
//!
//! A Collection, Document, or Comment is identified by the `RecordId` of the record that
//! created it, so a record that targets one carries that 64-character identity.

use std::fmt;

const MAGIC: &[u8] = b"TDOCS\x01";
pub(crate) const MAX_NAME: usize = 128;
pub(crate) const MAX_TITLE: usize = 256;
/// The largest Loro update one edit record carries, leaving headroom under the 262,144-byte
/// canonical bound for the envelope, author, and target.
pub(crate) const MAX_UPDATE: usize = 200 * 1024;
/// A Comment's Markdown is bounded like a Conversations Message.
pub(crate) const MAX_MARKDOWN: usize = 32 * 1024;
/// A quoted excerpt is short context, not a copy of the document.
pub(crate) const MAX_QUOTE: usize = 2 * 1024;
const TARGET_LEN: usize = 64;

/// The record vocabulary. Kind numbers are part of the canonical encoding and are never
/// reused: 10 is reserved for `document.edit`, 20–22 for Comments, 30 for `document.snapshot`.
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
    DocumentCreate {
        author: String,
        collection: String,
        title: String,
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
    /// One Loro update for a Document's body, exported since the author's previous edit.
    DocumentEdit {
        author: String,
        document: String,
        update: Vec<u8>,
    },
    /// A Comment on a Document, optionally replying to a parent Comment and quoting an excerpt.
    CommentAdd {
        author: String,
        document: String,
        parent: Option<String>,
        quote: Option<String>,
        markdown: String,
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
    InvalidMarkdown,
    InvalidQuote,
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
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(collection)?;
        validate_text(title, MAX_TITLE, DomainError::InvalidTitle)?;
        Ok(Self::DocumentCreate {
            author: author.into(),
            collection: collection.into(),
            title: title.into(),
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

    pub fn document_edit(
        author: &str,
        document: &str,
        update: Vec<u8>,
    ) -> Result<Self, DomainError> {
        validate_identity(author)?;
        validate_target(document)?;
        if update.is_empty() || update.len() > MAX_UPDATE {
            return Err(DomainError::InvalidUpdate);
        }
        Ok(Self::DocumentEdit {
            author: author.into(),
            document: document.into(),
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
        validate_identity(author)?;
        validate_target(document)?;
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
            | Self::DocumentEdit { author, .. }
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
            | Self::DocumentEdit { document, .. }
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
            Self::DocumentEdit { .. } => "document.edit",
            Self::CommentAdd { .. } => "comment.add",
            Self::CommentResolve { .. } => "comment.resolve",
            Self::CommentReopen { .. } => "comment.reopen",
        }
    }

    /// The purpose of body edit records, which the listing never needs to open.
    pub const EDIT_KIND: &'static str = "document.edit";

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
            } => {
                out.push(5);
                put_text(&mut out, author);
                put_text(&mut out, collection);
                put_text(&mut out, title);
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
            Self::DocumentEdit {
                author,
                document,
                update,
            } => {
                out.push(10);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_bytes(&mut out, update);
            }
            Self::CommentAdd {
                author,
                document,
                parent,
                quote,
                markdown,
            } => {
                out.push(20);
                put_text(&mut out, author);
                put_text(&mut out, document);
                put_text(&mut out, parent.as_deref().unwrap_or(""));
                put_text(&mut out, quote.as_deref().unwrap_or(""));
                put_text(&mut out, markdown);
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
                Self::document_create(&author, &collection, &title)?
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
            10 => {
                let document = take_text(bytes, &mut p)?;
                let update = take_bytes(bytes, &mut p)?;
                Self::document_edit(&author, &document, update)?
            }
            20 => {
                let document = take_text(bytes, &mut p)?;
                let parent = take_text(bytes, &mut p)?;
                let quote = take_text(bytes, &mut p)?;
                let markdown = take_text(bytes, &mut p)?;
                Self::comment_add(
                    &author,
                    &document,
                    (!parent.is_empty()).then_some(parent.as_str()),
                    (!quote.is_empty()).then_some(quote.as_str()),
                    &markdown,
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

/// A name or title: 1 to `max` UTF-8 bytes with no control characters and no leading or
/// trailing whitespace, which is the UI contract's grammar.
fn validate_text(v: &str, max: usize, error: DomainError) -> Result<(), DomainError> {
    if v.is_empty() || v.len() > max || v.chars().any(char::is_control) || v.trim() != v {
        Err(error)
    } else {
        Ok(())
    }
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
    fn every_kind_round_trips() {
        let records = [
            DomainRecord::collection_create("alice", "Planning").unwrap(),
            DomainRecord::collection_rename("alice", TARGET, "Plans").unwrap(),
            DomainRecord::collection_archive("alice", TARGET).unwrap(),
            DomainRecord::collection_restore("alice", TARGET).unwrap(),
            DomainRecord::document_create("alice", TARGET, "Q4 plan").unwrap(),
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
        let edit = DomainRecord::document_edit("alice", TARGET, vec![0, 255, 7]).unwrap();
        assert_eq!(DomainRecord::decode(&edit.encode()).unwrap(), edit);
        assert_eq!(edit.kind(), DomainRecord::EDIT_KIND);
        assert!(!DomainRecord::METADATA_KINDS.contains(&edit.kind()));
        assert!(DomainRecord::document_edit("alice", TARGET, Vec::new()).is_err());
        assert!(DomainRecord::document_edit("alice", TARGET, vec![1; MAX_UPDATE + 1]).is_err());
    }

    #[test]
    fn bounds_and_grammar_are_enforced() {
        assert!(DomainRecord::collection_create("alice", "").is_err());
        assert!(DomainRecord::collection_create("alice", " padded").is_err());
        assert!(DomainRecord::collection_create("alice", "tab\there").is_err());
        assert!(DomainRecord::collection_create("alice", &"n".repeat(MAX_NAME + 1)).is_err());
        assert!(
            DomainRecord::document_create("alice", TARGET, &"t".repeat(MAX_TITLE + 1)).is_err()
        );
        assert!(DomainRecord::document_create("alice", "not-a-target", "t").is_err());
        assert!(DomainRecord::document_create("alice", &TARGET.to_uppercase(), "t").is_err());
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
