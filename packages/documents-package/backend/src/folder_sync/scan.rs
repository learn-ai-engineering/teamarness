//! The mode-selected folder scan: what a mapped root holds now, by hash, without following links.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use super::codec::{EXTENSION, TEMP_SUFFIX, name_key, representable_folder_segment, sha256_reader};
use crate::domain::normalize_folder;
use crate::store::mappings::FileHash;

/// One file a scan found to sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScannedFile {
    /// The file's actual path spelling, relative to the root.
    pub rel: String,
    pub hash: FileHash,
    pub len: u64,
}

/// What a mapped root holds. Every list is sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// Eligible regular files, with whole relative paths in recursive mode.
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
    /// Files the file provider moved off this computer and left a placeholder for: iCloud
    /// "Optimize Storage" replaces `Plan.md` with the hidden `.Plan.md.icloud`. Each name is the
    /// evicted file's own name. Such a file still exists, so it is never archived.
    pub evicted: Vec<String>,
    /// Unsafe or unavailable subtrees; absence below these paths never means deletion.
    pub protected: Vec<(String, String)>,
}

/// The name of the file an iCloud placeholder `name` stands for: `.Plan.md.icloud` stands for
/// `Plan.md`.
fn evicted_name(name: &str) -> Option<&str> {
    let original = name.strip_prefix('.')?.strip_suffix(".icloud")?;
    (original.ends_with(EXTENSION) && !original.starts_with('.')).then_some(original)
}

/// The Installation's selected scan scope; flat never opens a subfolder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanMode {
    Flat,
    Recursive,
}

impl From<bool> for ScanMode {
    fn from(recursive: bool) -> Self {
        if recursive {
            Self::Recursive
        } else {
            Self::Flat
        }
    }
}

/// Scans without following links. Root failure is an error; subtree failures protect their
/// prefixes. Hidden entries, temporary files, and extensions other than lowercase `.md` stay out.
pub(crate) fn scan(root: &Path, mode: ScanMode) -> io::Result<Scan> {
    let mut scan = Scan::default();
    scan_directory(root, "", mode, &mut scan)?;
    scan.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan.subfolders.sort();
    scan.symlinks.sort();
    scan.unreadable.sort();
    scan.evicted.sort();
    scan.protected.sort();
    Ok(scan)
}

impl Scan {
    pub(crate) fn protects(&self, rel: &str) -> bool {
        self.protected.iter().any(|(prefix, _)| {
            rel == prefix
                || rel
                    .strip_prefix(prefix)
                    .is_some_and(|tail| tail.starts_with('/'))
        }) || self
            .unreadable
            .iter()
            .chain(&self.evicted)
            .any(|path| path == rel)
    }
}

fn scan_directory(root: &Path, parent: &str, mode: ScanMode, scan: &mut Scan) -> io::Result<()> {
    let entries = fs::read_dir(root.join(parent))?.collect::<Result<Vec<_>, _>>()?;
    let mut directories = std::collections::HashMap::<String, Vec<String>>::new();
    for entry in &entries {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            directories.entry(name_key(&name)).or_default().push(name);
        }
    }
    for entry in entries {
        let raw = entry.file_name();
        let lossy = raw.to_string_lossy();
        let relative = |name: &str| {
            if parent.is_empty() {
                name.to_owned()
            } else {
                format!("{parent}/{name}")
            }
        };
        let rel = relative(&lossy);
        if let Some(original) = evicted_name(&lossy) {
            scan.evicted.push(relative(original));
            if mode == ScanMode::Recursive {
                scan.protected
                    .push((relative(original), "unreadable".to_owned()));
            }
            continue;
        }
        if mode == ScanMode::Recursive
            && let Some(original) = lossy
                .strip_prefix('.')
                .and_then(|name| name.strip_suffix(".icloud"))
            && !original.is_empty()
            && !original.starts_with('.')
        {
            scan.protected
                .push((relative(original), "unreadable".to_owned()));
            continue;
        }
        if lossy.starts_with('.') || lossy.ends_with(TEMP_SUFFIX) {
            if mode == ScanMode::Recursive && lossy.ends_with(TEMP_SUFFIX) {
                scan.protected.push((rel, "invalid_folder".to_owned()));
            }
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            scan.unreadable.push(rel.clone());
            scan.protected.push((rel, "unreadable".to_owned()));
            continue;
        };
        if file_type.is_symlink() {
            scan.symlinks.push(rel.clone());
            if mode == ScanMode::Recursive {
                scan.protected.push((rel, "symlink".to_owned()));
            }
            continue;
        }
        if file_type.is_dir() {
            if mode == ScanMode::Flat {
                scan.subfolders.push(rel);
                continue;
            }
            let reason = if raw.to_str().is_none()
                || normalize_folder(&rel).is_err()
                || !representable_folder_segment(&lossy)
            {
                Some("invalid_folder")
            } else if directories
                .get(&name_key(&lossy))
                .is_some_and(|names| names.len() > 1)
            {
                Some("name_collision_unresolved")
            } else {
                None
            };
            if let Some(reason) = reason {
                scan.protected.push((rel, reason.to_owned()));
            } else if scan_directory(root, &rel, mode, scan).is_err() {
                scan.protected.push((rel, "unreadable".to_owned()));
            }
            continue;
        }
        if !file_type.is_file() {
            if mode == ScanMode::Recursive {
                scan.protected.push((rel, "unreadable".to_owned()));
            }
            continue;
        }
        if !lossy.ends_with(EXTENSION) {
            continue;
        }
        if raw.to_str().is_none() {
            scan.unreadable.push(rel);
            continue;
        }
        match File::open(entry.path()).and_then(sha256_reader) {
            Ok((hash, len)) => scan.files.push(ScannedFile { rel, hash, len }),
            Err(_) => scan.unreadable.push(rel),
        }
    }
    Ok(())
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

        let scan = scan(root.path(), ScanMode::Flat).unwrap();
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

        let scan = scan(path, ScanMode::Flat).unwrap();
        let names: Vec<&str> = scan.files.iter().map(|file| file.rel.as_str()).collect();
        assert_eq!(names, ["kept.md"]);
        assert_eq!(scan.subfolders, ["folder.md", "sub"]);
        assert!(scan.symlinks.is_empty());
        assert!(scan.unreadable.is_empty());
        assert_eq!(subfolders(path), scan.subfolders);
        assert!(subfolders(&path.join("missing")).is_empty());
    }

    #[test]
    fn scan_lists_icloud_placeholders_as_evicted_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        fs::write(path.join(".Plan.md.icloud"), "placeholder").unwrap();
        fs::write(path.join(".notes.txt.icloud"), "placeholder").unwrap();
        fs::write(path.join("..hidden.md.icloud"), "placeholder").unwrap();
        fs::write(path.join("kept.md"), "x").unwrap();

        let scan = scan(path, ScanMode::Flat).unwrap();
        assert_eq!(scan.evicted, ["Plan.md"]);
        let names: Vec<&str> = scan.files.iter().map(|file| file.rel.as_str()).collect();
        assert_eq!(names, ["kept.md"]);
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

        let scan = scan(&root, ScanMode::Flat).unwrap();
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
        let scan = scan(root.path(), ScanMode::Flat).unwrap();
        assert_eq!(scan, Scan::default());
    }

    #[test]
    fn recursive_scan_bounds_paths_and_protects_folder_placeholders() {
        let root = tempfile::tempdir().unwrap();
        let deep = "a/b/c/d/e/f/g/h";
        fs::create_dir_all(root.path().join(format!("{deep}/too-deep"))).unwrap();
        fs::write(root.path().join(format!("{deep}/Yes.md")), "yes").unwrap();
        fs::write(root.path().join(format!("{deep}/too-deep/No.md")), "no").unwrap();
        fs::create_dir(root.path().join("bad:name")).unwrap();
        fs::write(root.path().join("bad:name/No.md"), "no").unwrap();
        fs::write(root.path().join(".Evicted.icloud"), "placeholder").unwrap();
        let scanned = scan(root.path(), ScanMode::Recursive).unwrap();
        assert_eq!(
            scanned
                .files
                .iter()
                .map(|f| f.rel.as_str())
                .collect::<Vec<_>>(),
            [format!("{deep}/Yes.md")]
        );
        assert!(scanned.protects("a/b/c/d/e/f/g/h/too-deep/No.md"));
        assert!(scanned.protects("bad:name/No.md"));
        assert!(scanned.protects("Evicted/Note.md"));
    }

    #[cfg(unix)]
    #[test]
    fn recursive_scan_permission_error_protects_only_the_unreadable_subtree() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Blocked")).unwrap();
        fs::write(root.path().join("Blocked/Note.md"), "private").unwrap();
        fs::write(root.path().join("Sentinel.md"), "ok").unwrap();
        fs::set_permissions(
            root.path().join("Blocked"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let scanned = scan(root.path(), ScanMode::Recursive).unwrap();
        fs::set_permissions(
            root.path().join("Blocked"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(scanned.protects("Blocked/Note.md"));
        assert!(!scanned.protects("Sentinel.md"));
        assert_eq!(scanned.files.len(), 1);
    }

    #[test]
    fn recursive_directory_casefold_aliases_are_refused_as_whole_subtrees() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Straße")).unwrap();
        // Some filesystems equate these spellings themselves; then two directory entries
        // cannot exist. The codec's destination-alias refusal still covers that case.
        if fs::create_dir(root.path().join("STRASSE")).is_err() {
            return;
        }
        fs::write(root.path().join("Straße/One.md"), "one").unwrap();
        fs::write(root.path().join("STRASSE/Two.md"), "two").unwrap();
        let scanned = scan(root.path(), ScanMode::Recursive).unwrap();
        assert!(scanned.files.is_empty());
        assert!(scanned.protects("Straße/One.md"));
        assert!(scanned.protects("STRASSE/Two.md"));
        assert_eq!(scanned.protected.len(), 2);
        assert!(
            scanned
                .protected
                .iter()
                .all(|(_, reason)| reason == "name_collision_unresolved")
        );
    }

    #[test]
    fn scan_of_a_missing_root_is_an_error() {
        let parent = tempfile::tempdir().unwrap();
        assert!(scan(&parent.path().join("missing"), ScanMode::Flat).is_err());
        let file = parent.path().join("file.md");
        fs::write(&file, "x").unwrap();
        assert!(scan(&file, ScanMode::Flat).is_err());
    }
}
