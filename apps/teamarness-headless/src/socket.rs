//! Unix endpoint policy. Peer credentials use Tokio's Linux SO_PEERCRED and macOS getpeereid adapters.
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use crate::{ControlError, ErrorCode};

pub(crate) fn path(directory: &Path) -> Result<PathBuf, ControlError> {
    let path = directory.join("control.sock");
    // sockaddr_un.sun_path is 104 bytes on macOS and 108 on Linux, including NUL.
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
    if path.as_os_str().as_encoded_bytes().len() >= limit {
        return Err(ControlError::new(ErrorCode::UnsafePath));
    }
    Ok(path)
}

pub(crate) fn validate_directory(directory: &Path) -> Result<(), ControlError> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
    if !metadata.is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(ControlError::new(ErrorCode::UnsafePath));
    }
    Ok(())
}

fn socket_metadata(path: &Path) -> Result<std::fs::Metadata, ControlError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(ControlError::new(ErrorCode::UnsafePath));
    }
    Ok(metadata)
}

pub(crate) fn validate_endpoint(directory: &Path) -> Result<PathBuf, ControlError> {
    validate_directory(directory)?;
    let path = path(directory)?;
    socket_metadata(&path)?;
    Ok(path)
}

pub(crate) fn check_peer(stream: &UnixStream) -> Result<(), ControlError> {
    let credential = stream
        .peer_cred()
        .map_err(|_| ControlError::new(ErrorCode::UnauthorizedPeer))?;
    same_user(credential.uid())
}

fn same_user(uid: u32) -> Result<(), ControlError> {
    if uid != nix::unistd::geteuid().as_raw() {
        return Err(ControlError::new(ErrorCode::UnauthorizedPeer));
    }
    Ok(())
}

/// Created only through an Application which already owns the Core lock.
pub(crate) struct Endpoint {
    pub listener: Option<UnixListener>,
    path: PathBuf,
    identity: (u64, u64),
}

impl Endpoint {
    pub async fn bind(directory: &Path) -> Result<Self, ControlError> {
        validate_directory(directory)?;
        let path = path(directory)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                socket_metadata(&path)?;
                match tokio::time::timeout(crate::control::IO_TIMEOUT, UnixStream::connect(&path))
                    .await
                {
                    Ok(Err(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                        ) => {}
                    _ => return Err(ControlError::new(ErrorCode::Unavailable)),
                }
                socket_metadata(&path)?;
                std::fs::remove_file(&path)
                    .map_err(|_| ControlError::new(ErrorCode::UnsafePath))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ControlError::new(ErrorCode::UnsafePath)),
        }
        let listener =
            UnixListener::bind(&path).map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| ControlError::new(ErrorCode::UnsafePath))?;
        let metadata = socket_metadata(&path)?;
        Ok(Self {
            listener: Some(listener),
            path,
            identity: (metadata.dev(), metadata.ino()),
        })
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // Never unlink a replacement or unsafe occupant during cleanup.
        if let Ok(metadata) = socket_metadata(&self.path)
            && self.identity == (metadata.dev(), metadata.ino())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_peer_credentials_and_foreign_uid_refusal() {
        let (left, right) = UnixStream::pair().unwrap();
        assert_eq!(
            left.peer_cred().unwrap().uid(),
            nix::unistd::geteuid().as_raw()
        );
        check_peer(&left).unwrap();
        check_peer(&right).unwrap();
        assert!(same_user(nix::unistd::geteuid().as_raw().wrapping_add(1)).is_err());
    }
}
