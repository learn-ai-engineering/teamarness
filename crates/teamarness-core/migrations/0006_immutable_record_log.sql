CREATE TABLE sync_scopes (
    scope_id BLOB PRIMARY KEY CHECK (length(scope_id) = 32),
    package_identity TEXT NOT NULL,
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    journal_id BLOB NOT NULL CHECK (length(journal_id) = 16),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32),
    content_key BLOB NOT NULL CHECK (length(content_key) = 32),
    UNIQUE (package_identity, workspace_id)
) STRICT;

CREATE TABLE immutable_operations (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 32),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    record_id BLOB NOT NULL UNIQUE CHECK (length(record_id) = 32),
    order_key BLOB NOT NULL UNIQUE CHECK (length(order_key) = 41),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    author_sequence INTEGER NOT NULL CHECK (author_sequence >= 0),
    operation_bytes BLOB NOT NULL CHECK (length(operation_bytes) > 0),
    parents BLOB NOT NULL,
    causal_height INTEGER NOT NULL CHECK (causal_height >= 0),
    status TEXT NOT NULL CHECK (status IN ('accepted', 'pending', 'poisoned')),
    UNIQUE (scope_id, author_public_identity, author_sequence)
) STRICT;
CREATE TABLE immutable_operation_dependencies (
    operation_id BLOB NOT NULL REFERENCES immutable_operations(operation_id),
    dependency_operation_id BLOB NOT NULL REFERENCES immutable_operations(operation_id),
    PRIMARY KEY (operation_id, dependency_operation_id),
    CHECK (operation_id != dependency_operation_id)
) STRICT;
CREATE TABLE immutable_author_heads (
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    operation_id BLOB NOT NULL REFERENCES immutable_operations(operation_id),
    PRIMARY KEY (scope_id, author_public_identity)
) STRICT;
CREATE TABLE immutable_records (
    record_id BLOB PRIMARY KEY REFERENCES immutable_operations(record_id),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    purpose TEXT NOT NULL,
    canonical_digest BLOB NOT NULL CHECK (length(canonical_digest) = 32),
    signature BLOB NOT NULL CHECK (length(signature) = 64),
    sealed_record BLOB NOT NULL CHECK (length(sealed_record) > 0)
) STRICT;
CREATE TABLE immutable_idempotency (
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    purpose TEXT NOT NULL,
    canonical_digest BLOB NOT NULL CHECK (length(canonical_digest) = 32),
    record_id BLOB NOT NULL REFERENCES immutable_records(record_id),
    PRIMARY KEY (scope_id, author_public_identity, purpose, canonical_digest)
) STRICT;
CREATE TABLE sealed_envelopes (
    record_id BLOB PRIMARY KEY REFERENCES immutable_records(record_id),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    envelope BLOB NOT NULL CHECK (length(envelope) > 0)
) STRICT;
CREATE TABLE protection_state (
    scope_id BLOB PRIMARY KEY REFERENCES sync_scopes(scope_id),
    state_bytes BLOB NOT NULL CHECK (length(state_bytes) > 0),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32)
) STRICT;
CREATE TABLE protection_pre_keys (
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    member_public_identity BLOB NOT NULL CHECK (length(member_public_identity) = 32),
    certificate BLOB NOT NULL CHECK (length(certificate) > 0),
    PRIMARY KEY (scope_id, member_public_identity)
) STRICT;
CREATE TABLE protection_grant_slots (
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    recipient_public_identity BLOB NOT NULL CHECK (length(recipient_public_identity) = 32),
    admission_operation_id BLOB NOT NULL CHECK (length(admission_operation_id) = 32),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32),
    grant_bytes BLOB NOT NULL CHECK (length(grant_bytes) > 0),
    PRIMARY KEY (scope_id, recipient_public_identity, admission_operation_id)
) STRICT;
CREATE TABLE pending_records (
    operation_id BLOB PRIMARY KEY REFERENCES immutable_operations(operation_id),
    reason TEXT NOT NULL CHECK (reason IN ('membership', 'parents', 'welcome', 'key', 'protection')),
    sealed_record BLOB NULL
) STRICT;
CREATE TABLE record_delivery (
    delivery_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    journal_id BLOB NOT NULL CHECK (length(journal_id) = 16),
    record_id BLOB NOT NULL UNIQUE REFERENCES immutable_records(record_id),
    created_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE durable_wakes (
    scope_id BLOB PRIMARY KEY REFERENCES sync_scopes(scope_id),
    wake_generation INTEGER NOT NULL CHECK (wake_generation >= 0)
) STRICT;

CREATE INDEX immutable_operations_scope_order ON immutable_operations(scope_id, order_key);
CREATE INDEX immutable_operations_scope_status ON immutable_operations(scope_id, status);
CREATE INDEX immutable_operation_dependencies_dependency ON immutable_operation_dependencies(dependency_operation_id);
CREATE INDEX immutable_records_scope_order ON immutable_records(scope_id, record_id);
CREATE INDEX record_delivery_scope_sequence ON record_delivery(scope_id, delivery_sequence);
CREATE INDEX pending_records_reason ON pending_records(reason);

CREATE TRIGGER immutable_operations_no_update BEFORE UPDATE ON immutable_operations BEGIN SELECT RAISE(ABORT, 'immutable operations are append-only'); END;
CREATE TRIGGER immutable_operations_no_delete BEFORE DELETE ON immutable_operations BEGIN SELECT RAISE(ABORT, 'immutable operations are append-only'); END;
CREATE TRIGGER immutable_operation_dependencies_no_update BEFORE UPDATE ON immutable_operation_dependencies BEGIN SELECT RAISE(ABORT, 'operation dependencies are append-only'); END;
CREATE TRIGGER immutable_operation_dependencies_no_delete BEFORE DELETE ON immutable_operation_dependencies BEGIN SELECT RAISE(ABORT, 'operation dependencies are append-only'); END;
CREATE TRIGGER immutable_records_no_update BEFORE UPDATE ON immutable_records BEGIN SELECT RAISE(ABORT, 'immutable records are append-only'); END;
CREATE TRIGGER immutable_records_no_delete BEFORE DELETE ON immutable_records BEGIN SELECT RAISE(ABORT, 'immutable records are append-only'); END;
CREATE TRIGGER record_delivery_no_update BEFORE UPDATE ON record_delivery BEGIN SELECT RAISE(ABORT, 'delivery rows are append-only'); END;
CREATE TRIGGER record_delivery_no_delete BEFORE DELETE ON record_delivery BEGIN SELECT RAISE(ABORT, 'delivery rows are append-only'); END;
