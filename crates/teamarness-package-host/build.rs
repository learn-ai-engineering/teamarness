fn main() {
    println!(
        "cargo:rustc-env=TEAMARNESS_HOST_TARGET={}",
        std::env::var("TARGET").expect("Cargo provides TARGET")
    );
}
