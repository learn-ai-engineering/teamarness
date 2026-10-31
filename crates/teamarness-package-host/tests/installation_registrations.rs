//! The installation registration store: inspect, write, and remove entries of
//! `registrations.v1.json` without hand-editing the file (ADR 0036).

#![cfg(unix)]
#![allow(dead_code)]

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub use teamarness_package_host::{
    InspectedPackageKind, InstallationPublicIdentity, PackageErrorCode, PackageErrorEnvelope,
    PackageHost, PackageHostCommand, PackageHostConfig, PackageIdentity, PackageTier,
    RegistrationRequest, RegistrationStatus, ShutdownHost, inspect_package_directory,
    remove_registration, write_registration,
};

// The Host's own reader, so a written file is checked by exactly the code that admits it.
#[path = "../src/catalog.rs"]
mod catalog;
#[path = "../src/manifest.rs"]
mod manifest;

const RISKS: [&str; 3] = ["filesystem", "network", "process"];

struct Store {
    root: TempDir,
    application: PathBuf,
    private: PathBuf,
}

impl Store {
    /// An application with one bundled Package and an empty private data directory.
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let application = root.path().join("application");
        let private = root.path().join("private");
        fs::create_dir_all(&application).unwrap();
        fs::create_dir_all(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        common::write_node_package(&application.join("team"), "com.example.team");
        fs::write(
            application.join("application-packages.v1.json"),
            br#"{"schemaVersion":1,"packages":[{"directory":"team"}]}"#,
        )
        .unwrap();
        Self {
            root,
            application,
            private,
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

    /// Writes a valid Package Directory outside the application and returns its path.
    fn package(&self, name: &str, identity: &str) -> PathBuf {
        let path = self.root.path().join("sources").join(name);
        common::write_node_package(&path, identity);
        path
    }

    fn file(&self) -> PathBuf {
        self.private.join("packages/registrations.v1.json")
    }

    fn bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.file()).ok()
    }

    fn hand_write(&self, value: &serde_json::Value) {
        fs::create_dir_all(self.private.join("packages")).unwrap();
        fs::write(self.file(), serde_json::to_vec(value).unwrap()).unwrap();
        fs::set_permissions(self.file(), fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn entries(&self) -> Vec<serde_json::Value> {
        let value: serde_json::Value = serde_json::from_slice(&self.bytes().unwrap()).unwrap();
        value["registrations"].as_array().unwrap().clone()
    }
}

fn request(directory: &Path, identity: &str) -> RegistrationRequest {
    RegistrationRequest {
        directory: directory.to_owned(),
        expected_identity: identity.parse().unwrap(),
        accepted_risks: RISKS.map(str::to_owned).to_vec(),
        override_application: false,
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, PackageErrorEnvelope>) -> PackageErrorCode {
    result.unwrap_err().code
}

fn edit_manifest(root: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = root.join("teamarness.package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    edit(&mut manifest);
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn inspect_reports_what_registering_the_directory_would_do() {
    let store = Store::new();
    let config = store.config();
    let first = store.package("first", "com.example.personal");
    edit_manifest(&first, |manifest| {
        manifest["presentation"]["description"] = "A Personal fixture.".into();
        manifest["presentation"]["author"] = "Fixture Author".into();
    });

    let new = inspect_package_directory(&config, &first).unwrap();
    assert_eq!(new.kind, InspectedPackageKind::New);
    assert_eq!(new.identity.as_str(), "com.example.personal");
    assert_eq!(new.directory, fs::canonicalize(&first).unwrap());
    assert_eq!(new.version, "1.0.0");
    assert_eq!(
        new.presentation.description.as_deref(),
        Some("A Personal fixture.")
    );
    assert_eq!(new.presentation.author.as_deref(), Some("Fixture Author"));
    assert_eq!(new.presentation.homepage, None);
    assert_eq!(new.capabilities[0].identity, "teamarness.workspace-context");
    assert!(new.provided.is_empty() && new.consumed.is_empty());

    write_registration(&config, request(&first, "com.example.personal")).unwrap();
    assert_eq!(
        inspect_package_directory(&config, &first).unwrap().kind,
        InspectedPackageKind::Reload
    );
    let second = store.package("second", "com.example.personal");
    assert_eq!(
        inspect_package_directory(&config, &second).unwrap().kind,
        InspectedPackageKind::ReplacesRegistration
    );
    let team = store.package("team-build", "com.example.team");
    assert_eq!(
        inspect_package_directory(&config, &team).unwrap().kind,
        InspectedPackageKind::OverridesTeam
    );
    // Inspecting never writes Host state.
    assert!(!store.private.join("packages/ui-cache").exists());
}

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn inspect_refuses_an_unusable_directory_with_its_rule() {
    let store = Store::new();
    let config = store.config();
    let valid = store.package("valid", "com.example.valid");
    let link = store.root.path().join("link");
    symlink(&valid, &link).unwrap();
    let file = store.root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    let malformed = store.root.path().join("malformed");
    fs::create_dir_all(&malformed).unwrap();
    fs::write(malformed.join("teamarness.package.json"), b"{").unwrap();
    let tampered = store.package("tampered", "com.example.tampered");
    fs::write(tampered.join("backend/package.mjs"), b"tampered").unwrap();

    for (path, expected) in [
        (
            store.root.path().join("missing"),
            PackageErrorCode::SourceNotFound,
        ),
        (link, PackageErrorCode::InvalidRegistration),
        (
            PathBuf::from("sources/valid"),
            PackageErrorCode::InvalidRegistration,
        ),
        (file, PackageErrorCode::InvalidRegistration),
        (malformed, PackageErrorCode::ManifestMalformed),
        (tampered, PackageErrorCode::ArtifactChanged),
    ] {
        assert_eq!(
            code(inspect_package_directory(&config, &path)),
            expected,
            "{}",
            path.display()
        );
        assert_eq!(
            code(write_registration(
                &config,
                request(&path, "com.example.valid")
            )),
            expected,
            "{}",
            path.display()
        );
        assert_eq!(store.bytes(), None, "{}", path.display());
    }
}

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn written_file_meets_the_file_policy_and_the_host_reader_accepts_it() {
    let store = Store::new();
    let config = store.config();
    assert!(!store.private.join("packages").exists());
    let personal = store.package("personal", "com.example.personal");

    write_registration(&config, request(&personal, "com.example.personal")).unwrap();

    let parent = fs::metadata(store.private.join("packages")).unwrap();
    assert_eq!(parent.permissions().mode() & 0o777, 0o700);
    let file = fs::symlink_metadata(store.file()).unwrap();
    assert!(file.is_file());
    assert_eq!(file.permissions().mode() & 0o777, 0o600);
    let registrations = catalog::installation_registrations(&config).unwrap();
    assert_eq!(registrations.len(), 1);
    assert_eq!(
        registrations[0].directory,
        fs::canonicalize(&personal).unwrap()
    );
    assert_eq!(registrations[0].expected_identity, "com.example.personal");
    assert!(registrations[0].trust_complete);
    assert!(!registrations[0].override_application);
    assert!(registrations[0].path_error.is_none());
    // No temporary file is left behind, and the preflight populated no UI cache.
    let names: Vec<_> = fs::read_dir(store.private.join("packages"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.contains("registrations"))
        .collect();
    assert_eq!(names, vec!["registrations.v1.json"]);
    assert!(!store.private.join("packages/ui-cache").exists());
}

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn every_refused_write_leaves_the_file_as_it_was() {
    let store = Store::new();
    let config = store.config();
    let personal = store.package("personal", "com.example.personal");
    let team = store.package("team-build", "com.example.team");
    let unmet = store.package("unmet", "com.example.unmet");
    edit_manifest(&unmet, |manifest| {
        manifest["consumedPorts"] = serde_json::json!([{
            "identity":"com.example.absent", "major":1, "minMinor":0, "maxMinor":0,
            "required":true, "ports":[{"name":"absent.run","kind":"command"}]
        }]);
    });
    let refusals = |existing: &str| {
        let with_risks = |risks: &[&str]| RegistrationRequest {
            accepted_risks: risks.iter().map(|risk| (*risk).to_owned()).collect(),
            ..request(&personal, "com.example.personal")
        };
        vec![
            (
                format!("{existing}: two risks"),
                with_risks(&["filesystem", "network"]),
                PackageErrorCode::TrustEvidenceMissing,
            ),
            (
                format!("{existing}: a repeated risk"),
                with_risks(&["filesystem", "network", "process", "process"]),
                PackageErrorCode::TrustEvidenceMissing,
            ),
            (
                format!("{existing}: an unknown risk"),
                with_risks(&["filesystem", "network", "camera"]),
                PackageErrorCode::TrustEvidenceMissing,
            ),
            (
                format!("{existing}: another identity"),
                request(&personal, "com.example.other"),
                PackageErrorCode::IdentityMismatch,
            ),
            (
                format!("{existing}: a Team collision without override"),
                request(&team, "com.example.team"),
                PackageErrorCode::OverrideRequired,
            ),
            (
                format!("{existing}: a preflight refusal"),
                request(&unmet, "com.example.unmet"),
                PackageErrorCode::PortContractMissing,
            ),
        ]
    };

    for (name, request, expected) in refusals("absent") {
        assert_eq!(
            code(write_registration(&config, request)),
            expected,
            "{name}"
        );
        assert_eq!(store.bytes(), None, "{name}");
    }

    let earlier = store.package("earlier", "com.example.earlier");
    write_registration(&config, request(&earlier, "com.example.earlier")).unwrap();
    let before = store.bytes().unwrap();
    for (name, request, expected) in refusals("present") {
        assert_eq!(
            code(write_registration(&config, request)),
            expected,
            "{name}"
        );
        assert_eq!(store.bytes().unwrap(), before, "{name}");
    }
    assert_eq!(
        code(remove_registration(
            &config,
            &"com.example.personal".parse().unwrap()
        )),
        PackageErrorCode::NotRegistered
    );
    assert_eq!(store.bytes().unwrap(), before);

    // The confirmed override is written, as a Team-tier registration.
    write_registration(
        &config,
        RegistrationRequest {
            override_application: true,
            ..request(&team, "com.example.team")
        },
    )
    .unwrap();
    let entries = store.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1]["expectedIdentity"], "com.example.team");
    assert_eq!(entries[1]["overrideApplication"], true);
}

type Corruption<'a> = Box<dyn Fn(&Store) + 'a>;

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn an_invalid_existing_file_is_never_overwritten() {
    let store = Store::new();
    let config = store.config();
    let personal = store.package("personal", "com.example.personal");
    let identity: PackageIdentity = "com.example.personal".parse().unwrap();
    let readable = serde_json::json!({"schemaVersion":1,"registrations":[]});
    let cases: Vec<(&str, Corruption<'_>)> = vec![
        (
            "group-readable",
            Box::new(|store| {
                store.hand_write(&readable);
                fs::set_permissions(store.file(), fs::Permissions::from_mode(0o644)).unwrap();
            }),
        ),
        (
            "malformed",
            Box::new(|store| {
                store.hand_write(&readable);
                fs::write(store.file(), b"{\"schemaVersion\":1,").unwrap();
            }),
        ),
        (
            "unknown field",
            Box::new(|store| {
                store.hand_write(&serde_json::json!({
                    "schemaVersion":1,"registrations":[],"comment":"mine"
                }));
            }),
        ),
        (
            "unsupported version",
            Box::new(|store| {
                store.hand_write(&serde_json::json!({"schemaVersion":2,"registrations":[]}));
            }),
        ),
        (
            "symlink",
            Box::new(|store| {
                let target = store.root.path().join("elsewhere.json");
                fs::write(&target, serde_json::to_vec(&readable).unwrap()).unwrap();
                fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
                fs::create_dir_all(store.private.join("packages")).unwrap();
                let _ = fs::remove_file(store.file());
                symlink(target, store.file()).unwrap();
            }),
        ),
    ];
    for (name, corrupt) in cases {
        corrupt(&store);
        let before = fs::read(store.file()).unwrap();
        let mode = fs::symlink_metadata(store.file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            code(write_registration(
                &config,
                request(&personal, "com.example.personal")
            )),
            PackageErrorCode::RegistrationFileInvalid,
            "{name}"
        );
        assert_eq!(
            code(remove_registration(&config, &identity)),
            PackageErrorCode::RegistrationFileInvalid,
            "{name}"
        );
        assert_eq!(
            code(inspect_package_directory(&config, &personal)),
            PackageErrorCode::RegistrationFileInvalid,
            "{name}"
        );
        assert_eq!(fs::read(store.file()).unwrap(), before, "{name}");
        assert_eq!(
            fs::symlink_metadata(store.file())
                .unwrap()
                .permissions()
                .mode(),
            mode,
            "{name}"
        );
        fs::remove_file(store.file()).unwrap();
    }
}

/// Verifies: TEAM.SYS.PKG-R09
#[test]
fn a_hand_written_file_keeps_its_other_entries_through_write_and_remove() {
    let store = Store::new();
    let config = store.config();
    let kept = store.package("kept", "com.example.kept");
    let untrusted = store.package("untrusted", "com.example.untrusted");
    let added = store.package("added", "com.example.added");
    // Hand-written: one complete entry with no `overrideApplication`, one with no trust.
    let first = serde_json::json!({
        "directory": kept, "expectedIdentity": "com.example.kept",
        "trust": {"kind":"trusted_native_code_v1","acceptedRisks":["process","network","filesystem"]}
    });
    let second = serde_json::json!({
        "directory": untrusted, "expectedIdentity": "com.example.untrusted"
    });
    store.hand_write(&serde_json::json!({
        "schemaVersion": 1, "registrations": [first.clone(), second.clone()]
    }));
    let normalized = |mut entry: serde_json::Value| {
        if entry.get("overrideApplication").is_none() {
            entry["overrideApplication"] = false.into();
        }
        entry
    };

    write_registration(&config, request(&added, "com.example.added")).unwrap();

    let entries = store.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], normalized(first.clone()));
    assert_eq!(entries[1], normalized(second.clone()));
    assert_eq!(entries[2]["expectedIdentity"], "com.example.added");
    let registrations = catalog::installation_registrations(&config).unwrap();
    assert_eq!(registrations.len(), 3);
    assert!(registrations[0].trust_complete);
    assert!(!registrations[1].trust_complete);

    // A rebuilt directory for a registered identity replaces only that entry.
    let moved = store.package("moved", "com.example.added");
    write_registration(&config, request(&moved, "com.example.added")).unwrap();
    let entries = store.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[2]["directory"],
        fs::canonicalize(&moved).unwrap().to_str().unwrap()
    );

    remove_registration(&config, &"com.example.added".parse().unwrap()).unwrap();
    assert_eq!(
        store.entries(),
        vec![normalized(first.clone()), normalized(second)]
    );
    remove_registration(&config, &"com.example.untrusted".parse().unwrap()).unwrap();
    assert_eq!(store.entries(), vec![normalized(first)]);
    // Removing a registration never touches a Package Data Directory or the sources.
    assert!(kept.join("teamarness.package.json").exists());
    assert!(untrusted.join("teamarness.package.json").exists());
    let mode = fs::metadata(store.file()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Verifies: TEAM.SYS.PKG-R09
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_opened_host_lists_registrations_with_tier_and_status() {
    let fixture = common::Fixture::node(&[("team", "com.example.team")]).await;
    let sources = tempfile::tempdir().unwrap();
    let personal = sources.path().join("personal");
    common::write_node_package(&personal, "com.example.personal");
    write_registration(&fixture.config, request(&personal, "com.example.personal")).unwrap();
    // A broken rebuild after registration is refused at open, and keeps its entry.
    let broken = sources.path().join("broken");
    common::write_node_package(&broken, "com.example.broken");
    write_registration(&fixture.config, request(&broken, "com.example.broken")).unwrap();
    fs::write(broken.join("backend/package.mjs"), b"tampered").unwrap();

    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();

    let personal_directory = fs::canonicalize(&personal).unwrap();
    let mut registrations = snapshot.registrations.clone();
    registrations.sort_by(|left, right| left.expected_identity.cmp(&right.expected_identity));
    assert_eq!(registrations.len(), 2);
    assert_eq!(registrations[0].expected_identity, "com.example.broken");
    assert_eq!(registrations[0].tier, PackageTier::Personal);
    assert_eq!(
        registrations[0].status,
        RegistrationStatus::Refused {
            code: PackageErrorCode::ArtifactChanged
        }
    );
    assert_eq!(registrations[1].expected_identity, "com.example.personal");
    assert_eq!(
        registrations[1].directory,
        personal_directory.to_str().unwrap()
    );
    assert!(!registrations[1].override_application);
    assert_eq!(registrations[1].status, RegistrationStatus::Admitted);
    assert!(
        snapshot
            .instances
            .iter()
            .any(|instance| instance.key.package_identity.as_str() == "com.example.personal")
    );
    let json = serde_json::to_value(&registrations[0]).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "directory": fs::canonicalize(&broken).unwrap(),
            "expectedIdentity": "com.example.broken",
            "override": false,
            "tier": "personal",
            "status": {"kind": "refused", "code": "artifact_changed"}
        })
    );
    fixture.core.shutdown().await.unwrap();
}
