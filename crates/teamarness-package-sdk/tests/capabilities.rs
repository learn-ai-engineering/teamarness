//! Typed Semantic Capability clients, and the record log session that credits only after ingest.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::capabilities::record_log::{
    Availability, Cursor, OpenResult, RecordEvent, RecordLog, RecordLogSink, SessionOptions,
    SyncState, SyncWarning, run_session,
};
use teamarness_package_sdk::capabilities::{membership, record_log, signing, workspace_context};
use teamarness_package_sdk::testing::{Harness, HarnessBuilder, ScriptedCapability};
use teamarness_package_sdk::{BackendError, CancellationToken, Empty, Router};

const RECORD_LOG_SCHEMA: &str =
    "../../contracts/package-capabilities/v1/immutable-record-log.schema.json";

fn empty() -> JsonValue {
    JsonValue::Object(serde_json::Map::new())
}

fn schema() -> JsonValue {
    serde_json::from_str(&std::fs::read_to_string(RECORD_LOG_SCHEMA).unwrap()).unwrap()
}

fn strings(value: &JsonValue) -> BTreeSet<String> {
    value
        .as_array()
        .expect("a JSON array")
        .iter()
        .map(|entry| entry.as_str().expect("a string").to_owned())
        .collect()
}

#[test]
fn open_status_strings_match_the_capability_schema() {
    let schema = schema();
    let declared = &schema["$defs"]["open"]["properties"]["results"]["items"]["oneOf"][1]["properties"]
        ["status"]["enum"];
    let mut expected = strings(declared);
    // The `ready` branch names its status through a `const`, not the shared enum.
    expected.insert(
        schema["$defs"]["ready"]["properties"]["status"]["const"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let actual: BTreeSet<String> = OpenResult::statuses()
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn availability_strings_match_the_capability_schema() {
    let schema = schema();
    let expected = strings(&schema["$defs"]["stateEvent"]["properties"]["availability"]["enum"]);
    let actual: BTreeSet<String> = Availability::all()
        .into_iter()
        .map(|value| value.as_str().to_owned())
        .collect();
    assert_eq!(actual, expected);
}

fn hex(length: usize, seed: u64) -> String {
    format!("{:0width$x}", seed, width = length)
}

fn record_frame(seed: u64) -> JsonValue {
    serde_json::json!({
        "kind": "record",
        "cursor": hex(50, seed),
        "recordId": hex(64, seed),
        "orderKey": hex(82, seed),
        "author": hex(64, 9),
        "purpose": "fixture/v1",
        "signature": "c2lnbmF0dXJl",
        "canonical": "Y2Fub25pY2Fs",
        "sealedRecord": "c2VhbGVk",
    })
}

// ---------------------------------------------------------------------------
// Call clients
// ---------------------------------------------------------------------------

/// A router that exposes one capability call per request, so the harness can drive it.
fn client_router() -> Router<()> {
    Router::stateless()
        .request("fixture.append", |context, _: Empty| async move {
            let log = RecordLog::from_initialization(
                context.initialization(),
                context.capabilities().clone(),
            )?;
            let appended = log.append("fixture/v1", b"canonical").await?;
            Ok(serde_json::json!({
                "recordId": appended.record_id.as_str(),
                "orderKey": appended.order_key.as_str(),
                "author": appended.author,
                "sealedRecord": String::from_utf8_lossy(&appended.sealed_record),
            }))
        })
        .request("fixture.open_batch", |context, request: OpenBatch| async move {
            let log = RecordLog::from_initialization(
                context.initialization(),
                context.capabilities().clone(),
            )?;
            let inputs = vec![vec![7_u8; request.each]; request.count];
            let results = log.open(&inputs).await?;
            Ok(serde_json::json!({
                "statuses": results.iter().map(OpenResult::as_str).collect::<Vec<_>>(),
            }))
        })
        .request("fixture.schema_shapes", |context, _: Empty| async move {
            let workspace = workspace_context::WorkspaceContext::from_initialization(
                context.initialization(),
                context.capabilities().clone(),
            )?
            .get()
            .await?;
            let membership = membership::Membership::from_initialization(
                context.initialization(),
                context.capabilities().clone(),
            )?;
            let roster = membership.current().await?;
            let history = membership.was_member(&hex(64, 9), -12).await?;
            let signing = signing::Signing::from_initialization(
                context.initialization(),
                context.capabilities().clone(),
            )?;
            let signature = signing.sign("fixture/v1", b"payload").await?;
            let valid = signing
                .verify(&hex(64, 9), "fixture/v1", b"payload", &"s".repeat(86))
                .await?;
            Ok(serde_json::json!({
                "workspaceName": workspace.workspace_name,
                "installationDisplayName": workspace.installation_display_name,
                "epoch": roster.epoch,
                "members": roster.members.iter().map(|member| member.role.as_str()).collect::<Vec<_>>(),
                "wasMember": history.was_member,
                "warning": history.warning,
                "signer": signature.signer,
                "envelopeVersion": signature.envelope_version,
                "valid": valid,
            }))
        })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenBatch {
    count: usize,
    each: usize,
}

async fn start(
    router: Router<()>,
    capabilities: Vec<(&str, ScriptedCapability)>,
    directory: &tempfile::TempDir,
) -> Harness {
    let mut builder = HarnessBuilder::new(router.into_backend()).data_directory(directory.path());
    for (identity, scripted) in capabilities {
        builder = builder.capability(identity, scripted);
    }
    let mut harness = builder.start().await;
    harness.activate().await.unwrap();
    harness
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_decodes_a_committed_result() {
    let directory = tempfile::tempdir().unwrap();
    let valid = ScriptedCapability::new().calls(|method, _| {
        assert_eq!(method, "append");
        Ok(serde_json::json!({
            "recordId": hex(64, 1),
            "orderKey": hex(82, 1),
            "author": hex(64, 9),
            "signature": "c2ln",
            "sealedRecord": "c2VhbGVk",
            "durability": "committed",
        }))
    });
    let mut harness = start(
        client_router(),
        vec![(record_log::IDENTITY, valid)],
        &directory,
    )
    .await;
    let response = harness.request("fixture.append", empty()).await.unwrap();
    assert_eq!(response.get("recordId").unwrap(), &hex(64, 1));
    assert_eq!(response.get("sealedRecord").unwrap(), "sealed");
    harness.shutdown().await.unwrap();

    let broken = ScriptedCapability::new().calls(|_, _| {
        Ok(serde_json::json!({
            "recordId": "not-a-record-id",
            "orderKey": hex(82, 1),
            "author": hex(64, 9),
            "signature": "c2ln",
            "sealedRecord": "c2VhbGVk",
            "durability": "committed",
        }))
    });
    let mut harness = start(
        client_router(),
        vec![(record_log::IDENTITY, broken)],
        &directory,
    )
    .await;
    let error = harness
        .request("fixture.append", empty())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "package.invalid_capability_response");
    assert!(!error.retryable());
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_rejects_oversized_batches_before_calling() {
    let directory = tempfile::tempdir().unwrap();
    let scripted = ScriptedCapability::new()
        .calls(|_, _| panic!("open must refuse the batch before calling the host"));
    let mut harness = start(
        client_router(),
        vec![(record_log::IDENTITY, scripted.clone())],
        &directory,
    )
    .await;

    for request in [
        serde_json::json!({ "count": 33, "each": 8 }),
        serde_json::json!({ "count": 2, "each": 263_000 }),
    ] {
        let error = harness
            .request("fixture.open_batch", request)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "package.invalid_capability_request");
    }
    assert!(scripted.recorded_calls().is_empty());
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_membership_and_signing_clients_decode_schema_shapes() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = ScriptedCapability::new().calls(|method, _| {
        assert_eq!(method, "get");
        Ok(serde_json::json!({
            "workspaceId": "workspace1",
            "workspaceName": "Studio",
            "installationIdentity": hex(64, 9),
            "installationDisplayName": "Laptop",
        }))
    });
    let roster = ScriptedCapability::new().calls(|method, _| match method {
        "current" => Ok(serde_json::json!({
            "workspaceId": "workspace1",
            "epoch": "7",
            "membershipVersion": hex(64, 3),
            "members": [
                { "identity": hex(64, 9), "role": "manager" },
                { "identity": hex(64, 10), "role": "member" },
            ],
        })),
        "was_member" => Ok(serde_json::json!({
            "wasMember": true,
            "warning": "informational_not_authorization",
        })),
        other => panic!("unexpected membership method {other}"),
    });
    let signer = ScriptedCapability::new().calls(|method, _| match method {
        "sign" => Ok(serde_json::json!({
            "signer": hex(64, 9),
            "envelopeVersion": 1,
            "signature": "c2ln",
        })),
        "verify" => Ok(serde_json::json!({ "valid": true })),
        other => panic!("unexpected signing method {other}"),
    });
    let mut harness = start(
        client_router(),
        vec![
            (workspace_context::IDENTITY, workspace),
            (membership::IDENTITY, roster),
            (signing::IDENTITY, signer),
        ],
        &directory,
    )
    .await;

    let response = harness
        .request("fixture.schema_shapes", empty())
        .await
        .unwrap();
    assert_eq!(response.get("workspaceName").unwrap(), "Studio");
    assert_eq!(response.get("installationDisplayName").unwrap(), "Laptop");
    assert_eq!(response.get("epoch").unwrap(), 7);
    assert_eq!(
        response.get("members").unwrap(),
        &serde_json::json!(["manager", "member"])
    );
    assert_eq!(response.get("wasMember").unwrap(), true);
    assert_eq!(
        response.get("warning").unwrap(),
        "informational_not_authorization"
    );
    assert_eq!(response.get("signer").unwrap(), &hex(64, 9));
    assert_eq!(response.get("envelopeVersion").unwrap(), 1);
    assert_eq!(response.get("valid").unwrap(), true);
    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Stream-close retryability (register C-15)
// ---------------------------------------------------------------------------

/// The one table both paths are measured against: a host code means the same thing on a
/// capability call and on a capability stream close.
fn host_code_is_retryable(code: &str) -> bool {
    matches!(
        code,
        "overloaded" | "deadline_exceeded" | "host_unavailable"
    )
}

fn compare_router() -> Router<()> {
    Router::stateless().request(
        "fixture.compare",
        |context, request: CompareRequest| async move {
            let grant = context
                .initialization()
                .capability(record_log::IDENTITY)
                .ok_or_else(|| BackendError::new("fixture.no_grant", false, empty()))?
                .grant
                .clone();
            let call = context
                .capabilities()
                .call(
                    &grant,
                    "fail",
                    serde_json::json!({ "code": request.code.clone() }),
                )
                .await
                .expect_err("the scripted capability refused");
            let mut subscription = context.capabilities().subscribe(&grant, empty()).await?;
            let stream = match subscription.recv().await {
                Some(Err(error)) => error,
                _ => {
                    return Err(BackendError::new(
                        "fixture.unexpected_event",
                        false,
                        empty(),
                    ));
                }
            };
            Ok(serde_json::json!({
                "callCode": call.code(),
                "callRetryable": call.retryable(),
                "streamCode": stream.code(),
                "streamRetryable": stream.retryable(),
            }))
        },
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareRequest {
    code: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capability_stream_close_retryability_matches_call_errors() {
    let directory = tempfile::tempdir().unwrap();
    let scripted = ScriptedCapability::new().calls(|_, payload| {
        let code = payload
            .get("code")
            .and_then(JsonValue::as_str)
            .expect("the fixture names a code");
        Err(BackendError::new(
            code,
            host_code_is_retryable(code),
            empty(),
        ))
    });
    let mut harness = start(
        compare_router(),
        vec![(record_log::IDENTITY, scripted.clone())],
        &directory,
    )
    .await;

    for (index, code) in [
        "overloaded",
        "deadline_exceeded",
        "host_unavailable",
        "capability_unavailable",
        "fixture.rejected",
    ]
    .into_iter()
    .enumerate()
    {
        let pending = harness
            .start_request("fixture.compare", serde_json::json!({ "code": code }))
            .await;
        wait_for_opens(&scripted, index + 1).await;
        scripted.stream().close(code).await;
        let response = pending.result().await.unwrap();
        assert_eq!(response.get("callCode").unwrap(), code);
        assert_eq!(response.get("streamCode").unwrap(), code);
        assert_eq!(
            response.get("callRetryable").unwrap(),
            response.get("streamRetryable").unwrap(),
            "{code} must mean the same on a call and on a stream close"
        );
        assert_eq!(
            response.get("streamRetryable").unwrap(),
            &JsonValue::Bool(host_code_is_retryable(code))
        );
    }
    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// The record log session
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TestSink {
    cursor: Mutex<Option<Cursor>>,
    cursor_reads: AtomicUsize,
    ingested: Mutex<Vec<RecordEvent>>,
    fail_next_ingest: AtomicBool,
    gate: AtomicBool,
    resets: AtomicUsize,
    states: Mutex<Vec<SyncState>>,
    warnings: Mutex<Vec<String>>,
    errors: Mutex<Vec<String>>,
}

impl TestSink {
    fn open() -> Arc<Self> {
        let sink = Arc::new(Self::default());
        sink.gate.store(true, Ordering::SeqCst);
        sink
    }

    fn ingested_ids(&self) -> Vec<String> {
        self.ingested
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.record_id.as_str().to_owned())
            .collect()
    }
}

impl RecordLogSink for TestSink {
    fn committed_cursor(&self) -> Result<Option<Cursor>, BackendError> {
        self.cursor_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.cursor.lock().unwrap().clone())
    }

    fn ingest(&self, record: &RecordEvent) -> Result<(), BackendError> {
        while !self.gate.load(Ordering::SeqCst) {
            // simulated-delay: the fixture's own gate, which the test opens when it is ready.
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.fail_next_ingest.swap(false, Ordering::SeqCst) {
            return Err(BackendError::new("fixture.ingest_failed", true, empty()));
        }
        self.ingested.lock().unwrap().push(record.clone());
        *self.cursor.lock().unwrap() = Some(record.cursor.clone());
        Ok(())
    }

    fn reset_cursor(&self) -> Result<(), BackendError> {
        self.resets.fetch_add(1, Ordering::SeqCst);
        *self.cursor.lock().unwrap() = None;
        Ok(())
    }

    fn on_state(&self, state: &SyncState) {
        self.states.lock().unwrap().push(*state);
    }

    fn on_warning(&self, warning: &SyncWarning) {
        self.warnings.lock().unwrap().push(warning.code.clone());
    }

    fn on_session_error(&self, error: &BackendError) {
        self.errors.lock().unwrap().push(error.code().to_owned());
    }
}

struct SessionState {
    cancellation: CancellationToken,
}

fn session_router(sink: Arc<TestSink>, options: SessionOptions) -> Router<SessionState> {
    Router::new(move |activation| {
        let sink = sink.clone();
        let log = RecordLog::from_initialization(
            activation.initialization(),
            activation.capabilities().clone(),
        );
        async move {
            let cancellation = CancellationToken::new();
            tokio::spawn(run_session(log?, sink, cancellation.clone(), options));
            Ok(SessionState { cancellation })
        }
    })
    .on_dispose(|state: Arc<SessionState>| async move {
        state.cancellation.cancel();
        Ok(())
    })
}

async fn start_session(
    sink: Arc<TestSink>,
    options: SessionOptions,
    scripted: ScriptedCapability,
    directory: &tempfile::TempDir,
) -> Harness {
    let mut harness = HarnessBuilder::new(session_router(sink, options).into_backend())
        .data_directory(directory.path())
        .capability(record_log::IDENTITY, scripted)
        .start()
        .await;
    harness.activate().await.unwrap();
    harness
}

async fn wait_for_opens(scripted: &ScriptedCapability, count: usize) {
    for _ in 0..600 {
        if scripted.recorded_opens().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "the Package opened {} streams, expected {count}",
        scripted.recorded_opens().len()
    );
}

async fn wait_until(mut ready: impl FnMut() -> bool, what: &str) {
    for _ in 0..600 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

fn after_cursor(open: &JsonValue) -> &JsonValue {
    open.get("afterCursor").expect("every subscribe names one")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_redelivers_a_record_whose_ingest_failed() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    sink.fail_next_ingest.store(true, Ordering::SeqCst);
    let scripted = ScriptedCapability::new();
    let mut harness = start_session(
        sink.clone(),
        SessionOptions::default(),
        scripted.clone(),
        &directory,
    )
    .await;

    wait_for_opens(&scripted, 1).await;
    assert!(scripted.stream().send(record_frame(1)).await);

    // The failed ingest closes the stream; the session resubscribes from what was committed,
    // which is nothing, and the same record arrives again.
    wait_for_opens(&scripted, 2).await;
    assert!(scripted.stream().send(record_frame(1)).await);
    assert!(scripted.stream().send(record_frame(2)).await);
    wait_until(
        || sink.ingested.lock().unwrap().len() == 2,
        "both records to be ingested",
    )
    .await;

    assert_eq!(sink.ingested_ids(), vec![hex(64, 1), hex(64, 2)]);
    assert_eq!(sink.cursor_reads.load(Ordering::SeqCst), 2);
    let opens = scripted.recorded_opens();
    assert_eq!(after_cursor(&opens[0]), &JsonValue::Null);
    assert_eq!(after_cursor(&opens[1]), &JsonValue::Null);
    assert_eq!(
        sink.cursor.lock().unwrap().as_ref().map(Cursor::as_str),
        Some(hex(50, 2).as_str())
    );
    assert_eq!(
        *sink.errors.lock().unwrap(),
        vec!["fixture.ingest_failed".to_owned()]
    );
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_credits_only_after_ingest_returns() {
    let directory = tempfile::tempdir().unwrap();
    let sink = Arc::new(TestSink::default());
    let scripted = ScriptedCapability::new();
    let mut harness = start_session(
        sink.clone(),
        SessionOptions::default(),
        scripted.clone(),
        &directory,
    )
    .await;
    wait_for_opens(&scripted, 1).await;

    let sender = scripted.stream();
    let sent = Arc::new(AtomicUsize::new(0));
    let feeding = {
        let sent = sent.clone();
        tokio::spawn(async move {
            for seed in 1..=33_u64 {
                assert!(sender.send(record_frame(seed)).await);
                sent.store(usize::try_from(seed).unwrap(), Ordering::SeqCst);
            }
        })
    };

    // The host's window is 32 events. The sink is holding its first ingest, so no credit has
    // come back and the 33rd event cannot leave.
    wait_until(
        || sent.load(Ordering::SeqCst) == 32,
        "the credit window to fill",
    )
    .await;
    // negative-wait: a bounded window proving the 33rd event cannot leave.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sent.load(Ordering::SeqCst), 32);
    assert!(sink.ingested.lock().unwrap().is_empty());

    sink.gate.store(true, Ordering::SeqCst);
    feeding.await.unwrap();
    wait_until(
        || sink.ingested.lock().unwrap().len() == 33,
        "every record to be ingested",
    )
    .await;
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_reports_state_and_warning_events() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    let scripted = ScriptedCapability::new();
    let mut harness = start_session(
        sink.clone(),
        SessionOptions::default(),
        scripted.clone(),
        &directory,
    )
    .await;
    wait_for_opens(&scripted, 1).await;

    let sender = scripted.stream();
    assert!(
        sender
            .send(serde_json::json!({
                "kind": "state",
                "availability": "catching_up",
                "pendingRecordCount": "12",
            }))
            .await
    );
    assert!(
        sender
            .send(serde_json::json!({ "kind": "warning", "code": "record_quarantined" }))
            .await
    );
    assert!(
        sender
            .send(serde_json::json!({
                "kind": "state",
                "availability": "current",
                "pendingRecordCount": "0",
            }))
            .await
    );
    wait_until(
        || sink.states.lock().unwrap().len() == 2,
        "both state events",
    )
    .await;

    let states = sink.states.lock().unwrap().clone();
    assert_eq!(states[0].availability, Availability::CatchingUp);
    assert_eq!(states[0].pending_record_count, 12);
    assert_eq!(states[1].availability, Availability::Current);
    assert_eq!(
        *sink.warnings.lock().unwrap(),
        vec!["record_quarantined".to_owned()]
    );
    assert!(sink.ingested.lock().unwrap().is_empty());
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_resets_the_cursor_and_resubscribes_once_on_cursor_invalid() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    *sink.cursor.lock().unwrap() = Some(Cursor::parse(hex(50, 1)).unwrap());
    let scripted = ScriptedCapability::new();
    // A backoff far longer than the test: reaching the second subscribe proves none was taken.
    let options = SessionOptions {
        initial_backoff: Duration::from_secs(30),
        max_backoff: Duration::from_secs(30),
    };
    let mut harness = start_session(sink.clone(), options, scripted.clone(), &directory).await;

    wait_for_opens(&scripted, 1).await;
    let closed = Instant::now();
    scripted.stream().close("cursor_invalid").await;
    wait_for_opens(&scripted, 2).await;

    assert!(
        closed.elapsed() < Duration::from_secs(5),
        "the replay must not wait out the backoff"
    );
    assert_eq!(sink.resets.load(Ordering::SeqCst), 1);
    let opens = scripted.recorded_opens();
    assert_eq!(after_cursor(&opens[0]).as_str(), Some(hex(50, 1).as_str()));
    assert_eq!(after_cursor(&opens[1]), &JsonValue::Null);
    assert!(sink.errors.lock().unwrap().is_empty());
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cursor_invalid_from_a_null_cursor_reports_a_session_error_and_backs_off() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    let scripted = ScriptedCapability::new();
    let options = SessionOptions {
        initial_backoff: Duration::from_millis(400),
        max_backoff: Duration::from_millis(400),
    };
    let mut harness = start_session(sink.clone(), options, scripted.clone(), &directory).await;

    wait_for_opens(&scripted, 1).await;
    scripted.stream().close("cursor_invalid").await;
    wait_for_opens(&scripted, 2).await;
    let closed = Instant::now();
    scripted.stream().close("cursor_invalid").await;
    wait_for_opens(&scripted, 3).await;

    assert!(
        closed.elapsed() >= Duration::from_millis(350),
        "a second cursor_invalid from nothing waits out the backoff, took {:?}",
        closed.elapsed()
    );
    assert_eq!(
        *sink.errors.lock().unwrap(),
        vec!["cursor_invalid".to_owned(), "cursor_invalid".to_owned()]
    );
    for open in scripted.recorded_opens() {
        assert_eq!(after_cursor(&open), &JsonValue::Null);
    }
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_backs_off_and_retries_after_a_retryable_capability_error() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    let scripted = ScriptedCapability::new();
    let options = SessionOptions {
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(50),
    };
    let mut harness = start_session(sink.clone(), options, scripted.clone(), &directory).await;

    wait_for_opens(&scripted, 1).await;
    scripted.stream().close("deadline_exceeded").await;
    wait_for_opens(&scripted, 2).await;
    assert_eq!(
        *sink.errors.lock().unwrap(),
        vec!["deadline_exceeded".to_owned()]
    );

    // The retry is a real subscription: a record on it is ingested.
    assert!(scripted.stream().send(record_frame(4)).await);
    wait_until(
        || sink.ingested.lock().unwrap().len() == 1,
        "the record on the retried subscription",
    )
    .await;
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_stops_on_cancellation_during_backoff() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    let scripted = ScriptedCapability::new();
    let options = SessionOptions {
        initial_backoff: Duration::from_secs(30),
        max_backoff: Duration::from_secs(30),
    };
    let mut harness = start_session(sink.clone(), options, scripted.clone(), &directory).await;

    wait_for_opens(&scripted, 1).await;
    scripted.stream().close("host_unavailable").await;
    wait_until(
        || !sink.errors.lock().unwrap().is_empty(),
        "the session to report the failure",
    )
    .await;

    // Dispose cancels the session's token while it is sleeping out a 30 second backoff.
    let disposed = Instant::now();
    harness.dispose().await.unwrap();
    assert!(disposed.elapsed() < Duration::from_secs(5));
    // negative-wait: a bounded window proving a cancelled session never resubscribes.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        scripted.recorded_opens().len(),
        1,
        "a cancelled session never resubscribes"
    );
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_names_the_method_the_host_requires() {
    let directory = tempfile::tempdir().unwrap();
    let sink = TestSink::open();
    let scripted = ScriptedCapability::new();
    let mut harness = start_session(
        sink,
        SessionOptions::default(),
        scripted.clone(),
        &directory,
    )
    .await;
    wait_for_opens(&scripted, 1).await;

    // The host reads `method` from a capability stream payload and refuses the frame without it.
    assert_eq!(scripted.recorded_opens()[0]["method"], "subscribe");
    harness.dispose().await.unwrap();
    harness.shutdown().await.unwrap();
}
