#![forbid(unsafe_code)]

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    teamarness_repo_aware_package::router().run().await
}
