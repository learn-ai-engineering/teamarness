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

/// Restores owner access to every reachable directory, continuing past
/// failures so one bad entry cannot strand its siblings read-only. Every
/// failure is reported; the first one is returned.
#[cfg(unix)]
fn restore_directory_access(path: &Path) -> io::Result<()> {
    let mut first_error = None;
    restore_directory_access_into(path, &mut first_error);
    first_error.map_or(Ok(()), Err)
}

#[cfg(unix)]
fn restore_directory_access_into(path: &Path, first_error: &mut Option<io::Error>) {
    use std::os::unix::fs::PermissionsExt;

    let mut record = |error: io::Error| {
        eprintln!("cannot restore access to {}: {error}", path.display());
        first_error.get_or_insert(error);
    };
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => return record(error),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    if let Err(error) = std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
    ) {
        record(error);
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => return record(error),
    };
    let mut children = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => children.push(entry.path()),
            Err(error) => record(error),
        }
    }
    for child in children {
        restore_directory_access_into(&child, first_error);
    }
}

#[cfg(not(unix))]
fn restore_directory_access(_path: &Path) -> io::Result<()> {
    Ok(())
}
