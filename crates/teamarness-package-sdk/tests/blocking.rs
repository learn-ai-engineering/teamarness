use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{BackendError, CancellationToken, blocking};

fn empty() -> JsonValue {
    JsonValue::Object(Default::default())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_returns_the_closure_result() {
    let token = CancellationToken::new();
    let value = blocking(&token, |_| Ok(41 + 1)).await.unwrap();
    assert_eq!(value, 42);

    let error = blocking::<(), _>(&token, |_| {
        Err(BackendError::new("fixture.refused", false, empty()))
    })
    .await
    .unwrap_err();
    assert_eq!(error.code(), "fixture.refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_returns_cancelled_immediately_while_the_closure_observes_the_token() {
    let token = CancellationToken::new();
    let observed = Arc::new(AtomicBool::new(false));
    let flag = observed.clone();
    let work = blocking(&token, move |cancellation| {
        while !cancellation.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        flag.store(true, Ordering::SeqCst);
        Ok(())
    });
    let started = Instant::now();
    tokio::time::sleep(Duration::from_millis(30)).await;
    token.cancel();
    let error = work.await.unwrap_err();
    assert_eq!(error.code(), "cancelled");
    assert!(error.retryable());
    assert!(started.elapsed() < Duration::from_secs(1));

    let deadline = Instant::now() + Duration::from_secs(1);
    while !observed.load(Ordering::SeqCst) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(observed.load(Ordering::SeqCst), "the closure saw the token");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_maps_a_panic_to_blocking_task_failed() {
    let token = CancellationToken::new();
    let error = blocking::<(), _>(&token, |_| panic!("the Package panicked"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "package.blocking_task_failed");
    assert!(!error.retryable());
}
