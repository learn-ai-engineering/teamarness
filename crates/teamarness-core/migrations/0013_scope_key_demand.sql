CREATE TABLE sync_scope_key_demands (
    scope_id BLOB PRIMARY KEY CHECK (length(scope_id) = 32),
    package_identity TEXT NOT NULL,
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id) CHECK (length(workspace_id) = 16),
    demand_generation INTEGER NOT NULL DEFAULT 1 CHECK (demand_generation > 0),
    UNIQUE (package_identity, workspace_id)
) STRICT;

CREATE INDEX sync_scope_key_demands_workspace ON sync_scope_key_demands(workspace_id);

CREATE TABLE sync_scope_key_demand_peer_progress (
    scope_id BLOB NOT NULL REFERENCES sync_scope_key_demands(scope_id) ON DELETE CASCADE,
    peer_public_identity BLOB NOT NULL CHECK (length(peer_public_identity) = 32),
    consumed_generation INTEGER NOT NULL CHECK (consumed_generation >= 0),
    PRIMARY KEY (scope_id, peer_public_identity)
) STRICT;
