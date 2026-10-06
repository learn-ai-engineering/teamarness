CREATE TABLE history_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    epoch TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0)
);

CREATE TABLE history_changes (
    revision INTEGER PRIMARY KEY,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    order_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('insert', 'remove'))
);
CREATE INDEX history_changes_channel_revision ON history_changes (channel_id, revision);
