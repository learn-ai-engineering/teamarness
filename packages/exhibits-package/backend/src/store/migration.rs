use rusqlite::{Connection, TransactionBehavior};

const LATEST_SCHEMA_VERSION: i64 = 1;
const MIGRATION_0001: &str = include_str!("../../migrations/0001_exhibits.sql");

/// Why the schema could not be brought to the latest version.
#[derive(Debug)]
pub enum MigrationError {
    Storage(rusqlite::Error),
    /// The database was written by a newer version of this Package.
    UnsupportedSchema(i64),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(cause) => write!(f, "{cause}"),
            Self::UnsupportedSchema(version) => write!(f, "unsupported schema version {version}"),
        }
    }
}

pub(super) fn apply(connection: &mut Connection) -> Result<(), MigrationError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(MigrationError::Storage)?;
    let current = transaction
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(MigrationError::Storage)?;
    if current > LATEST_SCHEMA_VERSION {
        return Err(MigrationError::UnsupportedSchema(current));
    }
    if current < 1 {
        transaction
            .execute_batch(MIGRATION_0001)
            .map_err(MigrationError::Storage)?;
    }
    transaction
        .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
        .map_err(MigrationError::Storage)?;
    transaction.commit().map_err(MigrationError::Storage)
}
