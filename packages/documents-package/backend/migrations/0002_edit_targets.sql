-- A body edit record names the Document it belongs to so its sealed bytes can be found and
-- re-opened per Document after restart. The Document id is a record identity, not content.
ALTER TABLE records ADD COLUMN target TEXT;
CREATE INDEX records_target ON records (target, order_key);
