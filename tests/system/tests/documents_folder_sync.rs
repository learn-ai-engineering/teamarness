//! Two Installations, one with a Collection mapped to a folder: files and Documents follow
//! each other both ways.
//!
//! Hermetic like the Backlog two-Installation flow: real `CoreRuntime`s on loopback, real
//! Package Hosts, real SQLite and the real Documents Backend process, with its real file
//! watcher. Alice maps a Collection to a folder through `documents.mapping.set`. The scenario:
//! (1) the export writes each Document as its Canonical Markdown; (2) a file edit reaches Bob
//! while Bob edits elsewhere in the Document, and both edits survive; (3) Bob's edit reaches
//! Alice's file, with no echo record; (4) edits, new files, deletions and a rename made while
//! Alice is stopped import on restart, with no duplication; (5) a lossy file and an oversize
//! file are refused and left byte-identical; (6) Bob's archive removes Alice's file and his
//! restore brings it back.
//!
//! Every wait polls with a bounded timeout, so file watcher timing (FSEvents on macOS, inotify
//! on Linux CI) is never assumed.
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
use teamarness_documents_package::markdown::{export, merge};
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
    mount: UiMountId,
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
        Self { core, host, mount }
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
                mount: self.mount.clone(),
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
                mount: self.mount.clone(),
                subscription: subscription.clone(),
                operation: "documents.document.session".into(),
                payload: serde_json::from_value(json!({ "documentId": document })).unwrap(),
            }))
            .await
            .unwrap();
        let mut events = self
            .host
            .subscribe(PackageHostSubscriptionSelector::UiEvents {
                mount: self.mount.clone(),
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
                    mount: self.mount.clone(),
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

/// Verifies: slice B B-4, B-5, B-6, B-7, B-8, B-9, B-10 and B-11 end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_mapped_folder_and_its_collection_follow_each_other() {
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

    // (2) A file edit reaches Bob, and Bob's concurrent edit elsewhere survives.
    let plan_path = folder.join("Plan.md");
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

    // (4) Offline: 50 edits, 3 new files, 2 deletions and 1 rename while Alice is stopped.
    for index in 0..51 {
        fs::write(
            folder.join(format!("bulk-{index:02}.md")),
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
        let path = folder.join(format!("bulk-{index:02}.md"));
        let text = read(&path);
        fs::write(&path, format!("{text}\nOffline edit {index}.\n")).unwrap();
    }
    for index in 0..3 {
        fs::write(
            folder.join(format!("offline-{index}.md")),
            format!("# Offline {index}\n\nCreated offline.\n"),
        )
        .unwrap();
    }
    fs::remove_file(folder.join("Risks.md")).unwrap();
    fs::remove_file(folder.join("Ideas.md")).unwrap();
    fs::rename(folder.join("bulk-00.md"), folder.join("renamed-00.md")).unwrap();
    let restarted = Instant::now();
    let alice = Installation::open(&app, &alice_paths, &key).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);
    poll(Duration::from_secs(180), async || {
        let snapshot = bob.snapshot().await;
        if !(is_archived(&snapshot, &risks) && is_archived(&snapshot, &ideas)) {
            return false;
        }
        if document_id(&snapshot, "renamed-00").as_ref() != Some(&bulk[0]) {
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
    // Plan, 50 bulk files, the renamed one and 3 new files.
    assert_eq!(count, 1 + 50 + 1 + 3, "{titles:?}");
    for (index, id) in bulk.iter().enumerate().skip(1) {
        let markdown = bob.markdown(id).await.unwrap();
        assert_eq!(
            markdown.matches(&format!("Offline edit {index}.")).count(),
            1,
            "bulk-{index:02} duplicated its edit: {markdown}"
        );
        assert_eq!(markdown.matches("Paragraph one").count(), 1, "{markdown}");
    }

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

    alice.stop().await;
    bob.stop().await;
}
