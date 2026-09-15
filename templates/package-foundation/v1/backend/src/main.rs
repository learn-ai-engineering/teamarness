use teamarness_generated_package_backend::GeneratedBackend;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), teamarness_package_sdk::SdkError> {
    GeneratedBackend::serve().await
}
