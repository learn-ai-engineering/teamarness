-- Local presentation state; pending join intent has no Workspace FK until bootstrap import.
CREATE TABLE workspace_person_names (
    workspace_id BLOB NOT NULL,
    person_id BLOB NOT NULL,
    anchor_operation_id BLOB NOT NULL,
    owner_public_identity BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, person_id)
);
-- Local names remain queued until Phase 2 can publish a signed name edit.
-- Repeated offline renames coalesce to the latest requested name.
CREATE TABLE pending_person_name_edits (
    workspace_id BLOB NOT NULL,
    person_id BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, person_id),
    FOREIGN KEY (workspace_id, person_id)
        REFERENCES workspace_person_names (workspace_id, person_id)
);
CREATE TABLE pending_person_join_names (
    workspace_id BLOB NOT NULL,
    invitation_token_id BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, invitation_token_id)
);
ALTER TABLE workspace_creation_requests ADD COLUMN initial_person_name TEXT;
