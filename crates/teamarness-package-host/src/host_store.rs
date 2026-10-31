use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};

use crate::diag::fault;
use crate::{PackageErrorCode, PackageErrorEnvelope, PackageSourceKind};

#[derive(Debug)]
pub(crate) struct HostStoreFailure {
    pub(crate) code: PackageErrorCode,
}

pub(crate) struct HostStore {
    connection: Connection,
}

/// The one Package Host store, shared by every Package in a Host.
///
/// Opening runs directory checks, permission changes and the migration batch,
/// so it happens once per Host. Every later use crosses to a blocking thread:
/// SQLite is synchronous and must not run on an asynchronous worker.
#[derive(Clone)]
pub(crate) struct HostStoreHandle {
    store: Arc<std::sync::Mutex<HostStore>>,
    #[cfg(test)]
    opens: Arc<std::sync::atomic::AtomicUsize>,
}

impl HostStoreHandle {
    pub(crate) async fn open(private_data_directory: PathBuf) -> Result<Self, HostStoreFailure> {
        let store = tokio::task::spawn_blocking(move || HostStore::open(&private_data_directory))
            .await
            .map_err(fault!(storage_failure()))??;
        Ok(Self {
            store: Arc::new(std::sync::Mutex::new(store)),
            #[cfg(test)]
            opens: Arc::new(std::sync::atomic::AtomicUsize::new(1)),
        })
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&HostStore) -> T + Send + 'static,
    ) -> Result<T, HostStoreFailure> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let store = store.lock().map_err(fault!(storage_failure()))?;
            Ok(f(&store))
        })
        .await
        .map_err(fault!(storage_failure()))?
    }

    /// How many times this Host opened its store. Always one.
    #[cfg(test)]
    pub(crate) fn opens(&self) -> usize {
        self.opens.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessLease {
    pub(crate) package_identity: String,
    pub(crate) workspace_id: String,
    pub(crate) process_generation: u64,
    pub(crate) pid: u32,
    pub(crate) process_group: i32,
    pub(crate) start_time: String,
    pub(crate) canonical_executable: String,
    pub(crate) executable_digest: String,
    pub(crate) launch_token: String,
}

pub(crate) struct StoredAdmission<'a> {
    pub(crate) admission_digest: &'a str,
    pub(crate) package_identity: &'a str,
    pub(crate) package_version: &'a str,
    pub(crate) source: PackageSourceKind,
    pub(crate) manifest_digest: &'a str,
    pub(crate) backend_digest: &'a str,
    pub(crate) ui_contract_digest: &'a str,
}

impl HostStore {
    pub(crate) fn open(private_data_directory: &Path) -> Result<Self, HostStoreFailure> {
        let packages = private_data_directory.join("packages");
        ensure_private_directory(&packages)?;
        let database = packages.join("package-host.sqlite3");
        if let Ok(metadata) = fs::symlink_metadata(&database)
            && (metadata.file_type().is_symlink() || metadata.file_type().is_socket())
        {
            return Err(storage_failure());
        }
        let connection = Connection::open(&database).map_err(fault!(storage_failure()))?;
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600))
            .map_err(fault!(storage_failure()))?;
        connection
            .execute_batch(include_str!("../migrations/0001_package_host.sql"))
            .map_err(fault!(storage_failure()))?;
        connection
            .execute_batch(include_str!("../migrations/0002_port_providers.sql"))
            .map_err(fault!(storage_failure()))?;
        connection
            .execute_batch(include_str!("../migrations/0003_agent_port_selections.sql"))
            .map_err(fault!(storage_failure()))?;
        Ok(Self { connection })
    }

    pub(crate) fn port_selections(
        &self,
        installation: &str,
        workspace: &str,
        package: &str,
    ) -> Result<std::collections::BTreeMap<String, u16>, HostStoreFailure> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT contract_identity, major FROM agent_port_selections
                 WHERE installation_identity = ?1 AND workspace_id = ?2 AND package_identity = ?3
                 ORDER BY contract_identity",
            )
            .map_err(fault!(storage_failure()))?;
        statement
            .query_map(params![installation, workspace, package], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(fault!(storage_failure()))?
            .collect::<Result<_, _>>()
            .map_err(fault!(storage_failure()))
    }

    pub(crate) fn select_port(
        &self,
        installation: &str,
        workspace: &str,
        package: &str,
        contract: &str,
        major: Option<u16>,
    ) -> Result<(), HostStoreFailure> {
        if let Some(major) = major {
            self.connection.execute("INSERT INTO agent_port_selections(installation_identity, workspace_id,
                    package_identity, contract_identity, major) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(installation_identity, workspace_id, package_identity, contract_identity)
                 DO UPDATE SET major = excluded.major", params![installation, workspace, package, contract, major]).map_err(fault!(storage_failure()))?;
        } else {
            self.connection
                .execute(
                    "DELETE FROM agent_port_selections WHERE installation_identity = ?1
                 AND workspace_id = ?2 AND package_identity = ?3 AND contract_identity = ?4",
                    params![installation, workspace, package, contract],
                )
                .map_err(fault!(storage_failure()))?;
        }
        Ok(())
    }

    pub(crate) fn data_directory_mapping(
        &self,
        installation: &str,
        workspace: &str,
        package: &str,
    ) -> Result<Option<(String, String)>, HostStoreFailure> {
        self.connection
            .query_row(
                "SELECT relative_path, continuity_id FROM package_data_directories
                 WHERE installation_identity = ?1 AND workspace_id = ?2 AND package_identity = ?3",
                params![installation, workspace, package],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(fault!(storage_failure()))
    }

    pub(crate) fn record_data_directory(
        &self,
        installation: &str,
        workspace: &str,
        package: &str,
        relative_path: &str,
        continuity_id: &str,
    ) -> Result<(), HostStoreFailure> {
        self.connection
            .execute(
                "INSERT INTO package_data_directories(
                    installation_identity, workspace_id, package_identity, path_version,
                    relative_path, continuity_id
                 ) VALUES (?1, ?2, ?3, 1, ?4, ?5)
                 ON CONFLICT(installation_identity, workspace_id, package_identity) DO NOTHING",
                params![
                    installation,
                    workspace,
                    package,
                    relative_path,
                    continuity_id
                ],
            )
            .map_err(fault!(storage_failure()))?;
        Ok(())
    }

    pub(crate) fn record_admission(
        &self,
        admission: StoredAdmission<'_>,
    ) -> Result<(), HostStoreFailure> {
        let source = match admission.source {
            PackageSourceKind::Bundled => "bundled",
            PackageSourceKind::Local => "local",
        };
        self.connection
            .execute(
                "INSERT OR IGNORE INTO admissions(
                    admission_digest, package_identity, package_version, source_kind,
                    manifest_digest, backend_digest, ui_contract_digest, admitted_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                params![
                    admission.admission_digest,
                    admission.package_identity,
                    admission.package_version,
                    source,
                    admission.manifest_digest,
                    admission.backend_digest,
                    admission.ui_contract_digest
                ],
            )
            .map_err(fault!(storage_failure()))?;
        Ok(())
    }

    pub(crate) fn port_providers(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, HostStoreFailure> {
        let mut statement = self.connection.prepare(
            "SELECT contract_identity, package_identity FROM port_providers ORDER BY contract_identity"
        ).map_err(fault!(storage_failure()))?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(fault!(storage_failure()))?
            .collect::<Result<_, _>>()
            .map_err(fault!(storage_failure()))
    }

    pub(crate) fn record_port_provider(
        &self,
        contract: &str,
        package: &str,
    ) -> Result<(), HostStoreFailure> {
        self.connection.execute(
            "INSERT INTO port_providers(contract_identity, package_identity) VALUES (?1, ?2)
             ON CONFLICT(contract_identity) DO UPDATE SET package_identity = excluded.package_identity",
            params![contract, package],
        ).map_err(fault!(storage_failure()))?;
        Ok(())
    }

    pub(crate) fn process_leases(&self) -> Result<Vec<ProcessLease>, HostStoreFailure> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT package_identity, workspace_id, process_generation, pid, process_group,
                        start_time, canonical_executable, executable_digest, launch_token
                 FROM process_leases ORDER BY package_identity, workspace_id",
            )
            .map_err(fault!(storage_failure()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(ProcessLease {
                    package_identity: row.get(0)?,
                    workspace_id: row.get(1)?,
                    process_generation: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                    pid: row.get(3)?,
                    process_group: row.get(4)?,
                    start_time: row.get(5)?,
                    canonical_executable: row.get(6)?,
                    executable_digest: row.get(7)?,
                    launch_token: row.get(8)?,
                })
            })
            .map_err(fault!(storage_failure()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(fault!(storage_failure()))
    }

    pub(crate) fn record_process_lease(
        &self,
        lease: &ProcessLease,
    ) -> Result<(), HostStoreFailure> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO process_leases(
                    package_identity, workspace_id, process_generation, pid, process_group,
                    start_time, canonical_executable, executable_digest, launch_token
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    lease.package_identity,
                    lease.workspace_id,
                    i64::try_from(lease.process_generation).map_err(|_| storage_failure())?,
                    lease.pid,
                    lease.process_group,
                    lease.start_time,
                    lease.canonical_executable,
                    lease.executable_digest,
                    lease.launch_token,
                ],
            )
            .map_err(fault!(storage_failure()))?;
        Ok(())
    }

    pub(crate) fn delete_process_lease(
        &self,
        package: &str,
        workspace: &str,
    ) -> Result<(), HostStoreFailure> {
        self.connection
            .execute(
                "DELETE FROM process_leases WHERE package_identity = ?1 AND workspace_id = ?2",
                params![package, workspace],
            )
            .map_err(fault!(storage_failure()))?;
        Ok(())
    }

    pub(crate) fn record_diagnostic(
        &self,
        error: &PackageErrorEnvelope,
    ) -> Result<(), HostStoreFailure> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO diagnostics(
                    diagnostic_id, code, stage, package_identity, observed_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, 0)",
                params![
                    error.diagnostic_id.as_str(),
                    format!("{:?}", error.code),
                    format!("{:?}", error.stage),
                    error.affected_package.as_ref().map(ToString::to_string),
                ],
            )
            .map_err(fault!(storage_failure()))?;
        Ok(())
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), HostStoreFailure> {
    let expected_owner = path
        .parent()
        .and_then(|parent| fs::metadata(parent).ok())
        .map(|metadata| metadata.uid());
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(fault!(storage_failure()))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || expected_owner.is_some_and(|owner| metadata.uid() != owner)
        {
            return Err(storage_failure());
        }
    } else {
        fs::create_dir_all(path).map_err(fault!(storage_failure()))?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(fault!(storage_failure()))
}

const fn storage_failure() -> HostStoreFailure {
    HostStoreFailure {
        code: PackageErrorCode::DataDirectoryUnavailable,
    }
}
