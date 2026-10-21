PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

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

-- A Collection is identified by the record that created it. Its name is plaintext and
-- stays in memory; only ordering and archive state live here.
CREATE TABLE collections (
    collection_id TEXT PRIMARY KEY,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    state_order TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1))
);

-- A Document is identified by the record that created it. Its title is plaintext and
-- stays in memory; Collection membership, ordering, and archive state live here.
CREATE TABLE documents (
    document_id TEXT PRIMARY KEY,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    collection_id TEXT NOT NULL,
    move_order TEXT NOT NULL,
    state_order TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1))
);
CREATE INDEX documents_collection ON documents (collection_id, create_order);

CREATE TABLE metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    cursor TEXT,
    next_ingestion_ordinal INTEGER NOT NULL
);
INSERT INTO metadata (singleton, cursor, next_ingestion_ordinal) VALUES (1, NULL, 0);
