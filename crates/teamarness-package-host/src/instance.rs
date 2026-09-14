use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::sync::Mutex;

use crate::protocol::WireFailure;
use crate::supervisor::{self, CRASH_WINDOW, ProcessGeneration, ProcessSpec, RESTART_DELAYS};
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
    failures: VecDeque<Instant>,
    active_since: Option<Instant>,
    mounts: BTreeMap<UiMountId, u64>,
    requests: BTreeMap<UiRequestId, String>,
    request_tombstones: BTreeSet<UiRequestId>,
    subscriptions: BTreeMap<UiSubscriptionId, SubscriptionState>,
}

struct SubscriptionState {
    mount: UiMountId,
    wire_id: String,
    acknowledged: u64,
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
                failures: VecDeque::new(),
                active_since: None,
                mounts: BTreeMap::new(),
                requests: BTreeMap::new(),
                request_tombstones: BTreeSet::new(),
                subscriptions: BTreeMap::new(),
            }),
            lifecycle_gate: Mutex::new(()),
        }
    }

    pub(crate) async fn activate(self: &Arc<Self>) -> Result<(), WireFailure> {
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
            if attempt > 0 {
                tokio::time::sleep(RESTART_DELAYS[attempt - 1]).await;
            }
            let generation = {
                let mut state = self.state.lock().await;
                if state
                    .active_since
                    .is_some_and(|since| since.elapsed() >= CRASH_WINDOW)
                {
                    state.failures.clear();
                }
                state.generation += 1;
                state.lifecycle = if recovering || state.generation > 1 {
                    LifecycleState::Recovering
                } else {
                    LifecycleState::Activating
                };
                state.generation
            };
            match supervisor::spawn(&self.spec, generation, recovering || generation > 1).await {
                Ok(process) => {
                    let process = Arc::new(process);
                    {
                        let mut state = self.state.lock().await;
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
            mount_state: (!state.mounts.is_empty()).then_some(MountState::Mounted),
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
                    state.process = None;
                    state.mounts.clear();
                    state.requests.clear();
                    state.request_tombstones.clear();
                    state.subscriptions.clear();
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
        let mut state = self.state.lock().await;
        state.process = None;
        state.mounts.clear();
        state.requests.clear();
        state.request_tombstones.clear();
        state.subscriptions.clear();
        state.lifecycle = LifecycleState::Disposed;
        result
    }

    pub(crate) async fn open_mount(&self) -> Result<UiMountId, WireFailure> {
        let mut state = self.state.lock().await;
        if state.lifecycle != LifecycleState::Active {
            return Err(WireFailure::new(PackageErrorCode::InvalidLifecycleState));
        }
        let mut random = [0_u8; 24];
        getrandom::fill(&mut random)
            .map_err(|_| WireFailure::new(PackageErrorCode::UiLoadFailed))?;
        let mount = UiMountId::new(URL_SAFE_NO_PAD.encode(random))
            .map_err(|_| WireFailure::new(PackageErrorCode::UiLoadFailed))?;
        let generation = state.generation;
        state.mounts.insert(mount.clone(), generation);
        Ok(mount)
    }

    pub(crate) async fn close_mount(&self, mount: &UiMountId) -> bool {
        let (removed, process, streams) = {
            let mut state = self.state.lock().await;
            let removed = state.mounts.remove(mount).is_some();
            let streams = state
                .subscriptions
                .iter()
                .filter(|(_, stream)| &stream.mount == mount)
                .map(|(id, stream)| (id.clone(), stream.wire_id.clone()))
                .collect::<Vec<_>>();
            for (id, _) in &streams {
                state.subscriptions.remove(id);
            }
            (removed, state.process.clone(), streams)
        };
        if let Some(process) = process {
            for (_, stream) in streams {
                let _ = process.client.stream_close(&stream).await;
            }
        }
        removed
    }

    pub(crate) async fn request(
        self: &Arc<Self>,
        mount: &UiMountId,
        request: UiRequestId,
        operation: String,
        payload: PackageValue,
    ) -> Result<PackageValue, WireFailure> {
        let (process, wire_id) = {
            let mut state = self.state.lock().await;
            if state.lifecycle != LifecycleState::Active
                || state.mounts.get(mount) != Some(&state.generation)
            {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            if state.request_tombstones.remove(&request) {
                return Err(WireFailure::new(PackageErrorCode::Cancelled));
            }
            if state.requests.contains_key(&request) {
                return Err(WireFailure::new(PackageErrorCode::Overloaded));
            }
            let wire_id = format!("h:ui-{}", request.as_str());
            state.requests.insert(request.clone(), wire_id.clone());
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
            state.requests.remove(&request);
            state.request_tombstones.insert(request.clone());
            while state.request_tombstones.len() > 64 {
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
                        if state.process.as_ref().map(|live| live.generation)
                            == Some(process.generation)
                        {
                            state.process = None;
                            state.mounts.clear();
                            state.requests.clear();
                            state.request_tombstones.clear();
                            state.subscriptions.clear();
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

    pub(crate) async fn cancel(&self, request: &UiRequestId) -> Result<(), WireFailure> {
        let (process, wire_id) = {
            let mut state = self.state.lock().await;
            let Some(wire_id) = state.requests.get(request).cloned() else {
                state.request_tombstones.insert(request.clone());
                while state.request_tombstones.len() > 64 {
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
    ) -> Result<(), WireFailure> {
        let wire_id = format!("h:stream-{}", subscription.as_str());
        let process = {
            let mut state = self.state.lock().await;
            if state.lifecycle != LifecycleState::Active
                || state.mounts.get(mount) != Some(&state.generation)
            {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            if state.subscriptions.contains_key(&subscription) {
                return Err(WireFailure::new(PackageErrorCode::Overloaded));
            }
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            state.subscriptions.insert(
                subscription.clone(),
                SubscriptionState {
                    mount: mount.clone(),
                    wire_id: wire_id.clone(),
                    acknowledged: 0,
                },
            );
            process
        };
        if let Err(error) = process
            .client
            .stream_open(wire_id, &operation, mount.as_str(), payload)
            .await
        {
            self.state.lock().await.subscriptions.remove(&subscription);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn acknowledge(
        &self,
        mount: &UiMountId,
        subscription: &UiSubscriptionId,
        through: u64,
    ) -> Result<(), WireFailure> {
        let (process, stream, amount) = {
            let mut state = self.state.lock().await;
            let generation = state.generation;
            let process = state
                .process
                .clone()
                .ok_or_else(|| WireFailure::new(PackageErrorCode::ProcessExited))?;
            let stream = state
                .subscriptions
                .get_mut(subscription)
                .filter(|stream| &stream.mount == mount)
                .ok_or_else(|| WireFailure::new(PackageErrorCode::StaleMount))?;
            if through <= stream.acknowledged || through - stream.acknowledged > 32 {
                return Err(WireFailure::new(PackageErrorCode::ProtocolViolation));
            }
            if process.generation != generation {
                return Err(WireFailure::new(PackageErrorCode::StaleMount));
            }
            let amount = through - stream.acknowledged;
            stream.acknowledged = through;
            (process, stream.wire_id.clone(), amount)
        };
        process.client.credit(&stream, amount).await
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
                .remove(subscription)
                .filter(|stream| &stream.mount == mount)
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
                if state.lifecycle != LifecycleState::Active
                    || state.process.as_ref().map(|live| live.generation)
                        != Some(process.generation)
                {
                    None
                } else {
                    state.process = None;
                    state.mounts.clear();
                    state.requests.clear();
                    state.request_tombstones.clear();
                    state.subscriptions.clear();
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

    pub(crate) async fn shutdown(&self) {
        let process = self.state.lock().await.process.clone();
        if let Some(process) = process {
            process.shutdown().await;
        }
        let mut state = self.state.lock().await;
        state.process = None;
        state.mounts.clear();
        state.requests.clear();
        state.request_tombstones.clear();
        state.subscriptions.clear();
    }
}

fn deterministic(code: PackageErrorCode) -> bool {
    matches!(
        code,
        PackageErrorCode::ArtifactChanged
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
