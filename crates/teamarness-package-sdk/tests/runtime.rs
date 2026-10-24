//! The SDK runtime: cancellation, deadlines, lifecycle release and the isolated control plane.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use teamarness_package_protocol::{FrameKind, JsonValue};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, CancellationToken, EventSubscription, Package, SdkError,
    port_observation::ProviderPortStage,
};
use tokio::sync::Mutex;

mod support;

use support::{INITIALIZE, read_frame, write_json};

fn empty() -> JsonValue {
    JsonValue::Object(Default::default())
}

fn deadline_in(duration: Duration) -> String {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        + duration)
        .as_millis()
        .to_string()
}

/// A Backend that exposes what the SDK runtime gave each request.
#[derive(Default)]
struct Fixture {
    cancellation_observed: Arc<AtomicBool>,
    order: Arc<Mutex<Vec<&'static str>>>,
    long_request: Arc<Mutex<Option<CancellationToken>>>,
}

impl Backend for Fixture {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                // Polls the token from blocking work, as a `gix`-style library would.
                "fixture.poll_cancellation" => {
                    let flag = self.cancellation_observed.clone();
                    let token = request.cancellation().clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        while !token.is_cancelled() {
                            // simulated-delay: the fixture's own polling of the cancellation token.
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        flag.store(true, Ordering::SeqCst);
                    })
                    .await;
                    Ok(empty())
                }
                "fixture.deadline" => {
                    request.cancellation().cancelled().await;
                    Err(BackendError::new("fixture.deadline", false, empty()))
                }
                "fixture.long" => {
                    *self.long_request.lock().await = Some(request.cancellation().clone());
                    request.cancellation().cancelled().await;
                    self.order.lock().await.push("long_cancelled");
                    Ok(empty())
                }
                "lifecycle.suspend" => {
                    // The dispatcher cancels in-flight work before the lifecycle handler runs, so
                    // by the time the suspend handler is entered the long request's token is
                    // already cancelled, whichever task the runtime polls first.
                    let cancelled = match self.long_request.lock().await.as_ref() {
                        Some(token) => token.is_cancelled(),
                        None => false,
                    };
                    self.order.lock().await.push(if cancelled {
                        "suspend_saw_long_cancelled"
                    } else {
                        "suspend_saw_long_running"
                    });
                    Ok(empty())
                }
                "fixture.block" => {
                    // simulated-delay: the fixture's blocking work, which is what the test is about.
                    std::thread::sleep(Duration::from_secs(3));
                    Ok(empty())
                }
                "fixture.init" => {
                    let initialization = &request.initialization;
                    let mut fields = serde_json::Map::new();
                    fields.insert(
                        "dataDirectory".into(),
                        JsonValue::String(
                            initialization.data_directory().path().display().to_string(),
                        ),
                    );
                    fields.insert(
                        "continuityId".into(),
                        JsonValue::String(initialization.data_directory().continuity_id().into()),
                    );
                    fields.insert(
                        "workspaceName".into(),
                        JsonValue::String(initialization.workspace().name.clone()),
                    );
                    fields.insert(
                        "installation".into(),
                        JsonValue::String(initialization.installation().display_name.clone()),
                    );
                    fields.insert(
                        "packageVersion".into(),
                        JsonValue::String(initialization.package().version.clone()),
                    );
                    fields.insert(
                        "generation".into(),
                        JsonValue::String(initialization.process_generation().into()),
                    );
                    fields.insert(
                        "grantMinor".into(),
                        JsonValue::from(
                            initialization
                                .capability("teamarness.membership")
                                .map_or(0, |grant| grant.minor),
                        ),
                    );
                    Ok(JsonValue::Object(fields))
                }
                _ => Ok(request.payload),
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async { Err(BackendError::new("unsupported_stream", false, empty())) })
    }
}

struct Session {
    host_read: tokio::io::DuplexStream,
    host_write: tokio::io::DuplexStream,
    service: tokio::task::JoinHandle<Result<(), SdkError>>,
}

async fn start(backend: Arc<Fixture>) -> Session {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(backend).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, INITIALIZE).await;
    let mut session = Session {
        host_read,
        host_write,
        service,
    };
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::Initialized
    );
    session
}

async fn shutdown(mut session: Session) {
    write_json(
        &mut session.host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(session.host_write);
    session.service.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn port_observations_are_opt_in_bounded_and_payload_free() {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let (package, mut observed) = Package::builder(Arc::new(Fixture::default())).observe_ports();
    let service = tokio::spawn(package.serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, INITIALIZE).await;
    let mut session = Session {
        host_read,
        host_write,
        service,
    };
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::Initialized
    );
    let sentinel = "secret-markdown-not-in-observations";
    write_json(&mut session.host_write, &format!(
        r#"{{"kind":"request","id":"h:port-1-9","operation":"teamarness.port.command.v1","deadlineUnixMs":"{}","payload":{{"markdown":"{sentinel}"}}}}"#, deadline_in(Duration::from_secs(8))
    )).await;
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::Response
    );
    let received = observed.recv().await.unwrap();
    let completed = observed.recv().await.unwrap();
    assert_eq!(received.provider_id, "h:port-1-9");
    assert_eq!(received.stage, ProviderPortStage::Received);
    assert_eq!(completed.stage, ProviderPortStage::Completed);
    assert!(!format!("{received:?}{completed:?}").contains(sentinel));
    assert!(observed.try_recv().is_err());
    write_json(&mut session.host_write,
        r#"{"kind":"stream_open","id":"h:port-event-1-10","topic":"teamarness.port.event.v1","initialCredit":32,"payload":{"markdown":"secret-stream"}}"#).await;
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::StreamClose
    );
    let failed = observed.recv().await.unwrap();
    assert_eq!(failed.provider_id, "h:port-event-1-10");
    assert_eq!(failed.stage, ProviderPortStage::Failed);
    assert_eq!(failed.code.as_deref(), Some("unsupported_stream"));
    assert!(!format!("{failed:?}").contains("secret-stream"));
    shutdown(session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_frame_cancels_the_request_token_before_aborting() {
    let backend = Arc::new(Fixture::default());
    let mut session = start(backend.clone()).await;
    write_json(&mut session.host_write, &format!(
        r#"{{"kind":"request","id":"h:poll","operation":"fixture.poll_cancellation","deadlineUnixMs":"{}","payload":{{}}}}"#,
        deadline_in(Duration::from_secs(60)),
    )).await;
    // settle: lets the request reach the Backend before it is cancelled.
    tokio::time::sleep(Duration::from_millis(50)).await;
    write_json(
        &mut session.host_write,
        r#"{"kind":"cancel","id":"h:cancel-1","targetId":"h:poll"}"#,
    )
    .await;
    let error = read_frame(&mut session.host_read).await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "cancelled");

    let deadline = Instant::now() + Duration::from_secs(1);
    while !backend.cancellation_observed.load(Ordering::SeqCst) && Instant::now() < deadline {
        // poll-interval: the interval of this loop, which is bounded by its own deadline.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        backend.cancellation_observed.load(Ordering::SeqCst),
        "the blocking closure saw the cancellation"
    );
    shutdown(session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_deadline_cancels_the_token() {
    let mut session = start(Arc::new(Fixture::default())).await;
    write_json(
        &mut session.host_write,
        &format!(
            r#"{{"kind":"request","id":"h:deadline","operation":"fixture.deadline","deadlineUnixMs":"{}","payload":{{}}}}"#,
            deadline_in(Duration::from_millis(200)),
        ),
    )
    .await;
    let frame = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut session.host_read))
        .await
        .expect("the deadline cancels the request token");
    assert_eq!(frame.kind(), FrameKind::Error);
    assert_eq!(frame.field("code").unwrap(), "fixture.deadline");
    shutdown(session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspend_cancels_in_flight_request_tokens_first() {
    let backend = Arc::new(Fixture::default());
    let mut session = start(backend.clone()).await;
    write_json(&mut session.host_write, &format!(
        r#"{{"kind":"request","id":"h:long","operation":"fixture.long","deadlineUnixMs":"{}","payload":{{}}}}"#,
        deadline_in(Duration::from_secs(60)),
    )).await;
    // settle: lets the long request start before suspension is asked for.
    tokio::time::sleep(Duration::from_millis(50)).await;
    write_json(&mut session.host_write, &format!(
        r#"{{"kind":"request","id":"h:suspend","operation":"lifecycle.suspend","deadlineUnixMs":"{}","payload":{{}}}}"#,
        deadline_in(Duration::from_secs(30)),
    )).await;
    for _ in 0..2 {
        assert!(matches!(
            read_frame(&mut session.host_read).await.kind(),
            FrameKind::Response | FrameKind::Error
        ));
    }
    let order = backend.order.lock().await.clone();
    assert!(
        order.contains(&"suspend_saw_long_cancelled"),
        "suspend ran after the in-flight token was cancelled: {order:?}"
    );
    assert!(order.contains(&"long_cancelled"), "{order:?}");
    shutdown(session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_is_answered_while_every_package_worker_is_blocked() {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, host_write) = tokio::io::duplex(64 * 1024);
    // The Package runtime is a single-threaded runtime on its own thread, so the blocking handler
    // below occupies every worker it has.
    let package_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(Package::builder(Arc::new(Fixture::default())).serve_io(sdk_read, sdk_write))
    });

    let mut host_read = host_read;
    let mut host_write = host_write;
    write_json(&mut host_write, INITIALIZE).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, &format!(
        r#"{{"kind":"request","id":"h:block","operation":"fixture.block","deadlineUnixMs":"{}","payload":{{}}}}"#,
        deadline_in(Duration::from_secs(60)),
    )).await;
    // settle: lets the blocking request start before the control plane is pinged.
    tokio::time::sleep(Duration::from_millis(100)).await;
    write_json(&mut host_write, r#"{"kind":"ping","nonce":"cGluZw"}"#).await;
    let pong = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut host_read))
        .await
        .expect("the control plane answers while the Package runtime is blocked");
    assert_eq!(pong.kind(), FrameKind::Pong);
    assert_eq!(pong.field("nonce").unwrap(), "cGluZw");

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
    let _ = tokio::task::spawn_blocking(move || package_thread.join()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialization_exposes_typed_values() {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service =
        tokio::spawn(Package::builder(Arc::new(Fixture::default())).serve_io(sdk_read, sdk_write));
    let initialize = INITIALIZE.replace(
        r#""capabilities":[]"#,
        r#""capabilities":[{"identity":"teamarness.membership","grant":"Z3JhbnQx","major":1,"minor":3}]"#,
    );
    write_json(&mut host_write, &initialize).await;
    let mut host_read = host_read;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );
    write_json(&mut host_write, &format!(
        r#"{{"kind":"request","id":"h:init","operation":"fixture.init","deadlineUnixMs":"{}","payload":{{}}}}"#,
        deadline_in(Duration::from_secs(60)),
    )).await;
    let response = read_frame(&mut host_read).await;
    assert_eq!(response.kind(), FrameKind::Response);
    let payload = response.field("payload").unwrap();
    assert_eq!(payload.get("dataDirectory").unwrap(), "/tmp/package");
    assert_eq!(payload.get("continuityId").unwrap(), "Y29udGludWl0eQ");
    assert_eq!(payload.get("workspaceName").unwrap(), "Workspace");
    assert_eq!(payload.get("installation").unwrap(), "Installation");
    assert_eq!(payload.get("packageVersion").unwrap(), "1.0.0");
    assert_eq!(payload.get("generation").unwrap(), "1");
    assert_eq!(payload.get("grantMinor").unwrap(), 3);

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

/// An `initialize` missing `dataDirectory.continuityId` ends the session before the Package sees
/// it. The protocol decoder rejects the frame first, so the session ends as `parent_closed` rather
/// than `malformed_frame`; either way no half-typed `Initialization` reaches the Package.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_initialize_ends_the_session_without_initializing() {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service =
        tokio::spawn(Package::builder(Arc::new(Fixture::default())).serve_io(sdk_read, sdk_write));
    // The reader stays alive so the writer's own failure cannot mask the handshake verdict.
    let _host_read = host_read;
    let initialize = INITIALIZE.replace(r#","continuityId":"Y29udGludWl0eQ""#, "");
    write_json(&mut host_write, &initialize).await;
    let error = service.await.unwrap().unwrap_err();
    assert!(
        matches!(
            error,
            SdkError::ParentClosed | SdkError::Protocol("malformed_frame")
        ),
        "{error:?}"
    );
}
