//! The protocol-free Backend test harness: a Package's own tests without a single frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::ports::PortClient;
use teamarness_package_sdk::testing::{
    HarnessBuilder, ScriptedCapability, assert_router_matches_contract,
};
use teamarness_package_sdk::{BackendError, Empty, Router, RouterOperations, event_channel};

const CAPABILITY: &str = "dev.teamarness.fixture-capability";

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

/// The directory the Package is given, so a test proves the Backend really sees it.
struct Fixture {
    data_directory: std::path::PathBuf,
}

fn empty() -> JsonValue {
    JsonValue::Object(serde_json::Map::new())
}

fn fixture_router() -> Router<Fixture> {
    Router::new(|activation| {
        let data_directory = activation.data_directory().to_path_buf();
        async move { Ok(Fixture { data_directory }) }
    })
    .request("fixture.add", |_context, request: Add| async move {
        Ok(Sum {
            sum: request.a + request.b,
        })
    })
    .request("fixture.where", |context, _: Empty| async move {
        Ok(serde_json::json!({
            "dataDirectory": context.state().data_directory.to_string_lossy(),
        }))
    })
    .request("fixture.fail", |_context, _: Empty| async move {
        Err::<JsonValue, _>(BackendError::new(
            "fixture.rejected",
            true,
            serde_json::json!({ "reason": "scripted" }),
        ))
    })
    .request("fixture.forever", |context, _: Empty| async move {
        context.cancellation().cancelled().await;
        Ok(serde_json::json!({ "finished": true }))
    })
    .subscription("fixture.watch", |_context, _: Empty| async move {
        let (sender, subscription) = event_channel(4);
        tokio::spawn(async move {
            for step in 1..=3_i64 {
                if sender
                    .send(serde_json::json!({ "step": step }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(subscription)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_activates_with_a_real_data_directory_and_round_trips_requests() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(fixture_router().into_backend())
        .data_directory(directory.path())
        .start()
        .await;

    harness.activate().await.unwrap();
    let response = harness
        .request("fixture.add", serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .unwrap();
    assert_eq!(response.get("sum").unwrap(), 3);

    let seen = harness.request("fixture.where", empty()).await.unwrap();
    assert_eq!(
        seen.get("dataDirectory").unwrap().as_str().unwrap(),
        directory.path().to_string_lossy()
    );

    harness.dispose().await.unwrap();
    let after = harness
        .request("fixture.add", serde_json::json!({ "a": 1, "b": 2 }))
        .await;
    assert_eq!(after.unwrap_err().code(), "package.not_active");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_preserves_package_error_code_retryable_and_details() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(fixture_router().into_backend())
        .data_directory(directory.path())
        .start()
        .await;
    harness.activate().await.unwrap();

    let error = harness
        .request("fixture.fail", empty())
        .await
        .expect_err("the Backend refused");
    assert_eq!(error.code(), "fixture.rejected");
    assert!(error.retryable());
    assert_eq!(error.details().get("reason").unwrap(), "scripted");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_request_cancel_returns_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(fixture_router().into_backend())
        .data_directory(directory.path())
        .start()
        .await;
    harness.activate().await.unwrap();

    let pending = harness.start_request("fixture.forever", empty()).await;
    pending.cancel().await;
    let error = pending.result().await.expect_err("a cancelled request");
    assert_eq!(error.code(), "cancelled");
    assert!(error.retryable());
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_events_arrive_in_order_and_close_is_observed() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(fixture_router().into_backend())
        .data_directory(directory.path())
        .start()
        .await;
    harness.activate().await.unwrap();

    let mut subscription = harness.subscribe("fixture.watch", empty()).await.unwrap();
    for step in 1..=3_i64 {
        let event = subscription.next_event().await.unwrap().unwrap();
        assert_eq!(event.get("step").unwrap(), step);
    }
    assert!(
        subscription.next_event().await.is_none(),
        "the completed stream ends without an error"
    );
    harness.shutdown().await.unwrap();
}

/// A Backend that calls one capability, then consumes 33 stream events from it.
fn capability_router(gate: Arc<tokio::sync::Semaphore>) -> Router<()> {
    Router::stateless().request("fixture.capability", move |context, _: Empty| {
        let gate = gate.clone();
        async move {
            let grant = context
                .initialization()
                .capability(CAPABILITY)
                .ok_or_else(|| BackendError::new("fixture.no_grant", false, empty()))?
                .grant
                .clone();
            let greeting = context
                .capabilities()
                .call(&grant, "get", empty())
                .await?
                .get("value")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_owned();
            let mut subscription = context.capabilities().subscribe(&grant, empty()).await?;
            let mut received = 0_u64;
            while received < 33 {
                gate.acquire()
                    .await
                    .map_err(|_| BackendError::new("fixture.gate_closed", false, empty()))?
                    .forget();
                match subscription.recv().await {
                    Some(Ok(_)) => received += 1,
                    Some(Err(error)) => return Err(error),
                    None => break,
                }
            }
            subscription.close().await;
            Ok(serde_json::json!({ "greeting": greeting, "received": received }))
        }
    })
}

// Verifies: TEAM.SYS.PKG-R13, TEAM.SYS.PKG-R14
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_port_client_uses_exact_grant_live_query_and_event_subscription() {
    let directory = tempfile::tempdir().unwrap();
    let scripted = ScriptedCapability::new().calls(|method, payload| match method {
        "available" => {
            assert_eq!(payload, empty());
            Ok(serde_json::json!({"contracts":[]}))
        }
        "invoke" if payload["port"] == "message.title" => {
            Ok(serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"hello"}))
        }
        "invoke" => {
            assert_eq!(payload["contract"], "com.example.messages");
            assert_eq!(payload["port"], "message.post");
            assert_eq!(payload["kind"], "command");
            assert_eq!(payload["request"], serde_json::json!({"text":"hello"}));
            Ok(serde_json::json!({"accepted":true}))
        }
        _ => panic!("unexpected method {method}"),
    });
    let router = Router::stateless().request("fixture.ports", |context, _: Empty| async move {
        let grant = context
            .initialization()
            .capability("teamarness.package-ports")
            .unwrap()
            .grant
            .clone();
        let client = PortClient::new(context.capabilities().clone(), grant);
        let available = client.available().await?;
        let result = client
            .command(
                "com.example.messages",
                "message.post",
                serde_json::json!({"text":"hello"}),
            )
            .await?;
        let title = client.query("com.example.messages", "message.title", empty()).await?;
        let mut query = client
            .live_query("com.example.messages", "message.watch", empty())
            .await?;
        let event = query.recv().await.unwrap()?;
        query.close().await;
        let mut events = client.subscribe_event("com.example.messages", "message.created").await?;
        let created = events.recv().await.unwrap()?;
        events.close().await;
        Ok(serde_json::json!({"available":available,"result":result,"title":title,"event":event,"created":created}))
    });
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .capability("teamarness.package-ports", scripted.clone())
        .start()
        .await;
    harness.activate().await.unwrap();
    let pending = harness.start_request("fixture.ports", empty()).await;
    let sender = scripted.stream();
    assert!(
        sender
            .send(serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":[1,2]}))
            .await
    );
    for _ in 0..200 {
        if scripted.recorded_opens().len() == 2 {
            break;
        }
        // poll-interval: bounded by 200 attempts while the explicit event open is recorded.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(scripted.recorded_opens().len(), 2);
    assert!(
        sender
            .send(serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":"changed"}))
            .await
    );
    let answer = pending.result().await.unwrap();
    assert_eq!(answer["result"]["accepted"], true);
    assert_eq!(answer["title"], "hello");
    assert_eq!(answer["event"], serde_json::json!([1, 2]));
    assert_eq!(answer["created"], "changed");
    assert_eq!(scripted.recorded_calls().len(), 3);
    assert_eq!(scripted.recorded_opens()[1]["kind"], "event");
    assert_eq!(scripted.recorded_opens()[1]["port"], "message.created");
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_capability_answers_calls_and_streams_with_credit() {
    let directory = tempfile::tempdir().unwrap();
    let scripted = ScriptedCapability::new().calls(|method, payload| {
        assert_eq!(method, "get");
        assert_eq!(payload, empty());
        Ok(serde_json::json!({ "value": "hello" }))
    });
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut harness = HarnessBuilder::new(capability_router(gate.clone()).into_backend())
        .data_directory(directory.path())
        .capability(CAPABILITY, scripted.clone())
        .start()
        .await;
    harness.activate().await.unwrap();

    let pending = harness.start_request("fixture.capability", empty()).await;

    let sender = scripted.stream();
    let sent = Arc::new(AtomicUsize::new(0));
    let feeding = {
        let sent = sent.clone();
        tokio::spawn(async move {
            for step in 1..=33_i64 {
                assert!(sender.send(serde_json::json!({ "step": step })).await);
                sent.store(usize::try_from(step).unwrap(), Ordering::SeqCst);
            }
        })
    };

    // The Package opened the stream with 32 credits and has consumed nothing, so the 33rd event
    // cannot leave until the Backend's first `recv` returns a credit.
    for _ in 0..200 {
        if sent.load(Ordering::SeqCst) == 32 {
            break;
        }
        // poll-interval: the interval of this loop, which is bounded by its own count.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(sent.load(Ordering::SeqCst), 32);
    // negative-wait: a bounded window proving the 33rd event waits for credit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        sent.load(Ordering::SeqCst),
        32,
        "the scripted stream waits for credit before the 33rd event"
    );

    gate.add_permits(33);
    feeding.await.unwrap();
    let response = pending.result().await.unwrap();
    assert_eq!(response.get("greeting").unwrap(), "hello");
    assert_eq!(response.get("received").unwrap(), 33);
    assert_eq!(scripted.recorded_calls().len(), 1);
    assert_eq!(scripted.recorded_calls()[0].method, "get");
    assert_eq!(scripted.recorded_opens().len(), 1);
    harness.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_ends_the_session() {
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(fixture_router().into_backend())
        .data_directory(directory.path())
        .start()
        .await;
    harness.activate().await.unwrap();
    assert!(harness.ping().await < Duration::from_secs(5));
    harness.shutdown().await.unwrap();
}

fn declared_operations() -> RouterOperations {
    RouterOperations {
        requests: vec!["x.a".to_owned()],
        subscriptions: vec!["y.a".to_owned()],
    }
}

#[test]
fn assert_router_matches_contract_accepts_an_exact_match() {
    assert_router_matches_contract(
        &declared_operations(),
        r#"{"requests":{"x.a":{}},"subscriptions":{"y.a":{}}}"#,
    );
}

#[test]
#[should_panic(expected = "declared without handler: [\"x.b\"]")]
fn assert_router_matches_contract_reports_both_differences() {
    assert_router_matches_contract(
        &declared_operations(),
        r#"{"requests":{"x.b":{}},"subscriptions":{"y.a":{}}}"#,
    );
}
