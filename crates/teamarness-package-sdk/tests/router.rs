//! The typed router: dispatch, payload typing, lifecycle state and SDK-owned codes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use teamarness_package_protocol::{FrameKind, JsonValue, MAX_IN_FLIGHT_EACH_DIRECTION};
use teamarness_package_sdk::{
    ActivationKind, BackendError, Empty, Package, Router, SdkError, event_channel,
};

mod support;

use support::{INITIALIZE, read_frame, write_json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Add {
    a: i64,
    b: i64,
}

#[derive(Debug, Serialize)]
struct Sum {
    sum: i64,
}

/// Counts every lifecycle hook, and gives each activation a distinguishable identity.
#[derive(Default)]
struct Hooks {
    activate: AtomicU64,
    suspend: AtomicU64,
    resume: AtomicU64,
    dispose: AtomicU64,
    recover_kind: AtomicBool,
}

struct State {
    id: u64,
}

fn counting_router(hooks: Arc<Hooks>) -> Router<State> {
    let activate = hooks.clone();
    let suspend = hooks.clone();
    let resume = hooks.clone();
    let dispose = hooks.clone();
    Router::new(move |activation| {
        let hooks = activate.clone();
        async move {
            if activation.kind() == ActivationKind::Recover {
                hooks.recover_kind.store(true, Ordering::SeqCst);
            }
            assert!(activation.data_directory().as_os_str().len() > 1);
            let id = hooks.activate.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(State { id })
        }
    })
    .on_suspend(move |_| {
        let hooks = suspend.clone();
        async move {
            hooks.suspend.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    })
    .on_resume(move |_| {
        let hooks = resume.clone();
        async move {
            hooks.resume.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    })
    .on_dispose(move |_| {
        let hooks = dispose.clone();
        async move {
            hooks.dispose.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    })
    .request("fixture.add", |_context, request: Add| async move {
        Ok(Sum {
            sum: request.a + request.b,
        })
    })
    .request("fixture.state", |context, _: Empty| async move {
        Ok(serde_json::json!({ "id": context.state().id }))
    })
    .request("fixture.not_object", |_context, _: Empty| async move {
        Ok(JsonValue::from(7))
    })
    .request("fixture.blocking", |context, _: Empty| async move {
        let flag = Arc::new(AtomicBool::new(false));
        let observed = flag.clone();
        let result = context
            .blocking(move |cancellation| {
                while !cancellation.is_cancelled() {
                    // simulated-delay: the fixture's own polling of the cancellation token.
                    std::thread::sleep(Duration::from_millis(5));
                }
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;
        let _ = observed;
        result.map(|()| serde_json::json!({}))
    })
    .subscription("fixture.forever", |_context, _: Empty| async move {
        let (sender, subscription) = event_channel(2);
        // The sender is parked rather than dropped, so the stream stays open
        // until the host closes it. A stream whose task has finished is not an
        // open stream, so a bound on open streams needs one that has not.
        tokio::spawn(async move {
            std::future::pending::<()>().await;
            drop(sender);
        });
        Ok(subscription)
    })
    .port_request(
        "com.example.messages",
        "message.post",
        "command",
        |_context, actor, _: Empty| async move {
            let identity = actor.identity().to_owned();
            Ok(serde_json::json!({"actor": identity}))
        },
    )
    .port_request(
        "com.example.messages",
        "message.title",
        "query",
        |_context, _actor, _: Empty| async move { Ok("hello".to_owned()) },
    )
    .port_request(
        "com.example.messages",
        "message.marker",
        "query",
        |_context, _actor, _: Empty| async move {
            Ok(serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"}))
        },
    )
    .port_event(
        "com.example.messages",
        "message.marker_changed",
        |_context, _actor| async move {
            let (sender, subscription) = event_channel(2);
            tokio::spawn(async move {
                let _ = sender.send(serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"})).await;
            });
            Ok(subscription)
        },
    )
    .port_event(
        "com.example.messages",
        "message.title_changed",
        |_context, _actor| async move {
            let (sender, subscription) = event_channel(2);
            tokio::spawn(async move {
                let _ = sender.send_port_value(serde_json::json!([1, 2])).await;
            });
            Ok(subscription)
        },
    )
    .port_event(
        "com.example.messages",
        "message.created",
        |_context, actor| async move {
            let (sender, subscription) = event_channel(2);
            let identity = actor.identity().to_owned();
            tokio::spawn(async move {
                let _ = sender.send(serde_json::json!({"actor":identity})).await;
            });
            Ok(subscription)
        },
    )
    .port_live_query(
        "com.example.messages",
        "message.watch",
        |_context, actor, _: Empty| async move {
            let (sender, subscription) = event_channel(2);
            let identity = actor.identity().to_owned();
            tokio::spawn(async move {
                let _ = sender.send(serde_json::json!({"actor":identity})).await;
            });
            Ok(subscription)
        },
    )
    .subscription("fixture.watch", |context, request: Add| async move {
        let (sender, subscription) = event_channel(2);
        let id = context.state().id;
        tokio::spawn(async move {
            let _ = sender
                .send(serde_json::json!({ "sum": request.a + request.b, "state": id }))
                .await;
        });
        Ok(subscription)
    })
}

struct Session {
    host_read: tokio::io::DuplexStream,
    host_write: tokio::io::DuplexStream,
    service: tokio::task::JoinHandle<Result<(), SdkError>>,
}

async fn start(router: Router<State>) -> Session {
    let (host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service =
        tokio::spawn(Package::from_backend(router.into_backend()).serve_io(sdk_read, sdk_write));
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

impl Session {
    async fn request(
        &mut self,
        id: &str,
        operation: &str,
        payload: &str,
    ) -> teamarness_package_protocol::Frame {
        write_json(
            &mut self.host_write,
            &format!(
                r#"{{"kind":"request","id":"{id}","operation":"{operation}","deadlineUnixMs":"99999999999999","payload":{payload}}}"#
            ),
        )
        .await;
        read_frame(&mut self.host_read).await
    }

    async fn finish(mut self) {
        write_json(
            &mut self.host_write,
            r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
        )
        .await;
        // A finished stream may still be closing; skip whatever precedes the acknowledgement.
        loop {
            if read_frame(&mut self.host_read).await.kind() == FrameKind::ShutdownAck {
                break;
            }
        }
        drop(self.host_write);
        self.service.await.unwrap().unwrap();
    }
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_port_router_preserves_typed_host_actor_and_stream_query() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let envelope = r#"{"contract":"com.example.messages","major":2,"minor":0,"port":"message.post","actor":{"kind":"person","identity":"installation1"},"request":{}}"#;
    let response = session
        .request("h:port", "teamarness.port.command.v1", envelope)
        .await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(
        response.field("payload").unwrap()["$teamarnessPortValue"]["actor"],
        "installation1"
    );
    let forged = session.request("h:forged", "teamarness.port.command.v1",
        r#"{"contract":"com.example.messages","major":1,"minor":0,"port":"message.post","actor":{"kind":"agent","identity":"someone"},"request":{}}"#).await;
    assert_eq!(forged.field("code").unwrap(), "package.invalid_payload");
    write_json(&mut session.host_write,
        r#"{"kind":"stream_open","id":"h:live","topic":"teamarness.port.query.v1","initialCredit":32,"payload":{"contract":"com.example.messages","major":2,"minor":0,"port":"message.watch","actor":{"kind":"person","identity":"installation1"},"request":{}}}"#).await;
    let event = read_frame(&mut session.host_read).await;
    assert_eq!(event.kind(), FrameKind::Event);
    assert_eq!(
        event.field("payload").unwrap()["$teamarnessPortValue"]["actor"],
        "installation1"
    );
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::StreamClose
    );
    write_json(&mut session.host_write,
        r#"{"kind":"stream_open","id":"h:event","topic":"teamarness.port.event.v1","initialCredit":32,"payload":{"contract":"com.example.messages","major":2,"minor":0,"port":"message.created","actor":{"kind":"person","identity":"installation1"},"request":{}}}"#).await;
    let event = read_frame(&mut session.host_read).await;
    assert_eq!(event.kind(), FrameKind::Event);
    assert_eq!(
        event.field("payload").unwrap()["$teamarnessPortValue"]["actor"],
        "installation1"
    );
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::StreamClose
    );
    let response = session
        .request("h:after-event", "fixture.add", r#"{"a":2,"b":3}"#)
        .await;
    assert_eq!(response.field("payload").unwrap()["sum"], 5);
    session.finish().await;
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_object_port_outputs_are_encoded_without_changing_ui_frames() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let response = session.request("h:title", "teamarness.port.query.v1",
        r#"{"contract":"com.example.messages","major":2,"minor":0,"port":"message.title","actor":{"kind":"person","identity":"installation1"},"request":{}}"#).await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(
        response.field("payload").unwrap()["$teamarnessPortEnvelope"],
        1
    );
    assert_eq!(
        response.field("payload").unwrap()["$teamarnessPortValue"],
        "hello"
    );
    write_json(&mut session.host_write,
        r#"{"kind":"stream_open","id":"h:title-event","topic":"teamarness.port.event.v1","initialCredit":32,"payload":{"contract":"com.example.messages","major":2,"minor":0,"port":"message.title_changed","actor":{"kind":"person","identity":"installation1"},"request":{}}}"#).await;
    let event = read_frame(&mut session.host_read).await;
    assert_eq!(event.kind(), FrameKind::Event);
    assert_eq!(
        event.field("payload").unwrap()["$teamarnessPortValue"],
        serde_json::json!([1, 2])
    );
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::StreamClose
    );
    let marker = session.request("h:marker", "teamarness.port.query.v1",
        r#"{"contract":"com.example.messages","major":2,"minor":0,"port":"message.marker","actor":{"kind":"person","identity":"installation1"},"request":{}}"#).await;
    assert_eq!(
        marker.field("payload").unwrap()["$teamarnessPortValue"],
        serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"})
    );
    write_json(&mut session.host_write,
        r#"{"kind":"stream_open","id":"h:marker-event","topic":"teamarness.port.event.v1","initialCredit":32,"payload":{"contract":"com.example.messages","major":2,"minor":0,"port":"message.marker_changed","actor":{"kind":"person","identity":"installation1"},"request":{}}}"#).await;
    let marker_event = read_frame(&mut session.host_read).await;
    assert_eq!(marker_event.kind(), FrameKind::Event);
    assert_eq!(
        marker_event.field("payload").unwrap()["$teamarnessPortValue"],
        serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"literal"})
    );
    assert_eq!(
        read_frame(&mut session.host_read).await.kind(),
        FrameKind::StreamClose
    );
    let ui = session
        .request("h:ui", "fixture.add", r#"{"a":2,"b":3}"#)
        .await;
    assert_eq!(ui.field("payload").unwrap()["sum"], 5);
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routes_typed_requests_and_serializes_responses() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let response = session
        .request("h:add", "fixture.add", r#"{"a":1,"b":2}"#)
        .await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(response.field("payload").unwrap().get("sum").unwrap(), 3);
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejects_unknown_fields_as_invalid_payload() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let error = session
        .request("h:add", "fixture.add", r#"{"a":1,"b":2,"c":3}"#)
        .await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "package.invalid_payload");
    assert_eq!(error.field("retryable").unwrap(), false);
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_operation_and_subscription_use_sdk_codes() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let error = session.request("h:absent", "fixture.absent", "{}").await;
    assert_eq!(error.field("code").unwrap(), "package.unknown_operation");
    assert_eq!(error.field("retryable").unwrap(), false);

    write_json(&mut session.host_write, r#"{"kind":"stream_open","id":"h:stream-1","topic":"fixture.absent","mountId":"bW91bnQx","initialCredit":32,"payload":{}}"#).await;
    let closed = read_frame(&mut session.host_read).await;
    assert_eq!(closed.kind(), FrameKind::StreamClose);
    assert_eq!(
        closed.field("code").unwrap(),
        "package.unknown_subscription"
    );
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_before_activation_are_not_active_and_retryable() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    let error = session
        .request("h:add", "fixture.add", r#"{"a":1,"b":2}"#)
        .await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "package.not_active");
    assert_eq!(error.field("retryable").unwrap(), true);
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activate_stores_state_and_recover_after_suspend_resumes_it() {
    let hooks = Arc::new(Hooks::default());
    let mut session = start(counting_router(hooks.clone())).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let before = session.request("h:state-1", "fixture.state", "{}").await;
    session
        .request("h:suspend", "lifecycle.suspend", "{}")
        .await;
    session
        .request("h:recover", "lifecycle.recover", "{}")
        .await;
    let after = session.request("h:state-2", "fixture.state", "{}").await;

    assert_eq!(hooks.activate.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.suspend.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.resume.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.dispose.load(Ordering::SeqCst), 0);
    assert_eq!(
        before.field("payload").unwrap().get("id"),
        after.field("payload").unwrap().get("id"),
        "recover keeps the same activation state"
    );
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recover_without_state_activates_with_recover_kind() {
    let hooks = Arc::new(Hooks::default());
    let mut session = start(counting_router(hooks.clone())).await;
    let response = session
        .request("h:recover", "lifecycle.recover", "{}")
        .await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(hooks.activate.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.resume.load(Ordering::SeqCst), 0);
    assert!(hooks.recover_kind.load(Ordering::SeqCst));
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activate_while_active_disposes_the_previous_state_first() {
    let hooks = Arc::new(Hooks::default());
    let mut session = start(counting_router(hooks.clone())).await;
    session
        .request("h:activate-1", "lifecycle.activate", "{}")
        .await;
    let first = session.request("h:state-1", "fixture.state", "{}").await;
    session
        .request("h:activate-2", "lifecycle.activate", "{}")
        .await;
    let second = session.request("h:state-2", "fixture.state", "{}").await;

    assert_eq!(hooks.activate.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.dispose.load(Ordering::SeqCst), 1);
    assert_ne!(
        first.field("payload").unwrap().get("id"),
        second.field("payload").unwrap().get("id")
    );
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispose_clears_state() {
    let hooks = Arc::new(Hooks::default());
    let mut session = start(counting_router(hooks.clone())).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    session
        .request("h:dispose", "lifecycle.dispose", "{}")
        .await;
    let error = session.request("h:state", "fixture.state", "{}").await;
    assert_eq!(error.field("code").unwrap(), "package.not_active");
    assert_eq!(hooks.dispose.load(Ordering::SeqCst), 1);
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activation_error_is_returned_and_state_stays_inactive() {
    let router: Router<State> = Router::new(|_| async {
        Err(BackendError::new(
            "fixture.activation_refused",
            false,
            JsonValue::Object(Default::default()),
        ))
    })
    .request(
        "fixture.state",
        |context: teamarness_package_sdk::Context<State>, _: Empty| async move {
            Ok(serde_json::json!({ "id": context.state().id }))
        },
    );
    let mut session = start(router).await;
    let error = session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "fixture.activation_refused");
    let after = session.request("h:state", "fixture.state", "{}").await;
    assert_eq!(after.field("code").unwrap(), "package.not_active");
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_object_response_is_invalid_response() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    let error = session
        .request("h:not-object", "fixture.not_object", "{}")
        .await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "invalid_response");
    session.finish().await;
}

#[test]
fn operations_lists_registered_names_sorted() {
    let operations = counting_router(Arc::new(Hooks::default())).operations();
    assert_eq!(
        operations.requests,
        [
            "fixture.add",
            "fixture.blocking",
            "fixture.not_object",
            "fixture.state"
        ]
    );
    assert_eq!(
        operations.subscriptions,
        ["fixture.forever", "fixture.watch"]
    );
}

#[test]
#[should_panic(expected = "registered twice")]
fn duplicate_registration_panics() {
    let _ = Router::stateless()
        .request("fixture.read", |_, _: Empty| async { Ok(Empty {}) })
        .request("fixture.read", |_, _: Empty| async { Ok(Empty {}) });
}

#[test]
#[should_panic(expected = "lifecycle name")]
fn lifecycle_registration_panics() {
    let _ = Router::stateless().request("lifecycle.activate", |_, _: Empty| async { Ok(Empty {}) });
}

#[test]
#[should_panic(expected = "operation grammar")]
fn invalid_operation_name_panics() {
    let _ = Router::stateless().request("Fixture.Read", |_, _: Empty| async { Ok(Empty {}) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn context_blocking_observes_request_cancellation() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    write_json(&mut session.host_write, r#"{"kind":"request","id":"h:blocking","operation":"fixture.blocking","deadlineUnixMs":"99999999999999","payload":{}}"#).await;
    // settle: lets the request reach the router before it is cancelled.
    tokio::time::sleep(Duration::from_millis(50)).await;
    write_json(
        &mut session.host_write,
        r#"{"kind":"cancel","id":"h:cancel-1","targetId":"h:blocking"}"#,
    )
    .await;
    let error = read_frame(&mut session.host_read).await;
    assert_eq!(error.kind(), FrameKind::Error);
    assert_eq!(error.field("code").unwrap(), "cancelled");
    session.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_handlers_receive_typed_requests_and_stream_events() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    write_json(&mut session.host_write, r#"{"kind":"stream_open","id":"h:stream-1","topic":"fixture.watch","mountId":"bW91bnQx","initialCredit":32,"payload":{"a":2,"b":5}}"#).await;
    let event = read_frame(&mut session.host_read).await;
    assert_eq!(event.kind(), FrameKind::Event);
    assert_eq!(event.field("payload").unwrap().get("sum").unwrap(), 7);
    assert_eq!(event.field("payload").unwrap().get("state").unwrap(), 1);
    session.finish().await;
}

/// A stream past the SDK's own bound ends that stream, not the session.
///
/// The contract publishes no concurrent-stream limit and its fatal list names
/// a 33rd in-flight *request*, so ending the generation on an adapter-chosen
/// stream bound is what ADR 0019 §3 forbids in terms. The 33rd stream is
/// answered with `stream_close` carrying `overloaded`, which the SDK's own
/// table calls retryable, and the session keeps answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_past_the_open_bound_is_refused_without_ending_the_session() {
    let mut session = start(counting_router(Arc::new(Hooks::default()))).await;
    session
        .request("h:activate", "lifecycle.activate", "{}")
        .await;
    for index in 1..=MAX_IN_FLIGHT_EACH_DIRECTION {
        write_json(
            &mut session.host_write,
            &format!(
                r#"{{"kind":"stream_open","id":"h:forever-{index}","topic":"fixture.forever","mountId":"bW91bnQx","initialCredit":32,"payload":{{}}}}"#
            ),
        )
        .await;
    }
    write_json(
        &mut session.host_write,
        r#"{"kind":"stream_open","id":"h:one-too-many","topic":"fixture.forever","mountId":"bW91bnQx","initialCredit":32,"payload":{}}"#,
    )
    .await;

    let closed = read_frame(&mut session.host_read).await;
    assert_eq!(closed.kind(), FrameKind::StreamClose);
    assert_eq!(closed.field("streamId").unwrap(), "h:one-too-many");
    assert_eq!(closed.field("code").unwrap(), "overloaded");

    let answer = session
        .request("h:still-here", "fixture.add", r#"{"a":1,"b":2}"#)
        .await;
    assert_eq!(answer.kind(), FrameKind::Response);
    session.finish().await;
}
