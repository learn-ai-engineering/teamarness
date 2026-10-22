-- References only immutable accepted deliveries, never copies sealed records.
CREATE TABLE deferred_principal_names (
    workspace_id BLOB NOT NULL CHECK(length(workspace_id)=16),
    delivery_sequence INTEGER NOT NULL REFERENCES record_delivery(delivery_sequence),
    principal_id BLOB NOT NULL CHECK(length(principal_id)=32),
    PRIMARY KEY (workspace_id, delivery_sequence)
) STRICT;
CREATE INDEX deferred_principal_names_target ON deferred_principal_names(workspace_id, principal_id, delivery_sequence);
