CREATE TABLE membership_sync_wakes (
    workspace_id BLOB PRIMARY KEY REFERENCES workspaces(workspace_id),
    wake_generation INTEGER NOT NULL CHECK (wake_generation >= 0),
    consumed_generation INTEGER NOT NULL DEFAULT 0 CHECK (consumed_generation >= 0)
) STRICT;

INSERT INTO membership_sync_wakes(workspace_id, wake_generation)
SELECT workspace_id, 1 FROM workspaces;

CREATE TABLE parked_operations (
    operation_hash BLOB PRIMARY KEY CHECK (length(operation_hash) = 32),
    scope_id BLOB NOT NULL CHECK (length(scope_id) = 32),
    workspace_id BLOB NOT NULL CHECK (length(workspace_id) = 16),
    package_identity TEXT NOT NULL,
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    author_sequence INTEGER NOT NULL CHECK (author_sequence >= 0),
    reason TEXT NOT NULL CHECK (reason IN ('unknown_author', 'missing_content_key')),
    received_from BLOB NOT NULL CHECK (length(received_from) = 32),
    operation_bytes BLOB NOT NULL CHECK (length(operation_bytes) BETWEEN 1 AND 1048576),
    parked_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX parked_operations_author ON parked_operations(workspace_id, author_public_identity, author_sequence);
CREATE INDEX parked_operations_scope ON parked_operations(scope_id, reason);
CREATE INDEX parked_operations_peer ON parked_operations(scope_id, received_from);

DROP TABLE pending_protection_records;
