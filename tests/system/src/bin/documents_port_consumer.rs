#![forbid(unsafe_code)]
//! Test-only SDK consumer of Documents Port Contract 1.x: no UI operation and no product
//! registration. It relays numbered steps the system test writes into its Package Data
//! Directory (`step-<n>.json`) to the Host broker, and answers each in `answer-<n>.json`.
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use teamarness_package_sdk::{
    Activation, BackendError, CapabilityClient, CapabilitySubscription, Router, ports::PortClient,
};

const CONTRACT: &str = "dev.teamarness.documents";

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation: Activation| async move {
        let grant = activation
            .initialization()
            .capability("teamarness.package-ports")
            .ok_or_else(|| BackendError::new("proof.no_grant", false, json!({})))?
            .grant
            .clone();
        let raw = activation.capabilities().clone();
        let client = PortClient::new(raw.clone(), grant.clone());
        let directory = activation.data_directory().to_owned();
        tokio::fs::write(directory.join("port-proof-ready"), b"")
            .await
            .map_err(|_| BackendError::new("proof.storage", false, json!({})))?;
        tokio::spawn(relay(directory, client, raw, grant));
        Ok(())
    })
    .run()
    .await
}

/// Answers every step in order, for as long as the Package runs.
async fn relay(directory: PathBuf, client: PortClient, raw: CapabilityClient, grant: String) {
    let mut events: Option<CapabilitySubscription> = None;
    for step in 0u64.. {
        let path = directory.join(format!("step-{step}.json"));
        let request = loop {
            if let Ok(bytes) = tokio::fs::read(&path).await
                && let Ok(request) = serde_json::from_slice::<Value>(&bytes)
            {
                break request;
            }
            // poll-interval: the system test bounds every step it waits for with its own deadline.
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let answer = answer(&client, &raw, &grant, &mut events, &request).await;
        if publish(&directory.join(format!("answer-{step}.json")), &answer)
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn answer(
    client: &PortClient,
    raw: &CapabilityClient,
    grant: &str,
    events: &mut Option<CapabilitySubscription>,
    request: &Value,
) -> Value {
    let port = request["port"].as_str().unwrap_or_default();
    let body = request["request"].clone();
    let result = match request["op"].as_str().unwrap_or_default() {
        "query" => client.query(CONTRACT, port, body).await,
        "command" => client.command(CONTRACT, port, body).await,
        "available" => client.available().await,
        // A call that forges its own actor, which the Host must refuse.
        "forged" => {
            raw.call(
                grant,
                "invoke",
                json!({
                    "contract":CONTRACT,"port":port,"kind":"command","request":body,
                    "actor":{"kind":"person","identity":"forged"}
                }),
            )
            .await
        }
        "subscribe" => match client.subscribe_event(CONTRACT, port).await {
            Ok(subscription) => {
                *events = Some(subscription);
                Ok(json!({}))
            }
            Err(error) => Err(error),
        },
        "next_event" => {
            let Some(subscription) = events.as_mut() else {
                return json!({"ok":false,"code":"proof.no_subscription"});
            };
            let wait = Duration::from_millis(request["timeoutMs"].as_u64().unwrap_or(100));
            match tokio::time::timeout(wait, subscription.recv()).await {
                Err(_) => return json!({"ok":false,"timeout":true}),
                Ok(None) => return json!({"ok":false,"code":"proof.event_closed"}),
                Ok(Some(event)) => event,
            }
        }
        "close" => {
            if let Some(mut subscription) = events.take() {
                subscription.close().await;
            }
            Ok(json!({}))
        }
        other => return json!({"ok":false,"code":format!("proof.unknown_op.{other}")}),
    };
    match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({
            "ok": false,
            "code": error.code(),
            "provider": error.details()["provider"]["code"],
            "uncertain": error.outcome_uncertain(),
        }),
    }
}

async fn publish(path: &Path, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    tokio::fs::write(&temporary, serde_json::to_vec(value)?).await?;
    tokio::fs::rename(temporary, path).await
}
