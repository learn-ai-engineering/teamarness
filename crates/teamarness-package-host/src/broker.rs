//! Installation-scoped, generation-limited routing for exact consumed Ports.
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, Weak};

use serde_json::{Map, Value, json};
use teamarness_package_contract::manifest::ConsumedContract;
use teamarness_package_contract::port_contract::{CompiledPortContracts, PortKind};
use teamarness_package_protocol::{MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[path = "event_subscriptions.rs"]
mod subscriptions;

use crate::admission::PortAvailability;
use crate::instance::InstanceActor;
use crate::protocol::{PortQueryStream, WireFailure};
use crate::{PackageErrorCode, PackageIdentity, PackageInstanceKey, PackageValue, WorkspaceId};

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
    graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
    consumed: BTreeMap<PackageIdentity, Vec<ConsumedContract>>,
    contracts: BTreeMap<PackageIdentity, Arc<CompiledPortContracts>>,
    instances: RwLock<BTreeMap<PackageInstanceKey, Weak<InstanceActor>>>,
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

impl PortBroker {
    pub(crate) fn new(
        graph: BTreeMap<PackageIdentity, Vec<PortAvailability>>,
        consumed: BTreeMap<PackageIdentity, Vec<ConsumedContract>>,
        contracts: BTreeMap<PackageIdentity, Arc<CompiledPortContracts>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            graph,
            consumed,
            contracts,
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

    pub(crate) fn register(&self, key: PackageInstanceKey, actor: &Arc<InstanceActor>) {
        self.instances
            .write()
            .expect("port registry poisoned")
            .insert(key, Arc::downgrade(actor));
    }

    /// Only declared ports occur in this view; an optional absence has no callable route.
    pub(crate) async fn availability(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
    ) -> Value {
        let mut entries = Vec::new();
        for consumed in self.consumed.get(caller).into_iter().flatten() {
            let Some(route) = self.graph.get(caller).and_then(|routes| {
                routes
                    .iter()
                    .find(|entry| entry.contract == consumed.identity)
            }) else {
                continue;
            };
            let available = if route.unavailable.is_none() {
                let actor = route.provider.as_ref().and_then(|provider| {
                    self.instances
                        .read()
                        .expect("port registry poisoned")
                        .get(&PackageInstanceKey::new(
                            provider.clone(),
                            workspace.clone(),
                        ))
                        .and_then(Weak::upgrade)
                });
                match actor {
                    Some(actor) => matches!(
                        actor.snapshot().await.lifecycle,
                        crate::LifecycleState::Admitted | crate::LifecycleState::Active
                    ),
                    None => false,
                }
            } else {
                false
            };
            entries.push(
                json!({"contract":consumed.identity,"major":consumed.major,"minor":route.minor,
                "available":available,"ports":consumed.ports}),
            );
        }
        json!({"contracts":entries})
    }

    fn resolve(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        contract: &str,
        port: &str,
        kind: PortKind,
    ) -> Result<(Arc<InstanceActor>, Arc<CompiledPortContracts>, u16, u16), PackageErrorCode> {
        let requested = self
            .consumed
            .get(caller)
            .into_iter()
            .flatten()
            .find(|entry| entry.identity == contract)
            .ok_or(PackageErrorCode::PortUngranted)?;
        if !requested
            .ports
            .iter()
            .any(|entry| entry.name == port && entry.kind == kind)
        {
            return Err(PackageErrorCode::PortUngranted);
        }
        let route = self
            .graph
            .get(caller)
            .into_iter()
            .flatten()
            .find(|entry| entry.contract == contract)
            .ok_or(PackageErrorCode::PortUnavailable)?;
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
        let schemas = self
            .contracts
            .get(provider)
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

    pub(crate) async fn call(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        installation: &str,
        input: &Map<String, Value>,
        dispatched: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Value, WireFailure> {
        if input.len() != 5
            || !["method", "contract", "port", "kind", "request"]
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
        let (actor, schemas, major, minor) = self
            .resolve(caller, workspace, contract, port, kind)
            .map_err(WireFailure::new)?;
        schemas
            .validate(contract, port, "request", request)
            .map_err(|_| WireFailure::new(PackageErrorCode::PortRequestInvalid))?;
        let permit = self.permit(request).map_err(WireFailure::new)?;
        let payload = json!({"contract":contract,"major":major,"minor":minor,"port":port,
            "actor":{"kind":"person","identity":installation},"request":request});
        let payload: PackageValue = serde_json::from_value(payload)
            .map_err(|_| WireFailure::new(PackageErrorCode::PortRequestInvalid))?;
        let result = actor
            .port_request(kind, payload, permit, dispatched)
            .await?;
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
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        installation: &str,
        input: &Map<String, Value>,
    ) -> Result<PortQueryStream, PackageErrorCode> {
        if input.len() != 5
            || !["grant", "contract", "port", "kind", "request"]
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
        let (actor, schemas, major, minor) =
            self.resolve(caller, workspace, contract, port, PortKind::Query)?;
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
            "actor":{"kind":"person","identity":installation},"request":request}),
        )
        .map_err(|_| PackageErrorCode::PortRequestInvalid)?;
        actor
            .port_query_stream(
                payload,
                schemas,
                contract.to_owned(),
                port.to_owned(),
                permits,
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
mod tests {
    use super::*;

    // Verifies: TEAM.SYS.PKG-R14
    #[test]
    fn maximum_frame_and_aggregate_bytes_bound_routes_before_dispatch() {
        let broker = PortBroker::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
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
}
