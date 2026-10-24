#![forbid(unsafe_code)]

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    teamarness_documents_package::router().run().await
}
