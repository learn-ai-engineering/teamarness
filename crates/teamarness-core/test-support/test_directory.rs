use std::ops::Deref;
use std::path::{Path, PathBuf};

/// Owns a temporary parent and an initially absent fixture path inside it.
///
/// Unlike a bare PathBuf, this removes the whole fixture on return or panic.
/// The child may be created, removed, or replaced by a file without losing
/// cleanup ownership. Keep this owner alive across runtime shutdown/reopen,
/// and make sure it drops after every resource that uses its path: as a
/// struct field, declare it after those resources (fields drop in
/// declaration order); as a local binding, declare it before them (locals
/// drop in reverse order). Never pass a temporary, as in `open(&directory())`,
/// because it is dropped, and its folder deleted, at the end of the statement.
pub struct TestDirectory {
    path: PathBuf,
    _parent: tempfile::TempDir,
}

impl TestDirectory {
    pub fn new(name: &str) -> Self {
        let parent = tempfile::Builder::new()
            .prefix("teamarness-")
            .tempdir()
            .expect("a temporary fixture parent is created");
        Self {
            path: parent.path().join(name),
            _parent: parent,
        }
    }
}

impl Deref for TestDirectory {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TestDirectory {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl From<&TestDirectory> for PathBuf {
    fn from(directory: &TestDirectory) -> Self {
        directory.path.clone()
    }
}
