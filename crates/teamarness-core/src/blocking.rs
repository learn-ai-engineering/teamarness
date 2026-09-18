//! The one boundary between asynchronous code and Core runtime state.
//!
//! Core keeps a synchronous state lock and a synchronous command API. SQLite
//! and cryptographic work are blocking, so they run on blocking threads and
//! never on a Tokio worker. Four rules hold everywhere:
//!
//! 1. Asynchronous code never calls [`Mutex::lock`] or `try_lock` on
//!    [`RuntimeState`], and never calls a synchronous `CoreRuntime` method that
//!    does. It uses [`run_with_state`], [`receiver_call`], or `spawn_blocking`
//!    around the public API.
//! 2. Work inside one lock hold is bounded: one local append, one page of
//!    subscription records, one page of log rows, or one batch of remote
//!    operations.
//! 3. No `MutexGuard` crosses an `.await`.
//! 4. [`SyncReceiver`](crate::synchronization::SyncReceiver) stays a
//!    synchronous trait, so fakes stay easy; network code calls it only through
//!    [`receiver_call`].

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::runtime::RuntimeState;
use crate::synchronization::SyncReceiver;

/// Why runtime state could not be reached at all.
///
/// Every variant means the caller's work did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateUnavailable {
    /// The runtime has been dropped.
    Dropped,
    /// The state lock, or the state itself, is poisoned by an uncertain write.
    Poisoned,
    /// The blocking thread did not finish, so the outcome is unknown.
    Join,
}

/// Why a deadline-bounded lock attempt gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockUntilError {
    Timeout,
    Poisoned,
}

/// Runs `f` with the runtime state on a blocking thread.
///
/// Never call this from inside another `run_with_state`: the inner call would
/// wait for a lock the outer call holds.
pub(crate) async fn run_with_state<T, F>(
    state: &Weak<Mutex<RuntimeState>>,
    f: F,
) -> Result<T, StateUnavailable>
where
    T: Send + 'static,
    F: FnOnce(&mut RuntimeState) -> T + Send + 'static,
{
    let state = state.upgrade().ok_or(StateUnavailable::Dropped)?;
    tokio::task::spawn_blocking(move || {
        let mut guard = state.lock().map_err(|_| StateUnavailable::Poisoned)?;
        if guard.poisoned {
            return Err(StateUnavailable::Poisoned);
        }
        Ok(f(&mut guard))
    })
    .await
    .map_err(|_| StateUnavailable::Join)?
}

/// Takes the state lock on the calling (blocking) thread, giving up at `deadline`.
///
/// This is the onboarding shape: a deadline-bounded claim on state that another
/// thread may be holding. Waiting sleeps on the blocking thread, from 1 ms
/// doubling to 10 ms, so it never occupies an asynchronous worker.
pub(crate) fn lock_until<T>(
    state: &Mutex<T>,
    deadline: std::time::Instant,
) -> Result<MutexGuard<'_, T>, LockUntilError> {
    let mut backoff = std::time::Duration::from_millis(1);
    let ceiling = std::time::Duration::from_millis(10);
    loop {
        match state.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(LockUntilError::Poisoned),
            Err(std::sync::TryLockError::WouldBlock) => {
                let now = std::time::Instant::now();
                let Some(remaining) = deadline.checked_duration_since(now) else {
                    return Err(LockUntilError::Timeout);
                };
                std::thread::sleep(backoff.min(remaining));
                backoff = (backoff * 2).min(ceiling);
            }
        }
    }
}

/// Calls the synchronous [`SyncReceiver`] seam from asynchronous code.
///
/// The receiver takes the runtime state lock itself, so the call runs on a
/// blocking thread.
pub(crate) async fn receiver_call<T, F>(
    receiver: &Arc<dyn SyncReceiver>,
    f: F,
) -> Result<T, StateUnavailable>
where
    T: Send + 'static,
    F: FnOnce(&dyn SyncReceiver) -> T + Send + 'static,
{
    let receiver = Arc::clone(receiver);
    tokio::task::spawn_blocking(move || f(receiver.as_ref()))
        .await
        .map_err(|_| StateUnavailable::Join)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn hold<T>(lock: Arc<Mutex<T>>) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>)
    where
        T: Send + 'static,
    {
        let (acquired_sender, acquired) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let _guard = lock.lock().unwrap();
            acquired_sender.send(()).unwrap();
            let _ = release_receiver.recv();
        });
        acquired.recv().unwrap();
        (release, thread)
    }

    #[test]
    fn lock_until_times_out_while_another_thread_holds_the_lock() {
        let lock = Arc::new(Mutex::new(0_u32));
        let (release, thread) = hold(Arc::clone(&lock));
        let started = Instant::now();
        let outcome = lock_until(&lock, started + Duration::from_millis(50));
        let elapsed = started.elapsed();
        drop(release);
        thread.join().unwrap();
        assert_eq!(outcome.err(), Some(LockUntilError::Timeout));
        assert!(elapsed >= Duration::from_millis(50), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn lock_until_acquires_after_release() {
        let lock = Arc::new(Mutex::new(7_u32));
        let (release, thread) = hold(Arc::clone(&lock));
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(release);
        });
        let guard = lock_until(&lock, Instant::now() + Duration::from_secs(5))
            .expect("the lock is released well inside the deadline");
        assert_eq!(*guard, 7);
        drop(guard);
        releaser.join().unwrap();
        thread.join().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_with_state_reports_dropped_runtime() {
        let state: Weak<Mutex<RuntimeState>> = Weak::new();
        let outcome = run_with_state(&state, |_| ()).await;
        assert_eq!(outcome.err(), Some(StateUnavailable::Dropped));
    }
}
