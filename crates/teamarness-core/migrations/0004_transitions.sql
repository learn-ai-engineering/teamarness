CREATE TABLE membership_transitions (
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    membership_version BLOB NOT NULL CHECK (length(membership_version) = 32),
    cause_operation_id BLOB NOT NULL UNIQUE REFERENCES membership_operations(operation_id),
    snapshot_digest BLOB NOT NULL CHECK (length(snapshot_digest) = 32),
    PRIMARY KEY (workspace_id, epoch)
) STRICT;

CREATE TRIGGER membership_transitions_no_update
BEFORE UPDATE ON membership_transitions BEGIN
    SELECT RAISE(ABORT, 'membership transitions are append-only');
END;

CREATE TRIGGER membership_transitions_no_delete
BEFORE DELETE ON membership_transitions BEGIN
    SELECT RAISE(ABORT, 'membership transitions are append-only');
END;
