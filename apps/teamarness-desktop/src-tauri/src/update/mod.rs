//! Native acquisition only. No installer/restart operation is reachable from this module.
mod config;
mod transport;
mod verification;

use reqwest::Url;
use semver::Version;
use serde::Serialize;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::{
    sync::{Notify, watch},
    time::Instant,
};
use transport::{Progress, Resource, Transfer};
use verification::{OfficialVerifier, Verifier};

pub(crate) fn configure<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    // The official plugin requires a key even at initialization. Disabled builds
    // must neither initialize it nor register its denied guest operations.
    if config::enabled() {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
const PERIOD: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY: Duration = Duration::from_secs(60);

trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'_, ()>;
}
struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep_until(deadline))
    }
}
#[derive(Clone)]
struct Cancellation(watch::Sender<bool>);
impl Cancellation {
    fn new() -> Self {
        Self(watch::channel(false).0)
    }
    fn cancel(&self) {
        self.0.send_replace(true);
    }
    async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCategory {
    Metadata,
    Network,
    TooLarge,
    Timeout,
    Verification,
    Cancelled,
    StaleCandidate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Disabled,
    Idle,
    Checking,
    Downloading,
    Verifying,
    Ready,
    Stopped,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub revision: u64,
    pub state: State,
    pub installed_version: String,
    pub candidate_version: Option<String>,
    pub downloaded_bytes: usize,
    pub archive_limit: usize,
    pub error: Option<ErrorCategory>,
    pub pending_generation: Option<String>,
    pub notes: Option<String>,
}
struct Metadata {
    raw: serde_json::Value,
    version: Version,
    url: Url,
    signature: String,
    notes: String,
}
fn stable(value: &str) -> Result<Version, ErrorCategory> {
    let version = Version::parse(value).map_err(|_| ErrorCategory::Metadata)?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != value {
        return Err(ErrorCategory::Metadata);
    }
    Ok(version)
}
impl Metadata {
    fn parse(bytes: &[u8], installed: &Version) -> Result<Self, ErrorCategory> {
        if bytes.len() > config::FEED_LIMIT {
            return Err(ErrorCategory::TooLarge);
        }
        let raw: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| ErrorCategory::Metadata)?;
        let version = stable(raw["version"].as_str().ok_or(ErrorCategory::Metadata)?)?;
        if &version <= installed {
            return Err(ErrorCategory::Metadata);
        }
        let notes = match raw.get("notes") {
            None => "",
            Some(value) => value.as_str().ok_or(ErrorCategory::Metadata)?,
        };
        if notes.len() > 4096
            || notes.chars().any(|c| {
                matches!(c, '<' | '>') || (c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            })
        {
            return Err(ErrorCategory::Metadata);
        }
        let platform = &raw["platforms"]["darwin-aarch64"];
        if platform != &raw["platforms"]["darwin-aarch64-app"] {
            return Err(ErrorCategory::Metadata);
        }
        let url = Url::parse(platform["url"].as_str().ok_or(ErrorCategory::Metadata)?)
            .map_err(|_| ErrorCategory::Metadata)?;
        transport::validate_url(&url)?;
        let prefix = format!("/learn-ai-engineering/teamarness/releases/download/v{version}/");
        let asset = url
            .path()
            .strip_prefix(&prefix)
            .ok_or(ErrorCategory::Metadata)?;
        if url.host_str() != Some("github.com")
            || url.query().is_some()
            || asset.is_empty()
            || !asset
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(ErrorCategory::Metadata);
        }
        let signature = platform["signature"]
            .as_str()
            .ok_or(ErrorCategory::Metadata)?;
        if signature.is_empty() || signature.len() > 4096 {
            return Err(ErrorCategory::Metadata);
        }
        let signature = signature.to_owned();
        let notes = notes.to_owned();
        Ok(Self {
            raw,
            version,
            url,
            signature,
            notes,
        })
    }
}

// Consumed only by the terminal lifecycle owner in the next implementation phase.
#[allow(dead_code)]
pub(crate) struct Pending {
    pub update: Update,
    pub bytes: Vec<u8>,
}
struct Inner {
    snapshot: Snapshot,
    pending: Option<Pending>,
    terminal: bool,
    busy: bool,
    next_due: Instant,
    last_explicit: Option<Instant>,
    generation: u64,
}
struct System {
    clock: Arc<dyn Clock>,
    transfer: Arc<dyn Transfer>,
    verifier: Arc<dyn Verifier>,
}
#[derive(Clone)]
pub(crate) struct UpdateService {
    inner: Arc<Mutex<Inner>>,
    system: Option<Arc<System>>,
    cancel: Cancellation,
    changed: Arc<Notify>,
}
impl UpdateService {
    fn new(installed: Version, system: Option<System>) -> Self {
        let now = system
            .as_ref()
            .map_or_else(Instant::now, |system| system.clock.now());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                snapshot: Snapshot {
                    revision: 0,
                    state: if system.is_some() {
                        State::Idle
                    } else {
                        State::Disabled
                    },
                    installed_version: installed.to_string(),
                    candidate_version: None,
                    downloaded_bytes: 0,
                    archive_limit: config::ARCHIVE_LIMIT,
                    error: None,
                    pending_generation: None,
                    notes: None,
                },
                pending: None,
                terminal: false,
                busy: false,
                next_due: now,
                last_explicit: None,
                generation: 0,
            })),
            system: system.map(Arc::new),
            cancel: Cancellation::new(),
            changed: Arc::new(Notify::new()),
        }
    }
    pub(crate) fn start<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Self {
        let system = if config::enabled() {
            (|| {
                let clock: Arc<dyn Clock> = Arc::new(SystemClock);
                let updater = app
                    .updater_builder()
                    .pubkey(option_env!("TEAMARNESS_UPDATE_PUBLIC_KEY").unwrap_or_default())
                    .target("darwin-aarch64")
                    .build()
                    .map_err(|_| ErrorCategory::Verification)?;
                Ok(Some(System {
                    transfer: Arc::new(transport::BoundedTransfer::new(clock.clone())?),
                    clock,
                    verifier: Arc::new(OfficialVerifier(updater)),
                }))
            })()
        } else {
            Ok(None)
        };
        // Adapter initialization is also pre-consent work. It must not strand open
        // Core/Host modules or prevent offline startup; expose a safe status instead.
        let (system, error) = match system {
            Ok(system) => (system, None),
            Err(error) => (None, Some(error)),
        };
        let service = Self::new(app.package_info().version.clone(), system);
        service.inner.lock().unwrap().snapshot.error = error;
        if service.system.is_some() {
            let scheduler = service.clone();
            tokio::spawn(async move {
                scheduler.schedule().await;
            });
        }
        service
    }
    // Native-only interface; the main-only command adapter arrives in phase 5.
    #[allow(dead_code)]
    pub(crate) fn status(&self) -> Snapshot {
        self.inner.lock().unwrap().snapshot.clone()
    }
    #[allow(dead_code)]
    pub(crate) fn request_check(&self) -> bool {
        self.begin(true)
    }
    fn begin(&self, explicit: bool) -> bool {
        let Some(system) = &self.system else {
            return false;
        };
        let mut inner = self.inner.lock().unwrap();
        let now = system.clock.now();
        if inner.terminal || inner.busy || inner.pending.is_some() {
            return false;
        }
        if explicit {
            if inner
                .last_explicit
                .is_some_and(|last| now.duration_since(last) < RETRY)
            {
                return false;
            }
            inner.last_explicit = Some(now);
        } else if now < inner.next_due {
            return false;
        }
        inner.next_due = now + PERIOD;
        inner.busy = true;
        inner.snapshot.state = State::Checking;
        inner.snapshot.error = None;
        inner.snapshot.candidate_version = None;
        inner.snapshot.notes = None;
        inner.snapshot.downloaded_bytes = 0;
        inner.snapshot.revision += 1;
        drop(inner);
        let service = self.clone();
        tokio::spawn(async move {
            service.acquire().await;
        });
        true
    }
    async fn schedule(self) {
        let system = self.system.as_ref().unwrap();
        loop {
            self.begin(false);
            let deadline = {
                let inner = self.inner.lock().unwrap();
                if inner.terminal || inner.pending.is_some() {
                    return;
                }
                // A verifier may outlive a due interval. Await publication rather
                // than spinning or queuing another operation at an expired deadline.
                if inner.busy && inner.next_due <= system.clock.now() {
                    None
                } else {
                    Some(inner.next_due)
                }
            };
            tokio::select! { biased;
                _ = self.cancel.cancelled() => return,
                _ = self.changed.notified() => (),
                _ = async { match deadline { Some(deadline) => system.clock.sleep_until(deadline).await, None => std::future::pending().await } } => (),
            }
        }
    }
    fn publish(&self, change: impl FnOnce(&mut Snapshot)) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.terminal {
            change(&mut inner.snapshot);
            inner.snapshot.revision += 1;
            self.changed.notify_one();
        }
    }
    async fn acquire(&self) {
        let result = self.download_verified().await;
        let mut inner = self.inner.lock().unwrap();
        inner.busy = false;
        if inner.terminal {
            return;
        }
        match result {
            Ok(Some(pending)) => {
                inner.generation += 1;
                inner.snapshot.pending_generation = Some(inner.generation.to_string());
                inner.pending = Some(pending);
                inner.snapshot.state = State::Ready;
            }
            Ok(None) => {
                inner.snapshot.state = State::Idle;
            }
            Err(error) => {
                inner.snapshot.state = State::Idle;
                inner.snapshot.error = Some(error);
                inner.snapshot.candidate_version = None;
                inner.snapshot.notes = None;
                inner.snapshot.downloaded_bytes = 0;
            }
        }
        inner.snapshot.revision += 1;
        self.changed.notify_one();
    }
    async fn download_verified(&self) -> Result<Option<Pending>, ErrorCategory> {
        let system = self.system.as_ref().unwrap();
        let Some(feed) = system
            .transfer
            .fetch(
                Url::parse(config::ENDPOINT).unwrap(),
                Resource::Feed,
                self.cancel.clone(),
                Arc::new(|_| {}),
            )
            .await?
        else {
            return Ok(None);
        };
        let installed = stable(&self.inner.lock().unwrap().snapshot.installed_version)?;
        let metadata = Metadata::parse(&feed, &installed)?;
        self.publish(|snapshot| {
            snapshot.state = State::Downloading;
            snapshot.candidate_version = Some(metadata.version.to_string());
            snapshot.notes = Some(metadata.notes.clone());
        });
        let service = self.clone();
        let progress: Progress = Arc::new(move |count| {
            service.publish(|snapshot| {
                snapshot.downloaded_bytes = count.min(config::ARCHIVE_LIMIT);
            });
        });
        let bytes = system
            .transfer
            .fetch(
                metadata.url.clone(),
                Resource::Archive,
                self.cancel.clone(),
                progress,
            )
            .await?
            .ok_or(ErrorCategory::Network)?;
        if bytes.len() > config::ARCHIVE_LIMIT {
            return Err(ErrorCategory::TooLarge);
        }
        self.publish(|snapshot| snapshot.state = State::Verifying);
        let verifier = system.verifier.clone();
        // Keep verification off the runtime executor. Cancellation fences its late result;
        // the owned blocking task retains/frees its bytes even if Quit stops the waiter.
        let verified = tokio::task::spawn_blocking(move || {
            let update = verifier.verify(&metadata, &bytes)?;
            Ok(Pending { update, bytes })
        });
        tokio::select! { biased;
            _ = self.cancel.cancelled() => Err(ErrorCategory::Cancelled),
            result = verified => result.map_err(|_| ErrorCategory::Verification)?.map(Some),
        }
    }
    pub(crate) fn stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.terminal {
            return;
        }
        inner.terminal = true;
        inner.pending = None;
        inner.snapshot.state = State::Stopped;
        inner.snapshot.pending_generation = None;
        inner.snapshot.candidate_version = None;
        inner.snapshot.notes = None;
        inner.snapshot.downloaded_bytes = 0;
        inner.snapshot.revision += 1;
        self.cancel.cancel();
    }
    // Admission transfers the same verified bytes to one terminal owner. It cannot install.
    #[allow(dead_code)]
    pub(crate) fn admit_apply(&self, generation: &str) -> Result<Pending, ErrorCategory> {
        let mut inner = self.inner.lock().unwrap();
        if inner.terminal
            || inner.snapshot.state != State::Ready
            || inner.snapshot.pending_generation.as_deref() != Some(generation)
            || stable(
                inner
                    .snapshot
                    .candidate_version
                    .as_deref()
                    .unwrap_or_default(),
            )? <= stable(&inner.snapshot.installed_version)?
        {
            return Err(ErrorCategory::StaleCandidate);
        }
        let pending = inner.pending.take().ok_or(ErrorCategory::StaleCandidate)?;
        inner.terminal = true;
        inner.snapshot.state = State::Stopped;
        inner.snapshot.pending_generation = None;
        inner.snapshot.revision += 1;
        self.cancel.cancel();
        Ok(pending)
    }
}

#[cfg(test)]
mod tests;
