#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = teamarness_emojart_package::EmojartBackend::default()
        .serve()
        .await
    {
        eprintln!("Emojart Package stopped: {error}");
        std::process::exit(1);
    }
}
