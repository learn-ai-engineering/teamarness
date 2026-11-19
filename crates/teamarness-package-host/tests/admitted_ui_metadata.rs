mod common;

use serde_json::{Value, json};
use teamarness_package_host::{LifecycleState, PackageHost};

#[tokio::test]
async fn metadata_facade_is_read_only_and_never_activates_or_mounts_an_on_demand_instance() {
    let fixture = common::Fixture::node(&[("fixture", "dev.teamarness.fixture")]).await;
    let directory = fixture.config.application_root().join("fixture");
    let manifest_path = directory.join("teamarness.package.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["activation"] = json!("on_demand");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let source: Value =
        serde_json::from_slice(&std::fs::read(directory.join("ui/contract.json")).unwrap())
            .unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await;
    let instance = before.instances[0].key.clone();
    assert_eq!(before.instances[0].lifecycle, LifecycleState::Admitted);
    let operations = host.package_operations(&instance).await.unwrap();
    let schema = host
        .package_schema(&instance, "fixture.immediate")
        .await
        .unwrap();
    assert_eq!(
        operations["contractDigest"],
        manifest["ui"]["contract"]["sha256"]
    );
    assert_eq!(schema["contractDigest"], operations["contractDigest"]);
    assert_eq!(schema["schema"]["$defs"], source["$defs"]);
    assert_eq!(
        schema["schema"]["requests"],
        json!({"fixture.immediate":source["requests"]["fixture.immediate"]})
    );
    assert_eq!(
        host.snapshot().await,
        before,
        "discovery causes no Host revision, activation or mount"
    );
    std::fs::write(
        directory.join("ui/contract.json"),
        b"changed source is not admitted",
    )
    .unwrap();
    assert_eq!(
        host.package_schema(&instance, "fixture.immediate")
            .await
            .unwrap(),
        schema
    );
    assert!(host.package_schema(&instance, "unknown").await.is_err());
    host.execute(teamarness_package_host::PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost,
    ))
    .await
    .unwrap();
    fixture.core.shutdown().await.unwrap();
}
