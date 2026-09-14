PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS package_host_schema (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    applied_at_unix_ms INTEGER NOT NULL
);
INSERT OR IGNORE INTO package_host_schema(version, applied_at_unix_ms) VALUES (1, 0);

CREATE TABLE IF NOT EXISTS admissions (
    admission_digest TEXT PRIMARY KEY,
    package_identity TEXT NOT NULL,
    package_version TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    backend_digest TEXT NOT NULL,
    ui_contract_digest TEXT NOT NULL,
    admitted_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS package_data_directories (
    installation_identity TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    package_identity TEXT NOT NULL,
    path_version INTEGER NOT NULL CHECK (path_version = 1),
    relative_path TEXT NOT NULL UNIQUE,
    continuity_id TEXT NOT NULL UNIQUE,
    PRIMARY KEY (installation_identity, workspace_id, package_identity)
);

CREATE TABLE IF NOT EXISTS diagnostics (
    diagnostic_id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    stage TEXT NOT NULL,
    package_identity TEXT,
    observed_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS process_leases (
    package_identity TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    process_generation INTEGER NOT NULL,
    pid INTEGER NOT NULL,
    process_group INTEGER NOT NULL,
    start_time TEXT NOT NULL,
    canonical_executable TEXT NOT NULL,
    executable_digest TEXT NOT NULL,
    launch_token TEXT NOT NULL,
    PRIMARY KEY (package_identity, workspace_id)
);
