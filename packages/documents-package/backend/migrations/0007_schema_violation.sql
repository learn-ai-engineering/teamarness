-- A remote record that imports but leaves a tree the shared body schema does not allow is an
-- import failure of its own sanitized category. SQLite cannot widen a CHECK in place.
CREATE TABLE document_import_failures_v7 (
    document_id TEXT PRIMARY KEY,
    record_id TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('incompatible_encoding','decode_or_checksum','schema_violation','other'))
);
INSERT INTO document_import_failures_v7(document_id,record_id,category)
    SELECT document_id,record_id,category FROM document_import_failures;
DROP TABLE document_import_failures;
ALTER TABLE document_import_failures_v7 RENAME TO document_import_failures;
