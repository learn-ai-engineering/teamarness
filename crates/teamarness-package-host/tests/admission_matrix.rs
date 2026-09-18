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

#[path = "../src/admission.rs"]
mod admission;
#[path = "../src/catalog.rs"]
mod catalog;
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
