use rusqlite::{Connection, TransactionBehavior};

use crate::store::StoreError;

const LATEST_SCHEMA_VERSION: i64 = 5;
const MIGRATION_0001: &str = include_str!("../../migrations/0001_documents.sql");
const MIGRATION_0002: &str = include_str!("../../migrations/0002_edit_targets.sql");
const MIGRATION_0003: &str = include_str!("../../migrations/0003_comments.sql");
const MIGRATION_0004: &str = include_str!("../../migrations/0004_comment_time.sql");
const MIGRATION_0005: &str = include_str!("../../migrations/0005_edit_identity_index.sql");

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
    if current_version < 1 {
        transaction
            .execute_batch(MIGRATION_0001)
            .map_err(|_| StoreError::Migration)?;
    }
    if current_version < 2 {
        transaction
            .execute_batch(MIGRATION_0002)
            .map_err(|_| StoreError::Migration)?;
    }
    if current_version < 3 {
        transaction
            .execute_batch(MIGRATION_0003)
            .map_err(|_| StoreError::Migration)?;
    }
    if current_version < 4 {
        transaction
            .execute_batch(MIGRATION_0004)
            .map_err(|_| StoreError::Migration)?;
    }
    if current_version < 5 {
        transaction
            .execute_batch(MIGRATION_0005)
            .map_err(|_| StoreError::Migration)?;
    }
    transaction
        .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
        .map_err(|_| StoreError::Migration)?;
    transaction
        .commit()
        .map_err(|_| StoreError::UncertainOutcome)
}
