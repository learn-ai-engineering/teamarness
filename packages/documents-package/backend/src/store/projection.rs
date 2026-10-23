use rusqlite::{OptionalExtension, params};

use crate::domain::{DomainRecord, quarantine};
use crate::store::IngestResult;

/// The quarantine code of a record that waits for the record it targets.
pub(crate) const WAITING: &str = "waiting_for_target";

/// What one projected record changed outside SQLite, applied by the store after commit.
pub(crate) struct ProjectionOutcome {
    pub(crate) result: IngestResult,
    /// A Collection name or Document title to remember, with the order that set it.
    pub(crate) text: Option<(String, String, String)>,
    pub(crate) deferred: Option<DomainRecord>,
    /// A newly created target whose deferred dependants can now be retried.
    pub(crate) created: Option<String>,
    /// A body update to import, with the Document it belongs to.
    pub(crate) edit: Option<(String, Vec<u8>)>,
    /// A Comment's Markdown and optional quote to remember.
    pub(crate) comment: Option<(String, String, Option<String>)>,
}

impl ProjectionOutcome {
    pub(crate) fn applied() -> Self {
        Self {
            result: IngestResult::Applied,
            text: None,
            deferred: None,
            created: None,
            edit: None,
            comment: None,
        }
    }
    fn quarantined(
        tx: &rusqlite::Transaction<'_>,
        record_id: &str,
        code: &str,
    ) -> Result<Self, rusqlite::Error> {
        quarantine(tx, record_id, code)?;
        Ok(Self {
            result: IngestResult::Quarantined(code.into()),
            ..Self::applied()
        })
    }
    fn deferred(
        tx: &rusqlite::Transaction<'_>,
        record_id: &str,
        domain: &DomainRecord,
    ) -> Result<Self, rusqlite::Error> {
        quarantine(tx, record_id, WAITING)?;
        Ok(Self {
            result: IngestResult::Deferred(WAITING.into()),
            deferred: Some(domain.clone()),
            ..Self::applied()
        })
    }
}

fn collection_exists(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<bool, rusqlite::Error> {
    tx.query_row(
        "SELECT 1 FROM collections WHERE collection_id=?1",
        [id],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}
fn document_exists(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<bool, rusqlite::Error> {
    tx.query_row("SELECT 1 FROM documents WHERE document_id=?1", [id], |_| {
        Ok(())
    })
    .optional()
    .map(|found| found.is_some())
}
fn comment_exists(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<bool, rusqlite::Error> {
    tx.query_row("SELECT 1 FROM comments WHERE comment_id=?1", [id], |_| {
        Ok(())
    })
    .optional()
    .map(|found| found.is_some())
}
fn clear(tx: &rusqlite::Transaction<'_>, record_id: &str) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE records SET quarantine_code=NULL WHERE record_id=?1",
        [record_id],
    )
    .map(|_| ())
}

/// Projects one decoded record inside the caller's transaction.
///
/// Create records establish identity: the Collection or Document id is the record's own id.
/// Every other record is last-writer-wins by `order_key` on the state it changes, so a late
/// earlier-sorting record never overrides a later one and arrival order does not matter.
pub(crate) fn project_domain(
    tx: &rusqlite::Transaction<'_>,
    record_id: &str,
    order_key: &str,
    author: &str,
    domain: &DomainRecord,
) -> Result<ProjectionOutcome, rusqlite::Error> {
    let stored_purpose: String = tx.query_row(
        "SELECT purpose FROM records WHERE record_id=?1",
        [record_id],
        |row| row.get(0),
    )?;
    if domain.kind() != stored_purpose {
        return ProjectionOutcome::quarantined(tx, record_id, "purpose_mismatch");
    }
    if domain.author() != author {
        return ProjectionOutcome::quarantined(tx, record_id, "signer_mismatch");
    }
    match domain {
        DomainRecord::CollectionCreate { author, name } => {
            tx.execute(
                "INSERT OR IGNORE INTO collections(collection_id,creator,create_order,state_order,archived) VALUES(?1,?2,?3,?3,0)",
                params![record_id, author, order_key],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                text: Some((record_id.into(), order_key.into(), name.clone())),
                created: Some(record_id.into()),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::CollectionRename {
            collection, name, ..
        } => {
            if !collection_exists(tx, collection)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                text: Some((collection.clone(), order_key.into(), name.clone())),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::CollectionArchive { collection, .. }
        | DomainRecord::CollectionRestore { collection, .. } => {
            if !collection_exists(tx, collection)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            let archived = matches!(domain, DomainRecord::CollectionArchive { .. });
            tx.execute(
                "UPDATE collections SET archived=?2,state_order=?3 WHERE collection_id=?1 AND state_order<?3",
                params![collection, archived, order_key],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome::applied())
        }
        DomainRecord::DocumentCreate {
            author,
            collection,
            title,
        } => {
            if !collection_exists(tx, collection)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            tx.execute(
                "INSERT OR IGNORE INTO documents(document_id,creator,create_order,collection_id,move_order,state_order,archived) VALUES(?1,?2,?3,?4,?3,?3,0)",
                params![record_id, author, order_key, collection],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                text: Some((record_id.into(), order_key.into(), title.clone())),
                created: Some(record_id.into()),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::DocumentRename {
            document, title, ..
        } => {
            if !document_exists(tx, document)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                text: Some((document.clone(), order_key.into(), title.clone())),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::DocumentMove {
            document,
            collection,
            ..
        } => {
            if !document_exists(tx, document)? || !collection_exists(tx, collection)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            tx.execute(
                "UPDATE documents SET collection_id=?2,move_order=?3 WHERE document_id=?1 AND move_order<?3",
                params![document, collection, order_key],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome::applied())
        }
        DomainRecord::DocumentArchive { document, .. }
        | DomainRecord::DocumentRestore { document, .. } => {
            if !document_exists(tx, document)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            let archived = matches!(domain, DomainRecord::DocumentArchive { .. });
            tx.execute(
                "UPDATE documents SET archived=?2,state_order=?3 WHERE document_id=?1 AND state_order<?3",
                params![document, archived, order_key],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome::applied())
        }
        DomainRecord::DocumentEdit {
            document, update, ..
        }
        | DomainRecord::IdentifiedDocumentEdit {
            document, update, ..
        } => {
            tx.execute(
                "UPDATE records SET target=?2 WHERE record_id=?1",
                params![record_id, document],
            )?;
            let conflicted: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM document_identity_conflicts WHERE document_id=?1)",
                [document],
                |row| row.get(0),
            )?;
            if conflicted {
                return ProjectionOutcome::quarantined(tx, record_id, "identity_conflict");
            }
            if !document_exists(tx, document)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            let failed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM document_import_failures WHERE document_id=?1)",
                [document],
                |row| row.get(0),
            )?;
            if failed {
                return ProjectionOutcome::quarantined(tx, record_id, "import_failure");
            }
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                edit: Some((document.clone(), update.clone())),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::CommentAdd {
            author,
            document,
            parent,
            quote,
            markdown,
            created_at,
        } => {
            if !document_exists(tx, document)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            if let Some(parent) = parent {
                if !comment_exists(tx, parent)? {
                    return ProjectionOutcome::deferred(tx, record_id, domain);
                }
                let parent_document: String = tx.query_row(
                    "SELECT document_id FROM comments WHERE comment_id=?1",
                    [parent],
                    |row| row.get(0),
                )?;
                if parent_document != *document {
                    return ProjectionOutcome::quarantined(
                        tx,
                        record_id,
                        "parent_on_other_document",
                    );
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO comments(comment_id,document_id,parent_id,author,create_order,state_order,resolved,created_at) VALUES(?1,?2,?3,?4,?5,?5,0,?6)",
                params![record_id, document, parent, author, order_key, created_at],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome {
                comment: Some((record_id.into(), markdown.clone(), quote.clone())),
                created: Some(record_id.into()),
                ..ProjectionOutcome::applied()
            })
        }
        DomainRecord::CommentResolve { comment, .. }
        | DomainRecord::CommentReopen { comment, .. } => {
            if !comment_exists(tx, comment)? {
                return ProjectionOutcome::deferred(tx, record_id, domain);
            }
            let resolved = matches!(domain, DomainRecord::CommentResolve { .. });
            tx.execute(
                "UPDATE comments SET resolved=?2,state_order=?3 WHERE comment_id=?1 AND state_order<?3",
                params![comment, resolved, order_key],
            )?;
            clear(tx, record_id)?;
            Ok(ProjectionOutcome::applied())
        }
    }
}
