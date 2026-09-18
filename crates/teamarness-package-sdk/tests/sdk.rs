use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use teamarness_package_protocol::{Frame, FrameKind, JsonValue, decode_frame};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, EventSubscription, Package, event_channel,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Echo;

impl Backend for Echo {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            if request.operation == "fixture.capability" {
                return request
                    .capabilities
                    .call("Z3JhbnQx", "get", JsonValue::Object(Default::default()))
                    .await;
            }
            if request.operation == "fixture.capability_stream"
                || request.operation == "fixture.capability_stream_error"
            {
                let mut arguments = JsonValue::Object(Default::default());
                arguments
                    .as_object_mut()
                    .unwrap()
                    .insert("method".into(), JsonValue::String("subscribe".into()));
                arguments
                    .as_object_mut()
                    .unwrap()
                    .insert("afterCursor".into(), JsonValue::Null);
                let mut stream = request
                    .capabilities
                    .subscribe("Z3JhbnQx", arguments)
                    .await?;
                if request.operation == "fixture.capability_stream_error" {
                    let error = stream
                        .recv()
                        .await
                        .and_then(Result::err)
                        .unwrap_or_else(|| {
                            BackendError::new(
                                "stream_closed",
                                false,
                                JsonValue::Object(Default::default()),
                            )
                        });
                    return Err(error);
                }
                let event = stream.recv().await.ok_or_else(|| {
                    BackendError::new(
                        "stream_closed",
                        false,
                        JsonValue::Object(Default::default()),
                    )
                })??;
                stream.close().await;
                return Ok(event);
            }
            if request.operation == "fixture.slow" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            if request.operation == "fixture.error" {
                Err(BackendError::new(
                    "fixture.error",
                    false,
                    JsonValue::Object(Default::default()),
                ))
            } else {
                Ok(request.payload)
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async {
            let (sender, subscription) = event_channel(2);
            tokio::spawn(async move {
                let mut first = JsonValue::Object(Default::default());
                first
                    .as_object_mut()
                    .unwrap()
                    .insert("order".into(), JsonValue::from(1));
                let mut second = JsonValue::Object(Default::default());
                second
                    .as_object_mut()
                    .unwrap()
                    .insert("order".into(), JsonValue::from(2));
                sender.send(first).await.unwrap();
                sender.send(second).await.unwrap();
            });
            Ok(subscription)
        })
    }
}

async fn write_json(stream: &mut tokio::io::DuplexStream, value: &str) {
    let payload = value.as_bytes();
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    stream.write_all(payload).await.unwrap();
}

async fn read_frame(stream: &mut tokio::io::DuplexStream) -> Frame {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    decode_frame(&bytes).unwrap()
}

#[tokio::test]
async fn sdk_routes_capability_stream_events_and_returns_credit() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(Arc::new(Echo)).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, r#"{"kind":"request","id":"h:stream","operation":"fixture.capability_stream","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    let opened = read_frame(&mut host_read).await;
    assert_eq!(opened.kind(), FrameKind::StreamOpen);
    let stream_id = opened.field("id").unwrap().as_str().unwrap().to_owned();
    write_json(&mut host_write, &format!(r#"{{"kind":"event","streamId":"{stream_id}","sequence":"1","payload":{{"cursor":"next"}}}}"#)).await;
    let credit = read_frame(&mut host_read).await;
    assert_eq!(credit.kind(), FrameKind::Credit);
    assert_eq!(credit.field("amount").unwrap(), 1);
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::StreamClose
    );
    let response = read_frame(&mut host_read).await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(response.field("id").unwrap(), "h:stream");
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}

#[tokio::test]
async fn sdk_does_not_credit_a_terminal_capability_error() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(Arc::new(Echo)).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, r#"{"kind":"request","id":"h:stream-error","operation":"fixture.capability_stream_error","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    let opened = read_frame(&mut host_read).await;
    assert_eq!(opened.kind(), FrameKind::StreamOpen);
    let stream_id = opened.field("id").unwrap().as_str().unwrap();
    write_json(
        &mut host_write,
        &format!(
            r#"{{"kind":"stream_close","streamId":"{stream_id}","code":"capability_unavailable"}}"#
        ),
    )
    .await;
    let error = read_frame(&mut host_read).await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("id").unwrap(), "h:stream-error");
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::StreamClose
    );
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}

#[tokio::test]
async fn sdk_hides_framing_dispatches_work_and_honors_shutdown() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(Arc::new(Echo)).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );

    write_json(&mut host_write, r#"{"kind":"request","id":"h:one","operation":"fixture.echo","deadlineUnixMs":"9999999999999","payload":{"value":1}}"#).await;
    let response = read_frame(&mut host_read).await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(response.field("id").unwrap(), "h:one");

    write_json(&mut host_write, r#"{"kind":"request","id":"h:cap","operation":"fixture.capability","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    let capability = read_frame(&mut host_read).await;
    assert_eq!(capability.kind(), FrameKind::Request);
    let capability_payload = capability.field("payload").unwrap().as_object().unwrap();
    assert_eq!(capability_payload.len(), 1);
    assert_eq!(capability_payload.get("method").unwrap(), "get");
    let capability_id = capability.field("id").unwrap().as_str().unwrap();
    write_json(
        &mut host_write,
        &format!(r#"{{"kind":"response","id":"{capability_id}","payload":{{"ok":true}}}}"#),
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.field("id").unwrap(),
        "h:cap"
    );

    write_json(&mut host_write, r#"{"kind":"stream_open","id":"h:events","topic":"fixture.events","initialCredit":32,"payload":{}}"#).await;
    for sequence in ["1", "2"] {
        let event = read_frame(&mut host_read).await;
        assert_eq!(event.kind(), FrameKind::Event);
        assert_eq!(event.field("sequence").unwrap(), sequence);
    }
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::StreamClose
    );

    write_json(&mut host_write, r#"{"kind":"request","id":"h:slow","operation":"fixture.slow","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    write_json(&mut host_write, r#"{"kind":"ping","nonce":"cGluZw"}"#).await;
    assert_eq!(read_frame(&mut host_read).await.kind(), FrameKind::Pong);
    assert_eq!(
        read_frame(&mut host_read).await.field("id").unwrap(),
        "h:slow"
    );
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}

/// Keeps a `CapabilityClient` alive in Package-owned background work that never
/// finishes, like a long-lived synchronization session.
struct RetainsCapabilities;

impl Backend for RetainsCapabilities {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            let capabilities = request.capabilities;
            tokio::spawn(async move {
                let _retained = capabilities;
                std::future::pending::<()>().await;
            });
            Ok(JsonValue::Object(Default::default()))
        })
    }
}

const FIXTURE_INITIALIZE: &str = r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#;

async fn start_retaining_session() -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<Result<(), teamarness_package_sdk::SdkError>>,
) {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service =
        tokio::spawn(Package::builder(Arc::new(RetainsCapabilities)).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, FIXTURE_INITIALIZE).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, r#"{"kind":"request","id":"h:activate","operation":"lifecycle.activate","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    assert_eq!(read_frame(&mut host_read).await.kind(), FrameKind::Response);
    (host_read, host_write, service)
}

#[tokio::test]
async fn sdk_session_ends_on_shutdown_while_package_tasks_retain_capabilities() {
    let (mut host_read, mut host_write, service) = start_retaining_session().await;
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    // The host keeps its end open; the session must not wait for Package tasks.
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), service)
        .await
        .expect("serve_io returned after shutdown_ack");
    result.unwrap().unwrap();
    drop(host_write);
}

#[tokio::test]
async fn sdk_session_ends_on_parent_closure_while_package_tasks_retain_capabilities() {
    let (_host_read, host_write, service) = start_retaining_session().await;
    drop(host_write);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), service)
        .await
        .expect("serve_io returned after parent closure");
    assert_eq!(
        result.unwrap(),
        Err(teamarness_package_sdk::SdkError::ParentClosed)
    );
}

#[test]
fn sdk_dependency_boundary_excludes_host_and_runtime_internals() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    for forbidden in [
        "teamarness-core",
        "teamarness-package-host",
        "tauri",
        "iroh",
        "p2panda",
        "rusqlite",
    ] {
        assert!(!manifest.contains(forbidden), "SDK depends on {forbidden}");
    }
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    for forbidden in [
        "teamarness_core",
        "teamarness_package_host",
        "rusqlite",
        "iroh",
    ] {
        assert!(!source.contains(forbidden), "SDK source names {forbidden}");
    }
}

/// Calls a capability from Package-owned background work and reports what the
/// call returned, so a test can observe a call that the Host never answers.
struct ReportsCapabilityOutcome {
    outcome: tokio::sync::mpsc::Sender<Result<JsonValue, BackendError>>,
}

impl Backend for ReportsCapabilityOutcome {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        let outcome = self.outcome.clone();
        Box::pin(async move {
            let capabilities = request.capabilities;
            tokio::spawn(async move {
                let result = capabilities
                    .call("Z3JhbnQx", "get", JsonValue::Object(Default::default()))
                    .await;
                let _ = outcome.send(result).await;
            });
            Ok(JsonValue::Object(Default::default()))
        })
    }
}

async fn start_reporting_session(
    outcome: tokio::sync::mpsc::Sender<Result<JsonValue, BackendError>>,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<Result<(), teamarness_package_sdk::SdkError>>,
) {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(
        Package::builder(Arc::new(ReportsCapabilityOutcome { outcome }))
            .serve_io(sdk_read, sdk_write),
    );
    write_json(&mut host_write, FIXTURE_INITIALIZE).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, r#"{"kind":"request","id":"h:call","operation":"fixture.capability","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    assert_eq!(read_frame(&mut host_read).await.kind(), FrameKind::Response);
    // The capability request reaches the host, which deliberately never answers.
    let request = read_frame(&mut host_read).await;
    assert_eq!(request.kind(), FrameKind::Request);
    assert_eq!(request.field("operation").unwrap(), "capability.call");
    (host_read, host_write, service)
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test(start_paused = true)]
async fn capability_call_times_out_when_host_never_answers() {
    let (outcome, mut outcomes) = tokio::sync::mpsc::channel(1);
    let (_host_read, host_write, service) = start_reporting_session(outcome).await;
    tokio::time::advance(std::time::Duration::from_millis(11_001)).await;
    let result = outcomes.recv().await.expect("the call reported an outcome");
    let error = result.expect_err("an unanswered call gives up");
    assert_eq!(error.code(), "deadline_exceeded");
    assert!(error.retryable(), "a call that timed out may be retried");
    drop(host_write);
    let _ = service.await;
}

/// Verifies: TEAM.SYS.PKG-R10
#[tokio::test]
async fn pending_capability_calls_fail_when_the_session_ends() {
    let (outcome, mut outcomes) = tokio::sync::mpsc::channel(1);
    let (host_read, host_write, service) = start_reporting_session(outcome).await;
    drop(host_write);
    drop(host_read);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), outcomes.recv())
        .await
        .expect("the pending call was answered when the session ended")
        .expect("the call reported an outcome");
    let error = result.expect_err("nothing can answer the call now");
    assert_eq!(error.code(), "host_unavailable");
    let _ = service.await;
}

/// A Backend that refuses every subscription with its own code.
struct RefusesSubscriptions;

impl Backend for RefusesSubscriptions {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move { Ok(request.payload) })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async {
            Err(BackendError::new(
                "fixture.not_found",
                false,
                JsonValue::Object(Default::default()),
            ))
        })
    }
}

#[tokio::test]
async fn sdk_forwards_the_package_code_when_subscribe_fails() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(
        Package::builder(Arc::new(RefusesSubscriptions)).serve_io(sdk_read, sdk_write),
    );
    write_json(&mut host_write, r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, r#"{"kind":"stream_open","id":"h:stream-1","topic":"fixture.events","mountId":"bW91bnQx","initialCredit":32,"payload":{}}"#).await;
    let closed = read_frame(&mut host_read).await;
    assert_eq!(closed.kind(), FrameKind::StreamClose);
    assert_eq!(closed.field("code").unwrap(), "fixture.not_found");
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}
