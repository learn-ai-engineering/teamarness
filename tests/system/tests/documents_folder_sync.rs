//! Two real loopback Installations, Core/Host/Backends and filesystem watchers qualify
//! recursive mappings, local consent, concurrent edits, repeated moves, subtree lifecycle,
//! and a nested offline batch with no Documents UI mounted on restart.
//! Bounded polling avoids assuming watcher timing. Latency is printed, not asserted.
//! This qualifies the local macOS watcher, not Linux/Windows or a real editor save.
#![cfg(unix)]
#[allow(dead_code)] // Shared qualification helpers are used by different integration targets.
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use support::harness::{
    self, Addressing, PackageStage, create_invitation, introduce, open_core, open_host, open_mount,
    shutdown,
};
use teamarness_core::CoreRuntime;
use teamarness_documents_package::{
    DocumentStore,
    markdown::{export, merge},
};
use teamarness_package_host::{
    CloseUiSubscription, OpenUiSubscription, PackageHost, PackageHostCommand,
    PackageHostNotification, PackageHostOutcome, PackageHostSubscriptionSelector, PackageIdentity,
    PackageInstanceKey, PackageValue, UiMountId, UiRequest, UiRequestId, UiSubscriptionId,
    WorkspaceId,
};

const DOCUMENTS: &str = "dev.teamarness.documents";
const DEADLINE: Duration = Duration::from_secs(90);
static NEXT: AtomicU64 = AtomicU64::new(1);

const PLAN: &str = "# Plan\n\nFirst paragraph about scope.\n\nSecond paragraph about risk.\n\nThird paragraph about delivery.\n\n- one\n- two\n";
const RISKS: &str = "# Risks\n\n- Late *delivery*\n- Scope creep\n";
const IDEAS: &str = "## Ideas\n\nWhy `folders`?\n\n> Because people use editors.\n";

/// One Installation's private paths, so it can be stopped and reopened.
struct Paths {
    core: PathBuf,
    private: PathBuf,
}

struct Installation {
    core: Arc<CoreRuntime>,
    host: PackageHost,
    mount: Option<UiMountId>,
}

impl Installation {
    async fn open(application: &Path, paths: &Paths, key: &PackageInstanceKey) -> Self {
        let core = open_core(Addressing::Loopback, &paths.core).await;
        Self::with_core(application, paths, key, core).await
    }

    async fn with_core(
        application: &Path,
        paths: &Paths,
        key: &PackageInstanceKey,
        core: Arc<CoreRuntime>,
    ) -> Self {
        let host = open_host(application, paths.private.clone(), core.clone()).await;
        let mount = open_mount(&host, key).await;
        Self {
            core,
            host,
            mount: Some(mount),
        }
    }

    async fn open_background(application: &Path, paths: &Paths) -> Self {
        let core = open_core(Addressing::Loopback, &paths.core).await;
        let host = open_host(application, paths.private.clone(), core.clone()).await;
        Self {
            core,
            host,
            mount: None,
        }
    }

    async fn mount(&mut self, key: &PackageInstanceKey) {
        assert!(self.mount.is_none());
        self.mount = Some(open_mount(&self.host, key).await);
    }

    async fn stop(self) {
        shutdown(self.host).await;
        self.core.shutdown().await.unwrap();
    }

    async fn try_ui(&self, operation: &str, payload: Value) -> Result<Value, String> {
        let id = format!("folder-sync-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        match self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.mount.clone().expect("this caller needs a mounted UI"),
                request: UiRequestId::new(id).unwrap(),
                operation: operation.into(),
                payload: serde_json::from_value::<PackageValue>(payload).unwrap(),
            }))
            .await
        {
            Ok(PackageHostOutcome::UiResponse { payload }) => {
                Ok(serde_json::to_value(payload).unwrap())
            }
            Ok(other) => Err(format!("{other:?}")),
            Err(error) => Err(format!("{error:?}")),
        }
    }

    async fn ui(&self, operation: &str, payload: Value) -> Value {
        self.try_ui(operation, payload.clone())
            .await
            .unwrap_or_else(|cause| panic!("{operation} {payload}: {cause}"))
    }

    /// The Document's Canonical Markdown, or `None` while it is not readable here.
    async fn markdown(&self, document: &str) -> Option<String> {
        let first = self
            .try_ui(
                "documents.document.markdown",
                json!({ "documentId": document, "chunkIndex": 0 }),
            )
            .await
            .ok()?;
        let mut text = first["chunk"].as_str()?.to_owned();
        for index in 1..first["chunks"].as_u64()? {
            let next = self
                .try_ui(
                    "documents.document.markdown",
                    json!({ "documentId": document, "chunkIndex": index }),
                )
                .await
                .ok()?;
            text.push_str(next["chunk"].as_str()?);
        }
        Some(text)
    }

    async fn snapshot(&self) -> Value {
        self.ui("documents.snapshot", json!({})).await
    }

    /// Opens an editor session, as the UI does, and returns its doc, peer and subscription.
    async fn session(&self, document: &str) -> (loro::LoroDoc, u64, UiSubscriptionId) {
        let subscription = UiSubscriptionId::new(format!(
            "folder-sync-session-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        self.host
            .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
                mount: self.mount.clone().expect("this caller needs a mounted UI"),
                subscription: subscription.clone(),
                operation: "documents.document.session".into(),
                payload: serde_json::from_value(json!({ "documentId": document })).unwrap(),
            }))
            .await
            .unwrap();
        let mut events = self
            .host
            .subscribe(PackageHostSubscriptionSelector::UiEvents {
                mount: self.mount.clone().expect("this caller needs a mounted UI"),
                subscription: subscription.clone(),
            })
            .await
            .unwrap();
        let mut opening: Vec<Value> = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            while opening.last().is_none_or(|event| event["event"] != "end") {
                if let PackageHostNotification::UiEvent(event) = events.changed().await.unwrap() {
                    opening.push(serde_json::to_value(&event.payload).unwrap());
                }
            }
        })
        .await
        .expect("the editor session opens");
        let begin = &opening[0];
        assert_eq!(begin["editable"], true, "{begin}");
        let mut snapshot = Vec::new();
        for chunk in &opening[1..opening.len() - 1] {
            snapshot.extend(STANDARD.decode(chunk["chunk"].as_str().unwrap()).unwrap());
        }
        let doc = loro::LoroDoc::new();
        doc.import(&snapshot).unwrap();
        let peer: u64 = begin["sessionPeer"].as_str().unwrap().parse().unwrap();
        (doc, peer, subscription)
    }

    async fn close(&self, subscription: UiSubscriptionId) {
        self.host
            .execute(PackageHostCommand::CloseUiSubscription(
                CloseUiSubscription {
                    mount: self.mount.clone().expect("this caller needs a mounted UI"),
                    subscription,
                },
            ))
            .await
            .unwrap();
    }

    /// A Person editing in the Documents UI: one session update that merges the revised
    /// Markdown as the session's peer, sent through `documents.document.update`.
    async fn edit_in_app(&self, document: &str, revise: impl Fn(&str) -> String) {
        let (doc, peer, subscription) = self.session(document).await;
        let current = export(&doc).unwrap();
        let merged = merge(&doc, &doc.oplog_frontiers(), &revise(&current), peer).unwrap();
        let request_id = format!(
            "{:032x}",
            u128::from(NEXT.fetch_add(1, Ordering::Relaxed)) | (0xf01d << 64)
        );
        let answer = self
            .ui(
                "documents.document.update",
                json!({
                    "documentId": document, "sessionPeer": peer.to_string(),
                    "requestId": request_id, "update": STANDARD.encode(&merged.update),
                }),
            )
            .await;
        assert_eq!(answer["requestId"], request_id);
        self.close(subscription).await;
    }

    /// How many peers have written the Document's body here. Every file import and every
    /// editor session writes as a fresh peer, so this counts the body records written.
    async fn writers(&self, document: &str) -> usize {
        let (doc, _, subscription) = self.session(document).await;
        self.close(subscription).await;
        doc.oplog_vv().len()
    }

    async fn create_document(&self, collection: &str, title: &str, markdown: &str) -> String {
        let id = self
            .ui(
                "documents.document.create",
                json!({ "collectionId": collection, "title": title }),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.edit_in_app(&id, |_| markdown.to_owned()).await;
        id
    }
}

fn document_folder<'a>(snapshot: &'a Value, id: &str) -> Option<&'a str> {
    snapshot["documents"]
        .as_array()?
        .iter()
        .find(|document| document["id"] == id)?["folder"]
        .as_str()
}

fn record_count(core: &CoreRuntime, workspace: teamarness_core::WorkspaceId) -> usize {
    let mut records = core.subscribe_records(workspace, DOCUMENTS, None).unwrap();
    let mut count = 0;
    loop {
        let page = records.next_page(256).unwrap();
        if page.events.is_empty() {
            return count;
        }
        count += page.events.len();
    }
}

fn documents_database(root: &Path) -> PathBuf {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .is_some_and(|name| name == "documents.sqlite3")
        {
            return path;
        }
        if path.is_dir() {
            let found = documents_database(&path);
            if found.is_file() {
                return found;
            }
        }
    }
    PathBuf::new()
}

fn document_id(snapshot: &Value, title: &str) -> Option<String> {
    snapshot["documents"]
        .as_array()?
        .iter()
        .find(|document| document["title"] == title && document["archived"] == false)
        .map(|document| document["id"].as_str().unwrap().to_owned())
}

fn is_archived(snapshot: &Value, id: &str) -> bool {
    snapshot["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|document| document["id"] == id && document["archived"] == true)
}

fn live_titles(snapshot: &Value) -> Vec<String> {
    snapshot["documents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|document| document["archived"] == false)
        .map(|document| document["title"].as_str().unwrap().to_owned())
        .collect()
}

/// Text that no Markdown rule touches, `bytes` long.
fn unique_text(bytes: usize) -> String {
    let mut text = String::new();
    let mut index = 0u64;
    while text.len() < bytes {
        text.push_str(&format!(
            "{:016x} ",
            index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ));
        index += 1;
    }
    text.truncate(bytes);
    text.trim_end().to_owned()
}

/// Polls `ready` until it holds or `timeout` passes, and returns how long it took.
async fn poll(timeout: Duration, mut ready: impl AsyncFnMut() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready().await {
            return Some(start.elapsed());
        }
        // poll-interval: bounded by `timeout` above.
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Verifies: TEAM.SYS.DOCS-R14, TEAM.SYS.DOCS-R15 — recursive sync and local consent end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn recursive_mappings_preserve_identity_consent_and_nested_changes() {
    let application = harness::StagedApplication::with(&PackageStage::documents());
    let app = application.application_path.clone();
    let root = application.root.path().to_owned();
    let alice_paths = Paths {
        core: root.join("alice-core"),
        private: root.join("alice-private"),
    };
    let bob_paths = Paths {
        core: root.join("bob-core"),
        private: root.join("bob-private"),
    };
    let mapped = tempfile::tempdir().unwrap();
    let folder = mapped.path().join("notes");
    fs::create_dir(&folder).unwrap();
    let bob_folder = mapped.path().join("bob-notes");
    fs::create_dir(&bob_folder).unwrap();

    let alice_core = open_core(Addressing::Loopback, &alice_paths.core).await;
    let workspace = alice_core.create_workspace("Folder sync").unwrap().id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(DOCUMENTS).unwrap(),
        WorkspaceId::new(workspace.to_string()).unwrap(),
    );
    let alice = Installation::with_core(&app, &alice_paths, &key, alice_core).await;
    let collection = alice
        .ui("documents.collection.create", json!({ "name": "Notes" }))
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan = alice.create_document(&collection, "Plan", PLAN).await;
    let risks = alice.create_document(&collection, "Risks", RISKS).await;
    let ideas = alice.create_document(&collection, "Ideas", IDEAS).await;

    let invitation = create_invitation(&alice.core, workspace).await;
    let bob_core = open_core(Addressing::Loopback, &bob_paths.core).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob = Installation::with_core(&app, &bob_paths, &key, bob_core).await;
    let alice_plan = alice.markdown(&plan).await.unwrap();
    assert!(
        poll(DEADLINE, async || bob.markdown(&plan).await.as_deref()
            == Some(alice_plan.as_str()))
        .await
        .is_some(),
        "Bob did not receive Alice's Documents"
    );

    // (1) Export: an empty folder receives every Document as its Canonical Markdown.
    let preview = alice
        .ui(
            "documents.mapping.set",
            json!({
                "collectionId": collection, "path": folder, "dryRun": true
            }),
        )
        .await;
    assert_eq!(preview["direction"], "export");
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    let mapping = alice
        .ui(
            "documents.mapping.set",
            json!({ "collectionId": collection, "path": folder.to_str().unwrap() }),
        )
        .await;
    assert_eq!(mapping["direction"], "export");
    let files = [
        ("Plan.md", &plan),
        ("Risks.md", &risks),
        ("Ideas.md", &ideas),
    ];
    assert!(
        poll(DEADLINE, async || files
            .iter()
            .all(|(name, _)| folder.join(name).is_file()))
        .await
        .is_some(),
        "the export did not write every file"
    );
    for (name, id) in files {
        assert_eq!(
            Some(read(&folder.join(name))),
            alice.markdown(id).await,
            "{name} is not its Document's Canonical Markdown"
        );
    }

    // Seed a legacy flat mapping through the real store while Bob is stopped. Migration
    // defaults are covered by the store tests; this fixture retains real exported file bases.
    bob.ui(
        "documents.mapping.set",
        json!({"collectionId": collection, "path": bob_folder}),
    )
    .await;
    poll(DEADLINE, async || {
        read(&bob_folder.join("Plan.md")) == alice_plan
    })
    .await
    .expect("Bob exports the root Document");
    bob.stop().await;
    let database = documents_database(&bob_paths.private);
    let store = DocumentStore::open(&database).unwrap();
    let mut legacy = store.folder_mapping(&collection).unwrap().unwrap();
    legacy.recursive = false;
    store.set_folder_mapping(&legacy).unwrap();
    let root_state = store.mapped_file(&plan).unwrap().unwrap();
    drop(store);
    fs::create_dir_all(bob_folder.join("private/drafts")).unwrap();
    let private_text = "# Private draft\n\nNot shared before consent.\n";
    fs::write(bob_folder.join("private/drafts/Private.md"), private_text).unwrap();
    let bob = Installation::open(&app, &bob_paths, &key).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);

    // Alice's recursive mapping does not opt Bob in. A tracked root file stays associated
    // after remote placement; a newly imported nested Document must not flatten on Bob.
    alice
        .ui(
            "documents.document.place",
            json!({"documentId": plan, "folder": "planning/current"}),
        )
        .await;
    fs::create_dir_all(folder.join("incoming/deep")).unwrap();
    let nested_text = "# Nested\n\nImported below the root.\n";
    fs::write(folder.join("incoming/deep/Nested.md"), nested_text).unwrap();
    poll(DEADLINE, async || {
        let snapshot = bob.snapshot().await;
        document_folder(&snapshot, &plan) == Some("planning/current")
            && document_id(&snapshot, "Nested").is_some()
            && folder.join("planning/current/Plan.md").is_file()
    })
    .await
    .expect("nested data reaches Bob's projection");
    let nested = document_id(&bob.snapshot().await, "Nested").unwrap();
    // negative-wait: give Bob's flat worker time to react to remote placement and nested files.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(read(&bob_folder.join("Plan.md")), alice_plan);
    assert!(!bob_folder.join("planning").exists());
    assert!(!bob_folder.join("incoming").exists());
    assert!(!bob_folder.join("Nested.md").exists());
    assert!(document_id(&alice.snapshot().await, "Private").is_none());
    assert_eq!(
        read(&bob_folder.join("private/drafts/Private.md")),
        private_text
    );
    let store = DocumentStore::open(&database).unwrap();
    assert!(
        !store
            .folder_mapping(&collection)
            .unwrap()
            .unwrap()
            .recursive
    );
    assert_eq!(store.mapped_file(&plan).unwrap().unwrap(), root_state);
    let before_files = store.mapped_files(&collection).unwrap();
    let before_records = record_count(&bob.core, workspace);
    let preview = bob
        .ui(
            "documents.mapping.enable-recursive",
            json!({
                "collectionId": collection, "dryRun": true
            }),
        )
        .await;
    assert_eq!(preview["recursive"], false, "{preview}");
    assert_eq!(preview["report"]["counts"]["files"], 1);
    assert_eq!(
        preview["report"]["files"][0]["name"],
        "private/drafts/Private.md"
    );
    assert_eq!(preview["report"]["counts"]["relocations"], 2);
    assert_eq!(preview["report"]["blocked"], false);
    assert!(
        preview["report"]["relocations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["documentId"] == plan
                && entry["from"] == "Plan.md"
                && entry["to"] == "planning/current/Plan.md")
    );
    assert_eq!(store.mapped_files(&collection).unwrap(), before_files);
    assert_eq!(record_count(&bob.core, workspace), before_records);
    assert!(
        !store
            .folder_mapping(&collection)
            .unwrap()
            .unwrap()
            .recursive
    );
    assert!(!bob_folder.join("planning").exists());
    assert_eq!(read(&bob_folder.join("Plan.md")), alice_plan);
    bob.ui(
        "documents.mapping.enable-recursive",
        json!({"collectionId": collection}),
    )
    .await;
    poll(DEADLINE, async || {
        read(&bob_folder.join("planning/current/Plan.md")) == alice_plan
            && read(&bob_folder.join("incoming/deep/Nested.md")) == nested_text
            && document_id(&alice.snapshot().await, "Private").is_some()
    })
    .await
    .expect("Bob independently enables subfolders and imports the private draft once");
    let adopted = store.mapped_file(&plan).unwrap().unwrap();
    assert_eq!(adopted.document_id, root_state.document_id);
    assert_eq!(adopted.base_version, root_state.base_version);
    assert_eq!(adopted.written_hash, root_state.written_hash);
    assert_eq!(adopted.pending_write_hash, root_state.pending_write_hash);
    assert_eq!(
        adopted.pending_write_version,
        root_state.pending_write_version
    );
    assert_eq!(adopted.rel_path, "planning/current/Plan.md");
    assert!(
        store
            .folder_mapping(&collection)
            .unwrap()
            .unwrap()
            .recursive
    );
    drop(store);
    assert_eq!(
        document_folder(&bob.snapshot().await, &nested),
        Some("incoming/deep")
    );

    // (2) A file edit reaches Bob, and Bob's concurrent edit elsewhere survives.
    let plan_path = folder.join("planning/current/Plan.md");
    let edited = read(&plan_path).replacen("about risk", "about risk FILE-EDIT", 1);
    let wrote = Instant::now();
    let ((), ()) = tokio::join!(async { fs::write(&plan_path, &edited).unwrap() }, async {
        bob.edit_in_app(&plan, |markdown| {
            markdown.replacen("- two", "- two BOB-EDIT", 1)
        })
        .await;
    });
    poll(DEADLINE, async || {
        bob.markdown(&plan)
            .await
            .is_some_and(|markdown| markdown.contains("FILE-EDIT"))
    })
    .await
    .expect("the file edit did not reach Bob");
    println!(
        "latency: file edit on Alice -> Bob's Document: {} ms (with a concurrent edit by Bob)",
        wrote.elapsed().as_millis()
    );
    assert!(
        poll(DEADLINE, async || {
            let on_bob = bob.markdown(&plan).await.unwrap_or_default();
            let on_alice = alice.markdown(&plan).await.unwrap_or_default();
            let file = read(&plan_path);
            on_bob == on_alice
                && file == on_bob
                && read(&bob_folder.join("planning/current/Plan.md")) == on_bob
                && on_bob.matches("FILE-EDIT").count() == 1
                && on_bob.matches("BOB-EDIT").count() == 1
        })
        .await
        .is_some(),
        "the concurrent edits did not both survive in the Document and the file"
    );

    // (3) Bob's edit reaches Alice's file, and no echo record follows.
    let writers = bob.writers(&plan).await;
    bob.edit_in_app(&plan, |markdown| {
        markdown.replacen("about delivery", "about delivery BOB-APP", 1)
    })
    .await;
    assert!(
        poll(DEADLINE, async || read(&plan_path).contains("BOB-APP"))
            .await
            .is_some(),
        "Bob's edit did not reach Alice's file"
    );
    // negative-wait: long enough for an echo import to reach Bob, if Alice made one.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        bob.writers(&plan).await,
        writers + 1,
        "Alice's write-back was imported back as a record"
    );
    assert_eq!(Some(read(&plan_path)), bob.markdown(&plan).await);

    // Repeated hash-preserving moves retain the same ID and body on both Installations.
    let body = bob.markdown(&plan).await.unwrap();
    let before_moves = record_count(&bob.core, workspace);
    let mut previous = "planning/current";
    for destination in [
        "",
        "moves/one",
        "moves/two",
        "",
        "moves/one",
        "moves/two",
        "",
    ] {
        fs::create_dir_all(folder.join(destination)).unwrap();
        fs::rename(
            folder.join(previous).join("Plan.md"),
            folder.join(destination).join("Plan.md"),
        )
        .unwrap();
        poll(DEADLINE, async || {
            let snapshot = bob.snapshot().await;
            document_folder(&snapshot, &plan) == Some(destination)
                && read(&bob_folder.join(destination).join("Plan.md")) == body
                && !bob_folder.join(previous).join("Plan.md").exists()
                && (previous.is_empty() || !bob_folder.join(previous).exists())
        })
        .await
        .unwrap_or_else(|| {
            panic!(
                "move {previous:?} -> {destination:?}: remaining entries {:?}",
                fs::read_dir(bob_folder.join(previous)).map(|entries| entries
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<Vec<_>>())
            )
        });
        assert_eq!(
            document_id(&bob.snapshot().await, "Plan"),
            Some(plan.clone())
        );
        assert_eq!(alice.markdown(&plan).await.as_deref(), Some(body.as_str()));
        // Alice's on-disk operation leaves user-emptied parents alone. Bob's own move cleans up.
        if !previous.is_empty() {
            assert!(folder.join(previous).is_dir());
            assert!(!bob_folder.join(previous).exists());
        }
        previous = destination;
    }
    assert_eq!(
        record_count(&bob.core, workspace),
        before_moves + 7,
        "each move adds only placement, not body echoes"
    );
    let plan_path = folder.join("Plan.md");

    // A populated folder rename places every child, preserving IDs and unmanaged entries.
    for id in [&risks, &ideas] {
        alice
            .ui(
                "documents.document.place",
                json!({"documentId": id, "folder": "old/deep"}),
            )
            .await;
    }
    poll(DEADLINE, async || {
        ["Risks.md", "Ideas.md"].iter().all(|name| {
            folder.join("old/deep").join(name).is_file()
                && bob_folder.join("old/deep").join(name).is_file()
        })
    })
    .await
    .expect("parents are created for both Documents");
    fs::write(bob_folder.join("old/deep/keep.txt"), "unmanaged").unwrap();
    fs::rename(folder.join("old"), folder.join("renamed")).unwrap();
    poll(DEADLINE, async || {
        let snapshot = bob.snapshot().await;
        [&risks, &ideas]
            .iter()
            .all(|id| document_folder(&snapshot, id) == Some("renamed/deep"))
            && read(&bob_folder.join("renamed/deep/Risks.md")) == RISKS
            && read(&bob_folder.join("renamed/deep/Ideas.md")) == IDEAS
    })
    .await
    .expect("folder rename preserves each Document");
    assert_eq!(
        document_id(&bob.snapshot().await, "Risks"),
        Some(risks.clone())
    );
    assert_eq!(
        document_id(&bob.snapshot().await, "Ideas"),
        Some(ideas.clone())
    );
    assert_eq!(read(&bob_folder.join("old/deep/keep.txt")), "unmanaged");
    assert!(bob_folder.is_dir());
    poll(DEADLINE, async || {
        record_count(&alice.core, workspace) == record_count(&bob.core, workspace)
    })
    .await
    .expect("record counts converge after moves");
    let settled = record_count(&bob.core, workspace);
    // negative-wait: moves and write-back must not generate an echo after settling.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(record_count(&bob.core, workspace), settled);
    assert_eq!(record_count(&alice.core, workspace), settled);

    // Online subtree deletion is known absence because Plan.md remains at root.
    fs::remove_dir_all(folder.join("incoming")).unwrap();
    poll(DEADLINE, async || {
        is_archived(&bob.snapshot().await, &nested) && !bob_folder.join("incoming").exists()
    })
    .await
    .expect("online subtree deletion archives remotely and cleans its empty parents");
    assert!(folder.join("Plan.md").is_file());
    bob.ui("documents.document.restore", json!({"documentId": nested}))
        .await;
    poll(DEADLINE, async || {
        read(&folder.join("incoming/deep/Nested.md")) == nested_text
            && read(&bob_folder.join("incoming/deep/Nested.md")) == nested_text
    })
    .await
    .expect("online restore recreates the subtree on both Installations");
    assert_eq!(
        document_id(&bob.snapshot().await, "Nested"),
        Some(nested.clone())
    );

    // (4) Offline: 50 edits, 3 new files, 2 deletions and 1 rename while Alice is stopped.
    fs::create_dir_all(folder.join("bulk/deep")).unwrap();
    for index in 0..51 {
        fs::write(
            folder.join(format!("bulk/deep/bulk-{index:02}.md")),
            format!("# Bulk {index}\n\nParagraph one of bulk {index}.\n"),
        )
        .unwrap();
    }
    let mut bulk: Vec<String> = Vec::new();
    assert!(
        poll(DEADLINE, async || {
            let snapshot = bob.snapshot().await;
            let found: Option<Vec<String>> = (0..51)
                .map(|index| document_id(&snapshot, &format!("bulk-{index:02}")))
                .collect();
            let Some(found) = found else {
                return false;
            };
            if bob
                .markdown(&found[50])
                .await
                .is_none_or(|markdown| !markdown.contains("bulk 50."))
            {
                return false;
            }
            bulk = found;
            true
        })
        .await
        .is_some(),
        "the 51 new files did not become Documents on Bob"
    );
    // settle: let Alice's folder sync finish its last pass before it stops.
    tokio::time::sleep(Duration::from_secs(1)).await;
    alice.stop().await;
    for index in 1..51 {
        let path = folder.join(format!("bulk/deep/bulk-{index:02}.md"));
        let text = read(&path);
        fs::write(&path, format!("{text}\nOffline edit {index}.\n")).unwrap();
    }
    for index in 0..3 {
        fs::write(
            folder.join(format!("bulk/deep/offline-{index}.md")),
            format!("# Offline {index}\n\nCreated offline.\n"),
        )
        .unwrap();
    }
    fs::remove_dir_all(folder.join("renamed")).unwrap();
    fs::create_dir_all(folder.join("offline/moved")).unwrap();
    fs::rename(
        folder.join("bulk/deep/bulk-00.md"),
        folder.join("offline/moved/renamed-00.md"),
    )
    .unwrap();
    let restarted = Instant::now();
    let mut alice = Installation::open_background(&app, &alice_paths).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);
    poll(Duration::from_secs(180), async || {
        let snapshot = bob.snapshot().await;
        if !(is_archived(&snapshot, &risks) && is_archived(&snapshot, &ideas)) {
            return false;
        }
        if document_id(&snapshot, "renamed-00").as_ref() != Some(&bulk[0])
            || document_folder(&snapshot, &bulk[0]) != Some("offline/moved")
        {
            return false;
        }
        for index in 0..3 {
            let Some(id) = document_id(&snapshot, &format!("offline-{index}")) else {
                return false;
            };
            if bob
                .markdown(&id)
                .await
                .is_none_or(|markdown| !markdown.contains("Created offline."))
            {
                return false;
            }
        }
        for (index, id) in bulk.iter().enumerate().skip(1) {
            if bob
                .markdown(id)
                .await
                .is_none_or(|markdown| !markdown.contains(&format!("Offline edit {index}.")))
            {
                return false;
            }
        }
        true
    })
    .await
    .expect("Bob did not converge on the offline changes");
    println!(
        "latency: Alice restarted -> Bob converged on the offline changes: {} ms",
        restarted.elapsed().as_millis()
    );
    let snapshot = bob.snapshot().await;
    let mut titles = live_titles(&snapshot);
    titles.sort();
    let count = titles.len();
    titles.dedup();
    assert_eq!(titles.len(), count, "a title is duplicated: {titles:?}");
    // Plan, Nested, Private, 50 bulk files, the renamed one and 3 new files.
    assert_eq!(count, 3 + 50 + 1 + 3, "{titles:?}");
    for (index, id) in bulk.iter().enumerate().skip(1) {
        let markdown = bob.markdown(id).await.unwrap();
        assert_eq!(
            markdown.matches(&format!("Offline edit {index}.")).count(),
            1,
            "bulk-{index:02} duplicated its edit: {markdown}"
        );
        assert_eq!(markdown.matches("Paragraph one").count(), 1, "{markdown}");
    }

    poll(DEADLINE, async || {
        bulk.iter().enumerate().skip(1).all(|(index, _)| {
            let relative = format!("bulk/deep/bulk-{index:02}.md");
            read(&folder.join(&relative)) == read(&bob_folder.join(&relative))
                && read(&bob_folder.join(&relative))
                    .matches(&format!("Offline edit {index}."))
                    .count()
                    == 1
        }) && !bob_folder.join("renamed").exists()
    })
    .await
    .expect("both mapped trees converge without a UI on Alice");
    assert!(alice.mount.is_none());
    assert!(
        folder.join("Plan.md").is_file(),
        "sentinel keeps subtree deletion safe"
    );
    // One archive/restore cycle only: repeat archive by the same author remains accepted debt.
    for id in [&risks, &ideas] {
        bob.ui("documents.document.restore", json!({"documentId": id}))
            .await;
    }
    poll(DEADLINE, async || {
        ["Risks.md", "Ideas.md"].iter().all(|name| {
            folder.join("renamed/deep").join(name).is_file()
                && bob_folder.join("renamed/deep").join(name).is_file()
        })
    })
    .await
    .expect("restore recreates missing parents on both Installations");
    alice.mount(&key).await;
    for (index, id) in bulk.iter().enumerate().skip(1) {
        let relative = format!("bulk/deep/bulk-{index:02}.md");
        assert_eq!(alice.markdown(id).await, bob.markdown(id).await);
        assert_eq!(Some(read(&folder.join(relative))), bob.markdown(id).await);
    }

    for index in 0..3 {
        let title = format!("offline-{index}");
        let id = document_id(&bob.snapshot().await, &title).unwrap();
        let expected = format!("# Offline {index}\n\nCreated offline.\n");
        poll(DEADLINE, async || {
            read(&folder.join(format!("bulk/deep/{title}.md"))) == expected
                && read(&bob_folder.join(format!("bulk/deep/{title}.md"))) == expected
        })
        .await
        .expect("both mappings contain each offline addition exactly once");
        assert_eq!(
            alice.markdown(&id).await.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(bob.markdown(&id).await.as_deref(), Some(expected.as_str()));
    }
    let moved_body = "# Bulk 0\n\nParagraph one of bulk 0.\n";
    assert_eq!(alice.markdown(&bulk[0]).await.as_deref(), Some(moved_body));
    assert_eq!(bob.markdown(&bulk[0]).await.as_deref(), Some(moved_body));
    poll(DEADLINE, async || {
        read(&folder.join("offline/moved/renamed-00.md")) == moved_body
            && read(&bob_folder.join("offline/moved/renamed-00.md")) == moved_body
    })
    .await
    .expect("offline move retains exact body and both mapped files");

    // (5) A lossy file and an oversize file are refused and keep their bytes.
    let lossy = "# Lossy\n\nSee the guide.\n\n[unused]: https://example.com/lossy-target\n";
    let huge = format!("# Huge\n\n{}\n", unique_text(200 * 1024));
    fs::write(folder.join("lossy.md"), lossy).unwrap();
    fs::write(folder.join("huge.md"), &huge).unwrap();
    // A file written after them marks a reconcile that has seen all three.
    fs::write(folder.join("after.md"), "# After\n\nWritten last.\n").unwrap();
    assert!(
        poll(DEADLINE, async || document_id(
            &bob.snapshot().await,
            "after"
        )
        .is_some())
        .await
        .is_some(),
        "the file after the refused ones was not imported"
    );
    // negative-wait: no Document may appear for the refused files.
    tokio::time::sleep(Duration::from_millis(500)).await;
    for title in ["lossy", "huge"] {
        assert!(
            document_id(&alice.snapshot().await, title).is_none(),
            "{title} was imported"
        );
        assert!(
            document_id(&bob.snapshot().await, title).is_none(),
            "{title} reached Bob"
        );
    }
    assert_eq!(read(&folder.join("lossy.md")), lossy);
    assert_eq!(read(&folder.join("huge.md")), huge);

    // (6) Bob archives a Document: Alice's file goes. Bob restores it: the file returns.
    bob.ui("documents.document.archive", json!({ "documentId": plan }))
        .await;
    assert!(
        poll(DEADLINE, async || !plan_path.exists()).await.is_some(),
        "the archived Document's file was not removed"
    );
    bob.ui("documents.document.restore", json!({ "documentId": plan }))
        .await;
    assert!(
        poll(DEADLINE, async || plan_path.is_file()).await.is_some(),
        "the restored Document's file did not return"
    );
    assert!(
        poll(DEADLINE, async || Some(read(&plan_path))
            == bob.markdown(&plan).await)
        .await
        .is_some(),
        "the restored file is not the Document's Canonical Markdown"
    );

    // A separate mapping loses its only populated subtree: unavailable, no archives.
    let only_root = mapped.path().join("only-subtree");
    fs::create_dir_all(only_root.join("only/deep")).unwrap();
    fs::write(
        only_root.join("only/deep/Only.md"),
        "Only populated subtree.\n",
    )
    .unwrap();
    let only_collection = alice
        .ui("documents.collection.create", json!({"name": "Only"}))
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    alice
        .ui(
            "documents.mapping.set",
            json!({"collectionId": only_collection, "path": only_root}),
        )
        .await;
    poll(DEADLINE, async || {
        document_id(&bob.snapshot().await, "Only").is_some()
    })
    .await
    .expect("only-subtree Document arrives");
    let only = document_id(&bob.snapshot().await, "Only").unwrap();
    fs::remove_dir_all(only_root.join("only")).unwrap();
    poll(DEADLINE, async || {
        let listing = alice.ui("documents.mapping.list", json!({})).await;
        listing["collections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["collectionId"] == only_collection
                    && entry["mapping"]["state"] == "unavailable"
            })
    })
    .await
    .expect("zero eligible files marks the mapping unavailable");
    assert!(!is_archived(&alice.snapshot().await, &only));
    assert!(!is_archived(&bob.snapshot().await, &only));
    assert!(only_root.is_dir());
    poll(DEADLINE, async || {
        record_count(&alice.core, workspace) == record_count(&bob.core, workspace)
    })
    .await
    .expect("final record counts converge");
    let settled = record_count(&bob.core, workspace);
    // negative-wait: offline recovery and restored writes must settle without echo records.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(record_count(&alice.core, workspace), settled);
    assert_eq!(record_count(&bob.core, workspace), settled);

    alice.stop().await;
    bob.stop().await;
}
