use resonance_core::{CoreRuntime, CoreRuntimeConfig};

fn main() {
    let mut runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).unwrap();
    let _crdt = runtime.group_crdt_mut();
}
