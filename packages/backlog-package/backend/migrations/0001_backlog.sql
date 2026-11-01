PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- Every sealed record this Installation has received. Titles and bodies stay sealed here and
-- in Backend memory only (ADR 0027, ADR 0039).
CREATE TABLE records (
    record_id TEXT PRIMARY KEY,
    order_key TEXT NOT NULL,
    author TEXT NOT NULL,
    purpose TEXT NOT NULL,
    signature TEXT NOT NULL,
    sealed_record BLOB NOT NULL,
    cursor TEXT,
    ingestion_ordinal INTEGER NOT NULL,
    quarantine_code TEXT
);
CREATE INDEX records_purpose ON records (purpose);
CREATE INDEX records_order ON records (order_key);

-- An Epic is identified by the record that created it. Its title is plaintext and stays in
-- memory; only its ordering, archive state and creation live here.
CREATE TABLE epics (
    epic_id TEXT PRIMARY KEY,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    create_at INTEGER NOT NULL,
    title_order TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1)),
    archive_order TEXT NOT NULL
);

-- A Ticket is identified by the record that created it. Its title and body are plaintext and
-- stay in memory. Each field converges on its own `*_order`; `version` is the greatest Order
-- Key applied to the Ticket, and `last_editor`/`last_at` come from that record.
CREATE TABLE tickets (
    ticket_id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL,
    epic_order TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('not_ready', 'ready', 'in_progress', 'done')),
    status_order TEXT NOT NULL,
    priority TEXT NOT NULL CHECK (priority IN ('high', 'medium', 'low')),
    priority_order TEXT NOT NULL,
    assignee TEXT,
    assignee_order TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1)),
    archive_order TEXT NOT NULL,
    title_order TEXT NOT NULL,
    body_order TEXT NOT NULL,
    version TEXT NOT NULL,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    create_at INTEGER NOT NULL,
    last_editor TEXT NOT NULL,
    last_at INTEGER NOT NULL
);
CREATE INDEX tickets_epic ON tickets (epic_id, create_order);

-- A record whose Epic or Ticket has not arrived yet. It is applied when the target arrives.
CREATE TABLE waiting_for_target (
    record_id TEXT PRIMARY KEY REFERENCES records (record_id),
    target_id TEXT NOT NULL
);
CREATE INDEX waiting_for_target_target ON waiting_for_target (target_id);

CREATE TABLE metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    cursor TEXT,
    next_ingestion_ordinal INTEGER NOT NULL
);
INSERT INTO metadata (singleton, cursor, next_ingestion_ordinal) VALUES (1, NULL, 0);
