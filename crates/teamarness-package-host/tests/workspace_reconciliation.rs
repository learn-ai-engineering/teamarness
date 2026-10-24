mod common;

use teamarness_package_host::{
    LifecycleState, PackageHost, PackageHostCommand, PackageHostOutcome, ReconcileWorkspaces,
};

#[tokio::test]
async fn reconciliation_eagerly_activates_the_package_workspace_product() {
    let fixture =
        common::Fixture::node(&[("one", "dev.teamarness.one"), ("two", "dev.teamarness.two")])
            .await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await;
    assert_eq!(before.instances.len(), 2);
    assert!(
        before
            .instances
            .iter()
            .all(|instance| instance.lifecycle == LifecycleState::Active)
    );
    let existing = before
        .instances
        .iter()
        .map(|instance| (instance.key.clone(), instance.process_generation))
        .collect::<std::collections::BTreeMap<_, _>>();

    fixture.core.create_workspace("Second Workspace").unwrap();
    let outcome = host
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, PackageHostOutcome::Reconciled { .. }));

    let after = host.snapshot().await;
    assert_eq!(after.instances.len(), 4);
    assert!(
        after
            .instances
            .iter()
            .all(|instance| instance.lifecycle == LifecycleState::Active)
    );
    for instance in &after.instances {
        if let Some(generation) = existing.get(&instance.key) {
            assert_eq!(&instance.process_generation, generation);
        } else {
            assert_eq!(instance.process_generation, Some(1));
        }
    }
}

fn activation(fixture: &common::Fixture, directory: &str, value: &str) {
    let path = fixture
        .config
        .application_root()
        .join(directory)
        .join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["activation"] = value.into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn on_demand_slots_are_prepared_but_declarations_and_reconciliation_do_not_wake_them() {
    let fixture = common::Fixture::node(&[
        ("provider", "com.example.provider"),
        ("consumer", "com.example.consumer"),
    ])
    .await;
    activation(&fixture, "provider", "on_demand");
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let before = host.snapshot().await;
    assert_eq!(before.instances.len(), 2);
    assert!(
        before
            .instances
            .iter()
            .any(
                |slot| slot.key.package_identity.as_str() == "com.example.provider"
                    && slot.lifecycle == LifecycleState::Admitted
                    && slot.process_generation.is_none()
            )
    );
    assert!(
        before
            .instances
            .iter()
            .any(
                |slot| slot.key.package_identity.as_str() == "com.example.consumer"
                    && slot.lifecycle == LifecycleState::Active
            )
    );
    fixture.core.create_workspace("Next Workspace").unwrap();
    host.execute(PackageHostCommand::ReconcileWorkspaces(
        ReconcileWorkspaces {},
    ))
    .await
    .unwrap();
    let after = host.snapshot().await;
    assert_eq!(after.instances.len(), 4);
    assert_eq!(
        after
            .instances
            .iter()
            .filter(
                |slot| slot.key.package_identity.as_str() == "com.example.provider"
                    && slot.lifecycle == LifecycleState::Admitted
                    && slot.process_generation.is_none()
            )
            .count(),
        2
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost {},
    ))
    .await
    .unwrap();
    let reopened = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .snapshot()
            .await
            .instances
            .iter()
            .filter(
                |slot| slot.key.package_identity.as_str() == "com.example.provider"
                    && slot.lifecycle == LifecycleState::Admitted
                    && slot.process_generation.is_none()
            )
            .count(),
        2
    );
    reopened
        .execute(PackageHostCommand::Shutdown(
            teamarness_package_host::ShutdownHost {},
        ))
        .await
        .unwrap();
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn on_demand_mount_and_parallel_explicit_activation_start_one_generation() {
    use teamarness_package_host::{OpenUiMount, PackageInstanceKey, RecoverPackage, WorkspaceId};
    let fixture = common::Fixture::node(&[("provider", "com.example.provider")]).await;
    activation(&fixture, "provider", "on_demand");
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let workspace =
        WorkspaceId::new(fixture.core.list_workspaces().unwrap()[0].id().to_string()).unwrap();
    let key = PackageInstanceKey::new("com.example.provider".parse().unwrap(), workspace);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let host = host.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            host.execute(PackageHostCommand::Recover(RecoverPackage {
                instance: key,
            }))
            .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(
        host.snapshot().await.instances[0].process_generation,
        Some(1)
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost {},
    ))
    .await
    .unwrap();
    let reopened = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened.snapshot().await.instances[0].lifecycle,
        LifecycleState::Admitted
    );
    let result = reopened
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key,
        }))
        .await
        .unwrap();
    assert!(matches!(
        result,
        teamarness_package_host::PackageHostOutcome::UiMountOpened { .. }
    ));
    assert_eq!(
        reopened.snapshot().await.instances[0].process_generation,
        Some(1)
    );
    reopened
        .execute(PackageHostCommand::Shutdown(
            teamarness_package_host::ShutdownHost {},
        ))
        .await
        .unwrap();
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn registrations_rescan_only_on_reopen_not_workspace_reconciliation() {
    let fixture = common::Fixture::node(&[("initial", "com.example.initial")]).await;
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let added = fixture.config.application_root().join("added");
    common::write_node_package(&added, "com.example.added");
    let path = fixture.config.application_index();
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    index["packages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"directory":"added"}));
    std::fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
    fixture.core.create_workspace("More").unwrap();
    host.execute(PackageHostCommand::ReconcileWorkspaces(
        ReconcileWorkspaces {},
    ))
    .await
    .unwrap();
    assert_eq!(host.snapshot().await.packages.len(), 1);
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost {},
    ))
    .await
    .unwrap();
    let reopened = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert_eq!(reopened.snapshot().await.packages.len(), 2);
    assert_eq!(reopened.snapshot().await.instances.len(), 4);
    reopened
        .execute(PackageHostCommand::Shutdown(
            teamarness_package_host::ShutdownHost {},
        ))
        .await
        .unwrap();
}

// Verifies: TEAM.SYS.PKG-R13
#[tokio::test]
async fn authoring_catalog_reports_optional_availability_without_waking_provider() {
    let fixture = common::Fixture::node(&[
        ("consumer", "com.example.consumer"),
        ("provider", "com.example.provider"),
    ])
    .await;
    activation(&fixture, "provider", "on_demand");
    let root = fixture.config.application_root();
    let provider = root.join("provider/teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&provider).unwrap()).unwrap();
    let doc = serde_json::json!({"schemaVersion":1,"$defs":{},"contracts":[{
        "identity":"com.example.shared","major":1,"minor":1,"description":"Fixture values.",
        "ports":[{"name":"fixture.read","kind":"query","description":"Read fixture values.","request":true,"response":true}]}]});
    let bytes = serde_json::to_vec(&doc).unwrap();
    std::fs::write(root.join("provider/ports.json"), &bytes).unwrap();
    manifest["portContract"] =
        serde_json::json!({"path":"ports.json","sha256":common::digest(&bytes)});
    std::fs::write(&provider, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let consumer = root.join("consumer/teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&consumer).unwrap()).unwrap();
    manifest["consumedPorts"] = serde_json::json!([
        {"identity":"com.example.shared","major":1,"minMinor":0,"maxMinor":2,"ports":[{"name":"fixture.read","kind":"query"}]},
        {"identity":"com.example.absent","major":1,"minMinor":0,"maxMinor":1,"required":false,"ports":[{"name":"fixture.read","kind":"query"}]}]);
    std::fs::write(&consumer, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let entry = host
        .authoring_catalog()
        .into_iter()
        .find(|entry| entry.package.identity.as_str() == "com.example.consumer")
        .unwrap();
    assert_eq!(entry.availability.len(), 2);
    assert_eq!(entry.availability[0].minor, Some(1));
    assert_eq!(
        entry.availability[1].unavailable,
        Some(teamarness_package_host::PackageErrorCode::PortContractMissing)
    );
    assert_eq!(
        host.snapshot()
            .await
            .instances
            .iter()
            .find(|slot| slot.key.package_identity.as_str() == "com.example.provider")
            .unwrap()
            .lifecycle,
        LifecycleState::Admitted
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost {},
    ))
    .await
    .unwrap();
}

// Verifies: TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[tokio::test]
async fn authoring_catalog_keeps_valid_declarations_rejected_by_graph() {
    let fixture = common::Fixture::node(&[("consumer", "com.example.consumer")]).await;
    let path = fixture
        .config
        .application_root()
        .join("consumer/teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["consumedPorts"] = serde_json::json!([{
        "identity":"com.example.missing","major":1,"minMinor":0,"maxMinor":1,
        "ports":[{"name":"fixture.read","kind":"query"}]}]);
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert!(host.snapshot().await.packages.is_empty());
    let catalog = host.authoring_catalog();
    assert_eq!(catalog.len(), 1);
    assert_eq!(
        catalog[0].availability[0].unavailable,
        Some(teamarness_package_host::PackageErrorCode::PortContractMissing)
    );
    assert_eq!(
        catalog[0].diagnostics[0].code,
        teamarness_package_host::PackageErrorCode::PortContractMissing
    );
    host.execute(PackageHostCommand::Shutdown(
        teamarness_package_host::ShutdownHost {},
    ))
    .await
    .unwrap();
}
