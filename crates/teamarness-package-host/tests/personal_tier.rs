//! The live Personal tier: a running Host installs, reloads, and removes a
//! Personal Package without disturbing any Team Package (ADR 0036).

#![cfg(unix)]
// Host commands return the full, payload-free error envelope.
#![allow(clippy::result_large_err)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use teamarness_package_host::{
    ContentViewToken, InstallPersonal, LifecycleState, MountState, OpenUiMount, PackageErrorCode,
    PackageErrorEnvelope, PackageHost, PackageHostCommand, PackageHostOutcome, PackageIdentity,
    PackageInstanceKey, PackageInstanceSnapshot, PackageSourceKind, PackageTier,
    PendingRegistrationChange, PortStage, ReadContent, ReconcileWorkspaces, RegistrationStatus,
    ReloadPersonal, RemoveOverride, RemovePersonal, RestartPersonal, ShutdownHost, SuspendPackage,
    UiMountId, UiRequest, UiRequestId, WriteOverride,
};

const TEAM: &str = "com.example.team";
const CONTRACT: &str = "com.example.messages";
const RISKS: [&str; 3] = ["filesystem", "network", "process"];

/// A running Host with one eager Team Package in two Workspaces, and one open
/// Team mount.
struct Live {
    fixture: common::Fixture,
    host: PackageHost,
    team_mount: UiMountId,
    team_before: Vec<PackageInstanceSnapshot>,
}

fn capabilities() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"identity":"teamarness.workspace-context","major":1,"minMinor":0,"maxMinor":0}),
        serde_json::json!({"identity":"teamarness.package-ports","major":2,"minMinor":0,"maxMinor":0}),
    ]
}

impl Live {
    async fn open() -> Self {
        Self::open_with(|_| {}).await
    }

    /// Opens the Host after `team` edits the Team Package manifest.
    async fn open_with(team: impl FnOnce(&mut serde_json::Value)) -> Self {
        let fixture =
            common::Fixture::node_with_capabilities(&[("team", TEAM)], &capabilities()).await;
        let manifest_path = fixture
            .config
            .application_root()
            .join("team/teamarness.package.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        team(&mut manifest);
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        fixture.core.create_workspace("Second Workspace").unwrap();
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let team = instances(&host, TEAM).await;
        assert_eq!(team.len(), 2, "the Team Package runs in both Workspaces");
        let team_mount = mount(&host, team[0].key.clone()).await;
        let team_before = instances(&host, TEAM).await;
        assert_eq!(team_before[0].mount_state, Some(MountState::Mounted));
        Self {
            fixture,
            host,
            team_mount,
            team_before,
        }
    }

    /// Writes a Personal Package Directory outside the application.
    fn package(&self, name: &str, identity: &str, ports: Ports) -> PathBuf {
        let path = self.fixture._root.path().join("sources").join(name);
        common::write_node_package_with_capabilities(&path, identity, &capabilities());
        let manifest_path = path.join("teamarness.package.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        match ports {
            Ports::None => {}
            Ports::Provides => {
                let bytes = serde_json::to_vec(&serde_json::json!({
                    "schemaVersion":1,"$defs":{},"contracts":[{
                        "identity":CONTRACT,"major":1,"minor":0,"description":"Fixture messages",
                        "ports":[{"name":"message.post","kind":"command","description":"Post a message",
                            "request":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},
                            "response":{"type":"object","properties":{"accepted":{"type":"boolean"}},"required":["accepted"],"additionalProperties":false}}]
                    }]
                }))
                .unwrap();
                fs::write(path.join("ports.json"), &bytes).unwrap();
                manifest["portContract"] =
                    serde_json::json!({"path":"ports.json","sha256":common::digest(&bytes)});
            }
            Ports::Requires => {
                manifest["consumedPorts"] = serde_json::json!([{
                    "identity":CONTRACT,"major":1,"minMinor":0,"maxMinor":0,"required":true,
                    "ports":[{"name":"message.post","kind":"command"}]
                }]);
            }
        }
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        path
    }

    async fn install(
        &self,
        path: &Path,
        identity: &str,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        self.host
            .execute(PackageHostCommand::InstallPersonal(InstallPersonal {
                directory: path.to_owned(),
                expected_identity: identity.parse().unwrap(),
                accepted_risks: RISKS.map(str::to_owned).to_vec(),
            }))
            .await
    }

    async fn reload(&self, identity: &str) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        self.host
            .execute(PackageHostCommand::ReloadPersonal(ReloadPersonal {
                identity: identity.parse().unwrap(),
            }))
            .await
    }

    async fn remove(&self, identity: &str) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        self.host
            .execute(PackageHostCommand::RemovePersonal(RemovePersonal {
                identity: identity.parse().unwrap(),
            }))
            .await
    }

    /// The Team Instances still have the process generations and mount they
    /// had before, and the open Team mount still serves requests.
    async fn assert_team_undisturbed(&self) {
        assert_eq!(
            instances(&self.host, TEAM).await,
            self.team_before,
            "no Team Instance changed"
        );
        let answer = self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.team_mount.clone(),
                request: UiRequestId::new(format!("team-{}", rand_suffix())).unwrap(),
                operation: "fixture.immediate".into(),
                payload: serde_json::from_value(serde_json::json!({})).unwrap(),
            }))
            .await;
        assert!(
            matches!(answer, Ok(PackageHostOutcome::UiResponse { .. })),
            "the Team mount is still live: {answer:?}"
        );
    }

    /// The Package Data Directory of one Instance.
    fn data_directory(&self, key: &PackageInstanceKey) -> PathBuf {
        self.fixture
            ._private
            .join("packages/data/v1")
            .join(self.fixture.config.installation_public_identity().as_str())
            .join(key.workspace_id.as_str())
            .join(common::digest(key.package_identity.as_str().as_bytes()))
    }

    fn registration_file(&self) -> Option<Vec<u8>> {
        fs::read(self.fixture._private.join("packages/registrations.v1.json")).ok()
    }

    async fn shutdown(self) {
        self.host
            .execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
    }
}

#[derive(Clone, Copy)]
enum Ports {
    None,
    Provides,
    Requires,
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed).to_string()
}

async fn instances(host: &PackageHost, identity: &str) -> Vec<PackageInstanceSnapshot> {
    let mut instances: Vec<_> = host
        .snapshot()
        .await
        .instances
        .into_iter()
        .filter(|instance| instance.key.package_identity.as_str() == identity)
        .collect();
    instances.sort_by(|left, right| left.key.cmp(&right.key));
    instances
}

fn generations(instances: &[PackageInstanceSnapshot]) -> Vec<Option<u64>> {
    instances
        .iter()
        .map(|instance| instance.process_generation)
        .collect()
}

async fn mount(host: &PackageHost, key: PackageInstanceKey) -> UiMountId {
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(key)))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        other => panic!("{other:?}"),
    }
}

/// Posts one message through the Ports capability from a consumer mount.
async fn post(
    host: &PackageHost,
    mount: &UiMountId,
    text: &str,
) -> Result<serde_json::Value, PackageErrorEnvelope> {
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!("post-{}", rand_suffix())).unwrap(),
            operation: "fixture.capability".into(),
            payload: serde_json::from_value(serde_json::json!({
                "capability":"teamarness.package-ports","method":"invoke",
                "arguments":{"access":"declared","major":1,"contract":CONTRACT,"port":"message.post","kind":"command","request":{"text":text}}
            }))
            .unwrap(),
        }))
        .await?;
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        panic!("{outcome:?}")
    };
    Ok(serde_json::to_value(payload).unwrap())
}

/// Calls one Ports capability method from a mount and returns the UI response.
async fn capability(
    host: &PackageHost,
    mount: &UiMountId,
    method: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!("ports-{}", rand_suffix())).unwrap(),
            operation: "fixture.capability".into(),
            payload: serde_json::from_value(serde_json::json!({
                "capability":"teamarness.package-ports","method":method,"arguments":arguments
            }))
            .unwrap(),
        }))
        .await
        .unwrap();
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        panic!("{outcome:?}")
    };
    serde_json::to_value(payload).unwrap()
}

/// Sends one fixture request to a mount and returns its payload.
async fn fixture_request(
    host: &PackageHost,
    mount: &UiMountId,
    operation: &str,
) -> serde_json::Value {
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!("fixture-{}", rand_suffix())).unwrap(),
            operation: operation.into(),
            payload: serde_json::from_value(serde_json::json!({})).unwrap(),
        }))
        .await
        .unwrap();
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        panic!("{outcome:?}")
    };
    serde_json::to_value(payload).unwrap()
}

fn assert_active(instances: &[PackageInstanceSnapshot], workspaces: usize) {
    assert_eq!(instances.len(), workspaces, "one Instance per Workspace");
    for instance in instances {
        assert_eq!(instance.lifecycle, LifecycleState::Active, "{instance:?}");
    }
}

async fn registration_status(host: &PackageHost, identity: &str) -> Option<RegistrationStatus> {
    host.snapshot()
        .await
        .registrations
        .into_iter()
        .find(|registration| registration.expected_identity == identity)
        .map(|registration| {
            assert_eq!(registration.tier, PackageTier::Personal);
            registration.status
        })
}

/// Replaces the Backend bytes and, when `sign`, the manifest's digest and version.
fn rebuild(path: &Path, version: &str, sign: bool) {
    let backend_path = path.join("backend/package.mjs");
    let mut backend = fs::read(&backend_path).unwrap();
    backend.extend_from_slice(format!("\n// rebuilt as {version}\n").as_bytes());
    fs::write(&backend_path, &backend).unwrap();
    if sign {
        let manifest_path = path.join("teamarness.package.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["version"] = version.into();
        manifest["backends"][0]["size"] = backend.len().into();
        manifest["backends"][0]["sha256"] = common::digest(&backend).into();
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
}

/// The number of running processes whose command line names this Backend.
fn processes_running(path: &Path) -> usize {
    let backend = fs::canonicalize(path.join("backend/package.mjs")).unwrap();
    let listing = std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|line| line.contains(backend.to_string_lossy().as_ref()))
        .count()
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn install_runs_a_personal_package_in_every_workspace_without_touching_team() {
    let live = Live::open().await;
    let path = live.package("personal", "com.example.personal", Ports::None);

    let outcome = live.install(&path, "com.example.personal").await.unwrap();

    assert!(matches!(
        outcome,
        PackageHostOutcome::PersonalInstalled { .. }
    ));
    let personal = instances(&live.host, "com.example.personal").await;
    assert_active(&personal, 2);
    assert_eq!(generations(&personal), vec![Some(1), Some(1)]);
    let snapshot = live.host.snapshot().await;
    assert!(
        snapshot
            .packages
            .iter()
            .any(|package| package.identity.as_str() == "com.example.personal")
    );
    assert_eq!(
        registration_status(&live.host, "com.example.personal").await,
        Some(RegistrationStatus::Admitted)
    );
    let catalog = live.host.authoring_catalog();
    let entry = catalog
        .iter()
        .find(|entry| entry.package.identity.as_str() == "com.example.personal")
        .expect("the authoring catalog lists the Personal Package");
    assert!(
        entry
            .capabilities
            .iter()
            .any(|capability| capability.identity == "teamarness.workspace-context"),
        "the catalog carries the declared Semantic Capabilities"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn restart_replaces_one_personal_process_and_refuses_a_team_instance() {
    let live = Live::open().await;
    let path = live.package("personal", "com.example.personal", Ports::None);
    live.install(&path, "com.example.personal").await.unwrap();
    let before = instances(&live.host, "com.example.personal").await;
    assert_eq!(generations(&before), vec![Some(1), Some(1)]);
    let target = before[0].key.clone();
    let revision = live.host.snapshot().await.revision;

    let outcome = live
        .host
        .execute(PackageHostCommand::RestartPersonal(RestartPersonal {
            instance: target.clone(),
        }))
        .await
        .unwrap();

    let PackageHostOutcome::PersonalRestarted {
        revision: restarted,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(restarted > revision, "the restart bumps the revision");
    let after = instances(&live.host, "com.example.personal").await;
    assert_active(&after, 2);
    assert_eq!(
        generations(&after),
        vec![Some(2), Some(1)],
        "only the restarted Instance has a new process generation"
    );
    assert_eq!(after[0].key, target);

    let team = live.team_before[0].key.clone();
    let refused = live
        .host
        .execute(PackageHostCommand::RestartPersonal(RestartPersonal {
            instance: team,
        }))
        .await
        .unwrap_err();
    assert_eq!(refused.code, PackageErrorCode::OverrideRequired);
    live.assert_team_undisturbed().await;

    live.host
        .execute(PackageHostCommand::Suspend(SuspendPackage {
            instance: target.clone(),
        }))
        .await
        .unwrap();
    // The Instance actor decides under its lifecycle gate, so a refusal is
    // never reported as a restart and publishes no new revision.
    let paused_revision = live.host.snapshot().await.revision;
    let paused = live
        .host
        .execute(PackageHostCommand::RestartPersonal(RestartPersonal {
            instance: target.clone(),
        }))
        .await
        .unwrap_err();
    assert_eq!(
        paused.code,
        PackageErrorCode::InvalidLifecycleState,
        "a paused Instance is not restarted silently"
    );
    assert_eq!(paused.affected_instance.as_ref(), Some(&target));
    assert_eq!(live.host.snapshot().await.revision, paused_revision);
    assert_eq!(
        generations(&instances(&live.host, "com.example.personal").await),
        vec![Some(2), Some(1)]
    );
    assert_eq!(
        instances(&live.host, "com.example.personal").await[0].lifecycle,
        LifecycleState::Suspended
    );
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn reload_runs_rebuilt_bytes_at_a_higher_generation_and_keeps_data() {
    let live = Live::open().await;
    let path = live.package("personal", "com.example.personal", Ports::None);
    live.install(&path, "com.example.personal").await.unwrap();
    let before = instances(&live.host, "com.example.personal").await;
    let marker = live.data_directory(&before[0].key).join("marker");
    fs::write(&marker, b"kept").unwrap();
    let personal_mount = mount(&live.host, before[0].key.clone()).await;

    rebuild(&path, "1.0.1", true);
    let outcome = live.reload("com.example.personal").await.unwrap();

    assert!(matches!(
        outcome,
        PackageHostOutcome::PersonalReloaded { .. }
    ));
    let after = instances(&live.host, "com.example.personal").await;
    assert_active(&after, 2);
    for (old, new) in before.iter().zip(&after) {
        assert!(
            new.process_generation > old.process_generation,
            "the generation continues: {old:?} then {new:?}"
        );
    }
    let package = live
        .host
        .snapshot()
        .await
        .packages
        .into_iter()
        .find(|package| package.identity.as_str() == "com.example.personal")
        .unwrap();
    assert_eq!(package.version, "1.0.1", "the rebuilt manifest is admitted");
    assert_eq!(fs::read(&marker).unwrap(), b"kept");
    let stale = live
        .host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: personal_mount,
            request: UiRequestId::new("stale").unwrap(),
            operation: "fixture.immediate".into(),
            payload: serde_json::from_value(serde_json::json!({})).unwrap(),
        }))
        .await;
    assert_eq!(
        stale.unwrap_err().code,
        PackageErrorCode::StaleMount,
        "reload revoked the old mount"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn a_broken_rebuild_is_refused_and_a_fixed_rebuild_reloads() {
    let live = Live::open().await;
    let path = live.package("personal", "com.example.personal", Ports::None);
    live.install(&path, "com.example.personal").await.unwrap();
    let before = instances(&live.host, "com.example.personal").await;

    rebuild(&path, "1.0.1", false);
    let refused = live.reload("com.example.personal").await.unwrap_err();

    assert_eq!(
        registration_status(&live.host, "com.example.personal").await,
        Some(RegistrationStatus::Refused { code: refused.code }),
        "the registration stays and reports the refusal"
    );
    assert!(
        instances(&live.host, "com.example.personal")
            .await
            .is_empty()
    );
    assert!(
        live.host.snapshot().await.diagnostics.iter().any(|error| {
            error.code == refused.code
                && error.affected_package.as_ref().map(PackageIdentity::as_str)
                    == Some("com.example.personal")
        }),
        "the refusal is diagnosed"
    );
    live.assert_team_undisturbed().await;

    rebuild(&path, "1.0.2", true);
    live.reload("com.example.personal").await.unwrap();

    assert_eq!(
        registration_status(&live.host, "com.example.personal").await,
        Some(RegistrationStatus::Admitted)
    );
    let after = instances(&live.host, "com.example.personal").await;
    assert_active(&after, 2);
    assert!(after[0].process_generation > before[0].process_generation);
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn remove_keeps_data_and_a_reinstall_resumes_it() {
    let live = Live::open().await;
    let path = live.package("personal", "com.example.personal", Ports::None);
    live.install(&path, "com.example.personal").await.unwrap();
    let before = instances(&live.host, "com.example.personal").await;
    let marker = live.data_directory(&before[0].key).join("marker");
    fs::write(&marker, b"kept").unwrap();

    let outcome = live.remove("com.example.personal").await.unwrap();

    assert!(matches!(
        outcome,
        PackageHostOutcome::PersonalRemoved { .. }
    ));
    assert!(
        instances(&live.host, "com.example.personal")
            .await
            .is_empty()
    );
    let snapshot = live.host.snapshot().await;
    assert!(
        !snapshot
            .packages
            .iter()
            .any(|package| package.identity.as_str() == "com.example.personal")
    );
    assert_eq!(
        registration_status(&live.host, "com.example.personal").await,
        None
    );
    let file = String::from_utf8(live.registration_file().unwrap()).unwrap();
    assert!(!file.contains("com.example.personal"), "{file}");
    assert_eq!(fs::read(&marker).unwrap(), b"kept", "remove keeps the data");
    live.assert_team_undisturbed().await;

    live.install(&path, "com.example.personal").await.unwrap();

    let after = instances(&live.host, "com.example.personal").await;
    assert_active(&after, 2);
    assert_eq!(
        live.data_directory(&after[0].key).join("marker"),
        marker,
        "the reinstalled Instance has the same data directory"
    );
    assert_eq!(fs::read(&marker).unwrap(), b"kept");
    assert!(
        after[0].process_generation > before[0].process_generation,
        "generations stay monotonic across remove and reinstall"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn a_personal_consumer_follows_its_personal_provider() {
    let live = Live::open().await;
    let provider = live.package("provider", "com.example.provider", Ports::Provides);
    let consumer = live.package("consumer", "com.example.consumer", Ports::Requires);
    let bystander = live.package("bystander", "com.example.bystander", Ports::None);
    live.install(&provider, "com.example.provider")
        .await
        .unwrap();
    live.install(&consumer, "com.example.consumer")
        .await
        .unwrap();
    live.install(&bystander, "com.example.bystander")
        .await
        .unwrap();
    let consumers = instances(&live.host, "com.example.consumer").await;
    assert_active(&consumers, 2);
    let routed = mount(&live.host, consumers[0].key.clone()).await;
    let posted = post(&live.host, &routed, "hello").await.unwrap();
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    let bystander_before = instances(&live.host, "com.example.bystander").await;
    let consumer_before = generations(&instances(&live.host, "com.example.consumer").await);

    live.remove("com.example.provider").await.unwrap();

    let consumers = instances(&live.host, "com.example.consumer").await;
    assert_active(&consumers, 2);
    let consumer_removed = generations(&consumers);
    for (old, new) in consumer_before.iter().zip(&consumer_removed) {
        assert!(new > old, "the consumer restarted over its new routes");
    }
    let route = |host: &PackageHost| {
        host.authoring_catalog()
            .into_iter()
            .find(|entry| entry.package.identity.as_str() == "com.example.consumer")
            .unwrap()
            .availability
    };
    assert_eq!(
        route(&live.host)[0].unavailable,
        Some(PackageErrorCode::PortContractMissing)
    );
    let consumer_diagnosed = |host_diagnostics: &[PackageErrorEnvelope]| {
        host_diagnostics.iter().any(|error| {
            error.code == PackageErrorCode::PortContractMissing
                && error.affected_package.as_ref().map(PackageIdentity::as_str)
                    == Some("com.example.consumer")
        })
    };
    assert!(consumer_diagnosed(&live.host.snapshot().await.diagnostics));
    let unrouted = mount(&live.host, consumers[0].key.clone()).await;
    let refused = post(&live.host, &unrouted, "hello").await.unwrap();
    assert_eq!(refused["error"], "port_unavailable", "{refused}");
    assert_eq!(
        instances(&live.host, "com.example.bystander").await,
        bystander_before
    );
    live.assert_team_undisturbed().await;

    live.install(&provider, "com.example.provider")
        .await
        .unwrap();

    let consumers = instances(&live.host, "com.example.consumer").await;
    assert_active(&consumers, 2);
    for (old, new) in consumer_removed.iter().zip(generations(&consumers)) {
        assert!(
            new > *old,
            "the consumer restarted over its restored routes"
        );
    }
    assert_eq!(route(&live.host)[0].unavailable, None);
    assert!(!consumer_diagnosed(&live.host.snapshot().await.diagnostics));
    let rerouted = mount(&live.host, consumers[0].key.clone()).await;
    let posted = post(&live.host, &rerouted, "again").await.unwrap();
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    assert_eq!(
        instances(&live.host, "com.example.bystander").await,
        bystander_before,
        "only the consumer whose route changed restarted"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn a_second_live_provider_for_a_provided_contract_is_refused() {
    let live = Live::open().await;
    let first = live.package("first", "com.example.first", Ports::Provides);
    let second = live.package("second", "com.example.second", Ports::Provides);
    live.install(&first, "com.example.first").await.unwrap();
    let file = live.registration_file();
    let revision = live.host.snapshot().await.revision;

    let refused = live
        .install(&second, "com.example.second")
        .await
        .unwrap_err();

    assert_eq!(refused.code, PackageErrorCode::PortProviderConflict);
    assert_eq!(live.registration_file(), file, "the file is unchanged");
    assert!(instances(&live.host, "com.example.second").await.is_empty());
    assert_eq!(
        registration_status(&live.host, "com.example.second").await,
        None
    );
    assert_eq!(live.host.snapshot().await.revision, revision);
    assert_active(&instances(&live.host, "com.example.first").await, 2);
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn an_install_racing_workspace_reconciliation_creates_one_instance_per_workspace() {
    let live = Live::open().await;
    let path = live.package("racer", "com.example.racer", Ports::None);
    live.fixture
        .core
        .create_workspace("Third Workspace")
        .unwrap();

    let (installed, reconciled) = tokio::join!(
        live.install(&path, "com.example.racer"),
        live.host.execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {}
        ))
    );

    installed.unwrap();
    reconciled.unwrap();
    assert_active(&instances(&live.host, "com.example.racer").await, 3);
    assert_active(&instances(&live.host, TEAM).await, 3);
    assert_eq!(
        processes_running(&path),
        3,
        "exactly one Backend process per Workspace"
    );
    live.host
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .unwrap();
    assert_eq!(processes_running(&path), 3);
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R14
#[tokio::test]
async fn a_port_call_in_flight_when_its_provider_is_removed_ends_uncertain() {
    let live = Live::open().await;
    let provider = live.package("provider", "com.example.provider", Ports::Provides);
    let consumer = live.package("consumer", "com.example.consumer", Ports::Requires);
    live.install(&provider, "com.example.provider")
        .await
        .unwrap();
    live.install(&consumer, "com.example.consumer")
        .await
        .unwrap();
    let key = instances(&live.host, "com.example.consumer").await[0]
        .key
        .clone();
    let routed = mount(&live.host, key).await;
    let mut observations = live.host.observe_ports();

    let host = live.host.clone();
    let stalled = tokio::spawn(async move { post(&host, &routed, "stall").await });
    let dispatched = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let observation = observations.recv().await.unwrap();
            if observation.stage == PortStage::Dispatched {
                return observation;
            }
        }
    })
    .await
    .expect("the stalled command reaches its Provider");
    live.remove("com.example.provider").await.unwrap();

    let terminal = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let observation = observations.recv().await.unwrap();
            if observation.consumer_id == dispatched.consumer_id
                && matches!(observation.stage, PortStage::Completed | PortStage::Failed)
            {
                return observation;
            }
        }
    })
    .await
    .expect("the dispatched command ends");
    assert_eq!(terminal.stage, PortStage::Failed);
    assert!(
        terminal.outcome_uncertain,
        "a command that reached its Provider ends uncertain: {terminal:?}"
    );
    match stalled.await.expect("the request task does not panic") {
        // The consumer answered before its own restart over the new routes.
        Ok(answer) => {
            assert_eq!(answer["error"], "outcome_unknown", "{answer}");
            assert_eq!(answer["outcomeUncertain"], true);
            assert_eq!(answer["retryable"], false);
        }
        // The consumer restarted first, which ended its own UI request.
        Err(error) => assert!(
            matches!(
                error.code,
                PackageErrorCode::ProcessExited
                    | PackageErrorCode::OutcomeUncertain
                    | PackageErrorCode::StaleMount
                    | PackageErrorCode::Cancelled
            ),
            "{error:?}"
        ),
    }
    assert_active(&instances(&live.host, "com.example.consumer").await, 2);
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn an_override_is_written_to_the_file_only_and_applies_after_reopen() {
    let live = Live::open().await;
    let build = live.package("team-build", TEAM, Ports::None);
    let revision = live.host.snapshot().await.revision;
    let write = |risks: Vec<String>, identity: &str, directory: &Path| {
        PackageHostCommand::WriteOverride(WriteOverride {
            directory: directory.to_owned(),
            expected_identity: identity.parse().unwrap(),
            accepted_risks: risks,
        })
    };

    // Refusals leave the file absent.
    let incomplete = live
        .host
        .execute(write(vec!["filesystem".into()], TEAM, &build))
        .await
        .unwrap_err();
    assert_eq!(incomplete.code, PackageErrorCode::TrustEvidenceMissing);
    let personal = live.package("personal", "com.example.personal", Ports::None);
    let not_team = live
        .host
        .execute(write(
            RISKS.map(str::to_owned).to_vec(),
            "com.example.personal",
            &personal,
        ))
        .await
        .unwrap_err();
    assert_eq!(not_team.code, PackageErrorCode::InvalidRegistration);
    assert_eq!(live.registration_file(), None);

    let outcome = live
        .host
        .execute(write(RISKS.map(str::to_owned).to_vec(), TEAM, &build))
        .await
        .unwrap();
    assert_eq!(outcome, PackageHostOutcome::OverrideWritten);
    let file: serde_json::Value =
        serde_json::from_slice(&live.registration_file().unwrap()).unwrap();
    assert_eq!(file["registrations"][0]["expectedIdentity"], TEAM);
    assert_eq!(file["registrations"][0]["overrideApplication"], true);
    // No Instance changes. The snapshot shows the override as pending a restart.
    let snapshot = live.host.snapshot().await;
    assert!(
        snapshot.revision > revision,
        "the pending override is published"
    );
    let pending = snapshot
        .registrations
        .iter()
        .find(|registration| registration.expected_identity == TEAM)
        .unwrap();
    assert!(pending.override_application);
    assert_eq!(pending.tier, PackageTier::Team);
    assert_eq!(
        pending.status,
        RegistrationStatus::PendingRestart {
            change: PendingRegistrationChange::Written
        }
    );
    let source = |snapshot: &teamarness_package_host::PackageHostSnapshot| {
        snapshot
            .packages
            .iter()
            .find(|package| package.identity.as_str() == TEAM)
            .map(|package| package.source)
    };
    assert_eq!(source(&snapshot), Some(PackageSourceKind::Bundled));
    live.assert_team_undisturbed().await;

    // A live Personal operation cannot change the override.
    let reload = live.reload(TEAM).await.unwrap_err();
    assert_eq!(reload.code, PackageErrorCode::OverrideRequired);

    let config = live.fixture.config.clone();
    let core = live.fixture.core.clone();
    live.host
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    let reopened = PackageHost::open(config.clone(), core.clone())
        .await
        .unwrap();
    let snapshot = reopened.snapshot().await;
    assert_eq!(source(&snapshot), Some(PackageSourceKind::Local));
    let registration = snapshot
        .registrations
        .iter()
        .find(|registration| registration.expected_identity == TEAM)
        .unwrap();
    assert_eq!(registration.tier, PackageTier::Team);
    assert_eq!(registration.status, RegistrationStatus::Admitted);

    // Removing the override is file-only too, and needs an override entry.
    let missing = reopened
        .execute(PackageHostCommand::RemoveOverride(RemoveOverride {
            identity: "com.example.personal".parse().unwrap(),
        }))
        .await
        .unwrap_err();
    assert_eq!(missing.code, PackageErrorCode::NotRegistered);
    let before_remove = reopened.snapshot().await.revision;
    let remove_team = || {
        PackageHostCommand::RemoveOverride(RemoveOverride {
            identity: TEAM.parse().unwrap(),
        })
    };
    let removed = reopened.execute(remove_team()).await.unwrap();
    assert_eq!(removed, PackageHostOutcome::OverrideRemoved);
    let snapshot = reopened.snapshot().await;
    assert_eq!(source(&snapshot), Some(PackageSourceKind::Local));
    assert!(
        snapshot.revision > before_remove,
        "the removal is published"
    );
    let pending = snapshot
        .registrations
        .iter()
        .find(|registration| registration.expected_identity == TEAM)
        .unwrap();
    assert_eq!(
        pending.status,
        RegistrationStatus::PendingRestart {
            change: PendingRegistrationChange::Removed
        },
        "the running override stays listed until the restart"
    );
    // Nothing is left to remove until the restart applies the change.
    let again = reopened.execute(remove_team()).await.unwrap_err();
    assert_eq!(again.code, PackageErrorCode::NotRegistered);
    reopened
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    let restored = PackageHost::open(config, core).await.unwrap();
    assert_eq!(
        source(&restored.snapshot().await),
        Some(PackageSourceKind::Bundled)
    );
    restored
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    drop(live.fixture);
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn an_override_removed_before_the_restart_leaves_nothing_pending() {
    let live = Live::open().await;
    let build = live.package("team-build", TEAM, Ports::None);
    live.host
        .execute(PackageHostCommand::WriteOverride(WriteOverride {
            directory: build,
            expected_identity: TEAM.parse().unwrap(),
            accepted_risks: RISKS.map(str::to_owned).to_vec(),
        }))
        .await
        .unwrap();
    let written = live.host.snapshot().await.revision;
    let removed = live
        .host
        .execute(PackageHostCommand::RemoveOverride(RemoveOverride {
            identity: TEAM.parse().unwrap(),
        }))
        .await
        .unwrap();
    assert_eq!(removed, PackageHostOutcome::OverrideRemoved);
    let snapshot = live.host.snapshot().await;
    assert!(snapshot.revision > written);
    assert!(
        snapshot.registrations.is_empty(),
        "the bundled Package already runs, so nothing waits for a restart"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test]
async fn a_hand_written_entry_for_a_team_identity_is_removed_and_the_bundled_package_returns() {
    let live = Live::open().await;
    let build = live.package("team-build", TEAM, Ports::None);
    live.host
        .execute(PackageHostCommand::WriteOverride(WriteOverride {
            directory: build,
            expected_identity: TEAM.parse().unwrap(),
            accepted_risks: RISKS.map(str::to_owned).to_vec(),
        }))
        .await
        .unwrap();
    // A person edits the file by hand and drops `overrideApplication`.
    let path = live.fixture._private.join("packages/registrations.v1.json");
    let mut file: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    file["registrations"][0]["overrideApplication"] = serde_json::json!(false);
    fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let config = live.fixture.config.clone();
    let core = live.fixture.core.clone();
    live.host
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    let host = PackageHost::open(config.clone(), core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert!(
        !snapshot
            .packages
            .iter()
            .any(|package| package.identity.as_str() == TEAM),
        "the entry makes the Team identity unavailable"
    );
    let registration = &snapshot.registrations[0];
    assert_eq!(registration.expected_identity, TEAM);
    assert_eq!(
        registration.status,
        RegistrationStatus::Refused {
            code: PackageErrorCode::OverrideRequired
        }
    );

    // Reload and a live Remove are refused clearly and change nothing.
    let revision = snapshot.revision;
    for command in [
        PackageHostCommand::ReloadPersonal(ReloadPersonal {
            identity: TEAM.parse().unwrap(),
        }),
        PackageHostCommand::RemovePersonal(RemovePersonal {
            identity: TEAM.parse().unwrap(),
        }),
    ] {
        let refused = host.execute(command).await.unwrap_err();
        assert_eq!(refused.code, PackageErrorCode::OverrideRequired);
    }
    assert_eq!(host.snapshot().await.revision, revision);

    let removed = host
        .execute(PackageHostCommand::RemoveOverride(RemoveOverride {
            identity: TEAM.parse().unwrap(),
        }))
        .await
        .unwrap();
    assert_eq!(removed, PackageHostOutcome::OverrideRemoved);
    let snapshot = host.snapshot().await;
    assert!(snapshot.revision > revision);
    assert_eq!(
        snapshot.registrations[0].status,
        RegistrationStatus::PendingRestart {
            change: PendingRegistrationChange::Removed
        }
    );
    let file: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(file["registrations"], serde_json::json!([]));

    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    let restored = PackageHost::open(config, core).await.unwrap();
    let snapshot = restored.snapshot().await;
    assert!(snapshot.registrations.is_empty());
    assert_eq!(
        snapshot
            .packages
            .iter()
            .find(|package| package.identity.as_str() == TEAM)
            .map(|package| package.source),
        Some(PackageSourceKind::Bundled),
        "the bundled Package returns after the restart"
    );
    restored
        .execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
    drop(live.fixture);
}

/// The first declared metadata page's cursor from the Team mount. The Team
/// Package declares more optional contracts than one page holds.
async fn open_cursor(live: &Live) -> String {
    let first = capability(
        &live.host,
        &live.team_mount,
        "available",
        serde_json::json!({"access":"declared"}),
    )
    .await;
    first["result"]["nextCursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a second page exists: {first}"))
        .to_owned()
}

async fn next_page(live: &Live, cursor: &str) -> serde_json::Value {
    capability(
        &live.host,
        &live.team_mount,
        "available",
        serde_json::json!({"access":"declared","cursor":cursor}),
    )
    .await
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn personal_change_invalidates_open_metadata_cursors() {
    let live = Live::open_with(|manifest| {
        manifest["consumedPorts"] = (0..34)
            .map(|n| serde_json::json!({
                "identity":format!("com.example.absent{n:02}"),"major":1,"minMinor":0,"maxMinor":0,
                "required":false,"ports":[{"name":"value.read","kind":"query"}]
            }))
            .collect();
    })
    .await;
    let path = live.package("personal", "com.example.personal", Ports::None);
    let cursor = open_cursor(&live).await;
    let page = next_page(&live, &cursor).await;
    assert_eq!(
        page["result"]["contracts"].as_array().map(Vec::len),
        Some(2),
        "the cursor works before a Personal change: {page}"
    );

    live.install(&path, "com.example.personal").await.unwrap();
    let stale = next_page(&live, &cursor).await;
    assert_eq!(stale["error"], "port_catalog_changed", "install: {stale}");

    let cursor = open_cursor(&live).await;
    live.reload("com.example.personal").await.unwrap();
    let stale = next_page(&live, &cursor).await;
    assert_eq!(stale["error"], "port_catalog_changed", "reload: {stale}");

    let cursor = open_cursor(&live).await;
    live.remove("com.example.personal").await.unwrap();
    let stale = next_page(&live, &cursor).await;
    assert_eq!(stale["error"], "port_catalog_changed", "remove: {stale}");

    let cursor = open_cursor(&live).await;
    let page = next_page(&live, &cursor).await;
    assert!(
        page["result"]["contracts"].is_array(),
        "a cursor from the current snapshot works: {page}"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09
#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn team_snapshot_entries_are_identical_across_personal_changes() {
    // The Team Package optionally consumes the contract only a Personal Package provides.
    let live = Live::open_with(|manifest| {
        manifest["consumedPorts"] = serde_json::json!([{
            "identity":CONTRACT,"major":1,"minMinor":0,"maxMinor":0,"required":false,
            "ports":[{"name":"message.post","kind":"command"}]
        }]);
    })
    .await;
    let provider = live.package("provider", "com.example.provider", Ports::Provides);
    let consumer = live.package("consumer", "com.example.consumer", Ports::Requires);
    let before = live.host.port_snapshot_for_tests();
    let team_route = &before.team[&TEAM.parse::<PackageIdentity>().unwrap()].routes;
    assert_eq!(team_route.len(), 1);
    assert_eq!(team_route[0].provider, None, "no Team Provider exists");
    let mut revisions = vec![before.revision];

    for (step, change) in [
        "install provider",
        "install consumer",
        "reload provider",
        "remove provider",
    ]
    .into_iter()
    .enumerate()
    {
        match step {
            0 => live.install(&provider, "com.example.provider").await,
            1 => live.install(&consumer, "com.example.consumer").await,
            2 => live.reload("com.example.provider").await,
            _ => live.remove("com.example.provider").await,
        }
        .unwrap();
        let after = live.host.port_snapshot_for_tests();
        assert!(
            !revisions.contains(&after.revision),
            "{change} published a new revision"
        );
        revisions.push(after.revision);
        assert_eq!(
            after.team, before.team,
            "{change} left every Team entry unchanged"
        );
        assert_eq!(after.team_providers, before.team_providers);
        let personal: Vec<_> = after.personal_providers.keys().cloned().collect();
        let expected: &[&str] = if step == 3 { &[] } else { &[CONTRACT] };
        assert_eq!(personal, expected, "{change}");
    }
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R14
#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn in_flight_call_keeps_provider_resolved_before_swap() {
    let live = Live::open().await;
    let provider = live.package("provider", "com.example.provider", Ports::Provides);
    let consumer = live.package("consumer", "com.example.consumer", Ports::Requires);
    let bystander = live.package("bystander", "com.example.bystander", Ports::None);
    live.install(&provider, "com.example.provider")
        .await
        .unwrap();
    live.install(&consumer, "com.example.consumer")
        .await
        .unwrap();
    let consumer_key = instances(&live.host, "com.example.consumer").await[0]
        .key
        .clone();
    let provider_before = instances(&live.host, "com.example.provider")
        .await
        .into_iter()
        .find(|instance| instance.key.workspace_id == consumer_key.workspace_id)
        .unwrap();
    let routed = mount(&live.host, consumer_key.clone()).await;
    let consumer_identity: PackageIdentity = "com.example.consumer".parse().unwrap();
    let mut observations = live.host.observe_ports();

    // A call resolved before an unrelated swap completes on the Provider it resolved.
    let pause = live
        .host
        .pause_next_command_authorized_for_tests(consumer_identity.clone());
    let host = live.host.clone();
    let mount_id = routed.clone();
    let held = tokio::spawn(async move { post(&host, &mount_id, "kept").await });
    pause.paused().await;
    let revision = live.host.port_snapshot_for_tests().revision;
    live.install(&bystander, "com.example.bystander")
        .await
        .unwrap();
    assert_ne!(live.host.port_snapshot_for_tests().revision, revision);
    pause.resume();
    let answer = held.await.unwrap().unwrap();
    assert_eq!(answer["result"]["accepted"], true, "{answer}");
    let dispatched = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let observation = observations.recv().await.unwrap();
            if observation.stage == PortStage::Dispatched {
                return observation;
            }
        }
    })
    .await
    .expect("the held command reached its Provider");
    assert_eq!(
        dispatched.provider_generation, provider_before.process_generation,
        "the command went to the Provider generation it resolved"
    );

    // A call resolved before its Provider's reload never reaches the new generation.
    let pause = live
        .host
        .pause_next_command_authorized_for_tests(consumer_identity);
    let host = live.host.clone();
    let mount_id = routed.clone();
    let held = tokio::spawn(async move { post(&host, &mount_id, "stale").await });
    pause.paused().await;
    live.reload("com.example.provider").await.unwrap();
    pause.resume();
    match held.await.unwrap() {
        Ok(answer) => assert!(answer["error"].is_string(), "{answer}"),
        Err(error) => panic!("the held call failed instead of answering with an error: {error:?}"),
    }
    let provider_after = instances(&live.host, "com.example.provider")
        .await
        .into_iter()
        .find(|instance| instance.key.workspace_id == consumer_key.workspace_id)
        .unwrap();
    assert!(provider_after.process_generation > provider_before.process_generation);
    let provider_mount = mount(&live.host, provider_after.key.clone()).await;
    let last = fixture_request(&live.host, &provider_mount, "fixture.port_last_actor").await;
    assert_eq!(
        last["commands"], 0,
        "the reloaded Provider received no command resolved before the swap: {last}"
    );
    let posted = post(&live.host, &routed, "after").await.unwrap();
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

const UPSTREAM: &str = "com.example.upstream";
const AGENT_BINDING: &str = "teamarness.agent-binding";

/// Rewrites one Package Directory's manifest in place.
fn edit_manifest(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let manifest_path = path.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    edit(&mut manifest);
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

/// Makes a Package provide one contract with `ports`, replacing any it had.
fn provide(path: &Path, contract: &str, ports: serde_json::Value) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,"$defs":{},"contracts":[{
            "identity":contract,"major":1,"minor":0,"description":"Fixture contract","ports":ports
        }]
    }))
    .unwrap();
    fs::write(path.join("ports.json"), &bytes).unwrap();
    edit_manifest(path, |manifest| {
        manifest["portContract"] =
            serde_json::json!({"path":"ports.json","sha256":common::digest(&bytes)});
    });
}

/// The messages contract: a guide, a command, and an event.
fn message_ports() -> serde_json::Value {
    serde_json::json!([
        {"name":"runtime.guide","kind":"query","description":"Guide",
            "request":{"type":"object","additionalProperties":false},
            "response":{"type":"object","properties":{"markdown":{"type":"string","maxLength":100}},"required":["markdown"],"additionalProperties":false}},
        {"name":"message.post","kind":"command","description":"Post a message",
            "request":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},
            "response":{"type":"object","properties":{"accepted":{"type":"boolean"}},"required":["accepted"],"additionalProperties":false}},
        {"name":"message.created","kind":"event","description":"Message was created",
            "event":{"type":"object","properties":{"order":{"type":"integer"}},"required":["order"],"additionalProperties":false}}
    ])
}

/// Declares the messages contract as consumed, optionally or required.
fn consume_messages(manifest: &mut serde_json::Value, required: bool) {
    manifest["consumedPorts"] = serde_json::json!([{
        "identity":CONTRACT,"major":1,"minMinor":0,"maxMinor":0,"required":required,
        "ports":[{"name":"message.post","kind":"command"},{"name":"message.created","kind":"event"}]
    }]);
}

fn declare_agent_binding(manifest: &mut serde_json::Value) {
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"identity":AGENT_BINDING,"major":1,"minMinor":0,"maxMinor":0}));
}

/// Binds one new Agent to each `packages` in the Workspace of `key`; returns its Principal id.
fn bind_agent(live: &Live, key: &PackageInstanceKey, packages: &[&str]) -> String {
    let workspace = live
        .fixture
        .core
        .list_workspaces()
        .unwrap()
        .into_iter()
        .find(|workspace| workspace.id().to_string() == key.workspace_id.as_str())
        .unwrap();
    let agent = live
        .fixture
        .core
        .add_agent(workspace.id(), "Personal tier Agent")
        .unwrap();
    for package in packages {
        live.fixture
            .core
            .bind_agent_package(workspace.id(), agent, package)
            .unwrap();
    }
    agent.to_string()
}

fn invoke_post(access: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"access":access,"contract":CONTRACT,"major":1,"port":"message.post",
        "kind":"command","request":{"text":text}})
}

/// The Instance of `identity` in the Workspace of `key`.
async fn sibling(
    host: &PackageHost,
    identity: &str,
    key: &PackageInstanceKey,
) -> PackageInstanceKey {
    instances(host, identity)
        .await
        .into_iter()
        .find(|instance| instance.key.workspace_id == key.workspace_id)
        .unwrap()
        .key
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn team_caller_cannot_call_or_discover_personal_provider() {
    // The Team Package declares the contract that only a Personal Package provides, and is a
    // bound Agent harness, so every access view is tried.
    let live = Live::open_with(|manifest| {
        consume_messages(manifest, false);
        declare_agent_binding(manifest);
    })
    .await;
    let provider = live.package("provider", "com.example.provider", Ports::None);
    provide(&provider, CONTRACT, message_ports());
    let harness = live.package("harness", "com.example.harness", Ports::None);
    edit_manifest(&harness, |manifest| {
        consume_messages(manifest, false);
        declare_agent_binding(manifest);
    });
    live.install(&provider, "com.example.provider")
        .await
        .unwrap();
    live.install(&harness, "com.example.harness").await.unwrap();
    let team_key = live.team_before[0].key.clone();
    bind_agent(&live, &team_key, &[TEAM, "com.example.harness"]);
    let team = &live.team_mount;
    let refused = |answer: &serde_json::Value, code: &str, what: &str| {
        assert_eq!(answer["error"], code, "{what}: {answer}");
    };

    // Declared route and declared metadata.
    refused(
        &capability(&live.host, team, "invoke", invoke_post("declared", "team")).await,
        "port_unavailable",
        "declared call",
    );
    let declared = capability(
        &live.host,
        team,
        "available",
        serde_json::json!({"access":"declared"}),
    )
    .await;
    assert_eq!(
        declared["result"]["contracts"],
        serde_json::json!([{"contract":CONTRACT,"major":1,"minor":null,"description":null,
            "available":false,"enabled":false,"enabledMajor":null,"metadataSha256":null}]),
        "the declared row shows no Provider: {declared}"
    );
    // Agent metadata, selection, and a dynamic Agent call.
    let agent = capability(
        &live.host,
        team,
        "available",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    assert_eq!(
        agent["result"]["contracts"],
        serde_json::json!([]),
        "the guide is not discovered: {agent}"
    );
    refused(
        &capability(
            &live.host,
            team,
            "describe",
            serde_json::json!({"access":"agent","contract":CONTRACT,"metadataSha256":"0".repeat(64),"offset":0}),
        )
        .await,
        "port_unavailable",
        "Agent metadata",
    );
    refused(
        &capability(
            &live.host,
            team,
            "enable",
            serde_json::json!({"contract":CONTRACT,"major":1}),
        )
        .await,
        "port_unavailable",
        "selection",
    );
    refused(
        &capability(&live.host, team, "invoke", invoke_post("agent", "team")).await,
        "port_not_enabled",
        "dynamic Agent call",
    );
    let provider_key = sibling(&live.host, "com.example.provider", &team_key).await;
    let provider_mount = mount(&live.host, provider_key).await;
    let last = fixture_request(&live.host, &provider_mount, "fixture.port_last_actor").await;
    assert_eq!(
        last["commands"], 0,
        "no Team call reached the Provider: {last}"
    );

    // A Personal caller discovers, selects, and calls the same Provider in both views.
    let harness_key = sibling(&live.host, "com.example.harness", &team_key).await;
    let personal = mount(&live.host, harness_key).await;
    let agent = capability(
        &live.host,
        &personal,
        "available",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    assert_eq!(
        agent["result"]["contracts"][0]["contract"], CONTRACT,
        "{agent}"
    );
    assert_eq!(
        agent["result"]["contracts"][0]["available"], true,
        "{agent}"
    );
    let selected = capability(
        &live.host,
        &personal,
        "enable",
        serde_json::json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    assert_eq!(selected["result"]["enabled"], true, "{selected}");
    for access in ["declared", "agent"] {
        let answer = capability(&live.host, &personal, "invoke", invoke_post(access, "p")).await;
        assert_eq!(answer["result"]["accepted"], true, "{access}: {answer}");
    }
    let last = fixture_request(&live.host, &provider_mount, "fixture.port_last_actor").await;
    assert_eq!(last["commands"], 2, "{last}");
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// The consumed-Port routes the authoring catalog shows for `identity`.
fn catalog_routes(
    host: &PackageHost,
    identity: &str,
) -> Vec<teamarness_package_host::PortAvailability> {
    host.authoring_catalog()
        .into_iter()
        .find(|entry| entry.package.identity.as_str() == identity)
        .unwrap()
        .availability
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn provider_losing_required_dependency_is_unavailable_to_existing_and_new_consumers() {
    let live = Live::open().await;
    let upstream = live.package("upstream", UPSTREAM, Ports::None);
    provide(
        &upstream,
        UPSTREAM,
        serde_json::json!([{"name":"value.read","kind":"query","description":"Read",
            "request":true,"response":true}]),
    );
    // The middle Provider requires the upstream contract.
    let middle = live.package("middle", "com.example.middle", Ports::Provides);
    edit_manifest(&middle, |manifest| {
        manifest["consumedPorts"] = serde_json::json!([{
            "identity":UPSTREAM,"major":1,"minMinor":0,"maxMinor":0,"required":true,
            "ports":[{"name":"value.read","kind":"query"}]
        }]);
    });
    let consumer = live.package("consumer", "com.example.consumer", Ports::Requires);
    let late = live.package("late", "com.example.late", Ports::Requires);
    for (path, identity) in [
        (&upstream, UPSTREAM),
        (&middle, "com.example.middle"),
        (&consumer, "com.example.consumer"),
    ] {
        live.install(path, identity).await.unwrap();
    }
    let routed = catalog_routes(&live.host, "com.example.consumer");
    assert_eq!(
        routed[0].provider.as_ref().map(PackageIdentity::as_str),
        Some("com.example.middle")
    );

    live.remove(UPSTREAM).await.unwrap();

    // The middle Provider keeps running, but serves neither the existing consumer...
    assert_active(&instances(&live.host, "com.example.middle").await, 2);
    let existing = catalog_routes(&live.host, "com.example.consumer");
    assert_eq!(existing[0].provider, None, "{existing:?}");
    assert_eq!(
        existing[0].unavailable,
        Some(PackageErrorCode::PortContractMissing)
    );
    let consumers = instances(&live.host, "com.example.consumer").await;
    assert_active(&consumers, 2);
    let unrouted = mount(&live.host, consumers[0].key.clone()).await;
    let refused = post(&live.host, &unrouted, "hello").await.unwrap();
    assert_eq!(refused["error"], "port_unavailable", "{refused}");
    // ...nor a consumer installed now: both see the same availability.
    let late_refused = live.install(&late, "com.example.late").await.unwrap_err();
    assert_eq!(late_refused.code, existing[0].unavailable.unwrap());
    assert!(instances(&live.host, "com.example.late").await.is_empty());

    live.install(&upstream, UPSTREAM).await.unwrap();

    let restored = catalog_routes(&live.host, "com.example.consumer");
    assert_eq!(
        restored[0].provider.as_ref().map(PackageIdentity::as_str),
        Some("com.example.middle"),
        "{restored:?}"
    );
    let consumers = instances(&live.host, "com.example.consumer").await;
    let rerouted = mount(&live.host, consumers[0].key.clone()).await;
    let posted = post(&live.host, &rerouted, "again").await.unwrap();
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    live.install(&late, "com.example.late").await.unwrap();
    assert_active(&instances(&live.host, "com.example.late").await, 2);
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Sends one fixture request with `payload` to a mount and returns its payload.
async fn fixture_call(
    host: &PackageHost,
    mount: &UiMountId,
    operation: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let outcome = host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!("call-{}", rand_suffix())).unwrap(),
            operation: operation.into(),
            payload: serde_json::from_value(payload).unwrap(),
        }))
        .await
        .unwrap_or_else(|error| panic!("{operation}: {error:?}"));
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        panic!("{outcome:?}")
    };
    serde_json::to_value(payload).unwrap()
}

/// Waits until the Host has registered a subscription for `consumer`.
async fn wait_subscribed(
    observations: &mut teamarness_package_host::PortObservations,
    consumer: &str,
) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let event = observations.recv().await.expect("Port observation");
            if event.stage == PortStage::Subscribed && event.consumer_package == consumer {
                break;
            }
        }
    })
    .await
    .expect("the consumer's subscription was registered");
}

/// A Personal Provider of the messages contract, a declared Personal consumer, and a bound
/// Personal Agent harness, all installed; returns the Workspace key the mounts use.
async fn install_messages_tier(live: &Live) -> (PackageInstanceKey, String) {
    let provider = live.package("provider", "com.example.provider", Ports::None);
    provide(&provider, CONTRACT, message_ports());
    let consumer = live.package("consumer", "com.example.consumer", Ports::None);
    edit_manifest(&consumer, |manifest| consume_messages(manifest, false));
    let harness = live.package("harness", "com.example.harness", Ports::None);
    edit_manifest(&harness, declare_agent_binding);
    for (path, identity) in [
        (&provider, "com.example.provider"),
        (&consumer, "com.example.consumer"),
        (&harness, "com.example.harness"),
    ] {
        live.install(path, identity).await.unwrap();
    }
    let key = live.team_before[0].key.clone();
    let agent = bind_agent(live, &key, &["com.example.harness"]);
    (key, agent)
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R14
#[tokio::test]
async fn personal_provider_reload_ends_subscriptions_with_provider_exited() {
    let live = Live::open().await;
    let (key, _) = install_messages_tier(&live).await;
    let consumer = mount(
        &live.host,
        sibling(&live.host, "com.example.consumer", &key).await,
    )
    .await;
    let harness = mount(
        &live.host,
        sibling(&live.host, "com.example.harness", &key).await,
    )
    .await;
    let enabled = capability(
        &live.host,
        &harness,
        "enable",
        serde_json::json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    assert_eq!(enabled["result"]["enabled"], true, "{enabled}");
    let mut observations = live.host.observe_ports();
    fixture_call(
        &live.host,
        &consumer,
        "fixture.port_event_open",
        serde_json::json!({}),
    )
    .await;
    wait_subscribed(&mut observations, "com.example.consumer").await;
    fixture_call(
        &live.host,
        &harness,
        "fixture.port_event_open",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    wait_subscribed(&mut observations, "com.example.harness").await;
    let provider = mount(
        &live.host,
        sibling(&live.host, "com.example.provider", &key).await,
    )
    .await;
    // Both subscriptions share one upstream to the Provider.
    common::eventually(Duration::from_secs(8), "the shared upstream opens", || {
        let host = &live.host;
        let provider = provider.clone();
        async move {
            fixture_call(
                host,
                &provider,
                "fixture.port_event_stats",
                serde_json::json!({}),
            )
            .await["active"]
                == 1
        }
    })
    .await;
    fixture_call(
        &live.host,
        &provider,
        "fixture.port_event_emit",
        serde_json::json!({"order":1}),
    )
    .await;
    let events = |mount: UiMountId| {
        let host = live.host.clone();
        async move {
            fixture_call(
                &host,
                &mount,
                "fixture.port_event_events",
                serde_json::json!({}),
            )
            .await
        }
    };
    common::eventually(Duration::from_secs(8), "the first event", || {
        let (consumer, harness) = (consumer.clone(), harness.clone());
        async move {
            events(consumer).await["events"] == serde_json::json!([{"order":1}])
                && events(harness).await["events"] == serde_json::json!([{"order":1}])
        }
    })
    .await;
    let consumers_before = generations(&instances(&live.host, "com.example.consumer").await);
    let harness_before = generations(&instances(&live.host, "com.example.harness").await);

    live.reload("com.example.provider").await.unwrap();

    common::eventually(Duration::from_secs(8), "both streams end", || {
        let (consumer, harness) = (consumer.clone(), harness.clone());
        async move {
            events(consumer).await["closeCode"] == "provider_exited"
                && events(harness).await["closeCode"] == "provider_exited"
        }
    })
    .await;
    // The consumers kept running; only their streams ended.
    assert_eq!(
        generations(&instances(&live.host, "com.example.consumer").await),
        consumers_before
    );
    assert_eq!(
        generations(&instances(&live.host, "com.example.harness").await),
        harness_before
    );
    // A new subscription reaches the new generation and nothing is replayed.
    fixture_call(
        &live.host,
        &consumer,
        "fixture.port_event_open",
        serde_json::json!({}),
    )
    .await;
    wait_subscribed(&mut observations, "com.example.consumer").await;
    let provider = mount(
        &live.host,
        sibling(&live.host, "com.example.provider", &key).await,
    )
    .await;
    common::eventually(Duration::from_secs(8), "new upstream", || {
        let host = &live.host;
        let provider = provider.clone();
        async move {
            fixture_call(
                host,
                &provider,
                "fixture.port_event_stats",
                serde_json::json!({}),
            )
            .await["active"]
                == 1
        }
    })
    .await;
    assert_eq!(
        events(consumer.clone()).await["events"],
        serde_json::json!([])
    );
    fixture_call(
        &live.host,
        &provider,
        "fixture.port_event_emit",
        serde_json::json!({"order":2}),
    )
    .await;
    common::eventually(
        Duration::from_secs(8),
        "the new event without replay",
        || {
            let consumer = consumer.clone();
            async move { events(consumer).await["events"] == serde_json::json!([{"order":2}]) }
        },
    )
    .await;
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13
#[tokio::test]
async fn removed_personal_provider_selection_is_unavailable_then_returns() {
    let live = Live::open().await;
    let (key, _) = install_messages_tier(&live).await;
    let harness = mount(
        &live.host,
        sibling(&live.host, "com.example.harness", &key).await,
    )
    .await;
    let enabled = capability(
        &live.host,
        &harness,
        "enable",
        serde_json::json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    assert_eq!(enabled["result"]["enabled"], true, "{enabled}");
    let posted = capability(
        &live.host,
        &harness,
        "invoke",
        invoke_post("agent", "before"),
    )
    .await;
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    let provider = live.fixture._root.path().join("sources/provider");

    live.remove("com.example.provider").await.unwrap();

    let missing = capability(
        &live.host,
        &harness,
        "available",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    assert_eq!(
        missing["result"]["contracts"],
        serde_json::json!([{"contract":CONTRACT,"major":1,"minor":null,"description":null,
            "available":false,"enabled":true,"enabledMajor":1,"metadataSha256":null}]),
        "the selection is kept and shown unavailable: {missing}"
    );
    let refused = capability(&live.host, &harness, "invoke", invoke_post("agent", "gone")).await;
    assert_eq!(refused["error"], "port_unavailable", "{refused}");

    live.install(&provider, "com.example.provider")
        .await
        .unwrap();

    let returned = capability(
        &live.host,
        &harness,
        "available",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    let row = &returned["result"]["contracts"][0];
    assert_eq!(row["available"], true, "{returned}");
    assert_eq!(row["enabled"], true, "{returned}");
    assert_eq!(row["enabledMajor"], 1, "{returned}");
    let posted = capability(
        &live.host,
        &harness,
        "invoke",
        invoke_post("agent", "after"),
    )
    .await;
    assert_eq!(
        posted["result"]["accepted"], true,
        "the selection applies again: {posted}"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R14
///
/// The Host does not report "uncertain" to the reloaded harness: the caller that held the
/// command has exited, so the uncertainty belongs to that exited caller and to the Agent's own
/// record of its unresolved command. The Host's guarantee is at-most-once dispatch: the command
/// reaches its Provider once, nothing replays it to the new harness generation, and the binding
/// and selection survive the reload.
#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn personal_agent_reload_with_unresolved_command_dispatches_once_without_replay() {
    let live = Live::open().await;
    let (key, agent) = install_messages_tier(&live).await;
    let harness = mount(
        &live.host,
        sibling(&live.host, "com.example.harness", &key).await,
    )
    .await;
    let enabled = capability(
        &live.host,
        &harness,
        "enable",
        serde_json::json!({"contract":CONTRACT,"major":1}),
    )
    .await;
    assert_eq!(enabled["result"]["enabled"], true, "{enabled}");
    let harness_before = instances(&live.host, "com.example.harness").await;
    let generation_before = harness_before
        .iter()
        .find(|instance| instance.key.workspace_id == key.workspace_id)
        .and_then(|instance| instance.process_generation)
        .expect("the mounted harness is running before the reload");
    let mut observations = live.host.observe_ports();

    // Hold one authorized Agent command before dispatch, then reload its harness.
    let pause = live
        .host
        .pause_next_command_authorized_for_tests("com.example.harness".parse().unwrap());
    let _resume = pause.resume_on_drop();
    let held = tokio::spawn({
        let host = live.host.clone();
        let mount = harness.clone();
        async move {
            host.execute(PackageHostCommand::UiRequest(UiRequest {
                mount,
                request: UiRequestId::new("held-command").unwrap(),
                operation: "fixture.capability".into(),
                payload: serde_json::from_value(serde_json::json!({
                    "capability":"teamarness.package-ports","method":"invoke",
                    "arguments":invoke_post("agent", "held")
                }))
                .unwrap(),
            }))
            .await
        }
    });
    pause.paused().await;
    let reload = tokio::spawn({
        let host = live.host.clone();
        async move {
            host.execute(PackageHostCommand::ReloadPersonal(ReloadPersonal {
                identity: "com.example.harness".parse().unwrap(),
            }))
            .await
        }
    });
    // The harness process ends under the held command, so its request ends with the
    // process.
    let ended = tokio::time::timeout(Duration::from_secs(20), held)
        .await
        .expect("the held request ends with its process")
        .unwrap();
    match ended {
        Ok(answer) => panic!("the held command reported an outcome: {answer:?}"),
        Err(error) => assert_eq!(error.code, PackageErrorCode::ProcessExited, "{error:?}"),
    }
    pause.resume();
    reload.await.unwrap().unwrap();
    let harness_after = instances(&live.host, "com.example.harness").await;
    assert_active(&harness_after, 2);
    for (old, new) in harness_before.iter().zip(&harness_after) {
        assert!(new.process_generation > old.process_generation);
    }

    // The authorized command runs at most once, as the actor it was authorized for, and
    // nothing replays it to the new harness generation.
    let terminal = tokio::time::timeout(Duration::from_secs(20), async {
        let mut dispatched = Vec::new();
        loop {
            let observation = observations.recv().await.unwrap();
            if observation.consumer_package != "com.example.harness" {
                continue;
            }
            assert_eq!(
                observation.consumer_generation, generation_before,
                "{observation:?}"
            );
            match observation.stage {
                PortStage::Dispatched => dispatched.push(observation),
                PortStage::Completed | PortStage::Failed => return (dispatched, observation),
                _ => {}
            }
        }
    })
    .await
    .expect("the held command ends");
    assert_eq!(terminal.0.len(), 1, "dispatched once: {:?}", terminal.0);
    // The Provider finishes the command it received; the Port observation for it is certain,
    // even though its exited caller never learns the outcome.
    let terminal_observation = &terminal.1;
    assert_eq!(
        terminal_observation.stage,
        PortStage::Completed,
        "{terminal_observation:?}"
    );
    assert_eq!(terminal_observation.code, None, "{terminal_observation:?}");
    assert!(
        !terminal_observation.outcome_uncertain,
        "{terminal_observation:?}"
    );
    let provider = mount(
        &live.host,
        sibling(&live.host, "com.example.provider", &key).await,
    )
    .await;
    let last = fixture_request(&live.host, &provider, "fixture.port_last_actor").await;
    assert_eq!(last["commands"], 1, "no retry: {last}");
    assert_eq!(last["actor"]["principal"], agent.as_str(), "{last}");

    // The reloaded harness rereads its binding and selection; only a new call reaches the
    // Provider.
    let harness = mount(
        &live.host,
        sibling(&live.host, "com.example.harness", &key).await,
    )
    .await;
    let available = capability(
        &live.host,
        &harness,
        "available",
        serde_json::json!({"access":"agent"}),
    )
    .await;
    let row = &available["result"]["contracts"][0];
    assert_eq!(row["enabled"], true, "{available}");
    assert_eq!(row["enabledMajor"], 1, "{available}");
    assert_eq!(
        fixture_request(&live.host, &provider, "fixture.port_last_actor").await["commands"],
        1
    );
    let posted = capability(
        &live.host,
        &harness,
        "invoke",
        invoke_post("agent", "after"),
    )
    .await;
    assert_eq!(posted["result"]["accepted"], true, "{posted}");
    let last = fixture_request(&live.host, &provider, "fixture.port_last_actor").await;
    assert_eq!(last["commands"], 2, "{last}");
    assert_eq!(
        last["actor"]["principal"],
        agent.as_str(),
        "the binding survived: {last}"
    );
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}

/// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R16, TEAM.SYS.PKG-R17
#[tokio::test]
async fn personal_package_declaring_content_views_serves_content() {
    let live = Live::open().await;
    let path = live.package("viewer", "com.example.viewer", Ports::None);
    edit_manifest(&path, |manifest| {
        manifest["capabilities"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "identity":"teamarness.content-views","major":1,"minMinor":0,"maxMinor":0
            }));
    });
    live.install(&path, "com.example.viewer").await.unwrap();
    let key = instances(&live.host, "com.example.viewer").await[0]
        .key
        .clone();
    let (viewer, nested) = match live
        .host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(
            key.clone(),
        )))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, loader, .. } => (mount, loader.nested_content),
        other => panic!("{other:?}"),
    };
    assert!(nested, "the Personal mount reports nested content");
    let bytes = b"<h1>Personal exhibit</h1>";
    let source = live.data_directory(&key).join("content/index");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, bytes).unwrap();
    let opened = fixture_call(
        &live.host,
        &viewer,
        "fixture.capability",
        serde_json::json!({"capability":"teamarness.content-views","method":"open","arguments":{
            "entry":"index.html","files":[{"path":"index.html","sha256":common::digest(bytes),
                "size":bytes.len(),"source":{"kind":"file","path":"content/index"}}]
        }}),
    )
    .await;
    let token = opened["result"]["view"]
        .as_str()
        .unwrap_or_else(|| panic!("the view opens: {opened}"));
    let served = live
        .host
        .execute(PackageHostCommand::ReadContent(ReadContent {
            token: ContentViewToken::new(token.to_owned()).unwrap(),
            path: "index.html".into(),
        }))
        .await
        .unwrap();
    let PackageHostOutcome::Content {
        bytes: served,
        media_type,
    } = served
    else {
        panic!("{served:?}")
    };
    assert_eq!(served, bytes);
    assert_eq!(media_type, "text/html; charset=utf-8");

    // The open Team mount and its Instances are unchanged.
    live.assert_team_undisturbed().await;
    live.shutdown().await;
}
