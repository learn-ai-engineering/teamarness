-- ADR 0050: a Workspace whose deletion the Host has started. Written before Core erases the
-- Workspace and removed after its Package Data Directories are gone, so opening the Host finishes
-- a deletion that stopped part way, or drops the entry when Core still has the Workspace.
CREATE TABLE IF NOT EXISTS pending_workspace_deletions (
    installation_identity TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY (installation_identity, workspace_id)
);
