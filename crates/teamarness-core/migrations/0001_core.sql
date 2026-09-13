CREATE TABLE installation_identity (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    secret_key BLOB NOT NULL CHECK (length(secret_key) = 32),
    display_name TEXT NOT NULL
) STRICT;

CREATE TABLE identity_subjects (
    installation_public_identity BLOB PRIMARY KEY
        CHECK (length(installation_public_identity) = 32),
    human_identity_id BLOB NULL
) STRICT;
