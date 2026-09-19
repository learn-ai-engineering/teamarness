-- ADR 0017: each content key exists in exactly one column.
--
-- `protection_state.state_bytes` held a second plaintext copy of the scope's
-- content key that nothing ever read. Dropping the column drops its own
-- column-level CHECK with it, and `secure_delete` zeroes the pages it frees.
ALTER TABLE protection_state DROP COLUMN state_bytes;
