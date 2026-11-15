//! Package Ports 3.x. Person access is explicit and never falls back to another view.
use crate::capabilities::{invalid_request, invalid_response, is_lower_hex};
use crate::ports::{PortClient, decode_port_value};
use crate::{BackendError, CapabilityClient, CapabilitySubscription, Initialization};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use teamarness_package_contract::{
    identity::is_valid_package_identity, port_contract::CompiledPortContracts,
};

pub const IDENTITY: &str = "teamarness.package-ports";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortAccessV3 {
    Declared,
    Agent,
    Person,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideEligibility {
    Eligible,
    Missing,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSummary {
    pub identity: String,
    pub version: String,
    pub display_name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractSummaryV3 {
    pub contract: String,
    pub major: u16,
    pub minor: Option<u16>,
    pub description: Option<String>,
    pub available: bool,
    pub enabled: bool,
    pub enabled_major: Option<u16>,
    pub metadata_sha256: Option<String>,
    pub provider: Option<ProviderSummary>,
    pub guide_eligibility: GuideEligibility,
    pub route_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortPageV3 {
    pub contracts: Vec<ContractSummaryV3>,
    pub next_cursor: Option<String>,
}

/// Observed contract version and optional route precondition, never a Provider selector.
#[derive(Clone, Copy, Debug)]
pub struct PortContractRef<'a> {
    pub access: PortAccessV3,
    pub contract: &'a str,
    pub major: u16,
    pub route_revision: Option<&'a str>,
}

impl PortContractRef<'_> {
    fn request(self) -> Result<Value, BackendError> {
        if !is_valid_package_identity(self.contract)
            || self.major == 0
            || self
                .route_revision
                .is_some_and(|revision| !is_lower_hex(revision, 64))
        {
            return Err(invalid_request());
        }
        let mut request = json!({"access":self.access,"contract":self.contract,"major":self.major});
        if let Some(revision) = self.route_revision {
            request["routeRevision"] = revision.into();
        }
        Ok(request)
    }
}

#[derive(Clone)]
pub struct PortClientV3 {
    client: PortClient,
}

impl PortClientV3 {
    /// Requires a negotiated 3.x grant; older Hosts fail explicitly rather than downgrade.
    pub fn from_initialization(
        initialization: &Initialization,
        capabilities: CapabilityClient,
    ) -> Result<Self, BackendError> {
        let grant = initialization
            .capability(IDENTITY)
            .ok_or_else(crate::capabilities::not_granted)?;
        if grant.major != 3 {
            return Err(BackendError::new(
                "package.capability_version_unsupported",
                false,
                json!({}),
            ));
        }
        Ok(Self {
            client: PortClient::new(capabilities, grant.grant.clone()),
        })
    }

    /// One bounded page; callers own bounded traversal, caching, and catalog-change policy.
    pub async fn available(
        &self,
        access: PortAccessV3,
        cursor: Option<&str>,
    ) -> Result<PortPageV3, BackendError> {
        if cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > 2048) {
            return Err(invalid_request());
        }
        let mut request = json!({"access":access});
        if let Some(cursor) = cursor {
            request["cursor"] = cursor.into();
        }
        let response = self
            .client
            .capabilities
            .call(&self.client.grant, "available", request)
            .await?;
        if serde_json::to_vec(&response)
            .map_err(|_| invalid_response())?
            .len()
            > 32 * 1024
            || response
                .get("nextCursor")
                .is_some_and(|cursor| !cursor.is_string())
            || response
                .get("contracts")
                .and_then(Value::as_array)
                .is_none_or(|rows| {
                    rows.iter().any(|row| {
                        row.as_object().is_none_or(|fields| fields.len() != 11)
                            || row
                                .get("provider")
                                .filter(|provider| !provider.is_null())
                                .is_some_and(|provider| {
                                    provider.as_object().is_none_or(|fields| fields.len() != 4)
                                })
                    })
                })
        {
            return Err(invalid_response());
        }
        let page: PortPageV3 = serde_json::from_value(response).map_err(|_| invalid_response())?;
        if page.contracts.len() > 32
            || (page.contracts.is_empty() && page.next_cursor.is_some())
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 2048)
            || page
                .contracts
                .windows(2)
                .any(|rows| rows[0].contract >= rows[1].contract)
            || page.contracts.iter().any(|row| {
                !is_valid_package_identity(&row.contract)
                    || row.major == 0
                    || row.enabled_major == Some(0)
                    || row.description.as_ref().is_some_and(|description| {
                        description.is_empty() || description.chars().count() > 512
                    })
                    || row
                        .metadata_sha256
                        .as_ref()
                        .is_some_and(|digest| !is_lower_hex(digest, 64))
                    || row
                        .route_revision
                        .as_ref()
                        .is_some_and(|revision| !is_lower_hex(revision, 64))
                    || row.provider.as_ref().is_some_and(|provider| {
                        !is_valid_package_identity(&provider.identity)
                            || teamarness_package_contract::manifest::parse_package_version(
                                &provider.version,
                            )
                            .is_err()
                            || provider.display_name.is_empty()
                            || provider.display_name.chars().count() > 128
                            || provider.description.as_ref().is_some_and(|description| {
                                description.is_empty() || description.chars().count() > 1024
                            })
                    })
            })
        {
            return Err(invalid_response());
        }
        Ok(page)
    }

    pub async fn describe(
        &self,
        route: PortContractRef<'_>,
        metadata_sha256: &str,
    ) -> Result<CompiledPortContracts, BackendError> {
        let mut request = route.request()?;
        request["metadataSha256"] = metadata_sha256.into();
        let document = self.client.describe_request(request).await?;
        if document.contracts[0].major != route.major {
            return Err(invalid_response());
        }
        Ok(document)
    }

    pub async fn enable(&self, route: PortContractRef<'_>) -> Result<(), BackendError> {
        if route.access == PortAccessV3::Declared {
            return Err(invalid_request());
        }
        let result = self
            .client
            .capabilities
            .call(&self.client.grant, "enable", route.request()?)
            .await?;
        if result
            != json!({"access":route.access,"contract":route.contract,"major":route.major,"enabled":true})
        {
            return Err(invalid_response());
        }
        Ok(())
    }

    pub async fn disable(&self, access: PortAccessV3, contract: &str) -> Result<(), BackendError> {
        if access == PortAccessV3::Declared || !is_valid_package_identity(contract) {
            return Err(invalid_request());
        }
        let result = self
            .client
            .capabilities
            .call(
                &self.client.grant,
                "disable",
                json!({"access":access,"contract":contract}),
            )
            .await?;
        if result != json!({"access":access,"contract":contract,"enabled":false}) {
            return Err(invalid_response());
        }
        Ok(())
    }

    pub async fn command(
        &self,
        route: PortContractRef<'_>,
        port: &str,
        request: Value,
    ) -> Result<Value, BackendError> {
        self.invoke(route, port, "command", request).await
    }
    pub async fn query(
        &self,
        route: PortContractRef<'_>,
        port: &str,
        request: Value,
    ) -> Result<Value, BackendError> {
        self.invoke(route, port, "query", request).await
    }
    async fn invoke(
        &self,
        route: PortContractRef<'_>,
        port: &str,
        kind: &str,
        request: Value,
    ) -> Result<Value, BackendError> {
        let mut input = route.request()?;
        input["port"] = port.into();
        input["kind"] = kind.into();
        input["request"] = request;
        self.client
            .capabilities
            .call(&self.client.grant, "invoke", input)
            .await
            .map(decode_port_value)
    }
    pub async fn live_query(
        &self,
        route: PortContractRef<'_>,
        port: &str,
        request: Value,
    ) -> Result<CapabilitySubscription, BackendError> {
        let mut input = route.request()?;
        input["port"] = port.into();
        input["kind"] = "query".into();
        input["request"] = request;
        self.client
            .capabilities
            .subscribe(&self.client.grant, input)
            .await
            .map(CapabilitySubscription::with_port_values)
    }
    pub async fn subscribe_event(
        &self,
        route: PortContractRef<'_>,
        port: &str,
    ) -> Result<CapabilitySubscription, BackendError> {
        let mut input = route.request()?;
        input["port"] = port.into();
        input["kind"] = "event".into();
        self.client
            .capabilities
            .subscribe(&self.client.grant, input)
            .await
            .map(CapabilitySubscription::with_port_values)
    }
}
