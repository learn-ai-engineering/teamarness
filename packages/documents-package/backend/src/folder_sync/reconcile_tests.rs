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
use super::super::{FolderSync, Outcome, Refusal};
use super::{Crash, request_id, title_of};
use crate::backend::tests::{
    ALICE, Session, Writer, deliver, hex, qualify_empty_history, remote_edit, session_update,
    start, submit, wait_until,
};
use crate::store::DocumentView;
use crate::store::mappings::{FolderMapping, MappedFile, MappingState, RefusedFile};

/// A record log that signs every append as Alice, keeps it, and opens what it kept, so an
/// inclusion check can read a request back.
fn remembering_log() -> ScriptedCapability {
    let records = Arc::new(Mutex::new(HashMap::<String, (u64, String, String)>::new()));
    let next = Arc::new(AtomicU64::new(100));
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
        let mut session = start(data.path(), remembering_log()).await;
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
            })
            .unwrap();
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

// Verifies: B-4, B-8 — an export mapping writes every non-archived Document as its Canonical
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

// Verifies: B-3, B-7, B-9 — an import mapping creates one Document per file, titled by its
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

// Verifies: B-6, B-7, B-8 — a changed file is imported as one revision; an unchanged one is
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

// Verifies: B-5, B-7 — a file edit that is not yet imported is never overwritten: when a
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

// Verifies: B-10 — a file whose import would lose text stays byte for byte and is refused as
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

// Verifies: B-11 — an oversize file is refused before any record: no Document, empty or not.
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

// Verifies: B-9 — a missing root sets the mapping `unavailable` and archives nothing; when it
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

// Verifies: B-9 — a deleted file archives its Document; a file renamed outside renames its
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

// Verifies: B-9 — an archive while the file holds an unimported edit imports the edit first,
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

// Verifies: B-9 — names that differ only in case never share a file: Documents get distinct
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

// Verifies: B-12 — a crash after an import's append and before its file state is saved is
// retried as the same request: one revision, and no text twice.
#[tokio::test]
async fn a_crash_after_the_append_never_duplicates_the_revision() {
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", "First.\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;
    let stale = fixture.state(&document).unwrap();

    fixture.write("Doc.md", "First.\n\nSecond.\n");
    fixture.sync.crash_at(Crash::AfterImportAppend);
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

// Verifies: B-12 — a crash after a write-back renamed its file into place and before
// `finish_write` adopts the file on the next reconcile instead of importing it.
#[tokio::test]
async fn a_crash_after_the_rename_adopts_the_written_file() {
    let fixture = Fixture::new().await;
    fixture.write("Doc.md", "First.\n");
    fixture.folder().await;
    let document = fixture.titled("Doc").id;
    fixture.edit(&document, "First.\n\nFrom the app.\n").await;

    fixture.sync.crash_at(Crash::AfterWriteRename);
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

// Verifies: B-6 — an offline batch of 50 edits, 3 new files and 2 deletions imports as exactly
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

// Verifies: an archived Collection pauses its mapping and leaves its files; a restore resumes it.
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

// Verifies: symbolic links are never followed: a linked `.md` file is refused as `symlink`.
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
