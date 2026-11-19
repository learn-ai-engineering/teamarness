//! Stable kernel ownership, separate from SQLite's transactional writer locks.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use crate::StorageError;
use crate::diag::fault;

pub(super) struct ApplicationOwnership {
    // Rust opens files close-on-exec. Never clone or expose this descriptor.
    file: Option<File>,
}

impl ApplicationOwnership {
    pub(super) fn retire(&mut self) -> Result<(), StorageError> {
        if let Some(file) = &self.file {
            file.unlock().map_err(fault!(StorageError::Io))?;
        }
        self.file.take();
        Ok(())
    }

    #[cfg(unix)]
    pub(super) fn acquire(directory: &Path) -> Result<Self, StorageError> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        crate::storage::prepare_private_directory(directory)?;
        let path = directory.join("application.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| StorageError::InvalidApplicationLock)?;
        let metadata = file
            .metadata()
            .map_err(|_| StorageError::InvalidApplicationLock)?;
        let named =
            std::fs::symlink_metadata(&path).map_err(|_| StorageError::InvalidApplicationLock)?;
        if !metadata.is_file()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
            || named.file_type().is_symlink()
            || metadata.dev() != named.dev()
            || metadata.ino() != named.ino()
        {
            return Err(StorageError::InvalidApplicationLock);
        }
        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => StorageError::ApplicationAlreadyOwned,
            TryLockError::Error(_) => StorageError::Io,
        })?;
        Ok(Self { file: Some(file) })
    }

    #[cfg(not(unix))]
    pub(super) fn acquire(_directory: &Path) -> Result<Self, StorageError> {
        Err(StorageError::Permission)
    }
}

impl Drop for ApplicationOwnership {
    fn drop(&mut self) {
        // Closing only this descriptor can leave a fork's temporary duplicate
        // holding the lock until exec. Explicitly unlock the open description.
        // Terminal replacement uses checked retirement; Drop is best effort.
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}
