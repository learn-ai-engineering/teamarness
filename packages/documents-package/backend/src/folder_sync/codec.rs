//! The file codec: file names for Document titles, file hashes, the atomic write that never
//! overwrites an unseen change, and the splice writer that keeps the bytes of unchanged blocks.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use comrak::{Arena, Options, parse_document};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::markdown::canonical;
use crate::store::mappings::FileHash;

/// The most bytes a file name keeps before its `.md` extension.
pub(crate) const MAX_STEM_BYTES: usize = 200;

/// The name a title with no usable characters gets.
const UNTITLED: &str = "Untitled";

/// The extension of every synced file. Only this exact, lowercase form is synced.
pub(crate) const EXTENSION: &str = ".md";

/// The suffix of the temporary file [`write_atomic`] renames into place. The temporary file
/// starts with `.`, so a scan never syncs it.
pub(crate) const TEMP_SUFFIX: &str = ".teamarness-tmp";

/// Characters no file name keeps, on any common file system.
const RESERVED: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

// ---------------------------------------------------------------- naming

/// The file name for a Document title: NFC, reserved and control characters replaced with
/// `-`, dots and spaces trimmed from both ends, at most [`MAX_STEM_BYTES`] bytes, then `.md`.
/// A title with nothing left becomes `Untitled.md`.
pub(crate) fn file_name(title: &str) -> String {
    format!("{}{EXTENSION}", stem(title, MAX_STEM_BYTES))
}

/// The name key two file names collide on: the case-folded NFC form. Two files whose keys are
/// equal are the same file on a case-insensitive file system.
pub(crate) fn name_key(name: &str) -> String {
    let decomposed: String = name.nfd().collect();
    caseless::default_case_fold_str(&decomposed).nfc().collect()
}

/// The file name for `title` whose [`name_key`] is not in `taken`: [`file_name`], then
/// `<stem> (2).md`, `<stem> (3).md` and so on. The stem is cut so that the name with its suffix
/// still keeps at most [`MAX_STEM_BYTES`] bytes before `.md`.
pub(crate) fn unique_name(title: &str, taken: &HashSet<String>) -> String {
    let first = file_name(title);
    if !taken.contains(&name_key(&first)) {
        return first;
    }
    (2u64..)
        .map(|n| {
            let suffix = format!(" ({n})");
            let stem = stem(title, MAX_STEM_BYTES - suffix.len());
            format!("{stem}{suffix}{EXTENSION}")
        })
        .find(|candidate| !taken.contains(&name_key(candidate)))
        .expect("a finite set leaves a free name")
}

/// The sanitised title, at most `max` bytes, never empty.
fn stem(title: &str, max: usize) -> String {
    let replaced: String = title
        .nfc()
        .map(|c| {
            if RESERVED.contains(&c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cut = truncate(trim(&replaced), max);
    let stem = trim(cut);
    if stem.is_empty() {
        UNTITLED.to_owned()
    } else {
        stem.to_owned()
    }
}

fn trim(text: &str) -> &str {
    text.trim_matches(['.', ' '])
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let end = (0..=max)
        .rev()
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(0);
    &text[..end]
}

// ---------------------------------------------------------------- hashing

/// The SHA-256 digest of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> FileHash {
    Sha256::digest(bytes).into()
}

/// The SHA-256 digest and length of a reader's bytes, read in chunks.
pub(super) fn sha256_reader(mut reader: impl Read) -> io::Result<(FileHash, u64)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        hasher.update(&buffer[..read]);
        len += read as u64;
    }
    Ok((hasher.finalize().into(), len))
}

// ---------------------------------------------------------------- atomic write

/// Why [`write_atomic`] wrote nothing.
#[derive(Debug)]
pub(crate) enum WriteRefused {
    /// The file on disk is not the one the caller expected: it changed, appeared or went away.
    /// `found` is the hash of the file now there, if there is one.
    Changed { found: Option<FileHash> },
    /// The relative path is absolute, empty, or has a `.`, `..` or prefix component.
    OutsideRoot,
    /// The root is not a directory, or a folder on the path is not a directory.
    NotADirectory,
    /// A component of the path, or the file itself, is a symbolic link.
    Symlink,
    /// The path names something other than a regular file.
    NotAFile,
    /// The file system failed.
    Io(io::Error),
}

impl From<io::Error> for WriteRefused {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Writes `text` to `root/rel` atomically and returns its hash.
///
/// `expected` is the hash of the file the caller last saw, or `None` when the caller expects
/// no file. The file is read again first; if it is not what the caller expected, nothing is
/// written. The text goes to `.<name>.teamarness-tmp` in the same folder, is synced, and is
/// renamed over the file. A path that would leave `root`, or pass through a symbolic link below
/// it, is refused. The root itself may be reached through links.
pub(crate) fn write_atomic(
    root: &Path,
    rel: &Path,
    text: &str,
    expected: Option<FileHash>,
) -> Result<FileHash, WriteRefused> {
    let (folder, name) = resolve(root, rel)?;
    let path = folder.join(&name);
    // The replaced file's permissions, so a private note stays private after a write-back.
    let mut permissions = None;
    let found = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(WriteRefused::Symlink),
        Ok(metadata) if !metadata.is_file() => return Err(WriteRefused::NotAFile),
        Ok(metadata) => {
            permissions = Some(metadata.permissions());
            Some(sha256_reader(File::open(&path)?)?.0)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if found != expected {
        return Err(WriteRefused::Changed { found });
    }

    let temp = folder.join(temp_name(&name));
    // A temporary file left by a crash is replaced. Removing it first means the new one is
    // created fresh, never written through a link someone put in its place.
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    // The rename is durable once its folder is synced. Some platforms cannot sync a folder; the
    // file itself is already complete either way.
    if let Ok(folder) = File::open(&folder) {
        let _ = folder.sync_all();
    }
    Ok(sha256(text.as_bytes()))
}

/// The temporary file name for `name`.
pub(crate) fn temp_name(name: &str) -> String {
    format!(".{name}{TEMP_SUFFIX}")
}

/// Checks that `rel` stays inside `root` with no symbolic link below `root`, and returns the
/// folder that holds the file and the file's name.
fn resolve(root: &Path, rel: &Path) -> Result<(PathBuf, String), WriteRefused> {
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(WriteRefused::OutsideRoot);
            }
        }
    }
    let Some((name, folders)) = parts.split_last() else {
        return Err(WriteRefused::OutsideRoot);
    };
    let name = name.to_str().ok_or(WriteRefused::OutsideRoot)?.to_owned();
    if !fs::metadata(root)?.is_dir() {
        return Err(WriteRefused::NotADirectory);
    }
    let mut folder = root.to_path_buf();
    for part in folders {
        folder.push(part);
        let metadata = fs::symlink_metadata(&folder)?;
        if metadata.file_type().is_symlink() {
            return Err(WriteRefused::Symlink);
        }
        if !metadata.is_dir() {
            return Err(WriteRefused::NotADirectory);
        }
    }
    Ok((folder, name))
}

// ---------------------------------------------------------------- git

/// The nearest folder at or above `path` that contains `.git` (a folder, or the file a linked
/// worktree has), so a caller can warn that a mapped folder is inside a git working tree.
pub(crate) fn git_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

// ---------------------------------------------------------------- splice

/// The largest table of block pairs splice compares (16 MiB of `u32`). A pair of texts with
/// more blocks than this is written as exported.
const MAX_SPLICE_CELLS: usize = 1 << 22;

/// The text to write for `exported`, the Canonical Markdown of the Document, given the file's
/// `previous` text.
///
/// Every top-level block of `previous` whose canonical form is unchanged in `exported` keeps
/// its original bytes, in export order (the longest common subsequence of canonical block
/// strings). Every other block is written as exported. Splice is only an optimisation: if the
/// spliced text does not have the same Canonical Markdown as `exported`, `exported` is returned
/// unchanged.
pub(crate) fn splice(previous: &str, exported: &str) -> String {
    let Ok(expected) = canonical(exported) else {
        return exported.to_owned();
    };
    let spliced = splice_blocks(previous, exported);
    match canonical(&spliced) {
        Ok(actual) if actual == expected => spliced,
        _ => exported.to_owned(),
    }
}

fn splice_blocks(previous: &str, exported: &str) -> String {
    let old = blocks(previous);
    let new = blocks(exported);
    let key = |block: &String| canonical(block).map(|text| text.trim_end().to_owned()).ok();
    let old_key: Vec<Option<String>> = old.iter().map(key).collect();
    let new_key: Vec<Option<String>> = new.iter().map(key).collect();
    let same = |i: usize, j: usize| old_key[i].is_some() && old_key[i] == new_key[j];

    let (n, m) = (old.len(), new.len());
    if (n + 1).saturating_mul(m + 1) > MAX_SPLICE_CELLS {
        return exported.to_owned();
    }
    // lcs[i * (m + 1) + j] is the longest common subsequence of old[i..] and new[j..].
    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if same(i, j) {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    if lcs[0] == 0 {
        // Nothing to keep: the export is the text, byte for byte.
        return exported.to_owned();
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::with_capacity(m));
    while j < m {
        if i < n && same(i, j) {
            out.push(old[i].as_str());
            i += 1;
            j += 1;
        } else if i < n && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            i += 1;
        } else {
            out.push(new[j].as_str());
            j += 1;
        }
    }
    let mut text = out.join("\n\n");
    text.push('\n');
    text
}

/// The top-level blocks of `markdown`, each as its whole source lines.
///
/// The options recognise what Documents import recognises (strikethrough, tables and front
/// matter), so a table or front matter is one block. Link reference definitions are not blocks;
/// a kept block that needs one fails the canonical check in [`splice`].
fn blocks(markdown: &str) -> Vec<String> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());
    let root = parse_document(&arena, markdown, &options);
    let lines: Vec<&str> = markdown.lines().collect();
    root.children()
        .filter_map(|node| {
            let position = node.data.borrow().sourcepos;
            let start = position.start.line.max(1) - 1;
            let end = position.end.line.min(lines.len());
            (start < end).then(|| lines[start..end].join("\n"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ naming

    #[test]
    fn file_names_follow_the_naming_table() {
        let cases: &[(&str, &str)] = &[
            ("Plan", "Plan.md"),
            ("Q3 plan: draft?", "Q3 plan- draft-.md"),
            (r#"a/b\c:d*e?f"g<h>i|j"#, "a-b-c-d-e-f-g-h-i-j.md"),
            ("tab\there\nnew", "tab-here-new.md"),
            ("  .hidden. ", "hidden.md"),
            ("...", "Untitled.md"),
            ("", "Untitled.md"),
            ("   ", "Untitled.md"),
            ("notes.md", "notes.md.md"),
            ("Ünïcödé", "Ünïcödé.md"),
        ];
        for (title, expected) in cases {
            assert_eq!(file_name(title), *expected, "title {title:?}");
        }
    }

    #[test]
    fn file_names_are_nfc() {
        let decomposed = "Cafe\u{301}";
        assert_eq!(file_name(decomposed), "Caf\u{e9}.md");
    }

    #[test]
    fn file_names_keep_at_most_200_bytes_on_a_character_boundary() {
        let ascii = "a".repeat(300);
        assert_eq!(file_name(&ascii), format!("{}.md", "a".repeat(200)));

        // 199 ASCII bytes, then a 3-byte character that would end at byte 202.
        let title = format!("{}€tail", "b".repeat(199));
        let name = file_name(&title);
        assert_eq!(name, format!("{}.md", "b".repeat(199)));

        // 2-byte characters: 100 of them fill 200 bytes exactly.
        let wide = "é".repeat(150);
        let name = file_name(&wide);
        assert_eq!(name.len(), 200 + 3);
        assert_eq!(name, format!("{}.md", "é".repeat(100)));

        // A cut that leaves a trailing space or dot trims it.
        let spaced = format!("{} tail", "c".repeat(199));
        assert_eq!(file_name(&spaced), format!("{}.md", "c".repeat(199)));
    }

    #[test]
    fn name_keys_fold_case_and_normalisation() {
        assert_eq!(name_key("Plan.md"), name_key("PLAN.md"));
        assert_eq!(name_key("Straße.md"), name_key("STRASSE.md"));
        assert_eq!(name_key("Cafe\u{301}.md"), name_key("CAF\u{c9}.md"));
        assert_ne!(name_key("Plan.md"), name_key("Plans.md"));
    }

    #[test]
    fn unique_names_skip_taken_keys_including_case_only_collisions() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name("Plan", &taken), "Plan.md");
        taken.insert(name_key("plan.md"));
        assert_eq!(unique_name("Plan", &taken), "Plan (2).md");
        taken.insert(name_key("PLAN (2).md"));
        assert_eq!(unique_name("Plan", &taken), "Plan (3).md");
        assert_eq!(unique_name("Other", &taken), "Other.md");
    }

    #[test]
    fn unique_names_cut_the_stem_to_fit_the_suffix() {
        let title = "d".repeat(300);
        let taken = HashSet::from([name_key(&file_name(&title))]);
        let name = unique_name(&title, &taken);
        assert_eq!(name, format!("{} (2).md", "d".repeat(196)));
    }

    // ------------------------------------------------------------ atomic write

    fn temp_files(folder: &Path) -> Vec<String> {
        fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(TEMP_SUFFIX))
            .collect()
    }

    #[test]
    fn write_atomic_creates_and_replaces_with_the_expected_hash() {
        let root = tempfile::tempdir().unwrap();
        let hash = write_atomic(root.path(), Path::new("a.md"), "one\n", None).unwrap();
        assert_eq!(hash, sha256(b"one\n"));
        assert_eq!(
            fs::read_to_string(root.path().join("a.md")).unwrap(),
            "one\n"
        );

        let next = write_atomic(root.path(), Path::new("a.md"), "two\n", Some(hash)).unwrap();
        assert_eq!(next, sha256(b"two\n"));
        assert_eq!(
            fs::read_to_string(root.path().join("a.md")).unwrap(),
            "two\n"
        );
        assert!(temp_files(root.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_replaced_files_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private.md");
        fs::write(&path, "secret\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(
            root.path(),
            Path::new("private.md"),
            "still secret\n",
            Some(sha256(b"secret\n")),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "still secret\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn write_atomic_refuses_a_changed_file_and_leaves_no_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a.md");
        let hash = write_atomic(root.path(), Path::new("a.md"), "mine\n", None).unwrap();
        fs::write(&path, "theirs\n").unwrap();

        let refused = write_atomic(root.path(), Path::new("a.md"), "again\n", Some(hash));
        assert!(matches!(
            refused,
            Err(WriteRefused::Changed { found: Some(found) }) if found == sha256(b"theirs\n")
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");
        assert!(temp_files(root.path()).is_empty());

        // A file that appeared where none was expected, and one that went away.
        let refused = write_atomic(root.path(), Path::new("a.md"), "new\n", None);
        assert!(matches!(
            refused,
            Err(WriteRefused::Changed { found: Some(_) })
        ));
        let refused = write_atomic(root.path(), Path::new("gone.md"), "x\n", Some(hash));
        assert!(matches!(
            refused,
            Err(WriteRefused::Changed { found: None })
        ));
        assert!(!root.path().join("gone.md").exists());
        assert!(temp_files(root.path()).is_empty());
    }

    #[test]
    fn write_atomic_replaces_a_stale_temp_file() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(temp_name("a.md")), "stale").unwrap();
        write_atomic(root.path(), Path::new("a.md"), "fresh\n", None).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("a.md")).unwrap(),
            "fresh\n"
        );
        assert!(temp_files(root.path()).is_empty());
    }

    #[test]
    fn write_atomic_refuses_paths_that_leave_the_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        fs::create_dir(&root).unwrap();
        for rel in ["../escape.md", "./a.md", "", "sub/../../escape.md"] {
            let refused = write_atomic(&root, Path::new(rel), "x\n", None);
            assert!(
                matches!(refused, Err(WriteRefused::OutsideRoot)),
                "{rel:?}: {refused:?}"
            );
        }
        let absolute = parent.path().join("escape.md");
        let refused = write_atomic(&root, &absolute, "x\n", None);
        assert!(matches!(refused, Err(WriteRefused::OutsideRoot)));
        assert!(!parent.path().join("escape.md").exists());
    }

    #[test]
    fn write_atomic_refuses_a_file_that_is_a_folder() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("a.md")).unwrap();
        let refused = write_atomic(root.path(), Path::new("a.md"), "x\n", None);
        assert!(matches!(refused, Err(WriteRefused::NotAFile)));
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_symbolic_links() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("target.md"), "outside\n").unwrap();

        // A linked file.
        symlink(outside.join("target.md"), root.join("link.md")).unwrap();
        let hash = sha256(b"outside\n");
        let refused = write_atomic(&root, Path::new("link.md"), "x\n", Some(hash));
        assert!(matches!(refused, Err(WriteRefused::Symlink)));

        // A linked folder on the path.
        symlink(&outside, root.join("sub")).unwrap();
        let refused = write_atomic(&root, Path::new("sub/new.md"), "x\n", None);
        assert!(matches!(refused, Err(WriteRefused::Symlink)));

        assert_eq!(
            fs::read_to_string(outside.join("target.md")).unwrap(),
            "outside\n"
        );
        assert!(!outside.join("new.md").exists());
        assert!(temp_files(&root).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_reaches_a_root_through_a_link() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, parent.path().join("alias")).unwrap();
        write_atomic(&parent.path().join("alias"), Path::new("a.md"), "x\n", None).unwrap();
        assert_eq!(fs::read_to_string(real.join("a.md")).unwrap(), "x\n");
    }

    // ------------------------------------------------------------ git

    #[test]
    fn git_ancestor_finds_the_nearest_working_tree() {
        let parent = tempfile::tempdir().unwrap();
        let outer = parent.path().join("outer");
        let inner = outer.join("inner");
        let deep = inner.join("notes").join("deep");
        fs::create_dir_all(&deep).unwrap();
        let plain = parent.path().join("plain");
        fs::create_dir(&plain).unwrap();

        assert_eq!(git_ancestor(&plain), None);

        fs::create_dir(outer.join(".git")).unwrap();
        assert_eq!(git_ancestor(&deep), Some(outer.clone()));
        assert_eq!(git_ancestor(&outer), Some(outer.clone()));

        // A linked worktree has a `.git` file.
        fs::write(inner.join(".git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(git_ancestor(&deep), Some(inner.clone()));
        assert_eq!(git_ancestor(&plain), None);
    }

    // ------------------------------------------------------------ splice

    #[test]
    fn splice_keeps_the_bytes_of_unchanged_blocks() {
        // Hard wraps, `*` bullets and `*emphasis*` are not canonical, but their blocks are
        // unchanged in meaning, so their bytes survive.
        let previous = "# Plan\n\nA paragraph that is\nhard wrapped by its author.\n\n* one\n* two\n\nSome *emphasis* here.\n";
        let previous_canonical = canonical(previous).unwrap();
        assert_ne!(previous_canonical, previous);
        let exported = previous_canonical.replace("Some *emphasis* here.", "Changed text.");
        assert_ne!(exported, previous_canonical, "{previous_canonical}");

        let spliced = splice(previous, &exported);
        assert_eq!(
            spliced,
            "# Plan\n\nA paragraph that is\nhard wrapped by its author.\n\n* one\n* two\n\nChanged text.\n"
        );
        assert_eq!(canonical(&spliced).unwrap(), canonical(&exported).unwrap());
    }

    #[test]
    fn splice_writes_inserted_and_removed_blocks() {
        let previous = "First\nwrapped.\n\nMiddle.\n\nLast\nwrapped.\n";
        let exported = "First wrapped.\n\nNew block.\n\nLast wrapped.\n";
        assert_eq!(
            splice(previous, exported),
            "First\nwrapped.\n\nNew block.\n\nLast\nwrapped.\n"
        );
    }

    #[test]
    fn splice_with_nothing_to_keep_is_the_export() {
        assert_eq!(splice("Old\nwrapped.\n", "New text.\n"), "New text.\n");
        assert_eq!(splice("", "New text.\n"), "New text.\n");
        assert_eq!(splice("Old.\n", ""), "");
    }

    #[test]
    fn splice_of_an_unchanged_document_keeps_the_file() {
        let previous = "Line one\nline two.\n\n* a\n* b\n";
        let exported = canonical(previous).unwrap();
        assert_eq!(splice(previous, &exported), previous);
    }

    #[test]
    fn splice_falls_back_when_its_output_would_change_meaning() {
        // The kept code block has no closing fence. It ends the old file, so it means the same
        // as the exported, closed block; but a block written after it would become code. The
        // writer returns the export instead.
        let previous = "Intro\nwrapped.\n\n```\ncode\n";
        let exported = format!("{}\nNew tail.\n", canonical(previous).unwrap());
        assert_eq!(exported, "Intro wrapped.\n\n```\ncode\n```\n\nNew tail.\n");
        let spliced = splice_blocks(previous, &exported);
        assert_eq!(spliced, "Intro\nwrapped.\n\n```\ncode\n\nNew tail.\n");
        assert_ne!(canonical(&spliced).unwrap(), canonical(&exported).unwrap());

        assert_eq!(splice(previous, &exported), exported);
    }

    #[test]
    fn splice_writes_a_block_that_needs_a_reference_definition_as_exported() {
        // A reference definition is not a block, so the paragraph that uses it does not match
        // its exported form alone and is written canonically.
        let previous = "See [the docs][d]\nfor more.\n\nOld tail.\n\n[d]: https://example.com\n";
        let exported = canonical(previous)
            .unwrap()
            .replace("Old tail.", "New tail.");
        assert_eq!(
            splice(previous, &exported),
            "See [the docs](https://example.com) for more.\n\nNew tail.\n"
        );
    }

    #[test]
    fn splice_keeps_tables_and_front_matter_as_single_blocks() {
        let previous = "---\ntitle: x\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nOld\ntext.\n";
        let exported = canonical(previous)
            .unwrap()
            .replace("Old text.", "New text.");
        let spliced = splice(previous, &exported);
        assert!(spliced.ends_with("New text.\n"), "{spliced}");
        assert_eq!(canonical(&spliced).unwrap(), canonical(&exported).unwrap());
    }
}
