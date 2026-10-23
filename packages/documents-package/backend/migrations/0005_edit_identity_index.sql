-- Only opaque request provenance is projected. Canonical edits and their digest stay sealed.
CREATE TABLE edit_identities (
    request_id TEXT PRIMARY KEY,
    record_id TEXT NOT NULL UNIQUE,
    document_id TEXT NOT NULL,
    author TEXT NOT NULL
);
-- Monotonic document-scoped safety state; neither colliding edit is a winner.
CREATE TABLE document_identity_conflicts (
    document_id TEXT PRIMARY KEY
);
