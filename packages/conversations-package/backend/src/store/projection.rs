use rusqlite::{OptionalExtension, params};

use crate::domain::{DomainRecord, GENERAL_ID, hex_id, quarantine};
use crate::store::IngestResult;

pub(crate) struct ProjectionOutcome {
    pub(crate) result: IngestResult,
    pub(crate) slug: Option<(String, String)>,
    pub(crate) markdown: Option<(String, String)>,
    pub(crate) deferred: Option<DomainRecord>,
    pub(crate) retry_channel: Option<String>,
    pub(crate) removed_messages: Vec<String>,
}

impl ProjectionOutcome {
    pub(crate) fn applied() -> Self {
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

pub(crate) fn project_domain(
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
