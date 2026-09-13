CREATE TABLE workspace_creation_requests (
    workspace_id BLOB PRIMARY KEY CHECK (length(workspace_id) = 16),
    creator_public_identity BLOB NOT NULL CHECK (length(creator_public_identity) = 32),
    name TEXT NOT NULL,
    authored_at_unix_ms INTEGER NOT NULL,
    genesis_operation_id BLOB NOT NULL UNIQUE CHECK (length(genesis_operation_id) = 32),
    genesis_envelope BLOB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('Pending', 'Complete'))
) STRICT;

CREATE TABLE workspaces (
    workspace_id BLOB PRIMARY KEY CHECK (length(workspace_id) = 16),
    creator_public_identity BLOB NOT NULL CHECK (length(creator_public_identity) = 32),
    name TEXT NOT NULL,
    authored_at_unix_ms INTEGER NOT NULL,
    genesis_operation_id BLOB NOT NULL UNIQUE CHECK (length(genesis_operation_id) = 32),
    genesis_envelope BLOB NOT NULL
) STRICT;

CREATE TABLE membership_operations (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 32),
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    authored_at_unix_ms INTEGER NOT NULL,
    action INTEGER NOT NULL CHECK (action IN (0, 1, 2)),
    canonical_envelope BLOB NOT NULL UNIQUE
) STRICT;

CREATE TABLE membership_dependencies (
    operation_id BLOB NOT NULL REFERENCES membership_operations(operation_id),
    dependency_operation_id BLOB NOT NULL REFERENCES membership_operations(operation_id),
    PRIMARY KEY (operation_id, dependency_operation_id),
    CHECK (operation_id != dependency_operation_id)
) STRICT;

CREATE TABLE future_membership_operations (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 32),
    workspace_id BLOB NOT NULL CHECK (length(workspace_id) = 16),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    authored_at_unix_ms INTEGER NOT NULL,
    canonical_envelope BLOB NOT NULL UNIQUE,
    rejection_reason TEXT NULL
) STRICT;

CREATE TABLE future_membership_dependencies (
    operation_id BLOB NOT NULL REFERENCES future_membership_operations(operation_id),
    dependency_operation_id BLOB NOT NULL CHECK (length(dependency_operation_id) = 32),
    PRIMARY KEY (operation_id, dependency_operation_id),
    CHECK (operation_id != dependency_operation_id)
) STRICT;

CREATE TABLE workspace_membership_state (
    workspace_id BLOB PRIMARY KEY REFERENCES workspaces(workspace_id),
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    membership_version BLOB NOT NULL CHECK (length(membership_version) = 32)
) STRICT;

CREATE TABLE membership_snapshot_members (
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    member_identity BLOB NOT NULL CHECK (length(member_identity) = 32),
    role INTEGER NOT NULL CHECK (role IN (0, 1)),
    PRIMARY KEY (workspace_id, member_identity)
) STRICT;

CREATE TRIGGER workspaces_no_update
BEFORE UPDATE ON workspaces BEGIN
    SELECT RAISE(ABORT, 'ready workspace trust anchors are immutable');
END;

CREATE TRIGGER workspaces_no_delete
BEFORE DELETE ON workspaces BEGIN
    SELECT RAISE(ABORT, 'ready workspace trust anchors are immutable');
END;

CREATE TRIGGER membership_operations_no_update
BEFORE UPDATE ON membership_operations BEGIN
    SELECT RAISE(ABORT, 'canonical membership operations are append-only');
END;

CREATE TRIGGER membership_operations_no_delete
BEFORE DELETE ON membership_operations BEGIN
    SELECT RAISE(ABORT, 'canonical membership operations are append-only');
END;

CREATE TRIGGER membership_dependencies_no_update
BEFORE UPDATE ON membership_dependencies BEGIN
    SELECT RAISE(ABORT, 'canonical membership dependencies are append-only');
END;

CREATE TRIGGER membership_dependencies_no_delete
BEFORE DELETE ON membership_dependencies BEGIN
    SELECT RAISE(ABORT, 'canonical membership dependencies are append-only');
END;
