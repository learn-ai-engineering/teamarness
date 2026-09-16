#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = teamarness_repo_aware_package::RepoAwareBackend
        .serve()
        .await
    {
        eprintln!("Repo-aware Package stopped: {error}");
        std::process::exit(1);
    }
}