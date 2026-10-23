use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::sync::Mutex;

use teamarness_package_protocol::TOMBSTONE_CAPACITY;

use crate::capability::AppendFence;
use crate::protocol::{UiEventReceiver, UiEventRoute, WireFailure};
use crate::supervisor::{self, CRASH_WINDOW, ProcessGeneration, ProcessSpec, RESTART_DELAYS};
use crate::ui::{AdmittedUi, MAX_UI_SUBSCRIPTIONS_PER_MOUNT};
use crate::{
    LifecycleState, MountState, PackageErrorCode, PackageInstanceKey, PackageInstanceSnapshot,
    PackageValue, UiMountId, UiRequestId, UiSubscriptionId,
};

pub(crate) struct InstanceActor {
    key: PackageInstanceKey,
    spec: ProcessSpec,
    state: Mutex<InstanceState>,
    lifecycle_gate: Mutex<()>,
}

struct InstanceState {
    lifecycle: LifecycleState,
    generation: u64,
    process: Option<Arc<ProcessGeneration>>,
    append_fence: Option<Arc<AppendFence>>,
    failures: VecDeque<Instant>,
    active_since: Option<Instant>,
    mounts: BTreeMap<UiMountId, MountRecord>,
    requests: BTreeMap<(UiMountId, UiRequestId), String>,
    request_tombstones: BTreeSet<(UiMountId, UiRequestId)>,
    subscriptions: BTreeMap<(UiMountId, UiSubscriptionId), SubscriptionState>,
    next_wire_id: u64,
    /// Set once host shutdown begins; no generation may be started afterwards.
    shutting_down: bool,
}

struct MountRecord {
    generation: u64,
    state: MountState,
}

struct SubscriptionState {
    wire_id: String,
    route: Arc<UiEventRoute>,
}

impl InstanceActor {
    pub(crate) fn new(key: PackageInstanceKey, spec: ProcessSpec) -> Self {
        Self {
            key,
            spec,
            state: Mutex::new(InstanceState {
                lifecycle: LifecycleState::Admitted,
                generation: 0,
                process: None,
                append_fence: None,
                failures: VecDeque::new(),
                active_since: None,
                mounts: BTreeMap::new(),
                requests: BTreeMap::new(),
                request_tombstones: BTreeSet::new(),
                subscriptions: BTreeMap::new(),
                next_wire_id: 0,
                shutting_down: false,
            }),
            lifecycle_gate: Mutex::new(()),
        }
    }

    pub(crate) async fn activate(self: &Arc<Self>) -> Result<(), WireFailure> {
        let _gate = self.lifecycle_gate.lock().await;
        self.start_generation(false, true).await
    }

    async fn start_generation(
        self: &Arc<Self>,
        recovering: bool,
        automatic_retries: bool,
    ) -> Result<(), WireFailure> {
        let attempts = if automatic_retries { 4 } else { 1 };
        let mut last = WireFailure::new(PackageErrorCode::SpawnFailed);
        for attempt in 0..attempts {
            // Includes failed handshakes and activations: their capability calls can
            // outlive the process even when supervisor::spawn returned no generation.
            let previous = self.state.lock().await.append_fence.clone();
            if let Some(previous) = previous
                && !previous.retire(Duration::from_secs(30)).await
            {
                self.state.lock().await.lifecycle = LifecycleState::Failed;
                return Err(WireFailure::new(PackageErrorCode::OutcomeUncertain));
            }
            if attempt > 0 {
                tokio::time::sleep(RESTART_DELAYS[attempt - 1]).await;
            }
            let append_fence = Arc::new(AppendFence::default());
            let generation = {
                let mut state = self.state.lock().await;
                if state.shutting_down {
                    return Err(WireFailure::new(PackageErrorCode::ShuttingDown));
                }
                if state
                    .active_since
                    .is_some_and(|since| since.elapsed() >= CRASH_WINDOW)
                {
                    state.failures.clear();
                }
                state.generation += 1;
                state.append_fence = Some(append_fence.clone());
                state.lifecycle = if recovering || state.generation > 1 {
                    LifecycleState::Recovering
                } else {
                    LifecycleState::Activating
                };
                state.generation
            };
            match supervisor::spawn(
                &self.spec,
                generation,
                recovering || generation > 1,
                append_fence,
            )
            .await
            {
                Ok(process) => {
                    let process = Arc::new(process);
                    {
                        let mut state = self.state.lock().await;
                        if state.shutting_down {
                            // Shutdown began while this generation was spawning and
                            // could not see it; reap it here instead of orphaning it.
                            drop(state);
                            process.shutdown().await;
                            return Err(WireFailure::new(PackageErrorCode::ShuttingDown));
                        }
                        state.process = Some(process.clone());
                        state.lifecycle = LifecycleState::Active;
                        state.active_since = Some(Instant::now());
                    }
                    self.monitor(process);
                    return Ok(());
                }
                Err(error) => {
                    let deterministic = deterministic(error.code);
                    last = error.clone();
                    let mut state = self.state.lock().await;
                    state.failures.push_back(Instant::now());
                    while state
                        .failures
                        .front()
                        .is_some_and(|time| time.elapsed() > CRASH_WINDOW)
                    {
                        state.failures.pop_front();
                    }
                    state.lifecycle = if state.failures.len() >= 4 {
                        LifecycleState::CrashLoop
                    } else {
                        LifecycleState::Failed
                    };
                    if deterministic || state.lifecycle == LifecycleState::CrashLoop {
                        return Err(error);
                    }
                }
            }
        }
        Err(last)
    }

    pub(crate) async fn snapshot(&self) -> PackageInstanceSnapshot {
        let state = self.state.lock().await;
        PackageInstanceSnapshot {
            key: self.key.clone(),
            lifecycle: state.lifecycle,
            process_generation: (state.generation > 0).then_some(state.generation),
            mount_state: state
                .mounts
                .values()
                .map(|mount| mount.state)
                .max_by_key(|mount| match mount {
                    MountState::Failed => 2,
                    MountState::Mounted => 1,
                    _ => 0,
                }),
        }
    }

    pub(crate) async fn suspend(&self) -> Result<(), WireFailure> {
        let _gate = self.lifecycle_gate.lock().await;
        let process = {
            let mut state = self.state.lock().await;
            if state.lifecycle == LifecycleState::Suspended {
                return Ok(());
            }
            if state.lifecycle != LifecycleState::Active {
                return Err(WireFailure::new(PackageErrorCode::InvalidLifecycleState));
            }
            state.lifecycle = LifecycleState::Suspending;
            state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?
        };
        let result = process
            .client
            .request(
                format!("h:lifecycle-suspend-{}", process.generation),
                "lifecycle.suspend",
                None,
                PackageValue::Object(Default::default()),
                Duration::from_secs(10),
                true,
            )
            .await;
        let mut state = self.state.lock().await;
        state.lifecycle = if result.is_ok() {
            LifecycleState::Suspended
        } else {
            LifecycleState::Failed
        };
        result.map(drop)
    }

    pub(crate) async fn recover(self: &Arc<Self>) -> Result<(), WireFailure> {
        let _gate = self.lifecycle_gate.lock().await;
        let (lifecycle, process) = {
            let state = self.state.lock().await;
            (state.lifecycle, state.process.clone())
        };
        match lifecycle {
            LifecycleState::Active => Ok(()),
            LifecycleState::Suspended => {
                let process =
                    process.ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
                self.state.lock().await.lifecycle = LifecycleState::Recovering;
                let result = process
                    .client
                    .request(
                        format!("h:lifecycle-recover-{}", process.generation),
                        "lifecycle.recover",
                        None,
                        PackageValue::Object(Default::default()),
                        Duration::from_secs(30),
                        true,
                    )
                    .await;
                self.state.lock().await.lifecycle = if result.is_ok() {
                    LifecycleState::Active
                } else {
                    LifecycleState::Failed
                };
                result.map(drop)
            }
            LifecycleState::Failed | LifecycleState::CrashLoop => {
                if let Some(process) = process {
                    process.terminate().await;
                }
                {
                    let mut state = self.state.lock().await;
                    clear_process_state(&mut state);
                    state.failures.clear();
                }
                self.start_generation(true, false).await
            }
            LifecycleState::Disposed => Err(WireFailure::new(PackageErrorCode::Disposed)),
            _ => Err(WireFailure::new(PackageErrorCode::InvalidLifecycleState)),
        }
    }

    pub(crate) async fn dispose(&self) -> Result<(), WireFailure> {
        let _gate = self.lifecycle_gate.lock().await;
        let process = {
            let mut state = self.state.lock().await;
            if state.lifecycle == LifecycleState::Disposed {
                return Ok(());
            }
            state.lifecycle = LifecycleState::Disposing;
            state.process.clone()
        };
        let mut result = Ok(());
        if let Some(process) = process {
            result = process
                .client
                .request(
                    format!("h:lifecycle-dispose-{}", process.generation),
                    "lifecycle.dispose",
                    None,
                    PackageValue::Object(Default::default()),
                    Duration::from_secs(10),
                    true,
                )
                .await
                .map(drop);
            process.shutdown().await;
        }
        if let Some(fence) = self.state.lock().await.append_fence.clone()
            && !fence.retire(Duration::from_secs(30)).await
        {
            self.state.lock().await.lifecycle = LifecycleState::Failed;
            return Err(WireFailure::new(PackageErrorCode::OutcomeUncertain));
        }
        let mut state = self.state.lock().await;
        clear_process_state(&mut state);
        state.lifecycle = LifecycleState::Disposed;
        result
    }

    pub(crate) async fn open_mount(&self) -> Result<(UiMountId, u64), WireFailure> {
        let mut state = self.state.lock().await;
        if state.lifecycle != LifecycleState::Active {
            return Err(WireFailure::new(PackageErrorCode::InvalidLifecycleState));
        }
        state.mounts.clear();
        let mut random = [0_u8; 24];
        getrandom::fill(&mut random)
            .map_err(|_| WireFailure::new(PackageErrorCode::UiLoadFailed))?;
        let mount = UiMountId::new(URL_SAFE_NO_PAD.encode(random))
            .map_err(|_| WireFailure::new(PackageErrorCode::UiLoadFailed))?;
        let generation = state.generation;
        state.mounts.insert(
            mount.clone(),
            MountRecord {
                generation,
                state: MountState::Mounted,
            },
        );
        Ok((mount, generation))
    }

    pub(crate) async fn close_mount(&self, mount: &UiMountId) -> bool {
        let (removed, process, streams, requests) = {
            let mut state = self.state.lock().await;
            let removed = state.mounts.remove(mount).is_some();
            let keys = state
                .subscriptions
                .keys()
                .filter(|(stream_mount, _)| stream_mount == mount)
                .cloned()
                .collect::<Vec<_>>();
            let streams = keys
                .iter()
                .filter_map(|key| state.subscriptions.remove(key))
                .map(|stream| stream.wire_id)
                .collect::<Vec<_>>();
            let requests = state
                .requests
                .iter()
                .filter(|((request_mount, _), _)| request_mount == mount)
                .map(|(_, wire_id)| wire_id.clone())
                .collect::<Vec<_>>();
            state
                .requests
                .retain(|(request_mount, _), _| request_mount != mount);
            state
                .request_tombstones
                .retain(|(request_mount, _)| request_mount != mount);
            (removed, state.process.clone(), streams, requests)
        };
        if let Some(process) = process {
            for request in requests {
                let _ = process.client.cancel(&request).await;
            }
            for stream in streams {
                let _ = process.client.stream_close(&stream).await;
            }
        }
        removed
    }

    pub(crate) async fn fail_mount(&self, mount: &UiMountId) -> bool {
        let (changed, process, streams, requests) = {
            let mut state = self.state.lock().await;
            let changed = state.mounts.get_mut(mount).is_some_and(|record| {
                if record.state == MountState::Failed {
                    false
                } else {
                    record.state = MountState::Failed;
                    true
                }
            });
            let keys = state
                .subscriptions
                .keys()
                .filter(|(stream_mount, _)| stream_mount == mount)
                .cloned()
                .collect::<Vec<_>>();
            let streams = keys
                .iter()
                .filter_map(|key| state.subscriptions.remove(key))
                .map(|stream| stream.wire_id)
                .collect::<Vec<_>>();
            let requests = state
                .requests
                .iter()
                .filter(|((request_mount, _), _)| request_mount == mount)
                .map(|(_, wire_id)| wire_id.clone())
                .collect::<Vec<_>>();
            state
                .requests
                .retain(|(request_mount, _), _| request_mount != mount);
            (changed, state.process.clone(), streams, requests)
        };
        if let Some(process) = process {
            for request in requests {
                let _ = process.client.cancel(&request).await;
            }
            for stream in streams {
                let _ = process.client.stream_close(&stream).await;
            }
        }
        changed
    }

    pub(crate) async fn is_mount_current(&self, mount: &UiMountId, generation: u64) -> bool {
        let state = self.state.lock().await;
        state.lifecycle == LifecycleState::Active
            && state.generation == generation
            && state.mounts.get(mount).is_some_and(|record| {
                record.generation == generation && record.state == MountState::Mounted
            })
    }

    pub(crate) fn workspace_name(&self) -> &str {
        &self.spec.workspace_name
    }

    pub(crate) async fn request(
        self: &Arc<Self>,
        mount: &UiMountId,
        request: UiRequestId,
        operation: String,
        payload: PackageValue,
    ) -> Result<PackageValue, WireFailure> {
        let request_key = (mount.clone(), request.clone());
        let (process, wire_id) = {
            let mut state = self.state.lock().await;
            if state.lifecycle != LifecycleState::Active
                || !state.mounts.get(mount).is_some_and(|record| {
                    record.generation == state.generation && record.state == MountState::Mounted
                })
            {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            if state.request_tombstones.remove(&request_key) {
                return Err(WireFailure::new(PackageErrorCode::Cancelled));
            }
            if state.requests.contains_key(&request_key) {
                return Err(WireFailure::new(PackageErrorCode::Overloaded));
            }
            state.next_wire_id = state.next_wire_id.saturating_add(1);
            let wire_id = format!("h:ui-{}-{}", state.generation, state.next_wire_id);
            state.requests.insert(request_key.clone(), wire_id.clone());
            (process, wire_id)
        };
        let result = if let Some(code) = process.client.failure() {
            Err(WireFailure::new(code))
        } else {
            process
                .client
                .request(
                    wire_id,
                    &operation,
                    Some(mount.as_str()),
                    payload,
                    Duration::from_secs(30),
                    true,
                )
                .await
        };
        {
            let mut state = self.state.lock().await;
            state.requests.remove(&request_key);
            state.request_tombstones.insert(request_key);
            while state.request_tombstones.len() > TOMBSTONE_CAPACITY {
                state.request_tombstones.pop_first();
            }
        }
        match result {
            Ok(value) => serde_json::from_value(value)
                .map_err(|_| WireFailure::new(PackageErrorCode::ProtocolViolation)),
            Err(error) => {
                if error.code != PackageErrorCode::BackendError
                    && error.code != PackageErrorCode::Cancelled
                {
                    let _gate = self.lifecycle_gate.lock().await;
                    let handling = {
                        let mut state = self.state.lock().await;
                        if !state.shutting_down
                            && state.process.as_ref().map(|live| live.generation)
                                == Some(process.generation)
                        {
                            clear_process_state(&mut state);
                            state.failures.push_back(Instant::now());
                            while state
                                .failures
                                .front()
                                .is_some_and(|time| time.elapsed() > CRASH_WINDOW)
                            {
                                state.failures.pop_front();
                            }
                            state.lifecycle = if state.failures.len() >= 4 {
                                LifecycleState::CrashLoop
                            } else {
                                LifecycleState::Failed
                            };
                            Some(
                                state.lifecycle != LifecycleState::CrashLoop
                                    && restartable(error.code),
                            )
                        } else {
                            None
                        }
                    };
                    if let Some(should_restart) = handling {
                        process.terminate().await;
                        if should_restart {
                            let _ = self.start_generation(true, true).await;
                        }
                    }
                }
                Err(error)
            }
        }
    }

    pub(crate) async fn cancel(
        &self,
        mount: &UiMountId,
        request: &UiRequestId,
    ) -> Result<(), WireFailure> {
        let request_key = (mount.clone(), request.clone());
        let (process, wire_id) = {
            let mut state = self.state.lock().await;
            let Some(wire_id) = state.requests.get(&request_key).cloned() else {
                state.request_tombstones.insert(request_key);
                while state.request_tombstones.len() > TOMBSTONE_CAPACITY {
                    state.request_tombstones.pop_first();
                }
                return Ok(());
            };
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            (process, wire_id)
        };
        process.client.cancel(&wire_id).await
    }

    pub(crate) async fn stream_open(
        &self,
        mount: &UiMountId,
        subscription: UiSubscriptionId,
        operation: String,
        payload: PackageValue,
        contract: Arc<AdmittedUi>,
    ) -> Result<UiEventReceiver, WireFailure> {
        let key = (mount.clone(), subscription);
        let (process, wire_id) = {
            let mut state = self.state.lock().await;
            if state.lifecycle != LifecycleState::Active
                || !state.mounts.get(mount).is_some_and(|record| {
                    record.generation == state.generation && record.state == MountState::Mounted
                })
            {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            if state.subscriptions.contains_key(&key)
                || state
                    .subscriptions
                    .keys()
                    .filter(|(stream_mount, _)| stream_mount == mount)
                    .count()
                    >= MAX_UI_SUBSCRIPTIONS_PER_MOUNT
            {
                return Err(WireFailure::new(PackageErrorCode::Overloaded));
            }
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            state.next_wire_id = state.next_wire_id.saturating_add(1);
            let wire_id = format!("h:stream-{}-{}", state.generation, state.next_wire_id);
            (process, wire_id)
        };
        let (receiver, route) = process
            .client
            .open_ui_stream(
                wire_id.clone(),
                &operation,
                mount.as_str(),
                payload,
                contract,
            )
            .await?;
        self.state
            .lock()
            .await
            .subscriptions
            .insert(key, SubscriptionState { wire_id, route });
        Ok(receiver)
    }

    pub(crate) async fn acknowledge(
        &self,
        mount: &UiMountId,
        subscription: &UiSubscriptionId,
        through: u64,
    ) -> Result<(), WireFailure> {
        let (process, stream, route) = {
            let state = self.state.lock().await;
            let generation = state.generation;
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            let stream = state
                .subscriptions
                .get(&(mount.clone(), subscription.clone()))
                .ok_or_else(|| WireFailure::new(PackageErrorCode::StaleMount))?;
            if process.generation != generation {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            (process, stream.wire_id.clone(), stream.route.clone())
        };
        // The record moves only once the frame is queued. A `credit` frame
        // that never reached the writer would otherwise leave the Host
        // believing it had released credit the Package never received: the
        // window would stay shut, a conforming Package would stop sending, and
        // this path is the only producer of `credit` there is.
        match route.acknowledge(through).await? {
            Some(pending) => {
                pending
                    .commit(|amount| process.client.credit(&stream, amount))
                    .await
            }
            // The ignore verdict: the route has already credited these events,
            // so there is nothing to send and nothing to record. Crediting
            // them again is the one thing the credit section makes fatal.
            None => Ok(()),
        }
    }

    pub(crate) async fn close_subscription(
        &self,
        mount: &UiMountId,
        subscription: &UiSubscriptionId,
    ) -> Result<(), WireFailure> {
        let (process, stream) = {
            let mut state = self.state.lock().await;
            let stream = state
                .subscriptions
                .remove(&(mount.clone(), subscription.clone()))
                .ok_or_else(|| WireFailure::new(PackageErrorCode::StaleMount))?;
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            (process, stream.wire_id)
        };
        process.client.stream_close(&stream).await
    }

    fn monitor(self: &Arc<Self>, process: Arc<ProcessGeneration>) {
        let actor = Arc::downgrade(self);
        tokio::spawn(async move {
            let code = process.client.wait_for_failure().await;
            let Some(actor) = actor.upgrade() else {
                return;
            };
            let _gate = actor.lifecycle_gate.lock().await;
            let handling = {
                let mut state = actor.state.lock().await;
                // A process exiting because host shutdown stopped it is not a crash.
                if state.shutting_down
                    || state.lifecycle != LifecycleState::Active
                    || state.process.as_ref().map(|live| live.generation)
                        != Some(process.generation)
                {
                    None
                } else {
                    clear_process_state(&mut state);
                    state.failures.push_back(Instant::now());
                    while state
                        .failures
                        .front()
                        .is_some_and(|time| time.elapsed() > CRASH_WINDOW)
                    {
                        state.failures.pop_front();
                    }
                    state.lifecycle = if state.failures.len() >= 4 {
                        LifecycleState::CrashLoop
                    } else {
                        LifecycleState::Failed
                    };
                    Some(state.lifecycle != LifecycleState::CrashLoop && restartable(code))
                }
            };
            if let Some(should_restart) = handling {
                process.terminate().await;
                if should_restart {
                    let _ = Box::pin(actor.start_generation(true, true)).await;
                }
            }
        });
    }

    pub(crate) async fn shutdown(&self) -> bool {
        // Mark shutdown before waiting for the lifecycle gate so failure monitors and
        // in-flight (re)starts stop creating generations, then serialize with any
        // lifecycle transition that is already running so its process is visible here.
        self.state.lock().await.shutting_down = true;
        let _gate = self.lifecycle_gate.lock().await;
        let process = self.state.lock().await.process.clone();
        if let Some(process) = process {
            process.shutdown().await;
        }
        if let Some(fence) = self.state.lock().await.append_fence.clone()
            && !fence.retire(Duration::from_secs(30)).await
        {
            self.state.lock().await.lifecycle = LifecycleState::Failed;
            return false;
        }
        let mut state = self.state.lock().await;
        clear_process_state(&mut state);
        true
    }
}

fn clear_process_state(state: &mut InstanceState) {
    state.process = None;
    state.mounts.clear();
    state.requests.clear();
    state.request_tombstones.clear();
    state.subscriptions.clear();
}

fn deterministic(code: PackageErrorCode) -> bool {
    matches!(
        code,
        PackageErrorCode::ArtifactChanged
            | PackageErrorCode::BackendError
            | PackageErrorCode::HandshakeMismatch
            | PackageErrorCode::ProtocolViolation
            | PackageErrorCode::MalformedFrame
            | PackageErrorCode::FrameTooLarge
            | PackageErrorCode::CapabilityUnavailable
            | PackageErrorCode::CapabilityVersionUnsupported
    )
}

fn restartable(code: PackageErrorCode) -> bool {
    matches!(
        code,
        PackageErrorCode::ProcessExited
            | PackageErrorCode::DeadlineExceeded
            | PackageErrorCode::OutcomeUncertain
    )
}
