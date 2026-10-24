-- ADR 0027: an Agent Binding is Package configuration. One Package Instance in
-- one Workspace maps to at most one Agent; any number of Packages may share an
-- Agent. Existing rows (at most one per Agent) carry over without collision.
CREATE TABLE agent_package_bindings_by_package (
    workspace_id BLOB NOT NULL CHECK(length(workspace_id)=16),
    package_identity TEXT NOT NULL,
    agent_principal_id BLOB NOT NULL CHECK(length(agent_principal_id)=32),
    PRIMARY KEY(workspace_id,package_identity),
    FOREIGN KEY(workspace_id,agent_principal_id) REFERENCES agent_keys(workspace_id,agent_principal_id)
) STRICT;
INSERT INTO agent_package_bindings_by_package (workspace_id, package_identity, agent_principal_id)
SELECT workspace_id, package_identity, agent_principal_id FROM agent_package_bindings;
DROP TABLE agent_package_bindings;
ALTER TABLE agent_package_bindings_by_package RENAME TO agent_package_bindings;
-- The signing rule and removal cleanup look a binding up by Agent.
CREATE INDEX agent_package_bindings_agent ON agent_package_bindings(workspace_id, agent_principal_id);
