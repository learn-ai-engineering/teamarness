-- A revision identifies one pending intent even when names repeat or a completed row is deleted.
-- v14 pending rows retain their names with revision 1.
ALTER TABLE pending_person_name_edits ADD COLUMN intent_revision INTEGER NOT NULL DEFAULT 1 CHECK (intent_revision > 0);
CREATE TABLE workspace_person_name_intent_revisions (
    workspace_id BLOB NOT NULL,
    person_id BLOB NOT NULL,
    last_revision INTEGER NOT NULL CHECK (last_revision BETWEEN 1 AND 9223372036854775806),
    PRIMARY KEY (workspace_id, person_id)
);
INSERT INTO workspace_person_name_intent_revisions (workspace_id, person_id, last_revision)
SELECT workspace_id, person_id, intent_revision FROM pending_person_name_edits;
-- The creator's recovered genesis path inserts directly, while set/join use UPSERT.
-- Trigger both insert and name updates so every new intent advances the same durable counter.
CREATE TRIGGER person_name_intent_insert AFTER INSERT ON pending_person_name_edits BEGIN
    INSERT INTO workspace_person_name_intent_revisions (workspace_id, person_id, last_revision)
    VALUES (NEW.workspace_id, NEW.person_id, 1)
    ON CONFLICT(workspace_id, person_id) DO UPDATE SET last_revision = last_revision + 1;
    UPDATE pending_person_name_edits SET intent_revision = (
        SELECT last_revision FROM workspace_person_name_intent_revisions
        WHERE workspace_id = NEW.workspace_id AND person_id = NEW.person_id
    ) WHERE workspace_id = NEW.workspace_id AND person_id = NEW.person_id;
END;
CREATE TRIGGER person_name_intent_name_update AFTER UPDATE OF name ON pending_person_name_edits BEGIN
    UPDATE workspace_person_name_intent_revisions SET last_revision = last_revision + 1
    WHERE workspace_id = NEW.workspace_id AND person_id = NEW.person_id;
    UPDATE pending_person_name_edits SET intent_revision = (
        SELECT last_revision FROM workspace_person_name_intent_revisions
        WHERE workspace_id = NEW.workspace_id AND person_id = NEW.person_id
    ) WHERE workspace_id = NEW.workspace_id AND person_id = NEW.person_id;
END;

-- One signed materialized winner per Person. Canonical history belongs to the immutable record log.
CREATE TABLE workspace_person_name_winners (
    workspace_id BLOB NOT NULL,
    person_id BLOB NOT NULL,
    counter INTEGER NOT NULL CHECK (counter > 0),
    edit_id BLOB NOT NULL,
    canonical_edit BLOB NOT NULL,
    PRIMARY KEY (workspace_id, person_id)
);
-- Snapshot invalidation cursor: readers re-fetch the latest projection after any gap.
CREATE TABLE workspace_person_name_generations (
    workspace_id BLOB PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 0)
);
-- Last processed immutable delivery row; replay from it after any interrupted projection.
CREATE TABLE workspace_person_name_consumption (
    workspace_id BLOB PRIMARY KEY,
    delivery_sequence INTEGER NOT NULL CHECK (delivery_sequence >= 0)
);
