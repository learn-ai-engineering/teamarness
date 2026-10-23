//! One round trip of disagreement is not a protocol violation.
//!
//! Every test here drives the real reader and writer over a pipe and asserts
//! that a frame a conforming peer could have sent before it saw this side
//! retire the state it names leaves the generation alive.

#[allow(dead_code)]
#[path = "../src/protocol.rs"]
mod host_protocol;
#[allow(dead_code)]
#[path = "../src/manifest.rs"]
mod manifest;
#[allow(dead_code)]
#[path = "../src/ui.rs"]
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use teamarness_package_protocol::{
    Frame, FrameKind, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION, encode_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use host_protocol::{
    CapabilityFailure, HostStreamSender, ProcessClient, ProtocolLimits, start_with_capabilities,
};

pub use teamarness_package_host::{
    MAX_PACKAGE_ERROR_DETAILS_BYTES, MAX_PACKAGE_ERROR_DETAILS_DEPTH,
    MAX_UI_SUBSCRIPTIONS_PER_MOUNT, PackageErrorCode, PackageIdentity, PackageOwnedError,
    PackageValue,
};

/// The Package's end of the pipe: it writes what the Host reads.
struct FakePackage {
    stream: DuplexStream,
    buffer: Vec<u8>,
}

impl FakePackage {
    async fn send(&mut self, value: Value) {
        let frame = Frame::try_from(value).expect("valid frame");
        let bytes = encode_frame(&frame).expect("encoded frame");
        self.stream.write_all(&bytes).await.expect("write");
        self.stream.flush().await.expect("flush");
    }

    /// Proves the generation is still serving: a capability call that is
    /// answered has travelled the reader, the handler and the writer.
    async fn round_trip(&mut self, id: &str) {
        self.send(json!({
            "kind":"request", "id":id, "operation":"capability.call",
            "grant":"Z3JhbnQx", "deadlineUnixMs":"9007199254740998",
            "payload":{"method":"get"}
        }))
        .await;
        loop {
            let frame = self.next_frame().await;
            if matches!(frame.kind(), FrameKind::Response | FrameKind::Error)
                && frame.field("id").and_then(Value::as_str) == Some(id)
            {
                assert_eq!(frame.kind(), FrameKind::Response, "the probe was answered");
                return;
            }
        }
    }

    /// Reads frames until one of `kind` arrives, so the test knows the Host's
    /// writer has already observed everything queued before it.
    async fn read_until(&mut self, kind: FrameKind) -> Frame {
        loop {
            let frame = self.next_frame().await;
            if frame.kind() == kind {
                return frame;
            }
        }
    }

    /// Reads one chunk without completing a frame.
    ///
    /// The first byte of a frame the pipe cannot hold proves the Host's writer
    /// is inside `write_all` and will stay there until this end drains the
    /// rest, which is how a test holds the writer still without a sleep.
    async fn read_one_chunk(&mut self) {
        let mut chunk = [0_u8; 4096];
        let read = self.stream.read(&mut chunk).await.expect("read");
        assert!(read > 0, "the Host closed the pipe");
        self.buffer.extend_from_slice(&chunk[..read]);
    }

    async fn next_frame(&mut self) -> Frame {
        loop {
            if self.buffer.len() >= 4 {
                let length = u32::from_be_bytes(self.buffer[..4].try_into().unwrap()) as usize;
                if self.buffer.len() >= length + 4 {
                    let bytes = self.buffer[..length + 4].to_vec();
                    self.buffer.drain(..length + 4);
                    return teamarness_package_protocol::decode_frame(&bytes).expect("frame");
                }
            }
            let mut chunk = [0_u8; 4096];
            let read = self.stream.read(&mut chunk).await.expect("read");
            assert!(read > 0, "the Host closed the pipe");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

fn connect(
    capabilities: Arc<dyn host_protocol::CapabilityHandler>,
    pipe_bytes: usize,
) -> (ProcessClient, FakePackage) {
    connect_with_limits(capabilities, pipe_bytes, ProtocolLimits::CURRENT)
}

fn connect_with_limits(
    capabilities: Arc<dyn host_protocol::CapabilityHandler>,
    pipe_bytes: usize,
    limits: ProtocolLimits,
) -> (ProcessClient, FakePackage) {
    let (host_end, package_end) = tokio::io::duplex(pipe_bytes);
    let (host_read, host_write) = tokio::io::split(host_end);
    let client = start_with_capabilities(host_read, host_write, capabilities, limits);
    (
        client,
        FakePackage {
            stream: package_end,
            buffer: Vec::new(),
        },
    )
}

/// A capability handler that opens a stream and closes it immediately, the way
/// a record stream ends once its subscription is gone.
struct ClosingStream;

impl host_protocol::CapabilityHandler for ClosingStream {
    fn call<'a>(
        &'a self,
        _grant: Option<&'a str>,
        _payload: &'a Value,
        _started: Arc<AtomicBool>,
        _consumer_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    > {
        Box::pin(async { Ok(json!({})) })
    }

    fn open_stream<'a>(
        &'a self,
        stream_id: String,
        _topic: &'a str,
        _initial_credit: u64,
        _payload: &'a Value,
        output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(async move {
            tokio::spawn(async move {
                let _ = output.event(&stream_id, 1, json!({"order":1})).await;
                let _ = output.close(&stream_id, "capability_unavailable").await;
            });
            Ok(())
        })
    }

    fn add_credit<'a>(
        &'a self,
        _stream_id: &'a str,
        _amount: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn close_stream<'a>(
        &'a self,
        _stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// CR-003: a page of events larger than the writer's data budget is
/// backpressure, not a capability failure.
///
/// `run_record_stream`'s tail is modelled exactly: a failed `event` ends the
/// stream with `capability_unavailable`, which the SDK's own table calls
/// permanent. A contract-maximum record carries two base64 copies, so a page of
/// them is far more than the data lane holds at once, and the events must wait
/// for capacity rather than tear the stream down.
struct LargePage;

/// Events per page, and the payload size that overruns the data budget together.
const PAGE_EVENTS: u64 = 16;
const PAGE_EVENT_BYTES: usize = 512 * 1024;

impl host_protocol::CapabilityHandler for LargePage {
    fn call<'a>(
        &'a self,
        _grant: Option<&'a str>,
        _payload: &'a Value,
        _started: Arc<AtomicBool>,
        _consumer_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    > {
        Box::pin(async { Ok(json!({})) })
    }

    fn open_stream<'a>(
        &'a self,
        stream_id: String,
        _topic: &'a str,
        _initial_credit: u64,
        _payload: &'a Value,
        output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(async move {
            tokio::spawn(async move {
                let record = "A".repeat(PAGE_EVENT_BYTES);
                for sequence in 1..=PAGE_EVENTS {
                    let payload = json!({"kind":"record","canonical":record});
                    if output.event(&stream_id, sequence, payload).await.is_err() {
                        output.retire(&stream_id).await;
                        let _ = output.close(&stream_id, "capability_unavailable").await;
                        return;
                    }
                }
                output.retire(&stream_id).await;
                let _ = output.close(&stream_id, "completed").await;
            });
            Ok(())
        })
    }

    fn add_credit<'a>(
        &'a self,
        _stream_id: &'a str,
        _amount: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn close_stream<'a>(
        &'a self,
        _stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// CR-003: the whole page reaches the Package behind a pipe it drains slowly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_page_larger_than_the_data_budget_is_backpressure() {
    // A realistic 64 KiB pipe, far smaller than one event of this page.
    let (client, mut package) = connect(Arc::new(LargePage), 64 * 1024);
    package
        .send(json!({
            "kind":"stream_open", "id":"p:big-1", "topic":"capability.stream",
            "initialCredit":32, "payload":{"grant":"Z3JhbnQx"}
        }))
        .await;
    let mut delivered = 0_u64;
    loop {
        let frame = package.next_frame().await;
        match frame.kind() {
            FrameKind::Event => delivered += 1,
            FrameKind::StreamClose => {
                assert_eq!(
                    frame.field("code").and_then(Value::as_str),
                    Some("completed"),
                    "a slow reader is backpressure, not a capability outage"
                );
                break;
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert_eq!(delivered, PAGE_EVENTS, "every event of the page arrives");
    assert_eq!(client.failure(), None);
}

/// CR-002: credit the Package released for events it had already accepted
/// arrives after the Host's own `stream_close`.
#[tokio::test]
async fn late_credit_for_a_closed_capability_stream_leaves_the_generation_alive() {
    let (client, mut package) = connect(Arc::new(ClosingStream), 64 * 1024);
    package
        .send(json!({
            "kind":"stream_open", "id":"p:cap-1", "topic":"capability.stream",
            "initialCredit":32, "payload":{"grant":"Z3JhbnQx"}
        }))
        .await;
    package.read_until(FrameKind::Event).await;
    // Reading the close proves the writer has already observed it.
    package.read_until(FrameKind::StreamClose).await;
    package
        .send(json!({"kind":"credit","streamId":"p:cap-1","amount":1}))
        .await;
    package.round_trip("p:probe-late").await;
    assert_eq!(client.failure(), None);
}

/// CR-007's mirror on the Host's own sending side: an event written before the
/// Package's `stream_close` was observed is not a second close of anything.
#[tokio::test]
async fn a_duplicate_stream_close_from_the_package_is_ignored() {
    let (client, mut package) = connect(Arc::new(ClosingStream), 64 * 1024);
    package
        .send(json!({
            "kind":"stream_open", "id":"p:cap-1", "topic":"capability.stream",
            "initialCredit":32, "payload":{"grant":"Z3JhbnQx"}
        }))
        .await;
    package.read_until(FrameKind::StreamClose).await;
    package
        .send(json!({"kind":"stream_close","streamId":"p:cap-1","code":"closed"}))
        .await;
    package.round_trip("p:probe-dupe").await;
    assert_eq!(client.failure(), None);
}

/// A frame naming a stream that was never opened is still fatal: the tombstone
/// separates a peer that is behind from a peer that is inventing.
#[tokio::test]
async fn credit_for_a_stream_that_never_opened_is_still_fatal() {
    let (client, mut package) = connect(Arc::new(ClosingStream), 64 * 1024);
    package
        .send(json!({"kind":"credit","streamId":"p:ghost","amount":1}))
        .await;
    assert_eq!(
        wait_for_failure(&client).await,
        PackageErrorCode::ProtocolViolation
    );
}

/// The Host's own outbound `cancel` after the answer has already arrived.
#[tokio::test]
async fn a_cancel_that_races_its_answer_does_not_kill_the_writer() {
    let (client, mut package) = connect(Arc::new(ClosingStream), 64 * 1024);
    let request = client.clone();
    let answered = tokio::spawn(async move {
        request
            .request(
                "h:one".to_owned(),
                "ui.read",
                None,
                PackageValue::Object(Default::default()),
                Duration::from_secs(30),
                false,
            )
            .await
    });
    package.read_until(FrameKind::Request).await;
    package
        .send(json!({"kind":"response","id":"h:one","payload":{}}))
        .await;
    answered.await.expect("join").expect("answered");
    // The cancel the Host raced against its own answer must not be written, and
    // must not fail the generation the Host is serving.
    client.cancel("h:one").await.expect("cancel queued");
    package.round_trip("p:probe-cancel").await;
    assert_eq!(client.failure(), None);
}

/// CR2-001: a `response` that crossed the Host's own `cancel` is ignored.
///
/// The Host stops waiting for the request inside `cancel`, so the request is
/// retired there. Retiring only when the writer observes the queued `cancel`
/// leaves a window — one the writer holds open for as long as it is blocked on
/// a slow pipe — in which the answer names a request the classifier still
/// calls live and the adapter has already forgotten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_that_crossed_the_hosts_cancel_leaves_the_generation_alive() {
    // A pipe far smaller than the second request, so the writer blocks inside
    // `write_all` with the `cancel` still queued behind it.
    let (client, mut package) = connect(Arc::new(ClosingStream), 1024);
    let cancelled = client.clone();
    let first = tokio::spawn(async move {
        cancelled
            .request(
                "h:one".to_owned(),
                "ui.read",
                None,
                PackageValue::Object(Default::default()),
                Duration::from_secs(30),
                false,
            )
            .await
    });
    package.read_until(FrameKind::Request).await;

    let blocked = client.clone();
    let mut payload = std::collections::BTreeMap::new();
    payload.insert(
        "canonical".to_owned(),
        PackageValue::String("A".repeat(256 * 1024)),
    );
    let held = tokio::spawn(async move {
        blocked
            .request(
                "h:two".to_owned(),
                "ui.read",
                None,
                PackageValue::Object(payload),
                Duration::from_secs(30),
                false,
            )
            .await
    });
    // One chunk of the oversized request: the writer is now inside `write_all`
    // and cannot reach the `cancel` queued behind it.
    package.read_one_chunk().await;

    client.cancel("h:one").await.expect("cancel queued");
    package
        .send(json!({"kind":"response","id":"h:one","payload":{}}))
        .await;
    assert_eq!(
        first.await.expect("join").expect_err("cancelled").code,
        PackageErrorCode::Cancelled
    );

    // Draining the pipe releases the writer; the probe proves the reader, the
    // handler and the writer all survived the answer that crossed the cancel.
    // A reader that failed the generation never answers it.
    tokio::time::timeout(
        Duration::from_secs(10),
        package.round_trip("p:probe-crossing"),
    )
    .await
    .expect("the generation still answers after the crossing response");
    assert_eq!(client.failure(), None);
    held.abort();
}

/// CR-004: every in-flight capability call the contract permits is answered,
/// behind a pipe the Package drains slowly and with contract-maximum payloads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_in_flight_capability_call_is_answered_behind_a_slow_pipe() {
    struct LargeAnswer;
    impl host_protocol::CapabilityHandler for LargeAnswer {
        fn call<'a>(
            &'a self,
            _grant: Option<&'a str>,
            _payload: &'a Value,
            _started: Arc<AtomicBool>,
            _consumer_id: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
        > {
            // One contract-maximum canonical record, as `record_log.open`
            // legitimately returns.
            Box::pin(async { Ok(json!({"canonical":"A".repeat(349_526)})) })
        }

        fn open_stream<'a>(
            &'a self,
            _stream_id: String,
            _topic: &'a str,
            _initial_credit: u64,
            _payload: &'a Value,
            _output: HostStreamSender,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
        > {
            Box::pin(async { Err(PackageErrorCode::CapabilityRequestInvalid) })
        }

        fn add_credit<'a>(
            &'a self,
            _stream_id: &'a str,
            _amount: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }

        fn close_stream<'a>(
            &'a self,
            _stream_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    // A realistic 64 KiB pipe, far smaller than the answers it must carry.
    let (client, mut package) = connect(Arc::new(LargeAnswer), 64 * 1024);
    for index in 0..MAX_IN_FLIGHT_EACH_DIRECTION {
        package
            .send(json!({
                "kind":"request", "id":format!("p:call-{index}"),
                "operation":"capability.call", "grant":"Z3JhbnQx",
                "deadlineUnixMs":"9007199254740998",
                "payload":{"method":"open"}
            }))
            .await;
    }
    let mut answered = std::collections::BTreeSet::new();
    while answered.len() < MAX_IN_FLIGHT_EACH_DIRECTION {
        let frame = package.next_frame().await;
        assert!(
            matches!(frame.kind(), FrameKind::Response | FrameKind::Error),
            "unexpected frame {:?}",
            frame.kind()
        );
        let id = frame
            .field("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        assert!(answered.insert(id), "exactly one terminal frame per id");
    }
    assert_eq!(client.failure(), None);
}

/// SF2-002: Rule 1's third verdict, under the congestion that triggers it.
///
/// A capability answer that cannot get control-lane capacity within the call
/// deadline is answered for that one request id with a typed retryable error,
/// never by dropping the answer and never by failing the generation. The
/// fallback is itself that request's one terminal frame, so it waits for
/// capacity too: the budget is exactly full here, which is the one condition
/// under which a refusing send degrades the third verdict back to the
/// `ProcessExited` it exists to replace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capability_answer_that_outlasts_its_deadline_is_answered_overloaded() {
    /// Answers sized so two of them hold the whole control byte budget
    /// (`CONTROL_BYTES == 2 * MAX_FRAME_BYTES`) and leave nothing behind.
    struct BudgetFillingAnswer;

    impl host_protocol::CapabilityHandler for BudgetFillingAnswer {
        fn call<'a>(
            &'a self,
            _grant: Option<&'a str>,
            payload: &'a Value,
            _started: Arc<AtomicBool>,
            _consumer_id: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
        > {
            let bytes = payload
                .get("bytes")
                .and_then(Value::as_u64)
                .expect("the test sizes the answer") as usize;
            Box::pin(async move { Ok(json!({"canonical":"A".repeat(bytes)})) })
        }

        fn open_stream<'a>(
            &'a self,
            _stream_id: String,
            _topic: &'a str,
            _initial_credit: u64,
            _payload: &'a Value,
            _output: HostStreamSender,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
        > {
            Box::pin(async { Err(PackageErrorCode::CapabilityRequestInvalid) })
        }

        fn add_credit<'a>(
            &'a self,
            _stream_id: &'a str,
            _amount: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }

        fn close_stream<'a>(
            &'a self,
            _stream_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    const CALLS: usize = 8;
    // Half the control lane's byte budget per answer: two fill it exactly.
    let target = MAX_FRAME_BYTES;
    let sized = |id: &str, bytes: usize| -> usize {
        let frame = Frame::try_from(json!({
            "kind":"response", "id":id, "payload":{"canonical":"A".repeat(bytes)}
        }))
        .expect("valid frame");
        encode_frame(&frame).expect("encoded frame").len()
    };
    // One ASCII byte of payload is one byte of frame, so one probe is enough.
    let probe = sized("p:slow-0", 1024);
    let answer_bytes = 1024 + target - probe;
    assert_eq!(sized("p:slow-0", answer_bytes), target);

    // A pipe far smaller than one answer, and a call deadline short enough
    // that congestion outlasts it while this end holds the pipe shut.
    let (client, mut package) = connect_with_limits(
        Arc::new(BudgetFillingAnswer),
        1024,
        ProtocolLimits {
            capability_call: Duration::from_millis(50),
        },
    );
    for index in 0..CALLS {
        package
            .send(json!({
                "kind":"request", "id":format!("p:slow-{index}"),
                "operation":"capability.call", "grant":"Z3JhbnQx",
                "deadlineUnixMs":"9007199254740998",
                "payload":{"method":"open","bytes":answer_bytes}
            }))
            .await;
    }
    // The answers must reach the point of waiting for capacity, and that wait
    // must outlast the call deadline, before this end drains a byte.
    // settle: the writer exposes no event for "a queued frame is waiting".
    tokio::time::sleep(Duration::from_millis(400)).await;

    let mut answered = std::collections::BTreeMap::new();
    let collected = tokio::time::timeout(Duration::from_secs(30), async {
        while answered.len() < CALLS {
            let frame = package.next_frame().await;
            assert!(
                matches!(frame.kind(), FrameKind::Response | FrameKind::Error),
                "unexpected frame {:?}",
                frame.kind()
            );
            let id = frame
                .field("id")
                .and_then(Value::as_str)
                .unwrap()
                .to_owned();
            let code = frame
                .field("code")
                .and_then(Value::as_str)
                .unwrap_or("response")
                .to_owned();
            if code == "overloaded" {
                assert_eq!(
                    frame.field("retryable").and_then(Value::as_bool),
                    Some(true),
                    "the third verdict is retryable"
                );
            }
            assert!(
                answered.insert(id, code).is_none(),
                "exactly one terminal frame per id"
            );
        }
    })
    .await;
    assert!(
        collected.is_ok(),
        "only {} of {CALLS} calls were answered: {answered:?}",
        answered.len()
    );
    assert!(
        answered.values().any(|code| code == "overloaded"),
        "congestion that outlasts the deadline is answered `overloaded`: {answered:?}"
    );
    assert_eq!(
        client.failure(),
        None,
        "the generation survives its own congestion"
    );
}

async fn wait_for_failure(client: &ProcessClient) -> PackageErrorCode {
    tokio::time::timeout(Duration::from_secs(5), client.wait_for_failure())
        .await
        .expect("the generation failed")
}

/// Keeps the unused-import lint honest about the shared bound this file pins.
#[test]
fn the_host_owns_one_subscription_bound() {
    assert_eq!(MAX_UI_SUBSCRIPTIONS_PER_MOUNT, 8);
    assert_eq!(Ordering::Acquire, Ordering::Acquire);
}
