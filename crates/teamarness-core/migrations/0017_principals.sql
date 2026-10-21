-- Pre-production: old names are not migrated. Pending join intents remain local
-- until their membership claim commits; the intent table is recreated as well.
DROP TRIGGER person_name_intent_insert;
DROP TRIGGER person_name_intent_name_update;
DROP TABLE workspace_person_name_consumption;
DROP TABLE workspace_person_name_generations;
DROP TABLE workspace_person_name_winners;
DROP TABLE workspace_person_name_intent_revisions;
DROP TABLE pending_person_join_names;
DROP TABLE pending_person_name_edits;
DROP TABLE workspace_person_names;
-- Local presentation state; pending join intent has no Workspace FK until bootstrap import.
CREATE TABLE principal_names (
    workspace_id BLOB NOT NULL,
    principal_id BLOB NOT NULL,
    anchor_operation_id BLOB NOT NULL,
    owner_public_identity BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, principal_id)
);
-- Local names remain queued until Phase 2 can publish a signed name edit.
-- Repeated offline renames coalesce to the latest requested name.
CREATE TABLE pending_principal_name_edits (
    workspace_id BLOB NOT NULL,
    principal_id BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, principal_id),
    FOREIGN KEY (workspace_id, principal_id)
        REFERENCES principal_names (workspace_id, principal_id)
);
CREATE TABLE pending_principal_join_names (
    workspace_id BLOB NOT NULL,
    invitation_token_id BLOB NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (workspace_id, invitation_token_id)
);

-- A revision identifies one pending intent even when names repeat or a completed row is deleted.
-- v14 pending rows retain their names with revision 1.
ALTER TABLE pending_principal_name_edits ADD COLUMN intent_revision INTEGER NOT NULL DEFAULT 1 CHECK (intent_revision > 0);
CREATE TABLE principal_name_intent_revisions (
    workspace_id BLOB NOT NULL,
    principal_id BLOB NOT NULL,
    last_revision INTEGER NOT NULL CHECK (last_revision BETWEEN 1 AND 9223372036854775806),
    PRIMARY KEY (workspace_id, principal_id)
);
INSERT INTO principal_name_intent_revisions (workspace_id, principal_id, last_revision)
SELECT workspace_id, principal_id, intent_revision FROM pending_principal_name_edits;
-- The creator's recovered genesis path inserts directly, while set/join use UPSERT.
-- Trigger both insert and name updates so every new intent advances the same durable counter.
CREATE TRIGGER principal_name_intent_insert AFTER INSERT ON pending_principal_name_edits BEGIN
    INSERT INTO principal_name_intent_revisions (workspace_id, principal_id, last_revision)
    VALUES (NEW.workspace_id, NEW.principal_id, 1)
    ON CONFLICT(workspace_id, principal_id) DO UPDATE SET last_revision = last_revision + 1;
    UPDATE pending_principal_name_edits SET intent_revision = (
        SELECT last_revision FROM principal_name_intent_revisions
        WHERE workspace_id = NEW.workspace_id AND principal_id = NEW.principal_id
    ) WHERE workspace_id = NEW.workspace_id AND principal_id = NEW.principal_id;
END;
CREATE TRIGGER principal_name_intent_name_update AFTER UPDATE OF name ON pending_principal_name_edits BEGIN
    UPDATE principal_name_intent_revisions SET last_revision = last_revision + 1
    WHERE workspace_id = NEW.workspace_id AND principal_id = NEW.principal_id;
    UPDATE pending_principal_name_edits SET intent_revision = (
        SELECT last_revision FROM principal_name_intent_revisions
        WHERE workspace_id = NEW.workspace_id AND principal_id = NEW.principal_id
    ) WHERE workspace_id = NEW.workspace_id AND principal_id = NEW.principal_id;
END;

-- One signed materialized winner per Person. Canonical history belongs to the immutable record log.
CREATE TABLE principal_name_winners (
    workspace_id BLOB NOT NULL,
    principal_id BLOB NOT NULL,
    counter INTEGER NOT NULL CHECK (counter > 0),
    edit_id BLOB NOT NULL,
    canonical_edit BLOB NOT NULL,
    PRIMARY KEY (workspace_id, principal_id)
);
-- Snapshot invalidation cursor: readers re-fetch the latest projection after any gap.
CREATE TABLE principal_name_generations (
    workspace_id BLOB PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 0)
);
-- Last processed immutable delivery row; replay from it after any interrupted projection.
CREATE TABLE principal_name_consumption (
    workspace_id BLOB PRIMARY KEY,
    delivery_sequence INTEGER NOT NULL CHECK (delivery_sequence >= 0)
);

CREATE TABLE workspace_principals (
    workspace_id BLOB NOT NULL CHECK(length(workspace_id)=16),
    principal_id BLOB NOT NULL CHECK(length(principal_id)=32),
    kind TEXT NOT NULL CHECK(kind IN ('person','agent')),
    steward_principal_id BLOB CHECK(steward_principal_id IS NULL OR length(steward_principal_id)=32),
    public_identity BLOB NOT NULL CHECK(length(public_identity)=32),
    anchor_operation_id BLOB NOT NULL CHECK(length(anchor_operation_id)=32),
    PRIMARY KEY(workspace_id,principal_id),
    UNIQUE(workspace_id,public_identity)
) STRICT;
CREATE TABLE agent_keys (
    workspace_id BLOB NOT NULL CHECK(length(workspace_id)=16),
    agent_principal_id BLOB NOT NULL CHECK(length(agent_principal_id)=32),
    public_identity BLOB NOT NULL CHECK(length(public_identity)=32),
    secret_key BLOB NOT NULL CHECK(length(secret_key)=32),
    PRIMARY KEY(workspace_id,agent_principal_id),
    UNIQUE(workspace_id,public_identity)
) STRICT;
CREATE TABLE agent_package_bindings (
    workspace_id BLOB NOT NULL CHECK(length(workspace_id)=16),
    agent_principal_id BLOB NOT NULL CHECK(length(agent_principal_id)=32),
    package_identity TEXT NOT NULL,
    PRIMARY KEY(workspace_id,agent_principal_id),
    FOREIGN KEY(workspace_id,agent_principal_id) REFERENCES agent_keys(workspace_id,agent_principal_id)
) STRICT;
