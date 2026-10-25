//! Declared metadata from the broker's immutable, final admission snapshot.
use std::collections::BTreeSet;
use std::sync::Weak;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use teamarness_package_contract::port_contract::PortMetadata;
use teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES;

use super::PortBroker;
use crate::{LifecycleState, PackageErrorCode, PackageIdentity, PackageInstanceKey, WorkspaceId};

const PAGE_ROWS: usize = 32;
const PAGE_BYTES: usize = MAX_UI_PAYLOAD_BYTES / 8;
const CHUNK_BYTES: usize = MAX_UI_PAYLOAD_BYTES / 4;

impl PortBroker {
    fn declared_metadata(
        &self,
        caller: &PackageIdentity,
        contract: &str,
    ) -> Result<PortMetadata, PackageErrorCode> {
        let declaration = self
            .consumed
            .get(caller)
            .into_iter()
            .flatten()
            .find(|c| c.identity == contract)
            .ok_or(PackageErrorCode::PortUngranted)?;
        let route = self
            .graph
            .get(caller)
            .into_iter()
            .flatten()
            .find(|r| r.contract == contract)
            .filter(|r| r.unavailable.is_none())
            .ok_or(PackageErrorCode::PortUnavailable)?;
        let schemas = route
            .provider
            .as_ref()
            .and_then(|p| self.contracts.get(p))
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

    async fn metadata_available(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        contract: &str,
    ) -> bool {
        let provider = self
            .graph
            .get(caller)
            .into_iter()
            .flatten()
            .find(|r| r.contract == contract && r.unavailable.is_none())
            .and_then(|r| r.provider.as_ref());
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

    fn cursor(&self, caller: &PackageIdentity, workspace: &WorkspaceId, last: &str) -> String {
        // The random Host-open revision makes every previous admission snapshot stale.
        // Hash the exact scope and last identity with that private revision; no client state.
        let bytes = serde_json::to_vec(&json!([
            caller.as_str(),
            workspace.as_str(),
            "declared",
            last
        ]))
        .expect("JSON scope");
        let mut hash = Sha256::new();
        hash.update(&bytes);
        hash.update(self.metadata_revision);
        let digest = hash.finalize();
        format!("{}.{}", STANDARD.encode(bytes), STANDARD.encode(digest))
    }

    pub(crate) async fn metadata_page(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        input: &Map<String, Value>,
    ) -> Result<Value, PackageErrorCode> {
        if input.len() != 2 + usize::from(input.contains_key("cursor"))
            || input.get("access").and_then(Value::as_str) != Some("declared")
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
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
            if cursor != self.cursor(caller, workspace, last) {
                return Err(PackageErrorCode::PortCatalogChanged);
            }
            Some(last.clone())
        } else {
            None
        };
        let mut declarations: Vec<_> = self.consumed.get(caller).into_iter().flatten().collect();
        declarations.sort_by(|a, b| a.identity.cmp(&b.identity));
        let remaining: Vec<_> = declarations
            .into_iter()
            .filter(|c| last.as_ref().is_none_or(|last| c.identity > *last))
            .collect();
        let mut rows = Vec::new();
        let mut page = json!({"contracts":[]});
        for (index, declaration) in remaining.iter().enumerate() {
            let metadata = self.declared_metadata(caller, &declaration.identity).ok();
            let route = self
                .graph
                .get(caller)
                .into_iter()
                .flatten()
                .find(|r| r.contract == declaration.identity);
            let description = route
                .and_then(|r| r.provider.as_ref())
                .and_then(|p| self.contracts.get(p))
                .and_then(|s| {
                    s.contracts
                        .iter()
                        .find(|c| c.identity == declaration.identity)
                })
                .map(|c| c.description.as_str());
            let available = metadata.is_some()
                && self
                    .metadata_available(caller, workspace, &declaration.identity)
                    .await;
            rows.push(json!({"contract":declaration.identity,"major":declaration.major,"minor":route.and_then(|r| r.minor),
                "description":description,"available":available,"enabled":false,"enabledMajor":null,"metadataSha256":metadata.map(|m| m.sha256)}));
            let mut candidate = json!({"contracts":rows});
            if index + 1 < remaining.len() {
                candidate["nextCursor"] =
                    json!(self.cursor(caller, workspace, &declaration.identity));
            }
            if serde_json::to_vec(&candidate)
                .map_err(|_| PackageErrorCode::CapabilityUnavailable)?
                .len()
                > PAGE_BYTES
            {
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
    ) -> Result<Value, PackageErrorCode> {
        if input.len() != 5 || input.get("access").and_then(Value::as_str) != Some("declared") {
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
        let metadata = self.declared_metadata(caller, contract)?;
        if !self.metadata_available(caller, workspace, contract).await {
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
