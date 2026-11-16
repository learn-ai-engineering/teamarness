-- Installation-local intent survives placement append and filesystem rename crash points.
CREATE TABLE file_relocations (
    document_id TEXT PRIMARY KEY REFERENCES mapped_files ON DELETE CASCADE,
    source_path TEXT NOT NULL,
    destination_path TEXT NOT NULL,
    expected_hash BLOB NOT NULL CHECK(length(expected_hash) = 32),
    operation_id BLOB NOT NULL CHECK(length(operation_id) = 16),
    local_move INTEGER NOT NULL CHECK(local_move IN (0, 1))
);
