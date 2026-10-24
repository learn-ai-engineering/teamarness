//! The reply harness seam.
//!
//! Phase 4 ships the task slot, its start and stop discipline, and the inputs the loop needs; the
//! model client and the Channel loop land in the next phase. The stub task does nothing but wait
//! for its cancellation, so suspend, resume and dispose already exercise the lifecycle it will
//! live under.

use std::sync::Arc;

use teamarness_package_sdk::CancellationToken;
use teamarness_package_sdk::capabilities::principals::Principals;
use teamarness_package_sdk::ports::PortClient;
use tokio::task::JoinHandle;

use crate::settings::SettingsStore;
use crate::status::StatusCell;

/// The Conversations Port Contract this Package consumes.
pub const CONVERSATIONS_CONTRACT: &str = "dev.teamarness.conversations";

/// What the harness loop is given when it starts.
///
/// The settings store is read on each pass, so a saved change applies to the next reply; the key
/// is read from the store only while a request is being built, through `SettingsStore::api_key`.
#[derive(Clone)]
pub struct HarnessInputs {
    pub ports: PortClient,
    pub principals: Principals,
    pub settings: Arc<std::sync::Mutex<SettingsStore>>,
    pub status: Arc<StatusCell>,
}

/// One running background task and the token that stops it.
pub struct TaskHandle {
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl TaskHandle {
    /// Runs `work` under a fresh child of `parent`, so cancelling the parent stops it too.
    pub fn spawn<F, Fut>(parent: &CancellationToken, work: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cancellation = parent.child();
        let task = tokio::spawn(work(cancellation.clone()));
        Self { cancellation, task }
    }

    /// Cancels the task and waits for it to finish; a task that ignores the token is aborted.
    pub async fn stop(self) {
        self.cancellation.cancel();
        let abort = self.task.abort_handle();
        if tokio::time::timeout(std::time::Duration::from_secs(2), self.task)
            .await
            .is_err()
        {
            abort.abort();
        }
    }
}

/// Starts the reply harness. Phase 4 stub: waits for cancellation and never calls a Port.
///
/// The next phase replaces the body with: subscribe `messages.changed`; on start and on each
/// coalesced event run `channel.list` then `message.page` from the persisted high-water mark;
/// detect a mention of the Agent's current name; one in-flight reply per Channel; advance the
/// mark before `message.post`; map endpoint failures to `status.set_error`.
pub fn start(parent: &CancellationToken, inputs: HarnessInputs) -> TaskHandle {
    TaskHandle::spawn(parent, move |cancellation| async move {
        let _inputs = inputs;
        cancellation.cancelled().await;
    })
}
