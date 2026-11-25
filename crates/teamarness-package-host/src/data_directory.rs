use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::diag::fault;
use crate::host_store::HostStore;
use crate::manifest::sha256;
use crate::{InstallationPublicIdentity, PackageErrorCode, PackageIdentity, WorkspaceId};

#[derive(Debug)]
pub(crate) struct DataDirectoryFailure {
    pub(crate) code: PackageErrorCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataDirectoryGrant {
    pub(crate) path: PathBuf,
    pub(crate) continuity_id: String,
}

pub(crate) fn allocate(
    store: &HostStore,
    private_data_directory: &Path,
    installation: &InstallationPublicIdentity,
    workspace: &WorkspaceId,
    package: &PackageIdentity,
) -> Result<DataDirectoryGrant, DataDirectoryFailure> {
    let mapping = store
        .data_directory_mapping(installation.as_str(), workspace.as_str(), package.as_str())
        .map_err(|_| unavailable())?;
    let (relative_path, continuity_id) = mapping.unwrap_or_else(|| {
        let relative_path = format!(
            "packages/data/v1/{}/{}/{}",
            installation.as_str(),
            workspace.as_str(),
            sha256(package.as_str().as_bytes())
        );
        let continuity_digest = sha256(
            format!(
                "{}\0{}\0{}",
                installation.as_str(),
                workspace.as_str(),
                package.as_str()
            )
            .as_bytes(),
        );
        let continuity_id = URL_SAFE_NO_PAD.encode(continuity_digest.as_bytes());
        (relative_path, continuity_id)
    });
    let relative = validate_relative_mapping(&relative_path)?;
    let path = private_data_directory.join(relative);
    create_private_tree(private_data_directory, &path)?;
    protocol_path(&path)?;
    store
        .record_data_directory(
            installation.as_str(),
            workspace.as_str(),
            package.as_str(),
            &relative_path,
            &continuity_id,
        )
        .map_err(|_| unavailable())?;
    Ok(DataDirectoryGrant {
        path,
        continuity_id,
    })
}

pub(crate) fn protocol_path(path: &Path) -> Result<&str, DataDirectoryFailure> {
    path.to_str().ok_or_else(unrepresentable)
}

fn validate_relative_mapping(value: &str) -> Result<PathBuf, DataDirectoryFailure> {
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(permission());
    }
    Ok(path)
}

fn create_private_tree(root: &Path, leaf: &Path) -> Result<(), DataDirectoryFailure> {
    let expected_owner = fs::metadata(root).map_err(fault!(unavailable()))?.uid();
    let mut current = root.to_owned();
    let relative = leaf.strip_prefix(root).map_err(fault!(permission()))?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(permission());
        };
        current.push(component);
        if current.exists() {
            let metadata = fs::symlink_metadata(&current).map_err(fault!(unavailable()))?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != expected_owner
            {
                return Err(permission());
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(permission());
            }
        } else {
            fs::create_dir(&current).map_err(fault!(unavailable()))?;
            fs::set_permissions(&current, fs::Permissions::from_mode(0o700))
                .map_err(fault!(unavailable()))?;
        }
    }
    Ok(())
}

const fn unavailable() -> DataDirectoryFailure {
    DataDirectoryFailure {
        code: PackageErrorCode::DataDirectoryUnavailable,
    }
}

const fn permission() -> DataDirectoryFailure {
    DataDirectoryFailure {
        code: PackageErrorCode::DataDirectoryPermission,
    }
}

const fn unrepresentable() -> DataDirectoryFailure {
    DataDirectoryFailure {
        code: PackageErrorCode::DataDirectoryUnrepresentable,
    }
}

/// Removes a deleted Workspace's Package Data Directories and then its Host rows (ADR 0048).
///
/// Paths come from the stored mappings, not recomputed, and are validated like an allocation.
/// Removal never follows a symbolic link, so a folder a Package linked to (such as a mapped
/// Documents folder) is left as it is. A recorded process lease refuses: a process may be live.
pub(crate) fn remove_workspace_data(
    store: &HostStore,
    private_data_directory: &Path,
    installation: &InstallationPublicIdentity,
    workspace: &WorkspaceId,
) -> Result<(), DataDirectoryFailure> {
    if store
        .workspace_has_process_leases(workspace.as_str())
        .map_err(|failure| DataDirectoryFailure { code: failure.code })?
    {
        return Err(unavailable());
    }
    let mappings = store
        .workspace_data_directories(installation.as_str(), workspace.as_str())
        .map_err(|failure| DataDirectoryFailure { code: failure.code })?;
    for relative_path in mappings {
        let relative = validate_relative_mapping(&relative_path)?;
        remove_tree(private_data_directory, &relative)?;
    }
    let default_root = validate_relative_mapping(&format!(
        "packages/data/v1/{}/{}",
        installation.as_str(),
        workspace.as_str()
    ))?;
    remove_tree(private_data_directory, &default_root)?;
    store
        .forget_workspace(installation.as_str(), workspace.as_str())
        .map_err(|failure| DataDirectoryFailure { code: failure.code })
}

/// Removes `root/relative` without following a link at it, inside it, or at any directory
/// between `root` and it. Only Package data paths qualify. A missing path is already removed.
fn remove_tree(root: &Path, relative: &Path) -> Result<(), DataDirectoryFailure> {
    if !relative.starts_with("packages/data") {
        return Err(permission());
    }
    let mut ancestor = root.to_owned();
    let parents: Vec<_> = relative.components().collect();
    for component in &parents[..parents.len().saturating_sub(1)] {
        ancestor.push(component);
        match fs::symlink_metadata(&ancestor) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(unavailable()),
            Ok(metadata) if !metadata.is_dir() => return Err(permission()),
            Ok(_) => {}
        }
    }
    let path = root.join(relative);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(unavailable()),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(&path).map_err(fault!(unavailable()))
        }
        Ok(_) => fs::remove_file(&path).map_err(fault!(unavailable())),
    }
}
