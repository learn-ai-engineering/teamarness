#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{
    CloseUiMount, InstallationPublicIdentity, LifecycleState, OpenUiMount, PackageHost,
    PackageHostCommand, PackageHostConfig, PackageHostOutcome, PackageIdentity, PackageInstanceKey,
    PackageSourceKind, PackageTrustKind, PackageValue, ShutdownHost, UiRequest, UiRequestId,
    WorkspaceId,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through scripts/create-package.integration.test.mjs with built Package Directories"]
async fn fresh_bundled_tutorial_then_generated_local_package_run_together() {
    let tutorial_source = std::env::var("TEAMARNESS_BUNDLED_TUTORIAL_DIRECTORY")
        .expect("integration driver must provide the bundled Tutorial Package Directory");
    let local_source = std::env::var("TEAMARNESS_GENERATED_PACKAGE_DIRECTORY")
        .expect("integration driver must provide the generated local Package Directory");
    let local_display_name = std::env::var("TEAMARNESS_GENERATED_PACKAGE_DISPLAY_NAME")
        .expect("integration driver must provide the generated Package display name");
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("application");
    let private = root.path().join("private");
    let core_path = root.path().join("core");
    copy_directory(
        Path::new(&tutorial_source),
        &application.join("packages/dev.teamarness.tutorial"),
    );
    fs::create_dir_all(private.join("packages")).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(private.join("packages"), fs::Permissions::from_mode(0o700)).unwrap();
    let index = application.join("application-packages.v1.json");
    fs::create_dir_all(&application).unwrap();
    fs::write(
        &index,
        br#"{"schemaVersion":1,"packages":[{"directory":"packages/dev.teamarness.tutorial"}]}"#,
    )
    .unwrap();

    let core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(core_path))
            .await
            .unwrap(),
    );
    core.create_workspace("Authoring Workspace").unwrap();
    let workspace = WorkspaceId::new(core.list_workspaces().unwrap()[0].id().to_string()).unwrap();
    let installation = InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap();
    let host = PackageHost::open(
        PackageHostConfig::new(&application, &index, &private, installation.clone()),
        core.clone(),
    )
    .await
    .unwrap();
    let first = host.snapshot().await;
    assert!(first.diagnostics.is_empty(), "{:#?}", first.diagnostics);
    assert_eq!(first.packages.len(), 1);
    assert_eq!(first.packages[0].source, PackageSourceKind::Bundled);
    assert_eq!(
        first.packages[0].trust,
        PackageTrustKind::ApplicationBundled
    );
    assert!(!private.join("packages/registrations.v1.json").exists());
    assert!(
        object(
            &request(
                &host,
                instance_key("dev.teamarness.tutorial", &workspace),
                "tutorial.read"
            )
            .await
        )
        .contains_key("algorithms")
    );
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();

    let registration = private.join("packages/registrations.v1.json");
    fs::write(&registration, serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "registrations": [{
            "directory": fs::canonicalize(local_source).unwrap(),
            "expectedIdentity": "dev.teamarness.emojart",
            "trust": { "kind": "trusted_native_code_v1", "acceptedRisks": ["filesystem", "network", "process"] }
        }]
    })).unwrap()).unwrap();
    fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();

    let installation_again = installation.clone();
    let host = PackageHost::open(
        PackageHostConfig::new(&application, &index, &private, installation),
        core.clone(),
    )
    .await
    .unwrap();
    let second = host.snapshot().await;
    assert!(second.diagnostics.is_empty(), "{:#?}", second.diagnostics);
    assert_eq!(second.packages.len(), 2);
    let tutorial = second
        .packages
        .iter()
        .find(|package| package.identity.as_str() == "dev.teamarness.tutorial")
        .unwrap();
    assert_eq!(tutorial.source, PackageSourceKind::Bundled);
    let local = second
        .packages
        .iter()
        .find(|package| package.identity.as_str() == "dev.teamarness.emojart")
        .unwrap();
    assert_eq!(local.source, PackageSourceKind::Local);
    assert_eq!(local.trust, PackageTrustKind::TrustedNativeCodeV1);
    assert!(
        object(
            &request(
                &host,
                instance_key("dev.teamarness.tutorial", &workspace),
                "tutorial.read"
            )
            .await
        )
        .contains_key("algorithms")
    );
    let local_response = request(
        &host,
        instance_key("dev.teamarness.emojart", &workspace),
        "emojart.read",
    )
    .await;
    assert_eq!(
        object(&local_response).get("displayName"),
        Some(&PackageValue::String(local_display_name))
    );
    // The generated Package counts reads in its own Package Data Directory.
    assert_eq!(read_count(&local_response), 1);
    let second_read = request(
        &host,
        instance_key("dev.teamarness.emojart", &workspace),
        "emojart.read",
    )
    .await;
    assert_eq!(read_count(&second_read), 2);
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();

    // A new Host, and so a new Backend process, reads the same persisted count.
    let host = PackageHost::open(
        PackageHostConfig::new(&application, &index, &private, installation_again),
        core.clone(),
    )
    .await
    .unwrap();
    let after_restart = request(
        &host,
        instance_key("dev.teamarness.emojart", &workspace),
        "emojart.read",
    )
    .await;
    assert_eq!(read_count(&after_restart), 3);
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    core.shutdown().await.unwrap();
}

fn read_count(response: &PackageValue) -> u64 {
    match object(response).get("readCount") {
        Some(PackageValue::Unsigned(count)) => *count,
        Some(PackageValue::Integer(count)) => u64::try_from(*count).unwrap(),
        other => panic!("readCount must be a non-negative integer, got {other:?}"),
    }
}

async fn request(
    host: &PackageHost,
    instance: PackageInstanceKey,
    operation: &str,
) -> PackageValue {
    let (mount, loader) = match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount { instance }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader),
        outcome => panic!("unexpected mount outcome: {outcome:?}"),
    };
    assert_eq!(loader.package.lifecycle, LifecycleState::Active);
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("authoring-request").unwrap(),
            operation: operation.into(),
            payload: PackageValue::Object(BTreeMap::new()),
        }))
        .await
        .unwrap();
    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
        .await
        .unwrap();
    match outcome {
        PackageHostOutcome::UiResponse { payload } => payload,
        outcome => panic!("unexpected request outcome: {outcome:?}"),
    }
}

fn object(value: &PackageValue) -> &std::collections::BTreeMap<String, PackageValue> {
    let PackageValue::Object(fields) = value else {
        panic!("response is not an object")
    };
    fields
}

fn instance_key(identity: &str, workspace: &WorkspaceId) -> PackageInstanceKey {
    PackageInstanceKey::new(
        PackageIdentity::from_str(identity).unwrap(),
        workspace.clone(),
    )
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
