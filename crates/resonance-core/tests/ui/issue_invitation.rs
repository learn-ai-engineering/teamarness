use resonance_core::CoreRuntime;

fn low_level(runtime: &CoreRuntime) {
    let _ = runtime.issue_invitation(todo!(), i64::MAX);
}

fn main() {}
