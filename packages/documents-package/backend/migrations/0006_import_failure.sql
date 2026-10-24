-- An opaque, monotonic Document-local import disposition. Sealed records stay in records.
CREATE TABLE document_import_failures (
    document_id TEXT PRIMARY KEY,
    record_id TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('incompatible_encoding','decode_or_checksum','other'))
);
