ALTER TABLE protection_state ADD COLUMN creator_public_identity BLOB;

CREATE TABLE protection_control_operations (
    control_id BLOB PRIMARY KEY CHECK (length(control_id) = 32),
    scope_id BLOB NOT NULL REFERENCES sync_scopes(scope_id),
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('create', 'pre-key', 'grant')),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    recipient_public_identity BLOB NOT NULL CHECK (length(recipient_public_identity) = 32),
    admission_operation_id BLOB NOT NULL CHECK (length(admission_operation_id) = 32),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32),
    control_bytes BLOB NOT NULL CHECK (length(control_bytes) > 0),
    signature BLOB NOT NULL CHECK (length(signature) = 64),
    UNIQUE (scope_id, recipient_public_identity, admission_operation_id)
) STRICT;

CREATE INDEX protection_control_operations_scope ON protection_control_operations(scope_id);
CREATE TRIGGER protection_control_operations_no_update
BEFORE UPDATE ON protection_control_operations BEGIN
    SELECT RAISE(ABORT, 'protection controls are append-only');
END;
CREATE TRIGGER protection_control_operations_no_delete
BEFORE DELETE ON protection_control_operations BEGIN
    SELECT RAISE(ABORT, 'protection controls are append-only');
END;

CREATE TABLE pending_protection_records (
    pending_id INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_id BLOB NOT NULL CHECK (length(scope_id) = 32),
    package_identity TEXT NOT NULL,
    workspace_id BLOB NOT NULL CHECK (length(workspace_id) = 16),
    content_key_id BLOB NOT NULL CHECK (length(content_key_id) = 32),
    sealed_record BLOB NOT NULL CHECK (length(sealed_record) > 0),
    UNIQUE (scope_id, sealed_record)
) STRICT;
