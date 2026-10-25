#![forbid(unsafe_code)]
//! Test-only scripted client. Provider names and operation shapes come from Host metadata.
use std::process::ExitCode;

use serde_json::{Value, json};
use teamarness_package_sdk::{
    Activation, BackendError, CapabilitySubscription, Router,
    capabilities::agent_binding::AgentBinding,
    ports::{PortAccess, PortClient},
};
use tokio::sync::Mutex;

struct Client {
    ports: PortClient,
    binding: AgentBinding,
    stream: Mutex<Option<CapabilitySubscription>>,
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
        match action {
            "bind" => {
                let agent = self.binding.bind(field(&input, "principalId")?).await?;
                return Ok(json!({"principalId":agent.principal_id}));
            }
            "unbind" => {
                self.binding.unbind().await?;
                return Ok(json!({}));
            }
            "next" => {
                return self
                    .stream
                    .lock()
                    .await
                    .as_mut()
                    .ok_or_else(invalid)?
                    .recv()
                    .await
                    .ok_or_else(|| BackendError::new("proof.stream_ended", false, json!({})))?;
            }
            "close" => {
                if let Some(mut stream) = self.stream.lock().await.take() {
                    stream.close().await;
                }
                return Ok(json!({}));
            }
            "disable" => {
                self.ports.disable(field(&input, "contract")?).await?;
                return Ok(json!({}));
            }
            _ => {}
        }
        let summaries = self.ports.agent_contracts().await?;
        if action == "discover" {
            let mut rows = Vec::new();
            for summary in summaries {
                let document = if let Some(digest) = summary.metadata_sha256.as_deref() {
                    // The SDK assembles complete chunks, verifies their hash and compiles all schemas.
                    let compiled = self
                        .ports
                        .describe(PortAccess::Agent, &summary.contract, digest)
                        .await?;
                    json!({"schemaVersion":1,"$defs":compiled.definitions,"contracts":compiled.contracts})
                } else {
                    Value::Null
                };
                rows.push(json!({"summary":summary,"document":document}));
            }
            return Ok(json!(rows));
        }
        let contract = field(&input, "contract")?;
        let summary = summaries
            .iter()
            .find(|row| row.contract == contract)
            .ok_or_else(invalid)?;
        if action == "enable" {
            self.ports.enable(contract, summary.major).await?;
            return Ok(json!({}));
        }
        let digest = summary.metadata_sha256.as_deref().ok_or_else(invalid)?;
        let compiled = self
            .ports
            .describe(PortAccess::Agent, contract, digest)
            .await?;
        let name = field(&input, "port")?;
        let port = compiled.contracts[0]
            .ports
            .iter()
            .find(|port| port.name == name)
            .ok_or_else(invalid)?;
        let kind = serde_json::to_value(port.kind).map_err(|_| invalid())?;
        let request = input.get("request").cloned().unwrap_or_else(|| json!({}));
        if kind != "event" {
            compiled
                .validate(contract, name, "request", &request)
                .map_err(|_| invalid())?;
        }
        if action == "subscribe" {
            let stream = match kind.as_str() {
                Some("event") => {
                    self.ports
                        .subscribe_event(PortAccess::Agent, contract, summary.major, name)
                        .await?
                }
                Some("query") if port.event.is_some() => {
                    self.ports
                        .live_query(PortAccess::Agent, contract, summary.major, name, request)
                        .await?
                }
                _ => return Err(invalid()),
            };
            if let Some(mut previous) = self.stream.lock().await.replace(stream) {
                previous.close().await;
            }
            return Ok(json!({}));
        }
        if action != "invoke" || port.response.is_none() {
            return Err(invalid());
        }
        let response = match kind.as_str() {
            Some("command") => {
                self.ports
                    .command(PortAccess::Agent, contract, summary.major, name, request)
                    .await?
            }
            Some("query") => {
                self.ports
                    .query(PortAccess::Agent, contract, summary.major, name, request)
                    .await?
            }
            _ => return Err(invalid()),
        };
        compiled
            .validate(contract, name, "response", &response)
            .map_err(|_| invalid())?;
        Ok(response)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation: Activation| async move {
        let initialization = activation.initialization();
        let grant = initialization.capability("teamarness.package-ports").ok_or_else(invalid)?.grant.clone();
        Ok(Client {
            ports: PortClient::new(activation.capabilities().clone(), grant),
            binding: AgentBinding::from_initialization(initialization, activation.capabilities().clone())?,
            stream: Mutex::new(None),
        })
    })
    .request("proof.execute", |context, input: Value| async move {
        Ok::<_, BackendError>(match context.state().execute(input).await {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(error) => json!({"ok":false,"error":error.code(),"uncertain":error.outcome_uncertain(),"retryable":error.retryable()}),
        })
    })
    .run().await
}
