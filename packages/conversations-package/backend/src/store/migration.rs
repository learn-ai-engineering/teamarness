use rusqlite::{Connection, TransactionBehavior};

use crate::store::StoreError;

const LATEST_SCHEMA_VERSION: i64 = 2;
const MIGRATION_0001: &str = include_str!("../../migrations/0001_conversations.sql");
const MIGRATION_0002: &str = include_str!("../../migrations/0002_history_changes.sql");

pub(super) fn apply(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StoreError::Storage)?;
    let current_version = transaction
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| StoreError::Storage)?;

    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema);
    }

    // Version 0 with no Conversations tables is treated as new.
    // Version 0 with existing tables must pass validation before adoption.
    let mut version = current_version;
    if version < 1 {
        let has_records: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='records'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| StoreError::Storage)?;
        if has_records == 0 {
            // Fresh database.
            transaction
                .execute_batch(MIGRATION_0001)
                .map_err(|_| StoreError::Migration)?;
            version = 1;
        } else {
            // Legacy v0 database — validate before adopting.
            let required_tables = ["records", "channels", "messages", "unread", "metadata"];
            for table in required_tables {
                let exists: i64 = transaction
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |row| row.get(0),
                    )
                    .map_err(|_| StoreError::Storage)?;
                if exists == 0 {
                    return Err(StoreError::LegacyValidation(format!(
                        "missing table: {table}"
                    )));
                }
            }
            // Validate records columns.
            let record_columns: Vec<String> = {
                let mut stmt = transaction
                    .prepare("PRAGMA table_info('records')")
                    .map_err(|_| StoreError::Storage)?;
                let rows = stmt
                    .query_map([], |row| row.get(1))
                    .map_err(|_| StoreError::Storage)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|_| StoreError::Storage)?
            };
            let required_columns = [
                "record_id",
                "order_key",
                "author",
                "purpose",
                "signature",
                "sealed_record",
                "cursor",
                "ingestion_ordinal",
                "quarantine_code",
            ];
            for col in required_columns {
                if !record_columns.iter().any(|c| c == col) {
                    return Err(StoreError::LegacyValidation(format!(
                        "missing column: records.{col}"
                    )));
                }
            }
            // Adopt the legacy schema atomically.
            version = 1;
        }
    }

    if version < 2 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| StoreError::Migration)?;
        let epoch = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        transaction
            .execute_batch(MIGRATION_0002)
            .map_err(|_| StoreError::Migration)?;
        transaction
            .execute(
                "INSERT INTO history_state(singleton,epoch,revision) VALUES(1,?1,0)",
                [epoch],
            )
            .map_err(|_| StoreError::Migration)?;
    }
    transaction
        .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
        .map_err(|_| StoreError::Migration)?;

    transaction
        .commit()
        .map_err(|_| StoreError::UncertainOutcome)
}
