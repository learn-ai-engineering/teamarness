-- ADR 0048: Tickets gain the `cancelled` status, and the store remembers which codec
-- generation last re-evaluated records quarantined as `malformed_record`.
CREATE TABLE tickets_0002 (
    ticket_id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL,
    epic_order TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('not_ready', 'ready', 'in_progress', 'done', 'cancelled')),
    status_order TEXT NOT NULL,
    priority TEXT NOT NULL CHECK (priority IN ('high', 'medium', 'low')),
    priority_order TEXT NOT NULL,
    assignee TEXT,
    assignee_order TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1)),
    archive_order TEXT NOT NULL,
    title_order TEXT NOT NULL,
    body_order TEXT NOT NULL,
    version TEXT NOT NULL,
    creator TEXT NOT NULL,
    create_order TEXT NOT NULL,
    create_at INTEGER NOT NULL,
    last_editor TEXT NOT NULL,
    last_at INTEGER NOT NULL
);
INSERT INTO tickets_0002 SELECT * FROM tickets;
DROP TABLE tickets;
ALTER TABLE tickets_0002 RENAME TO tickets;
CREATE INDEX tickets_epic ON tickets (epic_id, create_order);

-- 1 is the codec before ADR 0048. A store below the current generation re-evaluates its
-- `malformed_record` quarantine once, then records the generation.
ALTER TABLE metadata ADD COLUMN codec_generation INTEGER NOT NULL DEFAULT 1;
