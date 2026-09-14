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
            ReconcileWorkspaces { workspaces: vec![] },
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
