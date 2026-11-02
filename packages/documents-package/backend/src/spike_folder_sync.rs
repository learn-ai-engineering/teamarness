//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! The pure core of mapping one Document to one Markdown file. It answers the prototype
//! questions; it is not the design. Each mapped file keeps, per Installation:
//!
//! - `base`: the Loro frontiers the file's contents were last written from (or imported at),
//!   so an external edit merges as a revision of that base through [`crate::markdown::merge`],
//!   exactly as `document.revise` does;
//! - `written`: the SHA-256 of the bytes Teamarness last wrote or imported, so its own writes
//!   (and unchanged files at start) are never re-imported.

use std::fs;
use std::io::Write;
use std::path::Path;

use comrak::{Arena, Options, parse_document};
use loro::{Frontiers, LoroDoc, PeerID};
use sha2::{Digest, Sha256};

use crate::markdown::{MergeError, canonical, export, merge};

pub type Hash = [u8; 32];

pub fn hash(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}

/// Per-Installation state for one mapped file.
#[derive(Clone, Debug)]
pub struct FileState {
    pub base: Frontiers,
    pub written: Hash,
}

/// How a changed Document is written back to its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// The whole file becomes the Canonical Markdown export.
    Canonical,
    /// Top-level blocks whose canonical form did not change keep their original bytes; only
    /// changed blocks are written canonically.
    Splice,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Import {
    /// The file is what Teamarness last wrote or imported: nothing to do (echo suppression).
    Unchanged,
    /// The file changed but its revision produces no ops (only Markdown style changed).
    NoOps,
    /// One body update to append (as one sealed body edit record) and import.
    Update(Vec<u8>),
}

/// Imports the current file text as a revision of `state.base`. The caller appends the
/// update, imports it into `doc`, then calls [`imported`].
pub fn import_file(
    doc: &LoroDoc,
    state: &FileState,
    text: &str,
    peer: PeerID,
) -> Result<Import, MergeError> {
    if hash(text.as_bytes()) == state.written {
        return Ok(Import::Unchanged);
    }
    let merged = merge(doc, &state.base, text, peer)?;
    if merged.ops == 0 {
        return Ok(Import::NoOps);
    }
    Ok(Import::Update(merged.update))
}

/// After an import: the file's text is now included in the doc. The base moves to the doc's
/// frontiers only when the doc's export equals what the file says, so the file still
/// describes `base` exactly; otherwise the caller must write back.
pub fn imported(doc: &LoroDoc, state: &mut FileState, text: &str) -> bool {
    state.written = hash(text.as_bytes());
    let current = export(doc).expect("valid body");
    if canonical(text).ok().as_deref() == Some(current.as_str()) {
        state.base = doc.oplog_frontiers();
        true
    } else {
        false
    }
}

/// Text to write for the doc's current body, given the file's current text.
pub fn render(doc: &LoroDoc, previous: &str, mode: WriteMode) -> String {
    let exported = export(doc).expect("valid body");
    match mode {
        WriteMode::Canonical => exported,
        WriteMode::Splice => splice(previous, &exported),
    }
}

/// Writes `text` atomically (temp file + rename) unless the file changed on disk since
/// `state.written` — then it returns `false` and the caller must import first.
pub fn write_back(path: &Path, state: &mut FileState, doc: &LoroDoc, text: &str) -> bool {
    if let Ok(on_disk) = fs::read(path) {
        if hash(&on_disk) != state.written {
            return false;
        }
    }
    let tmp = path.with_extension("md.teamarness-tmp");
    {
        let mut file = fs::File::create(&tmp).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
    fs::rename(&tmp, path).unwrap();
    state.written = hash(text.as_bytes());
    state.base = doc.oplog_frontiers();
    true
}

/// Top-level blocks of `markdown` as source line ranges (comrak sourcepos, 1-based inclusive).
fn blocks(markdown: &str) -> Vec<String> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.strikethrough = true;
    let root = parse_document(&arena, markdown, &options);
    let lines: Vec<&str> = markdown.lines().collect();
    root.children()
        .map(|node| {
            let pos = node.data.borrow().sourcepos;
            let (start, end) = (pos.start.line.max(1) - 1, pos.end.line.min(lines.len()));
            lines[start..end].join("\n")
        })
        .collect()
}

/// Keeps every original top-level block whose canonical form is unchanged in `exported`, in
/// export order (LCS over canonical block strings); every other block is written canonically.
fn splice(previous: &str, exported: &str) -> String {
    let old = blocks(previous);
    let old_canon: Vec<String> = old
        .iter()
        .map(|b| canonical(b).unwrap_or_default().trim_end().to_owned())
        .collect();
    let new = blocks(exported);
    let new_canon: Vec<String> = new
        .iter()
        .map(|b| canonical(b).unwrap_or_default().trim_end().to_owned())
        .collect();
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old_canon[i] == new_canon[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while j < m {
        if i < n && old_canon[i] == new_canon[j] {
            out.push(old[i].clone());
            i += 1;
            j += 1;
        } else if i < n && lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            out.push(new[j].clone());
            j += 1;
        }
    }
    let mut text = out.join("\n\n");
    text.push('\n');
    text
}
