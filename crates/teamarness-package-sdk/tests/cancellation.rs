use std::sync::atomic::Ordering;
use std::time::Duration;

use teamarness_package_sdk::CancellationToken;

#[tokio::test]
async fn cancelled_future_completes_for_cancel_before_and_after_await() {
    let before = CancellationToken::new();
    before.cancel();
    tokio::time::timeout(Duration::from_secs(1), before.cancelled())
        .await
        .expect("a token cancelled first completes at once");

    let after = CancellationToken::new();
    let waiting = after.clone();
    let task = tokio::spawn(async move { waiting.cancelled().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!after.is_cancelled());
    after.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("a token cancelled while awaited wakes its waiter")
        .unwrap();
}

#[tokio::test]
async fn child_tokens_follow_their_parent() {
    let parent = CancellationToken::new();
    let child = parent.child();
    let grandchild = child.child();
    parent.cancel();
    assert!(child.is_cancelled());
    assert!(grandchild.is_cancelled());

    // A child cancels on its own without touching its parent.
    let other = CancellationToken::new();
    let only_child = other.child();
    only_child.cancel();
    assert!(!other.is_cancelled());

    // A child taken from a token that is already cancelled starts cancelled.
    assert!(other.child().is_cancelled() == other.is_cancelled());
    other.cancel();
    assert!(other.child().is_cancelled());
}

#[tokio::test]
async fn interrupt_flag_is_shared() {
    let token = CancellationToken::new();
    let flag = token.interrupt_flag();
    assert!(!flag.load(Ordering::SeqCst));
    token.cancel();
    assert!(flag.load(Ordering::SeqCst));
    // A flag taken after cancellation reports the same state.
    assert!(token.interrupt_flag().load(Ordering::SeqCst));
}
