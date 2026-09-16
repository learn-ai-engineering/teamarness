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

CREATE TABLE channels (
    channel_id TEXT PRIMARY KEY,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    hidden_order TEXT
);

CREATE TABLE messages (
    message_id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    order_key TEXT NOT NULL,
    author TEXT NOT NULL,
    record_id TEXT NOT NULL,
    ingestion_ordinal INTEGER NOT NULL
);
CREATE INDEX messages_channel_order ON messages (channel_id, order_key DESC);

CREATE TABLE unread (
    channel_id TEXT PRIMARY KEY,
    last_read_ingestion_ordinal INTEGER NOT NULL
);

CREATE TABLE metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    cursor TEXT,
    next_ingestion_ordinal INTEGER NOT NULL
);
INSERT INTO metadata (singleton, cursor, next_ingestion_ordinal) VALUES (1, NULL, 0);