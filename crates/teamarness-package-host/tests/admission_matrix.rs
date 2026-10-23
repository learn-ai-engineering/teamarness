#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub use teamarness_package_host::{
    DiagnosticId, HostRevision, InstallationPublicIdentity, PackageCatalogEntry, PackageErrorCode,
    PackageErrorEnvelope, PackageErrorStage, PackageHostConfig, PackageHostSnapshot,
    PackageHostState, PackageIdentity, PackageSourceKind, PackageTrustKind, PackageValue,
    RecoveryAction,
};

pub struct AuthoringCatalogEntry {
    package: PackageCatalogEntry,
    provided: Vec<teamarness_package_contract::port_contract::ProvidedContract>,
    definitions: serde_json::Map<String, serde_json::Value>,
    consumed: Vec<teamarness_package_contract::manifest::ConsumedContract>,
    availability: Vec<admission::PortAvailability>,
    diagnostics: Vec<PackageErrorEnvelope>,
}

#[path = "../src/admission.rs"]
mod admission;
#[path = "../src/catalog.rs"]
mod catalog;
#[path = "../src/diag.rs"]
mod diag;
#[path = "../src/host_store.rs"]
mod host_store;
#[path = "../src/manifest.rs"]
mod manifest;
#[path = "../src/ui.rs"]
mod ui;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Fixture {
    root: TempDir,
    application: PathBuf,
    private: PathBuf,
    locals: Vec<TempDir>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application");
        let private = root.path().join("private");
        fs::create_dir_all(&application).unwrap();
        fs::create_dir_all(private.join("packages")).unwrap();
        fs::write(
            application.join("application-packages.v1.json"),
            br#"{"schemaVersion":1,"packages":[]}"#,
        )
        .unwrap();
        Self {
            root,
            application,
            private,
            locals: vec![],
        }
    }

    fn config(&self) -> PackageHostConfig {
        PackageHostConfig::new(
            &self.application,
            self.application.join("application-packages.v1.json"),
            &self.private,
            InstallationPublicIdentity::new("installation1").unwrap(),
        )
    }

    fn add_application(&self, directory: &str, identity: &str, version: &str) {
        write_package(&self.application.join(directory), identity, version);
        let path = self.application.join("application-packages.v1.json");
        let mut index: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        index["packages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"directory": directory}));
        fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
    }

    fn add_local(&mut self, identity: &str, version: &str) -> PathBuf {
        let temp = tempfile::tempdir().unwrap();
        write_package(temp.path(), identity, version);
        let path = temp.path().to_owned();
        self.locals.push(temp);
        path
    }

    fn registrations(&self, entries: Vec<serde_json::Value>) {
        let path = self.private.join("packages/registrations.v1.json");
        fs::write(
            path.clone(),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "registrations": entries
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_package(root: &Path, identity: &str, version: &str) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    let backend = format!("backend-{identity}-{version}").into_bytes();
    let entrypoint = format!("export const identity = '{identity}';").into_bytes();
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#.to_vec();
    fs::write(root.join("backend/package"), &backend).unwrap();
    fs::write(root.join("ui/index.js"), &entrypoint).unwrap();
    fs::write(root.join("ui/contract.json"), &contract).unwrap();
    fs::write(root.join("teamarness.package.json"), serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "identity": identity,
        "version": version,
        "presentation": {"displayName": identity},
        "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
        "backends": [{"target":"aarch64-apple-darwin","path":"backend/package","size":backend.len(),"sha256":digest(&backend)}],
        "ui": {
            "entrypoint":{"path":"ui/index.js","size":entrypoint.len(),"sha256":digest(&entrypoint)},
            "assets":[],
            "contract":{"path":"ui/contract.json","sha256":digest(&contract)}
        },
        "capabilities": []
    })).unwrap()).unwrap();
}

fn local_entry(path: &Path, identity: &str, override_application: bool) -> serde_json::Value {
    serde_json::json!({
        "directory": path,
        "expectedIdentity": identity,
        "trust": {"kind":"trusted_native_code_v1","acceptedRisks":["filesystem","network","process"]},
        "overrideApplication": override_application
    })
}

fn package_ids(report: &admission::AdmissionReport) -> Vec<String> {
    report
        .snapshot
        .packages
        .iter()
        .map(|item| item.identity.to_string())
        .collect()
}

type FixtureMutation = Box<dyn Fn(&mut Fixture)>;

/// Admits through a freshly opened store, as `PackageHost::open` does through
/// its shared store handle.
fn admit(
    config: &PackageHostConfig,
) -> Result<admission::AdmissionReport, admission::AdmissionFailure> {
    let store = host_store::HostStore::open(config.private_data_directory()).unwrap();
    admission::admit(config, &store)
}

#[test]
fn each_invalid_source_isolated_from_an_unrelated_valid_identity() {
    let cases: Vec<(&str, FixtureMutation)> = vec![
        (
            "source_not_found",
            Box::new(|fixture| {
                let path = fixture.application.join("application-packages.v1.json");
                let mut index: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                index["packages"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"directory":"missing"}));
                fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
            }),
        ),
        (
            "manifest_malformed",
            Box::new(|fixture| {
                fs::create_dir_all(fixture.application.join("bad")).unwrap();
                fs::write(
                    fixture.application.join("bad/teamarness.package.json"),
                    b"{",
                )
                .unwrap();
                let path = fixture.application.join("application-packages.v1.json");
                let mut index: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                index["packages"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"directory":"bad"}));
                fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
            }),
        ),
        (
            "artifact_changed",
            Box::new(|fixture| {
                fixture.add_application("bad", "com.example.bad", "1.0.0");
                fs::write(fixture.application.join("bad/backend/package"), b"tampered").unwrap();
            }),
        ),
        (
            "contract",
            Box::new(|fixture| {
                fixture.add_application("bad", "com.example.bad", "1.0.0");
                let path = fixture.application.join("bad/ui/contract.json");
                let bytes = br#"{"$ref":"https://example.com/schema"}"#;
                fs::write(&path, bytes).unwrap();
                let manifest_path = fixture.application.join("bad/teamarness.package.json");
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
                manifest["ui"]["contract"]["sha256"] = digest(bytes).into();
                fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            }),
        ),
    ];
    for (name, mutate) in cases {
        let mut fixture = Fixture::new();
        fixture.add_application("good", "com.example.good", "1.0.0");
        mutate(&mut fixture);
        let report = admit(&fixture.config()).unwrap();
        assert!(
            package_ids(&report).contains(&"com.example.good".to_owned()),
            "{name}"
        );
        assert!(!report.snapshot.diagnostics.is_empty(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn dangling_registration_symlink_is_reported_instead_of_silent_bundled_fallback() {
    let fixture = Fixture::new();
    fixture.add_application("reference", "dev.teamarness.reference", "1.0.0");
    let registration = fixture.private.join("packages/registrations.v1.json");
    symlink(
        fixture.root.path().join("missing-registrations"),
        registration,
    )
    .unwrap();

    let report = admit(&fixture.config()).unwrap();

    assert!(package_ids(&report).contains(&"dev.teamarness.reference".to_owned()));
    assert!(
        report
            .snapshot
            .diagnostics
            .iter()
            .any(|error| error.code == PackageErrorCode::InvalidRegistration)
    );
}

#[test]
fn registration_metadata_policy_rejects_wrong_owner_permissions_and_symlinks() {
    use catalog::RegistrationFileKind;
    assert!(
        catalog::validate_registration_file_policy(
            0o100600,
            501,
            501,
            RegistrationFileKind::Regular
        )
        .is_ok()
    );
    for (mode, owner, kind) in [
        (0o100640, 501, RegistrationFileKind::Regular),
        (0o100600, 502, RegistrationFileKind::Regular),
        (0o120777, 501, RegistrationFileKind::Symlink),
        (0o040700, 501, RegistrationFileKind::Other),
    ] {
        assert_eq!(
            catalog::validate_registration_file_policy(mode, owner, 501, kind)
                .unwrap_err()
                .code,
            PackageErrorCode::InvalidRegistration
        );
    }
}

#[test]
fn local_registration_can_add_a_package_without_an_application_counterpart() {
    let mut fixture = Fixture::new();
    let local = fixture.add_local("dev.teamarness.tutorial", "1.0.0");
    fixture.registrations(vec![local_entry(&local, "dev.teamarness.tutorial", false)]);

    let report = admit(&fixture.config()).unwrap();

    assert!(package_ids(&report).contains(&"dev.teamarness.tutorial".to_owned()));
    assert!(report.snapshot.diagnostics.is_empty());
}

#[test]
fn local_registration_requires_absolute_owner_only_non_symlink_file_and_complete_trust() {
    let mut fixture = Fixture::new();
    fixture.add_application("good", "com.example.good", "1.0.0");
    let local = fixture.add_local("com.example.local", "1.0.0");
    fixture.registrations(vec![local_entry(&local, "com.example.local", false)]);
    let report = admit(&fixture.config()).unwrap();
    assert!(package_ids(&report).contains(&"com.example.local".to_owned()));

    let path = fixture.private.join("packages/registrations.v1.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(
        report.snapshot.diagnostics[0].code,
        PackageErrorCode::InvalidRegistration
    );
    assert!(package_ids(&report).contains(&"com.example.good".to_owned()));

    fs::remove_file(&path).unwrap();
    let target = fixture.root.path().join("registrations-target.json");
    fs::write(&target, b"{}").unwrap();
    symlink(target, &path).unwrap();
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(
        report.snapshot.diagnostics[0].code,
        PackageErrorCode::InvalidRegistration
    );

    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        br#"{"schemaVersion":1,"schemaVersion":1,"registrations":[]}"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(
        report.snapshot.diagnostics[0].code,
        PackageErrorCode::InvalidRegistration
    );

    fixture.registrations(vec![serde_json::json!({
        "directory":"relative/path", "expectedIdentity":"com.example.local",
        "trust":{"kind":"trusted_native_code_v1","acceptedRisks":["filesystem"]}
    })]);
    let report = admit(&fixture.config()).unwrap();
    assert!(
        report
            .snapshot
            .diagnostics
            .iter()
            .any(|error| error.code == PackageErrorCode::InvalidRegistration)
    );

    fixture.registrations(vec![serde_json::json!({
        "directory":local, "expectedIdentity":"com.example.local",
        "trust":{"kind":"trusted_native_code_v1","acceptedRisks":["filesystem"]}
    })]);
    let report = admit(&fixture.config()).unwrap();
    assert!(
        report
            .snapshot
            .diagnostics
            .iter()
            .any(|error| error.code == PackageErrorCode::TrustEvidenceMissing)
    );
}

#[test]
fn explicit_override_matrix_is_order_and_version_independent() {
    for local_first in [false, true] {
        for local_version in ["0.1.0", "99.0.0"] {
            let mut fixture = Fixture::new();
            fixture.add_application("bundled", "com.example.same", "5.0.0");
            fixture.add_application("other-app", "com.example.app", "1.0.0");
            if local_first {
                let path = fixture.application.join("application-packages.v1.json");
                let mut index: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                index["packages"].as_array_mut().unwrap().reverse();
                fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
            }
            let local = fixture.add_local("com.example.same", local_version);
            let other_local = fixture.add_local("com.example.local", "1.0.0");
            let replacement = local_entry(&local, "com.example.same", true);
            let unrelated = local_entry(&other_local, "com.example.local", false);
            let entries = if local_first {
                vec![replacement, unrelated]
            } else {
                vec![unrelated, replacement]
            };
            fixture.registrations(entries);
            let report = admit(&fixture.config()).unwrap();
            let selected = report
                .snapshot
                .packages
                .iter()
                .find(|package| package.identity.as_str() == "com.example.same")
                .unwrap();
            assert_eq!(selected.source, PackageSourceKind::Local);
            assert_eq!(selected.version, local_version);
            assert_eq!(report.snapshot.packages.len(), 3);
        }
    }

    for defect in ["override", "trust", "identity"] {
        let mut fixture = Fixture::new();
        fixture.add_application("bundled", "com.example.same", "1.0.0");
        let declared = if defect == "identity" {
            "com.example.other"
        } else {
            "com.example.same"
        };
        let local = fixture.add_local(declared, "2.0.0");
        let mut entry = local_entry(&local, "com.example.same", defect != "override");
        if defect == "trust" {
            entry["trust"] = serde_json::Value::Null;
        }
        fixture.registrations(vec![entry]);
        let report = admit(&fixture.config()).unwrap();
        assert!(
            report.snapshot.packages.is_empty(),
            "{defect} must not fall back"
        );
        let expected = match defect {
            "override" => PackageErrorCode::OverrideRequired,
            "trust" => PackageErrorCode::TrustEvidenceMissing,
            _ => PackageErrorCode::IdentityMismatch,
        };
        assert!(
            report
                .snapshot
                .diagnostics
                .iter()
                .any(|error| error.code == expected)
        );
    }
}

#[test]
fn duplicate_local_or_application_identities_are_unavailable() {
    let fixture = Fixture::new();
    fixture.add_application("one", "com.example.same", "1.0.0");
    fixture.add_application("two", "com.example.same", "2.0.0");
    let report = admit(&fixture.config()).unwrap();
    assert!(report.snapshot.packages.is_empty());
    assert_eq!(
        report.snapshot.diagnostics[0].code,
        PackageErrorCode::DuplicateIdentity
    );

    let mut fixture = Fixture::new();
    let first = fixture.add_local("com.example.same", "1.0.0");
    let second = fixture.add_local("com.example.same", "2.0.0");
    fixture.registrations(vec![
        local_entry(&first, "com.example.same", false),
        local_entry(&second, "com.example.same", false),
    ]);
    let report = admit(&fixture.config()).unwrap();
    assert!(report.snapshot.packages.is_empty());
    assert_eq!(
        report.snapshot.diagnostics[0].code,
        PackageErrorCode::DuplicateIdentity
    );
}

#[test]
fn invalid_declared_override_never_falls_back_and_unrelated_package_remains() {
    let fixture = Fixture::new();
    fixture.add_application("same", "com.example.same", "1.0.0");
    fixture.add_application("good", "com.example.good", "1.0.0");
    let missing = fixture.root.path().join("missing-local");
    fixture.registrations(vec![local_entry(&missing, "com.example.same", true)]);
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.good"]);
    let diagnostic = report
        .snapshot
        .diagnostics
        .iter()
        .find(|error| error.code == PackageErrorCode::SourceNotFound)
        .unwrap();
    assert!(
        diagnostic
            .affected_package
            .as_ref()
            .is_some_and(|id| id.as_str() == "com.example.same")
    );
    let repeated = admit(&fixture.config()).unwrap();
    assert_eq!(
        repeated
            .snapshot
            .diagnostics
            .iter()
            .find(|error| error.code == PackageErrorCode::SourceNotFound)
            .unwrap()
            .diagnostic_id,
        diagnostic.diagnostic_id
    );
}

#[test]
fn ui_cache_is_digest_addressed_read_only_and_public_snapshot_is_url_neutral() {
    let fixture = Fixture::new();
    fixture.add_application("one", "com.example.one", "1.0.0");
    let report = admit(&fixture.config()).unwrap();
    let admitted = &report.admissions[0];
    assert_eq!(
        admitted
            .ui
            .cache
            .admitted_root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .len(),
        64
    );
    assert_eq!(
        fs::metadata(&admitted.ui.cache.admitted_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o500
    );
    assert_eq!(
        fs::metadata(admitted.ui.cache.admitted_root.join("ui/index.js"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
    let public = serde_json::to_string(&report.snapshot).unwrap();
    assert!(!public.contains(fixture.application.to_str().unwrap()));
    assert!(!public.contains("tauri"));
    assert!(!public.contains("asset:"));

    let second = admit(&fixture.config()).unwrap();
    assert_eq!(
        second.admissions[0].ui.cache.admitted_root,
        admitted.ui.cache.admitted_root
    );
}

fn declare_ports(root: &Path, provided: Vec<serde_json::Value>, consumed: Vec<serde_json::Value>) {
    let path = root.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    if !provided.is_empty() {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"schemaVersion":1,"$defs":{},"contracts":provided}),
        )
        .unwrap();
        fs::write(root.join("ports.json"), &bytes).unwrap();
        manifest["portContract"] = serde_json::json!({"path":"ports.json","sha256":digest(&bytes)});
    }
    if !consumed.is_empty() {
        manifest["consumedPorts"] = consumed.into();
    }
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

fn provided(identity: &str, minor: u16) -> serde_json::Value {
    serde_json::json!({"identity":identity,"major":1,"minor":minor,
        "description":"Provides fixture actions.","ports":[
            {"name":"fixture.run","kind":"command","description":"Run a fixture action.","request":true,"response":true},
            {"name":"fixture.read","kind":"query","description":"Read a fixture value.","request":true,"response":true}]})
}

fn consumed(
    identity: &str,
    min: u16,
    max: u16,
    required: bool,
    port: &str,
    kind: &str,
) -> serde_json::Value {
    serde_json::json!({"identity":identity,"major":1,"minMinor":min,"maxMinor":max,
        "required":required,"ports":[{"name":port,"kind":kind}]})
}

fn codes(report: &admission::AdmissionReport) -> Vec<(String, PackageErrorCode)> {
    report
        .snapshot
        .diagnostics
        .iter()
        .filter_map(|error| Some((error.affected_package.as_ref()?.to_string(), error.code)))
        .collect()
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn port_graph_is_order_independent_and_keeps_valid_incumbent_on_reopen() {
    for reverse in [false, true] {
        let fixture = Fixture::new();
        let registrations = [
            ("alpha", "com.example.alpha"),
            ("beta", "com.example.beta"),
            ("consumer", "com.example.consumer"),
            ("unrelated", "com.example.unrelated"),
        ];
        for (dir, id) in registrations {
            fixture.add_application(dir, id, "1.0.0");
        }
        for dir in ["alpha", "beta"] {
            declare_ports(
                &fixture.application.join(dir),
                vec![provided("com.example.shared", 2)],
                vec![],
            );
        }
        declare_ports(
            &fixture.application.join("consumer"),
            vec![],
            vec![consumed(
                "com.example.shared",
                1,
                3,
                true,
                "fixture.run",
                "command",
            )],
        );
        if reverse {
            let path = fixture.application.join("application-packages.v1.json");
            let mut index: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            index["packages"].as_array_mut().unwrap().reverse();
            fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
        }
        let first = admit(&fixture.config()).unwrap();
        assert_eq!(
            package_ids(&first),
            vec![
                "com.example.alpha",
                "com.example.consumer",
                "com.example.unrelated"
            ]
        );
        assert!(codes(&first).contains(&(
            "com.example.beta".into(),
            PackageErrorCode::PortProviderConflict
        )));
        assert_eq!(
            first.graph[&"com.example.consumer".parse().unwrap()][0].minor,
            Some(2)
        );
        let second = admit(&fixture.config()).unwrap();
        assert_eq!(package_ids(&second), package_ids(&first));
        // A valid incumbent outranks an otherwise lexically earlier newcomer.
        let path = fixture.application.join("beta/teamarness.package.json");
        let mut broken: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        broken["portContract"]["sha256"] = "0".repeat(64).into();
        fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();
        let third = admit(&fixture.config()).unwrap();
        assert_eq!(package_ids(&third), package_ids(&first));
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn graph_diagnoses_required_dependencies_and_keeps_optional_absence() {
    let fixture = Fixture::new();
    for (dir, id) in [
        ("a", "com.example.a"),
        ("b", "com.example.b"),
        ("c", "com.example.c"),
        ("d", "com.example.d"),
        ("e", "com.example.e"),
        ("f", "com.example.f"),
        ("g", "com.example.g"),
    ] {
        fixture.add_application(dir, id, "1.0.0");
    }
    declare_ports(
        &fixture.application.join("a"),
        vec![],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("b"),
        vec![provided("com.example.bport", 0)],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("c"),
        vec![],
        vec![consumed(
            "com.example.bport",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("d"),
        vec![],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            false,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("e"),
        vec![],
        vec![consumed(
            "com.example.bport",
            1,
            2,
            false,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("f"),
        vec![provided("com.example.fport", 0)],
        vec![consumed(
            "com.example.gport",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("g"),
        vec![provided("com.example.gport", 0)],
        vec![consumed(
            "com.example.fport",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.d", "com.example.e"]);
    for (id, code) in [
        ("a", PackageErrorCode::PortContractMissing),
        ("b", PackageErrorCode::PortContractMissing),
        ("c", PackageErrorCode::PortContractMissing),
        ("f", PackageErrorCode::PortDependencyCycle),
        ("g", PackageErrorCode::PortDependencyCycle),
    ] {
        assert!(
            codes(&report).contains(&(format!("com.example.{id}"), code)),
            "{id}: {:?}",
            codes(&report)
        );
    }
    assert_eq!(
        report.graph[&"com.example.d".parse().unwrap()][0].minor,
        None
    );
    assert_eq!(
        report.graph[&"com.example.e".parse().unwrap()][0].minor,
        None
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn graph_distinguishes_missing_port_kind_and_version_and_replaces_invalid_incumbent() {
    let fixture = Fixture::new();
    for (dir, id) in [
        ("provider", "com.example.provider"),
        ("consumer", "com.example.consumer"),
        ("version", "com.example.version"),
        ("kind", "com.example.kind"),
    ] {
        fixture.add_application(dir, id, "1.0.0");
    }
    declare_ports(
        &fixture.application.join("provider"),
        vec![provided("com.example.shared", 2)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("consumer"),
        vec![],
        vec![consumed(
            "com.example.shared",
            0,
            3,
            true,
            "fixture.absent",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("version"),
        vec![],
        vec![consumed(
            "com.example.shared",
            3,
            4,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("kind"),
        vec![],
        vec![consumed(
            "com.example.shared",
            0,
            3,
            true,
            "fixture.run",
            "event",
        )],
    );
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.provider"]);
    assert!(
        codes(&report).contains(&("com.example.consumer".into(), PackageErrorCode::PortMissing))
    );
    assert!(codes(&report).contains(&(
        "com.example.version".into(),
        PackageErrorCode::PortVersionMismatch
    )));
    assert!(codes(&report).contains(&("com.example.kind".into(), PackageErrorCode::PortMissing)));
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn incumbent_survives_earlier_newcomer_and_invalid_incumbent_is_replaced() {
    let fixture = Fixture::new();
    fixture.add_application("beta", "com.example.beta", "1.0.0");
    declare_ports(
        &fixture.application.join("beta"),
        vec![provided("com.example.shared", 1)],
        vec![],
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.beta"]
    );
    fixture.add_application("alpha", "com.example.alpha", "1.0.0");
    declare_ports(
        &fixture.application.join("alpha"),
        vec![provided("com.example.shared", 2)],
        vec![],
    );
    let reopened = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&reopened), vec!["com.example.beta"]);
    assert!(codes(&reopened).contains(&(
        "com.example.alpha".into(),
        PackageErrorCode::PortProviderConflict
    )));
    let path = fixture.application.join("beta/teamarness.package.json");
    let mut broken: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    broken["portContract"]["sha256"] = "0".repeat(64).into();
    fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();
    let replaced = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&replaced), vec!["com.example.alpha"]);
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.alpha"]
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn dependency_invalid_incumbent_yields_to_a_viable_provider_on_reopen() {
    let fixture = Fixture::new();
    fixture.add_application("beta", "com.example.beta", "1.0.0");
    declare_ports(
        &fixture.application.join("beta"),
        vec![provided("com.example.shared", 1)],
        vec![],
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.beta"]
    );
    fixture.add_application("alpha", "com.example.alpha", "1.0.0");
    fixture.add_application("consumer", "com.example.consumer", "1.0.0");
    declare_ports(
        &fixture.application.join("alpha"),
        vec![provided("com.example.shared", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("consumer"),
        vec![],
        vec![consumed(
            "com.example.shared",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("beta"),
        vec![provided("com.example.shared", 1)],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    let reopened = admit(&fixture.config()).unwrap();
    assert_eq!(
        package_ids(&reopened),
        vec!["com.example.alpha", "com.example.consumer"]
    );
    assert!(codes(&reopened).contains(&(
        "com.example.beta".into(),
        PackageErrorCode::PortContractMissing
    )));
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.alpha", "com.example.consumer"]
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn rival_breaks_provisional_cycle_without_discarding_dependent_provider() {
    let fixture = Fixture::new();
    fixture.add_application("alpha", "com.example.alpha", "1.0.0");
    declare_ports(
        &fixture.application.join("alpha"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.alpha"]
    );
    fixture.add_application("beta", "com.example.beta", "1.0.0");
    fixture.add_application("delta", "com.example.delta", "1.0.0");
    fixture.add_application("consumer", "com.example.consumer", "1.0.0");
    declare_ports(
        &fixture.application.join("alpha"),
        vec![provided("com.example.x", 1)],
        vec![consumed(
            "com.example.y",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("beta"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("delta"),
        vec![provided("com.example.y", 1)],
        vec![consumed(
            "com.example.x",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("consumer"),
        vec![],
        vec![consumed(
            "com.example.y",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    let reopened = admit(&fixture.config()).unwrap();
    assert_eq!(
        package_ids(&reopened),
        vec![
            "com.example.beta",
            "com.example.consumer",
            "com.example.delta"
        ]
    );
    assert_eq!(
        reopened.graph[&"com.example.delta".parse().unwrap()][0].provider,
        Some("com.example.beta".parse().unwrap())
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec![
            "com.example.beta",
            "com.example.consumer",
            "com.example.delta"
        ]
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn alternative_dependency_preserves_feasible_incumbent_chain() {
    for reverse in [false, true] {
        let fixture = Fixture::new();
        for (dir, id) in [("a", "a"), ("c", "c"), ("d", "d")] {
            fixture.add_application(dir, &format!("com.example.{id}"), "1.0.0");
        }
        declare_ports(
            &fixture.application.join("a"),
            vec![provided("com.example.x", 1)],
            vec![consumed(
                "com.example.y",
                0,
                1,
                true,
                "fixture.run",
                "command",
            )],
        );
        declare_ports(
            &fixture.application.join("c"),
            vec![provided("com.example.y", 1)],
            vec![consumed(
                "com.example.z",
                0,
                1,
                true,
                "fixture.run",
                "command",
            )],
        );
        declare_ports(
            &fixture.application.join("d"),
            vec![provided("com.example.z", 1)],
            vec![],
        );
        assert_eq!(
            package_ids(&admit(&fixture.config()).unwrap()),
            vec!["com.example.a", "com.example.c", "com.example.d"]
        );
        fixture.add_application("b", "com.example.b", "1.0.0");
        fixture.add_application("e", "com.example.e", "1.0.0");
        declare_ports(
            &fixture.application.join("b"),
            vec![provided("com.example.x", 1)],
            vec![],
        );
        declare_ports(
            &fixture.application.join("e"),
            vec![provided("com.example.z", 1)],
            vec![],
        );
        declare_ports(
            &fixture.application.join("d"),
            vec![provided("com.example.z", 1)],
            vec![consumed(
                "com.example.missing",
                0,
                1,
                true,
                "fixture.run",
                "command",
            )],
        );
        if reverse {
            let path = fixture.application.join("application-packages.v1.json");
            let mut index: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            index["packages"].as_array_mut().unwrap().reverse();
            fs::write(path, serde_json::to_vec(&index).unwrap()).unwrap();
        }
        for _ in 0..2 {
            let report = admit(&fixture.config()).unwrap();
            assert_eq!(
                package_ids(&report),
                vec!["com.example.a", "com.example.c", "com.example.e"]
            );
            assert!(codes(&report).contains(&(
                "com.example.d".into(),
                PackageErrorCode::PortContractMissing
            )));
        }
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn two_contested_winners_choose_graph_with_sticky_incumbent() {
    let fixture = Fixture::new();
    fixture.add_application("a", "com.example.a", "1.0.0");
    fixture.add_application("c", "com.example.c", "1.0.0");
    declare_ports(
        &fixture.application.join("a"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("c"),
        vec![provided("com.example.y", 1)],
        vec![],
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.a", "com.example.c"]
    );
    fixture.add_application("b", "com.example.b", "1.0.0");
    fixture.add_application("d", "com.example.d", "1.0.0");
    declare_ports(
        &fixture.application.join("a"),
        vec![provided("com.example.x", 1)],
        vec![consumed(
            "com.example.y",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("c"),
        vec![provided("com.example.y", 1)],
        vec![consumed(
            "com.example.x",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("b"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("d"),
        vec![provided("com.example.y", 1)],
        vec![],
    );
    for _ in 0..2 {
        let report = admit(&fixture.config()).unwrap();
        assert_eq!(package_ids(&report), vec!["com.example.a", "com.example.d"]);
        assert_eq!(
            report.graph[&"com.example.a".parse().unwrap()][0].provider,
            Some("com.example.d".parse().unwrap())
        );
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn required_edge_connects_components_in_both_directions() {
    let fixture = Fixture::new();
    for id in ["a", "b", "c"] {
        fixture.add_application(id, &format!("com.example.{id}"), "1.0.0");
    }
    declare_ports(
        &fixture.application.join("a"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("b"),
        vec![provided("com.example.y", 1)],
        vec![consumed(
            "com.example.x",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    declare_ports(
        &fixture.application.join("c"),
        vec![provided("com.example.y", 1)],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.a", "com.example.b"]);
    assert!(codes(&report).contains(&(
        "com.example.c".into(),
        PackageErrorCode::PortContractMissing
    )));
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn multi_contract_claim_is_rejected_as_one_package_when_invalid() {
    let fixture = Fixture::new();
    for id in ["a", "b", "c"] {
        fixture.add_application(id, &format!("com.example.{id}"), "1.0.0");
    }
    declare_ports(
        &fixture.application.join("a"),
        vec![provided("com.example.x", 1), provided("com.example.y", 1)],
        vec![],
    );
    assert_eq!(
        package_ids(&admit(&fixture.config()).unwrap()),
        vec!["com.example.a", "com.example.b", "com.example.c"]
    );
    declare_ports(
        &fixture.application.join("b"),
        vec![provided("com.example.x", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("c"),
        vec![provided("com.example.y", 1)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("a"),
        vec![provided("com.example.x", 1), provided("com.example.y", 1)],
        vec![consumed(
            "com.example.missing",
            0,
            1,
            true,
            "fixture.run",
            "command",
        )],
    );
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.b", "com.example.c"]);
    assert!(codes(&report).contains(&(
        "com.example.a".into(),
        PackageErrorCode::PortContractMissing
    )));
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn graph_search_limit_isolates_contested_component_and_preserves_metadata() {
    let fixture = Fixture::new();
    fixture.add_application("unrelated", "com.example.unrelated", "1.0.0");
    for index in 0..12 {
        for rival in ["a", "b"] {
            let id = format!("com.example.{rival}{index:02}");
            let directory = format!("{rival}{index:02}");
            fixture.add_application(&directory, &id, "1.0.0");
            declare_ports(
                &fixture.application.join(&directory),
                vec![provided(&format!("com.example.port{index:02}"), 1)],
                vec![consumed(
                    &format!("com.example.port{:02}", (index + 1) % 12),
                    0,
                    1,
                    true,
                    "fixture.run",
                    "command",
                )],
            );
        }
    }
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(package_ids(&report), vec!["com.example.unrelated"]);
    assert_eq!(
        codes(&report)
            .iter()
            .filter(|(_, code)| *code == PackageErrorCode::PortGraphSearchLimit)
            .count(),
        24
    );
    let store = host_store::HostStore::open(fixture.config().private_data_directory()).unwrap();
    assert!(store.port_providers().unwrap().is_empty());
    let second = admit(&fixture.config()).unwrap();
    assert_eq!(codes(&report), codes(&second));
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn additive_provider_minor_negotiates_highest_minor_in_consumers_range() {
    let fixture = Fixture::new();
    fixture.add_application("provider", "com.example.provider", "1.0.0");
    fixture.add_application("consumer", "com.example.consumer", "1.0.0");
    declare_ports(
        &fixture.application.join("provider"),
        vec![provided("com.example.shared", 3)],
        vec![],
    );
    declare_ports(
        &fixture.application.join("consumer"),
        vec![],
        vec![consumed(
            "com.example.shared",
            0,
            1,
            true,
            "fixture.read",
            "query",
        )],
    );
    let report = admit(&fixture.config()).unwrap();
    assert_eq!(
        package_ids(&report),
        vec!["com.example.consumer", "com.example.provider"]
    );
    assert_eq!(
        report.graph[&"com.example.consumer".parse().unwrap()][0].minor,
        Some(1)
    );
    assert!(report.snapshot.diagnostics.is_empty());
}
