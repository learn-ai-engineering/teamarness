use resonance_core::{CoreRuntime, CoreRuntimeConfig};

fn main() {
    let runtime = CoreRuntime::open(CoreRuntimeConfig::new("private-data")).unwrap();
    let _connection = runtime.connection();
}
