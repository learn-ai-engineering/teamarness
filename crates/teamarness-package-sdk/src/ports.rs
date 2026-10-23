//! Generic Package Port adapters. The operation names are language-neutral wire names.
use serde::{Deserialize, Serialize};
use teamarness_package_protocol::JsonValue;

use crate::{BackendError, CapabilityClient, CapabilitySubscription};

/// Host-authored actor context; v1 only emits an unbound Installation Person.
/// Agent bindings are integrated when the Core binding API lands.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortActor {
    Person { identity: String },
}

/// A Package's opaque, generation-scoped capability grant, never a Provider selector.
#[derive(Clone)]
pub struct PortClient {
    capabilities: CapabilityClient,
    grant: String,
}

impl PortClient {
    #[must_use]
    pub fn new(capabilities: CapabilityClient, grant: impl Into<String>) -> Self {
        Self {
            capabilities,
            grant: grant.into(),
        }
    }

    /// Lists only exactly declared Ports and their availability.
    pub async fn available(&self) -> Result<JsonValue, BackendError> {
        self.capabilities
            .call(&self.grant, "available", serde_json::json!({}))
            .await
    }

    pub async fn command(
        &self,
        contract: &str,
        port: &str,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        self.invoke(contract, port, "command", request).await
    }

    pub async fn query(
        &self,
        contract: &str,
        port: &str,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        self.invoke(contract, port, "query", request).await
    }

    async fn invoke(
        &self,
        contract: &str,
        port: &str,
        kind: &str,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        self.capabilities
            .call(
                &self.grant,
                "invoke",
                serde_json::json!({
                    "contract":contract,"port":port,"kind":kind,"request":request
                }),
            )
            .await
            .map(decode_port_value)
    }

    /// A one-to-one consumer-initiated query stream, not an event subscription.
    pub async fn live_query(
        &self,
        contract: &str,
        port: &str,
        request: JsonValue,
    ) -> Result<CapabilitySubscription, BackendError> {
        self.capabilities
            .subscribe(
                &self.grant,
                serde_json::json!({
                    "contract":contract,"port":port,"kind":"query","request":request
                }),
            )
            .await
            .map(CapabilitySubscription::with_port_values)
    }

    /// Explicitly observes live Provider events; a declaration alone never subscribes.
    pub async fn subscribe_event(
        &self,
        contract: &str,
        port: &str,
    ) -> Result<CapabilitySubscription, BackendError> {
        self.capabilities
            .subscribe(
                &self.grant,
                serde_json::json!({"contract":contract,"port":port,"kind":"event"}),
            )
            .await
            .map(CapabilitySubscription::with_port_values)
    }
}

// Package Process 1.0 carries object payloads. Only non-object Port values
// need this Port-specific object envelope; ordinary object Port values keep
// their existing wire representation.
pub(crate) fn encode_port_value(value: JsonValue) -> JsonValue {
    if value.is_object() {
        value
    } else {
        serde_json::json!({"$teamarnessPortEnvelope":1,"$teamarnessPortValue":value})
    }
}

pub(crate) fn decode_port_value(value: JsonValue) -> JsonValue {
    if let JsonValue::Object(ref fields) = value
        && fields.len() == 2
        && fields.get("$teamarnessPortEnvelope") == Some(&JsonValue::from(1))
        && fields.contains_key("$teamarnessPortValue")
    {
        return fields["$teamarnessPortValue"].clone();
    }
    value
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderEnvelope {
    pub(crate) contract: String,
    pub(crate) major: u16,
    #[serde(rename = "minor")]
    pub(crate) _minor: u16,
    pub(crate) port: String,
    pub(crate) actor: PortActor,
    pub(crate) request: JsonValue,
}
