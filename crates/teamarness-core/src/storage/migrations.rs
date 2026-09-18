use rusqlite::{Connection, TransactionBehavior};

use crate::error::StorageError;

const LATEST_SCHEMA_VERSION: i64 = 11;
const MIGRATION_0001: &str = include_str!("../../migrations/0001_core.sql");
const MIGRATION_0002: &str = include_str!("../../migrations/0002_workspaces_membership.sql");
const MIGRATION_0003: &str = include_str!("../../migrations/0003_invitations.sql");
const MIGRATION_0004: &str = include_str!("../../migrations/0004_transitions.sql");
const MIGRATION_0005: &str = include_str!("../../migrations/0005_invitation_recovery.sql");
const MIGRATION_0006: &str = include_str!("../../migrations/0006_immutable_record_log.sql");
const MIGRATION_0007: &str = include_str!("../../migrations/0007_p2panda_operation_projection.sql");
const MIGRATION_0008: &str = include_str!("../../migrations/0008_protection_controls.sql");
const MIGRATION_0009: &str = include_str!("../../migrations/0009_synchronization_worker.sql");
const MIGRATION_0010: &str = include_str!("../../migrations/0010_membership_replication.sql");
const MIGRATION_0011: &str = include_str!("../../migrations/0011_sync_bounds_and_recovery.sql");

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
    }
    if current_version < 2 {
        transaction
            .execute_batch(MIGRATION_0002)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 3 {
        transaction
            .execute_batch(MIGRATION_0003)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 4 {
        transaction
            .execute_batch(MIGRATION_0004)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 5 {
        transaction
            .execute_batch(MIGRATION_0005)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 6 {
        transaction
            .execute_batch(MIGRATION_0006)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 7 {
        transaction
            .execute_batch(MIGRATION_0007)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 8 {
        transaction
            .execute_batch(MIGRATION_0008)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 9 {
        transaction
            .execute_batch(MIGRATION_0009)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 10 {
        transaction
            .execute_batch(MIGRATION_0010)
            .map_err(|_| StorageError::Migration)?;
    }
    if current_version < 11 {
        transaction
            .execute_batch(MIGRATION_0011)
            .map_err(|_| StorageError::Migration)?;
    }
    transaction
        .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
        .map_err(|_| StorageError::Migration)?;

    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)
}
