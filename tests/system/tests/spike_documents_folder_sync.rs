//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! Two real Installations (alice, bob) on loopback with the real Documents Backend. Alice's
//! Documents Backend maps Collections to folders under a `PROTOTYPE-wipe-me-*` temp dir
//! (`folder_sync_spike.rs`). Prints measurements; asserts only what the scenario needs to
//! proceed.
//!
//! Run: cargo test -p teamarness-system-tests --test spike_documents_folder_sync -- --nocapture
#![cfg(unix)]
#[allow(dead_code)]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use support::harness::{
    self, Addressing, PackageStage, create_invitation, introduce, open_core, open_host,
    open_mount, shutdown,
};
use teamarness_core::CoreRuntime;
use teamarness_documents_package::markdown::{export, merge};
use teamarness_package_host::{
    CloseUiSubscription, OpenUiSubscription, PackageHost, PackageHostCommand,
    PackageHostNotification, PackageHostOutcome, PackageHostSubscriptionSelector, PackageIdentity,
    PackageInstanceKey, PackageValue, RecoverPackage, SuspendPackage, UiMountId, UiRequest,
    UiRequestId, UiSubscriptionId, WorkspaceId,
};

const DOCUMENTS: &str = "dev.teamarness.documents";
const DEADLINE: Duration = Duration::from_secs(60);
static NEXT: AtomicU64 = AtomicU64::new(1);

const PLAN: &str = "# Plan\n\nFirst paragraph about scope.\n\nSecond paragraph about risk.\n\nThird paragraph about delivery.\n\n- one\n- two\n";

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

fn stats(label: &str, samples: &[f64]) {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = s[s.len() / 2];
    println!(
        "  {label}: n={} min={:.0}ms median={:.0}ms max={:.0}ms all={:?}",
        s.len(),
        s[0],
        median,
        s[s.len() - 1],
        samples.iter().map(|x| x.round() as i64).collect::<Vec<_>>()
    );
}

struct Inst {
    core: Arc<CoreRuntime>,
    host: PackageHost,
    mount: Option<UiMountId>,
    private: PathBuf,
    core_path: PathBuf,
}

impl Inst {
    async fn open(app: &Path, core_path: PathBuf, private: PathBuf, key: &PackageInstanceKey, mount: bool) -> Self {
        let core = open_core(Addressing::Loopback, &core_path).await;
        Self::with_core(app, core, core_path, private, key, mount).await
    }

    async fn with_core(app: &Path, core: Arc<CoreRuntime>, core_path: PathBuf, private: PathBuf, key: &PackageInstanceKey, mount: bool) -> Self {
        let host = open_host(app, private.clone(), core.clone()).await;
        let mount = if mount { Some(open_mount(&host, key).await) } else { None };
        Self { core, host, mount, private, core_path }
    }

    async fn stop(self) {
        shutdown(self.host).await;
        self.core.shutdown().await.unwrap();
    }

    fn mount(&self) -> UiMountId {
        self.mount.clone().expect("mounted")
    }

    async fn try_ui(&self, operation: &str, payload: Value) -> Result<Value, String> {
        let id = format!("spike-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        match self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.mount(),
                request: UiRequestId::new(id).unwrap(),
                operation: operation.into(),
                payload: serde_json::from_value::<PackageValue>(payload).unwrap(),
            }))
            .await
        {
            Ok(PackageHostOutcome::UiResponse { payload }) => Ok(serde_json::to_value(payload).unwrap()),
            Ok(other) => Err(format!("{other:?}")),
            Err(error) => Err(format!("{error:?}")),
        }
    }

    async fn ui(&self, operation: &str, payload: Value) -> Value {
        self.try_ui(operation, payload.clone())
            .await
            .unwrap_or_else(|e| panic!("{operation} {payload}: {e}"))
    }

    async fn markdown(&self, document: &str) -> Option<String> {
        let first = self
            .try_ui("documents.document.markdown", json!({"documentId":document,"chunkIndex":0}))
            .await
            .ok()?;
        let mut text = first["chunk"].as_str()?.to_owned();
        for index in 1..first["chunks"].as_u64()? {
            let next = self
                .try_ui("documents.document.markdown", json!({"documentId":document,"chunkIndex":index}))
                .await
                .ok()?;
            text.push_str(next["chunk"].as_str()?);
        }
        Some(text)
    }

    async fn snapshot(&self) -> Value {
        self.ui("documents.snapshot", json!({})).await
    }

    /// Opens an editor session (the UI's path) and returns its doc and peer.
    async fn session(&self, document: &str) -> (loro::LoroDoc, u64, UiSubscriptionId) {
        let subscription = UiSubscriptionId::new(format!("spike-session-{}", NEXT.fetch_add(1, Ordering::Relaxed))).unwrap();
        self.host
            .execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
                mount: self.mount(),
                subscription: subscription.clone(),
                operation: "documents.document.session".into(),
                payload: serde_json::from_value(json!({ "documentId": document })).unwrap(),
            }))
            .await
            .unwrap();
        let mut events = self
            .host
            .subscribe(PackageHostSubscriptionSelector::UiEvents { mount: self.mount(), subscription: subscription.clone() })
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
        .expect("session opens");
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
            .execute(PackageHostCommand::CloseUiSubscription(CloseUiSubscription { mount: self.mount(), subscription }))
            .await
            .unwrap();
    }

    /// A Person editing in the Documents UI: one session update made by merging the revised
    /// Markdown as the session peer, sent through `documents.document.update`.
    async fn revise_in_ui(&self, document: &str, f: impl Fn(&str) -> String) {
        let (doc, peer, subscription) = self.session(document).await;
        let current = export(&doc).unwrap();
        let merged = merge(&doc, &doc.oplog_frontiers(), &f(&current), peer).unwrap();
        let request_id = format!("{:032x}", (now_ms() << 20) | u128::from(NEXT.fetch_add(1, Ordering::Relaxed)));
        let answer = self
            .ui(
                "documents.document.update",
                json!({"documentId":document,"sessionPeer":peer.to_string(),"requestId":request_id,
                       "update":STANDARD.encode(&merged.update)}),
            )
            .await;
        assert_eq!(answer["requestId"], request_id);
        self.close(subscription).await;
    }

    /// Distinct writer peers and Loro changes in the Document's body as this Installation holds
    /// it. Every revise (file import) and every editor session writes from a fresh peer, so the
    /// peer count counts body records written.
    async fn body_counts(&self, document: &str) -> (usize, usize) {
        let (doc, _, subscription) = self.session(document).await;
        self.close(subscription).await;
        (doc.oplog_vv().len(), doc.len_changes())
    }
}

fn find_file(root: &Path, filename: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.file_name().is_some_and(|name| name == filename) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file(&path, filename)
        {
            return Some(found);
        }
    }
    None
}

fn unique_text(bytes: usize) -> String {
    let mut text = String::new();
    let mut index = 0u64;
    while text.len() < bytes {
        text.push_str(&format!("{:016x} ", index.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
        index += 1;
    }
    text.truncate(bytes);
    text.trim_end().to_owned()
}

fn doc_id(snapshot: &Value, title: &str) -> Option<String> {
    snapshot["documents"].as_array()?.iter().find(|d| d["title"] == title).map(|d| d["id"].as_str().unwrap().to_owned())
}

fn live_titles(snapshot: &Value) -> Vec<String> {
    snapshot["documents"].as_array().unwrap().iter().filter(|d| d["archived"] == false).map(|d| d["title"].as_str().unwrap().to_owned()).collect()
}

fn read_log(data: &Path) -> Vec<String> {
    fs::read_to_string(data.join("PROTOTYPE-folder-sync.log")).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn log_time(line: &str) -> u128 {
    line.split(' ').next().unwrap().parse().unwrap()
}

/// Writes `text` the way an editor does: in place (`atomic == false`) or temp + rename.
fn editor_write(path: &Path, text: &str, atomic: bool) {
    if atomic {
        let tmp = path.with_file_name(format!(".{}.swp", path.file_name().unwrap().to_string_lossy()));
        fs::write(&tmp, text).unwrap();
        fs::rename(tmp, path).unwrap();
    } else {
        fs::write(path, text).unwrap();
    }
}

async fn poll(timeout: Duration, period: Duration, mut f: impl AsyncFnMut() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f().await {
            return Some(start.elapsed());
        }
        tokio::time::sleep(period).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn spike_documents_folder_sync() {
    let application = harness::StagedApplication::with(&PackageStage::documents());
    let app = application.application_path.clone();
    let root = application.root.path().to_owned();
    let map_root = tempfile::Builder::new().prefix("PROTOTYPE-wipe-me-").tempdir().unwrap();
    let map_root_path = map_root.path().canonicalize().unwrap();
    println!("\nmapped root: {}", map_root_path.display());

    let alice_core = open_core(Addressing::Loopback, &root.join("alice-core")).await;
    let workspace = alice_core.create_workspace("Folder sync spike").unwrap().id();
    let key = PackageInstanceKey::new(PackageIdentity::from_str(DOCUMENTS).unwrap(), WorkspaceId::new(workspace.to_string()).unwrap());
    let alice = Inst::with_core(&app, alice_core, root.join("alice-core"), root.join("alice-private"), &key, true).await;

    // An existing Collection with Documents, made in the app.
    let collection = alice.ui("documents.collection.create", json!({"name":"Notes"})).await["id"].as_str().unwrap().to_owned();
    let mut ids = Vec::new();
    for (title, body) in [("Plan", PLAN.to_owned()), ("Risks", "# Risks\n\n- Late *delivery*\n- Scope creep\n".to_owned()), ("Q&A: what/why?", "## Q\n\nWhy `folders`?\n\n> Because people use editors.\n".to_owned())] {
        let id = alice.ui("documents.document.create", json!({"collectionId":collection,"title":title})).await["id"].as_str().unwrap().to_owned();
        alice.revise_in_ui(&id, |_| body.clone()).await;
        ids.push(id);
    }
    let plan = ids[0].clone();

    let invitation = create_invitation(&alice.core, workspace).await;
    let bob_core = open_core(Addressing::Loopback, &root.join("bob-core")).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob_core]);
    bob_core.join_workspace(&invitation).await.unwrap();
    let bob = Inst::with_core(&app, bob_core, root.join("bob-core"), root.join("bob-private"), &key, true).await;
    let alice_plan = alice.markdown(&plan).await.unwrap();
    assert!(poll(DEADLINE, Duration::from_millis(100), async || bob.markdown(&plan).await.as_deref() == Some(alice_plan.as_str())).await.is_some(), "bob converges");

    // ------------------------------------------------------------ 1. map an existing Collection
    println!("\n== 1. Map an existing Collection ==");
    let data = find_file(&alice.private, "documents.sqlite3").unwrap().parent().unwrap().to_owned();
    println!("  alice Documents Package Data Directory: {}", data.display());
    let mapped_at = Instant::now();
    fs::write(data.join("PROTOTYPE-folder-map.json"), serde_json::to_vec(&json!({"root": map_root_path})).unwrap()).unwrap();
    let folder = map_root_path.join("Notes");
    let files = ["Plan.md", "Risks.md", "Q&A- what-why-.md"];
    let took = poll(DEADLINE, Duration::from_millis(20), async || files.iter().all(|f| folder.join(f).exists())).await;
    println!("  files written after config drop: {:?} (incl. up to 1s config poll)", took);
    for (file, id) in files.iter().zip(&ids) {
        let on_disk = fs::read_to_string(folder.join(file)).unwrap();
        let canonical = alice.markdown(id).await.unwrap();
        println!("  {file:22} == Canonical Markdown: {}", on_disk == canonical);
    }
    let _ = mapped_at;

    let peers_before = bob.body_counts(&plan).await;
    let alice_peers_before = alice.body_counts(&plan).await;

    // ------------------------------------------------------------ 2. external edit -> bob
    println!("\n== 2. External file edit on alice -> bob's Document ==");
    let plan_path = folder.join("Plan.md");
    let mut latencies = Vec::new();
    let mut to_event = Vec::new();
    let mut to_import = Vec::new();
    for run in 0..6 {
        let marker = format!("F{run}x");
        let text = fs::read_to_string(&plan_path).unwrap().replacen("about risk", &format!("about risk {marker}"), 1);
        let wrote_at = now_ms();
        let t0 = Instant::now();
        editor_write(&plan_path, &text, run % 2 == 1);
        let took = poll(DEADLINE, Duration::from_millis(5), async || bob.markdown(&plan).await.is_some_and(|m| m.contains(&marker))).await.expect("reached bob");
        let _ = t0;
        latencies.push(took.as_secs_f64() * 1e3);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let log = read_log(&data);
        if let Some(event) = log.iter().find(|l| l.ends_with("fs-event") && log_time(l) >= wrote_at) {
            to_event.push((log_time(event) - wrote_at) as f64);
        }
        if let Some(import) = log.iter().find(|l| l.contains(" import ") && log_time(l) >= wrote_at) {
            to_import.push((log_time(import) - wrote_at) as f64);
        }
    }
    stats("file write -> bob markdown has edit (poll 5ms)", &latencies);
    stats("file write -> alice watcher event (FSEvents)", &to_event);
    stats("file write -> alice import appended (incl 75ms debounce)", &to_import);
    println!("  (runs alternate in-place write and temp+rename atomic save)");

    // ------------------------------------------------------------ 3. bob edit -> alice file
    println!("\n== 3. bob edit (editor session update) -> alice's file ==");
    let mut latencies = Vec::new();
    for run in 0..6 {
        let marker = format!("B{run}y");
        let t0 = Instant::now();
        bob.revise_in_ui(&plan, |m| m.replacen("about delivery", &format!("about delivery {marker}"), 1)).await;
        let submitted = t0.elapsed();
        let took = poll(DEADLINE, Duration::from_millis(5), async || fs::read_to_string(&plan_path).is_ok_and(|t| t.contains(&marker))).await.expect("reached alice file");
        latencies.push((submitted + took).as_secs_f64() * 1e3);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    stats("bob update request start -> alice file has edit (poll 5ms)", &latencies);

    // ------------------------------------------------------------ 4. echo
    println!("\n== 4. Echo suppression ==");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let peers_after = bob.body_counts(&plan).await;
    let alice_peers_after = alice.body_counts(&plan).await;
    println!("  bob's Plan body (peers, changes) before 2+3: {peers_before:?}, after: {peers_after:?}; alice's: {alice_peers_before:?} -> {alice_peers_after:?}");
    println!("  new writer peers = {} (expected 12: 6 file imports + 6 bob sessions; more = echo records)", peers_after.0 - peers_before.0);
    let log = read_log(&data);
    let imports = log.iter().filter(|l| l.contains(" import ") && l.contains(&plan) && l.contains("changed=true")).count();
    let noop = log.iter().filter(|l| l.contains(" import ") && l.contains(&plan) && l.contains("changed=false")).count();
    let writes = log.iter().filter(|l| l.contains("write-back ") && l.contains(&plan)).count();
    println!("  alice log for Plan: imports appended {imports}, imports no-op {noop}, write-backs {writes}");
    let file_now = fs::read_to_string(&plan_path).unwrap();
    println!("  alice file == bob canonical: {}", Some(file_now.clone()) == bob.markdown(&plan).await);

    // ------------------------------------------------------------ 5. concurrent
    println!("\n== 5. Concurrent edits to different paragraphs ==");
    let text = fs::read_to_string(&plan_path).unwrap().replacen("about scope", "about scope CFILE", 1);
    let t0 = Instant::now();
    let (_, ()) = tokio::join!(
        async { editor_write(&plan_path, &text, true) },
        bob.revise_in_ui(&plan, |m| m.replacen("- two", "- two CBOB", 1))
    );
    let converged = poll(DEADLINE, Duration::from_millis(10), async || {
        let file = fs::read_to_string(&plan_path).unwrap_or_default();
        let b = bob.markdown(&plan).await.unwrap_or_default();
        let a = alice.markdown(&plan).await.unwrap_or_default();
        file.contains("CFILE") && file.contains("CBOB") && b == a && b.contains("CFILE") && b.contains("CBOB")
    })
    .await;
    let file = fs::read_to_string(&plan_path).unwrap();
    let b = bob.markdown(&plan).await.unwrap();
    println!("  converged (file, alice, bob all hold both): {:?} after {:?}", converged.is_some(), converged.or(Some(t0.elapsed())));
    println!("  counts in bob: CFILE={} CBOB={}; file: CFILE={} CBOB={}; file==bob canonical: {}", b.matches("CFILE").count(), b.matches("CBOB").count(), file.matches("CFILE").count(), file.matches("CBOB").count(), file == b);
    let refused = read_log(&data).iter().filter(|l| l.contains("write-back-refused")).count();
    println!("  alice write-back refusals so far (race refuse-then-import): {refused}");

    // ------------------------------------------------------------ 6. offline bulk
    println!("\n== 6. Offline bulk edit ==");
    let t0 = Instant::now();
    for i in 0..50 {
        fs::write(folder.join(format!("bulk-{i:02}.md")), format!("# Bulk {i}\n\nParagraph one of bulk {i}.\n\nParagraph two.\n")).unwrap();
    }
    let online_create = poll(Duration::from_secs(120), Duration::from_millis(100), async || {
        let snap = bob.snapshot().await;
        let Some(id) = doc_id(&snap, "bulk-49") else { return false };
        live_titles(&snap).iter().filter(|t| t.starts_with("bulk-")).count() == 50 && bob.markdown(&id).await.is_some_and(|m| m.contains("bulk 49"))
    })
    .await;
    println!("  (online) 50 new files -> 50 Documents with bodies on bob: {:?}", online_create.or(Some(t0.elapsed())));
    let snap = bob.snapshot().await;
    let bulk_ids: Vec<String> = (0..50).map(|i| doc_id(&snap, &format!("bulk-{i:02}")).unwrap()).collect();
    let risks_id = ids[1].clone();
    let qa_id = ids[2].clone();
    let lines_before = read_log(&data).len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    // App closed: the whole Installation stops.
    alice.stop().await;
    for i in 0..50 {
        let path = folder.join(format!("bulk-{i:02}.md"));
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{text}\nOffline edit {i}.\n")).unwrap();
    }
    for k in 0..3 {
        fs::write(folder.join(format!("offline-new-{k}.md")), format!("# New {k}\n\nCreated offline.\n")).unwrap();
    }
    fs::remove_file(folder.join("Risks.md")).unwrap();
    fs::remove_file(folder.join("Q&A- what-why-.md")).unwrap();
    let t0 = Instant::now();
    // Reopen WITHOUT a UI mount: does the Backend sync with no Documents UI open?
    let alice = Inst::open(&app, root.join("alice-core"), root.join("alice-private"), &key, false).await;
    introduce(Addressing::Loopback, &[&alice.core, &bob.core]);
    let opened = t0.elapsed();
    let converged = poll(Duration::from_secs(180), Duration::from_millis(200), async || {
        let snap = bob.snapshot().await;
        let archived = |id: &str| snap["documents"].as_array().unwrap().iter().any(|d| d["id"] == id && d["archived"] == true);
        if !(archived(&risks_id) && archived(&qa_id)) {
            return false;
        }
        let titles = live_titles(&snap);
        if !(0..3).all(|k| titles.contains(&format!("offline-new-{k}"))) {
            return false;
        }
        for (k, id) in (0..3).map(|k| (k, doc_id(&snap, &format!("offline-new-{k}")).unwrap())) {
            if !bob.markdown(&id).await.is_some_and(|m| m.contains("Created offline")) {
                let _ = k;
                return false;
            }
        }
        for (i, id) in bulk_ids.iter().enumerate() {
            if !bob.markdown(id).await.is_some_and(|m| m.contains(&format!("Offline edit {i}."))) {
                return false;
            }
        }
        true
    })
    .await;
    println!("  alice Installation reopened in {opened:?} (no UI mount)");
    println!("  bob converged (50 edits, 3 new, 2 archived): {:?} after reopen began", converged);
    let log = read_log(&data);
    let after: Vec<&String> = log[lines_before..].iter().collect();
    let count = |needle: &str| after.iter().filter(|l| l.contains(needle)).count();
    println!(
        "  alice appended after restart: {} edit records (imports changed=true), {} creates, {} archives, {} renames; no-op imports {}; retries {}",
        after.iter().filter(|l| l.contains(" import ") && l.contains("changed=true")).count(),
        count("append create"),
        count("append archive"),
        count("append rename"),
        after.iter().filter(|l| l.contains(" import ") && l.contains("changed=false")).count(),
        count("import-retry")
    );
    if let (Some(start), Some(q)) = (after.iter().find(|l| l.ends_with(" start")), after.iter().find(|l| l.contains("qualified=true"))) {
        println!("  backend task start -> qualified: {}ms", log_time(q) - log_time(start));
    }
    for line in after.iter().filter(|l| l.contains("reconcile appended")) {
        println!("  {line}");
    }

    // ------------------------------------------------------------ 7. oversize
    println!("\n== 7. A file beyond the bounds ==");
    let alice = {
        // Re-mount for markdown reads.
        let mount = open_mount(&alice.host, &key).await;
        Inst { mount: Some(mount), ..alice }
    };
    let huge = unique_text(200 * 1024);
    let lines_before = read_log(&data).len();
    fs::write(folder.join("huge.md"), format!("# Huge\n\n{huge}\n")).unwrap();
    let plan_text = fs::read_to_string(&plan_path).unwrap();
    let plan_huge = format!("{plan_text}\n{huge}\n");
    fs::write(&plan_path, &plan_huge).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let log = read_log(&data);
    for line in &log[lines_before..] {
        if !line.ends_with("fs-event") {
            println!("  log: {}", if line.len() > 220 { &line[..220] } else { line });
        }
    }
    let plan_bob = bob.markdown(&plan).await.unwrap();
    let plan_alice = alice.markdown(&plan).await.unwrap();
    println!("  Plan on bob contains huge text: {}; on alice: {}; alice file still holds the user's huge edit: {}", plan_bob.contains(&huge[..64]), plan_alice.contains(&huge[..64]), fs::read_to_string(&plan_path).unwrap() == plan_huge);
    let snap = bob.snapshot().await;
    if let Some(id) = doc_id(&snap, "huge") {
        println!("  'huge' Document on bob: exists; body = {:?}", bob.markdown(&id).await.map(|m| m.len()));
    } else {
        println!("  'huge' Document on bob: absent");
    }
    // Recovery: the user trims the file; it imports again.
    fs::write(&plan_path, plan_text.replacen("about scope", "about scope TRIMMED", 1)).unwrap();
    let recovered = poll(DEADLINE, Duration::from_millis(20), async || bob.markdown(&plan).await.is_some_and(|m| m.contains("TRIMMED"))).await;
    println!("  after trimming the file, the edit reaches bob: {recovered:?}");

    // ------------------------------------------------------------ 8. lifecycle
    println!("\n== 8. Suspend / recover ==");
    let lines_before = read_log(&data).len();
    let suspended = tokio::time::timeout(Duration::from_secs(20), alice.host.execute(PackageHostCommand::Suspend(SuspendPackage { instance: key.clone() }))).await;
    println!("  suspend: {:?}", suspended.as_ref().map(|r| r.as_ref().map(|_| "ok").map_err(|e| format!("{e:?}"))));
    let text = fs::read_to_string(&plan_path).unwrap().replacen("about scope", "about scope SUSP", 1);
    fs::write(&plan_path, &text).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("  edit while suspended reached bob: {}", bob.markdown(&plan).await.is_some_and(|m| m.contains("SUSP")));
    let recovered = tokio::time::timeout(Duration::from_secs(20), alice.host.execute(PackageHostCommand::Recover(RecoverPackage { instance: key.clone() }))).await;
    println!("  recover: {:?}", recovered.as_ref().map(|r| r.as_ref().map(|_| "ok").map_err(|e| format!("{e:?}"))));
    let after_recover = poll(Duration::from_secs(30), Duration::from_millis(20), async || bob.markdown(&plan).await.is_some_and(|m| m.contains("SUSP"))).await;
    println!("  edit made while suspended reached bob after recover: {after_recover:?}");
    for line in &read_log(&data)[lines_before..] {
        if !line.ends_with("fs-event") {
            println!("  log: {}", if line.len() > 200 { &line[..200] } else { line });
        }
    }

    if let Some(out) = std::env::var_os("SPIKE_LOG_OUT") {
        fs::copy(data.join("PROTOTYPE-folder-sync.log"), &out).unwrap();
        println!("\n  alice log copied to {}", PathBuf::from(out).display());
    }
    let _ = (alice.core_path.clone(), bob.core_path.clone());
    alice.stop().await;
    bob.stop().await;
    if std::env::var_os("SPIKE_KEEP").is_some() {
        let kept = map_root.keep();
        println!("  kept mapped root: {}", kept.display());
    }
}
