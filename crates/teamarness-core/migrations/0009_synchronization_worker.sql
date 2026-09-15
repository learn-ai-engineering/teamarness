ALTER TABLE durable_wakes ADD COLUMN consumed_generation INTEGER NOT NULL DEFAULT 0 CHECK (consumed_generation >= 0);
