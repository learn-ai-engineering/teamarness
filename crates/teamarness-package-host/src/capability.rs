use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex as BlockingMutex};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value, json};
use teamarness_core::{
    CoreRuntime, MembershipEpoch, MembershipRole, PublicIdentity, PublicSignature, SigningContext,
};
use teamarness_package_protocol::INITIAL_STREAM_CREDIT;
use tokio::sync::{Mutex, Semaphore, mpsc, watch};

use crate::protocol::{CapabilityFailure, HostStreamSender, PortQueryStream, StreamEnd};
use crate::{InstallationPublicIdentity, PackageErrorCode, PackageIdentity, WorkspaceId};

const SIGNING_CONTEXT: &str = "package-signing/v1";
// Every bound below is the Host's half of one capability schema. `capability_limits_match_contract_schemas`
// reads the schema files and fails when the two drift.
pub(crate) const MAX_PURPOSE_BYTES: usize = 64;
pub(crate) const MAX_SIGNING_PAYLOAD_BYTES: usize = 256 * 1024;
/// The largest batch of sealed records one `open` call may carry.
pub(crate) const MAX_OPEN_RECORDS: usize = 32;
/// The largest total decoded size of one `open` call's sealed records.
pub(crate) const MAX_OPEN_DECODED_BYTES: usize = 512 * 1024;
/// The largest canonical payload one `append` call may carry.
pub(crate) const MAX_CANONICAL_DECODED_BYTES: usize = 262_144;
/// The largest `candidates` answer, matching the `teamarness.principals`
/// snapshot bound a Workspace roster already honours.
pub(crate) const MAX_AGENT_CANDIDATES: usize = 1024;
/// The credit window the contract grants one stream.
const MAX_STREAM_CREDIT: u64 = INITIAL_STREAM_CREDIT as u64;
/// The largest record page one blocking read may return.
///
/// A page is read against the credit the Package has released, so it can never
/// exceed the window the contract grants. `record_page_fits_the_writer_budget`
/// asserts that a full page of records at `BUDGETED_RECORD_BYTES` fits the
/// writer's outbound data budget. A page of larger records does not fail: the
/// stream's `event` waits for writer capacity, because a slow reader is
/// backpressure rather than a capability outage.
const RECORD_PAGE: u64 = MAX_STREAM_CREDIT;
/// The record size one page is budgeted for.
///
/// A record event carries two unpadded base64url copies of the record — its
/// canonical bytes and its sealed form — so one page of them is roughly
/// `RECORD_PAGE * 2 * ceil(bytes * 4 / 3)` on the wire.
/// Read by `record_page_fits_the_writer_budget`. Nothing in production reads
/// it, because a page of larger records is backpressure, not an error.
#[allow(dead_code)]
const BUDGETED_RECORD_BYTES: usize = MAX_CANONICAL_DECODED_BYTES / 32;
/// Slots in one stream task's command queue.
///
/// A conformant Package returns credit only for events it has already accepted,
/// and the Host sends no event without credit, so at most `MAX_STREAM_CREDIT`
/// `credit` frames can be queued before the stream task drains any of them —
/// the SDK returns credit one event at a time, so a 32-event catch-up really
/// does produce 32 frames. One more slot carries `Close`. Anything smaller
/// turns routine catch-up backpressure into a refused `credit` frame, which the
/// reader reports as a Package protocol violation and kills the Backend for.
const STREAM_COMMAND_QUEUE: usize = MAX_STREAM_CREDIT as usize + 1;

/// The Host-wide bound on concurrent blocking Core work for Package capability
/// calls.
///
/// Core's state lock is synchronous, so a capability call does its Core work on
/// a blocking thread. Without a bound, Packages could occupy the whole blocking
/// pool and the call deadline would stop meaning anything.
#[derive(Clone)]
pub(crate) struct CoreCallLimiter {
    permits: Arc<Semaphore>,
}

impl CoreCallLimiter {
    pub(crate) const PERMITS: usize = 8;

    pub(crate) fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(Self::PERMITS)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_permits(permits: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits)),
        }
    }

    /// Waits for a permit, then runs `f` on a blocking thread, holding the
    /// permit until `f` returns.
    ///
    /// The permit moves into the closure, so a call that outlived its deadline
    /// still releases its permit only when its Core work really ends. `started`
    /// becomes true as the closure's first statement, which is what makes an
    /// `append` outcome uncertain rather than failed.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        started: Arc<AtomicBool>,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PackageErrorCode> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
        tokio::task::spawn_blocking(move || {
            started.store(true, Ordering::Release);
            let value = f();
            drop(permit);
            value
        })
        .await
        .map_err(|_| PackageErrorCode::CapabilityUnavailable)
    }

    async fn run_append<T: Send + 'static>(
        &self,
        fence: Arc<AppendFence>,
        started: Arc<AtomicBool>,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PackageErrorCode> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
        tokio::task::spawn_blocking(move || {
            let _running = fence.enter().ok_or(PackageErrorCode::Cancelled)?;
            started.store(true, Ordering::Release);
            let result = f();
            drop(permit);
            Ok(result)
        })
        .await
        .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
    }
}

/// Admission and quiescence of append work belonging to one Package Process generation.
/// Closing the gate and entering a Core closure share the same lock. A caller deadline
/// cannot make a closure which already entered disappear from the drain count.
#[derive(Default)]
pub(crate) struct AppendFence {
    state: BlockingMutex<AppendFenceState>,
    drained: Condvar,
}

#[derive(Default)]
struct AppendFenceState {
    retired: bool,
    running: usize,
}

struct RunningAppend<'a>(&'a AppendFence);

impl Drop for RunningAppend<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.running -= 1;
        self.0.drained.notify_all();
    }
}

impl AppendFence {
    fn enter(&self) -> Option<RunningAppend<'_>> {
        let mut state = self.state.lock().unwrap();
        if state.retired {
            return None;
        }
        state.running += 1;
        Some(RunningAppend(self))
    }

    /// Refuse all queued work atomically with entry, then await closure exit.
    pub(crate) async fn retire(self: &Arc<Self>, deadline: Duration) -> bool {
        let fence = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let end = Instant::now() + deadline;
            let mut state = fence.state.lock().unwrap();
            state.retired = true;
            while state.running > 0 {
                let remaining = end.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let (next, _) = fence.drained.wait_timeout(state, remaining).unwrap();
                state = next;
            }
            true
        })
        .await
        .unwrap_or(false)
    }
}

/// The stable name of one Semantic Capability contract shape.
///
/// A family evolves by version within a major. A different shape — document-oriented
/// synchronization, say — is a new family declared beside the existing ones, never a new version
/// of one, so adding it changes no lifecycle operation, no Package Process Protocol version and no
/// UI contract.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct CapabilityFamily(&'static str);

impl CapabilityFamily {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub(crate) const fn name(self) -> &'static str {
        self.0
    }
}

/// What the Host provides for one capability family: a major, and the minors it can speak.
#[derive(Clone, Copy, Debug)]
struct Descriptor {
    family: CapabilityFamily,
    major: u16,
    min_minor: u16,
    max_minor: u16,
    kind: CapabilityKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CapabilityKind {
    WorkspaceContext,
    Membership,
    Principals,
    AgentBinding,
    Signing,
    ImmutableRecordLog,
    PackagePorts,
    ContentViews,
    #[cfg(test)]
    Neutral,
}

/// What a Package declares: one capability family, and the versions of it the Package accepts.
#[derive(Clone)]
pub(crate) struct CapabilityRequirement {
    pub(crate) family: String,
    pub(crate) major: u16,
    pub(crate) min_minor: u16,
    pub(crate) max_minor: u16,
}

#[derive(Clone)]
pub(crate) struct CapabilityScope {
    pub(crate) package_identity: PackageIdentity,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) workspace_name: String,
    pub(crate) installation: InstallationPublicIdentity,
    /// The Package Data Directory content views resolve their sources under.
    pub(crate) data_directory: PathBuf,
    pub(crate) content_views: Arc<crate::content_views::ContentViews>,
}

#[derive(Clone)]
struct BoundGrant {
    descriptor: Descriptor,
    minor: u16,
    package_identity: PackageIdentity,
    workspace_id: WorkspaceId,
    installation: InstallationPublicIdentity,
    generation: u64,
}

#[derive(Clone)]
pub(crate) struct AdvertisedGrant {
    pub(crate) grant: String,
    pub(crate) identity: &'static str,
    pub(crate) major: u16,
    pub(crate) minor: u16,
}

/// Constructor inputs only: append retirement and authority leases remain independent guards.
pub(crate) struct GenerationGuards {
    pub(crate) append_fence: Arc<AppendFence>,
    pub(crate) authority: crate::authority::GenerationAuthority,
}

pub(crate) struct GenerationCapabilities {
    authority: crate::authority::GenerationAuthority,
    core: Arc<CoreRuntime>,
    limiter: CoreCallLimiter,
    append_fence: Arc<AppendFence>,
    scope: CapabilityScope,
    generation: u64,
    grants: BTreeMap<String, BoundGrant>,
    advertised: Vec<AdvertisedGrant>,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    cancelled: watch::Sender<bool>,
    broker: std::sync::Weak<crate::broker::PortBroker>,
}

impl GenerationCapabilities {
    pub(crate) fn bind(
        core: Arc<CoreRuntime>,
        scope: CapabilityScope,
        requirements: &[CapabilityRequirement],
        generation: u64,
        limiter: CoreCallLimiter,
        guards: GenerationGuards,
        broker: std::sync::Weak<crate::broker::PortBroker>,
    ) -> Result<Arc<Self>, PackageErrorCode> {
        if scope.package_identity.as_str() == crate::values::RESERVED_PRINCIPALS_SCOPE {
            return Err(PackageErrorCode::InvalidPackageIdentity);
        }
        let registry = Registry::production();
        let mut grants = BTreeMap::new();
        let mut advertised = Vec::with_capacity(requirements.len());
        for requirement in requirements {
            let (descriptor, minor) = registry.negotiate(requirement)?;
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
            let grant = URL_SAFE_NO_PAD.encode(random);
            grants.insert(
                grant.clone(),
                BoundGrant {
                    descriptor,
                    minor,
                    package_identity: scope.package_identity.clone(),
                    workspace_id: scope.workspace_id.clone(),
                    installation: scope.installation.clone(),
                    generation,
                },
            );
            advertised.push(AdvertisedGrant {
                grant,
                identity: descriptor.family.name(),
                major: descriptor.major,
                minor,
            });
        }
        let (cancelled, _) = watch::channel(false);
        Ok(Arc::new(Self {
            authority: guards.authority,
            core,
            limiter,
            append_fence: guards.append_fence,
            scope,
            generation,
            grants,
            advertised,
            streams: Arc::new(Mutex::new(BTreeMap::new())),
            cancelled,
            broker,
        }))
    }

    pub(crate) fn advertised(&self) -> &[AdvertisedGrant] {
        &self.advertised
    }

    /// Validates the request on the asynchronous task, then runs the Core work
    /// on a blocking thread through the shared limiter.
    pub(crate) async fn call(
        &self,
        grant: Option<&str>,
        payload: &Value,
        started: Arc<AtomicBool>,
        consumer_id: &str,
    ) -> Result<Value, CapabilityFailure> {
        let bound = self
            .bound_grant(grant)
            .map_err(CapabilityFailure::certain)?;
        if bound.descriptor.kind == CapabilityKind::PackagePorts {
            let object = payload
                .as_object()
                .ok_or(PackageErrorCode::CapabilityRequestInvalid)
                .map_err(CapabilityFailure::certain)?;
            let method = string_field(object, "method").map_err(CapabilityFailure::certain)?;
            let broker = self
                .broker
                .upgrade()
                .ok_or(PackageErrorCode::CapabilityUnavailable)
                .map_err(CapabilityFailure::certain)?;
            return match method {
                "available" | "describe" => {
                    let state = self
                        .port_access_state(&broker, object)
                        .await
                        .map_err(CapabilityFailure::certain)?;
                    let result = if method == "available" {
                        broker
                            .metadata_page(
                                &self.scope.package_identity,
                                &self.scope.workspace_id,
                                object,
                                &state,
                            )
                            .await
                    } else {
                        broker
                            .metadata_chunk(
                                &self.scope.package_identity,
                                &self.scope.workspace_id,
                                object,
                                &state,
                            )
                            .await
                    };
                    result.map_err(CapabilityFailure::certain)
                }
                "enable" | "disable" => self
                    .port_selection(&broker, method, object)
                    .await
                    .map_err(CapabilityFailure::certain),
                "invoke" => {
                    #[cfg(feature = "test-hooks")]
                    broker
                        .pause_command_actor(&self.scope.package_identity, object)
                        .await;
                    let route = self.authorize_port(&broker, object).await.map_err(|code| {
                        broker.record_call_refusal(
                            &self.scope.package_identity,
                            consumer_id,
                            self.generation,
                            object,
                            code,
                        );
                        CapabilityFailure::certain(code)
                    })?;
                    #[cfg(feature = "test-hooks")]
                    broker
                        .pause_port_work(&self.scope.package_identity, false)
                        .await;
                    #[cfg(feature = "test-hooks")]
                    broker
                        .pause_command_authorized(&self.scope.package_identity, object)
                        .await;
                    broker
                        .call(
                            &self.scope.package_identity,
                            route,
                            object,
                            started,
                            consumer_id,
                            self.generation,
                        )
                        .await
                        .map(crate::protocol::encode_port_value)
                        .map_err(|failure| CapabilityFailure {
                            code: if failure.outcome_uncertain
                                && matches!(
                                    failure.code,
                                    PackageErrorCode::ProcessExited
                                        | PackageErrorCode::DeadlineExceeded
                                        | PackageErrorCode::Cancelled
                                ) {
                                PackageErrorCode::OutcomeUncertain
                            } else {
                                failure.code
                            },
                            outcome_uncertain: failure.outcome_uncertain,
                            package_error: failure.package_error,
                        })
                }
                _ => Err(CapabilityFailure::certain(
                    PackageErrorCode::CapabilityRequestInvalid,
                )),
            };
        }
        self.call_inner(grant, payload, started)
            .await
            .map_err(CapabilityFailure::certain)
    }

    async fn call_inner(
        &self,
        grant: Option<&str>,
        payload: &Value,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let grant = self.bound_grant(grant)?;
        let object = payload
            .as_object()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let method = string_field(object, "method")?;
        match grant.descriptor.kind {
            CapabilityKind::WorkspaceContext => {
                self.workspace_context(method, object, started).await
            }
            CapabilityKind::Membership => self.membership(method, object, started).await,
            CapabilityKind::Principals => self.principals(method, object, started).await,
            CapabilityKind::AgentBinding => self.agent_binding(method, object, started).await,
            CapabilityKind::Signing => self.signing(method, object, started).await,
            CapabilityKind::ImmutableRecordLog => {
                self.immutable_record(method, object, grant.minor, started)
                    .await
            }
            CapabilityKind::PackagePorts => Err(PackageErrorCode::CapabilityRequestInvalid),
            CapabilityKind::ContentViews => self.content_views(method, object).await,
            #[cfg(test)]
            CapabilityKind::Neutral => Ok(json!({"neutral":true})),
        }
    }

    pub(crate) async fn open_stream(
        &self,
        stream_id: String,
        topic: &str,
        initial_credit: u64,
        payload: &Value,
        output: HostStreamSender,
    ) -> Result<(), PackageErrorCode> {
        if topic != "capability.stream" || initial_credit != MAX_STREAM_CREDIT {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let object = payload
            .as_object()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let grant = self.bound_grant(Some(string_field(object, "grant")?))?;
        if grant.descriptor.kind == CapabilityKind::PackagePorts {
            let broker = self
                .broker
                .upgrade()
                .ok_or(PackageErrorCode::CapabilityUnavailable)?;
            let route = self.authorize_port(&broker, object).await?;
            #[cfg(feature = "test-hooks")]
            broker
                .pause_port_work(&self.scope.package_identity, false)
                .await;
            if object.get("kind").and_then(Value::as_str) == Some("event") {
                let subscription = broker
                    .subscribe_event(
                        &self.scope.package_identity,
                        &self.scope.workspace_id,
                        &self.core.public_identity().to_string(),
                        route,
                        object,
                        self.generation,
                    )
                    .await?;
                let (commands, receiver) = mpsc::channel(STREAM_COMMAND_QUEUE);
                let mut streams = self.streams.lock().await;
                if streams.len() >= 32 || streams.insert(stream_id.clone(), commands).is_some() {
                    drop(streams);
                    broker.drop_event_subscription(&subscription).await;
                    return Err(PackageErrorCode::Overloaded);
                }
                drop(streams);
                broker.record_event_subscription(&subscription, &stream_id);
                let all_streams = self.streams.clone();
                let cancelled = self.cancelled.subscribe();
                tokio::spawn(async move {
                    broker
                        .clone()
                        .run_event_subscription(
                            stream_id.clone(),
                            subscription,
                            receiver,
                            output,
                            cancelled,
                        )
                        .await;
                    all_streams.lock().await.remove(&stream_id);
                });
                return Ok(());
            }
            let query = broker.query_stream(route, object).await?;
            let (commands, receiver) = mpsc::channel(STREAM_COMMAND_QUEUE);
            let mut streams = self.streams.lock().await;
            if streams.len() >= 32 || streams.insert(stream_id.clone(), commands).is_some() {
                let _ = query.client.stream_close(&query.id).await;
                return Err(PackageErrorCode::Overloaded);
            }
            drop(streams);
            tokio::spawn(run_port_query(
                stream_id,
                query,
                receiver,
                output,
                self.streams.clone(),
                self.cancelled.subscribe(),
            ));
            return Ok(());
        }
        let method = string_field(object, "method")?;
        let core_workspace =
            teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let after_epoch =
            if grant.descriptor.kind == CapabilityKind::Membership && method == "subscribe" {
                exact_fields(object, &["grant", "method", "afterEpoch"])?;
                Some(decimal_field(object, "afterEpoch")?)
            } else {
                None
            };
        let after_profile_cursor =
            if grant.descriptor.kind == CapabilityKind::Principals && method == "subscribe" {
                exact_fields(object, &["grant", "method", "afterCursor"])?;
                Some(decimal_field(object, "afterCursor")?)
            } else {
                None
            };
        let after_cursor = if grant.descriptor.kind == CapabilityKind::ImmutableRecordLog
            && method == "subscribe"
        {
            exact_fields(object, &["grant", "method", "afterCursor"])?;
            Some(optional_cursor(object.get("afterCursor"))?)
        } else {
            None
        };
        if after_epoch.is_none() && after_cursor.is_none() && after_profile_cursor.is_none() {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let (commands, command_receiver) = mpsc::channel(STREAM_COMMAND_QUEUE);
        let mut streams = self.streams.lock().await;
        if streams.len() >= 32 || streams.insert(stream_id.clone(), commands).is_some() {
            return Err(PackageErrorCode::Overloaded);
        }
        drop(streams);
        let core = self.core.clone();
        let all_streams = self.streams.clone();
        match grant.descriptor.kind {
            CapabilityKind::Membership => {
                let after_epoch = after_epoch.expect("validated membership cursor");
                tokio::spawn(run_membership_stream(
                    core,
                    core_workspace,
                    stream_id,
                    after_epoch,
                    initial_credit,
                    command_receiver,
                    output,
                    all_streams,
                    self.cancelled.subscribe(),
                ));
            }
            CapabilityKind::Principals => {
                tokio::spawn(run_principal_stream(
                    core,
                    core_workspace,
                    stream_id,
                    after_profile_cursor.expect("validated profile cursor"),
                    initial_credit,
                    command_receiver,
                    output,
                    all_streams,
                    self.cancelled.subscribe(),
                ));
            }
            CapabilityKind::ImmutableRecordLog => {
                let after_cursor = after_cursor.expect("validated record cursor");
                tokio::spawn(run_record_stream(
                    core,
                    core_workspace,
                    self.scope.package_identity.as_str().to_owned(),
                    stream_id,
                    after_cursor,
                    initial_credit,
                    command_receiver,
                    output,
                    all_streams,
                    self.cancelled.subscribe(),
                    grant.minor,
                    #[cfg(feature = "test-hooks")]
                    self.broker.upgrade().and_then(|broker| {
                        broker.take_record_availability_pause(&self.scope.package_identity)
                    }),
                ));
            }
            _ => {
                self.streams.lock().await.remove(&stream_id);
                return Err(PackageErrorCode::CapabilityRequestInvalid);
            }
        }
        Ok(())
    }

    /// Routes one already-classified `credit` frame to its stream.
    ///
    /// `ContractState` has decided the frame before it arrives here: the amount
    /// is inside the window the contract grants, and credit for a stream this
    /// generation has retired never reaches this function. What is left is a
    /// send that `STREAM_COMMAND_QUEUE` is sized to accept.
    ///
    /// The stream tasks therefore add what they are given and do not clamp it.
    /// The classifier decrements at dequeue and the task at enqueue, so the
    /// task's credit is never above the classifier's, and the classifier
    /// refuses any grant past the window on the reader before this is reached
    /// (`state.rs`,
    /// `credit_cannot_carry_a_stream_past_the_window_it_was_opened_with`). A
    /// clamp here would be a second copy of a classifier rule inside an
    /// adapter, which is how the two sides come to disagree.
    pub(crate) async fn add_credit(&self, stream_id: &str, amount: u64) {
        let Some(sender) = self.streams.lock().await.get(stream_id).cloned() else {
            return;
        };
        let _ = sender.try_send(StreamCommand::Credit(amount));
    }

    pub(crate) async fn close_stream(&self, stream_id: &str) {
        if let Some(sender) = self.streams.lock().await.remove(stream_id) {
            let _ = sender.try_send(StreamCommand::Close);
        }
    }

    fn bound_grant(&self, grant: Option<&str>) -> Result<&BoundGrant, PackageErrorCode> {
        let grant = grant
            .and_then(|grant| self.grants.get(grant))
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        if grant.package_identity != self.scope.package_identity
            || grant.workspace_id != self.scope.workspace_id
            || grant.installation != self.scope.installation
            || grant.generation != self.generation
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        Ok(grant)
    }

    async fn workspace_context(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        if method != "get" {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        exact_fields(object, &["method"])?;
        let core = Arc::clone(&self.core);
        let workspace_id = self.scope.workspace_id.as_str().to_owned();
        let workspace_name = self.scope.workspace_name.clone();
        let installation = self.scope.installation.as_str().to_owned();
        self.limiter
            .run(started, move || {
                Ok(json!({
                    "workspaceId": workspace_id,
                    "workspaceName": workspace_name,
                    "installationIdentity": installation,
                    "installationDisplayName": core.display_name(),
                }))
            })
            .await?
    }

    async fn membership(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let core = Arc::clone(&self.core);
        match method {
            "current" => {
                exact_fields(object, &["method"])?;
                self.limiter
                    .run(started, move || {
                        let snapshot = core
                            .membership_snapshot(workspace)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let members = snapshot
                            .members()
                            .iter()
                            .map(|member| {
                                json!({
                                    "identity":member.identity().to_string(),
                                    "role":match member.role() {
                                        MembershipRole::Member => "member",
                                        MembershipRole::Manager => "manager",
                                    }
                                })
                            })
                            .collect::<Vec<_>>();
                        Ok(json!({
                            "workspaceId":snapshot.workspace_id().to_string(),
                            "epoch":snapshot.epoch().value().to_string(),
                            "membershipVersion":snapshot.version().to_string(),
                            "members":members,
                        }))
                    })
                    .await?
            }
            "was_member" => {
                exact_fields(object, &["method", "identity", "atUnixMs"])?;
                let identity = PublicIdentity::from_str(string_field(object, "identity")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let at_unix_ms = signed_decimal_field(object, "atUnixMs")?;
                self.limiter
                    .run(started, move || {
                        let was_member = core
                            .was_member(workspace, identity, at_unix_ms)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        Ok(json!({
                            "wasMember":was_member,
                            "warning":"informational_not_authorization",
                        }))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    async fn principals(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let core = Arc::clone(&self.core);
        match method {
            "current" => {
                exact_fields(object, &["method"])?;
                self.limiter
                    .run(started, move || principal_snapshot(&core, workspace))
                    .await?
            }
            "lookup" => {
                let principal_id = if object.contains_key("principalId") {
                    exact_fields(object, &["method", "principalId"])?;
                    Some(
                        teamarness_core::PrincipalId::from_str(string_field(
                            object,
                            "principalId",
                        )?)
                        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?,
                    )
                } else {
                    exact_fields(object, &["method", "identity"])?;
                    None
                };
                let identity = if principal_id.is_none() {
                    Some(
                        PublicIdentity::from_str(string_field(object, "identity")?)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?,
                    )
                } else {
                    None
                };
                self.limiter
                    .run(started, move || {
                        let principal = if let Some(id) = principal_id {
                            core.principal(workspace, id)
                        } else {
                            core.workspace_principal(
                                workspace,
                                identity.expect("validated identity"),
                            )
                        }
                        .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
                        Ok(json!({"principal":principal.as_ref().map(principal_value)}))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    /// `teamarness.agent-binding` 1.0: a Package reads and sets its own Agent
    /// Binding. The grant fixes the Package and Workspace, so no request names
    /// a Package, and Core enforces stewardship (ADR 0030).
    async fn agent_binding(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let package = self.scope.package_identity.as_str().to_owned();
        let core = Arc::clone(&self.core);
        let broker = self.broker.clone();
        let key = self.instance_key();
        #[cfg(feature = "test-hooks")]
        if matches!(method, "bind" | "unbind")
            && let Some(broker) = self.broker.upgrade()
        {
            broker
                .pause_binding_write(&self.scope.package_identity)
                .await;
        }
        match method {
            "current" => {
                exact_fields(object, &["method"])?;
                self.limiter
                    .run(started, move || current_binding(&core, workspace, &package))
                    .await?
            }
            "candidates" => {
                exact_fields(object, &["method"])?;
                self.limiter
                    .run(started, move || {
                        let bound = core
                            .bound_agent(workspace, &package)
                            .map_err(binding_error)?;
                        let (_, principals) =
                            core.principals_snapshot(workspace).map_err(binding_error)?;
                        let own_key = core.public_identity();
                        let own_person = principals
                            .iter()
                            .find(|principal| {
                                principal.kind() == teamarness_core::PrincipalKind::Person
                                    && principal.keys().contains(&own_key)
                            })
                            .map(teamarness_core::WorkspacePrincipal::id);
                        let agents = principals
                            .iter()
                            .filter(|principal| {
                                principal.kind() == teamarness_core::PrincipalKind::Agent
                                    && own_person.is_some()
                                    && principal.steward_principal_id() == own_person
                            })
                            .map(|agent| {
                                let mut row = bound_agent_value(agent);
                                row["bound"] = Value::Bool(bound == Some(agent.id()));
                                row
                            })
                            .collect::<Vec<_>>();
                        if agents.len() > MAX_AGENT_CANDIDATES {
                            return Err(PackageErrorCode::CapabilityUnavailable);
                        }
                        Ok(json!({"agents":agents}))
                    })
                    .await?
            }
            "bind" => {
                exact_fields(object, &["method", "principalId"])?;
                let id =
                    teamarness_core::PrincipalId::from_str(string_field(object, "principalId")?)
                        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                self.authority
                    .run(&self.limiter, started, move || {
                        core.bind_agent_package(workspace, id, &package)
                            .map_err(binding_error)?;
                        if let Some(broker) = broker.upgrade() {
                            broker
                                .reconcile_binding(&key, &crate::port_access::actor(&core, &key)?);
                        }
                        current_binding(&core, workspace, &package)
                    })
                    .await?
            }
            "unbind" => {
                exact_fields(object, &["method"])?;
                self.authority
                    .run(&self.limiter, started, move || {
                        core.unbind_agent_package(workspace, &package)
                            .map_err(binding_error)?;
                        if let Some(broker) = broker.upgrade() {
                            broker
                                .reconcile_binding(&key, &crate::port_access::actor(&core, &key)?);
                        }
                        Ok(json!({"agent":Value::Null}))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    /// `teamarness.content-views` 1.0: open and close views over files in this Package's own
    /// Data Directory. The grant fixes the Package, Workspace and generation, so a view is this
    /// generation's and no request names a Package or a directory.
    async fn content_views(
        &self,
        method: &str,
        object: &Map<String, Value>,
    ) -> Result<Value, PackageErrorCode> {
        let owner = crate::content_views::ViewOwner {
            instance: crate::PackageInstanceKey::new(
                self.scope.package_identity.clone(),
                self.scope.workspace_id.clone(),
            ),
            generation: self.generation,
        };
        match method {
            "open" => {
                let request = crate::content_views::parse_open(object)?;
                let views = Arc::clone(&self.scope.content_views);
                let root = self.scope.data_directory.clone();
                // Resolving every source is blocking file work.
                tokio::task::spawn_blocking(move || views.open(owner, &root, request))
                    .await
                    .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
            }
            "close" => {
                exact_fields(object, &["method", "view"])?;
                let view = crate::ContentViewToken::new(string_field(object, "view")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                Ok(json!({"closed": self.scope.content_views.close(&owner, &view)}))
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    fn instance_key(&self) -> crate::PackageInstanceKey {
        crate::PackageInstanceKey::new(
            self.scope.package_identity.clone(),
            self.scope.workspace_id.clone(),
        )
    }

    async fn port_access_state(
        &self,
        broker: &Arc<crate::broker::PortBroker>,
        object: &Map<String, Value>,
    ) -> Result<crate::port_access::AccessState, PackageErrorCode> {
        let agent = match object.get("access").and_then(Value::as_str) {
            Some("agent") => true,
            Some("declared") => false,
            _ => return Err(PackageErrorCode::CapabilityRequestInvalid),
        };
        broker
            .access_work(
                self.core.clone(),
                self.instance_key(),
                &self.authority,
                move |_, _, state| {
                    if agent && state.actor["kind"] != "agent" {
                        return Err(PackageErrorCode::AgentNotBound);
                    }
                    Ok(state)
                },
            )
            .await
    }

    async fn authorize_port(
        &self,
        broker: &Arc<crate::broker::PortBroker>,
        object: &Map<String, Value>,
    ) -> Result<crate::port_access::ResolvedPort, PackageErrorCode> {
        let key = self.instance_key();
        // Subscribe before reading authority, so accepted membership loss cannot be missed
        // between issuing a lease and starting its idle-stream watcher.
        let changes = if object.contains_key("method") {
            None
        } else {
            let core = self.core.clone();
            let workspace = teamarness_core::WorkspaceId::from_str(key.workspace_id.as_str())
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
            Some(
                tokio::task::spawn_blocking(move || core.subscribe_principals(workspace))
                    .await
                    .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
                    .map_err(binding_error)?,
            )
        };
        let input = object.clone();
        let resolved_key = key.clone();
        let generation = self.generation;
        let authority_revision = self.authority.revision();
        let route = broker
            .access_work(
                self.core.clone(),
                key.clone(),
                &self.authority,
                move |broker, _, state| {
                    broker.reconcile_binding(&resolved_key, &state.actor);
                    let mut route = broker.resolve_access(
                        &resolved_key.package_identity,
                        &resolved_key.workspace_id,
                        &input,
                        &state,
                    )?;
                    if !input.contains_key("method") {
                        route.lease = Some(broker.subscription_lease(
                            resolved_key,
                            &input,
                            &route,
                            generation,
                            authority_revision,
                        ));
                    }
                    Ok(route)
                },
            )
            .await?;
        if !matches!(
            route.provider.snapshot().await.lifecycle,
            crate::LifecycleState::Admitted
                | crate::LifecycleState::Active
                | crate::LifecycleState::Activating
                | crate::LifecycleState::Recovering
        ) {
            return Err(PackageErrorCode::PortUnavailable);
        }
        if let (Some(mut changes), Some(lease)) = (changes, &route.lease) {
            let mut ended = lease.watch();
            let weak = Arc::downgrade(lease);
            let broker = Arc::downgrade(broker);
            let core = self.core.clone();
            let authority = self.authority.clone();
            let limiter = self.limiter.clone();
            let mut cancelled = self.cancelled.subscribe();
            tokio::spawn(async move {
                loop {
                    if ended.borrow().is_some() || *cancelled.borrow() {
                        break;
                    }
                    tokio::select! {
                        change = changes.changed() => if change.is_err() { break; },
                        _ = ended.changed() => break,
                        _ = cancelled.changed() => break,
                    }
                    let Some(broker) = broker.upgrade() else {
                        break;
                    };
                    if weak.strong_count() == 0 {
                        break;
                    }
                    let core = core.clone();
                    let key = key.clone();
                    let _ = authority
                        .run(&limiter, Arc::new(AtomicBool::new(false)), move || {
                            if let Ok(actor) = crate::port_access::actor(&core, &key) {
                                broker.reconcile_binding(&key, &actor);
                            }
                        })
                        .await;
                }
            });
        }
        Ok(route)
    }

    async fn port_selection(
        &self,
        broker: &Arc<crate::broker::PortBroker>,
        method: &str,
        object: &Map<String, Value>,
    ) -> Result<Value, PackageErrorCode> {
        let enable = method == "enable";
        exact_fields(
            object,
            if enable {
                &["method", "contract", "major"]
            } else {
                &["method", "contract"]
            },
        )?;
        let contract = string_field(object, "contract")?.to_owned();
        if !teamarness_package_contract::identity::is_valid_package_identity(&contract) {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let major = if enable {
            Some(
                object
                    .get("major")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                    .filter(|v| *v != 0)
                    .ok_or(PackageErrorCode::CapabilityRequestInvalid)?,
            )
        } else {
            None
        };
        broker
            .select_contract(
                self.core.clone(),
                self.instance_key(),
                &self.authority,
                contract,
                major,
            )
            .await
    }

    /// The Host-authored actor for a Port dispatch by this generation: the
    /// Workspace Agent the calling Package is bound to, otherwise the
    /// Installation Person (ADR 0030). Read once per dispatch; the lookup
    /// never marks the command as started, because nothing has reached the
    /// Provider yet.
    #[cfg(test)]
    async fn port_actor(&self) -> Result<Value, PackageErrorCode> {
        let key = self.instance_key();
        let core = self.core.clone();
        self.authority
            .run(&self.limiter, Arc::new(AtomicBool::new(false)), move || {
                crate::port_access::actor(&core, &key)
            })
            .await?
    }

    async fn immutable_record(
        &self,
        method: &str,
        object: &Map<String, Value>,
        minor: u16,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let workspace = teamarness_core::WorkspaceId::from_str(self.scope.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
        let package = self.scope.package_identity.as_str().to_owned();
        let core = Arc::clone(&self.core);
        match method {
            "append" => {
                if minor == 0 {
                    exact_fields(object, &["method", "purpose", "canonical"])?;
                } else if object.contains_key("principal") {
                    exact_fields(object, &["method", "purpose", "canonical", "principal"])?;
                } else {
                    exact_fields(object, &["method", "purpose", "canonical"])?;
                }
                let principal = object
                    .get("principal")
                    .map(|_| {
                        teamarness_core::PrincipalId::from_str(string_field(object, "principal")?)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
                    })
                    .transpose()?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?.to_owned();
                let canonical = decode_bounded(
                    string_field(object, "canonical")?,
                    MAX_CANONICAL_DECODED_BYTES,
                )?;
                self.limiter
                    .run_append(self.append_fence.clone(), started, move || {
                        let record = core
                            .append_record_as_principal(
                                workspace, &package, &purpose, &canonical, principal,
                            )
                            .map_err(sync_error)?;
                        Ok(json!({
                            "recordId": record.record_id().to_string(),
                            "orderKey": record.order_key().to_string(),
                            "author": record.author().to_string(),
                            "signature": encode_signature(&record.signature())?,
                            "sealedRecord": encode_base64(record.sealed_record()),
                            "durability": record.durability(),
                        }))
                    })
                    .await?
            }
            "open" => {
                exact_fields(object, &["method", "sealedRecords"])?;
                let values = object
                    .get("sealedRecords")
                    .and_then(Value::as_array)
                    .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                if !(1..=MAX_OPEN_RECORDS).contains(&values.len()) {
                    return Err(PackageErrorCode::CapabilityRequestInvalid);
                }
                let mut sealed = Vec::with_capacity(values.len());
                let mut total = 0_usize;
                for value in values {
                    let bytes = decode_bounded(
                        value
                            .as_str()
                            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?,
                        MAX_OPEN_DECODED_BYTES,
                    )?;
                    total = total
                        .checked_add(bytes.len())
                        .filter(|total| *total <= MAX_OPEN_DECODED_BYTES)
                        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                    sealed.push(bytes);
                }
                // Every envelope of one request is opened in one blocking
                // closure, but still one `open_records` call each, so each
                // result is classified independently.
                self.limiter
                    .run(started, move || {
                        let mut results = Vec::with_capacity(sealed.len());
                        for envelope in &sealed {
                            match core.open_records(
                                workspace,
                                &package,
                                std::slice::from_ref(envelope),
                            ) {
                                Ok(records) => {
                                    let record = records
                                        .into_iter()
                                        .next()
                                        .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                                    results.push(json!({
                                        "status":"ready",
                                        "recordId":record.record_id().to_string(),
                                        "orderKey":record.order_key().to_string(),
                                        "author":record.author().to_string(),
                                        "purpose":record.purpose(),
                                        "signature":encode_signature(&record.signature())?,
                                        "canonical":encode_base64(record.canonical()),
                                    }));
                                }
                                Err(error) => {
                                    let status = match error {
                                        teamarness_core::SynchronizationError::WrongScope => {
                                            "wrong_scope"
                                        }
                                        teamarness_core::SynchronizationError::CorruptRecord => {
                                            "corrupt"
                                        }
                                        _ => "waiting_for_history_access",
                                    };
                                    results.push(json!({"status":status}));
                                }
                            }
                        }
                        Ok(json!({"results":results}))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }

    async fn signing(
        &self,
        method: &str,
        object: &Map<String, Value>,
        started: Arc<AtomicBool>,
    ) -> Result<Value, PackageErrorCode> {
        let core = Arc::clone(&self.core);
        match method {
            "sign" => {
                exact_fields(object, &["method", "purpose", "payload"])?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?;
                let payload = decoded_payload(string_field(object, "payload")?)?;
                let envelope = signing_envelope(
                    &self.scope.package_identity,
                    &self.scope.workspace_id,
                    purpose,
                    &payload,
                )?;
                self.limiter
                    .run(started, move || {
                        let context = SigningContext::new(SIGNING_CONTEXT)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let signature = core
                            .sign(&context, &envelope)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        let signature_bytes = decode_hex::<64>(&signature.to_string())?;
                        Ok(json!({
                            "signer":core.public_identity().to_string(),
                            "envelopeVersion":1,
                            "signature":URL_SAFE_NO_PAD.encode(signature_bytes),
                        }))
                    })
                    .await?
            }
            "verify" => {
                exact_fields(
                    object,
                    &["method", "signer", "purpose", "payload", "signature"],
                )?;
                let signer = PublicIdentity::from_str(string_field(object, "signer")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let purpose = validated_purpose(string_field(object, "purpose")?)?;
                let payload = decoded_payload(string_field(object, "payload")?)?;
                let signature = URL_SAFE_NO_PAD
                    .decode(string_field(object, "signature")?)
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                if signature.len() != 64 {
                    return Err(PackageErrorCode::CapabilityRequestInvalid);
                }
                let signature = PublicSignature::from_str(&encode_hex(&signature))
                    .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                let envelope = signing_envelope(
                    &self.scope.package_identity,
                    &self.scope.workspace_id,
                    purpose,
                    &payload,
                )?;
                self.limiter
                    .run(started, move || {
                        let context = SigningContext::new(SIGNING_CONTEXT)
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                        Ok(json!({
                            "valid":signer.verify(&context, &envelope, &signature).is_ok()
                        }))
                    })
                    .await?
            }
            _ => Err(PackageErrorCode::CapabilityRequestInvalid),
        }
    }
}

impl Drop for GenerationCapabilities {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
        if let Some(broker) = self.broker.upgrade() {
            broker.revoke_consumer(
                &self.instance_key(),
                self.generation,
                self.authority.revision(),
            );
        }
    }
}

impl crate::protocol::CapabilityHandler for GenerationCapabilities {
    fn call<'a>(
        &'a self,
        grant: Option<&'a str>,
        payload: &'a Value,
        started: Arc<AtomicBool>,
        consumer_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, CapabilityFailure>> + Send + 'a>,
    > {
        Box::pin(GenerationCapabilities::call(
            self,
            grant,
            payload,
            started,
            consumer_id,
        ))
    }

    fn open_stream<'a>(
        &'a self,
        stream_id: String,
        topic: &'a str,
        initial_credit: u64,
        payload: &'a Value,
        output: HostStreamSender,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), PackageErrorCode>> + Send + 'a>,
    > {
        Box::pin(GenerationCapabilities::open_stream(
            self,
            stream_id,
            topic,
            initial_credit,
            payload,
            output,
        ))
    }

    fn add_credit<'a>(
        &'a self,
        stream_id: &'a str,
        amount: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(GenerationCapabilities::add_credit(self, stream_id, amount))
    }

    fn close_stream<'a>(
        &'a self,
        stream_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(GenerationCapabilities::close_stream(self, stream_id))
    }
}

struct Registry {
    descriptors: BTreeMap<&'static str, Descriptor>,
}

impl Registry {
    fn production() -> Self {
        let mut registry = Self {
            descriptors: BTreeMap::new(),
        };
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.workspace-context"),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::WorkspaceContext,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.membership"),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::Membership,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.principals"),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::Principals,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.agent-binding"),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::AgentBinding,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.signing"),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::Signing,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.package-ports"),
            major: 2,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::PackagePorts,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new(crate::content_views::FAMILY),
            major: 1,
            min_minor: 0,
            max_minor: 0,
            kind: CapabilityKind::ContentViews,
        });
        registry.register(Descriptor {
            family: CapabilityFamily::new("teamarness.synchronization.immutable-record-log"),
            major: 1,
            min_minor: 0,
            max_minor: 1,
            kind: CapabilityKind::ImmutableRecordLog,
        });
        registry
    }

    fn register(&mut self, descriptor: Descriptor) {
        self.descriptors
            .insert(descriptor.family.name(), descriptor);
    }

    /// Selects one version of the requested family, or says why it cannot.
    ///
    /// The family is matched first: a family the Host does not provide is an unavailable
    /// capability, not a version error, so an author who asks for a capability that does not exist
    /// is not told their versions are wrong. Within a family, the highest minor both sides accept
    /// is selected, because a minor may only add.
    fn negotiate(
        &self,
        requirement: &CapabilityRequirement,
    ) -> Result<(Descriptor, u16), PackageErrorCode> {
        let descriptor = *self
            .descriptors
            .get(requirement.family.as_str())
            .ok_or(PackageErrorCode::CapabilityUnavailable)?;
        if descriptor.major != requirement.major {
            return Err(PackageErrorCode::CapabilityVersionUnsupported);
        }
        let low = descriptor.min_minor.max(requirement.min_minor);
        let high = descriptor.max_minor.min(requirement.max_minor);
        if low > high {
            return Err(PackageErrorCode::CapabilityVersionUnsupported);
        }
        Ok((descriptor, high))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum StreamCommand {
    Credit(u64),
    Close,
}

/// Applies every queued stream command. Returns `false` when the stream must
/// end.
async fn drain_commands(commands: &mut mpsc::Receiver<StreamCommand>, credit: &mut u64) -> bool {
    while let Ok(command) = commands.try_recv() {
        match command {
            StreamCommand::Credit(amount) => {
                *credit = credit.saturating_add(amount);
            }
            StreamCommand::Close => return false,
        }
    }
    true
}

/// Waits for credit, a close, or generation cancellation. Returns `false` when
/// the stream must end.
async fn await_credit(
    commands: &mut mpsc::Receiver<StreamCommand>,
    credit: &mut u64,
    cancelled: &mut watch::Receiver<bool>,
) -> bool {
    while *credit == 0 {
        tokio::select! {
            command = commands.recv() => match command {
                Some(StreamCommand::Credit(amount)) => {
                    *credit = credit.saturating_add(amount);
                }
                Some(StreamCommand::Close) | None => return false,
            },
            result = cancelled.changed() => {
                if result.is_err() || *cancelled.borrow() {
                    return false;
                }
            }
        }
    }
    true
}

fn principal_value(principal: &teamarness_core::WorkspacePrincipal) -> Value {
    json!({
        "principalId":principal.id().to_string(),
        "kind":match principal.kind() {
            teamarness_core::PrincipalKind::Person => "person",
            teamarness_core::PrincipalKind::Agent => "agent",
        },
        "stewardPrincipalId":principal.steward_principal_id().map(|id| id.to_string()),
        "keys":principal.keys().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "name":principal.name(),
        "fingerprint":principal.fingerprint(),
    })
}

fn principal_snapshot(
    core: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
) -> Result<Value, PackageErrorCode> {
    let (cursor, principals) = core
        .principals_snapshot(workspace)
        .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
    if principals.len() > 1024 {
        return Err(PackageErrorCode::CapabilityUnavailable);
    }
    Ok(json!({
        "workspaceId":workspace.to_string(),
        "cursor":cursor.to_string(),
        "principals":principals.iter().map(principal_value).collect::<Vec<_>>(),
    }))
}

/// One bound-Agent row: the Principal ID, its current signing key and name.
fn bound_agent_value(agent: &teamarness_core::WorkspacePrincipal) -> Value {
    json!({
        "principalId":agent.id().to_string(),
        "identity":agent.keys().first().map(ToString::to_string),
        "name":agent.name(),
    })
}

/// `{agent}` for the calling Package: `null` when unbound or the bound Agent
/// is no longer current.
fn current_binding(
    core: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
    package: &str,
) -> Result<Value, PackageErrorCode> {
    let agent = core
        .bound_agent(workspace, package)
        .map_err(binding_error)?
        .map(|id| core.principal(workspace, id).map_err(binding_error))
        .transpose()?
        .flatten();
    Ok(json!({"agent":agent.as_ref().map(bound_agent_value)}))
}

/// Core's binding and registry errors in the capability's own vocabulary.
pub(crate) fn binding_error(error: teamarness_core::PersonNameError) -> PackageErrorCode {
    match error {
        teamarness_core::PersonNameError::NotOwnPerson => PackageErrorCode::AgentNotStewarded,
        teamarness_core::PersonNameError::AgentNotFound => PackageErrorCode::AgentNotFound,
        teamarness_core::PersonNameError::InvalidName
        | teamarness_core::PersonNameError::InvalidPackageIdentity
        | teamarness_core::PersonNameError::WorkspaceNotFound => {
            PackageErrorCode::CapabilityRequestInvalid
        }
        teamarness_core::PersonNameError::Membership(_)
        | teamarness_core::PersonNameError::Storage(_) => PackageErrorCode::CapabilityUnavailable,
    }
}

// A profile cursor invalidates a snapshot; it is not a replay ledger. A lagging
// consumer receives the latest bounded snapshot and resumes at its cursor.
async fn run_port_query(
    id: String,
    mut query: PortQueryStream,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let mut revocation = query.receiver.revocation();
    let mut terminal = query.receiver.terminal();
    let mut credit = MAX_STREAM_CREDIT;
    let mut sequence = 0;
    let reason = loop {
        if *cancelled.borrow() || !drain_commands(&mut commands, &mut credit).await {
            break "closed";
        }
        if credit == 0 {
            if let Some(code) = revocation.as_ref().and_then(|watch| *watch.borrow()) {
                break crate::protocol::capability_code(code);
            }
            if let Some(end) = terminal.borrow().clone() {
                break match end {
                    StreamEnd::Completed => "completed",
                    StreamEnd::Host(code) => crate::protocol::capability_code(code),
                    _ => "provider_exited",
                };
            }
        }
        tokio::select! {
            biased;
            _ = async {
                if let Some(watch) = &mut revocation { let _ = watch.changed().await; }
                else { std::future::pending::<()>().await; }
            }, if credit == 0 => {},
            _ = terminal.changed(), if credit == 0 => {},
            change = cancelled.changed() => if change.is_err() || *cancelled.borrow() { break "closed"; },
            command = commands.recv() => match command {
                Some(StreamCommand::Credit(amount)) => credit += amount,
                Some(StreamCommand::Close) | None => break "closed",
            },
            event = query.receiver.changed(), if credit > 0 => match event {
                Ok(event) => {
                    sequence += 1;
                    credit -= 1;
                    let value = match serde_json::to_value(event.payload) {
                        Ok(value) => value,
                        Err(_) => break "port_event_invalid",
                    };
                    if output.event(&id, sequence, value).await.is_err() { break "process_exited"; }
                    if query.client.credit(&query.id, 1).await.is_err() { break "process_exited"; }
                }
                Err(StreamEnd::Completed) => break "completed",
                Err(StreamEnd::Host(PackageErrorCode::PortEventInvalid)) => break "port_event_invalid",
                Err(StreamEnd::Host(PackageErrorCode::Overloaded)) => break "overloaded",
                Err(StreamEnd::Host(PackageErrorCode::PortNotEnabled)) => break "port_not_enabled",
                Err(StreamEnd::Host(PackageErrorCode::AgentNotBound)) => break "agent_not_bound",
                Err(StreamEnd::Host(PackageErrorCode::AgentBindingChanged)) => break "agent_binding_changed",
                Err(_) => break "provider_exited",
            },
        }
    };
    let _ = query.client.stream_close(&query.id).await;
    output.retire(&id).await;
    streams.lock().await.remove(&id);
    let _ = output.close(&id, reason).await;
}

#[allow(clippy::too_many_arguments)]
async fn run_principal_stream(
    core: Arc<CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    stream_id: String,
    mut after_cursor: u64,
    mut credit: u64,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let watch_core = core.clone();
    let opened =
        tokio::task::spawn_blocking(move || watch_core.subscribe_principals(workspace)).await;
    let mut changes = match opened {
        Ok(Ok(receiver)) => receiver,
        _ => {
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            let _ = output.close(&stream_id, "capability_unavailable").await;
            return;
        }
    };
    let mut sequence = 0_u64;
    let mut reason = "capability_unavailable";
    loop {
        if *cancelled.borrow()
            || !drain_commands(&mut commands, &mut credit).await
            || !await_credit(&mut commands, &mut credit, &mut cancelled).await
        {
            break;
        }
        if *changes.borrow_and_update() < after_cursor {
            reason = "cursor_invalid";
            break;
        }
        if *changes.borrow() <= after_cursor {
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { break; },
                command = commands.recv() => match command {
                    Some(StreamCommand::Credit(amount)) => credit = credit.saturating_add(amount),
                    Some(StreamCommand::Close) | None => break,
                },
                result = cancelled.changed() => if result.is_err() || *cancelled.borrow() { break; },
            }
            continue;
        }
        let snapshot_core = core.clone();
        let snapshot =
            tokio::task::spawn_blocking(move || principal_snapshot(&snapshot_core, workspace))
                .await;
        let Ok(Ok(snapshot)) = snapshot else {
            break;
        };
        let Some(cursor) = snapshot["cursor"]
            .as_str()
            .and_then(|text| text.parse::<u64>().ok())
        else {
            break;
        };
        if cursor <= after_cursor {
            continue;
        }
        sequence = sequence.saturating_add(1);
        if output.event(&stream_id, sequence, snapshot).await.is_err() {
            break;
        }
        after_cursor = cursor;
        credit -= 1;
    }
    output.retire(&stream_id).await;
    streams.lock().await.remove(&stream_id);
    let _ = output.close(&stream_id, reason).await;
}

#[allow(clippy::too_many_arguments)]
async fn run_membership_stream(
    core: Arc<CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    stream_id: String,
    mut after_epoch: u64,
    mut credit: u64,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let watch_core = core.clone();
    let opened =
        tokio::task::spawn_blocking(move || watch_core.membership_epoch_watch(workspace)).await;
    let mut epochs = match opened {
        Ok(Ok(receiver)) => receiver,
        _ => {
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            let _ = output.close(&stream_id, "capability_unavailable").await;
            return;
        }
    };
    let mut sequence = 0_u64;
    loop {
        if *cancelled.borrow() {
            break;
        }
        if !drain_commands(&mut commands, &mut credit).await
            || !await_credit(&mut commands, &mut credit, &mut cancelled).await
        {
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            return;
        }
        // The epoch is the only thing that says a transition exists, so the
        // stream waits on it instead of reopening a subscription on a timer.
        if *epochs.borrow_and_update() <= after_epoch {
            tokio::select! {
                changed = epochs.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                command = commands.recv() => match command {
                    Some(StreamCommand::Credit(amount)) => {
                        credit = credit.saturating_add(amount);
                    }
                    Some(StreamCommand::Close) | None => {
                        output.retire(&stream_id).await;
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                },
                result = cancelled.changed() => {
                    if result.is_err() || *cancelled.borrow() {
                        output.retire(&stream_id).await;
                        streams.lock().await.remove(&stream_id);
                        return;
                    }
                }
            }
            continue;
        }
        let next_core = core.clone();
        let next = tokio::task::spawn_blocking(move || {
            let mut subscription = next_core
                .subscribe_membership_bounded(workspace, MembershipEpoch::new(after_epoch), 1)
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
            subscription
                .next_transition_timeout(std::time::Duration::ZERO)
                .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
        })
        .await;
        let transition = match next {
            Ok(Ok(Some(transition))) => transition,
            Ok(Ok(None)) => continue,
            _ => break,
        };
        sequence = sequence.saturating_add(1);
        let payload = json!({
            "epoch":transition.epoch().value().to_string(),
            "membershipVersion":transition.version().to_string(),
            "cause":transition.cause().to_string(),
            "snapshotDigest":transition.snapshot_digest().to_string(),
        });
        if output.event(&stream_id, sequence, payload).await.is_err() {
            break;
        }
        after_epoch = transition.epoch().value();
        credit -= 1;
    }
    output.retire(&stream_id).await;
    streams.lock().await.remove(&stream_id);
    let _ = output.close(&stream_id, "capability_unavailable").await;
}

fn sync_error(error: teamarness_core::SynchronizationError) -> PackageErrorCode {
    match error {
        teamarness_core::SynchronizationError::NotCurrentMember
        | teamarness_core::SynchronizationError::RevokedMember
        // The Package asked for something this Installation cannot serve yet,
        // not something malformed: the scope's content key has not arrived.
        | teamarness_core::SynchronizationError::ContentKeyUnavailable
        | teamarness_core::SynchronizationError::KeyOriginRejected
        // A scope isolated by a local integrity failure is unavailable until
        // the Installation is reset; the request itself was well formed.
        | teamarness_core::SynchronizationError::ScopeQuarantined => {
            PackageErrorCode::CapabilityUnavailable
        }
        teamarness_core::SynchronizationError::PrincipalNotBound => PackageErrorCode::PrincipalNotBound,
        teamarness_core::SynchronizationError::WorkspaceNotFound
        | teamarness_core::SynchronizationError::InvalidInput
        | teamarness_core::SynchronizationError::WrongScope
        | teamarness_core::SynchronizationError::CorruptRecord
        | teamarness_core::SynchronizationError::CursorScopeMismatch
        | teamarness_core::SynchronizationError::CursorAhead => {
            PackageErrorCode::CapabilityRequestInvalid
        }
        teamarness_core::SynchronizationError::RuntimeUnavailable
        | teamarness_core::SynchronizationError::Storage(_) => {
            PackageErrorCode::CapabilityUnavailable
        }
        _ => PackageErrorCode::CapabilityRequestInvalid,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn encode_signature(
    signature: &teamarness_core::PublicSignature,
) -> Result<String, PackageErrorCode> {
    Ok(encode_base64(&decode_hex::<64>(&signature.to_string())?))
}

fn decode_bounded(value: &str, max: usize) -> Result<Vec<u8>, PackageErrorCode> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(bytes)
}

fn optional_cursor(
    value: Option<&Value>,
) -> Result<Option<teamarness_core::Cursor>, PackageErrorCode> {
    match value {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(value)) => value
            .parse()
            .map(Some)
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid),
        _ => Err(PackageErrorCode::CapabilityRequestInvalid),
    }
}

/// What the stream has already told the Package about its availability.
struct StateTracking {
    last: Option<(teamarness_core::SyncAvailability, u64)>,
    /// Records sent since the last state event.
    since_state: usize,
    /// Codes already sent once on this stream.
    sent: std::collections::BTreeSet<&'static str>,
}

impl StateTracking {
    fn new() -> Self {
        Self {
            last: None,
            since_state: 0,
            sent: std::collections::BTreeSet::new(),
        }
    }

    /// Whether this availability is worth another state event.
    fn should_emit(&self, reported: &teamarness_core::RecordAvailability) -> bool {
        let catching_up = reported.availability == teamarness_core::SyncAvailability::CatchingUp;
        match self.last {
            None => true,
            Some((availability, pending)) => {
                availability != reported.availability
                    || (!catching_up && pending != reported.pending_records)
                    || (catching_up && self.since_state >= RECORDS_PER_STATE)
            }
        }
    }
}

/// While catching up, a state event is sent at most every this many records.
const RECORDS_PER_STATE: usize = 128;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_record_stream(
    core: Arc<CoreRuntime>,
    workspace: teamarness_core::WorkspaceId,
    package: String,
    stream_id: String,
    after: Option<teamarness_core::Cursor>,
    mut credit: u64,
    mut commands: mpsc::Receiver<StreamCommand>,
    output: HostStreamSender,
    streams: Arc<Mutex<BTreeMap<String, mpsc::Sender<StreamCommand>>>>,
    mut cancelled: watch::Receiver<bool>,
    negotiated_minor: u16,
    #[cfg(feature = "test-hooks")] mut availability_pause: Option<Arc<crate::AuthorityPause>>,
) {
    let subscribe_core = core.clone();
    let subscribe_package = package.clone();
    let opened = tokio::task::spawn_blocking(move || {
        subscribe_core.subscribe_records(workspace, &subscribe_package, after)
    })
    .await;
    let mut subscription = match opened {
        Ok(Ok(subscription)) => subscription,
        // A cursor from another journal, or beyond this one, is the Package's
        // to reset: it clears the cursor and replays by record identity.
        Ok(Err(
            teamarness_core::SynchronizationError::CursorAhead
            | teamarness_core::SynchronizationError::CursorScopeMismatch,
        )) => {
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            let _ = output.close(&stream_id, "cursor_invalid").await;
            return;
        }
        Ok(Err(error)) => {
            eprintln!("record stream subscribe failed: stream={stream_id} error={error}");
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            let _ = output.close(&stream_id, "capability_unavailable").await;
            return;
        }
        Err(error) => {
            eprintln!("record stream subscribe task failed: stream={stream_id} error={error}");
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            let _ = output.close(&stream_id, "capability_unavailable").await;
            return;
        }
    };
    // Page and availability reads advance the subscription's read cursor. Keep
    // notifications independent so a health change between reads cannot be lost.
    let mut changes = subscription.changes();
    let mut sequence = 0_u64;
    let mut pending_replay: Option<teamarness_core::InitialReplayBoundary> = None;
    let mut tracking = StateTracking::new();
    // The first event of every stream is a state event.
    let mut need_state = true;
    let mut refreshed_after_empty = false;
    loop {
        if *cancelled.borrow() {
            break;
        }
        if !drain_commands(&mut commands, &mut credit).await
            || !await_credit(&mut commands, &mut credit, &mut cancelled).await
        {
            output.retire(&stream_id).await;
            streams.lock().await.remove(&stream_id);
            return;
        }
        if let Some(boundary) = pending_replay.take() {
            if negotiated_minor >= 1
                && let Some(cursor) = boundary.cursor
            {
                sequence = sequence.saturating_add(1);
                let payload = json!({"kind":"replay_end","cursor":cursor.to_string(),"gapFree":boundary.gap_free});
                if output.event(&stream_id, sequence, payload).await.is_err() {
                    break;
                }
                credit -= 1;
            }
            continue;
        }
        if need_state {
            let read = tokio::task::spawn_blocking(move || {
                let reported = subscription.availability();
                (subscription, reported)
            })
            .await;
            let reported = match read {
                Ok((returned, Ok(reported))) => {
                    subscription = returned;
                    reported
                }
                Ok((_returned, Err(error))) => {
                    eprintln!("record stream availability failed: stream={stream_id} {error}");
                    break;
                }
                Err(error) => {
                    eprintln!("record stream availability task failed: stream={stream_id} {error}");
                    break;
                }
            };
            let mut delivered = true;
            for warning in &reported.warnings {
                let code = warning.as_str();
                // A record quarantine is reported once per page that skipped
                // one; the other codes are reported once per stream.
                let once_per_stream = *warning != teamarness_core::SyncWarning::RecordQuarantined;
                if credit == 0 || (once_per_stream && !tracking.sent.insert(code)) {
                    continue;
                }
                sequence = sequence.saturating_add(1);
                let payload = json!({"kind":"warning","code":code});
                if output.event(&stream_id, sequence, payload).await.is_err() {
                    delivered = false;
                    break;
                }
                credit -= 1;
            }
            if !delivered {
                break;
            }
            if credit > 0 && tracking.should_emit(&reported) {
                sequence = sequence.saturating_add(1);
                let payload = json!({
                    "kind":"state",
                    "availability":reported.availability.as_str(),
                    "pendingRecordCount":reported.pending_records.to_string(),
                });
                if output.event(&stream_id, sequence, payload).await.is_err() {
                    break;
                }
                credit -= 1;
                tracking.last = Some((reported.availability, reported.pending_records));
                tracking.since_state = 0;
            }
            need_state = false;
            #[cfg(feature = "test-hooks")]
            if refreshed_after_empty && let Some(pause) = availability_pause.take() {
                pause.wait().await;
            }
            continue;
        }
        let limit = usize::try_from(credit.min(RECORD_PAGE)).unwrap_or(1);
        let read = tokio::task::spawn_blocking(move || {
            let page = subscription.next_page(limit);
            (subscription, page)
        })
        .await;
        let page = match read {
            Ok((returned, Ok(page))) => {
                subscription = returned;
                page
            }
            Ok((_returned, Err(error))) => {
                eprintln!("record stream read failed: stream={stream_id} error={error}");
                break;
            }
            Err(error) => {
                eprintln!("record stream read task failed: stream={stream_id} error={error}");
                break;
            }
        };
        if page.quarantined_skipped > 0 {
            need_state = true;
        }
        if negotiated_minor >= 1 {
            pending_replay = page.initial_replay;
        }
        let mut delivered = true;
        for event in &page.events {
            let Ok(signature) = encode_signature(&event.record().signature()) else {
                delivered = false;
                break;
            };
            sequence = sequence.saturating_add(1);
            let payload = json!({
                "kind":"record",
                "cursor":event.cursor().to_string(),
                "recordId":event.record_id().to_string(),
                "orderKey":event.order_key().to_string(),
                "author":event.author().to_string(),
                "purpose":event.purpose(),
                "signature":signature,
                "canonical":encode_base64(event.canonical()),
                "sealedRecord":encode_base64(event.sealed_record()),
            });
            if output.event(&stream_id, sequence, payload).await.is_err() {
                eprintln!("record stream delivery failed: stream={stream_id} sequence={sequence}");
                delivered = false;
                break;
            }
            credit -= 1;
        }
        if !delivered {
            break;
        }
        if !page.events.is_empty() {
            tracking.since_state = tracking.since_state.saturating_add(page.events.len());
            if tracking.since_state >= RECORDS_PER_STATE {
                need_state = true;
            }
            refreshed_after_empty = false;
            continue;
        }
        // Nothing was ready. The availability is re-read once, so a stream that
        // has caught up says so, and then the subscription waits on the journal
        // head rather than polling storage.
        if !refreshed_after_empty {
            refreshed_after_empty = true;
            need_state = true;
            continue;
        }
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    break;
                }
                need_state = true;
                refreshed_after_empty = false;
            }
            command = commands.recv() => match command {
                Some(StreamCommand::Credit(amount)) => {
                    credit = credit.saturating_add(amount);
                }
                Some(StreamCommand::Close) | None => {
                    output.retire(&stream_id).await;
                    streams.lock().await.remove(&stream_id);
                    return;
                }
            },
            result = cancelled.changed() => {
                if result.is_err() || *cancelled.borrow() {
                    output.retire(&stream_id).await;
                    streams.lock().await.remove(&stream_id);
                    return;
                }
            }
        }
    }
    output.retire(&stream_id).await;
    streams.lock().await.remove(&stream_id);
    let _ = output.close(&stream_id, "capability_unavailable").await;
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), PackageErrorCode> {
    if object.len() == expected.len() && object.keys().all(|key| expected.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(PackageErrorCode::CapabilityRequestInvalid)
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, PackageErrorCode> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or(PackageErrorCode::CapabilityRequestInvalid)
}

fn decimal_field(object: &Map<String, Value>, name: &str) -> Result<u64, PackageErrorCode> {
    let value = string_field(object, name)?;
    if value != "0" && value.starts_with('0') {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    value
        .parse()
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
}

fn signed_decimal_field(object: &Map<String, Value>, name: &str) -> Result<i64, PackageErrorCode> {
    let value = string_field(object, name)?;
    if value.starts_with('+') || value == "-0" || (value.starts_with('0') && value.len() > 1) {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    value
        .parse()
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
}

fn validated_purpose(value: &str) -> Result<&str, PackageErrorCode> {
    if value.is_empty()
        || value.len() > MAX_PURPOSE_BYTES
        || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(value)
}

fn decoded_payload(value: &str) -> Result<Vec<u8>, PackageErrorCode> {
    let payload = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    if payload.len() > MAX_SIGNING_PAYLOAD_BYTES {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    Ok(payload)
}

fn signing_envelope(
    package: &PackageIdentity,
    workspace: &WorkspaceId,
    purpose: &str,
    payload: &[u8],
) -> Result<Vec<u8>, PackageErrorCode> {
    let package_length = u16::try_from(package.as_str().len())
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let workspace_length = u16::try_from(workspace.as_str().len())
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let purpose_length =
        u16::try_from(purpose.len()).map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let mut envelope = Vec::with_capacity(
        2 + 2
            + 2
            + package.as_str().len()
            + 2
            + workspace.as_str().len()
            + 2
            + purpose.len()
            + 4
            + payload.len(),
    );
    envelope.extend_from_slice(&1_u16.to_be_bytes());
    envelope.extend_from_slice(&0_u16.to_be_bytes());
    envelope.extend_from_slice(&package_length.to_be_bytes());
    envelope.extend_from_slice(package.as_str().as_bytes());
    envelope.extend_from_slice(&workspace_length.to_be_bytes());
    envelope.extend_from_slice(workspace.as_str().as_bytes());
    envelope.extend_from_slice(&purpose_length.to_be_bytes());
    envelope.extend_from_slice(purpose.as_bytes());
    envelope.extend_from_slice(&payload_length.to_be_bytes());
    envelope.extend_from_slice(payload);
    Ok(envelope)
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], PackageErrorCode> {
    if value.len() != N * 2 {
        return Err(PackageErrorCode::CapabilityRequestInvalid);
    }
    let mut bytes = [0_u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    }
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

    /// Binds the immutable-record capability against a real Core runtime with
    /// one Workspace, and returns the generation and its grant.
    async fn bound_immutable_record(
        core: &Arc<CoreRuntime>,
        limiter: CoreCallLimiter,
    ) -> (Arc<GenerationCapabilities>, String) {
        let workspace = core.create_workspace("Capability limiter").unwrap();
        let capabilities = GenerationCapabilities::bind(
            Arc::clone(core),
            CapabilityScope {
                package_identity: "dev.teamarness.fixture".parse().unwrap(),
                workspace_id: crate::WorkspaceId::new(workspace.id().to_string()).unwrap(),
                workspace_name: workspace.name().to_owned(),
                installation: InstallationPublicIdentity::new(core.public_identity().to_string())
                    .unwrap(),
                data_directory: PathBuf::new(),
                content_views: Arc::default(),
            },
            &[CapabilityRequirement {
                family: "teamarness.synchronization.immutable-record-log".into(),
                major: 1,
                min_minor: 0,
                max_minor: 0,
            }],
            1,
            limiter,
            GenerationGuards {
                append_fence: Arc::new(AppendFence::default()),
                authority: crate::authority::AuthorityGeneration::default()
                    .advance()
                    .await
                    .unwrap(),
            },
            std::sync::Weak::new(),
        )
        .unwrap();
        let grant = capabilities.advertised()[0].grant.clone();
        (capabilities, grant)
    }

    /// Verifies: TEAM.SYS.PKG-R13
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn replaced_generation_rejects_late_actor_bind_and_unbind() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(
            CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(
                directory.path().join("core"),
            ))
            .await
            .unwrap(),
        );
        let workspace = core.create_workspace("Authority generations").unwrap();
        let a = core.add_agent(workspace.id(), "Scout").unwrap();
        let b = core.add_agent(workspace.id(), "Other").unwrap();
        let package: PackageIdentity = "dev.teamarness.fixture".parse().unwrap();
        let key = crate::PackageInstanceKey::new(
            package.clone(),
            WorkspaceId::new(workspace.id().to_string()).unwrap(),
        );
        let broker = crate::broker::empty_broker().await;
        let scope = CapabilityScope {
            package_identity: package.clone(),
            workspace_id: key.workspace_id.clone(),
            workspace_name: "test".into(),
            installation: InstallationPublicIdentity::new(core.public_identity().to_string())
                .unwrap(),
            data_directory: PathBuf::new(),
            content_views: Arc::default(),
        };
        let old = GenerationCapabilities::bind(
            core.clone(),
            scope.clone(),
            &[],
            1,
            CoreCallLimiter::new(),
            GenerationGuards {
                append_fence: Arc::new(AppendFence::default()),
                authority: broker.authority_generation(&key).advance().await.unwrap(),
            },
            Arc::downgrade(&broker),
        )
        .unwrap();
        core.bind_agent_package(workspace.id(), a, package.as_str())
            .unwrap();
        assert_eq!(old.port_actor().await.unwrap()["principal"], a.to_string());
        // Re-fetch as a fresh ProcessSpec constructor would, even with a reset process counter.
        let new = GenerationCapabilities::bind(
            core.clone(),
            scope,
            &[],
            1,
            CoreCallLimiter::new(),
            GenerationGuards {
                append_fence: Arc::new(AppendFence::default()),
                authority: broker.authority_generation(&key).advance().await.unwrap(),
            },
            Arc::downgrade(&broker),
        )
        .unwrap();
        new.agent_binding(
            "bind",
            json!({"method":"bind","principalId":b.to_string()})
                .as_object()
                .unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        assert_eq!(
            old.port_actor().await.unwrap_err(),
            PackageErrorCode::Cancelled
        );
        for (method, request) in [
            ("bind", json!({"method":"bind","principalId":a.to_string()})),
            ("unbind", json!({"method":"unbind"})),
        ] {
            assert_eq!(
                old.agent_binding(
                    method,
                    request.as_object().unwrap(),
                    Arc::new(AtomicBool::new(false))
                )
                .await
                .unwrap_err(),
                PackageErrorCode::Cancelled
            );
            assert_eq!(
                core.bound_agent(workspace.id(), package.as_str()).unwrap(),
                Some(b)
            );
        }
        assert_eq!(new.port_actor().await.unwrap()["principal"], b.to_string());
        core.shutdown().await.unwrap();
    }

    fn append_request() -> Value {
        json!({"method":"append","purpose":"message.post","canonical":"aGVsbG8"})
    }

    #[tokio::test]
    async fn retired_append_fence_cancels_queued_work_and_waits_for_started_closure() {
        let fence = Arc::new(AppendFence::default());
        let entered = fence.enter().expect("first append entered Core closure");
        let drain = tokio::spawn({
            let fence = fence.clone();
            async move { fence.retire(Duration::from_secs(2)).await }
        });
        let limit = std::time::Instant::now() + Duration::from_secs(1);
        while !fence.state.lock().unwrap().retired {
            assert!(
                std::time::Instant::now() < limit,
                "retirement did not close admission"
            );
            tokio::task::yield_now().await;
        }
        assert!(
            fence.enter().is_none(),
            "a queued append cannot enter after retirement"
        );
        assert!(
            !drain.is_finished(),
            "started append must drain through closure exit"
        );
        drop(entered);
        assert!(drain.await.unwrap());
    }

    #[tokio::test]
    async fn append_fence_timeout_does_not_reopen_admission() {
        let fence = Arc::new(AppendFence::default());
        let entered = fence.enter().unwrap();
        assert!(!fence.retire(Duration::from_millis(1)).await);
        assert!(fence.enter().is_none());
        drop(entered);
        assert!(fence.retire(Duration::from_secs(1)).await);
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "current_thread")]
    async fn capability_call_does_not_block_the_executor_while_core_is_locked() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(
            CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(
                directory.path().join("core"),
            ))
            .await
            .unwrap(),
        );
        let (capabilities, grant) = bound_immutable_record(&core, CoreCallLimiter::new()).await;

        // Core is busy: the append cannot finish until the lock is released.
        let hold = core.hold_state_for_tests();
        let calling = Arc::clone(&capabilities);
        let call = tokio::spawn(async move {
            calling
                .call(
                    Some(&grant),
                    &append_request(),
                    Arc::new(AtomicBool::new(false)),
                    "p:test",
                )
                .await
        });
        let (finished, sibling) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let _ = finished.send(());
        });
        let progressed = tokio::time::timeout(Duration::from_secs(5), sibling)
            .await
            .is_ok_and(|delivered| delivered.is_ok());
        hold.release();
        assert!(
            progressed,
            "the capability call held the executor while Core was locked"
        );
        let appended = call.await.unwrap().expect("the append completes");
        assert_eq!(
            appended.get("durability").and_then(Value::as_str),
            Some("committed")
        );
        drop(capabilities);
        core.shutdown().await.unwrap();
    }

    /// Verifies: TEAM.SYS.TRANS-R07
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn core_call_limiter_bounds_concurrent_blocking_work() {
        let limiter = CoreCallLimiter::with_permits(2);
        let started = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((std::sync::Mutex::new(false), Condvar::new()));
        let mut running = Vec::new();
        for _ in 0..3 {
            let limiter = limiter.clone();
            let started = Arc::clone(&started);
            let gate = Arc::clone(&gate);
            running.push(tokio::spawn(async move {
                limiter
                    .run(Arc::new(AtomicBool::new(false)), move || {
                        started.fetch_add(1, Ordering::AcqRel);
                        let (open, waiting) = &*gate;
                        let mut open = open.lock().unwrap();
                        while !*open {
                            open = waiting.wait(open).unwrap();
                        }
                    })
                    .await
            }));
        }
        // Two permits, so exactly two closures can be inside the pool; the
        // third cannot start until one of them returns, which needs the gate.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while started.load(Ordering::Acquire) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "two closures never started"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(started.load(Ordering::Acquire), 2);

        let (open, waiting) = &*gate;
        *open.lock().unwrap() = true;
        waiting.notify_all();
        for handle in running {
            handle.await.unwrap().unwrap();
        }
        assert_eq!(started.load(Ordering::Acquire), 3);
    }

    #[test]
    fn canonical_signing_envelope_has_a_stable_golden() {
        let envelope = signing_envelope(
            &"dev.teamarness.fixture".parse().unwrap(),
            &WorkspaceId::new("00112233445566778899aabbccddeeff").unwrap(),
            "fixture.read",
            b"hello",
        )
        .unwrap();
        let golden: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/signing-envelope.golden.json"
        ))
        .unwrap();
        assert_eq!(
            encode_hex(&envelope),
            golden["envelopeHex"].as_str().unwrap()
        );
    }

    /// A second family, provided only to tests, standing in for a later capability shape such as
    /// document-oriented synchronization.
    fn with_second_family(min_minor: u16, max_minor: u16) -> Registry {
        let mut registry = Registry::production();
        registry.register(Descriptor {
            family: CapabilityFamily::new("example.neutral"),
            major: 1,
            min_minor,
            max_minor,
            kind: CapabilityKind::Neutral,
        });
        registry
    }

    fn requirement(
        family: &str,
        major: u16,
        min_minor: u16,
        max_minor: u16,
    ) -> CapabilityRequirement {
        CapabilityRequirement {
            family: family.into(),
            major,
            min_minor,
            max_minor,
        }
    }

    #[test]
    fn record_log_v11_negotiates_without_changing_v10_consumers() {
        let registry = Registry::production();
        assert_eq!(
            registry
                .negotiate(&requirement(
                    "teamarness.synchronization.immutable-record-log",
                    1,
                    0,
                    0
                ))
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            registry
                .negotiate(&requirement(
                    "teamarness.synchronization.immutable-record-log",
                    1,
                    1,
                    1
                ))
                .unwrap()
                .1,
            1
        );
        assert_eq!(
            registry
                .negotiate(&requirement(
                    "teamarness.synchronization.immutable-record-log",
                    1,
                    0,
                    1
                ))
                .unwrap()
                .1,
            1
        );
    }

    #[test]
    fn registry_accepts_a_neutral_test_only_extension() {
        let (descriptor, minor) = with_second_family(0, 0)
            .negotiate(&requirement("example.neutral", 1, 0, 0))
            .unwrap();
        assert_eq!(descriptor.family.name(), "example.neutral");
        assert_eq!(minor, 0);
    }

    #[test]
    fn negotiates_the_highest_common_minor() {
        let registry = with_second_family(0, 2);
        assert_eq!(
            registry
                .negotiate(&requirement("example.neutral", 1, 1, 3))
                .unwrap()
                .1,
            2
        );
        assert_eq!(
            registry
                .negotiate(&requirement("example.neutral", 1, 0, 0))
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            registry
                .negotiate(&requirement("example.neutral", 1, 3, 4))
                .unwrap_err(),
            PackageErrorCode::CapabilityVersionUnsupported
        );
        assert_eq!(
            registry
                .negotiate(&requirement("example.neutral", 2, 0, 2))
                .unwrap_err(),
            PackageErrorCode::CapabilityVersionUnsupported
        );
    }

    #[test]
    fn package_ports_only_negotiates_the_current_major() {
        let registry = Registry::production();
        assert_eq!(
            registry
                .negotiate(&requirement("teamarness.package-ports", 1, 0, 0))
                .unwrap_err(),
            PackageErrorCode::CapabilityVersionUnsupported
        );
        assert_eq!(
            registry
                .negotiate(&requirement("teamarness.package-ports", 2, 0, 0))
                .unwrap()
                .0
                .major,
            2
        );
    }

    #[test]
    fn unknown_capability_family_is_not_a_version_error() {
        // A family the Host does not provide, asked for with versions the Host would otherwise
        // accept: the answer names the missing capability, not the versions.
        assert_eq!(
            Registry::production()
                .negotiate(&requirement("teamarness.synchronization.document", 1, 0, 0))
                .unwrap_err(),
            PackageErrorCode::CapabilityUnavailable
        );
        assert_eq!(
            Registry::production()
                .negotiate(&requirement(
                    "teamarness.synchronization.immutable-record-log",
                    1,
                    0,
                    0
                ))
                .unwrap()
                .1,
            0
        );
    }

    #[test]
    fn adding_a_capability_family_does_not_change_lifecycle_or_ui_contracts() {
        let production = Registry::production();
        let extended = with_second_family(0, 0);

        // Every family the Host already provides negotiates to exactly what it did before.
        for family in [
            "teamarness.workspace-context",
            "teamarness.membership",
            "teamarness.signing",
            "teamarness.synchronization.immutable-record-log",
        ] {
            let before = production.negotiate(&requirement(family, 1, 0, 0)).unwrap();
            let after = extended.negotiate(&requirement(family, 1, 0, 0)).unwrap();
            assert_eq!(before.0.family, after.0.family);
            assert_eq!(before.0.major, after.0.major);
            assert_eq!(before.1, after.1);
        }
        // The new family is granted, and nothing the Package Process Protocol, the lifecycle
        // contract or the UI bridge declares moves with it.
        assert_eq!(
            extended
                .negotiate(&requirement("example.neutral", 1, 0, 0))
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            teamarness_package_contract::manifest::SUPPORTED_PROTOCOL,
            (1, 0),
            "adding a capability family must not move the Package Process Protocol version"
        );
        // The lifecycle contract and the UI bridge are declared elsewhere and unchanged: a UI
        // contract compiles to the same verdicts with the extra family registered.
        let contract = crate::ui::compile_contract(
            br#"{"schemaVersion":1,"$defs":{},"requests":{"fixture.read":{"request":{"type":"object","additionalProperties":false},"response":{"type":"object","additionalProperties":false}}},"subscriptions":{}}"#,
        )
        .unwrap();
        assert!(
            contract
                .validate_request(
                    "fixture.read",
                    &crate::PackageValue::Object(Default::default())
                )
                .is_ok()
        );
        assert!(
            contract
                .validate_request(
                    "fixture.missing",
                    &crate::PackageValue::Object(Default::default())
                )
                .is_err()
        );
    }

    /// Rule 2's floor: one page of budgeted records fits the writer's data lane.
    ///
    /// The page the Host reads and the budget it writes into were chosen
    /// independently, and a page that cannot fit was reported to the Package as
    /// `capability_unavailable` — a code the SDK's own table calls permanent.
    #[test]
    fn record_page_fits_the_writer_budget() {
        // base64url without padding, twice: the canonical bytes and the sealed form.
        let encoded = |bytes: usize| (bytes * 4).div_ceil(3);
        let page = RECORD_PAGE as usize * 2 * encoded(BUDGETED_RECORD_BYTES);
        assert!(
            page <= crate::protocol::DATA_BYTES,
            "a page of {RECORD_PAGE} records at {BUDGETED_RECORD_BYTES} bytes is {page} wire bytes, over the {} byte data budget",
            crate::protocol::DATA_BYTES
        );
        assert_eq!(RECORD_PAGE, MAX_STREAM_CREDIT);
        assert_eq!(MAX_STREAM_CREDIT as usize, INITIAL_STREAM_CREDIT);
        // A page of contract-maximum records does not fit any sane budget, and
        // is what `HostStreamSender::event` waits for capacity to write.
        assert!(
            RECORD_PAGE as usize * 2 * encoded(MAX_CANONICAL_DECODED_BYTES)
                > crate::protocol::DATA_BYTES
        );
    }

    #[test]
    fn capability_limits_match_contract_schemas() {
        // The Host owns these bounds; the schemas publish them. Both are read here so neither can
        // move alone.
        let record_log: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/immutable-record-log.schema.json"
        ))
        .unwrap();
        let signing: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/signing.schema.json"
        ))
        .unwrap();
        let agent_binding: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/agent-binding.schema.json"
        ))
        .unwrap();
        // base64url without padding, which is how every byte string crosses the capability wire.
        let encoded = |bytes: usize| (bytes * 4).div_ceil(3);

        assert_eq!(
            record_log["$defs"]["purpose"]["maxLength"]
                .as_u64()
                .unwrap() as usize,
            MAX_PURPOSE_BYTES
        );
        assert_eq!(
            signing["$defs"]["purpose"]["maxLength"].as_u64().unwrap() as usize,
            MAX_PURPOSE_BYTES
        );
        assert_eq!(
            record_log["$defs"]["open"]["properties"]["request"]["properties"]["sealedRecords"]
                ["maxItems"]
                .as_u64()
                .unwrap() as usize,
            MAX_OPEN_RECORDS
        );
        assert_eq!(
            record_log["$defs"]["append"]["properties"]["request"]["properties"]["canonical"]
                ["maxLength"]
                .as_u64()
                .unwrap() as usize,
            encoded(MAX_CANONICAL_DECODED_BYTES)
        );
        assert_eq!(
            record_log["$defs"]["sealed"]["maxLength"].as_u64().unwrap() as usize,
            encoded(MAX_OPEN_DECODED_BYTES)
        );
        assert_eq!(
            signing["$defs"]["bytes"]["maxLength"].as_u64().unwrap() as usize,
            encoded(MAX_SIGNING_PAYLOAD_BYTES)
        );
        assert_eq!(
            agent_binding["$defs"]["candidatesResult"]["properties"]["agents"]["maxItems"]
                .as_u64()
                .unwrap() as usize,
            MAX_AGENT_CANDIDATES
        );
    }

    #[test]
    fn content_view_limits_and_allowlist_match_the_contract_schema() {
        use teamarness_package_contract::media_types as content;
        let schema: Value = serde_json::from_str(include_str!(
            "../../../contracts/package-capabilities/v1/content-views.schema.json"
        ))
        .unwrap();
        let defs = &schema["$defs"];
        let limits = &defs["limits"]["properties"];
        let number = |value: &Value| value.as_u64().unwrap();
        assert_eq!(
            number(&limits["maxFiles"]["const"]) as usize,
            content::MAX_CONTENT_FILES
        );
        assert_eq!(
            number(&defs["openCall"]["properties"]["files"]["maxItems"]) as usize,
            content::MAX_CONTENT_FILES
        );
        assert_eq!(
            number(&limits["maxTotalBytes"]["const"]),
            content::MAX_CONTENT_TOTAL_BYTES
        );
        assert_eq!(
            number(&limits["maxFileBytes"]["const"]),
            content::MAX_CONTENT_FILE_BYTES
        );
        assert_eq!(
            number(&defs["file"]["properties"]["size"]["maximum"]),
            content::MAX_CONTENT_FILE_BYTES
        );
        assert_eq!(
            number(&limits["maxPathBytes"]["const"]) as usize,
            content::MAX_CONTENT_PATH_BYTES
        );
        assert_eq!(
            number(&defs["contentPath"]["maxLength"]) as usize,
            content::MAX_CONTENT_PATH_BYTES
        );
        assert_eq!(
            number(&defs["sourcePath"]["maxLength"]) as usize,
            crate::content_views::MAX_SOURCE_PATH_BYTES
        );
        assert_eq!(
            number(&limits["maxViewsPerGeneration"]["const"]) as usize,
            crate::content_views::MAX_VIEWS_PER_GENERATION
        );
        let published = defs["mediaTypes"]["const"].as_object().unwrap();
        assert_eq!(published.len(), content::MEDIA_TYPES.len());
        for (extension, media_type) in content::MEDIA_TYPES {
            assert_eq!(published[*extension], *media_type, "{extension}");
        }
        assert_eq!(
            Registry::production()
                .negotiate(&requirement(crate::content_views::FAMILY, 1, 0, 0))
                .unwrap()
                .0
                .kind,
            CapabilityKind::ContentViews
        );
    }
}
