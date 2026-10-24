#![forbid(unsafe_code)]
//! The Documents Backend the system tests drive: the shipped router plus `documents.test.type`,
//! which stands in for a person's editor session. The shipped Backend is `router()` alone.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    teamarness_documents_package::system_test_router()
        .run()
        .await
}
