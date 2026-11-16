-- The format generation this Installation last ran under. 0 means a build from before generations.
ALTER TABLE metadata ADD COLUMN format_generation INTEGER NOT NULL DEFAULT 0;
