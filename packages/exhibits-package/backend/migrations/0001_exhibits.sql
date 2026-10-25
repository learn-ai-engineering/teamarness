-- Exhibit Drafts and their file lists. File bytes live in the content store
-- (`content/sha256/<hex>`), keyed by SHA-256 and shared by every Draft.
CREATE TABLE drafts (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    entry TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- The Host-derived actor of the latest update: 'person' or 'agent', and its key.
    updated_by_kind TEXT NOT NULL CHECK (updated_by_kind IN ('person', 'agent')),
    updated_by TEXT NOT NULL
) STRICT;

CREATE INDEX drafts_newest_first ON drafts (updated_at DESC, id DESC);

CREATE TABLE draft_files (
    draft_id TEXT NOT NULL REFERENCES drafts (id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    PRIMARY KEY (draft_id, path)
) STRICT;

CREATE INDEX draft_files_by_content ON draft_files (sha256);

-- One row per complete file in the content store. `released_at` is when the
-- last Draft stopped referencing it (or when it was stored, before any Draft
-- referenced it); NULL while a Draft references it. Garbage collection deletes
-- files released for longer than the grace period.
CREATE TABLE content (
    sha256 TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    stored_at INTEGER NOT NULL,
    released_at INTEGER
) STRICT;

-- One row per partial upload in `content/partial/<hex>/`. `touched_at` is the
-- last chunk received; garbage collection deletes uploads idle for longer than
-- the grace period.
CREATE TABLE partials (
    sha256 TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    chunk_count INTEGER NOT NULL CHECK (chunk_count > 0),
    started_at INTEGER NOT NULL,
    touched_at INTEGER NOT NULL
) STRICT;
