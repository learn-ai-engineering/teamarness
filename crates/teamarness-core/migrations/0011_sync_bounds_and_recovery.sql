CREATE TABLE sync_peer_progress (
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    peer_public_identity BLOB NOT NULL CHECK (length(peer_public_identity) = 32),
    consumed_generation INTEGER NOT NULL CHECK (consumed_generation >= 0),
    PRIMARY KEY (scope_id, peer_public_identity)
) STRICT;
CREATE TABLE membership_peer_progress (
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    peer_public_identity BLOB NOT NULL CHECK (length(peer_public_identity) = 32),
    consumed_generation INTEGER NOT NULL CHECK (consumed_generation >= 0),
    PRIMARY KEY (workspace_id, peer_public_identity)
) STRICT;
CREATE TABLE scope_validation (
    scope_id BLOB PRIMARY KEY REFERENCES sync_scopes(scope_id),
    validated_through_sequence INTEGER NOT NULL CHECK (validated_through_sequence >= 0)
) STRICT;
CREATE TABLE quarantined_scopes (
    scope_id BLOB PRIMARY KEY REFERENCES sync_scopes(scope_id),
    reason TEXT NOT NULL CHECK (reason IN ('scope_binding', 'content_key_id', 'projection')),
    detected_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE quarantined_records (
    record_id BLOB PRIMARY KEY CHECK (length(record_id) = 32),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    reason TEXT NOT NULL CHECK (reason IN ('decode', 'order_key', 'open', 'projection')),
    detected_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX quarantined_records_scope ON quarantined_records(scope_id);
