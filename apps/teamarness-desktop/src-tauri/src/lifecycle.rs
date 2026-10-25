//! One terminal owner. Admission, intent selection and candidate transfer share one lock.
use serde::Serialize;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(crate) type Install = Box<dyn FnOnce() -> Result<(), InstallFailure> + Send>;
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Running,
    Stopping,
    Installing,
    Restarting,
    Stopped,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCategory {
    Shutdown,
    ShutdownTimeout,
    Install,
    Uncertain,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShutdownErrors {
    pub host: bool,
    pub core: bool,
    pub uncertain: bool,
}
impl ShutdownErrors {
    fn failed(&self) -> bool {
        self.host || self.core || self.uncertain
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub revision: u64,
    pub phase: Phase,
    pub failure: Option<FailureCategory>,
    pub shutdown: ShutdownErrors,
    // Only the local main Shell may display this; never include it in diagnostics.
    pub previous_app: Option<PathBuf>,
    pub guidance: Option<&'static str>,
}
pub(crate) struct InstallFailure {
    pub previous_app: Option<PathBuf>,
}
pub(crate) trait TerminalAdapter: Send + Sync + 'static {
    fn stop_updates(&self);
    fn cleanup(&self) -> BoxFuture<'_, ShutdownErrors>;
    fn install(&self, pending: Install) -> Result<(), InstallFailure>;
    fn restart(&self);
    fn exit(&self);
}
#[derive(Clone)]
enum Intent {
    Quit,
    Apply(String),
}
struct Inner {
    status: Status,
    intent: Option<Intent>,
    active: usize,
    forwarders: usize,
    finished: bool,
    recovery_quit: bool,
    exit_allowed: bool,
}
struct Shared {
    runtime: tokio::runtime::Handle,
    inner: Mutex<Inner>,
    changes: watch::Sender<u64>,
    cancel: watch::Sender<bool>,
}
#[derive(Clone)]
pub(crate) struct Lifecycle(Arc<Shared>);
pub(crate) struct Lease {
    lifecycle: Lifecycle,
    forwarding: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut inner = self.lifecycle.0.inner.lock().unwrap();
        if self.forwarding {
            inner.forwarders -= 1;
        } else {
            inner.active -= 1;
        }
        self.lifecycle.signal(&mut inner);
    }
}
impl Lifecycle {
    pub(crate) fn new() -> Self {
        Self::on_runtime(tauri::async_runtime::handle().inner().clone())
    }
    // Retain the native runtime explicitly; synchronous callbacks have no entered runtime.
    // A private handle also keeps paused-clock tests on their owning executor.
    fn on_runtime(runtime: tokio::runtime::Handle) -> Self {
        Self(Arc::new(Shared {
            runtime,
            inner: Mutex::new(Inner {
                status: Status {
                    revision: 0,
                    phase: Phase::Running,
                    failure: None,
                    shutdown: ShutdownErrors::default(),
                    previous_app: None,
                    guidance: None,
                },
                intent: None,
                active: 0,
                forwarders: 0,
                finished: false,
                recovery_quit: false,
                exit_allowed: false,
            }),
            changes: watch::channel(0).0,
            cancel: watch::channel(false).0,
        }))
    }
    fn signal(&self, inner: &mut Inner) {
        inner.status.revision += 1;
        self.0.changes.send_replace(inner.status.revision);
    }
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changes.subscribe()
    }
    pub(crate) fn status(&self) -> Status {
        self.0.inner.lock().unwrap().status.clone()
    }
    pub(crate) fn exit_allowed(&self) -> bool {
        self.0.inner.lock().unwrap().exit_allowed
    }
    pub(crate) fn admit(&self) -> Result<Lease, ()> {
        let mut inner = self.0.inner.lock().unwrap();
        if inner.intent.is_some() {
            return Err(());
        }
        inner.active += 1;
        Ok(Lease {
            lifecycle: self.clone(),
            forwarding: false,
        })
    }
    // Called only by an already admitted invocation. Count separately: idle waits
    // carry no ordinary lease, but their cancellation and in-flight sends are joined.
    pub(crate) fn forwarder(&self) -> Lease {
        self.0.inner.lock().unwrap().forwarders += 1;
        Lease {
            lifecycle: self.clone(),
            forwarding: true,
        }
    }
    // Only an admitted invocation/forwarder may fork a blocking operation.
    // Its lease lives inside the OS-thread closure, not its async JoinHandle.
    pub(crate) fn blocking_work(&self) -> Lease {
        self.0.inner.lock().unwrap().active += 1;
        Lease {
            lifecycle: self.clone(),
            forwarding: false,
        }
    }
    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.0.cancel.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
    pub(crate) async fn dispatch<F, T>(&self, operation: F) -> Result<T, ()>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let lease = self.admit()?;
        tokio::spawn(async move {
            let _lease = lease;
            operation.await
        })
        .await
        .map_err(|_| ())
    }
    pub(crate) fn apply(
        &self,
        generation: &str,
        acquire: impl FnOnce() -> Result<Install, ()>,
        adapter: Arc<dyn TerminalAdapter>,
    ) -> Result<(), ()> {
        let mut inner = self.0.inner.lock().unwrap();
        if let Some(intent) = &inner.intent {
            return if matches!(intent, Intent::Apply(selected) if selected == generation)
                && inner.status.phase != Phase::Failed
            {
                Ok(())
            } else {
                Err(())
            };
        }
        let pending = acquire()?;
        inner.intent = Some(Intent::Apply(generation.to_owned()));
        self.start(&mut inner, Some(pending), adapter);
        Ok(())
    }
    pub(crate) fn quit(&self, adapter: Arc<dyn TerminalAdapter>) {
        let mut inner = self.0.inner.lock().unwrap();
        if inner.intent.is_some() {
            if inner.status.phase == Phase::Failed && !inner.recovery_quit {
                inner.recovery_quit = true;
                if inner.finished {
                    inner.exit_allowed = true;
                    drop(inner);
                    adapter.exit();
                }
            }
            return;
        }
        inner.intent = Some(Intent::Quit);
        self.start(&mut inner, None, adapter);
    }
    fn start(
        &self,
        inner: &mut Inner,
        pending: Option<Install>,
        adapter: Arc<dyn TerminalAdapter>,
    ) {
        inner.status.phase = Phase::Stopping;
        self.0.cancel.send_replace(true);
        adapter.stop_updates();
        self.signal(inner);
        let lifecycle = self.clone();
        self.0.runtime.spawn(async move {
            lifecycle.run(pending, adapter).await;
        });
    }
    async fn drained(&self) {
        let mut changes = self.0.changes.subscribe();
        loop {
            {
                let inner = self.0.inner.lock().unwrap();
                if inner.active == 0 && inner.forwarders == 0 {
                    return;
                }
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
    async fn run(&self, pending: Option<Install>, adapter: Arc<dyn TerminalAdapter>) {
        let cleanup_lifecycle = self.clone();
        let cleanup_adapter = adapter.clone();
        let mut cleanup = tokio::spawn(async move {
            cleanup_lifecycle.drained().await;
            cleanup_adapter.cleanup().await
        });
        let (errors, timed_out) = match tokio::time::timeout(SHUTDOWN_DEADLINE, &mut cleanup).await
        {
            Ok(result) => (
                result.unwrap_or(ShutdownErrors {
                    uncertain: true,
                    ..Default::default()
                }),
                false,
            ),
            Err(_) => {
                self.fail(FailureCategory::ShutdownTimeout, None);
                // Observe late completion; never drop/abort cleanup or install later.
                (
                    cleanup.await.unwrap_or(ShutdownErrors {
                        uncertain: true,
                        ..Default::default()
                    }),
                    true,
                )
            }
        };
        {
            let mut inner = self.0.inner.lock().unwrap();
            inner.status.shutdown = errors.clone();
            self.signal(&mut inner);
        }
        if timed_out || errors.failed() {
            if !timed_out {
                self.fail(FailureCategory::Shutdown, None);
            }
        } else if let Some(pending) = pending {
            self.phase(Phase::Installing);
            let installer = adapter.clone();
            // There is intentionally no timeout/abandon path for replacement.
            match tokio::task::spawn_blocking(move || installer.install(pending)).await {
                Ok(Ok(())) => {
                    self.phase(Phase::Restarting);
                    adapter.restart();
                }
                Ok(Err(error)) => self.fail(FailureCategory::Install, error.previous_app),
                Err(_) => self.fail(FailureCategory::Uncertain, None),
            }
        } else {
            self.phase(Phase::Stopped);
            self.0.inner.lock().unwrap().exit_allowed = true;
            adapter.exit();
        }
        let mut inner = self.0.inner.lock().unwrap();
        inner.finished = true;
        let exit = inner.recovery_quit;
        if exit {
            inner.exit_allowed = true;
        }
        self.signal(&mut inner);
        drop(inner);
        if exit {
            adapter.exit();
        }
    }
    fn phase(&self, phase: Phase) {
        let mut inner = self.0.inner.lock().unwrap();
        inner.status.phase = phase;
        self.signal(&mut inner);
    }
    fn fail(&self, category: FailureCategory, previous_app: Option<PathBuf>) {
        let mut inner = self.0.inner.lock().unwrap();
        inner.status.phase = Phase::Failed;
        inner.status.failure = Some(category);
        inner.status.previous_app = previous_app;
        inner.status.guidance = Some(
            "Quit and reinstall Teamarness from a trusted disk image. Do not retry installation in this session.",
        );
        self.signal(&mut inner);
    }
    #[cfg(test)]
    async fn wait_for_phase(&self, phase: Phase) {
        let mut changes = self.0.changes.subscribe();
        while self.status().phase != phase {
            changes.changed().await.unwrap();
        }
    }
    #[cfg(test)]
    pub(crate) async fn finished(&self) {
        let mut changes = self.0.changes.subscribe();
        loop {
            if self.0.inner.lock().unwrap().finished {
                return;
            }
            changes.changed().await.unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
