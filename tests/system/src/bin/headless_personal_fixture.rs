#![forbid(unsafe_code)]
//! Unknown Personal Package used only by headless administration qualification.
use std::process::ExitCode;

use serde_json::json;
use teamarness_package_sdk::{Activation, BackendError, Router};

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation: Activation| async move {
        let directory = activation.data_directory();
        let count = directory.join("activations");
        let prior = match std::fs::read_to_string(&count) {
            Ok(text) => text.parse::<u64>().map_err(|_| storage())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return Err(storage()),
        };
        std::fs::write(count, (prior + 1).to_string()).map_err(|_| storage())?;
        let retained = directory.join("retained");
        if !retained.exists() {
            std::fs::write(retained, b"fixture-owned persistent value").map_err(|_| storage())?;
        }
        Ok(())
    })
    .run()
    .await
}

fn storage() -> BackendError {
    BackendError::new("qualification.storage", false, json!({}))
}
