//! Installation-scoped, generation-limited routing for exact consumed Ports.
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, Weak};

use serde_json::{Map, Value, json};
use teamarness_package_contract::manifest::ConsumedContract;
use teamarness_package_contract::port_contract::{CompiledPortContracts, PortKind};
use teamarness_package_protocol::{MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

#[path = "port_metadata.rs"]
mod metadata;
#[path = "event_subscriptions.rs"]
mod subscriptions;

use crate::admission::PortAvailability;
use crate::instance::InstanceActor;
use crate::port_observation::{PortObservation, PortObservations, PortStage};
use crate::protocol::{PortQueryStream, WireFailure, capability_code};
use crate::{
    PackageErrorCode, PackageIdentity, PackageInstanceKey, PackageTier, PackageValue, WorkspaceId,
};

// Four maximum request/response pairs fit this Host-wide byte budget; the per-generation
// limit leaves at least 24 of the protocol's 32 request slots to UI/lifecycle traffic.
const ROUTES: usize = MAX_IN_FLIGHT_EACH_DIRECTION / 2;
const PROVIDER_ROUTES: usize = MAX_IN_FLIGHT_EACH_DIRECTION / 4;
const ROUTE_BYTES: usize = 8 * MAX_FRAME_BYTES;
const _: () = {
    assert!(PROVIDER_ROUTES < MAX_IN_FLIGHT_EACH_DIRECTION);
    assert!(ROUTE_BYTES >= 2 * MAX_FRAME_BYTES);
};

pub(crate) struct PortBroker {
    #[cfg(feature = "test-hooks")]
    pub(crate) authorized_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) command_authorized_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) delivery_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) selection_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::PortSelectionPause>)>>,
    authority_generations:
        std::sync::Mutex<BTreeMap<PackageInstanceKey, crate::authority::AuthorityGeneration>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) binding_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) actor_pause: std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) record_availability_pause:
        std::sync::Mutex<Option<(PackageIdentity, Arc<crate::AuthorityPause>)>>,
    pub(crate) subscription_leases:
        std::sync::Mutex<Vec<Weak<crate::port_access::SubscriptionLease>>>,
    pub(crate) access: crate::port_access::PortAccess,
    /// The admission-derived routing every operation reads. Replaced whole by
    /// [`PortBroker::publish`], never changed in place (ADR 0035, ADR 0036).
    snapshot: RwLock<Arc<PortSnapshot>>,
    observations: broadcast::Sender<PortObservation>,
    pub(crate) instances: RwLock<BTreeMap<PackageInstanceKey, Weak<InstanceActor>>>,
    routes: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    events: Arc<tokio::sync::Mutex<subscriptions::EventRegistry>>,
    event_streams: Arc<Semaphore>,
    event_subscribers: Arc<Semaphore>,
    event_bytes: Arc<Semaphore>,
    event_open_locks:
        std::sync::Mutex<BTreeMap<subscriptions::EventKey, Arc<tokio::sync::Mutex<()>>>>,
}

pub(crate) struct RoutePermits {
    _route: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

/// One caller's admission-derived Port entries.
pub(crate) struct CallerPorts {
    pub(crate) routes: Vec<PortAvailability>,
    pub(crate) consumed: Vec<ConsumedContract>,
    pub(crate) contracts: Option<Arc<CompiledPortContracts>>,
}

/// One immutable view of the admission-derived Port graph.
///
/// Every resolve, metadata page or chunk, and access check clones the current
/// snapshot once and reads only it (ADR 0035). A Personal change builds the
/// next snapshot, which copies every Team entry unchanged, and publishes it in
/// one swap with a fresh `revision` (ADR 0036). Metadata cursors hash the
/// revision, so a cursor from an earlier snapshot is refused.
pub(crate) struct PortSnapshot {
    revision: [u8; 32],
    graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    consumed: BTreeMap<PackageIdentity, Vec<ConsumedContract>>,
    contracts: BTreeMap<PackageIdentity, Arc<CompiledPortContracts>>,
    tiers: BTreeMap<PackageIdentity, PackageTier>,
    team_providers: BTreeMap<String, PackageIdentity>,
    personal_providers: BTreeMap<String, PackageIdentity>,
}

impl PortSnapshot {
    /// The snapshot at Host open, over every admitted Package of both tiers.
    pub(crate) fn new(
        graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
        consumed: BTreeMap<PackageIdentity, Vec<ConsumedContract>>,
        contracts: BTreeMap<PackageIdentity, Arc<CompiledPortContracts>>,
        tiers: BTreeMap<PackageIdentity, PackageTier>,
        team_providers: BTreeMap<String, PackageIdentity>,
        personal_providers: BTreeMap<String, PackageIdentity>,
    ) -> Self {
        Self {
            revision: fresh_revision(),
            graph,
            consumed,
            contracts,
            tiers,
            team_providers,
            personal_providers,
        }
    }

    /// The next snapshot: this one's Team entries unchanged, the Personal tier
    /// replaced by `personal` and `personal_providers`, and a fresh revision.
    pub(crate) fn with_personal(
        &self,
        personal: BTreeMap<PackageIdentity, CallerPorts>,
        personal_providers: BTreeMap<String, PackageIdentity>,
    ) -> Self {
        let team =
            |identity: &PackageIdentity| self.tiers.get(identity) == Some(&PackageTier::Team);
        let mut next = Self {
            revision: fresh_revision(),
            graph: keep(&self.graph, team),
            consumed: keep(&self.consumed, team),
            contracts: keep(&self.contracts, team),
            tiers: keep(&self.tiers, team),
            team_providers: self.team_providers.clone(),
            personal_providers,
        };
        for (identity, ports) in personal {
            debug_assert!(!team(&identity), "a Team caller's entries never change");
            next.graph.insert(identity.clone(), ports.routes);
            next.consumed.insert(identity.clone(), ports.consumed);
            if let Some(contracts) = ports.contracts {
                next.contracts.insert(identity.clone(), contracts);
            }
            next.tiers.insert(identity, PackageTier::Personal);
        }
        next
    }

    pub(crate) fn revision(&self) -> &[u8; 32] {
        &self.revision
    }

    /// The tier of `caller`. A caller this snapshot does not know gets the Team
    /// tier's narrower view.
    pub(crate) fn tier_of(&self, caller: &PackageIdentity) -> PackageTier {
        self.tiers.get(caller).copied().unwrap_or(PackageTier::Team)
    }

    /// The Providers a caller in `tier` may reach, by contract identity. A Team
    /// caller sees only Team Providers; a Personal caller sees both tiers, which
    /// never overlap because admission refuses an overlap with
    /// `port_provider_conflict`.
    pub(crate) fn providers_for(&self, tier: PackageTier) -> BTreeMap<String, PackageIdentity> {
        let mut providers = self.team_providers.clone();
        if tier == PackageTier::Personal {
            providers.extend(
                self.personal_providers
                    .iter()
                    .map(|(contract, provider)| (contract.clone(), provider.clone())),
            );
        }
        providers
    }

    /// One caller's consumed Port declarations.
    pub(crate) fn consumed_of(&self, caller: &PackageIdentity) -> &[ConsumedContract] {
        self.consumed.get(caller).map_or(&[], Vec::as_slice)
    }

    /// One caller's selected route for `contract`.
    pub(crate) fn route_of(
        &self,
        caller: &PackageIdentity,
        contract: &str,
    ) -> Option<&PortAvailability> {
        self.graph
            .get(caller)
            .and_then(|routes| routes.iter().find(|route| route.contract == contract))
    }

    /// The contracts one Provider declares.
    pub(crate) fn provided_contracts(
        &self,
        provider: &PackageIdentity,
    ) -> Option<&Arc<CompiledPortContracts>> {
        self.contracts.get(provider)
    }

    /// The guided contract `caller` may reach through Agent access, with its
    /// Provider, schemas, major, and minor.
    pub(crate) fn agent_contract(
        &self,
        caller: &PackageIdentity,
        contract: &str,
    ) -> Result<(PackageIdentity, Arc<CompiledPortContracts>, u16, u16), PackageErrorCode> {
        let provider = self
            .providers_for(self.tier_of(caller))
            .remove(contract)
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let schemas = self
            .provided_contracts(&provider)
            .cloned()
            .ok_or(PackageErrorCode::PortUnavailable)?;
        if !schemas.has_guide(contract) {
            return Err(PackageErrorCode::PortUngranted);
        }
        let descriptor = schemas
            .contracts
            .iter()
            .find(|c| c.identity == contract)
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let (major, minor) = (descriptor.major, descriptor.minor);
        Ok((provider, schemas, major, minor))
    }
}

#[cfg(feature = "test-hooks")]
impl PortSnapshot {
    pub(crate) fn probe(&self) -> crate::PortSnapshotProbe {
        crate::PortSnapshotProbe {
            revision: self.revision,
            team: self
                .tiers
                .iter()
                .filter(|(_, tier)| **tier == PackageTier::Team)
                .map(|(identity, _)| {
                    (
                        identity.clone(),
                        crate::TeamPortEntry {
                            routes: self.graph.get(identity).cloned().unwrap_or_default(),
                            consumed: self.consumed_of(identity).to_vec(),
                            contracts: self
                                .contracts
                                .get(identity)
                                .map(|contracts| Arc::as_ptr(contracts) as usize),
                        },
                    )
                })
                .collect(),
            team_providers: self.team_providers.clone(),
            personal_providers: self.personal_providers.clone(),
        }
    }
}

fn fresh_revision() -> [u8; 32] {
    let mut revision = [0; 32];
    getrandom::fill(&mut revision).expect("Port snapshot revision entropy");
    revision
}

fn keep<V: Clone>(
    map: &BTreeMap<PackageIdentity, V>,
    team: impl Fn(&PackageIdentity) -> bool,
) -> BTreeMap<PackageIdentity, V> {
    map.iter()
        .filter(|(identity, _)| team(identity))
        .map(|(identity, value)| (identity.clone(), value.clone()))
        .collect()
}

impl PortBroker {
    /// Builds the broker over the initial snapshot of every admitted Package.
    pub(crate) fn new(snapshot: PortSnapshot, access: crate::port_access::PortAccess) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(feature = "test-hooks")]
            authorized_pause: std::sync::Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            command_authorized_pause: std::sync::Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            delivery_pause: std::sync::Mutex::new(None),
            access,
            subscription_leases: std::sync::Mutex::new(Vec::new()),
            #[cfg(feature = "test-hooks")]
            selection_pause: std::sync::Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            record_availability_pause: std::sync::Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            actor_pause: std::sync::Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            binding_pause: std::sync::Mutex::new(None),
            authority_generations: std::sync::Mutex::new(BTreeMap::new()),
            snapshot: RwLock::new(Arc::new(snapshot)),
            observations: crate::port_observation::channel(),
            instances: RwLock::new(BTreeMap::new()),
            routes: Arc::new(Semaphore::new(ROUTES)),
            bytes: Arc::new(Semaphore::new(ROUTE_BYTES)),
            events: Arc::new(tokio::sync::Mutex::new(
                subscriptions::EventRegistry::default(),
            )),
            event_streams: Arc::new(Semaphore::new(subscriptions::MAX_EVENT_UPSTREAMS)),
            event_subscribers: Arc::new(Semaphore::new(subscriptions::MAX_EVENT_SUBSCRIBERS)),
            event_bytes: Arc::new(Semaphore::new(subscriptions::EVENT_BYTES)),
            event_open_locks: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) async fn pause_port_work(&self, package: &PackageIdentity, delivery: bool) {
        let pending = if delivery {
            &self.delivery_pause
        } else {
            &self.authorized_pause
        };
        let pause = {
            let mut pending = pending.lock().expect("test pause poisoned");
            if pending
                .as_ref()
                .is_some_and(|(expected, _)| expected == package)
            {
                pending.take().map(|(_, pause)| pause)
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.wait().await;
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn take_record_availability_pause(
        &self,
        package: &PackageIdentity,
    ) -> Option<Arc<crate::AuthorityPause>> {
        let mut pending = self
            .record_availability_pause
            .lock()
            .expect("test pause poisoned");
        if pending
            .as_ref()
            .is_some_and(|(expected, _)| expected == package)
        {
            pending.take().map(|(_, pause)| pause)
        } else {
            None
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) async fn pause_command_actor(
        &self,
        package: &PackageIdentity,
        object: &Map<String, Value>,
    ) {
        let pause = {
            let mut pending = self.actor_pause.lock().expect("test pause poisoned");
            if object.get("kind").and_then(Value::as_str) == Some("command")
                && pending
                    .as_ref()
                    .is_some_and(|(expected, _)| expected == package)
            {
                pending.take().map(|(_, pause)| pause)
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.wait().await;
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) async fn pause_command_authorized(
        &self,
        package: &PackageIdentity,
        object: &Map<String, Value>,
    ) {
        let pause = {
            let mut pending = self
                .command_authorized_pause
                .lock()
                .expect("test pause poisoned");
            if object.get("kind").and_then(Value::as_str) == Some("command")
                && pending
                    .as_ref()
                    .is_some_and(|(expected, _)| expected == package)
            {
                pending.take().map(|(_, pause)| pause)
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.wait().await;
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) async fn pause_binding_write(&self, package: &PackageIdentity) {
        let pause = {
            let mut pending = self.binding_pause.lock().expect("test pause poisoned");
            if pending
                .as_ref()
                .is_some_and(|(expected, _)| expected == package)
            {
                pending.take().map(|(_, pause)| pause)
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.wait().await;
        }
    }

    pub(crate) fn record_call_refusal(
        &self,
        caller: &PackageIdentity,
        consumer_id: &str,
        generation: u64,
        input: &Map<String, Value>,
        code: PackageErrorCode,
    ) {
        self.record(PortObservation {
            kind: if input.get("kind").and_then(Value::as_str) == Some("command") {
                "command"
            } else {
                "query"
            },
            contract: None,
            port: None,
            consumer_id: consumer_id.to_owned(),
            consumer_package: caller.as_str().to_owned(),
            consumer_generation: generation,
            provider_id: None,
            provider_generation: None,
            stage: PortStage::Rejected,
            code: Some(capability_code(code)),
            outcome_uncertain: false,
        });
    }

    pub(crate) fn observe(&self) -> PortObservations {
        self.observations.subscribe()
    }

    fn record(&self, observation: PortObservation) {
        // `send` refuses if nobody subscribed, and never waits for a slow receiver.
        let _ = self.observations.send(observation);
    }

    /// Host-lifetime ownership survives any InstanceActor/ProcessSpec reconstruction.
    pub(crate) fn authority_generation(
        &self,
        key: &PackageInstanceKey,
    ) -> crate::authority::AuthorityGeneration {
        self.authority_generations
            .lock()
            .expect("authority registry poisoned")
            .entry(key.clone())
            .or_default()
            .clone()
    }

    pub(crate) fn register(&self, key: PackageInstanceKey, actor: &Arc<InstanceActor>) {
        self.instances
            .write()
            .expect("port registry poisoned")
            .insert(key, Arc::downgrade(actor));
    }

    /// Makes one Instance unreachable for new routes. A call that already
    /// resolved it keeps its own reference and ends under the R14 rules.
    pub(crate) fn unregister(&self, key: &PackageInstanceKey) {
        self.instances
            .write()
            .expect("port registry poisoned")
            .remove(key);
    }

    /// The current snapshot. An operation clones it once and reads only it, so a
    /// later [`publish`](Self::publish) never changes what it already resolved.
    pub(crate) fn snapshot(&self) -> Arc<PortSnapshot> {
        self.snapshot
            .read()
            .expect("port snapshot poisoned")
            .clone()
    }

    /// Replaces the snapshot in one swap. Callers serialize publishes; only the
    /// Personal tier publishes, under its tier lock.
    pub(crate) fn publish(&self, next: PortSnapshot) {
        *self.snapshot.write().expect("port snapshot poisoned") = Arc::new(next);
    }

    /// Resolves one declared Port against `snapshot`, the caller's one snapshot.
    pub(crate) fn resolve(
        &self,
        snapshot: &PortSnapshot,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        contract: &str,
        port: &str,
        kind: PortKind,
    ) -> Result<(Arc<InstanceActor>, Arc<CompiledPortContracts>, u16, u16), PackageErrorCode> {
        let requested = snapshot
            .consumed_of(caller)
            .iter()
            .find(|entry| entry.identity == contract)
            .ok_or(PackageErrorCode::PortUngranted)?;
        let route = snapshot
            .route_of(caller, contract)
            .ok_or(PackageErrorCode::PortUngranted)?;
        if !requested
            .ports
            .iter()
            .any(|entry| entry.name == port && entry.kind == kind)
        {
            return Err(PackageErrorCode::PortUngranted);
        }
        if route.unavailable.is_some() {
            return Err(PackageErrorCode::PortUnavailable);
        }
        let provider = route
            .provider
            .as_ref()
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let actor = self
            .instances
            .read()
            .expect("port registry poisoned")
            .get(&PackageInstanceKey::new(
                provider.clone(),
                workspace.clone(),
            ))
            .and_then(Weak::upgrade)
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let schemas = snapshot
            .provided_contracts(provider)
            .cloned()
            .ok_or(PackageErrorCode::PortUnavailable)?;
        Ok((
            actor,
            schemas,
            requested.major,
            route.minor.ok_or(PackageErrorCode::PortUnavailable)?,
        ))
    }

    fn permit(&self, request: &Value) -> Result<RoutePermits, PackageErrorCode> {
        let size = serde_json::to_vec(request)
            .map_err(|_| PackageErrorCode::PortRequestInvalid)?
            .len();
        if size > MAX_FRAME_BYTES {
            return Err(PackageErrorCode::PortRequestInvalid);
        }
        let route = self
            .routes
            .clone()
            .try_acquire_owned()
            .map_err(|_| PackageErrorCode::Overloaded)?;
        // Reserve the maximum response before dispatch as well as the actual request.
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(
                u32::try_from(size + MAX_FRAME_BYTES).map_err(|_| PackageErrorCode::Overloaded)?,
            )
            .map_err(|_| PackageErrorCode::Overloaded)?;
        Ok(RoutePermits {
            _route: route,
            _bytes: bytes,
        })
    }

    #[allow(clippy::too_many_arguments)] // Scope, wire ID, and generation remain explicit at this boundary.
    pub(crate) async fn call(
        &self,
        caller: &PackageIdentity,
        route: crate::port_access::ResolvedPort,
        input: &Map<String, Value>,
        dispatched: Arc<std::sync::atomic::AtomicBool>,
        consumer_id: &str,
        consumer_generation: u64,
    ) -> Result<Value, WireFailure> {
        let provider_id = Arc::new(std::sync::Mutex::new(None::<(String, u64)>));
        let result = self
            .call_inner(
                caller,
                route,
                input,
                dispatched,
                consumer_id,
                consumer_generation,
                provider_id.clone(),
            )
            .await;
        let assigned = provider_id
            .lock()
            .expect("port observation poisoned")
            .clone();
        // Untrusted names are not recorded when the route was rejected before dispatch.
        let (contract, port) = if assigned.is_some() {
            (
                input
                    .get("contract")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                input.get("port").and_then(Value::as_str).map(str::to_owned),
            )
        } else {
            (None, None)
        };
        let kind = if input.get("kind").and_then(Value::as_str) == Some("command") {
            "command"
        } else {
            "query"
        };
        self.record(PortObservation {
            kind,
            contract,
            port,
            consumer_id: consumer_id.to_owned(),
            consumer_package: caller.as_str().to_owned(),
            consumer_generation,
            provider_id: assigned.as_ref().map(|entry| entry.0.clone()),
            provider_generation: assigned.as_ref().map(|entry| entry.1),
            stage: match &result {
                Ok(_) => PortStage::Completed,
                Err(_) if assigned.is_none() => PortStage::Rejected,
                Err(_) => PortStage::Failed,
            },
            code: result
                .as_ref()
                .err()
                .map(|failure| capability_code(failure.code)),
            outcome_uncertain: result
                .as_ref()
                .err()
                .is_some_and(|failure| failure.outcome_uncertain),
        });
        result
    }

    #[allow(clippy::too_many_arguments)] // The private dispatch path also carries the optional ID join.
    async fn call_inner(
        &self,
        caller: &PackageIdentity,
        route: crate::port_access::ResolvedPort,
        input: &Map<String, Value>,
        dispatched: Arc<std::sync::atomic::AtomicBool>,
        consumer_id: &str,
        consumer_generation: u64,
        provider_id: Arc<std::sync::Mutex<Option<(String, u64)>>>,
    ) -> Result<Value, WireFailure> {
        if input.len() != 7
            || ![
                "method", "access", "major", "contract", "port", "kind", "request",
            ]
            .iter()
            .all(|key| input.contains_key(*key))
        {
            return Err(WireFailure::new(PackageErrorCode::CapabilityRequestInvalid));
        }
        let contract = text(input, "contract")?;
        let port = text(input, "port")?;
        let kind = match text(input, "kind")? {
            "command" => PortKind::Command,
            "query" => PortKind::Query,
            _ => return Err(WireFailure::new(PackageErrorCode::PortUngranted)),
        };
        let request = input
            .get("request")
            .ok_or_else(|| WireFailure::new(PackageErrorCode::PortRequestInvalid))?;
        let crate::port_access::ResolvedPort {
            provider,
            schemas,
            major,
            minor,
            actor,
            lease: _,
        } = route;
        schemas
            .validate(contract, port, "request", request)
            .map_err(|_| WireFailure::new(PackageErrorCode::PortRequestInvalid))?;
        let permit = self.permit(request).map_err(WireFailure::new)?;
        // `actor` is Host-authored from the caller's binding, never from the request.
        let payload = json!({"contract":contract,"major":major,"minor":minor,"port":port,
            "actor":actor,"request":request});
        let payload: PackageValue = serde_json::from_value(payload)
            .map_err(|_| WireFailure::new(PackageErrorCode::PortRequestInvalid))?;
        let observations = self.observations.clone();
        let contract_name = contract.to_owned();
        let port_name = port.to_owned();
        let consumer = consumer_id.to_owned();
        let caller_name = caller.as_str().to_owned();
        let result = provider
            .port_request(kind, payload, permit, dispatched, move |id, generation| {
                *provider_id.lock().expect("port observation poisoned") =
                    Some((id.clone(), generation));
                let _ = observations.send(PortObservation {
                    kind: if kind == PortKind::Command {
                        "command"
                    } else {
                        "query"
                    },
                    contract: Some(contract_name),
                    port: Some(port_name),
                    consumer_id: consumer,
                    consumer_package: caller_name,
                    consumer_generation,
                    provider_id: Some(id),
                    provider_generation: Some(generation),
                    stage: PortStage::Dispatched,
                    code: None,
                    outcome_uncertain: false,
                });
            })
            .await?;
        let result = crate::protocol::decode_port_value(&result).clone();
        schemas
            .validate(contract, port, "response", &result)
            .map_err(|_| {
                let mut failure = WireFailure::new(PackageErrorCode::PortResponseInvalid);
                failure.outcome_uncertain = kind == PortKind::Command;
                failure
            })?;
        Ok(result)
    }

    pub(crate) async fn query_stream(
        &self,
        route: crate::port_access::ResolvedPort,
        input: &Map<String, Value>,
    ) -> Result<PortQueryStream, PackageErrorCode> {
        if input.len() != 7
            || ![
                "grant", "access", "major", "contract", "port", "kind", "request",
            ]
            .iter()
            .all(|key| input.contains_key(*key))
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let contract = input
            .get("contract")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let port = input
            .get("port")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        if input.get("kind").and_then(Value::as_str) != Some("query") {
            return Err(PackageErrorCode::PortUngranted);
        }
        let request = input
            .get("request")
            .ok_or(PackageErrorCode::PortRequestInvalid)?;
        let crate::port_access::ResolvedPort {
            provider,
            schemas,
            major,
            minor,
            actor,
            lease,
        } = route;
        if let Some(lease) = &lease {
            lease.enqueue(|| ())?;
        }
        let descriptor = schemas
            .contracts
            .iter()
            .find(|entry| entry.identity == contract)
            .and_then(|entry| entry.ports.iter().find(|entry| entry.name == port))
            .ok_or(PackageErrorCode::PortUngranted)?;
        if descriptor.event.is_none() {
            return Err(PackageErrorCode::PortUngranted);
        }
        schemas
            .validate(contract, port, "request", request)
            .map_err(|_| PackageErrorCode::PortRequestInvalid)?;
        let permits = self.permit(request)?;
        let payload: PackageValue = serde_json::from_value(
            json!({"contract":contract,"major":major,"minor":minor,"port":port,
            "actor":actor,"request":request}),
        )
        .map_err(|_| PackageErrorCode::PortRequestInvalid)?;
        provider
            .port_query_stream(
                payload,
                schemas,
                contract.to_owned(),
                port.to_owned(),
                permits,
                lease,
            )
            .await
            .map_err(|error| error.code)
    }
}

pub(crate) const PROVIDER_LIMIT: usize = PROVIDER_ROUTES;

fn text<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, WireFailure> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| WireFailure::new(PackageErrorCode::CapabilityRequestInvalid))
}

#[cfg(test)]
pub(crate) async fn empty_broker() -> Arc<PortBroker> {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::host_store::HostStoreHandle::open(directory.path().to_owned())
        .await
        .unwrap();
    PortBroker::new(
        PortSnapshot::new(
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        ),
        crate::port_access::PortAccess::new(store),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn optional_observer_lag_does_not_consume_route_capacity_or_copy_payload() {
        let broker = empty_broker().await;
        let mut slow = broker.observe();
        let observation = PortObservation {
            kind: "command",
            contract: None,
            port: None,
            consumer_id: "p:local".into(),
            consumer_package: "dev.example.consumer".into(),
            consumer_generation: 1,
            provider_id: None,
            provider_generation: None,
            stage: PortStage::Rejected,
            code: Some("ungranted_port"),
            outcome_uncertain: false,
        };
        for _ in 0..256 {
            broker.record(observation.clone());
        }
        assert!(matches!(
            slow.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        assert!(!format!("{observation:?}").contains("markdown"));
        assert!(broker.permit(&json!({})).is_ok());
    }

    // Verifies: TEAM.SYS.PKG-R14
    #[tokio::test]
    async fn maximum_frame_and_aggregate_bytes_bound_routes_before_dispatch() {
        let broker = empty_broker().await;
        assert!(matches!(
            broker.permit(&json!({"value":"x".repeat(MAX_FRAME_BYTES)})),
            Err(PackageErrorCode::PortRequestInvalid)
        ));
        let request = json!({"value": "x".repeat(MAX_FRAME_BYTES / 8)});
        let mut permits = Vec::new();
        while let Ok(permit) = broker.permit(&request) {
            permits.push(permit);
        }
        assert!(!permits.is_empty());
        assert!(permits.len() <= ROUTE_BYTES / MAX_FRAME_BYTES);
        assert!(matches!(
            broker.permit(&request),
            Err(PackageErrorCode::Overloaded)
        ));
        permits.clear();
        assert!(broker.permit(&request).is_ok());
    }

    // Verifies: TEAM.SYS.PKG-R09
    #[tokio::test]
    async fn snapshot_revision_changes_on_publish() {
        let broker = empty_broker().await;
        let before = broker.snapshot();
        let provider: PackageIdentity = "dev.example.provider".parse().unwrap();

        broker.publish(before.with_personal(
            BTreeMap::new(),
            BTreeMap::from([("dev.example.messages".to_owned(), provider.clone())]),
        ));

        let after = broker.snapshot();
        assert_ne!(after.revision(), before.revision());
        assert!(
            before.providers_for(PackageTier::Personal).is_empty(),
            "a snapshot an operation already holds never changes"
        );
        assert_eq!(
            after.providers_for(PackageTier::Personal)["dev.example.messages"],
            provider
        );
        assert!(after.providers_for(PackageTier::Team).is_empty());
        broker.publish(after.with_personal(BTreeMap::new(), BTreeMap::new()));
        assert_ne!(broker.snapshot().revision(), after.revision());
    }
}
