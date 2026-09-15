CREATE TABLE p2panda_operations (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 32),
    teamarness_operation_id BLOB NOT NULL UNIQUE REFERENCES immutable_operations(operation_id),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    log_id BLOB NOT NULL CHECK (length(log_id) = 32),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    author_sequence INTEGER NOT NULL CHECK (author_sequence >= 0),
    backlink BLOB NULL CHECK (backlink IS NULL OR length(backlink) = 32),
    payload_hash BLOB NOT NULL CHECK (length(payload_hash) = 32),
    payload BLOB NOT NULL CHECK (length(payload) > 0),
    header BLOB NOT NULL CHECK (length(header) > 0),
    operation_bytes BLOB NOT NULL CHECK (length(operation_bytes) > 0),
    record_id BLOB NOT NULL UNIQUE CHECK (length(record_id) = 32),
    order_key BLOB NOT NULL CHECK (length(order_key) = 41),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32),
    causal_parents BLOB NOT NULL,
    causal_height INTEGER NOT NULL CHECK (causal_height >= 0),
    true_height INTEGER NOT NULL CHECK (true_height >= 0),
    operation_kind TEXT NOT NULL CHECK (length(operation_kind) > 0),
    status TEXT NOT NULL CHECK (status IN ('accepted', 'pending', 'poisoned'))
) STRICT;

CREATE UNIQUE INDEX p2panda_operations_scope_author_sequence
    ON p2panda_operations(scope_id, author_public_identity, author_sequence);
CREATE INDEX p2panda_operations_scope_log_height
    ON p2panda_operations(scope_id, log_id, author_sequence);
CREATE INDEX p2panda_operations_scope_status
    ON p2panda_operations(scope_id, status);
CREATE INDEX p2panda_operations_backlink
    ON p2panda_operations(backlink);
CREATE INDEX p2panda_operations_teamarness_id
    ON p2panda_operations(teamarness_operation_id);

CREATE TABLE p2panda_projection_recovery (
    teamarness_operation_id BLOB PRIMARY KEY
        REFERENCES immutable_operations(operation_id),
    reason TEXT NOT NULL CHECK (reason IN ('missing_author_key', 'invalid_projection'))
) STRICT;
CREATE INDEX p2panda_projection_recovery_reason
    ON p2panda_projection_recovery(reason);

CREATE TRIGGER p2panda_operations_no_update
BEFORE UPDATE ON p2panda_operations
BEGIN
    SELECT RAISE(ABORT, 'p2panda operations are append-only');
END;
CREATE TRIGGER p2panda_operations_no_delete
BEFORE DELETE ON p2panda_operations
BEGIN
    SELECT RAISE(ABORT, 'p2panda operations are append-only');
END;
