use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{
    CloseUiMount, DisposePackage, InstallationPublicIdentity, LifecycleState, OpenUiMount,
    OpenUiSubscription, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageHostNotification, PackageHostOutcome, PackageHostSubscriptionSelector, PackageIdentity,
    PackageInstanceKey, PackageValue, RecoverPackage, ShutdownHost, SuspendPackage, UiRequest,
    UiRequestId, UiSubscriptionId, WorkspaceId,
};

/// Waits for a condition instead of guessing how long it takes.
async fn eventually<F, Fut>(timeout: Duration, what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let expiry = std::time::Instant::now() + timeout;
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < expiry,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundled_local_and_version_restarts_preserve_reference_state_and_processes() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("application");
    let private = root.path().join("private");
    let core_path = root.path().join("core");
    fs::create_dir_all(&application).unwrap();
    fs::create_dir_all(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let backend = reference_backend(root.path());
    write_package(
        &application.join("reference"),
        "dev.teamarness.reference",
        "1.0.0",
        &backend,
    );
    write_package(
        &application.join("other"),
        "com.example.reference-copy",
        "1.0.0",
        &backend,
    );
    let index = application.join("application-packages.v1.json");
    fs::write(
        &index,
        br#"{"schemaVersion":1,"packages":[{"directory":"reference"},{"directory":"other"}]}"#,
    )
    .unwrap();

    let core = Arc::new(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(core_path))
            .await
            .unwrap(),
    );
    core.create_workspace("Workspace One").unwrap();
    core.create_workspace("Workspace Two").unwrap();
    let workspaces = core.list_workspaces().unwrap();
    let workspace = WorkspaceId::new(workspaces[0].id().to_string()).unwrap();
    let other_workspace = WorkspaceId::new(workspaces[1].id().to_string()).unwrap();
    let installation = InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap();
    let config = PackageHostConfig::new(&application, &index, &private, installation);

    let host = PackageHost::open(config.clone(), core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert_eq!(snapshot.instances.len(), 4);
    assert!(
        snapshot
            .instances
            .iter()
            .all(|instance| instance.lifecycle == LifecycleState::Active)
    );
    assert_eq!(live_lease_count(&private), 4);
    for identity in ["dev.teamarness.reference", "com.example.reference-copy"] {
        for workspace_id in [&workspace, &other_workspace] {
            assert_eq!(
                snapshot
                    .instances
                    .iter()
                    .filter(|instance| {
                        instance.key.package_identity.as_str() == identity
                            && &instance.key.workspace_id == workspace_id
                    })
                    .count(),
                1
            );
        }
    }

    let key = instance_key("dev.teamarness.reference", &workspace);
    let other_key = instance_key("com.example.reference-copy", &workspace);
    let first = read_reference(&host, &key).await;
    assert_eq!(integer(&first, "counter"), 0);
    let incremented = request(&host, &key, "reference.increment").await;
    assert_eq!(integer(&incremented, "counter"), 1);
    assert_eq!(
        integer(&read_reference(&host, &other_key).await, "counter"),
        0
    );

    let generation = instance_generation(&host, &key).await;
    let (mount, _) = open_mount(&host, &key).await;
    host.execute(PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
        mount: mount.clone(),
        subscription: UiSubscriptionId::new("changes").unwrap(),
        operation: "reference.changes".into(),
        payload: PackageValue::Object(Default::default()),
    }))
    .await
    .unwrap();
    let mut events = host
        .subscribe(PackageHostSubscriptionSelector::UiEvents {
            mount: mount.clone(),
            subscription: UiSubscriptionId::new("changes").unwrap(),
        })
        .await
        .unwrap();
    assert!(matches!(
        events.changed().await.unwrap(),
        PackageHostNotification::UiEvent(_)
    ));
    let (replacement, _) = open_mount(&host, &key).await;
    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount {
        mount: replacement,
    }))
    .await
    .unwrap();
    assert_eq!(instance_generation(&host, &key).await, generation);
    let baseline = integer(&first, "backgroundMarker");
    eventually(
        Duration::from_secs(5),
        "background activity advances",
        || async { integer(&read_reference(&host, &key).await, "backgroundMarker") > baseline },
    )
    .await;
    let active_marker = integer(&read_reference(&host, &key).await, "backgroundMarker");
    assert!(active_marker > baseline);

    host.execute(PackageHostCommand::Suspend(SuspendPackage {
        instance: key.clone(),
    }))
    .await
    .unwrap();
    // negative-wait: a bounded window proving a suspended Backend's marker stays put.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(persisted_marker(&private, &key), active_marker);
    host.execute(PackageHostCommand::Recover(RecoverPackage {
        instance: key.clone(),
    }))
    .await
    .unwrap();
    assert_eq!(instance_generation(&host, &key).await, generation);
    eventually(
        Duration::from_secs(5),
        "a recovered Backend advances its marker again",
        || async {
            integer(&read_reference(&host, &key).await, "backgroundMarker") > active_marker
        },
    )
    .await;
    host.execute(PackageHostCommand::Dispose(DisposePackage {
        instance: other_key.clone(),
    }))
    .await
    .unwrap();
    assert_eq!(
        instance_lifecycle(&host, &other_key).await,
        LifecycleState::Disposed
    );

    let local = root.path().join("local-reference");
    copy_directory(&application.join("reference"), &local);
    assert_declared_bytes_equal(&application.join("reference"), &local);
    write_registration(&private, &local);
    assert_eq!(source_for(&host).await, "bundled");
    shutdown(host).await;

    let host = PackageHost::open(config.clone(), core.clone())
        .await
        .unwrap();
    assert_eq!(source_for(&host).await, "local");
    let (local_mount, local_loader) = open_mount(&host, &key).await;
    assert_eq!(
        local_loader.package.source,
        teamarness_package_host::PackageSourceKind::Local
    );
    assert_eq!(
        local_loader.package.trust,
        teamarness_package_host::PackageTrustKind::TrustedNativeCodeV1
    );
    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount {
        mount: local_mount,
    }))
    .await
    .unwrap();
    assert_eq!(integer(&read_reference(&host, &key).await, "counter"), 1);
    shutdown(host).await;

    rewrite_version(&local, "1.1.0");
    let host = PackageHost::open(config.clone(), core.clone())
        .await
        .unwrap();
    let catalog = host
        .snapshot()
        .await
        .packages
        .into_iter()
        .find(|package| package.identity.as_str() == "dev.teamarness.reference")
        .unwrap();
    assert_eq!(catalog.version, "1.1.0");
    assert_eq!(integer(&read_reference(&host, &key).await, "counter"), 1);
    shutdown(host).await;

    fs::remove_file(private.join("packages/registrations.v1.json")).unwrap();
    let host = PackageHost::open(config, core.clone()).await.unwrap();
    assert_eq!(source_for(&host).await, "bundled");
    assert_eq!(integer(&read_reference(&host, &key).await, "counter"), 1);
    shutdown(host).await;
    core.shutdown().await.unwrap();
}

#[test]
fn reference_operations_do_not_enter_host_or_tauri_lifecycle_interfaces() {
    let host = include_str!("../src/lib.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    let tauri =
        fs::read_to_string("../../apps/teamarness-desktop/src-tauri/src/package_commands.rs")
            .unwrap();
    for operation in ["reference.read", "reference.increment", "reference.changes"] {
        assert!(!host.contains(operation));
        assert!(
            !tauri
                .split("#[cfg(test)]")
                .next()
                .unwrap()
                .contains(operation)
        );
    }
}

fn reference_backend(root: &Path) -> PathBuf {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let existing = repository.join("target/debug/teamarness-reference-package");
    if existing.is_file() {
        return existing;
    }
    let target = root.join("reference-build");
    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "teamarness-reference-package",
            "--locked",
            "--target-dir",
        ])
        .arg(&target)
        .current_dir(repository)
        .status()
        .unwrap();
    assert!(status.success());
    target.join("debug/teamarness-reference-package")
}

fn write_package(root: &Path, identity: &str, version: &str, backend_source: &Path) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let backend = fs::read(backend_source).unwrap();
    let backend_path = root.join("backend/teamarness-reference-package-aarch64-apple-darwin");
    fs::write(&backend_path, &backend).unwrap();
    fs::set_permissions(&backend_path, fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let contract = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/reference-package/ui-contract.json"),
    )
    .unwrap();
    fs::write(root.join("ui/index.js"), ui).unwrap();
    fs::write(root.join("ui/contract.json"), &contract).unwrap();
    fs::write(root.join("teamarness.package.json"), serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,
        "identity":identity,
        "version":version,
        "presentation":{"displayName":"Reference Package"},
        "protocol":{"major":1,"minMinor":0,"maxMinor":0},
        "backends":[{"target":"aarch64-apple-darwin","path":"backend/teamarness-reference-package-aarch64-apple-darwin","size":backend.len(),"sha256":digest(&backend)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],"contract":{"path":"ui/contract.json","sha256":digest(&contract)}},
        "capabilities":[
            {"identity":"teamarness.workspace-context","major":1,"minMinor":0,"maxMinor":0},
            {"identity":"teamarness.membership","major":1,"minMinor":0,"maxMinor":0},
            {"identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0}
        ]
    })).unwrap()).unwrap();
}

async fn open_mount(
    host: &PackageHost,
    key: &PackageInstanceKey,
) -> (
    teamarness_package_host::UiMountId,
    teamarness_package_host::UiLoaderDescriptor,
) {
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(
            key.clone(),
        )))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader),
        outcome => panic!("unexpected mount outcome: {outcome:?}"),
    }
}

async fn request(host: &PackageHost, key: &PackageInstanceKey, operation: &str) -> PackageValue {
    let (mount, loader) = open_mount(host, key).await;
    assert_eq!(loader.package.lifecycle, LifecycleState::Active);
    assert_eq!(loader.package.identity, key.package_identity);
    assert!(!loader.package.version.is_empty());
    assert_eq!(
        loader.package.process_generation,
        instance_generation(host, key).await
    );
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("request").unwrap(),
            operation: operation.into(),
            payload: PackageValue::Object(Default::default()),
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

async fn read_reference(host: &PackageHost, key: &PackageInstanceKey) -> PackageValue {
    request(host, key, "reference.read").await
}

async fn instance_generation(host: &PackageHost, key: &PackageInstanceKey) -> u64 {
    host.snapshot()
        .await
        .instances
        .into_iter()
        .find(|instance| instance.key == *key)
        .unwrap()
        .process_generation
        .unwrap()
}

async fn instance_lifecycle(host: &PackageHost, key: &PackageInstanceKey) -> LifecycleState {
    host.snapshot()
        .await
        .instances
        .into_iter()
        .find(|instance| instance.key == *key)
        .unwrap()
        .lifecycle
}

async fn source_for(host: &PackageHost) -> &'static str {
    let package = host
        .snapshot()
        .await
        .packages
        .into_iter()
        .find(|package| package.identity.as_str() == "dev.teamarness.reference")
        .unwrap();
    match package.source {
        teamarness_package_host::PackageSourceKind::Bundled => "bundled",
        teamarness_package_host::PackageSourceKind::Local => "local",
    }
}

async fn shutdown(host: PackageHost) {
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}

fn instance_key(identity: &str, workspace: &WorkspaceId) -> PackageInstanceKey {
    PackageInstanceKey::new(
        PackageIdentity::from_str(identity).unwrap(),
        workspace.clone(),
    )
}

fn integer(value: &PackageValue, name: &str) -> i64 {
    let PackageValue::Object(fields) = value else {
        panic!("response is not an object")
    };
    match fields.get(name).unwrap() {
        PackageValue::Integer(value) => *value,
        PackageValue::Unsigned(value) => i64::try_from(*value).unwrap(),
        value => panic!("{name} is not an integer: {value:?}"),
    }
}

fn live_lease_count(private: &Path) -> i64 {
    Connection::open(private.join("packages/package-host.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM process_leases", [], |row| row.get(0))
        .unwrap()
}

fn persisted_marker(private: &Path, key: &PackageInstanceKey) -> i64 {
    let host = Connection::open(private.join("packages/package-host.sqlite3")).unwrap();
    let relative: String = host
        .query_row(
            "SELECT relative_path FROM package_data_directories WHERE workspace_id = ?1 AND package_identity = ?2",
            [key.workspace_id.as_str(), key.package_identity.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    Connection::open(private.join(relative).join("state.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT background_marker FROM reference_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn write_registration(private: &Path, local: &Path) {
    let path = private.join("packages/registrations.v1.json");
    fs::write(&path, serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,
        "registrations":[{
            "directory":local,
            "expectedIdentity":"dev.teamarness.reference",
            "trust":{"kind":"trusted_native_code_v1","acceptedRisks":["filesystem","network","process"]},
            "overrideApplication":true
        }]
    })).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
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

fn assert_declared_bytes_equal(left: &Path, right: &Path) {
    let left_manifest = fs::read(left.join("teamarness.package.json")).unwrap();
    assert_eq!(
        left_manifest,
        fs::read(right.join("teamarness.package.json")).unwrap()
    );
    let manifest: serde_json::Value = serde_json::from_slice(&left_manifest).unwrap();
    let mut paths = vec![
        manifest["backends"][0]["path"].as_str().unwrap(),
        manifest["ui"]["entrypoint"]["path"].as_str().unwrap(),
        manifest["ui"]["contract"]["path"].as_str().unwrap(),
    ];
    paths.extend(
        manifest["ui"]["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap()),
    );
    for path in paths {
        assert_eq!(
            fs::read(left.join(path)).unwrap(),
            fs::read(right.join(path)).unwrap()
        );
    }
}

fn rewrite_version(root: &Path, version: &str) {
    let path = root.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["version"] = serde_json::Value::String(version.into());
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The Reference Package's declared failure is the live fixture for Package error fidelity: its
/// code, retryability and bounded details reach the Host beside the host's own code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reference_fail_reaches_the_host_as_a_package_owned_error() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("application");
    let private = root.path().join("private");
    fs::create_dir_all(&application).unwrap();
    fs::create_dir_all(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let backend = reference_backend(root.path());
    write_package(
        &application.join("reference"),
        "dev.teamarness.reference",
        "1.0.0",
        &backend,
    );
    let index = application.join("application-packages.v1.json");
    fs::write(
        &index,
        br#"{"schemaVersion":1,"packages":[{"directory":"reference"}]}"#,
    )
    .unwrap();

    let core = Arc::new(
        CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(root.path().join("core")))
            .await
            .unwrap(),
    );
    core.create_workspace("Workspace One").unwrap();
    let workspace = WorkspaceId::new(core.list_workspaces().unwrap()[0].id().to_string()).unwrap();
    let installation = InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap();
    let host = PackageHost::open(
        PackageHostConfig::new(&application, &index, &private, installation),
        core.clone(),
    )
    .await
    .unwrap();

    let key = instance_key("dev.teamarness.reference", &workspace);
    let (mount, _) = open_mount(&host, &key).await;
    let error = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new("failing").unwrap(),
            operation: "reference.fail".into(),
            payload: PackageValue::Object(Default::default()),
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        teamarness_package_host::PackageErrorCode::BackendError
    );
    assert!(!error.retryable);
    let package_error = error.package_error.as_ref().expect("Package-owned error");
    assert_eq!(package_error.code, "reference.requested_failure");
    assert!(!package_error.retryable);
    assert_eq!(
        package_error.details["reason"],
        serde_json::json!("requested")
    );

    host.execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
        .await
        .unwrap();
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}
