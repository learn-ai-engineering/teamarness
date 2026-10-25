//! The live Personal tier: a running Host installs, reloads, and removes a
//! Personal Package without disturbing any Team Package (ADR 0031).

#![cfg(unix)]
// Host commands return the full, payload-free error envelope.
#![allow(clippy::result_large_err)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use teamarness_package_host::{
    InstallPersonal, LifecycleState, MountState, OpenUiMount, PackageErrorCode,
    PackageErrorEnvelope, PackageHost, PackageHostCommand, PackageHostOutcome, PackageIdentity,
    PackageInstanceKey, PackageInstanceSnapshot, PackageSourceKind, PackageTier, PortStage,
    ReconcileWorkspaces, RegistrationStatus, ReloadPersonal, RemoveOverride, RemovePersonal,
    ShutdownHost, UiMountId, UiRequest, UiRequestId, WriteOverride,
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
        serde_json::json!({"identity":"teamarness.package-ports","major":1,"minMinor":0,"maxMinor":0}),
    ]
}

impl Live {
    async fn open() -> Self {
        let fixture =
            common::Fixture::node_with_capabilities(&[("team", TEAM)], &capabilities()).await;
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
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key,
        }))
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
                "arguments":{"contract":CONTRACT,"port":"message.post","kind":"command","request":{"text":text}}
            }))
            .unwrap(),
        }))
        .await?;
    let PackageHostOutcome::UiResponse { payload } = outcome else {
        panic!("{outcome:?}")
    };
    Ok(serde_json::to_value(payload).unwrap())
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
    assert!(
        live.host
            .authoring_catalog()
            .iter()
            .any(|entry| entry.package.identity.as_str() == "com.example.personal")
    );
    live.assert_team_undisturbed().await;
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
    // The running Host is unchanged: same revision, same Team Package and Instances.
    let snapshot = live.host.snapshot().await;
    assert_eq!(snapshot.revision, revision);
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
    let removed = reopened
        .execute(PackageHostCommand::RemoveOverride(RemoveOverride {
            identity: TEAM.parse().unwrap(),
        }))
        .await
        .unwrap();
    assert_eq!(removed, PackageHostOutcome::OverrideRemoved);
    assert_eq!(
        source(&reopened.snapshot().await),
        Some(PackageSourceKind::Local)
    );
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
