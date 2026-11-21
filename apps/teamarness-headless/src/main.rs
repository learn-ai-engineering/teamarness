#[tokio::main]
async fn main() {
    std::process::exit(teamarness_headless::cli::run(std::env::args_os().skip(1)).await);
}
