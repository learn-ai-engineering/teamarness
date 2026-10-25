//! Per-Instance ordering of actor/binding Core work against Backend replacement.
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;

use crate::PackageErrorCode;
use crate::capability::CoreCallLimiter;

#[derive(Clone, Default)]
pub(crate) struct AuthorityGeneration(Arc<RwLock<u64>>);

#[derive(Clone)]
pub(crate) struct GenerationAuthority {
    generation: AuthorityGeneration,
    expected: u64,
}

impl AuthorityGeneration {
    /// Never bypass outstanding Core work on timeout. The write lease is acquired before
    /// initializing a replacement Backend, including replacements with reset process counters.
    pub(crate) async fn advance(&self) -> Result<GenerationAuthority, PackageErrorCode> {
        let mut epoch = self.0.write().await;
        *epoch = epoch
            .checked_add(1)
            .ok_or(PackageErrorCode::CapabilityUnavailable)?;
        Ok(GenerationAuthority {
            generation: self.clone(),
            expected: *epoch,
        })
    }
}

impl GenerationAuthority {
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        limiter: &CoreCallLimiter,
        started: Arc<AtomicBool>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PackageErrorCode> {
        let lease = self.generation.0.clone().read_owned().await;
        if *lease != self.expected {
            return Err(PackageErrorCode::Cancelled);
        }
        limiter
            .run(started, move || {
                // Like the limiter permit, this lease belongs to actual Core work, not its
                // async waiter. Dropping/aborting that waiter cannot admit a replacement early.
                let _lease = lease;
                work()
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queued_old_lease_validates_after_generation_advances() {
        use std::future::Future;
        use std::task::Poll;
        let gate = AuthorityGeneration::default();
        let old = gate.advance().await.unwrap();
        let held = gate.0.read().await;
        let mut advancement = std::pin::pin!(gate.advance());
        // Poll once: the replacement writer is now queued behind held Core work.
        std::future::poll_fn(|cx| {
            assert!(advancement.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let queued = Arc::new(tokio::sync::Notify::new());
        let work_queued = queued.clone();
        let old_work = tokio::spawn(async move {
            work_queued.notify_one();
            old.run(
                &CoreCallLimiter::new(),
                Arc::new(AtomicBool::new(false)),
                || panic!("queued old work reached Core after replacement"),
            )
            .await
        });
        queued.notified().await;
        // An unrelated Instance has its own gate and remains responsive.
        AuthorityGeneration::default().advance().await.unwrap();
        drop(held);
        advancement.await.unwrap();
        assert_eq!(
            old_work.await.unwrap().unwrap_err(),
            PackageErrorCode::Cancelled
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn running_core_work_fences_replacement_after_waiter_abort() {
        let gate = AuthorityGeneration::default();
        let old = gate.advance().await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let committed = Arc::new(AtomicBool::new(false));
        let work_entered = entered.clone();
        let work_release = release.clone();
        let work_committed = committed.clone();
        let task = tokio::spawn(async move {
            old.run(
                &CoreCallLimiter::new(),
                Arc::new(AtomicBool::new(false)),
                move || {
                    work_entered.notify_one();
                    let (lock, notify) = &*work_release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = notify.wait(released).unwrap();
                    }
                    work_committed.store(true, Ordering::Release);
                },
            )
            .await
        });
        entered.notified().await;
        task.abort();
        let _ = task.await;
        let replacement_gate = gate.clone();
        let mut replacement = tokio::spawn(async move { replacement_gate.advance().await });
        let overtook = tokio::time::timeout(Duration::from_millis(100), &mut replacement)
            .await
            .is_ok();
        {
            let (lock, notify) = &*release;
            *lock.lock().unwrap() = true;
            notify.notify_one();
        }
        assert!(!overtook, "replacement overtook a still-running Core write");
        replacement.await.unwrap().unwrap();
        assert!(committed.load(Ordering::Acquire));
    }
}
