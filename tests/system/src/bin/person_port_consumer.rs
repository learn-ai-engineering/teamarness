#![forbid(unsafe_code)]
//! Test-only paged client. Scripts supply observed metadata, never an actor or Provider.
use std::process::ExitCode;

use serde_json::{Value, json};
use teamarness_package_sdk::{
    Activation, BackendError, Router,
    ports::{PortAccess, PortClient},
    ports_v3::{PortAccessV3, PortClientV3, PortContractRef},
};

enum Client {
    Legacy(PortClient),
    Person(PortClientV3),
}

fn invalid() -> BackendError {
    BackendError::new("proof.invalid_script", false, json!({}))
}
fn field<'a>(input: &'a Value, name: &str) -> Result<&'a str, BackendError> {
    input[name].as_str().ok_or_else(invalid)
}

impl Client {
    async fn execute(&self, input: Value) -> Result<Value, BackendError> {
        let action = field(&input, "action")?;
        let access: PortAccessV3 =
            serde_json::from_value(input["access"].clone()).map_err(|_| invalid())?;
        let cursor = input["cursor"].as_str();
        if action == "page" {
            return match self {
                Self::Person(client) => {
                    serde_json::to_value(client.available(access, cursor).await?)
                        .map_err(|_| invalid())
                }
                Self::Legacy(client) => {
                    serde_json::to_value(client.available(legacy(access)?, cursor).await?)
                        .map_err(|_| invalid())
                }
            };
        }
        let contract = field(&input, "contract")?;
        if action == "disable" {
            match self {
                Self::Person(client) => client.disable(access, contract).await?,
                Self::Legacy(client) => client.disable(contract).await?,
            }
            return Ok(json!({}));
        }
        let major = input["major"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(invalid)?;
        let route = PortContractRef {
            access,
            contract,
            major,
            route_revision: input["routeRevision"].as_str(),
        };
        if action == "enable" {
            match self {
                Self::Person(client) => client.enable(route).await?,
                Self::Legacy(client) => client.enable(contract, major).await?,
            }
            return Ok(json!({}));
        }
        if action == "describe" {
            let digest = field(&input, "metadataSha256")?;
            let document = match self {
                Self::Person(client) => client.describe(route, digest).await?,
                Self::Legacy(client) => client.describe(legacy(access)?, contract, digest).await?,
            };
            return Ok(
                json!({"schemaVersion":1,"$defs":document.definitions,"contracts":document.contracts}),
            );
        }
        let port = field(&input, "port")?;
        let request = input.get("request").cloned().unwrap_or_else(|| json!({}));
        match (self, action) {
            (Self::Person(client), "query") => client.query(route, port, request).await,
            (Self::Person(client), "command") => client.command(route, port, request).await,
            (Self::Legacy(client), "query") => {
                client
                    .query(legacy(access)?, contract, major, port, request)
                    .await
            }
            (Self::Legacy(client), "command") => {
                client
                    .command(legacy(access)?, contract, major, port, request)
                    .await
            }
            _ => Err(invalid()),
        }
    }
}
fn legacy(access: PortAccessV3) -> Result<PortAccess, BackendError> {
    match access {
        PortAccessV3::Declared => Ok(PortAccess::Declared),
        PortAccessV3::Agent => Ok(PortAccess::Agent),
        PortAccessV3::Person => Err(invalid()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation: Activation| async move {
        let initialization = activation.initialization();
        let grant = initialization.capability("teamarness.package-ports").ok_or_else(invalid)?;
        match grant.major {
            2 => Ok(Client::Legacy(PortClient::new(activation.capabilities().clone(), grant.grant.clone()))),
            3 => Ok(Client::Person(PortClientV3::from_initialization(initialization, activation.capabilities().clone())?)),
            _ => Err(invalid()),
        }
    })
    .request("proof.execute", |context, input: Value| async move {
        Ok::<_, BackendError>(match context.state().execute(input).await {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(error) => json!({"ok":false,"error":error.code(),"uncertain":error.outcome_uncertain(),"retryable":error.retryable()}),
        })
    })
    .run().await
}
