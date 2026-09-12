use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{CoreRuntime, CoreRuntimeConfig};
use rusqlite::{Connection, OptionalExtension, params};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn migration_0001_reserves_nullable_human_identity_subjects() {
    let directory = private_data_directory("migration-human-identity");
    CoreRuntime::open(CoreRuntimeConfig::new(&directory)).unwrap();
    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();

    let human_column: Option<(i64, String)> = connection
        .query_row(
            "SELECT [notnull], COALESCE(dflt_value, 'NULL')
             FROM pragma_table_info('identity_subjects')
             WHERE name = 'human_identity_id'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap();
    assert_eq!(human_column, Some((0, "NULL".to_owned())));

    let local_subject: (i64, Option<Vec<u8>>) = connection
        .query_row(
            "SELECT COUNT(*), human_identity_id FROM identity_subjects",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(local_subject, (1, None));

    connection
        .execute(
            "INSERT INTO identity_subjects (
                installation_public_identity,
                human_identity_id
             ) VALUES (?1, NULL)",
            params![[7_u8; 32].as_slice()],
        )
        .unwrap();
    let ungrouped_subjects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM identity_subjects WHERE human_identity_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ungrouped_subjects, 2);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ordered_migration_is_idempotent_on_reopen() {
    let directory = private_data_directory("migration-reopen");
    let config = CoreRuntimeConfig::new(&directory);

    CoreRuntime::open(config.clone()).unwrap();
    CoreRuntime::open(config).unwrap();

    let connection = Connection::open(directory.join("core.sqlite3")).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let installation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM installation_identity", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(installation_count, 1);

    drop(connection);
    fs::remove_dir_all(directory).unwrap();
}
