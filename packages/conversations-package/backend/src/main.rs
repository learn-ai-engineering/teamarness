#![forbid(unsafe_code)]

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::var("TEAMARNESS_CONVERSATIONS_PORT_DIAGNOSTICS").as_deref() != Ok("1") {
        return teamarness_conversations_package::router().run().await;
    }
    let (package, mut observations) = teamarness_package_sdk::Package::from_backend(
        teamarness_conversations_package::router().into_backend(),
    )
    .observe_ports();
    let diagnostics = tokio::spawn(async move {
        loop {
            match observations.recv().await {
                Ok(observation) => eprintln!(
                    "{}",
                    serde_json::json!({
                        "kind":"conversations.port", "providerId":observation.provider_id,
                        "operation":observation.operation, "stage":format!("{:?}", observation.stage),
                        "code":observation.code, "outcomeUncertain":observation.outcome_uncertain,
                    })
                ),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => eprintln!(
                    "{}",
                    serde_json::json!({"kind":"conversations.port.lagged", "dropped":count})
                ),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let result = package.serve().await;
    diagnostics.abort();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Package Backend stopped: {error}");
            ExitCode::FAILURE
        }
    }
}
