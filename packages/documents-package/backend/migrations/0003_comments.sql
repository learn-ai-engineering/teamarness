-- A Comment is identified by the record that created it and anchored to one Document, with an
-- optional parent Comment for threads. Its Markdown and quote are plaintext and stay in memory;
-- identity, threading, ordering, and resolved state live here.
CREATE TABLE comments (
    comment_id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL,
    parent_id TEXT,
    author TEXT NOT NULL,
    create_order TEXT NOT NULL,
    state_order TEXT NOT NULL,
    resolved INTEGER NOT NULL CHECK (resolved IN (0, 1))
);
CREATE INDEX comments_document ON comments (document_id, create_order);
