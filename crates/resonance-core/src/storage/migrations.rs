use rusqlite::{Connection, TransactionBehavior};

use crate::error::StorageError;

const LATEST_SCHEMA_VERSION: i64 = 1;
const MIGRATION_0001: &str = include_str!("../../migrations/0001_core.sql");

pub(super) fn apply(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::Database)?;
    let current_version = transaction
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| StorageError::Database)?;

    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchema);
    }

    if current_version < 1 {
        transaction
            .execute_batch(MIGRATION_0001)
            .map_err(|_| StorageError::Migration)?;
        transaction
            .pragma_update(None, "user_version", 1_i64)
            .map_err(|_| StorageError::Migration)?;
    }

    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)
}
