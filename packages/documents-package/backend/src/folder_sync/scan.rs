//! The flat folder scan: what a mapped root holds now, by hash, without following links.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use super::codec::{EXTENSION, TEMP_SUFFIX, sha256_reader};
use crate::store::mappings::FileHash;

/// One file a scan found to sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScannedFile {
    /// The file's name, relative to the root.
    pub rel: String,
    pub hash: FileHash,
    pub len: u64,
}

/// What a mapped root holds. Every list is sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// Regular files directly in the root whose name ends in lowercase `.md` and does not start
    /// with `.`.
    pub files: Vec<ScannedFile>,
    /// Folders directly in the root whose name does not start with `.`. They are not synced
    /// yet.
    pub subfolders: Vec<String>,
    /// Symbolic links directly in the root whose name does not start with `.`. They are never
    /// followed.
    pub symlinks: Vec<String>,
    /// Entries that would be synced but cannot be: the name is not UTF-8, or the file cannot be
    /// read. A name that is not UTF-8 is shown lossily.
    pub unreadable: Vec<String>,
}

/// Scans the files directly in `root`. It does not descend into folders and does not follow
/// symbolic links. Hidden entries, Teamarness's temporary files, other extensions and special
/// files are skipped. An error means the root itself is missing or unreadable.
pub(crate) fn scan(root: &Path) -> io::Result<Scan> {
    let mut scan = Scan::default();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let raw = entry.file_name();
        let lossy = raw.to_string_lossy();
        if lossy.starts_with('.') || lossy.ends_with(TEMP_SUFFIX) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            if lossy.ends_with(EXTENSION) {
                scan.unreadable.push(lossy.into_owned());
            }
            continue;
        };
        if file_type.is_symlink() {
            scan.symlinks.push(lossy.into_owned());
            continue;
        }
        if file_type.is_dir() {
            scan.subfolders.push(lossy.into_owned());
            continue;
        }
        if !file_type.is_file() || !lossy.ends_with(EXTENSION) {
            continue;
        }
        let Some(name) = raw.to_str() else {
            scan.unreadable.push(lossy.into_owned());
            continue;
        };
        match File::open(entry.path()).and_then(sha256_reader) {
            Ok((hash, len)) => scan.files.push(ScannedFile {
                rel: name.to_owned(),
                hash,
                len,
            }),
            Err(_) => scan.unreadable.push(name.to_owned()),
        }
    }
    scan.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan.subfolders.sort();
    scan.symlinks.sort();
    scan.unreadable.sort();
    Ok(scan)
}

/// The folders directly in `root` whose name does not start with `.`, sorted. Unlike [`scan`],
/// it reads no file, so the settings listing can call it often. A missing or unreadable root
/// has none.
pub(crate) fn subfolders(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut folders: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    folders.sort();
    folders
}

#[cfg(test)]
mod tests {
    use super::super::codec::{sha256, temp_name};
    use super::*;

    #[test]
    fn scan_lists_markdown_files_with_hash_and_length() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("b.md"), "bee\n").unwrap();
        fs::write(root.path().join("a.md"), "").unwrap();

        let scan = scan(root.path()).unwrap();
        assert_eq!(
            scan.files,
            vec![
                ScannedFile {
                    rel: "a.md".to_owned(),
                    hash: sha256(b""),
                    len: 0,
                },
                ScannedFile {
                    rel: "b.md".to_owned(),
                    hash: sha256(b"bee\n"),
                    len: 4,
                },
            ]
        );
        assert!(scan.subfolders.is_empty());
        assert!(scan.symlinks.is_empty());
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn scan_skips_hidden_temp_and_other_files_and_lists_subfolders() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        fs::write(path.join("kept.md"), "x").unwrap();
        fs::write(path.join(".hidden.md"), "x").unwrap();
        fs::write(path.join(temp_name("kept.md")), "x").unwrap();
        fs::write(path.join("upper.MD"), "x").unwrap();
        fs::write(path.join("notes.txt"), "x").unwrap();
        fs::write(path.join("markdown"), "x").unwrap();
        fs::create_dir(path.join("sub")).unwrap();
        fs::write(path.join("sub").join("nested.md"), "x").unwrap();
        fs::create_dir(path.join("folder.md")).unwrap();
        fs::create_dir(path.join(".git")).unwrap();

        let scan = scan(path).unwrap();
        let names: Vec<&str> = scan.files.iter().map(|file| file.rel.as_str()).collect();
        assert_eq!(names, ["kept.md"]);
        assert_eq!(scan.subfolders, ["folder.md", "sub"]);
        assert!(scan.symlinks.is_empty());
        assert!(scan.unreadable.is_empty());
        assert_eq!(subfolders(path), scan.subfolders);
        assert!(subfolders(&path.join("missing")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn scan_lists_symbolic_links_without_following_them() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("target.md"), "outside").unwrap();
        fs::write(outside.join("inner.md"), "outside").unwrap();
        symlink(outside.join("target.md"), root.join("linked.md")).unwrap();
        symlink(&outside, root.join("linked-folder")).unwrap();
        symlink(root.join("missing.md"), root.join("dangling.md")).unwrap();
        symlink(outside.join("target.md"), root.join(".hidden-link.md")).unwrap();

        let scan = scan(&root).unwrap();
        assert!(scan.files.is_empty());
        assert!(scan.subfolders.is_empty());
        assert_eq!(scan.symlinks, ["dangling.md", "linked-folder", "linked.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_special_files() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("socket.md");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let scan = scan(root.path()).unwrap();
        assert_eq!(scan, Scan::default());
    }

    #[test]
    fn scan_of_a_missing_root_is_an_error() {
        let parent = tempfile::tempdir().unwrap();
        assert!(scan(&parent.path().join("missing")).is_err());
        let file = parent.path().join("file.md");
        fs::write(&file, "x").unwrap();
        assert!(scan(&file).is_err());
    }
}
