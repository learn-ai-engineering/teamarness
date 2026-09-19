use teamarness_core::CoreRuntime;

fn low_level(bytes: &[u8]) {
    let _ = CoreRuntime::authenticate_membership_operation(bytes);
}

fn main() {}
