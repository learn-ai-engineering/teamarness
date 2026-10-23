#![forbid(unsafe_code)]
//! Test-only SDK consumer: no UI operation and no product registration.
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use teamarness_package_sdk::{
    Activation, BackendError, CapabilityClient, Router, ports::PortClient,
};

const CONTRACT: &str = "dev.teamarness.conversations";
const GENERAL: &str = "00000000000000000000000000000000";

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
        tokio::spawn(async move {
            let path = directory.join("port-proof.json");
            let result = tokio::time::timeout(Duration::from_secs(20), async {
                while !directory.join("port-proof-start").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                scenario(&client, &raw, &grant).await
            })
            .await;
            let output = match result {
                Ok(Ok(value)) => json!({"ok":true,"result":value}),
                Ok(Err(code)) => json!({"ok":false,"error":code}),
                Err(_) => json!({"ok":false,"error":"proof.timeout"}),
            };
            let _ = tokio::fs::write(path, serde_json::to_vec(&output).unwrap()).await;
            while !directory.join("port-proof-failure-start").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let after_exit = client
                .query(CONTRACT, "message.page", json!({"channelId":GENERAL}))
                .await;
            let result = match after_exit {
                Ok(_) => json!({"ok":true}),
                Err(error) => {
                    json!({"ok":false,"code":error.code(),"uncertain":error.outcome_uncertain()})
                }
            };
            let _ = tokio::fs::write(
                directory.join("port-proof-failure.json"),
                serde_json::to_vec(&result).unwrap(),
            )
            .await;
        });
        Ok(())
    })
    .run()
    .await
}

async fn scenario(
    client: &PortClient,
    raw: &CapabilityClient,
    grant: &str,
) -> Result<Value, String> {
    let available = client.available().await.map_err(code)?;
    let mut events = client
        .subscribe_event(CONTRACT, "messages.changed")
        .await
        .map_err(code)?;
    // The subscription is live: it does not produce an initial snapshot.
    let initial = tokio::time::timeout(Duration::from_millis(100), events.recv())
        .await
        .is_err();
    if !initial {
        return Err("proof.initial_event".into());
    }
    let first = client
        .command(
            CONTRACT,
            "message.post",
            json!({"channelId":GENERAL,"markdown":"port-proof-first"}),
        )
        .await
        .map_err(code)?;
    let event = tokio::time::timeout(Duration::from_secs(8), events.recv())
        .await
        .map_err(|_| "proof.event_timeout".to_owned())?
        .ok_or("proof.event_closed")?
        .map_err(code)?;
    let page = client
        .query(CONTRACT, "message.page", json!({"channelId":GENERAL}))
        .await
        .map_err(code)?;
    events.close().await;
    let second = client
        .command(
            CONTRACT,
            "message.post",
            json!({"channelId":GENERAL,"markdown":"port-proof-second"}),
        )
        .await
        .map_err(code)?;
    // Let the Provider's local projection notifier settle before opening a new live stream.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut late = client
        .subscribe_event(CONTRACT, "messages.changed")
        .await
        .map_err(code)?;
    let replay = tokio::time::timeout(Duration::from_millis(100), late.recv())
        .await
        .is_ok();
    late.close().await;
    let recovered = client
        .query(CONTRACT, "message.page", json!({"channelId":GENERAL}))
        .await
        .map_err(code)?;
    let forged = raw
        .call(
            grant,
            "invoke",
            json!({
                "contract":CONTRACT,"port":"message.post","kind":"command",
                "request":{"channelId":GENERAL,"markdown":"forged"},
                "actor":{"kind":"agent","identity":"forged"}
            }),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    let denied = client
        .command(CONTRACT, "not.granted", json!({}))
        .await
        .err()
        .map(|err| err.code().to_owned());
    let malformed = client
        .command(
            CONTRACT,
            "message.post",
            json!({"channelId":GENERAL,"markdown":""}),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    Ok(
        json!({"available":available,"initialQuiet":initial,"first":first,"event":event,
        "page":page,"second":second,"lateReplay":replay,"recovered":recovered,
        "denied":denied,"malformed":malformed,"forged":forged}),
    )
}

fn code(error: BackendError) -> String {
    error.code().to_owned()
}
