CREATE TABLE used_invitation_nonces (
    nonce BLOB PRIMARY KEY CHECK (length(nonce) = 32),
    workspace_id BLOB NOT NULL REFERENCES workspaces(workspace_id),
    invitation_token_id BLOB NOT NULL UNIQUE CHECK (length(invitation_token_id) = 32),
    admission_operation_id BLOB NOT NULL UNIQUE REFERENCES membership_operations(operation_id)
) STRICT;
