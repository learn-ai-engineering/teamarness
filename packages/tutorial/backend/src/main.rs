#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = teamarness_tutorial_package::TutorialBackend::default()
        .serve()
        .await
    {
        eprintln!("Tutorial Package stopped: {error}");
        std::process::exit(1);
    }
}
