//! Test-only tempfile adapter for fixtures containing immutable Package UI caches.
//!
//! Production cache directories stay read-only throughout a test. Before the
//! owned TempDir is removed, restore owner access to directories inside it.
//! Never follow symlinks: their targets are not owned by this fixture.
#![allow(dead_code)]

use std::io;
use std::path::Path;

pub struct TempDir {
    inner: Option<::tempfile::TempDir>,
}

pub fn tempdir() -> io::Result<TempDir> {
    ::tempfile::tempdir().map(TempDir::from)
}

pub fn tempdir_in(path: impl AsRef<Path>) -> io::Result<TempDir> {
    ::tempfile::tempdir_in(path).map(TempDir::from)
}

impl TempDir {
    pub fn new() -> io::Result<Self> {
        tempdir()
    }

    pub fn path(&self) -> &Path {
        self.inner
            .as_ref()
            .expect("the fixture is still owned")
            .path()
    }

    pub fn close(mut self) -> io::Result<()> {
        let inner = self.inner.take().expect("the fixture is still owned");
        restore_directory_access(inner.path())?;
        inner.close()
    }
}

impl From<::tempfile::TempDir> for TempDir {
    fn from(inner: ::tempfile::TempDir) -> Self {
        Self { inner: Some(inner) }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(inner) = &self.inner
            && let Err(error) = restore_directory_access(inner.path())
        {
            eprintln!("cannot prepare temporary fixture for cleanup: {error}");
        }
        // The inner TempDir then removes the tree, including during unwinding.
    }
}

#[cfg(unix)]
fn restore_directory_access(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
    )?;
    for entry in std::fs::read_dir(path)? {
        restore_directory_access(&entry?.path())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restore_directory_access(_path: &Path) -> io::Result<()> {
    Ok(())
}
