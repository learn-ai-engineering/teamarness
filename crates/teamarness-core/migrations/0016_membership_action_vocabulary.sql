-- Workspace Persons are key groups (ADR 0023). Canonical membership operations
-- gain four actions beside create (0), admit (1) and revoke (2): create person
-- (3), attach person (4), add agent key (5) and remove agent key (6). SQLite
-- cannot widen a CHECK in place, so the append-only table is recreated with the
-- wider vocabulary and its rows and guards carried over. Migrations run with
-- foreign keys off, so the dependent tables keep referencing the table by name.
CREATE TABLE membership_operations_v16 (
    operation_id BLOB PRIMARY KEY CHECK (length(operation_id) = 32),
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    author_public_identity BLOB NOT NULL CHECK (length(author_public_identity) = 32),
    authored_at_unix_ms INTEGER NOT NULL,
    action INTEGER NOT NULL CHECK (action IN (0, 1, 2, 3, 4, 5, 6)),
    canonical_envelope BLOB NOT NULL UNIQUE
) STRICT;

INSERT INTO membership_operations_v16 (
    operation_id, workspace_id, author_public_identity, authored_at_unix_ms, action,
    canonical_envelope
)
SELECT operation_id, workspace_id, author_public_identity, authored_at_unix_ms, action,
       canonical_envelope
FROM membership_operations;

DROP TRIGGER membership_operations_no_update;
DROP TRIGGER membership_operations_no_delete;
DROP TABLE membership_operations;
ALTER TABLE membership_operations_v16 RENAME TO membership_operations;

CREATE TRIGGER membership_operations_no_update
BEFORE UPDATE ON membership_operations BEGIN
    SELECT RAISE(ABORT, 'canonical membership operations are append-only');
END;

CREATE TRIGGER membership_operations_no_delete
BEFORE DELETE ON membership_operations BEGIN
    SELECT RAISE(ABORT, 'canonical membership operations are append-only');
END;
