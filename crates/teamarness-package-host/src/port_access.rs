//! Self-scoped durable selections and authorization over one immutable Port snapshot per operation.
use std::{collections::BTreeMap, str::FromStr, sync::Arc};

use serde_json::{Map, Value, json};
use teamarness_core::CoreRuntime;
use teamarness_package_contract::port_contract::{CompiledPortContracts, PortKind};

use crate::{PackageErrorCode, PackageIdentity, PackageInstanceKey, WorkspaceId};
use crate::{
    authority::GenerationAuthority, broker::PortBroker, host_store::HostStoreHandle,
    instance::InstanceActor,
};

pub(crate) struct PortAccess {
    pub(crate) store: HostStoreHandle,
}

impl PortAccess {
    pub(crate) fn new(store: HostStoreHandle) -> Self {
        Self { store }
    }
}

pub(crate) struct AccessState {
    pub(crate) actor: Value,
    pub(crate) selections: BTreeMap<String, u16>,
    pub(crate) scope: Value,
}

pub(crate) struct ResolvedPort {
    pub(crate) provider: Arc<InstanceActor>,
    pub(crate) schemas: Arc<CompiledPortContracts>,
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) actor: Value,
    pub(crate) lease: Option<Arc<SubscriptionLease>>,
}

pub(crate) fn actor(
    core: &CoreRuntime,
    key: &PackageInstanceKey,
) -> Result<Value, PackageErrorCode> {
    let workspace = teamarness_core::WorkspaceId::from_str(key.workspace_id.as_str())
        .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
    let agent = core
        .bound_agent(workspace, key.package_identity.as_str())
        .map_err(crate::capability::binding_error)?;
    if let Some(agent) = agent
        && let Some(principal) = core
            .principal(workspace, agent)
            .map_err(crate::capability::binding_error)?
        && let Some(identity) = principal.keys().first()
    {
        return Ok(
            json!({"kind":"agent","principal":agent.to_string(),"identity":identity.to_string()}),
        );
    }
    Ok(json!({"kind":"person","identity":core.public_identity().to_string()}))
}

impl PortBroker {
    /// Always reread durable state. There is no success cache to publish after a lost waiter
    /// or an uncertain SQLite completion. Both leases move into the actual blocking work.
    pub(crate) async fn access_work<T: Send + 'static>(
        self: &Arc<Self>,
        core: Arc<CoreRuntime>,
        key: PackageInstanceKey,
        authority: &GenerationAuthority,
        generation: u64,
        person: bool,
        work: impl FnOnce(
            &Self,
            &crate::host_store::HostStore,
            AccessState,
        ) -> Result<T, PackageErrorCode>
        + Send
        + 'static,
    ) -> Result<T, PackageErrorCode> {
        let leases = authority.leases().await?;
        let revision = authority.revision();
        let broker = self.clone();
        self.access
            .store
            .run(move |store| {
                let _leases = leases;
                let actor = actor(&core, &key)?;
                if person {
                    if actor["kind"] == "agent" {
                        return Err(PackageErrorCode::PortActorModeInvalid);
                    }
                    let workspace =
                        teamarness_core::WorkspaceId::from_str(key.workspace_id.as_str())
                            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)?;
                    if core
                        .workspace_person(workspace, core.public_identity())
                        .map_err(crate::capability::binding_error)?
                        .is_none()
                    {
                        return Err(PackageErrorCode::PersonNotCurrent);
                    }
                }
                let selections = store
                    .port_selections(
                        crate::host_store::PortSelectionScope {
                            installation: &core.public_identity().to_string(),
                            workspace: key.workspace_id.as_str(),
                            package: key.package_identity.as_str(),
                        },
                        person,
                    )
                    .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
                let scope = json!([
                    core.public_identity().to_string(),
                    key.workspace_id.as_str(),
                    key.package_identity.as_str(),
                    generation,
                    revision
                ]);
                work(
                    &broker,
                    store,
                    AccessState {
                        actor,
                        selections,
                        scope,
                    },
                )
            })
            .await
            .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
    }

    pub(crate) async fn select_contract(
        self: &Arc<Self>,
        core: Arc<CoreRuntime>,
        key: PackageInstanceKey,
        authority: &GenerationAuthority,
        generation: u64,
        input: Map<String, Value>,
    ) -> Result<Value, PackageErrorCode> {
        let installation = core.public_identity().to_string();
        let person = input.get("access").and_then(Value::as_str) == Some("person");
        let contract = input["contract"]
            .as_str()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?
            .to_owned();
        let major = input
            .get("major")
            .and_then(Value::as_u64)
            .map(|major| major as u16);
        self.access_work(
            core,
            key.clone(),
            authority,
            generation,
            person,
            move |broker, store, state| {
                if !person && state.actor["kind"] != "agent" {
                    return Err(PackageErrorCode::AgentNotBound);
                }
                if let Some(major) = major {
                    let snapshot = broker.snapshot();
                    let (_, schemas, selected, _) =
                        snapshot.dynamic_contract(&key.package_identity, &contract, !person)?;
                    broker.check_route_revision(
                        &snapshot,
                        &key.package_identity,
                        &input,
                        &state,
                    )?;
                    if selected != major {
                        return Err(PackageErrorCode::PortVersionMismatch);
                    }
                    if person && !schemas.has_guide(&contract) {
                        return Err(PackageErrorCode::PortGuideRequired);
                    }
                }
                #[cfg(feature = "test-hooks")]
                let failed_completion = {
                    let pause = {
                        let mut pending = broker
                            .selection_pause
                            .lock()
                            .expect("selection pause poisoned");
                        if pending
                            .as_ref()
                            .is_some_and(|(package, _)| package == &key.package_identity)
                        {
                            pending.take().map(|(_, pause)| pause)
                        } else {
                            None
                        }
                    };
                    pause.is_some_and(|pause| pause.wait())
                };
                store
                    .select_port(
                        crate::host_store::PortSelectionScope {
                            installation: &installation,
                            workspace: key.workspace_id.as_str(),
                            package: key.package_identity.as_str(),
                        },
                        &contract,
                        major,
                        person,
                    )
                    .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
                if major.is_none() {
                    broker.revoke_contract(&key, &contract);
                }
                #[cfg(feature = "test-hooks")]
                if failed_completion {
                    return Err(PackageErrorCode::CapabilityUnavailable);
                }
                let mut result = if let Some(major) = major {
                    json!({"contract":contract,"major":major,"enabled":true})
                } else {
                    json!({"contract":contract,"enabled":false})
                };
                if let Some(access) = input.get("access") {
                    result["access"] = access.clone();
                }
                Ok(result)
            },
        )
        .await
    }

    pub(crate) fn resolve_access(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        input: &Map<String, Value>,
        state: &AccessState,
    ) -> Result<ResolvedPort, PackageErrorCode> {
        let contract = input
            .get("contract")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let port = input
            .get("port")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let kind = match input.get("kind").and_then(Value::as_str) {
            Some("command") => PortKind::Command,
            Some("query") => PortKind::Query,
            Some("event") => PortKind::Event,
            _ => return Err(PackageErrorCode::PortUngranted),
        };
        let is_agent = state.actor["kind"] == "agent";
        // One snapshot for the whole check; a later publish never changes this route.
        let snapshot = self.snapshot();
        self.check_route_revision(&snapshot, caller, input, state)?;
        let (provider, schemas, major, minor) = match input.get("access").and_then(Value::as_str) {
            Some("declared") => self.resolve(&snapshot, caller, workspace, contract, port, kind)?,
            Some("agent" | "person") => {
                let person = input["access"] == "person";
                if !person && !is_agent {
                    return Err(PackageErrorCode::AgentNotBound);
                }
                let requested = input
                    .get("major")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                    .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
                if state.selections.get(contract) != Some(&requested) {
                    return Err(PackageErrorCode::PortNotEnabled);
                }
                let (identity, schemas, major, minor) =
                    snapshot.dynamic_contract(caller, contract, !person)?;
                if major != requested {
                    return Err(PackageErrorCode::PortUnavailable);
                }
                let provider = self
                    .instances
                    .read()
                    .expect("port registry poisoned")
                    .get(&PackageInstanceKey::new(identity, workspace.clone()))
                    .and_then(std::sync::Weak::upgrade)
                    .ok_or(PackageErrorCode::PortUnavailable)?;
                (provider, schemas, major, minor)
            }
            _ => return Err(PackageErrorCode::CapabilityRequestInvalid),
        };
        if input.get("major").and_then(Value::as_u64) != Some(u64::from(major)) {
            return Err(PackageErrorCode::PortVersionMismatch);
        }
        if is_agent && schemas.has_guide(contract) && state.selections.get(contract) != Some(&major)
        {
            return Err(PackageErrorCode::PortNotEnabled);
        }
        let descriptor = schemas
            .contracts
            .iter()
            .find(|c| c.identity == contract)
            .and_then(|c| c.ports.iter().find(|p| p.name == port && p.kind == kind))
            .ok_or(PackageErrorCode::PortUngranted)?;
        if input.get("method").and_then(Value::as_str) == Some("invoke")
            && descriptor.response.is_none()
        {
            return Err(PackageErrorCode::PortUngranted);
        }
        Ok(ResolvedPort {
            provider,
            schemas,
            major,
            minor,
            actor: state.actor.clone(),
            lease: None,
        })
    }
}

/// One authorization's delivery gate. Enqueue and revocation use this same lock;
/// accepted bounded queues may drain, but a revoked lease cannot accept a new value.
pub(crate) struct SubscriptionLease {
    key: PackageInstanceKey,
    provider: PackageInstanceKey,
    contract: String,
    binding: Option<String>,
    person_identity: Option<String>,
    enabled: bool,
    access: String,
    generation: u64,
    authority_revision: u64,
    pub(crate) delivery: Arc<crate::protocol::PortStreamLease>,
}

impl SubscriptionLease {
    pub(crate) fn watch(&self) -> tokio::sync::watch::Receiver<Option<PackageErrorCode>> {
        self.delivery.watch()
    }
    pub(crate) fn enqueue<T>(&self, work: impl FnOnce() -> T) -> Result<T, PackageErrorCode> {
        self.delivery.enqueue(work)
    }
    fn revoke(&self, code: PackageErrorCode) {
        self.delivery.revoke(code);
    }
}

impl PortBroker {
    pub(crate) fn subscription_lease(
        &self,
        key: PackageInstanceKey,
        input: &Map<String, Value>,
        route: &ResolvedPort,
        generation: u64,
        authority_revision: u64,
    ) -> Arc<SubscriptionLease> {
        let contract = input["contract"]
            .as_str()
            .expect("resolved contract")
            .to_owned();
        let lease = Arc::new(SubscriptionLease {
            provider: PackageInstanceKey::new(
                route.provider.identity().clone(),
                key.workspace_id.clone(),
            ),
            generation,
            authority_revision,
            access: input["access"]
                .as_str()
                .expect("resolved access")
                .to_owned(),
            key,
            enabled: input["access"] == "person"
                || (route.actor["kind"] == "agent" && route.schemas.has_guide(&contract)),
            binding: route.actor["principal"].as_str().map(str::to_owned),
            person_identity: (input["access"] == "person").then(|| {
                route.actor["identity"]
                    .as_str()
                    .expect("Person actor")
                    .to_owned()
            }),
            contract,
            delivery: Arc::new(crate::protocol::PortStreamLease::default()),
        });
        let mut leases = self
            .subscription_leases
            .lock()
            .expect("subscription registry poisoned");
        leases.retain(|lease| lease.strong_count() > 0);
        leases.push(Arc::downgrade(&lease));
        lease
    }

    fn revoke_subscriptions(
        &self,
        reason: impl Fn(&SubscriptionLease) -> Option<PackageErrorCode>,
    ) {
        self.subscription_leases
            .lock()
            .expect("subscription registry poisoned")
            .retain(|weak| {
                let Some(lease) = weak.upgrade() else {
                    return false;
                };
                if let Some(code) = reason(&lease) {
                    lease.revoke(code);
                }
                true
            });
    }
    pub(crate) fn revoke_contract(&self, key: &PackageInstanceKey, contract: &str) {
        self.revoke_subscriptions(|lease| {
            (lease.key == *key
                && lease.enabled
                && matches!(lease.access.as_str(), "agent" | "declared" | "person")
                && lease.contract == contract)
                .then_some(PackageErrorCode::PortNotEnabled)
        });
    }
    /// Called inside the same operation gate as the durable local binding transition.
    pub(crate) fn reconcile_binding(&self, key: &PackageInstanceKey, actor: &Value) {
        self.revoke_subscriptions(|lease| {
            if lease.key == *key && lease.access == "person" && actor["kind"] == "agent" {
                return Some(PackageErrorCode::PortActorModeInvalid);
            }
            if lease.key != *key
                || lease.binding.is_none()
                || lease.binding.as_deref() == actor["principal"].as_str()
            {
                return None;
            }
            Some(if actor["kind"] == "agent" {
                PackageErrorCode::AgentBindingChanged
            } else {
                PackageErrorCode::AgentNotBound
            })
        });
    }
    /// Recheck idle Person leases on canonical membership notifications, failing closed
    /// when Core cannot establish current membership.
    pub(crate) fn reconcile_person(&self, core: &CoreRuntime, key: &PackageInstanceKey) {
        let current = teamarness_core::WorkspaceId::from_str(key.workspace_id.as_str())
            .map_err(|_| PackageErrorCode::CapabilityRequestInvalid)
            .and_then(|workspace| {
                core.workspace_person(workspace, core.public_identity())
                    .map_err(crate::capability::binding_error)
            });
        let identity = core.public_identity().to_string();
        self.revoke_subscriptions(|lease| {
            if lease.key != *key || lease.access != "person" {
                return None;
            }
            match &current {
                Ok(Some(_)) if lease.person_identity.as_deref() == Some(identity.as_str()) => None,
                Ok(_) => Some(PackageErrorCode::PersonNotCurrent),
                Err(code) => Some(*code),
            }
        });
    }

    pub(crate) fn revoke_person(&self, key: &PackageInstanceKey, code: PackageErrorCode) {
        self.revoke_subscriptions(|lease| {
            (lease.key == *key && lease.access == "person").then_some(code)
        });
    }

    pub(crate) fn revoke_consumer(
        &self,
        key: &PackageInstanceKey,
        generation: u64,
        authority_revision: u64,
    ) {
        self.revoke_subscriptions(|lease| {
            (lease.key == *key
                && lease.generation == generation
                && lease.authority_revision == authority_revision)
                .then_some(PackageErrorCode::Cancelled)
        });
    }
    pub(crate) fn revoke_provider(&self, key: &PackageInstanceKey) {
        self.revoke_subscriptions(|lease| {
            (lease.provider == *key).then_some(PackageErrorCode::ProcessExited)
        });
    }
}
