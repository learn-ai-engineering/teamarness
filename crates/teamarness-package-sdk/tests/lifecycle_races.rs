//! The Package's half of the one classifier.
//!
//! Each test drives the real `serve_io` session and feeds every frame the
//! Package writes through a `ContractState`, which is what the Host does with
//! them. A frame the Host would call fatal fails the test here.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use teamarness_package_protocol::{ContractState, Direction, Frame, FrameKind, JsonValue, Verdict};
use teamarness_package_sdk::{Backend, BackendError, BackendRequest, Package, SdkError};

mod support;

use support::{INITIALIZE, read_frame, write_json};

/// The Host's view of the session: the same classifier the real Host runs.
struct HostView {
    state: ContractState,
}

impl HostView {
    fn new() -> Self {
        Self {
            state: ContractState::default(),
        }
    }

    /// Observes one frame the Package wrote, as the Host's reader does.
    fn observe_from_package(&mut self, frame: &Frame) -> Verdict {
        self.state
            .observe(Direction::PackageToHost, frame)
            .unwrap_or_else(|error| {
                panic!(
                    "the Host would fail the generation on a {:?} frame: {}",
                    frame.kind(),
                    error.code()
                )
            })
    }

    /// Observes one frame the Host is about to write, as its writer does.
    fn observe_to_package(&mut self, value: &str) {
        let parsed: serde_json::Value = serde_json::from_str(value).expect("frame JSON");
        let frame = Frame::try_from(parsed).expect("valid frame");
        self.state
            .observe(Direction::HostToPackage, &frame)
            .expect("the Host's own frame is in contract");
    }
}

struct Fixture;

impl Backend for Fixture {
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
            if request.operation == "fixture.capability_stream" {
                let mut arguments = JsonValue::Object(Default::default());
                let object = arguments.as_object_mut().unwrap();
                object.insert("method".into(), JsonValue::String("subscribe".into()));
                object.insert("afterCursor".into(), JsonValue::Null);
                let mut stream = request
                    .capabilities
                    .subscribe("Z3JhbnQx", arguments)
                    .await?;
                let event = stream.recv().await.ok_or_else(|| {
                    BackendError::new(
                        "stream_closed",
                        false,
                        JsonValue::Object(Default::default()),
                    )
                })??;
                // The route goes before the close reaches the Host, which is
                // what leaves the Host's already-written events in the pipe.
                stream.close().await;
                return Ok(event);
            }
            Ok(request.payload)
        })
    }
}

type Session = tokio::task::JoinHandle<Result<(), SdkError>>;

fn start() -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    Session,
    HostView,
) {
    start_with(Arc::new(Fixture), 1024 * 1024)
}

fn start_with<B>(
    backend: Arc<B>,
    pipe_bytes: usize,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    Session,
    HostView,
)
where
    B: Backend + 'static,
{
    let (host_read, sdk_write) = tokio::io::duplex(pipe_bytes);
    let (sdk_read, host_write) = tokio::io::duplex(pipe_bytes);
    let service = tokio::spawn(Package::builder(backend).serve_io(sdk_read, sdk_write));
    (host_read, host_write, service, HostView::new())
}

/// CR2-002: every legal in-budget request is answered, however saturated the
/// Package's own writer is.
///
/// The contract's guarantee is absolute: "A sender never withholds a frame it
/// owes. Exactly one `response` or `error` still reaches the wire for every
/// request." A control lane that refuses on bytes breaks it silently — the
/// handler task has already claimed the request's one terminal frame, so the
/// cancel path will not write one either, and the request receives nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_in_flight_request_is_answered_behind_a_saturated_writer() {
    /// Answers big enough that only two fit the control lane's byte budget.
    const ANSWER_BYTES: usize = 900_000;
    const REQUESTS: usize = 8;

    /// Holds every handler until all of them have an answer to write, so they
    /// contend for the byte budget before any frame has drained.
    struct SaturatingAnswers {
        released: Arc<tokio::sync::Barrier>,
    }

    impl Backend for SaturatingAnswers {
        fn handle(
            &self,
            _request: BackendRequest,
        ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
            Box::pin(async move {
                self.released.wait().await;
                let mut payload = JsonValue::Object(Default::default());
                payload.as_object_mut().unwrap().insert(
                    "canonical".into(),
                    JsonValue::String("A".repeat(ANSWER_BYTES)),
                );
                Ok(payload)
            })
        }
    }

    // A pipe far smaller than one answer, so a frame that has acquired its
    // permits holds them for many round trips.
    let (mut host_read, mut host_write, service, mut host) = start_with(
        Arc::new(SaturatingAnswers {
            released: Arc::new(tokio::sync::Barrier::new(REQUESTS)),
        }),
        1024,
    );
    handshake(&mut host_read, &mut host_write, &mut host).await;
    for index in 0..REQUESTS {
        let request = format!(
            r#"{{"kind":"request","id":"h:big-{index}","operation":"fixture.echo","deadlineUnixMs":"9999999999999","payload":{{}}}}"#
        );
        host.observe_to_package(&request);
        write_json(&mut host_write, &request).await;
    }

    let mut answered = std::collections::BTreeSet::new();
    let collected = tokio::time::timeout(Duration::from_secs(30), async {
        while answered.len() < REQUESTS {
            let frame = read_frame(&mut host_read).await;
            assert!(
                matches!(frame.kind(), FrameKind::Response | FrameKind::Error),
                "unexpected frame {:?}",
                frame.kind()
            );
            host.observe_from_package(&frame);
            answered.insert(
                frame
                    .field("id")
                    .and_then(JsonValue::as_str)
                    .unwrap()
                    .to_owned(),
            );
        }
    })
    .await;
    assert!(
        collected.is_ok(),
        "only {} of {REQUESTS} requests were answered: {answered:?}",
        answered.len()
    );
    drop(host_write);
    let _ = service.await;
}

async fn handshake(
    host_read: &mut tokio::io::DuplexStream,
    host_write: &mut tokio::io::DuplexStream,
    host: &mut HostView,
) {
    host.observe_to_package(INITIALIZE);
    write_json(host_write, INITIALIZE).await;
    let initialized = read_frame(host_read).await;
    assert_eq!(initialized.kind(), FrameKind::Initialized);
    host.observe_from_package(&initialized);
}

/// CR-005: a cancel that races a finishing request must not produce a second
/// terminal frame for the same id.
///
/// Exactly one `response` or `error` is terminal for a request, and the Host
/// fails the generation on a second. Before the interlock, 13 ids in 300 got
/// two: the handler task wrote its answer while the dispatcher was answering
/// the cancel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_racing_a_finishing_request_never_emits_a_second_terminal() {
    const ROUNDS: usize = 300;
    let (mut host_read, mut host_write, service, _host) = start();
    let mut view = HostView::new();
    handshake(&mut host_read, &mut host_write, &mut view).await;

    let writer = tokio::spawn(async move {
        for round in 0..ROUNDS {
            write_json(
                &mut host_write,
                &format!(
                    r#"{{"kind":"request","id":"h:race-{round}","operation":"fixture.echo","deadlineUnixMs":"9999999999999","payload":{{}}}}"#
                ),
            )
            .await;
            tokio::task::yield_now().await;
            write_json(
                &mut host_write,
                &format!(
                    r#"{{"kind":"cancel","id":"h:cancel-{round}","targetId":"h:race-{round}"}}"#
                ),
            )
            .await;
        }
        // Closing the pipe ends the session once every round is written.
        drop(host_write);
    });

    let mut terminals: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    while let Some(frame) = try_read_frame(&mut host_read).await {
        if matches!(frame.kind(), FrameKind::Response | FrameKind::Error) {
            let id = frame
                .field("id")
                .and_then(JsonValue::as_str)
                .unwrap()
                .to_owned();
            *terminals.entry(id).or_default() += 1;
        }
    }
    writer.await.expect("writer");
    let doubled = terminals
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(id, count)| format!("{id} x{count}"))
        .collect::<Vec<_>>();
    assert!(
        doubled.is_empty(),
        "{} ids received a second terminal frame: {doubled:?}",
        doubled.len()
    );
    assert!(!terminals.is_empty(), "the rounds produced answers");
    // The session ended because the Host closed the pipe, not because the
    // Package's own writer refused a frame it should never have produced.
    let outcome = tokio::time::timeout(Duration::from_secs(20), service)
        .await
        .expect("the session ended")
        .expect("join");
    assert_eq!(outcome, Err(SdkError::ParentClosed));
}

/// Reads one frame, or `None` once the peer closes the pipe.
async fn try_read_frame(stream: &mut tokio::io::DuplexStream) -> Option<Frame> {
    use tokio::io::AsyncReadExt as _;
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await.ok()?;
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.ok()?;
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    teamarness_package_protocol::decode_frame(&bytes).ok()
}

/// CR-007: events the Host wrote before it could see the Package's own
/// `stream_close` arrive after the route is gone.
#[tokio::test]
async fn an_event_racing_the_packages_own_stream_close_keeps_the_session_alive() {
    let (mut host_read, mut host_write, service, mut host) = start();
    handshake(&mut host_read, &mut host_write, &mut host).await;
    let request = r#"{"kind":"request","id":"h:stream","operation":"fixture.capability_stream","deadlineUnixMs":"9999999999999","payload":{}}"#;
    host.observe_to_package(request);
    write_json(&mut host_write, request).await;

    let open = read_frame(&mut host_read).await;
    assert_eq!(open.kind(), FrameKind::StreamOpen);
    host.observe_from_package(&open);
    let stream = open
        .field("id")
        .and_then(JsonValue::as_str)
        .unwrap()
        .to_owned();

    let event = format!(
        r#"{{"kind":"event","streamId":"{stream}","sequence":"1","payload":{{"order":1}}}}"#
    );
    host.observe_to_package(&event);
    write_json(&mut host_write, &event).await;

    // Drain until the Package's own close, so the route is provably gone.
    loop {
        let frame = read_frame(&mut host_read).await;
        host.observe_from_package(&frame);
        if frame.kind() == FrameKind::StreamClose {
            break;
        }
    }
    // In reality this event was already in the pipe when the close was written.
    write_json(
        &mut host_write,
        &format!(
            r#"{{"kind":"event","streamId":"{stream}","sequence":"2","payload":{{"order":2}}}}"#
        ),
    )
    .await;

    let terminal = read_frame(&mut host_read).await;
    assert_eq!(terminal.kind(), FrameKind::Response);
    host.observe_from_package(&terminal);
    assert!(!service.is_finished(), "the session survives the race");
    drop(host_write);
    let _ = service.await;
}

/// CR-008: the Host always answers, and its answer can arrive after the
/// Package's own call deadline has passed.
#[tokio::test(start_paused = true)]
async fn a_capability_answer_after_the_call_deadline_keeps_the_session_alive() {
    let (mut host_read, mut host_write, service, mut host) = start();
    handshake(&mut host_read, &mut host_write, &mut host).await;
    let request = r#"{"kind":"request","id":"h:call","operation":"fixture.capability","deadlineUnixMs":"9999999999999","payload":{}}"#;
    host.observe_to_package(request);
    write_json(&mut host_write, request).await;

    let call = read_frame(&mut host_read).await;
    assert_eq!(call.kind(), FrameKind::Request);
    host.observe_from_package(&call);
    let id = call
        .field("id")
        .and_then(JsonValue::as_str)
        .unwrap()
        .to_owned();

    // The Package gives up one second after the Host's own deadline.
    tokio::time::advance(Duration::from_millis(11_001)).await;
    let terminal = read_frame(&mut host_read).await;
    assert_eq!(terminal.kind(), FrameKind::Error);
    host.observe_from_package(&terminal);

    // The Host's late answer, built before it could see the Package give up.
    write_json(
        &mut host_write,
        &format!(r#"{{"kind":"error","id":"{id}","code":"deadline_exceeded","retryable":true,"details":{{}}}}"#),
    )
    .await;
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    assert!(
        !service.is_finished(),
        "a late answer is ignored, not fatal"
    );
    drop(host_write);
    let _ = service.await;
}

/// A response for a call the Package never made is still fatal.
#[tokio::test]
async fn an_answer_for_a_call_that_never_existed_ends_the_session() {
    let (mut host_read, mut host_write, service, mut host) = start();
    handshake(&mut host_read, &mut host_write, &mut host).await;
    write_json(
        &mut host_write,
        r#"{"kind":"response","id":"p:never","payload":{}}"#,
    )
    .await;
    let result = tokio::time::timeout(Duration::from_secs(5), service)
        .await
        .expect("the session ended")
        .expect("join");
    assert_eq!(result, Err(SdkError::Protocol("unknown_response")));
    drop(host_write);
    drop(host_read);
    let _ = &mut host;
}
