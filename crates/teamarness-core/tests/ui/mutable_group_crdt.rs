use teamarness_core::{CoreRuntime, CoreRuntimeConfig};

async fn main() {
    let mut runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).await.unwrap();
    let _crdt = runtime.group_crdt_mut();
}
