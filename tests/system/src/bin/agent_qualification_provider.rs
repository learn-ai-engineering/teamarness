#![forbid(unsafe_code)]
//! Test-only Provider with an identity absent from the Agent's historical catalog.
use std::{process::ExitCode, sync::Mutex};

use serde_json::{Value, json};
use teamarness_package_sdk::{Activation, BackendError, Context, Empty, Router, ports::PortActor};

const CONTRACT: &str = "dev.teamarness.qualification";

#[derive(Default)]
struct Provider(Mutex<Vec<Value>>);

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|_: Activation| async { Ok(Provider::default()) })
        .request("proof.rows", |context: Context<Provider>, _: Empty| async move {
            Ok::<_, BackendError>(json!({"rows": *context.state().0.lock().unwrap()}))
        })
        .port_request(CONTRACT, "runtime.guide", "query", |_: Context<Provider>, _: PortActor, _: Empty| async {
            Ok::<_, BackendError>(json!({"markdown": "# Qualification\nRead the sample, then write one value. Use the tool schemas, not this guide, for arguments. Never replay an uncertain write."}))
        })
        .port_request(CONTRACT, "sample.read", "query", |_: Context<Provider>, actor: PortActor, request: Value| async move {
            Ok::<_, BackendError>(json!({"answer": request["offset"].as_u64().unwrap() + 35, "actor": actor}))
        })
        .port_request(CONTRACT, "sample.write", "command", |context: Context<Provider>, actor: PortActor, request: Value| async move {
            if request["value"] == "certain-refusal" {
                return Err(BackendError::new("qualification.refused", false, json!({})));
            }
            let row = json!({"value": request["value"], "actor": actor});
            context.state().0.lock().unwrap().push(row.clone());
            Ok(row)
        })
        .run().await
}
