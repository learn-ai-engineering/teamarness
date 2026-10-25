//! Self-scoped durable selections and authorization over the immutable admission snapshot.
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
    pub(crate) providers: BTreeMap<String, PackageIdentity>,
}

pub(crate) struct AccessState {
    pub(crate) actor: Value,
    pub(crate) selections: BTreeMap<String, u16>,
}

pub(crate) struct ResolvedPort {
    pub(crate) provider: Arc<InstanceActor>,
    pub(crate) schemas: Arc<CompiledPortContracts>,
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) actor: Value,
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
        work: impl FnOnce(
            &Self,
            &crate::host_store::HostStore,
            AccessState,
        ) -> Result<T, PackageErrorCode>
        + Send
        + 'static,
    ) -> Result<T, PackageErrorCode> {
        let leases = authority.leases().await?;
        let broker = self.clone();
        self.access
            .store
            .run(move |store| {
                let _leases = leases;
                let actor = actor(&core, &key)?;
                let selections = store
                    .port_selections(
                        &core.public_identity().to_string(),
                        key.workspace_id.as_str(),
                        key.package_identity.as_str(),
                    )
                    .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
                work(&broker, store, AccessState { actor, selections })
            })
            .await
            .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
    }

    pub(crate) async fn select_contract(
        self: &Arc<Self>,
        core: Arc<CoreRuntime>,
        key: PackageInstanceKey,
        authority: &GenerationAuthority,
        contract: String,
        major: Option<u16>,
    ) -> Result<Value, PackageErrorCode> {
        let installation = core.public_identity().to_string();
        self.access_work(core, key.clone(), authority, move |broker, store, state| {
            if state.actor["kind"] != "agent" {
                return Err(PackageErrorCode::AgentNotBound);
            }
            if let Some(major) = major {
                let (_, _, selected, _) = broker.agent_contract(&contract)?;
                if selected != major {
                    return Err(PackageErrorCode::PortVersionMismatch);
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
                    &installation,
                    key.workspace_id.as_str(),
                    key.package_identity.as_str(),
                    &contract,
                    major,
                )
                .map_err(|_| PackageErrorCode::CapabilityUnavailable)?;
            #[cfg(feature = "test-hooks")]
            if failed_completion {
                return Err(PackageErrorCode::CapabilityUnavailable);
            }
            Ok(if let Some(major) = major {
                json!({"contract":contract,"major":major,"enabled":true})
            } else {
                json!({"contract":contract,"enabled":false})
            })
        })
        .await
    }

    pub(crate) fn agent_contract(
        &self,
        contract: &str,
    ) -> Result<(PackageIdentity, Arc<CompiledPortContracts>, u16, u16), PackageErrorCode> {
        let provider = self
            .access
            .providers
            .get(contract)
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let schemas = self
            .contracts
            .get(provider)
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
        Ok((provider.clone(), schemas, major, minor))
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
        let (provider, schemas, major, minor) = match input.get("access").and_then(Value::as_str) {
            Some("declared") => self.resolve(caller, workspace, contract, port, kind)?,
            Some("agent") => {
                if !is_agent {
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
                let (identity, schemas, major, minor) = self.agent_contract(contract)?;
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
        })
    }
}
