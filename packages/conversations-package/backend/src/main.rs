#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = teamarness_conversations_package::ConversationsBackend::serve().await {
        eprintln!("conversations Package stopped: {error}");
        std::process::exit(1);
    }
}
