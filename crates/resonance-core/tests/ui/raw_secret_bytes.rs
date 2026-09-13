use resonance_core::{CoreRuntime, CoreRuntimeConfig};

async fn main() {
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).await.unwrap();
    let _bytes = runtime.secret_key().to_bytes();
}
