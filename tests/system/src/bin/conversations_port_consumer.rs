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
                    // poll-interval: the surrounding timeout bounds the marker wait.
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
            let _ = publish_result(path, &output).await;
            while !directory.join("port-proof-failure-start").exists() {
                // poll-interval: this test-only marker wait ends when the Host signals failure injection.
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let after_exit = client
                .query(
                    teamarness_package_sdk::ports::PortAccess::Declared,
                    CONTRACT,
                    1,
                    "message.page",
                    json!({"channelId":GENERAL}),
                )
                .await;
            let result = match after_exit {
                Ok(_) => json!({"ok":true}),
                Err(error) => {
                    json!({"ok":false,"code":error.code(),"uncertain":error.outcome_uncertain()})
                }
            };
            let _ = publish_result(directory.join("port-proof-failure.json"), &result).await;
        });
        Ok(())
    })
    .run()
    .await
}

async fn publish_result(path: std::path::PathBuf, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    tokio::fs::write(&temporary, serde_json::to_vec(value)?).await?;
    tokio::fs::rename(temporary, path).await
}

async fn scenario(
    client: &PortClient,
    raw: &CapabilityClient,
    grant: &str,
) -> Result<Value, String> {
    let available = client
        .available(teamarness_package_sdk::ports::PortAccess::Declared, None)
        .await
        .map_err(code)?;
    let summaries = client.declared_contracts().await.map_err(code)?;
    let digest = summaries
        .iter()
        .find(|row| row.contract == CONTRACT)
        .and_then(|row| row.metadata_sha256.as_deref())
        .ok_or("proof.no_metadata")?;
    let document = client
        .describe(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            digest,
        )
        .await
        .map_err(code)?;
    let metadata = json!({"identity":document.contracts[0].identity,"ports":document.contracts[0].ports.len(),"definitions":document.definitions.keys().collect::<Vec<_>>()});
    let mut events = client
        .subscribe_event(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "messages.changed",
        )
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
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
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
        .query(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.page",
            json!({"channelId":GENERAL}),
        )
        .await
        .map_err(code)?;
    events.close().await;
    let second = client
        .command(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.post",
            json!({"channelId":GENERAL,"markdown":"port-proof-second"}),
        )
        .await
        .map_err(code)?;
    // settle: the Provider's local projection notifier has no completion signal for this
    // negative replay assertion; the bounded window precedes a fresh subscription.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut late = client
        .subscribe_event(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "messages.changed",
        )
        .await
        .map_err(code)?;
    let replay = tokio::time::timeout(Duration::from_millis(100), late.recv())
        .await
        .is_ok();
    late.close().await;
    let recovered = client
        .query(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.page",
            json!({"channelId":GENERAL}),
        )
        .await
        .map_err(code)?;
    let hidden = client
        .command(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.post",
            json!({"channelId":"ffffffffffffffffffffffffffffffff","markdown":"not-visible"}),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    let forged = raw
        .call(
            grant,
            "invoke",
            json!({
                "access":"declared","major":1,"contract":CONTRACT,"port":"message.post","kind":"command",
                "request":{"channelId":GENERAL,"markdown":"forged"},
                "actor":{"kind":"agent","identity":"forged"}
            }),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    let denied = client
        .command(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "not.granted",
            json!({}),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    let undeclared_changes = client
        .query(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.changes",
            json!({"channelId":GENERAL,"epoch":"0".repeat(32),"after":"0"}),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    let malformed = client
        .command(
            teamarness_package_sdk::ports::PortAccess::Declared,
            CONTRACT,
            1,
            "message.post",
            json!({"channelId":GENERAL,"markdown":""}),
        )
        .await
        .err()
        .map(|err| err.code().to_owned());
    Ok(
        json!({"available":available,"metadata":metadata,"initialQuiet":initial,"first":first,"event":event,
        "page":page,"second":second,"lateReplay":replay,"recovered":recovered,
        "denied":denied,"undeclaredChanges":undeclared_changes,"malformed":malformed,"forged":forged,"hidden":hidden}),
    )
}

fn code(error: BackendError) -> String {
    error.code().to_owned()
}
