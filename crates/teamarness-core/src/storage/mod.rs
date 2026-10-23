mod migrations;

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::Duration;

use iroh_base::SecretKey;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use zeroize::Zeroizing;

use crate::diag::fault;
use crate::error::StorageError;
use crate::identity::DEFAULT_DISPLAY_NAME;

pub(crate) const DATABASE_FILE_NAME: &str = "core.sqlite3";

/// Marks a directory as one the Core Runtime owns, so a directory holding
/// unrelated files is never adopted or restricted.
pub(crate) const PRIVATE_DIRECTORY_MARKER: &str = ".teamarness-private-directory";
#[cfg_attr(not(unix), expect(dead_code, reason = "the marker is Unix-only"))]
const PRIVATE_DIRECTORY_MARKER_CONTENT: &[u8] = b"teamarness private data directory v1\n";

pub(crate) struct StoredRuntime {
    pub(crate) connection: Connection,
    pub(crate) secret_key: SecretKey,
    pub(crate) display_name: String,
    /// What restart validation checked and isolated, for tests and diagnostics.
    pub(crate) recovery: crate::synchronization::RecoveryReport,
}

#[derive(Clone, Copy)]
pub(crate) enum IdentityTransactionBoundary {
    Before,
    AfterCommit,
}

pub(crate) fn open(
    private_data_directory: &Path,
    now_unix_ms: i64,
) -> Result<StoredRuntime, StorageError> {
    open_inner(private_data_directory, now_unix_ms, |_| Ok(()))
}

#[cfg(test)]
pub(crate) fn open_with_fault(
    private_data_directory: &Path,
    now_unix_ms: i64,
    fault: impl FnMut(IdentityTransactionBoundary) -> Result<(), StorageError>,
) -> Result<StoredRuntime, StorageError> {
    open_inner(private_data_directory, now_unix_ms, fault)
}

/// A migrated, empty schema in memory, for tests that exercise one statement
/// rather than a whole runtime. Foreign keys stay off, so a test writes the one
/// table it is about.
#[cfg(test)]
pub(crate) fn memory_schema_for_test() -> Connection {
    let mut connection = Connection::open_in_memory().expect("an in-memory database opens");
    migrations::apply(&mut connection).expect("the migration batch applies");
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("foreign keys can be turned off");
    connection
}

fn open_inner(
    private_data_directory: &Path,
    now_unix_ms: i64,
    mut fault: impl FnMut(IdentityTransactionBoundary) -> Result<(), StorageError>,
) -> Result<StoredRuntime, StorageError> {
    prepare_private_directory(private_data_directory)?;
    let database_path = private_data_directory.join(DATABASE_FILE_NAME);
    prepare_private_database_file(&database_path)?;

    let mut connection =
        Connection::open(&database_path).map_err(fault!(StorageError::Database))?;
    // ADR 0017: freed pages are overwritten, so a dropped column's key bytes
    // and a deleted row's contents do not survive on disk. Set before the
    // migrations run, because migration 0012 frees exactly such pages.
    connection
        .pragma_update(None, "secure_delete", "ON")
        .map_err(fault!(StorageError::Database))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(fault!(StorageError::Database))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(fault!(StorageError::Database))?;
    migrations::apply(&mut connection)?;

    fault(IdentityTransactionBoundary::Before)?;
    let (secret_key, display_name) = initialize_identity(&mut connection)?;
    fault(IdentityTransactionBoundary::AfterCommit)?;
    authenticate_local_subject(&connection, &secret_key)?;
    crate::workspace::recover_pending(&mut connection, &secret_key)?;
    let recovery = crate::synchronization::recover(&mut connection, &secret_key, now_unix_ms)?;

    Ok(StoredRuntime {
        connection,
        secret_key,
        display_name,
        recovery,
    })
}

fn load_identity(connection: &Connection) -> Result<Option<(SecretKey, String)>, StorageError> {
    let stored = connection
        .query_row(
            "SELECT secret_key, display_name
             FROM installation_identity
             WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    // The seed leaves SQLite straight into a zeroizing buffer.
                    Zeroizing::new(row.get::<_, Vec<u8>>(0)?),
                    row.get::<_, String>(1)?,
                ))
            },
        )
        .optional()
        .map_err(fault!(StorageError::Database))?;

    stored
        .map(|(secret_bytes, display_name)| {
            if secret_bytes.len() != 32 {
                return Err(StorageError::CorruptIdentity);
            }
            let mut bytes = Zeroizing::new([0_u8; 32]);
            bytes.copy_from_slice(secret_bytes.as_slice());
            crate::identity::validate_display_name(&display_name)
                .map_err(|_| StorageError::CorruptIdentity)?;
            Ok((SecretKey::from_bytes(&bytes), display_name))
        })
        .transpose()
}

fn initialize_identity(connection: &mut Connection) -> Result<(SecretKey, String), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(fault!(StorageError::Database))?;
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
                    params![
                        Zeroizing::new(secret_key.to_bytes()).as_slice(),
                        DEFAULT_DISPLAY_NAME
                    ],
                )
                .map_err(fault!(StorageError::Database))?;
            transaction
                .execute(
                    "INSERT INTO identity_subjects (
                        installation_public_identity,
                        human_identity_id
                     ) VALUES (?1, NULL)",
                    params![public_key.as_bytes().as_slice()],
                )
                .map_err(fault!(StorageError::Database))?;
            (secret_key, DEFAULT_DISPLAY_NAME.to_owned())
        }
    };
    transaction
        .commit()
        .map_err(fault!(StorageError::UncertainOutcome))?;
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
        .map_err(fault!(StorageError::Database))?;
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
        .map_err(fault!(StorageError::Database))?;
    let updated = transaction
        .execute(
            "UPDATE installation_identity SET display_name = ?1 WHERE singleton_id = 1",
            [display_name],
        )
        .map_err(fault!(StorageError::Database))?;
    if updated != 1 {
        return Err(StorageError::CorruptIdentity);
    }
    transaction
        .commit()
        .map_err(fault!(StorageError::UncertainOutcome))
}

/// Decides whether a path may be used as the private data directory, and
/// whether the marker still has to be written.
///
/// A pure predicate so the rule can be tested without `chown`. `Ok(true)`
/// means the directory is acceptable and needs its marker; `Ok(false)` means
/// it is already marked.
#[cfg_attr(not(unix), expect(dead_code, reason = "the rule is Unix-only"))]
fn directory_acceptable(
    is_symlink: bool,
    is_dir: bool,
    owner_uid: u32,
    effective_uid: u32,
    has_marker: bool,
    is_empty: bool,
    has_core_database: bool,
) -> Result<bool, StorageError> {
    if is_symlink || !is_dir || owner_uid != effective_uid {
        return Err(StorageError::InvalidDataDirectory);
    }
    if has_marker {
        return Ok(false);
    }
    if is_empty || has_core_database {
        return Ok(true);
    }
    Err(StorageError::InvalidDataDirectory)
}

/// Accepts a private data directory, or refuses the path.
///
/// Only the final path component is checked for a symbolic link: macOS
/// temporary paths reach through system links such as `/var` to
/// `/private/var`, so an ancestor link is normal and is not a signal.
#[cfg(unix)]
fn prepare_private_directory(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::MetadataExt;

    let write_marker = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let is_symlink = file_type.is_symlink();
            let is_dir = file_type.is_dir();
            let (has_marker, is_empty, has_core_database) = if is_dir {
                (
                    path.join(PRIVATE_DIRECTORY_MARKER).exists(),
                    fs::read_dir(path)
                        .map_err(fault!(StorageError::Io))?
                        .next()
                        .is_none(),
                    path.join(DATABASE_FILE_NAME).exists(),
                )
            } else {
                (false, false, false)
            };
            directory_acceptable(
                is_symlink,
                is_dir,
                metadata.uid(),
                nix::unistd::geteuid().as_raw(),
                has_marker,
                is_empty,
                has_core_database,
            )?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(fault!(StorageError::Io))?;
            true
        }
        Err(_) => return Err(StorageError::Io),
    };

    set_directory_permissions(path)?;
    if write_marker {
        write_private_directory_marker(path)?;
    }
    Ok(())
}

/// Fails closed where the directory cannot be restricted to its owner.
#[cfg(not(unix))]
fn prepare_private_directory(_path: &Path) -> Result<(), StorageError> {
    Err(StorageError::Permission)
}

#[cfg(unix)]
fn write_private_directory_marker(path: &Path) -> Result<(), StorageError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path.join(PRIVATE_DIRECTORY_MARKER))
    {
        Ok(mut marker) => marker
            .write_all(PRIVATE_DIRECTORY_MARKER_CONTENT)
            .map_err(fault!(StorageError::Io)),
        // Another runtime opening the same directory wrote it first.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(StorageError::Io),
    }
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
        .map_err(fault!(StorageError::Io))
}

#[cfg(not(unix))]
fn create_database_file(path: &Path) -> Result<(), StorageError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(|_| ())
        .map_err(fault!(StorageError::Io))
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(fault!(StorageError::Permission))
}

#[cfg(unix)]
fn set_database_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(fault!(StorageError::Permission))
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
        let result = open_with_fault(&directory, 0, |boundary| match boundary {
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

        let reopened = open(&directory, 0).unwrap();
        authenticate_local_subject(&reopened.connection, &reopened.secret_key).unwrap();
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interruption_after_identity_commit_reloads_the_committed_identity() {
        let directory = test_directory("after-identity");
        let result = open_with_fault(&directory, 0, |boundary| match boundary {
            IdentityTransactionBoundary::Before => Ok(()),
            IdentityTransactionBoundary::AfterCommit => Err(StorageError::UncertainOutcome),
        });
        assert!(matches!(result, Err(StorageError::UncertainOutcome)));

        let reopened = open(&directory, 0).unwrap();
        let identity = reopened.secret_key.public();
        drop(reopened);
        let reopened_again = open(&directory, 0).unwrap();
        assert_eq!(reopened_again.secret_key.public(), identity);
        drop(reopened_again);
        fs::remove_dir_all(directory).unwrap();
    }

    // Verifies: TEAM.SYS.ID-R07
    #[test]
    fn secure_delete_is_enabled_on_open() {
        let directory = test_directory("secure-delete");
        let stored = open(&directory, 0).unwrap();

        let secure_delete: i64 = stored
            .connection
            .pragma_query_value(None, "secure_delete", |row| row.get(0))
            .unwrap();

        assert_eq!(secure_delete, 1);
        drop(stored);
        fs::remove_dir_all(directory).unwrap();
    }

    // Verifies: TEAM.SYS.ID-R07
    #[test]
    fn directory_predicate_rejects_symlink_foreign_owner_and_unrelated_contents() {
        // A symbolic link, even one pointing at a marked directory of ours.
        assert_eq!(
            directory_acceptable(true, false, 501, 501, true, false, true),
            Err(StorageError::InvalidDataDirectory)
        );
        // A regular file where the directory should be.
        assert_eq!(
            directory_acceptable(false, false, 501, 501, false, false, false),
            Err(StorageError::InvalidDataDirectory)
        );
        // Another user's directory, however well marked.
        assert_eq!(
            directory_acceptable(false, true, 502, 501, true, false, true),
            Err(StorageError::InvalidDataDirectory)
        );
        // Somebody's home directory: ours to neither adopt nor chmod.
        assert_eq!(
            directory_acceptable(false, true, 501, 501, false, false, false),
            Err(StorageError::InvalidDataDirectory)
        );
    }

    // Verifies: TEAM.SYS.ID-R07
    #[test]
    fn directory_predicate_accepts_marker_empty_and_legacy_core_database() {
        // Already marked: accepted without rewriting the marker.
        assert_eq!(
            directory_acceptable(false, true, 501, 501, true, false, true),
            Ok(false)
        );
        // Empty: accepted, and marked.
        assert_eq!(
            directory_acceptable(false, true, 501, 501, false, true, false),
            Ok(true)
        );
        // Holds a core database but lost its marker: adopted, and re-marked.
        assert_eq!(
            directory_acceptable(false, true, 501, 501, false, false, true),
            Ok(true)
        );
    }
}
