-- ADR 0049: Installation-local Workspace rename. ADR 0050: sole-creator Workspace deletion.

-- Only the local name may change; every trust-anchor column stays immutable.
DROP TRIGGER IF EXISTS workspaces_no_update;
CREATE TRIGGER workspaces_no_update
BEFORE UPDATE OF workspace_id, creator_public_identity, authored_at_unix_ms,
                 genesis_operation_id, genesis_envelope ON workspaces BEGIN
    SELECT RAISE(ABORT, 'ready workspace trust anchors are immutable');
END;

-- The Workspace being erased by the open transaction. Erasure writes the row, deletes every row
-- of the Workspace, then deletes the marker before it commits, so no marker outlives its
-- transaction. It has no foreign key: the Workspace row it names is deleted while it exists.
CREATE TABLE IF NOT EXISTS workspace_deletions (
    workspace_id BLOB PRIMARY KEY CHECK (length(workspace_id) = 16),
    requested_at_unix_ms INTEGER NOT NULL
) STRICT;

-- Append-only rows stay append-only, except while their own Workspace is marked for deletion.
DROP TRIGGER IF EXISTS workspaces_no_delete;
CREATE TRIGGER workspaces_no_delete
BEFORE DELETE ON workspaces
WHEN NOT EXISTS (SELECT 1 FROM workspace_deletions WHERE workspace_id = OLD.workspace_id) BEGIN
    SELECT RAISE(ABORT, 'ready workspace trust anchors are immutable');
END;

DROP TRIGGER IF EXISTS membership_operations_no_delete;
CREATE TRIGGER membership_operations_no_delete
BEFORE DELETE ON membership_operations
WHEN NOT EXISTS (SELECT 1 FROM workspace_deletions WHERE workspace_id = OLD.workspace_id) BEGIN
    SELECT RAISE(ABORT, 'canonical membership operations are append-only');
END;

DROP TRIGGER IF EXISTS membership_dependencies_no_delete;
CREATE TRIGGER membership_dependencies_no_delete
BEFORE DELETE ON membership_dependencies
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN membership_operations o ON o.workspace_id = d.workspace_id
    WHERE o.operation_id = OLD.operation_id
) BEGIN
    SELECT RAISE(ABORT, 'canonical membership dependencies are append-only');
END;

DROP TRIGGER IF EXISTS membership_transitions_no_delete;
CREATE TRIGGER membership_transitions_no_delete
BEFORE DELETE ON membership_transitions
WHEN NOT EXISTS (SELECT 1 FROM workspace_deletions WHERE workspace_id = OLD.workspace_id) BEGIN
    SELECT RAISE(ABORT, 'membership transitions are append-only');
END;

DROP TRIGGER IF EXISTS used_invitation_nonces_no_delete;
CREATE TRIGGER used_invitation_nonces_no_delete
BEFORE DELETE ON used_invitation_nonces
WHEN NOT EXISTS (SELECT 1 FROM workspace_deletions WHERE workspace_id = OLD.workspace_id) BEGIN
    SELECT RAISE(ABORT, 'spent invitation bindings are append-only');
END;

DROP TRIGGER IF EXISTS immutable_operations_no_delete;
CREATE TRIGGER immutable_operations_no_delete
BEFORE DELETE ON immutable_operations
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    WHERE s.scope_id = OLD.scope_id
) BEGIN
    SELECT RAISE(ABORT, 'immutable operations are append-only');
END;

DROP TRIGGER IF EXISTS immutable_operation_dependencies_no_delete;
CREATE TRIGGER immutable_operation_dependencies_no_delete
BEFORE DELETE ON immutable_operation_dependencies
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    JOIN immutable_operations o ON o.scope_id = s.scope_id
    WHERE o.operation_id = OLD.operation_id
) BEGIN
    SELECT RAISE(ABORT, 'operation dependencies are append-only');
END;

DROP TRIGGER IF EXISTS immutable_records_no_delete;
CREATE TRIGGER immutable_records_no_delete
BEFORE DELETE ON immutable_records
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    WHERE s.scope_id = OLD.scope_id
) BEGIN
    SELECT RAISE(ABORT, 'immutable records are append-only');
END;

DROP TRIGGER IF EXISTS record_delivery_no_delete;
CREATE TRIGGER record_delivery_no_delete
BEFORE DELETE ON record_delivery
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    WHERE s.scope_id = OLD.scope_id
) BEGIN
    SELECT RAISE(ABORT, 'delivery rows are append-only');
END;

DROP TRIGGER p2panda_operations_no_delete;
CREATE TRIGGER p2panda_operations_no_delete
BEFORE DELETE ON p2panda_operations
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    WHERE s.scope_id = OLD.scope_id
) BEGIN
    SELECT RAISE(ABORT, 'p2panda operations are append-only');
END;

DROP TRIGGER IF EXISTS protection_control_operations_no_delete;
CREATE TRIGGER protection_control_operations_no_delete
BEFORE DELETE ON protection_control_operations
WHEN NOT EXISTS (
    SELECT 1 FROM workspace_deletions d
    JOIN sync_scopes s ON s.workspace_id = d.workspace_id
    WHERE s.scope_id = OLD.scope_id
) BEGIN
    SELECT RAISE(ABORT, 'protection controls are append-only');
END;

