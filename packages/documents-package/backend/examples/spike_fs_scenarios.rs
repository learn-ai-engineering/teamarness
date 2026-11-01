//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! Scenario harness for the folder-mapping questions, driven through the real Markdown merge
//! and export. Two Installations are two Loro documents; a "record" is a body update that the
//! other side imports, as the record log delivers it. Prints the state after every scenario.
//!
//! Run: cargo run --release -p teamarness-documents-package --example spike_fs_scenarios -- <corpus dir>

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use loro::{LoroDoc, PeerID};
use teamarness_documents_package::markdown::{canonical, export, merge, write_tree};
use teamarness_documents_package::spike_folder_sync::{
    FileState, Import, WriteMode, hash, import_file, imported, render, write_back,
};

const MAX_UPDATE: usize = 160 * 1024;

static PEERS: AtomicU64 = AtomicU64::new(0x1000);
fn fresh_peer() -> PeerID {
    PEERS.fetch_add(1, Ordering::Relaxed)
}

/// A new Document whose body is `markdown` (genesis + one revision), and its update history.
fn create(markdown: &str) -> (LoroDoc, Vec<Vec<u8>>) {
    let doc = LoroDoc::new();
    write_tree(&doc, &[]).unwrap();
    let genesis = doc.export(loro::ExportMode::all_updates()).unwrap();
    let merged = merge(&doc, &doc.oplog_frontiers(), markdown, fresh_peer()).unwrap();
    doc.import(&merged.update).unwrap();
    (doc, vec![genesis, merged.update])
}

fn replica(history: &[Vec<u8>]) -> LoroDoc {
    let doc = LoroDoc::new();
    for update in history {
        doc.import(update).unwrap();
    }
    doc
}

/// An in-app (or remote) edit: revise the current export with `f` and return the update.
fn edit(doc: &LoroDoc, f: impl Fn(&str) -> String) -> Vec<u8> {
    let current = export(doc).unwrap();
    let merged = merge(doc, &doc.oplog_frontiers(), &f(&current), fresh_peer()).unwrap();
    doc.import(&merged.update).unwrap();
    merged.update
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("PROTOTYPE-wipe-me-fsdocs-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Maps a Document to `path`: writes its export and records the state.
fn map(doc: &LoroDoc, path: &Path) -> FileState {
    let text = export(doc).unwrap();
    fs::write(path, &text).unwrap();
    FileState { base: doc.oplog_frontiers(), written: hash(text.as_bytes()) }
}

/// The full file sync step for one Installation: import the file if it changed, then write
/// the doc back if the file no longer describes it. Returns the update to send, if any.
fn sync(doc: &LoroDoc, state: &mut FileState, path: &Path, mode: WriteMode) -> Option<Vec<u8>> {
    let text = fs::read_to_string(path).unwrap();
    let mut sent = None;
    match import_file(doc, state, &text, fresh_peer()).unwrap() {
        Import::Unchanged => {}
        Import::NoOps => {
            imported(doc, state, &text);
        }
        Import::Update(update) => {
            doc.import(&update).unwrap();
            sent = Some(update);
            imported(doc, state, &text);
        }
    }
    let on_disk = fs::read_to_string(path).unwrap();
    if canonical(&on_disk).unwrap() != export(doc).unwrap() {
        let out = render(doc, &on_disk, mode);
        assert!(write_back(path, state, doc, &out), "file changed during sync");
    } else {
        state.base = doc.oplog_frontiers();
    }
    sent
}

const BODY: &str = "# Plan\n\nFirst paragraph about scope.\n\nSecond paragraph about risk.\n\nThird paragraph about delivery.\n\n- one\n- two\n";

fn s1_round_trip() {
    println!("\n== Q1 round trip on disk ==");
    let dir = tmpdir("q1");
    let (doc, _) = create(BODY);
    let path = dir.join("plan.md");
    let mut state = map(&doc, &path);
    let unchanged = import_file(&doc, &state, &fs::read_to_string(&path).unwrap(), fresh_peer()).unwrap();
    println!("re-read of our own write: {unchanged:?}");
    // A hard-wrapped, `*`-bulleted file (style differs, content same as export).
    let styled = "# Plan\n\nFirst paragraph\nabout scope.\n\nSecond paragraph about risk.\n\nThird paragraph about delivery.\n\n* one\n* two";
    fs::write(&path, styled).unwrap();
    let result = import_file(&doc, &state, styled, fresh_peer()).unwrap();
    println!("style-only external rewrite: {result:?}");
    let moved = imported(&doc, &mut state, styled);
    println!("base moved without rewriting the file: {moved}; file kept as written: {}", fs::read_to_string(&path).unwrap() == styled);
}

fn s2_concurrent() {
    println!("\n== Q2 external file edit + concurrent edits ==");
    let dir = tmpdir("q2");
    let (a, history) = create(BODY);
    let b = replica(&history);
    let path = dir.join("plan.md");
    let mut state = map(&a, &path);
    // B edits the third paragraph and, separately, a word in the first paragraph.
    let ub = edit(&b, |t| t.replace("about delivery", "about delivery (B)").replace("First paragraph", "Opening paragraph"));
    // Meanwhile the file on A changes the first paragraph's end and the second paragraph.
    let file = fs::read_to_string(&path).unwrap()
        .replace("about scope.", "about scope (file).")
        .replace("about risk.", "about risk (file).");
    fs::write(&path, &file).unwrap();
    let started = Instant::now();
    let ua = sync(&a, &mut state, &path, WriteMode::Canonical).expect("file edit imported");
    let import_ms = started.elapsed().as_secs_f64() * 1e3;
    // Exchange records both ways.
    a.import(&ub).unwrap();
    b.import(&ua).unwrap();
    sync(&a, &mut state, &path, WriteMode::Canonical);
    let (ea, eb) = (export(&a).unwrap(), export(&b).unwrap());
    println!("converged: {}; file == export on A: {}", ea == eb, fs::read_to_string(&path).unwrap() == ea);
    for marker in ["(B)", "Opening paragraph", "about scope (file).", "about risk (file)."] {
        println!("  {marker:22} present {} time(s)", count(&ea, marker));
    }
    println!("file import (merge + import) took {import_ms:.2} ms; update {} bytes", ua.len());
}

fn s3_offline_bulk(corpus: &[String]) {
    println!("\n== Q3 offline bulk edit (app closed), then start ==");
    let dir = tmpdir("q3");
    let n = corpus.len().min(200);
    let mut docs = Vec::new();
    let mut states = BTreeMap::new();
    for (i, body) in corpus.iter().take(n).enumerate() {
        let (doc, _) = create(body);
        let path = dir.join(format!("doc-{i:03}.md"));
        states.insert(path.clone(), map(&doc, &path));
        docs.push((path, doc));
    }
    // App closed: edit 150 files three ways, delete 20, add 20, rename 10.
    let mut kinds = BTreeMap::new();
    for (i, (path, _)) in docs.iter().enumerate().take(150) {
        let text = fs::read_to_string(path).unwrap();
        let new = match i % 3 {
            0 => format!("{text}\nAppended offline paragraph {i}.\n"),
            1 => text.replacen("the", "THE", 3),
            _ => text.lines().rev().collect::<Vec<_>>().join("\n"), // heavy restructure
        };
        *kinds.entry(i % 3).or_insert(0) += 1;
        fs::write(path, new).unwrap();
    }
    let mut deleted = Vec::new();
    for (path, _) in docs.iter().skip(150).take(20) {
        fs::remove_file(path).unwrap();
        deleted.push(path.clone());
    }
    let mut renamed = BTreeMap::new();
    for (path, _) in docs.iter().skip(170).take(10) {
        let to = path.with_file_name(format!("renamed-{}", path.file_name().unwrap().to_string_lossy()));
        fs::rename(path, &to).unwrap();
        renamed.insert(to, path.clone());
    }
    for i in 0..20 {
        fs::write(dir.join(format!("new-{i:02}.md")), format!("# New {i}\n\nCreated offline.\n")).unwrap();
    }
    // Start: scan the folder.
    let started = Instant::now();
    let on_disk: BTreeMap<PathBuf, String> = fs::read_dir(&dir).unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| { let t = fs::read_to_string(&p).unwrap(); (p, t) })
        .collect();
    let mut records = 0;
    let mut bytes = 0;
    let mut largest = 0;
    let mut over = 0;
    let mut missing = Vec::new();
    for (path, doc) in &docs {
        let state = states.get_mut(path).unwrap();
        let Some(text) = on_disk.get(path) else { missing.push(path.clone()); continue };
        match import_file(doc, state, text, fresh_peer()).unwrap() {
            Import::Update(update) => {
                records += 1;
                bytes += update.len();
                largest = largest.max(update.len());
                if update.len() > MAX_UPDATE { over += 1; }
                doc.import(&update).unwrap();
                imported(doc, state, text);
            }
            Import::NoOps | Import::Unchanged => {}
        }
    }
    // Unknown files: a rename keeps the hash we last wrote; anything else is a new Document.
    let by_hash: BTreeMap<_, _> = missing.iter().map(|p| (states[p].written, p.clone())).collect();
    let (mut renames, mut creates) = (0, 0);
    for (path, text) in &on_disk {
        if states.contains_key(path) { continue; }
        if by_hash.contains_key(&hash(text.as_bytes())) { renames += 1 } else { creates += 1; records += 1 }
    }
    let archives = missing.len() - renames;
    records += archives;
    let elapsed = started.elapsed();
    let correct = docs.iter().filter(|(p, d)| on_disk.get(p).is_some_and(|t| canonical(t).unwrap() == export(d).unwrap())).count();
    println!("{n} mapped; offline: 150 edited {kinds:?} (0=append,1=word,2=reverse lines), 20 deleted, 10 renamed, 20 added");
    println!("start scan: {:.0} ms total; {records} records ({} body edits, {creates} creates, {archives} archives, {renames} renames detected by hash)", elapsed.as_secs_f64() * 1e3, records - creates - archives);
    println!("body update bytes total {bytes}, largest {largest}, over 160 KiB MAX_UPDATE: {over}");
    println!("docs whose export now equals their file's canonical text: {correct}/{}", n - 30);
    let _ = (deleted, renamed);
}

fn s4_race() {
    println!("\n== Q4 remote edit arrives while the file has an unimported edit ==");
    let dir = tmpdir("q4");
    let (a, history) = create(BODY);
    let b = replica(&history);
    let path = dir.join("plan.md");
    let mut state = map(&a, &path);
    let file = fs::read_to_string(&path).unwrap().replace("about risk.", "about risk (unimported file edit).");
    fs::write(&path, &file).unwrap();
    let ub = edit(&b, |t| t.replace("- two", "- two\n- three (remote)"));
    a.import(&ub).unwrap();
    // Naive write-back first: must refuse because the file changed since our last write.
    let naive = write_back(&path, &mut state, &a, &render(&a, &file, WriteMode::Canonical));
    println!("write-back over an unimported edit refused: {}", !naive);
    let ua = sync(&a, &mut state, &path, WriteMode::Canonical).expect("file edit imported");
    b.import(&ua).unwrap();
    let on_disk = fs::read_to_string(&path).unwrap();
    println!("file has both edits: {}; converged: {}", on_disk.contains("(unimported file edit)") && on_disk.contains("three (remote)"), export(&a).unwrap() == export(&b).unwrap());
    // Overwrite window: time between the hash check and the rename.
    let mut worst = 0f64;
    for _ in 0..200 {
        let t = Instant::now();
        let text = render(&a, &on_disk, WriteMode::Canonical);
        let check = fs::read(&path).unwrap();
        let _ = hash(&check);
        let tmp = path.with_extension("w");
        fs::write(&tmp, &text).unwrap();
        fs::rename(&tmp, &path).unwrap();
        worst = worst.max(t.elapsed().as_secs_f64() * 1e6);
    }
    println!("check-then-rename window (worst of 200, incl. fsync-less write): {worst:.0} µs");
}

fn s6_git_double_apply() {
    println!("\n== Q6 the same edit arrives by git pull and by p2p ==");
    for (label, change) in [
        ("insert sentence", Box::new(|t: &str| t.replace("about risk.", "about risk. Added by B.")) as Box<dyn Fn(&str) -> String>),
        ("replace word", Box::new(|t: &str| t.replace("Second paragraph", "Middle paragraph"))),
        ("delete paragraph", Box::new(|t: &str| t.replace("Third paragraph about delivery.\n\n", ""))),
        ("new list item", Box::new(|t: &str| t.replace("- two", "- two\n- three"))),
    ] {
        // git first: A's file is overwritten by the pull before B's record arrives.
        let dir = tmpdir("q6");
        let (a, history) = create(BODY);
        let b = replica(&history);
        let path = dir.join("plan.md");
        let mut state = map(&a, &path);
        let ub = edit(&b, &change);
        fs::write(&path, export(&b).unwrap()).unwrap(); // git pull brings B's committed file
        let ua = sync(&a, &mut state, &path, WriteMode::Canonical);
        a.import(&ub).unwrap();
        if let Some(ua) = ua { b.import(&ua).unwrap(); }
        let ea = export(&a).unwrap();
        let expected = change(BODY);
        let dup = canonical(&expected).unwrap() != ea;
        // p2p first: the record lands and is written to the file, then git brings the same file.
        let (a2, history2) = create(BODY);
        let b2 = replica(&history2);
        let path2 = dir.join("plan2.md");
        let mut state2 = map(&a2, &path2);
        let ub2 = edit(&b2, &change);
        a2.import(&ub2).unwrap();
        sync(&a2, &mut state2, &path2, WriteMode::Canonical);
        fs::write(&path2, export(&b2).unwrap()).unwrap();
        let second = sync(&a2, &mut state2, &path2, WriteMode::Canonical);
        println!("{label:16}: git-first corrupts: {dup}{}; p2p-first second import is a record: {}",
            if dup { format!(" -> {:?}", ea.lines().filter(|l| !BODY.contains(*l)).collect::<Vec<_>>()) } else { String::new() },
            second.is_some());
    }
}

fn s_splice(corpus: &[(PathBuf, String)]) {
    println!("\n== Write-back fidelity: canonical vs splice after one remote paragraph edit ==");
    let (mut lines_canon, mut lines_splice, mut ok, mut total, mut untouched_files) = (0usize, 0usize, 0, 0, 0);
    for (_, original) in corpus {
        let (doc, _) = create(original);
        let exported = export(&doc).unwrap();
        // A remote edit: append a sentence to the last paragraph-like line of the export.
        let Some(target) = exported.lines().rev().find(|l| l.len() > 20 && !l.starts_with('#') && !l.starts_with('|')).map(str::to_owned) else { continue };
        let target_t = target.clone();
        edit(&doc, move |t| t.replacen(&target_t, &format!("{target_t} Remote addition."), 1));
        total += 1;
        let c = render(&doc, original, WriteMode::Canonical);
        let s = render(&doc, original, WriteMode::Splice);
        let diff = |new: &str| {
            let old: Vec<&str> = original.lines().collect();
            new.lines().filter(|l| !old.contains(l)).count()
        };
        lines_canon += diff(&c);
        let ds = diff(&s);
        lines_splice += ds;
        if ds <= 2 { untouched_files += 1; }
        // Splice output must still describe the doc exactly.
        if canonical(&s).unwrap() == export(&doc).unwrap() { ok += 1; }
    }
    println!("{total} files: new/changed lines written — canonical {lines_canon}, splice {lines_splice}");
    println!("splice: files where only the edited block changed (<=2 lines): {untouched_files}/{total}; splice re-imports as no-op: {ok}/{total}");
}

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "md") { out.push(p) }
        }
    }
    for d in &dirs { walk(Path::new(d), &mut files); }
    files.sort();
    let corpus: Vec<(PathBuf, String)> = files.into_iter().map(|p| { let t = fs::read_to_string(&p).unwrap(); (p, t) }).collect();
    let bodies: Vec<String> = corpus.iter().map(|(_, t)| t.clone()).collect();
    s1_round_trip();
    s2_concurrent();
    s3_offline_bulk(&bodies);
    s4_race();
    s6_git_double_apply();
    s_splice(&corpus);
}
