//! The Backlog's durable projection: sealed records, ordering, enums, archive state and
//! attribution in SQLite; titles and bodies only in memory (ADR 0027, ADR 0039).

pub(crate) mod migration;
pub mod projection;

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::{DomainRecord, Priority, Status};
use projection::{EpicState, Outcome, Projection, TicketState, Touched, WAITING};

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

/// One record as the record log delivered it, with its opened canonical bytes.
#[derive(Debug, Clone, Copy)]
pub struct Incoming<'a> {
    pub record_id: &'a str,
    pub order_key: &'a str,
    /// The record's signer.
    pub author: &'a str,
    pub purpose: &'a str,
    pub signature: &'a str,
    pub canonical: &'a [u8],
    pub sealed: &'a [u8],
    pub cursor: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestResult {
    Applied,
    Duplicate,
    /// Waits for the Epic or Ticket named here.
    Waiting(String),
    Quarantined(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicView {
    pub id: String,
    /// `None` until the create or latest rename record has been opened.
    pub title: Option<String>,
    pub archived: bool,
    pub creator: String,
    /// The Order Key of the create record, which breaks title ties.
    pub create_order: String,
    pub create_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketView {
    pub id: String,
    pub epic_id: String,
    /// `None` until the record that set it has been opened.
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Status,
    pub priority: Priority,
    pub assignee: Option<String>,
    pub archived: bool,
    /// The greatest Order Key applied to the Ticket: the stale-save token.
    pub version: String,
    pub creator: String,
    pub create_order: String,
    pub create_at: u64,
    pub last_editor: String,
    pub last_at: u64,
}

#[derive(Debug, Clone)]
pub struct BacklogStore {
    inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
    conn: Connection,
    projection: Projection,
}

fn rerr(_error: rusqlite::Error) -> StoreError {
    StoreError::Storage
}

impl BacklogStore {
    /// Opens or creates the store and loads the persisted projection. Titles and bodies start
    /// empty; they come back as records are opened.
    ///
    /// # Errors
    ///
    /// Storage or migration failure, or a schema newer than this Package.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut conn = Connection::open(path.as_ref()).map_err(|_| StoreError::Storage)?;
        migration::apply(&mut conn)?;
        let mut projection = Projection::default();
        load(&conn, &mut projection).map_err(rerr)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(StoreInner { conn, projection })),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, StoreInner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Lock)
    }

    /// The committed delivery cursor.
    ///
    /// # Errors
    ///
    /// Storage failure.
    pub fn cursor(&self) -> Result<Option<String>, StoreError> {
        self.lock()?
            .conn
            .query_row("SELECT cursor FROM metadata WHERE singleton=1", [], |row| {
                row.get(0)
            })
            .map_err(rerr)
    }

    /// Stores one delivered record and projects it.
    ///
    /// A record already stored with identical fields is a duplicate. A malformed record, one
    /// whose purpose is not its kind, or one whose author is not its signer is kept sealed and
    /// quarantined. A record whose target is unknown waits.
    ///
    /// # Errors
    ///
    /// Storage failure; nothing was committed and the in-memory projection is reloaded.
    pub fn ingest(&self, incoming: Incoming<'_>) -> Result<IngestResult, StoreError> {
        let mut guard = self.lock()?;
        let inner = &mut *guard;
        let result = ingest_in(&mut inner.conn, &mut inner.projection, incoming);
        if result.is_err() {
            // The projection may hold changes SQLite did not commit; restore the committed
            // state. Remembered text only ever moves forward by order, so it is kept.
            let texts = inner.projection.texts().clone();
            let mut reloaded = Projection::default();
            if load(&inner.conn, &mut reloaded).is_ok() {
                *reloaded.texts_mut() = texts;
                inner.projection = reloaded;
            }
        }
        result
    }

    /// Every Epic, by id.
    ///
    /// # Errors
    ///
    /// The store lock is poisoned.
    pub fn epics(&self) -> Result<Vec<EpicView>, StoreError> {
        let inner = self.lock()?;
        let projection = &inner.projection;
        Ok(projection
            .epics()
            .map(|(id, epic)| EpicView {
                id: id.clone(),
                title: projection.texts().title(id).map(str::to_owned),
                archived: epic.archived,
                creator: epic.creator.clone(),
                create_order: epic.create_order.clone(),
                create_at: epic.create_at,
            })
            .collect())
    }

    /// Every Ticket, by id.
    ///
    /// # Errors
    ///
    /// The store lock is poisoned.
    pub fn tickets(&self) -> Result<Vec<TicketView>, StoreError> {
        let inner = self.lock()?;
        let projection = &inner.projection;
        Ok(projection
            .tickets()
            .map(|(id, ticket)| ticket_view(projection, id, ticket))
            .collect())
    }

    /// One Ticket.
    ///
    /// # Errors
    ///
    /// The store lock is poisoned.
    pub fn ticket(&self, id: &str) -> Result<Option<TicketView>, StoreError> {
        let inner = self.lock()?;
        let projection = &inner.projection;
        Ok(projection
            .ticket(id)
            .map(|ticket| ticket_view(projection, id, ticket)))
    }

    /// Record ids waiting for a target, with that target, from SQLite.
    ///
    /// # Errors
    ///
    /// Storage failure.
    pub fn waiting(&self) -> Result<Vec<(String, String)>, StoreError> {
        let inner = self.lock()?;
        let mut statement = inner
            .conn
            .prepare("SELECT record_id,target_id FROM waiting_for_target ORDER BY record_id")
            .map_err(rerr)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(rerr)?;
        rows.collect::<Result<_, _>>().map_err(rerr)
    }
}

fn ticket_view(projection: &Projection, id: &str, ticket: &TicketState) -> TicketView {
    TicketView {
        id: id.to_owned(),
        epic_id: ticket.epic.clone(),
        title: projection.texts().title(id).map(str::to_owned),
        body: projection.texts().body(id).map(str::to_owned),
        status: ticket.status,
        priority: ticket.priority,
        assignee: ticket.assignee.clone(),
        archived: ticket.archived,
        version: ticket.version.clone(),
        creator: ticket.creator.clone(),
        create_order: ticket.create_order.clone(),
        create_at: ticket.create_at,
        last_editor: ticket.last_editor.clone(),
        last_at: ticket.last_at,
    }
}

fn ingest_in(
    conn: &mut Connection,
    projection: &mut Projection,
    incoming: Incoming<'_>,
) -> Result<IngestResult, StoreError> {
    let tx = conn.transaction().map_err(rerr)?;
    let existing: Option<(String, String, String, String, Vec<u8>)> = tx
        .query_row(
            "SELECT order_key,author,purpose,signature,sealed_record FROM records WHERE record_id=?1",
            [incoming.record_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(rerr)?;
    if let Some((order_key, author, purpose, signature, sealed)) = existing {
        let result = if order_key == incoming.order_key
            && author == incoming.author
            && purpose == incoming.purpose
            && signature == incoming.signature
            && sealed == incoming.sealed
        {
            IngestResult::Duplicate
        } else {
            IngestResult::Quarantined("immutable_field_mismatch".into())
        };
        if let Some(cursor) = incoming.cursor {
            tx.execute("UPDATE metadata SET cursor=?1 WHERE singleton=1", [cursor])
                .map_err(rerr)?;
        }
        tx.commit().map_err(|_| StoreError::UncertainOutcome)?;
        return Ok(result);
    }
    let ordinal: i64 = tx
        .query_row(
            "SELECT next_ingestion_ordinal FROM metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(rerr)?;
    let ordinal = ordinal + 1;
    tx.execute(
        "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)",
        params![
            incoming.record_id,
            incoming.order_key,
            incoming.author,
            incoming.purpose,
            incoming.signature,
            incoming.sealed,
            incoming.cursor,
            ordinal
        ],
    )
    .map_err(rerr)?;
    let result = match DomainRecord::decode(incoming.canonical) {
        Err(_) => quarantine(&tx, incoming.record_id, "malformed_record")?,
        Ok(record) if record.kind() != incoming.purpose => {
            quarantine(&tx, incoming.record_id, "purpose_mismatch")?
        }
        Ok(record) => match projection.apply(
            incoming.record_id,
            incoming.order_key,
            incoming.author,
            &record,
        ) {
            Outcome::Quarantined(code) => quarantine(&tx, incoming.record_id, code)?,
            Outcome::Waiting { target } => {
                wait(&tx, incoming.record_id, &target)?;
                IngestResult::Waiting(target)
            }
            Outcome::Applied {
                touched,
                released,
                rewaiting,
            } => {
                for entity in &touched {
                    persist(&tx, projection, entity).map_err(rerr)?;
                }
                for id in &released {
                    tx.execute("DELETE FROM waiting_for_target WHERE record_id=?1", [id])
                        .map_err(rerr)?;
                    tx.execute(
                        "UPDATE records SET quarantine_code=NULL WHERE record_id=?1",
                        [id],
                    )
                    .map_err(rerr)?;
                }
                for (id, target) in &rewaiting {
                    wait(&tx, id, target)?;
                }
                IngestResult::Applied
            }
        },
    };
    tx.execute(
        "UPDATE metadata SET cursor=COALESCE(?1,cursor),next_ingestion_ordinal=?2 WHERE singleton=1",
        params![incoming.cursor, ordinal],
    )
    .map_err(rerr)?;
    tx.commit().map_err(|_| StoreError::UncertainOutcome)?;
    Ok(result)
}

fn quarantine(tx: &Transaction<'_>, id: &str, code: &str) -> Result<IngestResult, StoreError> {
    tx.execute(
        "UPDATE records SET quarantine_code=?2 WHERE record_id=?1",
        params![id, code],
    )
    .map_err(rerr)?;
    Ok(IngestResult::Quarantined(code.to_owned()))
}

fn wait(tx: &Transaction<'_>, id: &str, target: &str) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE records SET quarantine_code=?2 WHERE record_id=?1",
        params![id, WAITING],
    )
    .map_err(rerr)?;
    tx.execute(
        "INSERT OR REPLACE INTO waiting_for_target(record_id,target_id) VALUES(?1,?2)",
        params![id, target],
    )
    .map_err(rerr)?;
    Ok(())
}

/// Writes one Epic's or Ticket's non-text state.
fn persist(
    tx: &Transaction<'_>,
    projection: &Projection,
    entity: &Touched,
) -> Result<(), rusqlite::Error> {
    match entity {
        Touched::Epic(id) => {
            let Some(epic) = projection.epic(id) else {
                return Ok(());
            };
            tx.execute(
                "INSERT OR REPLACE INTO epics(epic_id,creator,create_order,create_at,title_order,archived,archive_order) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    epic.creator,
                    epic.create_order,
                    to_sql_time(epic.create_at),
                    epic.title_order,
                    epic.archived,
                    epic.archive_order
                ],
            )?;
        }
        Touched::Ticket(id) => {
            let Some(ticket) = projection.ticket(id) else {
                return Ok(());
            };
            tx.execute(
                "INSERT OR REPLACE INTO tickets(ticket_id,epic_id,epic_order,status,status_order,priority,priority_order,assignee,assignee_order,archived,archive_order,title_order,body_order,version,creator,create_order,create_at,last_editor,last_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![
                    id,
                    ticket.epic,
                    ticket.epic_order,
                    ticket.status.as_str(),
                    ticket.status_order,
                    ticket.priority.as_str(),
                    ticket.priority_order,
                    ticket.assignee,
                    ticket.assignee_order,
                    ticket.archived,
                    ticket.archive_order,
                    ticket.title_order,
                    ticket.body_order,
                    ticket.version,
                    ticket.creator,
                    ticket.create_order,
                    to_sql_time(ticket.create_at),
                    ticket.last_editor,
                    to_sql_time(ticket.last_at)
                ],
            )?;
        }
    }
    Ok(())
}

/// Author-clock times are validated to the year 9999, well inside `i64`.
fn to_sql_time(at: u64) -> i64 {
    i64::try_from(at).unwrap_or(i64::MAX)
}

fn from_sql_time(at: i64) -> u64 {
    u64::try_from(at).unwrap_or(0)
}

fn enum_error() -> rusqlite::Error {
    rusqlite::Error::InvalidQuery
}

/// Loads the persisted Epics and Tickets.
fn load(conn: &Connection, projection: &mut Projection) -> Result<(), rusqlite::Error> {
    let mut epics = conn.prepare(
        "SELECT epic_id,creator,create_order,create_at,title_order,archived,archive_order FROM epics",
    )?;
    let rows = epics.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            EpicState {
                creator: row.get(1)?,
                create_order: row.get(2)?,
                create_at: from_sql_time(row.get(3)?),
                title_order: row.get(4)?,
                archived: row.get(5)?,
                archive_order: row.get(6)?,
            },
        ))
    })?;
    for row in rows {
        let (id, epic) = row?;
        projection.restore_epic(id, epic);
    }
    let mut tickets = conn.prepare(
        "SELECT ticket_id,epic_id,epic_order,status,status_order,priority,priority_order,assignee,assignee_order,archived,archive_order,title_order,body_order,version,creator,create_order,create_at,last_editor,last_at FROM tickets",
    )?;
    let rows = tickets.query_map([], |row| {
        let status: String = row.get(3)?;
        let priority: String = row.get(5)?;
        Ok((
            row.get::<_, String>(0)?,
            TicketState {
                epic: row.get(1)?,
                epic_order: row.get(2)?,
                status: Status::parse(&status).map_err(|_| enum_error())?,
                status_order: row.get(4)?,
                priority: Priority::parse(&priority).map_err(|_| enum_error())?,
                priority_order: row.get(6)?,
                assignee: row.get(7)?,
                assignee_order: row.get(8)?,
                archived: row.get(9)?,
                archive_order: row.get(10)?,
                title_order: row.get(11)?,
                body_order: row.get(12)?,
                version: row.get(13)?,
                creator: row.get(14)?,
                create_order: row.get(15)?,
                create_at: from_sql_time(row.get(16)?),
                last_editor: row.get(17)?,
                last_at: from_sql_time(row.get(18)?),
            },
        ))
    })?;
    for row in rows {
        let (id, ticket) = row?;
        projection.restore_ticket(id, ticket);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TicketChanges;

    const ALICE: &str = "alice";
    const BOB: &str = "bob";
    const SECRET_TITLE: &str = "Quarterly plaintext marker";
    const SECRET_BODY: &str = "Body plaintext marker";

    fn id(seed: u64) -> String {
        format!("{seed:064x}")
    }

    fn order(seed: u64) -> String {
        format!("{seed:082x}")
    }

    struct Fixture {
        store: BacklogStore,
        path: std::path::PathBuf,
        _directory: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backlog.sqlite3");
        Fixture {
            store: BacklogStore::open(&path).unwrap(),
            path,
            _directory: directory,
        }
    }

    fn ingest(
        store: &BacklogStore,
        seed: u64,
        signer: &str,
        record: &DomainRecord,
    ) -> IngestResult {
        let record_id = id(seed);
        let order_key = order(seed);
        let cursor = format!("c{seed}");
        let canonical = record.encode();
        store
            .ingest(Incoming {
                record_id: &record_id,
                order_key: &order_key,
                author: signer,
                purpose: record.kind(),
                signature: "sig",
                canonical: &canonical,
                // Sealed bytes are opaque to the store; these never hold plaintext.
                sealed: b"sealed",
                cursor: Some(&cursor),
            })
            .unwrap()
    }

    fn create_epic_and_ticket(store: &BacklogStore) {
        assert_eq!(
            ingest(
                store,
                1,
                ALICE,
                &DomainRecord::epic_create(ALICE, 10, "Launch").unwrap()
            ),
            IngestResult::Applied
        );
        assert_eq!(
            ingest(
                store,
                2,
                ALICE,
                &DomainRecord::ticket_create(
                    ALICE,
                    20,
                    &id(1),
                    SECRET_TITLE,
                    SECRET_BODY,
                    Status::Ready,
                    Priority::High,
                )
                .unwrap(),
            ),
            IngestResult::Applied
        );
    }

    #[test]
    fn ingested_records_project_epics_and_tickets() {
        let fixture = fixture();
        create_epic_and_ticket(&fixture.store);
        let epics = fixture.store.epics().unwrap();
        assert_eq!(epics.len(), 1);
        assert_eq!(epics[0].title.as_deref(), Some("Launch"));
        assert_eq!(epics[0].create_at, 10);
        let ticket = fixture.store.ticket(&id(2)).unwrap().unwrap();
        assert_eq!(ticket.epic_id, id(1));
        assert_eq!(ticket.title.as_deref(), Some(SECRET_TITLE));
        assert_eq!(ticket.body.as_deref(), Some(SECRET_BODY));
        assert_eq!(ticket.status, Status::Ready);
        assert_eq!(ticket.priority, Priority::High);
        assert_eq!(ticket.version, order(2));
        assert_eq!(ticket.creator, ALICE);
        assert_eq!(ticket.last_editor, ALICE);
        assert_eq!(fixture.store.cursor().unwrap().as_deref(), Some("c2"));
    }

    // Verifies: TEAM.SYS.BACKLOG-R10 — reopening keeps every field but text, which returns only
    // when records are opened again.
    #[test]
    fn reopening_restores_state_without_text() {
        let fixture = fixture();
        create_epic_and_ticket(&fixture.store);
        ingest(
            &fixture.store,
            3,
            BOB,
            &DomainRecord::ticket_assign(BOB, 30, &id(2), Some(&id(77))).unwrap(),
        );
        let before = fixture.store.ticket(&id(2)).unwrap().unwrap();
        drop(fixture.store);
        let reopened = BacklogStore::open(&fixture.path).unwrap();
        let after = reopened.ticket(&id(2)).unwrap().unwrap();
        assert_eq!(after.title, None);
        assert_eq!(after.body, None);
        assert_eq!(
            TicketView {
                title: before.title.clone(),
                body: before.body.clone(),
                ..after
            },
            before
        );
        assert_eq!(before.last_editor, BOB);
        assert_eq!(before.assignee, Some(id(77)));
        assert_eq!(reopened.epics().unwrap()[0].title, None);
    }

    #[test]
    fn duplicates_and_conflicting_replays_are_told_apart() {
        let fixture = fixture();
        create_epic_and_ticket(&fixture.store);
        let record = DomainRecord::epic_create(ALICE, 10, "Launch").unwrap();
        assert_eq!(
            ingest(&fixture.store, 1, ALICE, &record),
            IngestResult::Duplicate
        );
        let other = DomainRecord::epic_create(ALICE, 10, "Other").unwrap();
        let canonical = other.encode();
        let record_id = id(1);
        let result = fixture
            .store
            .ingest(Incoming {
                record_id: &record_id,
                order_key: &order(1),
                author: ALICE,
                purpose: other.kind(),
                signature: "different",
                canonical: &canonical,
                sealed: b"other",
                cursor: None,
            })
            .unwrap();
        assert_eq!(
            result,
            IngestResult::Quarantined("immutable_field_mismatch".into())
        );
        assert_eq!(
            fixture.store.epics().unwrap()[0].title.as_deref(),
            Some("Launch")
        );
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — a forged author is quarantined and never applied.
    #[test]
    fn signer_mismatch_malformed_and_purpose_mismatch_are_quarantined() {
        let fixture = fixture();
        create_epic_and_ticket(&fixture.store);
        let forged = DomainRecord::ticket_archive(ALICE, 30, &id(2)).unwrap();
        assert_eq!(
            ingest(&fixture.store, 3, BOB, &forged),
            IngestResult::Quarantined(projection::SIGNER_MISMATCH.into())
        );
        assert!(!fixture.store.ticket(&id(2)).unwrap().unwrap().archived);

        let malformed = fixture
            .store
            .ingest(Incoming {
                record_id: &id(4),
                order_key: &order(4),
                author: ALICE,
                purpose: "ticket.archive",
                signature: "sig",
                canonical: b"TBLOG\x01\x0e",
                sealed: b"sealed",
                cursor: None,
            })
            .unwrap();
        assert_eq!(
            malformed,
            IngestResult::Quarantined("malformed_record".into())
        );

        let archive = DomainRecord::ticket_archive(ALICE, 50, &id(2))
            .unwrap()
            .encode();
        let mislabeled = fixture
            .store
            .ingest(Incoming {
                record_id: &id(5),
                order_key: &order(5),
                author: ALICE,
                purpose: "ticket.restore",
                signature: "sig",
                canonical: &archive,
                sealed: b"sealed",
                cursor: None,
            })
            .unwrap();
        assert_eq!(
            mislabeled,
            IngestResult::Quarantined("purpose_mismatch".into())
        );
        let ticket = fixture.store.ticket(&id(2)).unwrap().unwrap();
        assert!(!ticket.archived);
        assert_eq!(ticket.version, order(2));
    }

    #[test]
    fn waiting_records_are_persisted_and_cleared_when_their_target_arrives() {
        let fixture = fixture();
        let update = DomainRecord::ticket_update(
            BOB,
            40,
            &id(2),
            TicketChanges {
                status: Some(Status::Done),
                ..TicketChanges::default()
            },
        )
        .unwrap();
        assert_eq!(
            ingest(&fixture.store, 4, BOB, &update),
            IngestResult::Waiting(id(2))
        );
        assert_eq!(fixture.store.waiting().unwrap(), vec![(id(4), id(2))]);
        create_epic_and_ticket(&fixture.store);
        assert!(fixture.store.waiting().unwrap().is_empty());
        let ticket = fixture.store.ticket(&id(2)).unwrap().unwrap();
        assert_eq!(ticket.status, Status::Done);
        assert_eq!(ticket.version, order(4));
        assert_eq!(ticket.last_editor, BOB);
        let inner = fixture.store.lock().unwrap();
        let code: Option<String> = inner
            .conn
            .query_row(
                "SELECT quarantine_code FROM records WHERE record_id=?1",
                [id(4)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(code, None);
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    // Verifies: TEAM.SYS.BACKLOG-R10 — no column holds a title or body, and no stored value
    // contains one.
    #[test]
    fn the_schema_holds_no_title_or_body_text() {
        let fixture = fixture();
        create_epic_and_ticket(&fixture.store);
        let inner = fixture.store.lock().unwrap();
        let mut statement = inner
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            tables.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "records",
                "epics",
                "tickets",
                "waiting_for_target",
                "metadata"
            ]
        );
        for table in &tables {
            for column in columns(&inner.conn, table) {
                assert!(
                    !matches!(
                        column.as_str(),
                        "title" | "body" | "name" | "text" | "markdown"
                    ),
                    "{table}.{column} would hold plaintext"
                );
            }
        }
        assert_eq!(
            columns(&inner.conn, "tickets")
                .into_iter()
                .filter(|column| column.ends_with("_order"))
                .collect::<Vec<_>>(),
            [
                "epic_order",
                "status_order",
                "priority_order",
                "assignee_order",
                "archive_order",
                "title_order",
                "body_order",
                "create_order"
            ]
        );
        for table in &tables {
            for column in columns(&inner.conn, table) {
                let mut values = inner
                    .conn
                    .prepare(&format!(
                        "SELECT CAST(\"{column}\" AS BLOB) FROM \"{table}\""
                    ))
                    .unwrap();
                let values: Vec<Option<Vec<u8>>> = values
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                for value in values.into_iter().flatten() {
                    let value = String::from_utf8_lossy(&value);
                    assert!(!value.contains(SECRET_TITLE), "{table}.{column}");
                    assert!(!value.contains(SECRET_BODY), "{table}.{column}");
                    assert!(!value.contains("Launch"), "{table}.{column}");
                }
            }
        }
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let fixture = fixture();
        drop(fixture.store);
        let conn = Connection::open(&fixture.path).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);
        assert!(matches!(
            BacklogStore::open(&fixture.path),
            Err(StoreError::UnsupportedSchema)
        ));
    }
}
