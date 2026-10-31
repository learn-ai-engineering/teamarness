CREATE TABLE IF NOT EXISTS agent_port_selections (
    installation_identity TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    package_identity TEXT NOT NULL,
    contract_identity TEXT NOT NULL,
    major INTEGER NOT NULL CHECK (major BETWEEN 1 AND 65535),
    PRIMARY KEY (installation_identity, workspace_id, package_identity, contract_identity)
);
