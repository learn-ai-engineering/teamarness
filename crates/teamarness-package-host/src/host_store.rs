use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{PackageErrorCode, PackageErrorEnvelope, PackageSourceKind};

#[derive(Debug)]
pub(crate) struct HostStoreFailure {
    pub(crate) code: PackageErrorCode,
}

pub(crate) struct HostStore {
    connection: Connection,
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
        let connection = Connection::open(&database).map_err(|_| storage_failure())?;
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600))
            .map_err(|_| storage_failure())?;
        connection
            .execute_batch(include_str!("../migrations/0001_package_host.sql"))
            .map_err(|_| storage_failure())?;
        Ok(Self { connection })
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
            .map_err(|_| storage_failure())
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
            .map_err(|_| storage_failure())?;
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
            .map_err(|_| storage_failure())?;
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
            .map_err(|_| storage_failure())?;
        Ok(())
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), HostStoreFailure> {
    let expected_owner = path
        .parent()
        .and_then(|parent| fs::metadata(parent).ok())
        .map(|metadata| metadata.uid());
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|_| storage_failure())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || expected_owner.is_some_and(|owner| metadata.uid() != owner)
        {
            return Err(storage_failure());
        }
    } else {
        fs::create_dir_all(path).map_err(|_| storage_failure())?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| storage_failure())
}

const fn storage_failure() -> HostStoreFailure {
    HostStoreFailure {
        code: PackageErrorCode::DataDirectoryUnavailable,
    }
}
