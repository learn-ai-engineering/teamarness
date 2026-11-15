//! Declared and Agent metadata, each page or chunk read from one immutable Port snapshot.
use std::collections::BTreeSet;
use std::sync::Weak;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use teamarness_package_contract::port_contract::PortMetadata;
use teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES;

use super::{PortBroker, PortSnapshot};
use crate::{LifecycleState, PackageErrorCode, PackageIdentity, PackageInstanceKey, WorkspaceId};

const PAGE_ROWS: usize = 32;
const PAGE_BYTES: usize = MAX_UI_PAYLOAD_BYTES / 8;
const CHUNK_BYTES: usize = MAX_UI_PAYLOAD_BYTES / 4;

impl PortBroker {
    fn route_revision(
        snapshot: &PortSnapshot,
        state: &crate::port_access::AccessState,
        access: &str,
        route: (&str, u16, &str, &PackageIdentity),
    ) -> String {
        let bytes = serde_json::to_vec(&json!([
            state.scope,
            access,
            route.0,
            route.1,
            route.2,
            route.3
        ]))
        .expect("route scope");
        let mut hash = Sha256::new();
        hash.update(b"teamarness.package-ports.route-revision/v3\0");
        hash.update(snapshot.revision());
        hash.update(bytes);
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn check_route_revision(
        &self,
        snapshot: &PortSnapshot,
        caller: &PackageIdentity,
        input: &Map<String, Value>,
        state: &crate::port_access::AccessState,
    ) -> Result<(), PackageErrorCode> {
        let Some(revision) = input.get("routeRevision") else {
            return Ok(());
        };
        let contract = input["contract"]
            .as_str()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let access = input["access"]
            .as_str()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let (provider, major, metadata) = if access == "declared" {
            let metadata = Self::declared_metadata(snapshot, caller, contract)?;
            let route = snapshot
                .route_of(caller, contract)
                .ok_or(PackageErrorCode::PortUnavailable)?;
            (
                route
                    .provider
                    .clone()
                    .ok_or(PackageErrorCode::PortUnavailable)?,
                snapshot
                    .consumed_of(caller)
                    .iter()
                    .find(|declaration| declaration.identity == contract)
                    .ok_or(PackageErrorCode::PortUnavailable)?
                    .major,
                metadata,
            )
        } else {
            let (provider, schemas, major, _) =
                snapshot.dynamic_contract(caller, contract, access == "agent")?;
            let metadata = schemas
                .project_metadata(contract, None)
                .map_err(|_| PackageErrorCode::PortUnavailable)?;
            (provider, major, metadata)
        };
        if revision.as_str()
            != Some(&Self::route_revision(
                snapshot,
                state,
                access,
                (contract, major, &metadata.sha256, &provider),
            ))
        {
            return Err(PackageErrorCode::PortCatalogChanged);
        }
        Ok(())
    }

    fn declared_metadata(
        snapshot: &PortSnapshot,
        caller: &PackageIdentity,
        contract: &str,
    ) -> Result<PortMetadata, PackageErrorCode> {
        let declaration = snapshot
            .consumed_of(caller)
            .iter()
            .find(|c| c.identity == contract)
            .ok_or(PackageErrorCode::PortUngranted)?;
        let route = snapshot
            .route_of(caller, contract)
            .filter(|r| r.unavailable.is_none())
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let schemas = route
            .provider
            .as_ref()
            .and_then(|p| snapshot.provided_contracts(p))
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let names = declaration
            .ports
            .iter()
            .map(|p| p.name.clone())
            .collect::<BTreeSet<_>>();
        schemas
            .project_metadata(contract, Some(&names))
            .map_err(|_| PackageErrorCode::PortUnavailable)
    }

    pub(crate) async fn provider_available(
        &self,
        workspace: &WorkspaceId,
        provider: Option<&PackageIdentity>,
    ) -> bool {
        let actor = provider.and_then(|provider| {
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
                LifecycleState::Admitted | LifecycleState::Active
            ),
            None => false,
        }
    }

    fn cursor(
        snapshot: &PortSnapshot,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        access: &str,
        last: &str,
    ) -> String {
        // Each published snapshot has a fresh random revision, so a cursor from any earlier
        // snapshot is stale. Hash the exact scope and last identity with that private
        // revision; no client state.
        let bytes = serde_json::to_vec(&json!([caller.as_str(), workspace.as_str(), access, last]))
            .expect("JSON scope");
        let mut hash = Sha256::new();
        hash.update(&bytes);
        hash.update(snapshot.revision());
        let digest = hash.finalize();
        format!("{}.{}", STANDARD.encode(bytes), STANDARD.encode(digest))
    }

    pub(crate) async fn metadata_page(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        input: &Map<String, Value>,
        state: &crate::port_access::AccessState,
        wire_major: u16,
    ) -> Result<Value, PackageErrorCode> {
        if input.len() != 2 + usize::from(input.contains_key("cursor"))
            || !matches!(
                input.get("access").and_then(Value::as_str),
                Some("declared" | "agent" | "person")
            )
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let access = input["access"]
            .as_str()
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        // One snapshot for the whole page, including the cursor it checks and issues.
        let snapshot = self.snapshot();
        if access == "agent" && state.actor["kind"] != "agent" {
            return Err(PackageErrorCode::AgentNotBound);
        }
        let last = if let Some(cursor) = input.get("cursor") {
            let cursor = cursor
                .as_str()
                .filter(|s| s.len() <= 2048)
                .ok_or(PackageErrorCode::PortCatalogChanged)?;
            let (encoded, _) = cursor
                .split_once('.')
                .ok_or(PackageErrorCode::PortCatalogChanged)?;
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| PackageErrorCode::PortCatalogChanged)?;
            let scope: Vec<String> =
                serde_json::from_slice(&bytes).map_err(|_| PackageErrorCode::PortCatalogChanged)?;
            let last = scope.get(3).ok_or(PackageErrorCode::PortCatalogChanged)?;
            if cursor != Self::cursor(&snapshot, caller, workspace, access, last) {
                return Err(PackageErrorCode::PortCatalogChanged);
            }
            Some(last.clone())
        } else {
            None
        };
        let consumed = snapshot.consumed_of(caller);
        let mut identities: BTreeSet<String> = if access == "declared" {
            consumed.iter().map(|c| c.identity.clone()).collect()
        } else {
            snapshot
                .providers_for(snapshot.tier_of(caller))
                .keys()
                .filter(|c| {
                    snapshot
                        .dynamic_contract(caller, c, access == "agent")
                        .is_ok()
                })
                .cloned()
                .chain(state.selections.keys().cloned())
                .collect()
        };
        identities.retain(|c| last.as_ref().is_none_or(|last| c > last));
        let remaining: Vec<_> = identities.into_iter().collect();
        let mut rows = Vec::new();
        let mut page = json!({"contracts":[]});
        for (index, contract) in remaining.iter().enumerate() {
            let (provider, metadata, major, minor, description) = if access == "declared" {
                let declaration = consumed
                    .iter()
                    .find(|c| &c.identity == contract)
                    .expect("declared identity");
                let route = snapshot.route_of(caller, contract);
                let provider = route.and_then(|r| r.provider.clone());
                let description = provider
                    .as_ref()
                    .and_then(|p| snapshot.provided_contracts(p))
                    .and_then(|s| {
                        s.contracts
                            .iter()
                            .find(|c| &c.identity == contract)
                            .map(|c| c.description.clone())
                    });
                (
                    provider,
                    Self::declared_metadata(&snapshot, caller, contract).ok(),
                    declaration.major,
                    route.and_then(|r| r.minor),
                    description,
                )
            } else if let Ok((provider, schemas, major, minor)) =
                snapshot.dynamic_contract(caller, contract, access == "agent")
            {
                let description = schemas
                    .contracts
                    .iter()
                    .find(|c| &c.identity == contract)
                    .map(|c| c.description.clone());
                (
                    Some(provider),
                    schemas.project_metadata(contract, None).ok(),
                    major,
                    Some(minor),
                    description,
                )
            } else {
                let provider = if wire_major == 3 {
                    snapshot
                        .providers_for(snapshot.tier_of(caller))
                        .get(contract)
                        .cloned()
                } else {
                    None
                };
                (provider, None, state.selections[contract], None, None)
            };
            let available = metadata.is_some()
                && self.provider_available(workspace, provider.as_ref()).await
                && (access == "declared"
                    || state
                        .selections
                        .get(contract)
                        .is_none_or(|saved| *saved == major));
            let enabled_major = state.selections.get(contract);
            let mut row = json!({"contract":contract,"major":major,"minor":minor,"description":description,"available":available,"enabled":enabled_major.is_some(),"enabledMajor":enabled_major,"metadataSha256":metadata.as_ref().map(|m| &m.sha256)});
            if wire_major == 3 {
                row["provider"] = provider
                    .as_ref()
                    .and_then(|identity| snapshot.presentations.get(identity))
                    .cloned()
                    .unwrap_or(Value::Null);
                row["guideEligibility"] = json!(match provider
                    .as_ref()
                    .and_then(|identity| snapshot.provided_contracts(identity))
                {
                    Some(schemas) if schemas.has_guide(contract) => "eligible",
                    Some(_) => "missing",
                    None => "unavailable",
                });
                row["routeRevision"] = match (&provider, &metadata) {
                    (Some(provider), Some(metadata)) => json!(Self::route_revision(
                        &snapshot,
                        state,
                        access,
                        (contract, major, &metadata.sha256, provider)
                    )),
                    _ => Value::Null,
                };
            }
            rows.push(row);
            let mut candidate = json!({"contracts":rows});
            if index + 1 < remaining.len() {
                candidate["nextCursor"] =
                    json!(Self::cursor(&snapshot, caller, workspace, access, contract));
            }
            if serde_json::to_vec(&candidate)
                .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
                .len()
                > PAGE_BYTES
            {
                if rows.len() == 1 {
                    return Err(PackageErrorCode::PortUnavailable);
                }
                break;
            }
            page = candidate;
            if rows.len() == PAGE_ROWS {
                break;
            }
        }
        Ok(page)
    }

    pub(crate) async fn metadata_chunk(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        input: &Map<String, Value>,
        state: &crate::port_access::AccessState,
        wire_major: u16,
    ) -> Result<Value, PackageErrorCode> {
        if input.len()
            != 5 + usize::from(wire_major == 3) + usize::from(input.contains_key("routeRevision"))
            || !matches!(
                input.get("access").and_then(Value::as_str),
                Some("declared" | "agent" | "person")
            )
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let text = |key| {
            input
                .get(key)
                .and_then(Value::as_str)
                .ok_or(PackageErrorCode::CapabilityRequestInvalid)
        };
        let contract = text("contract")?;
        let digest = text("metadataSha256")?;
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        // One snapshot for the whole chunk.
        let snapshot = self.snapshot();
        self.check_route_revision(&snapshot, caller, input, state)?;
        let (metadata, provider) = if input["access"] != "declared" {
            if input["access"] == "agent" && state.actor["kind"] != "agent" {
                return Err(PackageErrorCode::AgentNotBound);
            }
            let (provider, schemas, _, _) = snapshot
                .dynamic_contract(caller, contract, input["access"] == "agent")
                .map_err(|code| {
                    if state.selections.contains_key(contract) {
                        PackageErrorCode::PortUnavailable
                    } else {
                        code
                    }
                })?;
            (
                schemas
                    .project_metadata(contract, None)
                    .map_err(|_| PackageErrorCode::PortUnavailable)?,
                Some(provider),
            )
        } else {
            let metadata = Self::declared_metadata(&snapshot, caller, contract)?;
            let provider = snapshot
                .route_of(caller, contract)
                .and_then(|r| r.provider.clone());
            (metadata, provider)
        };
        if wire_major == 3 {
            let described: Value = serde_json::from_slice(&metadata.bytes)
                .map_err(|_| PackageErrorCode::PortUnavailable)?;
            if input["major"] != described["contracts"][0]["major"] {
                return Err(PackageErrorCode::PortVersionMismatch);
            }
        }
        if !self.provider_available(workspace, provider.as_ref()).await {
            return Err(PackageErrorCode::PortUnavailable);
        }
        if digest != metadata.sha256 {
            return Err(PackageErrorCode::PortCatalogChanged);
        }
        if offset >= metadata.bytes.len() {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let end = (offset + CHUNK_BYTES).min(metadata.bytes.len());
        let mut response = json!({"metadataSha256":metadata.sha256,"totalBytes":metadata.bytes.len(),"offset":offset,"data":STANDARD.encode(&metadata.bytes[offset..end])});
        if end < metadata.bytes.len() {
            response["nextOffset"] = json!(end);
        }
        Ok(response)
    }
}
