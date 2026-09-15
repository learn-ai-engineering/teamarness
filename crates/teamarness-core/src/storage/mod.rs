mod migrations;

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::Duration;

use iroh_base::SecretKey;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::StorageError;
use crate::identity::DEFAULT_DISPLAY_NAME;

pub(crate) const DATABASE_FILE_NAME: &str = "core.sqlite3";

pub(crate) struct StoredRuntime {
    pub(crate) connection: Connection,
    pub(crate) secret_key: SecretKey,
    pub(crate) display_name: String,
}

#[derive(Clone, Copy)]
pub(crate) enum IdentityTransactionBoundary {
    Before,
    AfterCommit,
}

pub(crate) fn open(private_data_directory: &Path) -> Result<StoredRuntime, StorageError> {
    open_inner(private_data_directory, |_| Ok(()))
}

#[cfg(test)]
pub(crate) fn open_with_fault(
    private_data_directory: &Path,
    fault: impl FnMut(IdentityTransactionBoundary) -> Result<(), StorageError>,
) -> Result<StoredRuntime, StorageError> {
    open_inner(private_data_directory, fault)
}

fn open_inner(
    private_data_directory: &Path,
    mut fault: impl FnMut(IdentityTransactionBoundary) -> Result<(), StorageError>,
) -> Result<StoredRuntime, StorageError> {
    prepare_private_directory(private_data_directory)?;
    let database_path = private_data_directory.join(DATABASE_FILE_NAME);
    prepare_private_database_file(&database_path)?;

    let mut connection = Connection::open(&database_path).map_err(|_| StorageError::Database)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| StorageError::Database)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| StorageError::Database)?;
    migrations::apply(&mut connection)?;

    fault(IdentityTransactionBoundary::Before)?;
    let (secret_key, display_name) = initialize_identity(&mut connection)?;
    fault(IdentityTransactionBoundary::AfterCommit)?;
    authenticate_local_subject(&connection, &secret_key)?;
    crate::workspace::recover_pending(&mut connection)?;
    crate::synchronization::recover(&mut connection, &secret_key)?;

    Ok(StoredRuntime {
        connection,
        secret_key,
        display_name,
    })
}

fn load_identity(connection: &Connection) -> Result<Option<(SecretKey, String)>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT secret_key, display_name
             FROM installation_identity
             WHERE singleton_id = 1",
            [],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|_| StorageError::Database)?;

    stored
        .map(|(secret_bytes, display_name)| {
            let bytes: [u8; 32] = secret_bytes
                .try_into()
                .map_err(|_| StorageError::CorruptIdentity)?;
            crate::identity::validate_display_name(&display_name)
                .map_err(|_| StorageError::CorruptIdentity)?;
            Ok((SecretKey::from_bytes(&bytes), display_name))
        })
        .transpose()
}

fn initialize_identity(connection: &mut Connection) -> Result<(SecretKey, String), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::Database)?;
    let stored = load_identity(&transaction)?;
    let identity = match stored {
        Some(identity) => identity,
        None => {
            let secret_key = SecretKey::generate();
            let public_key = secret_key.public();
            transaction
                .execute(
                    "INSERT INTO installation_identity (singleton_id, secret_key, display_name)
                     VALUES (1, ?1, ?2)",
                    params![secret_key.to_bytes().as_slice(), DEFAULT_DISPLAY_NAME],
                )
                .map_err(|_| StorageError::Database)?;
            transaction
                .execute(
                    "INSERT INTO identity_subjects (
                        installation_public_identity,
                        human_identity_id
                     ) VALUES (?1, NULL)",
                    params![public_key.as_bytes().as_slice()],
                )
                .map_err(|_| StorageError::Database)?;
            (secret_key, DEFAULT_DISPLAY_NAME.to_owned())
        }
    };
    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)?;
    Ok(identity)
}

fn authenticate_local_subject(
    connection: &Connection,
    secret_key: &SecretKey,
) -> Result<(), StorageError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM identity_subjects
             WHERE installation_public_identity = ?1",
            params![secret_key.public().as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::Database)?;
    if count != 1 {
        return Err(StorageError::CorruptIdentity);
    }
    Ok(())
}

pub(crate) fn update_display_name(
    connection: &mut Connection,
    display_name: &str,
) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StorageError::Database)?;
    let updated = transaction
        .execute(
            "UPDATE installation_identity SET display_name = ?1 WHERE singleton_id = 1",
            [display_name],
        )
        .map_err(|_| StorageError::Database)?;
    if updated != 1 {
        return Err(StorageError::CorruptIdentity);
    }
    transaction
        .commit()
        .map_err(|_| StorageError::UncertainOutcome)
}

fn prepare_private_directory(path: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(path).map_err(|_| StorageError::Io)?;
    if !fs::metadata(path).map_err(|_| StorageError::Io)?.is_dir() {
        return Err(StorageError::InvalidDataDirectory);
    }
    set_directory_permissions(path)
}

fn prepare_private_database_file(path: &Path) -> Result<(), StorageError> {
    create_database_file(path)?;
    set_database_permissions(path)
}

#[cfg(unix)]
fn create_database_file(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|_| StorageError::Io)
}

#[cfg(not(unix))]
fn create_database_file(path: &Path) -> Result<(), StorageError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_| ())
        .map_err(|_| StorageError::Io)
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| StorageError::Permission)
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(unix)]
fn set_database_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| StorageError::Permission)
}

#[cfg(not(unix))]
fn set_database_permissions(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "teamarness-storage-{name}-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn interruption_before_identity_transaction_leaves_no_identity() {
        let directory = test_directory("before-identity");
        let result = open_with_fault(&directory, |boundary| match boundary {
            IdentityTransactionBoundary::Before => Err(StorageError::Database),
            IdentityTransactionBoundary::AfterCommit => Ok(()),
        });
        assert!(matches!(result, Err(StorageError::Database)));

        let connection = Connection::open(directory.join(DATABASE_FILE_NAME)).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM installation_identity", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        drop(connection);

        let reopened = open(&directory).unwrap();
        authenticate_local_subject(&reopened.connection, &reopened.secret_key).unwrap();
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interruption_after_identity_commit_reloads_the_committed_identity() {
        let directory = test_directory("after-identity");
        let result = open_with_fault(&directory, |boundary| match boundary {
            IdentityTransactionBoundary::Before => Ok(()),
            IdentityTransactionBoundary::AfterCommit => Err(StorageError::UncertainOutcome),
        });
        assert!(matches!(result, Err(StorageError::UncertainOutcome)));

        let reopened = open(&directory).unwrap();
        let identity = reopened.secret_key.public();
        drop(reopened);
        let reopened_again = open(&directory).unwrap();
        assert_eq!(reopened_again.secret_key.public(), identity);
        drop(reopened_again);
        fs::remove_dir_all(directory).unwrap();
    }
}
