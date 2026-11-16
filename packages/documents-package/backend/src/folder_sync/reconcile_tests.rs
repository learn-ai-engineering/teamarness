//! Reconciler tests: a temporary folder, an activation on a scripted record log that keeps what
//! it appends, and the reconciler driven directly, with no watcher.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::testing::ScriptedCapability;
use teamarness_package_sdk::{BackendError, CancellationToken};

use super::super::codec::sha256;
use super::{Crash, FolderSync, Outcome, Refusal, rename_file, request_id, title_of};
use crate::backend::tests::{
    ALICE, Session, Writer, deliver, hex, qualify_empty_history, remote_edit, session_update,
    start_without_folder_sync, submit, wait_until,
};
use crate::store::DocumentView;
use crate::store::mappings::{FolderMapping, MappedFile, MappingState, RefusedFile};

/// A record log that signs every append as Alice, keeps it, and opens what it kept, so an
/// inclusion check can read a request back.
pub(in crate::backend::folder_sync) fn remembering_log() -> ScriptedCapability {
    remembering_history(Vec::new())
}

fn remembering_history(history: Vec<JsonValue>) -> ScriptedCapability {
    let next = Arc::new(AtomicU64::new(100 + history.len() as u64));
    let records = Arc::new(Mutex::new(
        history
            .into_iter()
            .enumerate()
            .map(|(index, payload)| {
                let seed = 100 + index as u64;
                (
                    URL_SAFE_NO_PAD.encode(seed.to_string()),
                    (
                        seed,
                        payload["purpose"].as_str().unwrap().to_owned(),
                        payload["canonical"].as_str().unwrap().to_owned(),
                    ),
                )
            })
            .collect::<HashMap<String, (u64, String, String)>>(),
    ));
    ScriptedCapability::new().calls(move |method, payload| match method {
        "append" => {
            let seed = next.fetch_add(1, Ordering::SeqCst);
            let sealed = URL_SAFE_NO_PAD.encode(seed.to_string());
            records.lock().unwrap().insert(
                sealed.clone(),
                (
                    seed,
                    payload["purpose"].as_str().unwrap().to_owned(),
                    payload["canonical"].as_str().unwrap().to_owned(),
                ),
            );
            Ok(json!({
                "recordId": hex(64, seed), "orderKey": hex(82, seed), "author": ALICE,
                "signature": "c2ln", "sealedRecord": sealed, "durability": "committed",
            }))
        }
        "open" => {
            let records = records.lock().unwrap();
            let results: Vec<JsonValue> = payload["sealedRecords"]
                .as_array()
                .unwrap()
                .iter()
                .map(|sealed| match records.get(sealed.as_str().unwrap()) {
                    Some((seed, purpose, canonical)) => json!({
                        "status": "ready", "recordId": hex(64, *seed),
                        "orderKey": hex(82, *seed), "author": ALICE, "signature": "c2ln",
                        "purpose": purpose, "canonical": canonical,
                    }),
                    None => json!({ "status": "waiting_for_history_access" }),
                })
                .collect();
            Ok(json!({ "results": results }))
        }
        _ => Err(BackendError::new(
            "capability_unavailable",
            false,
            json!({}),
        )),
    })
}

struct Fixture {
    session: Session,
    sync: FolderSync,
    collection: String,
    root: PathBuf,
    _folder: tempfile::TempDir,
    _data: tempfile::TempDir,
}

static NEXT_EDIT: AtomicU64 = AtomicU64::new(1);

impl Fixture {
    /// A qualified activation with one Collection mapped to an empty folder.
    async fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut session = start_without_folder_sync(data.path(), remembering_log()).await;
        qualify_empty_history(&session).await;
        let collection = session
            .harness
            .request("documents.collection.create", json!({ "name": "Plans" }))
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let handles = &session.handles;
        let sync = FolderSync::new(
            &handles.log,
            &handles.hub,
            &handles.changes,
            ALICE.to_owned(),
            CancellationToken::new(),
        );
        let root = folder.path().join("plans");
        fs::create_dir(&root).unwrap();
        let fixture = Self {
            session,
            sync,
            collection,
            root,
            _folder: folder,
            _data: data,
        };
        fixture.map();
        fixture
    }

    fn map(&self) {
        self.session
            .handles
            .store
            .set_folder_mapping(&FolderMapping {
                collection_id: self.collection.clone(),
                root_path: self.root.to_str().unwrap().to_owned(),
                state: MappingState::Active,
                created_at: 1,
                recursive: false,
            })
            .unwrap();
    }

    async fn restart(self) -> Self {
        let Self {
            session,
            sync,
            collection,
            root,
            _folder,
            _data,
        } = self;
        let history: Vec<JsonValue> = session
            .scripted
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .map(|call| call.payload)
            .collect();
        let scripted = remembering_history(history.clone());
        session.harness.shutdown().await.unwrap();
        drop(sync);
        let session = start_without_folder_sync(_data.path(), scripted).await;
        session.scripted.stream().opened().await;
        for (index, payload) in history.iter().enumerate() {
            let seed = 100 + index as u64;
            deliver(&session, json!({"kind":"record", "cursor":hex(50, seed), "recordId":hex(64,seed), "orderKey":hex(82,seed), "author":ALICE, "signature":"c2ln", "purpose":payload["purpose"], "canonical":payload["canonical"], "sealedRecord":URL_SAFE_NO_PAD.encode(seed.to_string())})).await;
        }
        deliver(&session, json!({"kind":"replay_end", "cursor":hex(50, 99 + history.len() as u64), "gapFree":true})).await;
        wait_until(
            || session.handles.qualified.load(Ordering::Acquire),
            "replayed persisted records",
        )
        .await;
        let handles = &session.handles;
        let sync = FolderSync::new(
            &handles.log,
            &handles.hub,
            &handles.changes,
            ALICE.to_owned(),
            CancellationToken::new(),
        );
        Self {
            session,
            sync,
            collection,
            root,
            _folder,
            _data,
        }
    }

    fn recursive(&self) {
        let mut mapping = self.mapping();
        mapping.recursive = true;
        self.session
            .handles
            .store
            .set_folder_mapping(&mapping)
            .unwrap();
    }

    async fn place(&mut self, document: &str, folder: &str) {
        self.request(
            "documents.document.place",
            json!({"documentId": document, "folder": folder}),
        )
        .await;
    }

    fn mapping(&self) -> FolderMapping {
        self.session
            .handles
            .store
            .folder_mapping(&self.collection)
            .unwrap()
            .unwrap()
    }

    async fn request(&mut self, operation: &str, payload: JsonValue) -> JsonValue {
        self.session
            .harness
            .request(operation, payload)
            .await
            .unwrap()
    }

    /// A Document of the Collection, created in the app, holding `markdown`.
    async fn document(&mut self, title: &str, markdown: &str) -> String {
        let collection = self.collection.clone();
        let created = self
            .request(
                "documents.document.create",
                json!({ "collectionId": collection, "title": title }),
            )
            .await;
        let document = created["id"].as_str().unwrap().to_owned();
        if !markdown.is_empty() {
            self.edit(&document, markdown).await;
        }
        document
    }

    /// An in-app edit of `document` to `markdown`, through the one local write path.
    async fn edit(&self, document: &str, markdown: &str) {
        let id = format!("5e{:030x}", NEXT_EDIT.fetch_add(1, Ordering::Relaxed));
        let update = session_update(&self.session.handles, document, markdown);
        submit(&self.session.handles, document, &id, &update)
            .await
            .unwrap();
    }

    /// A teammate's edit of `document` to `markdown`, delivered on the record log stream.
    async fn remote(&self, document: &str, markdown: &str, seed: u64) {
        let update = Writer::of(&self.session.handles, document).write(markdown);
        deliver(&self.session, remote_edit(document, "bob", seed, update)).await;
        wait_until(
            || self.text(document) == markdown,
            "the remote edit to arrive",
        )
        .await;
    }

    fn text(&self, document: &str) -> String {
        self.session.handles.bodies.lock().unwrap()[document]
            .text()
            .to_owned()
    }

    fn appends(&self) -> usize {
        self.session
            .scripted
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .count()
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.root.join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name)).unwrap()
    }

    /// The names in the folder, hidden ones too, sorted.
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn documents(&self) -> Vec<DocumentView> {
        self.session
            .handles
            .store
            .documents()
            .unwrap()
            .into_iter()
            .filter(|document| document.collection_id == self.collection)
            .collect()
    }

    fn titled(&self, title: &str) -> DocumentView {
        self.documents()
            .into_iter()
            .find(|document| document.title.as_deref() == Some(title))
            .unwrap_or_else(|| panic!("a Document titled {title}"))
    }

    fn state(&self, document: &str) -> Option<MappedFile> {
        self.session.handles.store.mapped_file(document).unwrap()
    }

    fn refusals(&self) -> Vec<RefusedFile> {
        self.session
            .handles
            .store
            .refused_files(&self.collection)
            .unwrap()
    }

    async fn folder(&self) -> Outcome {
        self.sync.reconcile_folder(&self.collection).await.unwrap()
    }

    async fn reconcile(&self, document: &str) -> Outcome {
        self.sync.reconcile_document(document).await.unwrap()
    }

    /// Reconciles until nothing changes, and asserts that the last pass appended nothing.
    async fn settled(&self) {
        let before = self.appends();
        let outcome = self.folder().await;
        assert_eq!(
            outcome,
            Outcome::default(),
            "a settled folder changes nothing"
        );
        assert_eq!(self.appends(), before, "a settled folder appends nothing");
    }

    async fn shutdown(self) {
        self.session.harness.shutdown().await.unwrap();
    }
}

fn path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

#[test]
fn request_ids_are_deterministic_bounded_and_distinct() {
    let hash = sha256(b"text\n");
    let id = request_id("doc", "base", &hash);
    assert_eq!(id.len(), 32);
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(id, request_id("doc", "base", &hash));
    assert_ne!(id, request_id("doc", "other", &hash));
    assert_ne!(id, request_id("other", "base", &hash));
    assert_ne!(id, request_id("doc", "base", &sha256(b"text")));
    // The separators keep fields apart.
    assert_ne!(request_id("ab", "c", &hash), request_id("a", "bc", &hash));
}

#[test]
fn titles_come_from_file_stems() {
    assert_eq!(title_of("Plan.md"), "Plan");
    assert_eq!(title_of(" Plan (2) .md"), "Plan (2)");
    assert_eq!(title_of("a\u{7}b.md"), "a-b");
    assert_eq!(title_of("   .md"), "Untitled");
}

// Verifies: TEAM.SYS.DOCS-R14 (Export, Write-back) — an export mapping writes every non-archived Document as its Canonical
// Markdown; a second reconcile writes and appends nothing.
#[tokio::test]
async fn export_writes_every_document_as_canonical_markdown_once() {
    let mut fixture = Fixture::new().await;
    // Before the mapping exists, so its file is never written.
    fixture
        .session
        .handles
        .store
        .clear_folder_mapping(&fixture.collection)
        .unwrap();
    let plan = fixture.document("Plan", "# Plan\n\nShip *it*.\n").await;
    let notes = fixture.document("Notes: Q4", "- one\n- two\n").await;
    let old = fixture.document("Old", "gone\n").await;
    fixture
        .request("documents.document.archive", json!({ "documentId": old }))
        .await;
    let empty = fixture.document("Empty", "").await;
    fixture.map();
    let before = fixture.appends();

    let outcome = fixture.folder().await;
    assert_eq!(outcome.written, 3, "{outcome:?}");
    assert_eq!(fixture.appends(), before, "an export appends nothing");
    assert_eq!(fixture.files(), ["Empty.md", "Notes- Q4.md", "Plan.md"]);
    assert_eq!(fixture.read("Plan.md"), fixture.text(&plan));
    assert_eq!(fixture.read("Notes- Q4.md"), fixture.text(&notes));
    assert_eq!(fixture.read("Empty.md"), fixture.text(&empty));
    let state = fixture.state(&plan).unwrap();
    assert_eq!(
        state.written_hash,
        Some(sha256(fixture.text(&plan).as_bytes()))
    );
    assert!(state.pending_write_hash.is_none());
    assert!(fixture.state(&old).is_none());

    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Mapping rules, Import, Write-back) — an import mapping creates one Document per file, titled by its
// stem; the first sync never rewrites a file, even a hard-wrapped one.
#[tokio::test]
async fn import_creates_documents_and_the_first_sync_never_rewrites() {
    const WRAPPED: &str =
        "Title\n=====\n\nThis paragraph is\nhard wrapped at\nshort lines.\n\n* star item\n";
    let fixture = Fixture::new().await;
    fixture.write("Wrapped.md", WRAPPED);
    fixture.write("Empty.md", "");
    fixture.write("notes.txt", "not synced\n");
    fs::create_dir(path(&fixture.root, "sub")).unwrap();
    fs::write(path(&fixture.root, "sub/inner.md"), "later\n").unwrap();
    let before = fixture.appends();

    let outcome = fixture.folder().await;
    assert_eq!(outcome.created, 2, "{outcome:?}");
    assert_eq!(outcome.imported, 1, "{outcome:?}");
    assert_eq!(outcome.written, 0, "{outcome:?}");
    // Two creates and one revision: the empty file needs none.
    assert_eq!(fixture.appends(), before + 3);
    let wrapped = fixture.titled("Wrapped");
    assert!(fixture.text(&wrapped.id).contains("hard wrapped at"));
    assert_ne!(fixture.text(&wrapped.id), WRAPPED, "the export differs");
    assert_eq!(
        fixture.read("Wrapped.md"),
        WRAPPED,
        "the file is not rewritten"
    );
    assert_eq!(fixture.text(&fixture.titled("Empty").id), "");
    let state = fixture.state(&wrapped.id).unwrap();
    assert_eq!(state.written_hash, Some(sha256(WRAPPED.as_bytes())));
    assert_eq!(state.base_version, fixture.sync.version(&wrapped.id));
    assert_eq!(fixture.documents().len(), 2, "subfolders are not synced");

    fixture.settled().await;
    assert_eq!(fixture.read("Wrapped.md"), WRAPPED);
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Import, Write-back) — a changed file is imported as one revision; an unchanged one is
// left; an in-app edit is spliced back, keeping unchanged blocks byte for byte, and the
// write-back is never imported.
#[tokio::test]
async fn changed_files_import_and_edits_splice_back_without_echo() {
    const FILE: &str = "Title\n=====\n\nFirst paragraph.\n";
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", FILE);
    fixture.folder().await;
    let document = fixture.titled("Doc").id;

    // Changed: one revision; the file stays as the author wrote it.
    let edited = "Title\n=====\n\nFirst paragraph, edited.\n";
    fixture.write("Doc.md", edited);
    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.imported, 1, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 1);
    assert!(fixture.text(&document).contains("First paragraph, edited."));
    assert_eq!(fixture.read("Doc.md"), edited);
    fixture.settled().await;

    // An in-app edit is written back by splice: the setext heading keeps its bytes.
    let mut markdown = fixture.text(&document);
    markdown.push_str("\nAdded in the app.\n");
    fixture.edit(&document, &markdown).await;
    let before = fixture.appends();
    let outcome = fixture.reconcile(&document).await;
    assert_eq!(outcome.written, 1, "{outcome:?}");
    let file = fixture.read("Doc.md");
    assert!(file.starts_with("Title\n=====\n"), "{file}");
    assert!(file.ends_with("Added in the app.\n"), "{file}");
    assert_eq!(
        crate::markdown::canonical(&file).unwrap(),
        fixture.text(&document)
    );
    // No echo: the write-back is not imported.
    assert_eq!(fixture.folder().await, Outcome::default());
    assert_eq!(fixture.appends(), before);
    assert_eq!(fixture.reconcile(&document).await, Outcome::default());
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Import, Write-back) — a file edit that is not yet imported is never overwritten: when a
// teammate's edit needs writing back, the file is imported first, and both edits survive in
// the Document and the file.
#[tokio::test]
async fn an_unimported_file_edit_is_imported_before_the_write_back() {
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", "Alpha.\n\nBeta.\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;

    fixture
        .remote(&document, "Alpha.\n\nBeta, by Bob.\n", 9001)
        .await;
    fixture.write("Doc.md", "Alpha, by Alice.\n\nBeta.\n");
    let before = fixture.appends();
    let outcome = fixture.reconcile(&document).await;
    assert_eq!(outcome.imported, 1, "{outcome:?}");
    assert_eq!(outcome.written, 1, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 1, "only the file's revision");
    let merged = "Alpha, by Alice.\n\nBeta, by Bob.\n";
    assert_eq!(fixture.text(&document), merged);
    assert_eq!(fixture.read("Doc.md"), merged);
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (No lost text) — a file whose import would lose text stays byte for byte and is refused as
// `lossy` with its lost tokens, once; a mapped file edited into a lossy shape keeps both its
// bytes and its Document's text, and is not overwritten by a write-back. A fixed file imports.
#[tokio::test]
async fn lossy_files_are_refused_and_left_byte_for_byte() {
    const LOSSY: &str = "Text with no link.\n\n[unused]: https://example.com/zebra\n";
    let fixture = Fixture::new().await;
    fixture.write("Lossy.md", LOSSY);
    fixture.write("Kept.md", "Kept.\n");
    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.refused, 1, "{outcome:?}");
    assert_eq!(fixture.documents().len(), 1, "only the sound file");
    assert_eq!(fixture.appends(), before + 2);
    assert_eq!(fixture.read("Lossy.md"), LOSSY);
    let refusals = fixture.refusals();
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].rel_path, "Lossy.md");
    assert_eq!(refusals[0].reason, Refusal::Lossy.as_str());
    assert_eq!(refusals[0].refused_hash, Some(sha256(LOSSY.as_bytes())));
    let detail = refusals[0].detail.clone().unwrap();
    assert!(detail.contains("zebra"), "{detail}");
    // The same bytes are not retried.
    fixture.settled().await;

    // A mapped file edited into a lossy shape.
    let kept = fixture.titled("Kept").id;
    let lossy_edit = "Kept, edited.\n\n[gone]: https://example.com/yak\n";
    fixture.write("Kept.md", lossy_edit);
    let outcome = fixture.folder().await;
    assert_eq!(outcome.refused, 1, "{outcome:?}");
    assert_eq!(fixture.text(&kept), "Kept.\n");
    fixture.remote(&kept, "Kept, by Bob.\n", 9002).await;
    let outcome = fixture.reconcile(&kept).await;
    assert_eq!(outcome.written, 0, "{outcome:?}");
    assert_eq!(fixture.read("Kept.md"), lossy_edit, "never overwritten");

    // Fixed with an inline link: imported, and the refusal is gone.
    fixture.write(
        "Lossy.md",
        "Text with [a link](https://example.com/zebra).\n",
    );
    let outcome = fixture.folder().await;
    assert_eq!(outcome.created, 1, "{outcome:?}");
    assert!(
        fixture
            .refusals()
            .iter()
            .all(|refusal| refusal.rel_path != "Lossy.md")
    );
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (No lost text) — an oversize file is refused before any record: no Document, empty or not.
#[tokio::test]
async fn an_oversize_file_leaves_no_document() {
    let fixture = Fixture::new().await;
    let paragraph = "word ".repeat(40);
    let large = format!("{paragraph}\n\n").repeat(crate::body::MAX_TEXT_BYTES / 200 + 10);
    fixture.write("Large.md", &large);
    let huge = "x".repeat(usize::try_from(super::MAX_FILE_BYTES).unwrap() + 1);
    fixture.write("Huge.md", &huge);
    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.refused, 2, "{outcome:?}");
    assert_eq!(fixture.appends(), before);
    assert!(fixture.documents().is_empty());
    let refusals = fixture.refusals();
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal.reason == Refusal::TooLarge.as_str()),
        "{refusals:?}"
    );
    assert_eq!(fixture.read("Large.md"), large);
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — a missing root sets the mapping `unavailable` and archives nothing; when it
// returns, the mapping is active again and nothing changed.
#[tokio::test]
async fn a_missing_root_archives_nothing() {
    let fixture = Fixture::new().await;
    fixture.write("A.md", "a\n");
    fixture.write("B.md", "b\n");
    fixture.folder().await;
    let away = fixture.root.with_file_name("away");
    fs::rename(&fixture.root, &away).unwrap();
    let before = fixture.appends();

    assert_eq!(fixture.folder().await, Outcome::default());
    assert_eq!(fixture.mapping().state, MappingState::Unavailable);
    for document in fixture.documents() {
        assert_eq!(fixture.reconcile(&document.id).await, Outcome::default());
    }
    assert_eq!(fixture.appends(), before);
    assert!(
        fixture
            .documents()
            .iter()
            .all(|document| !document.archived)
    );

    fs::rename(&away, &fixture.root).unwrap();
    fixture.settled().await;
    assert_eq!(fixture.mapping().state, MappingState::Active);
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — a root that still exists but holds none of its synced
// files (an empty mount point, a folder made again, evicted files) is unavailable and archives
// nothing; an iCloud placeholder keeps its file's Document unarchived.
#[tokio::test]
async fn an_emptied_root_or_an_evicted_file_archives_nothing() {
    let fixture = Fixture::new().await;
    fixture.write("A.md", "a\n");
    fixture.write("B.md", "b\n");
    fixture.folder().await;
    fixture.settled().await;
    let before = fixture.appends();

    // Every synced file is gone, but the folder is still there.
    fs::rename(
        path(&fixture.root, "A.md"),
        fixture.root.with_file_name("A.md"),
    )
    .unwrap();
    fs::rename(
        path(&fixture.root, "B.md"),
        fixture.root.with_file_name("B.md"),
    )
    .unwrap();
    assert_eq!(fixture.folder().await, Outcome::default());
    assert_eq!(fixture.mapping().state, MappingState::Unavailable);
    assert_eq!(fixture.appends(), before);
    assert!(
        fixture
            .documents()
            .iter()
            .all(|document| !document.archived)
    );

    // The files come back: the mapping is active again and nothing changed.
    fs::rename(
        fixture.root.with_file_name("A.md"),
        path(&fixture.root, "A.md"),
    )
    .unwrap();
    fs::rename(
        fixture.root.with_file_name("B.md"),
        path(&fixture.root, "B.md"),
    )
    .unwrap();
    fixture.settled().await;
    assert_eq!(fixture.mapping().state, MappingState::Active);
    assert_eq!(fixture.appends(), before);

    // iCloud "Optimize Storage" evicts A.md and leaves `.A.md.icloud`: A is not archived.
    fs::remove_file(path(&fixture.root, "A.md")).unwrap();
    fixture.write(".A.md.icloud", "placeholder");
    assert_eq!(fixture.folder().await, Outcome::default());
    assert_eq!(fixture.mapping().state, MappingState::Active);
    assert_eq!(fixture.appends(), before);
    assert!(!fixture.titled("A").archived);
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Import, Write-back) — a deleted file archives its Document; a file renamed outside renames its
// Document; a Document rename renames its file; a remote archive removes the file and a restore
// writes it again; a new Document gets a new file.
#[tokio::test]
async fn files_and_documents_follow_each_other_through_their_lifecycle() {
    let mut fixture = Fixture::new().await;
    fixture.write("Keep.md", "keep\n");
    fixture.write("Drop.md", "drop\n");
    fixture.write("Old name.md", "renamed\n");
    fixture.folder().await;
    let drop = fixture.titled("Drop").id;
    let renamed = fixture.titled("Old name").id;
    let keep = fixture.titled("Keep").id;

    // Deleted outside: archived. Renamed outside: renamed, with no new Document.
    fs::remove_file(path(&fixture.root, "Drop.md")).unwrap();
    fs::rename(
        path(&fixture.root, "Old name.md"),
        path(&fixture.root, "New name.md"),
    )
    .unwrap();
    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.archived, 1, "{outcome:?}");
    assert_eq!(outcome.renamed, 1, "{outcome:?}");
    assert_eq!(outcome.created, 0, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 2, "one archive and one rename");
    let documents = fixture.documents();
    assert!(documents.iter().any(|d| d.id == drop && d.archived));
    assert!(
        documents
            .iter()
            .any(|d| d.id == renamed && d.title.as_deref() == Some("New name"))
    );
    assert_eq!(fixture.state(&renamed).unwrap().rel_path, "New name.md");
    fixture.settled().await;

    // Renamed in the app: the file follows.
    fixture
        .request(
            "documents.document.rename",
            json!({ "documentId": keep, "title": "Kept/Done" }),
        )
        .await;
    let outcome = fixture.reconcile(&keep).await;
    assert_eq!(outcome.moved, 1, "{outcome:?}");
    assert!(fixture.files().contains(&"Kept-Done.md".to_owned()));
    assert!(!fixture.files().contains(&"Keep.md".to_owned()));
    assert_eq!(fixture.read("Kept-Done.md"), "keep\n");
    fixture.settled().await;

    // Archived elsewhere: the file goes. Restored: it comes back.
    fixture
        .request("documents.document.archive", json!({ "documentId": keep }))
        .await;
    let outcome = fixture.reconcile(&keep).await;
    assert_eq!(outcome.removed, 1, "{outcome:?}");
    assert!(!fixture.files().contains(&"Kept-Done.md".to_owned()));
    assert!(fixture.state(&keep).is_none());
    fixture.settled().await;
    fixture
        .request("documents.document.restore", json!({ "documentId": keep }))
        .await;
    let outcome = fixture.reconcile(&keep).await;
    assert_eq!(outcome.written, 1, "{outcome:?}");
    assert_eq!(fixture.read("Kept-Done.md"), "keep\n");
    fixture.settled().await;

    // A new Document gets a new file.
    let fresh = fixture.document("Fresh", "fresh\n").await;
    let outcome = fixture.reconcile(&fresh).await;
    assert_eq!(outcome.written, 1, "{outcome:?}");
    assert_eq!(fixture.read("Fresh.md"), "fresh\n");
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Write-back) — an archive while the file holds an unimported edit imports the edit first,
// then removes the file.
#[tokio::test]
async fn an_archive_imports_an_unimported_edit_before_removing_the_file() {
    let mut fixture = Fixture::new().await;
    fixture.write("Doc.md", "before\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;
    fixture
        .request(
            "documents.document.archive",
            json!({ "documentId": document }),
        )
        .await;
    fixture.write("Doc.md", "after\n");
    let before = fixture.appends();
    let outcome = fixture.reconcile(&document).await;
    assert_eq!(outcome.imported, 1, "{outcome:?}");
    assert_eq!(outcome.removed, 1, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 1);
    assert_eq!(fixture.text(&document), "after\n");
    assert!(!fixture.files().contains(&"Doc.md".to_owned()));
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (No lost text) — names that differ only in case never share a file: Documents get distinct
// files, and a new file whose name collides with a mapped one is refused.
#[tokio::test]
async fn case_only_collisions_get_distinct_names_or_are_refused() {
    let mut fixture = Fixture::new().await;
    let upper = fixture.document("Plan", "upper\n").await;
    let lower = fixture.document("plan", "lower\n").await;
    fixture.folder().await;
    let upper_file = fixture.state(&upper).unwrap().rel_path;
    let lower_file = fixture.state(&lower).unwrap().rel_path;
    assert_eq!(upper_file, "Plan.md");
    assert_eq!(lower_file, "plan (2).md");
    assert_eq!(fixture.read(&upper_file), "upper\n");
    assert_eq!(fixture.read(&lower_file), "lower\n");
    fixture.settled().await;

    // On a case-sensitive file system, "PLAN.md" is another file with a colliding name.
    fixture.write("PLAN.md", "third\n");
    if fixture.files().contains(&"PLAN.md".to_owned()) && fixture.read("Plan.md") == "upper\n" {
        let outcome = fixture.folder().await;
        assert_eq!(outcome.refused, 1, "{outcome:?}");
        assert_eq!(outcome.created, 0, "{outcome:?}");
        let refusals = fixture.refusals();
        assert_eq!(refusals[0].rel_path, "PLAN.md");
        assert_eq!(
            refusals[0].reason,
            Refusal::NameCollisionUnresolved.as_str()
        );
    }
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — a case-only file rename renames the
// file itself; it never replaces another file whose name differs only in case.
#[test]
fn a_case_only_rename_never_replaces_another_file() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    fs::write(root.join("plan.md"), "mapped\n").unwrap();
    fs::write(root.join("Plan.md"), "refused\n").unwrap();
    let case_sensitive = fs::read_to_string(root.join("plan.md")).unwrap() == "mapped\n";
    if case_sensitive {
        let refused = rename_file(root, "plan.md", "Plan.md", sha256(b"mapped\n")).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(root.join("plan.md")).unwrap(),
            "mapped\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("Plan.md")).unwrap(),
            "refused\n"
        );
        fs::remove_file(root.join("Plan.md")).unwrap();
    }
    // The target is the file itself (a case-insensitive file system) or absent: it is renamed.
    fs::write(root.join("plan.md"), "mapped\n").unwrap();
    rename_file(root, "plan.md", "Plan.md", sha256(b"mapped\n")).unwrap();
    let names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Plan.md"]);
    assert_eq!(
        fs::read_to_string(root.join("Plan.md")).unwrap(),
        "mapped\n"
    );
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — a crash after an import's append and before its file state is saved is
// retried as the same request: one revision, and no text twice.
#[tokio::test]
async fn a_crash_after_the_append_never_duplicates_the_revision() {
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", "First.\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;
    let stale = fixture.state(&document).unwrap();

    fixture.write("Doc.md", "First.\n\nSecond.\n");
    fixture.sync.crash_at(Crash::ImportAppended);
    let before = fixture.appends();
    let crashed = fixture.sync.reconcile_folder(&fixture.collection).await;
    assert_eq!(crashed.unwrap_err().code(), "documents.test_crash");
    assert_eq!(
        fixture.appends(),
        before + 1,
        "the revision reached the log"
    );
    assert_eq!(
        fixture.state(&document).unwrap(),
        stale,
        "no state was saved"
    );

    let outcome = fixture.folder().await;
    assert_eq!(outcome.imported, 1, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 1, "not appended again");
    assert_eq!(fixture.text(&document), "First.\n\nSecond.\n");
    assert_eq!(fixture.read("Doc.md"), "First.\n\nSecond.\n");
    assert_eq!(
        fixture.state(&document).unwrap().written_hash,
        Some(sha256(b"First.\n\nSecond.\n"))
    );
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — a crash after a write-back renamed its file into place and before
// `finish_write` adopts the file on the next reconcile instead of importing it.
#[tokio::test]
async fn a_crash_after_the_rename_adopts_the_written_file() {
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", "First.\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;
    fixture.edit(&document, "First.\n\nFrom the app.\n").await;

    fixture.sync.crash_at(Crash::BodyWritten);
    let crashed = fixture.sync.reconcile_document(&document).await;
    assert_eq!(crashed.unwrap_err().code(), "documents.test_crash");
    assert_eq!(fixture.read("Doc.md"), "First.\n\nFrom the app.\n");
    let pending = fixture.state(&document).unwrap();
    assert_eq!(
        pending.pending_write_hash,
        Some(sha256(b"First.\n\nFrom the app.\n"))
    );

    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.adopted, 1, "{outcome:?}");
    assert_eq!(outcome.imported, 0, "{outcome:?}");
    assert_eq!(fixture.appends(), before);
    let state = fixture.state(&document).unwrap();
    assert_eq!(state.written_hash, pending.pending_write_hash);
    assert_eq!(state.base_version, pending.pending_write_version);
    assert!(state.pending_write_hash.is_none());
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Import) — an offline batch of 50 edits, 3 new files and 2 deletions imports as exactly
// 50 revisions, 3 creates with 3 first revisions, and 2 archives, with no duplication.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offline_batch_imports_with_the_expected_record_count() {
    let fixture = Fixture::new().await;
    for index in 0..52 {
        fixture.write(&format!("Doc {index:02}.md"), &format!("Doc {index}.\n"));
    }
    fixture.folder().await;
    assert_eq!(fixture.documents().len(), 52);

    for index in 0..50 {
        fixture.write(
            &format!("Doc {index:02}.md"),
            &format!("Doc {index}.\n\nEdited offline.\n"),
        );
    }
    for index in 0..3 {
        fixture.write(&format!("New {index}.md"), &format!("New {index}.\n"));
    }
    fs::remove_file(path(&fixture.root, "Doc 50.md")).unwrap();
    fs::remove_file(path(&fixture.root, "Doc 51.md")).unwrap();
    let before = fixture.appends();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.imported, 53, "{outcome:?}");
    assert_eq!(outcome.created, 3, "{outcome:?}");
    assert_eq!(outcome.archived, 2, "{outcome:?}");
    assert_eq!(fixture.appends(), before + 50 + 3 * 2 + 2);
    for index in 0..50 {
        let document = fixture.titled(&format!("Doc {index:02}")).id;
        assert_eq!(
            fixture.text(&document),
            format!("Doc {index}.\n\nEdited offline.\n")
        );
    }
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (Safety) — an archived Collection pauses its mapping and leaves its files; a restore resumes it.
#[tokio::test]
async fn an_archived_collection_pauses_its_mapping() {
    let mut fixture = Fixture::new().await;
    fixture.write("Doc.md", "doc\n");
    fixture.folder().await;
    let collection = fixture.collection.clone();
    fixture
        .request(
            "documents.collection.archive",
            json!({ "collectionId": collection }),
        )
        .await;
    fs::remove_file(path(&fixture.root, "Doc.md")).unwrap();
    fixture.write("New.md", "new\n");
    let before = fixture.appends();
    assert_eq!(fixture.folder().await, Outcome::default());
    assert_eq!(fixture.mapping().state, MappingState::Paused);
    assert_eq!(fixture.appends(), before);

    fixture
        .request(
            "documents.collection.restore",
            json!({ "collectionId": collection }),
        )
        .await;
    let outcome = fixture.folder().await;
    assert_eq!(fixture.mapping().state, MappingState::Active);
    assert_eq!(outcome.archived, 1, "{outcome:?}");
    assert_eq!(outcome.created, 1, "{outcome:?}");
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14 (What is synced, No lost text) — symbolic links are never followed: a linked `.md` file is refused as `symlink`.
#[cfg(unix)]
#[tokio::test]
async fn a_linked_file_is_refused_and_not_followed() {
    let fixture = Fixture::new().await;
    let outside = fixture.root.with_file_name("outside.md");
    fs::write(&outside, "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, path(&fixture.root, "Link.md")).unwrap();
    let outcome = fixture.folder().await;
    assert_eq!(outcome.refused, 1, "{outcome:?}");
    assert!(fixture.documents().is_empty());
    let refusals = fixture.refusals();
    assert_eq!(refusals[0].rel_path, "Link.md");
    assert_eq!(refusals[0].reason, Refusal::Symlink.as_str());
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15.
#[tokio::test]
async fn recursive_import_export_preserves_folders_and_basename_identity() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fs::create_dir_all(fixture.root.join("Plans/été")).unwrap();
    fixture.write("Plans/été/Note.md", "Nested\nwrapped.\n");
    fixture.write("Note.md", "Root.\n");
    assert_eq!(fixture.folder().await.created, 2);
    let nested = fixture
        .documents()
        .into_iter()
        .find(|d| d.folder == "Plans/été")
        .unwrap();
    assert_eq!(nested.title.as_deref(), Some("Note"));
    assert_eq!(fixture.text(&nested.id), "Nested wrapped.\n");
    assert_eq!(fixture.read("Plans/été/Note.md"), "Nested\nwrapped.\n");
    let exported = fixture.document("Note", "Exported.\n").await;
    fixture
        .request(
            "documents.document.place",
            json!({"documentId": exported, "folder": "New/deep"}),
        )
        .await;
    fixture.reconcile(&exported).await;
    assert_eq!(
        fixture.state(&exported).unwrap().rel_path,
        "New/deep/Note.md"
    );
    assert_eq!(fixture.read("New/deep/Note.md"), "Exported.\n");
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15.
#[tokio::test]
async fn recursive_moves_repeat_and_directory_rename_keep_ids_and_bases() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fixture.write("Note.md", "Original.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    let base = fixture.state(&document).unwrap();
    for folder in ["A", "B", "A", "B", ""] {
        let old = fixture.state(&document).unwrap().rel_path;
        fs::create_dir_all(fixture.root.join(folder)).unwrap();
        let new = if folder.is_empty() {
            "Note.md".to_owned()
        } else {
            format!("{folder}/Note.md")
        };
        fs::rename(fixture.root.join(old), fixture.root.join(&new)).unwrap();
        fixture.folder().await;
        assert_eq!(fixture.titled("Note").folder, folder);
        assert_eq!(fixture.titled("Note").id, document);
        let state = fixture.state(&document).unwrap();
        assert_eq!(state.base_version, base.base_version);
        assert_eq!(state.written_hash, base.written_hash);
        fixture.settled().await;
    }
    fixture.place(&document, "Old/deep").await;
    fixture.reconcile(&document).await;
    fs::write(fixture.root.join("Old/deep/Other.md"), "Other.\n").unwrap();
    fixture.folder().await;
    let other = fixture.titled("Other").id;
    fs::rename(fixture.root.join("Old"), fixture.root.join("Renamed")).unwrap();
    fixture.folder().await;
    assert_eq!(fixture.titled("Note").folder, "Renamed/deep");
    assert_eq!(fixture.titled("Other").folder, "Renamed/deep");
    assert_eq!(fixture.titled("Other").id, other);
    assert_eq!(fixture.documents().len(), 2);
    fixture.settled().await;
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15.
#[tokio::test]
async fn recursive_relocation_crash_refuses_changed_destination_without_duplicate_document() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fixture.write("Note.md", "Original.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    fixture.place(&document, "New").await;
    fixture.sync.crash_at(Crash::FileRelocated);
    assert_eq!(
        fixture
            .sync
            .reconcile_document(&document)
            .await
            .unwrap_err()
            .code(),
        "documents.test_crash"
    );
    fixture.write("New/Note.md", "Changed after move.\n");
    let fixture = fixture.restart().await;
    fixture.folder().await;
    assert_eq!(
        fixture.documents().len(),
        1,
        "an uncertain move must not create another Document"
    );
    assert_eq!(fixture.titled("Note").folder, "New");
    assert!(!fixture.titled("Note").archived);
    assert_eq!(fixture.read("New/Note.md"), "Changed after move.\n");
    assert!(!fixture.refusals().is_empty());
    fixture.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recursive_symlink_subtree_protects_documents_and_never_reads_outside() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new().await;
    fixture.recursive();
    fs::create_dir(fixture.root.join("Sub")).unwrap();
    fixture.write("Sub/Note.md", "Inside.\n");
    fixture.write("Sentinel.md", "Sentinel.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    let outside = fixture._folder.path().join("outside");
    fs::rename(fixture.root.join("Sub"), &outside).unwrap();
    fs::write(outside.join("Note.md"), "Never import.\n").unwrap();
    symlink(&outside, fixture.root.join("Sub")).unwrap();
    fixture.folder().await;
    assert!(!fixture.titled("Note").archived);
    fixture.reconcile(&document).await;
    assert_eq!(fixture.text(&document), "Inside.\n");
    assert_eq!(
        fs::read_to_string(outside.join("Note.md")).unwrap(),
        "Never import.\n"
    );
    assert!(
        fixture
            .refusals()
            .iter()
            .any(|r| r.rel_path == "Sub" && r.reason == "symlink")
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_ambiguous_hash_moves_refuse_without_archiving_or_pairing() {
    let fixture = Fixture::new().await;
    fixture.recursive();
    fixture.write("One.md", "Same.\n");
    fixture.write("Two.md", "Same.\n");
    fixture.folder().await;
    fs::create_dir(fixture.root.join("New")).unwrap();
    fs::rename(fixture.root.join("One.md"), fixture.root.join("New/One.md")).unwrap();
    fs::rename(fixture.root.join("Two.md"), fixture.root.join("New/Two.md")).unwrap();
    fixture.folder().await;
    assert_eq!(fixture.documents().len(), 2);
    assert!(
        fixture
            .documents()
            .iter()
            .all(|d| !d.archived && d.folder.is_empty())
    );
    assert_eq!(
        fixture.state(&fixture.titled("One").id).unwrap().rel_path,
        "One.md"
    );
    assert!(!fixture.refusals().is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_cleanup_only_removes_parents_emptied_by_our_operation() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    let document = fixture.document("Note", "Text.\n").await;
    fixture.place(&document, "Old/deep").await;
    fixture.reconcile(&document).await;
    fs::create_dir(fixture.root.join("UserEmpty")).unwrap();
    fixture.place(&document, "New/deep").await;
    fixture.reconcile(&document).await;
    assert!(!fixture.root.join("Old").exists());
    assert!(fixture.root.join("UserEmpty").is_dir());
    fixture.write("New/.keep", "hidden");
    fixture.place(&document, "").await;
    fixture.reconcile(&document).await;
    assert!(!fixture.root.join("New/deep").exists());
    assert!(fixture.root.join("New/.keep").is_file());
    assert!(fixture.root.is_dir());
    fixture.shutdown().await;
}

// Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15.
#[tokio::test]
async fn recursive_pending_body_move_and_placement_crash_recover_on_restart() {
    for crash in [Crash::PlacementAppended, Crash::FileRelocated] {
        let mut fixture = Fixture::new().await;
        fixture.recursive();
        fixture.write("Note.md", "First.\n");
        fixture.folder().await;
        let document = fixture.titled("Note").id;
        fixture.edit(&document, "First.\n\nSecond.\n").await;
        fixture.sync.crash_at(Crash::BodyWritten);
        assert_eq!(
            fixture
                .sync
                .reconcile_document(&document)
                .await
                .unwrap_err()
                .code(),
            "documents.test_crash"
        );
        assert!(
            fixture
                .state(&document)
                .unwrap()
                .pending_write_hash
                .is_some()
        );
        if crash == Crash::PlacementAppended {
            fs::create_dir(fixture.root.join("New")).unwrap();
            fs::rename(
                fixture.root.join("Note.md"),
                fixture.root.join("New/Renamed.md"),
            )
            .unwrap();
            fixture.sync.crash_at(crash);
            assert_eq!(
                fixture
                    .sync
                    .reconcile_folder(&fixture.collection)
                    .await
                    .unwrap_err()
                    .code(),
                "documents.test_crash"
            );
        } else {
            fixture.place(&document, "New").await;
            fixture.sync.crash_at(crash);
            assert_eq!(
                fixture
                    .sync
                    .reconcile_document(&document)
                    .await
                    .unwrap_err()
                    .code(),
                "documents.test_crash"
            );
        }
        let persisted = fixture
            .session
            .handles
            .store
            .file_relocation(&document)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.source_path, "Note.md");
        assert_eq!(persisted.expected_hash, sha256(b"First.\n\nSecond.\n"));
        let fixture = fixture.restart().await;
        fixture.folder().await;
        assert_eq!(fixture.documents().len(), 1);
        assert_eq!(fixture.documents()[0].id, document);
        assert_eq!(fixture.documents()[0].folder, "New");
        assert_eq!(fixture.text(&document), "First.\n\nSecond.\n");
        let state = fixture.state(&document).unwrap();
        assert_eq!(fixture.read(&state.rel_path), "First.\n\nSecond.\n");
        assert!(state.pending_write_hash.is_none());
        assert!(
            fixture
                .session
                .handles
                .store
                .file_relocation(&document)
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .session
                .scripted
                .recorded_calls()
                .iter()
                .filter(|call| call.method == "append")
                .all(|call| call.payload["purpose"] == "document.rename"),
            "only the interrupted basename rename may still need appending"
        );
        fixture.settled().await;
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn recursive_remote_move_imports_source_edit_and_refuses_occupied_target() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fixture.write("Note.md", "First.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    fs::create_dir(fixture.root.join("New")).unwrap();
    fixture.write("New/Note.md", "Unrelated.\n");
    fixture.write("Note.md", "First.\n\nOutside.\n");
    fixture.place(&document, "New").await;
    fixture.reconcile(&document).await;
    assert_eq!(fixture.text(&document), "First.\n\nOutside.\n");
    assert_eq!(fixture.read("New/Note.md"), "Unrelated.\n");
    assert_eq!(fixture.read("Note.md"), "First.\n\nOutside.\n");
    assert!(!fixture.refusals().is_empty());
    fs::remove_file(fixture.root.join("New/Note.md")).unwrap();
    fixture.folder().await;
    assert_eq!(fixture.state(&document).unwrap().rel_path, "New/Note.md");
    assert_eq!(fixture.read("New/Note.md"), "First.\n\nOutside.\n");
    fixture.settled().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_subtree_delete_restore_and_only_subtree_guard() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fs::create_dir_all(fixture.root.join("Sub/deep")).unwrap();
    fixture.write("Sub/deep/Note.md", "Nested.\n");
    fixture.write("Sentinel.md", "Keep.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    fs::remove_dir_all(fixture.root.join("Sub")).unwrap();
    assert_eq!(fixture.folder().await.archived, 1);
    assert!(fixture.titled("Note").archived);
    fixture
        .request("documents.document.restore", json!({"documentId":document}))
        .await;
    fixture.reconcile(&document).await;
    assert_eq!(fixture.read("Sub/deep/Note.md"), "Nested.\n");
    fs::remove_file(fixture.root.join("Sentinel.md")).unwrap();
    fixture.folder().await;
    fs::remove_dir_all(fixture.root.join("Sub")).unwrap();
    assert_eq!(fixture.folder().await.archived, 0);
    assert_eq!(fixture.mapping().state, MappingState::Unavailable);
    assert!(!fixture.titled("Note").archived);
    fixture.shutdown().await;
}

#[tokio::test]
async fn flat_mapping_keeps_root_associations_and_ignores_remote_folders_after_restart() {
    let mut fixture = Fixture::new().await;
    fixture.write("Note.md", "Root.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    fixture.place(&document, "Remote").await;
    fixture.reconcile(&document).await;
    fs::create_dir(fixture.root.join("Ignored")).unwrap();
    fixture.write("Ignored/Private.md", "Private.\n");
    let new = fixture.document("New", "Not flat.\n").await;
    fixture.place(&new, "Remote").await;
    fixture.reconcile(&new).await;
    let fixture = fixture.restart().await;
    fixture.write("Note.md", "Root edited.\n");
    fs::rename(fixture.root.join("Note.md"), fixture.root.join("Local.md")).unwrap();
    // Import the edit first at the last associated path; changed-content moves are not promised.
    fs::rename(fixture.root.join("Local.md"), fixture.root.join("Note.md")).unwrap();
    fixture.folder().await;
    fs::rename(fixture.root.join("Note.md"), fixture.root.join("Local.md")).unwrap();
    fixture.folder().await;
    assert_eq!(fixture.documents().len(), 2);
    assert_eq!(fixture.titled("Local").folder, "Remote");
    assert_eq!(fixture.text(&document), "Root edited.\n");
    assert_eq!(fixture.state(&document).unwrap().rel_path, "Local.md");
    assert!(!fixture.mapping().recursive);
    assert!(!fixture.root.join("Remote").exists());
    assert!(!fixture.root.join("New.md").exists());
    fixture.settled().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_unicode_case_moves_and_folder_scoped_suffixes() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    fs::create_dir(fixture.root.join("Cafe\u{301}")).unwrap();
    fixture.write("Cafe\u{301}/Note.md", "Original.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    assert_eq!(fixture.titled("Note").folder, "Café");
    fs::rename(
        fixture.root.join("Cafe\u{301}/Note.md"),
        fixture.root.join("Cafe\u{301}/note.md"),
    )
    .unwrap();
    fixture.folder().await;
    assert_eq!(fixture.titled("note").id, document);
    let duplicate = fixture.document("note", "Second.\n").await;
    fixture.place(&duplicate, "Café").await;
    fixture.reconcile(&duplicate).await;
    assert_eq!(fixture.read("Cafe\u{301}/note (2).md"), "Second.\n");
    fixture.place(&document, "CAFÉ").await;
    fixture.reconcile(&document).await;
    assert_eq!(fixture.read("Cafe\u{301}/note.md"), "Original.\n");
    assert!(
        !fixture.refusals().is_empty(),
        "a distinct logical case alias cannot silently merge"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_invalid_folder_refusal_is_visible_through_mapping_list_contract() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    let document = fixture.document("Note", "Safe.\n").await;
    fixture.place(&document, ".hidden").await;
    fixture.reconcile(&document).await;
    let listed = fixture.request("documents.mapping.list", json!({})).await;
    assert_eq!(
        listed["collections"][0]["mapping"]["refused"][0]["reason"],
        "invalid_folder"
    );
    let compiled = teamarness_package_contract::ui_contract::compile_ui_contract(include_bytes!(
        "../../../ui-contract.json"
    ))
    .unwrap();
    compiled
        .validate_response("documents.mapping.list", &listed)
        .unwrap();
    assert_eq!(fixture.titled("Note").folder, ".hidden");
    assert!(!fixture.root.join(".hidden").exists());
    fixture.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn recursive_unreadable_and_evicted_subtrees_preserve_associations() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new().await;
    fixture.recursive();
    fs::create_dir(fixture.root.join("Sub")).unwrap();
    fixture.write("Sub/Note.md", "Private.\n");
    fixture.write("Sentinel.md", "Keep.\n");
    fixture.folder().await;
    let document = fixture.titled("Note").id;
    let before = fixture.state(&document).unwrap();
    fs::set_permissions(fixture.root.join("Sub"), fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = fixture.folder().await;
    fs::set_permissions(fixture.root.join("Sub"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(outcome.archived, 0);
    assert_eq!(fixture.state(&document).unwrap(), before);
    assert!(
        fixture
            .refusals()
            .iter()
            .any(|r| r.rel_path == "Sub" && r.reason == "unreadable")
    );
    fs::rename(
        fixture.root.join("Sub/Note.md"),
        fixture.root.join("Sub/.Note.md.icloud"),
    )
    .unwrap();
    assert_eq!(fixture.folder().await.archived, 0);
    assert_eq!(fixture.state(&document).unwrap(), before);
    fixture.shutdown().await;
}

#[tokio::test]
async fn recursive_collection_move_preserves_folder_and_imports_source_bytes() {
    let mut fixture = Fixture::new().await;
    fixture.recursive();
    let document = fixture.document("Note", "First.\n").await;
    fixture.place(&document, "Sub/deep").await;
    fixture.reconcile(&document).await;
    fixture.write("Sub/deep/Note.md", "First.\n\nOutside.\n");
    let collection = fixture
        .request("documents.collection.create", json!({"name":"Other"}))
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let destination = fixture._folder.path().join("other");
    fs::create_dir(&destination).unwrap();
    let mut mapping = fixture.mapping();
    mapping.collection_id = collection.clone();
    mapping.root_path = destination.to_str().unwrap().to_owned();
    fixture
        .session
        .handles
        .store
        .set_folder_mapping(&mapping)
        .unwrap();
    fixture
        .request(
            "documents.document.move",
            json!({"documentId":document,"collectionId":collection}),
        )
        .await;
    fixture.reconcile(&document).await;
    assert_eq!(fixture.text(&document), "First.\n\nOutside.\n");
    assert_eq!(
        fs::read_to_string(destination.join("Sub/deep/Note.md")).unwrap(),
        "First.\n\nOutside.\n"
    );
    assert!(!fixture.root.join("Sub").exists());
    fixture.shutdown().await;
}
