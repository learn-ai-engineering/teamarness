-- A Workspace-wide revision log of projected Comment changes, for a Consumer that acts on each
-- new Comment exactly once (Port 1.3 comment.changes). The epoch is set randomly when the log
-- is created; a Consumer whose saved epoch differs must resynchronize rather than replay.
CREATE TABLE comment_history_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    epoch TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0)
);

CREATE TABLE comment_history_changes (
    revision INTEGER PRIMARY KEY,
    comment_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('add', 'resolve', 'reopen'))
);
