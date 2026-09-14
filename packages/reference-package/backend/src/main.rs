#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = teamarness_reference_package::ReferenceBackend::serve().await {
        eprintln!("reference Package stopped: {error}");
        std::process::exit(1);
    }
}
