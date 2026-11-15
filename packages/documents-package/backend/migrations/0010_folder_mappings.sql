-- Folder sync state. It belongs to this Installation only and is never replicated.

-- One mapped folder per Collection. `state` is `active`, `unavailable` or `paused`.
CREATE TABLE folder_mappings (
    collection_id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- The file of one mapped Document. `name_key` is the case-folded file name, unique per mapping.
-- `base_version` and `written_hash` name the version and bytes the file was last synced at;
-- the pending pair is set before a write-back renames the file and cleared after.
CREATE TABLE mapped_files (
    document_id TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES folder_mappings ON DELETE CASCADE,
    rel_path TEXT NOT NULL,
    name_key TEXT NOT NULL,
    base_version TEXT,
    written_hash BLOB,
    pending_write_hash BLOB,
    pending_write_version TEXT,
    UNIQUE (collection_id, name_key)
);

-- A file folder sync refused, which may have no Document. `refused_hash` stops a retry of the
-- same bytes.
CREATE TABLE refused_files (
    collection_id TEXT NOT NULL REFERENCES folder_mappings ON DELETE CASCADE,
    rel_path TEXT NOT NULL,
    refused_hash BLOB,
    reason TEXT NOT NULL,
    detail TEXT,
    PRIMARY KEY (collection_id, rel_path)
);
