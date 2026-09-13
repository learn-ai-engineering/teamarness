use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

async fn main() {
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).await.unwrap();
    let _serialized = runtime.serialize_secret_key();
}
