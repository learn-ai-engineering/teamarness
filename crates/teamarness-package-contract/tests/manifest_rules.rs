//! Manifest rules, pinned by code and rule name, plus the Package Directory path.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use teamarness_package_contract::manifest::{load_package_directory, parse_manifest, sha256};
use teamarness_package_contract::{ContractError, ContractErrorCode};

const TARGET: &str = "aarch64-apple-darwin";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest(patch: Value) -> Value {
    let mut base = json!({
        "schemaVersion": 1,
        "identity": "dev.example.fixture",
        "version": "1.2.3",
        "presentation": {"displayName": "Fixture"},
        "protocol": {"major": 1, "minMinor": 0, "maxMinor": 0},
        "backends": [{
            "target": TARGET,
            "path": "backend/package",
            "size": 7,
            "sha256": "0".repeat(64),
        }],
        "ui": {
            "entrypoint": {"path": "ui/index.js", "size": 7, "sha256": "0".repeat(64)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": "0".repeat(64)},
        },
        "capabilities": [],
    });
    let (Value::Object(base_object), Value::Object(patch)) = (&mut base, patch) else {
        return base;
    };
    for (key, value) in patch {
        base_object.insert(key, value);
    }
    base
}

fn reject(patch: Value) -> ContractError {
    parse_manifest(&serde_json::to_vec(&manifest(patch)).unwrap()).unwrap_err()
}

#[test]
fn accepts_bundled_package_manifests() {
    let packages = repository_root().join("packages");
    let mut seen = 0;
    for entry in fs::read_dir(&packages).unwrap() {
        let path = entry.unwrap().path().join("teamarness.package.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        let document = parse_manifest(&bytes)
            .unwrap_or_else(|error| panic!("{} rejected: {error}", path.display()));
        assert!(
            document.backend_path(TARGET).is_some(),
            "{}",
            path.display()
        );
        seen += 1;
    }
    assert_eq!(seen, 6, "expected six bundled Package manifests");
}

/// The schema's `maxLength` counts code points, so the manifest rule does too.
///
/// Counting bytes rejected a display name the schema admits: 128 characters
/// outside ASCII are more than 128 bytes.
#[test]
fn a_display_name_is_bounded_in_code_points_as_the_schema_publishes_it() {
    let longest = "é".repeat(128);
    assert_eq!(longest.chars().count(), 128);
    assert_eq!(longest.len(), 256);
    let document = parse_manifest(
        &serde_json::to_vec(&manifest(
            json!({"presentation": {"displayName": longest.clone()}}),
        ))
        .unwrap(),
    )
    .expect("the schema admits 128 code points");
    assert_eq!(document.display_name, longest);

    let error = reject(json!({"presentation": {"displayName": "é".repeat(129)}}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.display_name");
}

#[test]
fn accepts_placeholder_sizes_and_digests_in_a_source_tree() {
    let document = parse_manifest(
        &serde_json::to_vec(&manifest(json!({
            "backends": [{
                "target": TARGET,
                "path": "backend/package",
                "size": 0,
                "sha256": "0".repeat(64),
            }],
        })))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(document.backend_path(TARGET), Some("backend/package"));
}

#[test]
fn rejects_non_strict_versions() {
    for version in ["1.2", "1.02.3", "v1.2.3", "1.2.3+", ""] {
        let error = reject(json!({"version": version}));
        assert_eq!(
            error.code,
            ContractErrorCode::ManifestUnsupported,
            "{version}"
        );
        assert_eq!(error.rule, "manifest.version_not_strict", "{version}");
    }
}

#[test]
fn rejects_unsupported_protocol_ranges() {
    for protocol in [
        json!({"major": 2, "minMinor": 0, "maxMinor": 0}),
        json!({"major": 1, "minMinor": 1, "maxMinor": 2}),
        json!({"major": 0, "minMinor": 0, "maxMinor": 0}),
        json!({"major": 1, "minMinor": 2, "maxMinor": 1}),
    ] {
        let error = reject(json!({"protocol": protocol.clone()}));
        assert_eq!(
            error.code,
            ContractErrorCode::ManifestUnsupported,
            "{protocol}"
        );
        assert_eq!(error.rule, "manifest.protocol_unsupported", "{protocol}");
        assert_eq!(error.pointer.as_deref(), Some("/protocol"));
    }
}

#[test]
fn rejects_unnormalized_paths() {
    for path in [
        "/absolute",
        "../escape",
        "ui/./index.js",
        "ui\\index.js",
        "ui//index.js",
        "ui/",
        "",
    ] {
        let error = reject(json!({"ui": {
            "entrypoint": {"path": path, "size": 7, "sha256": "0".repeat(64)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": "0".repeat(64)},
        }}));
        assert_eq!(error.code, ContractErrorCode::ManifestMalformed, "{path}");
        assert_eq!(error.rule, "manifest.path_not_normalized", "{path}");
    }
}

#[test]
fn rejects_invalid_extension_keys() {
    let error = reject(json!({"extensions": {"not-reverse-dns": {"a": 1}}}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.extension_key");
}

#[test]
fn rejects_duplicate_targets_and_paths() {
    let artifact = json!({
        "target": TARGET, "path": "backend/package", "size": 7, "sha256": "0".repeat(64),
    });
    let error = reject(json!({"backends": [artifact.clone(), artifact]}));
    assert_eq!(error.code, ContractErrorCode::TargetUnsupported);
    assert_eq!(error.rule, "manifest.duplicate_target");

    let error = reject(json!({"backends": [
        {"target": TARGET, "path": "backend/package", "size": 7, "sha256": "0".repeat(64)},
        {"target": "x86_64-apple-darwin", "path": "backend/package", "size": 7, "sha256": "0".repeat(64)},
    ]}));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.duplicate_path");
}

fn write_directory(root: &Path, backend: &[u8], entrypoint: &[u8], contract: &[u8]) {
    fs::create_dir_all(root.join("backend")).unwrap();
    fs::create_dir_all(root.join("ui")).unwrap();
    fs::write(root.join("backend/package"), backend).unwrap();
    fs::write(root.join("ui/index.js"), entrypoint).unwrap();
    fs::write(root.join("ui/contract.json"), contract).unwrap();
    let document = manifest(json!({
        "backends": [{
            "target": TARGET, "path": "backend/package",
            "size": backend.len(), "sha256": sha256(backend),
        }],
        "ui": {
            "entrypoint": {"path": "ui/index.js", "size": entrypoint.len(), "sha256": sha256(entrypoint)},
            "assets": [],
            "contract": {"path": "ui/contract.json", "sha256": sha256(contract)},
        },
    }));
    fs::write(
        root.join("teamarness.package.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
}

#[test]
fn loads_a_package_directory_and_reports_a_digest_mismatch() {
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    let temp = TempDir::new().unwrap();
    write_directory(temp.path(), b"backend", b"export {};", contract);
    let loaded = load_package_directory(temp.path(), TARGET).unwrap();
    assert_eq!(loaded.identity, "dev.example.fixture");
    assert_eq!(loaded.backend.relative_path, "backend/package");

    fs::write(temp.path().join("backend/package"), b"changed").unwrap();
    let error = load_package_directory(temp.path(), TARGET).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::ArtifactChanged);
    assert_eq!(error.rule, "manifest.digest_mismatch");
    assert_eq!(error.pointer.as_deref(), Some("backend/package"));
}

#[test]
fn reports_a_missing_target_and_a_missing_artifact() {
    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    let temp = TempDir::new().unwrap();
    write_directory(temp.path(), b"backend", b"export {};", contract);

    let error = load_package_directory(temp.path(), "riscv64-unknown-linux-gnu").unwrap_err();
    assert_eq!(error.code, ContractErrorCode::TargetUnsupported);
    assert_eq!(error.rule, "manifest.target_missing");

    fs::remove_file(temp.path().join("backend/package")).unwrap();
    let error = load_package_directory(temp.path(), TARGET).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::ArtifactMissing);
    assert_eq!(error.rule, "manifest.artifact_missing");
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn activation_policy_is_closed_and_legacy_default_is_eager() {
    use teamarness_package_contract::manifest::ActivationPolicy;
    for (patch, policy) in [
        (json!({}), ActivationPolicy::Eager),
        (json!({"activation":"eager"}), ActivationPolicy::Eager),
        (
            json!({"activation":"on_demand"}),
            ActivationPolicy::OnDemand,
        ),
    ] {
        let document = parse_manifest(&serde_json::to_vec(&manifest(patch)).unwrap()).unwrap();
        assert_eq!(document.activation, policy);
    }
    for value in [json!(null), json!("lazy"), json!(true), json!("OnDemand")] {
        assert_eq!(
            reject(json!({"activation":value})).code,
            ContractErrorCode::ManifestMalformed
        );
    }
}

fn presentation(extra: Value) -> Value {
    let mut value = json!({"displayName": "Fixture"});
    let (Value::Object(base), Value::Object(extra)) = (&mut value, extra) else {
        return value;
    };
    base.extend(extra);
    json!({"presentation": value})
}

fn accept(patch: Value) -> teamarness_package_contract::manifest::ManifestDocument {
    parse_manifest(&serde_json::to_vec(&manifest(patch)).unwrap())
        .unwrap_or_else(|error| panic!("rejected: {error}"))
}

fn assert_refused(patch: Value, rule: &str, pointer: &str) {
    let error = reject(patch);
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, rule);
    assert_eq!(error.pointer.as_deref(), Some(pointer));
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn presentation_metadata_is_optional() {
    let document = accept(json!({}));
    assert_eq!(document.description, None);
    assert_eq!(document.author, None);
    assert_eq!(document.homepage, None);

    let document = accept(presentation(json!({
        "description": "Reads and writes fixtures.",
        "author": "Example Author",
        "homepage": "https://example.com/fixture",
    })));
    assert_eq!(
        document.description.as_deref(),
        Some("Reads and writes fixtures.")
    );
    assert_eq!(document.author.as_deref(), Some("Example Author"));
    assert_eq!(
        document.homepage.as_deref(),
        Some("https://example.com/fixture")
    );
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn an_unknown_presentation_key_is_still_refused() {
    let error = reject(presentation(json!({"tagline": "Unknown"})));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.malformed");
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn a_description_is_bounded_trimmed_and_allows_only_newline_controls() {
    let longest = "é".repeat(1024);
    assert_eq!(
        accept(presentation(json!({"description": longest.clone()}))).description,
        Some(longest)
    );
    assert_eq!(
        accept(presentation(json!({"description": "x"}))).description,
        Some("x".to_owned())
    );
    assert_eq!(
        accept(presentation(
            json!({"description": "First line.\nSecond line."})
        ))
        .description,
        Some("First line.\nSecond line.".to_owned())
    );
    for refused in [
        String::new(),
        "é".repeat(1025),
        " padded".to_owned(),
        "padded\n".to_owned(),
        "tab\there".to_owned(),
        "carriage\rreturn".to_owned(),
        "bell\u{7}".to_owned(),
    ] {
        assert_refused(
            presentation(json!({"description": refused})),
            "manifest.description",
            "/presentation/description",
        );
    }
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn an_author_is_bounded_and_has_no_control_characters() {
    let longest = "é".repeat(128);
    assert_eq!(
        accept(presentation(json!({"author": longest.clone()}))).author,
        Some(longest)
    );
    assert_eq!(
        accept(presentation(json!({"author": "A"}))).author,
        Some("A".to_owned())
    );
    for refused in [
        String::new(),
        "é".repeat(129),
        "two\nlines".to_owned(),
        "tab\there".to_owned(),
    ] {
        assert_refused(
            presentation(json!({"author": refused})),
            "manifest.author",
            "/presentation/author",
        );
    }
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn a_homepage_is_an_absolute_bounded_https_url() {
    let prefix = "https://example.com/";
    let longest = format!("{prefix}{}", "a".repeat(2048 - prefix.len()));
    assert_eq!(longest.len(), 2048);
    assert_eq!(
        accept(presentation(json!({"homepage": longest.clone()}))).homepage,
        Some(longest.clone())
    );
    assert_eq!(
        accept(presentation(json!({"homepage": "https://example.com"}))).homepage,
        Some("https://example.com".to_owned())
    );
    for refused in [
        format!("{longest}a"),
        String::new(),
        "http://example.com/".to_owned(),
        "/relative/path".to_owned(),
        "example.com/path".to_owned(),
        "//example.com/path".to_owned(),
        "https://".to_owned(),
        "https:///path".to_owned(),
        "HTTPS://example.com/".to_owned(),
        "https://example.com/with space".to_owned(),
        " https://example.com/".to_owned(),
        "https://example.com/\n".to_owned(),
        "mailto:author@example.com".to_owned(),
        "javascript:alert(1)".to_owned(),
    ] {
        assert_refused(
            presentation(json!({"homepage": refused})),
            "manifest.homepage",
            "/presentation/homepage",
        );
    }
}

/// The published schema agrees with the semantic rules on every case it can express.
/// The `description` pattern also enforces trimming, so the schema refuses
/// leading or trailing whitespace there as the semantic rules do.
// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn the_schema_publishes_the_presentation_metadata_bounds() {
    let schema: Value = serde_json::from_slice(
        &fs::read(repository_root().join("contracts/packages/v1/schemas/manifest.schema.json"))
            .unwrap(),
    )
    .unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    let longest_homepage = format!("https://example.com/{}", "a".repeat(2028));
    let cases = [
        (json!({"description": "é".repeat(1024)}), true),
        (json!({"description": "One.\nTwo."}), true),
        (json!({"description": ""}), false),
        (json!({"description": "é".repeat(1025)}), false),
        (json!({"description": "tab\there"}), false),
        (json!({"description": " padded"}), false),
        (json!({"description": "padded\n"}), false),
        (json!({"description": "\u{a0}padded"}), false),
        (json!({"description": "padded\u{3000}"}), false),
        (json!({"description": "x"}), true),
        (json!({"description": "inner\u{a0}space"}), true),
        (json!({"description": "\u{feff}not trimmed"}), true),
        (json!({"author": "é".repeat(128)}), true),
        (json!({"author": "é".repeat(129)}), false),
        (json!({"author": "two\nlines"}), false),
        (json!({"homepage": "https://example.com"}), true),
        (json!({"homepage": longest_homepage.clone()}), true),
        (json!({"homepage": format!("{longest_homepage}a")}), false),
        (json!({"homepage": "http://example.com/"}), false),
        (json!({"homepage": "/relative/path"}), false),
        (json!({"homepage": "https:///path"}), false),
        (json!({"homepage": "https://user@example.com/"}), false),
        (json!({"homepage": "https://example.com/with space"}), false),
        (json!({"tagline": "Unknown"}), false),
    ];
    for (extra, valid) in cases {
        let document = manifest(presentation(extra.clone()));
        assert_eq!(validator.is_valid(&document), valid, "schema: {extra}");
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&document).unwrap()).is_ok(),
            valid,
            "rule: {extra}"
        );
    }
}

fn ui_views(views: Value) -> Value {
    json!({"ui": {
        "entrypoint": {"path": "ui/index.js", "size": 7, "sha256": "0".repeat(64)},
        "assets": [],
        "contract": {"path": "ui/contract.json", "sha256": "0".repeat(64)},
        "views": views,
    }})
}

// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn ui_views_are_optional_and_settings_is_the_only_declarable_view() {
    use teamarness_package_contract::manifest::UiView;

    assert!(accept(json!({})).ui_views.is_empty());
    assert!(accept(ui_views(json!([]))).ui_views.is_empty());
    assert_eq!(
        accept(ui_views(json!(["settings"]))).ui_views,
        vec![UiView::Settings]
    );

    assert_refused(
        ui_views(json!(["settings", "settings"])),
        "manifest.ui_view_duplicate",
        "/ui/views/1",
    );
    for (views, pointer) in [
        (json!(["main"]), "/ui/views/0"),
        (json!(["preferences"]), "/ui/views/0"),
        (json!(["settings", "Settings"]), "/ui/views/1"),
        (json!([""]), "/ui/views/0"),
        (json!([1]), "/ui/views/0"),
        (json!([null]), "/ui/views/0"),
    ] {
        assert_refused(ui_views(views), "manifest.ui_view", pointer);
    }
    let error = reject(ui_views(json!("settings")));
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.malformed");
}

/// The published schema and the semantic rule agree on `ui.views`.
// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn the_schema_publishes_the_ui_views_rule() {
    let schema: Value = serde_json::from_slice(
        &fs::read(repository_root().join("contracts/packages/v1/schemas/manifest.schema.json"))
            .unwrap(),
    )
    .unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    for (views, valid) in [
        (json!([]), true),
        (json!(["settings"]), true),
        (json!(["settings", "settings"]), false),
        (json!(["main"]), false),
        (json!(["preferences"]), false),
        (json!([1]), false),
        (json!("settings"), false),
    ] {
        let document = manifest(ui_views(views.clone()));
        assert_eq!(validator.is_valid(&document), valid, "schema: {views}");
        assert_eq!(
            parse_manifest(&serde_json::to_vec(&document).unwrap()).is_ok(),
            valid,
            "rule: {views}"
        );
    }
}

/// A loaded Package Directory carries its declared views.
// Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
#[test]
fn a_loaded_package_directory_carries_its_declared_views() {
    use teamarness_package_contract::manifest::UiView;

    let contract = br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{}}"#;
    let root = TempDir::new().unwrap();
    write_directory(root.path(), b"backend", b"export {}", contract);
    let mut document: Value =
        serde_json::from_slice(&fs::read(root.path().join("teamarness.package.json")).unwrap())
            .unwrap();
    document["ui"]["views"] = json!(["settings"]);
    fs::write(
        root.path().join("teamarness.package.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    let loaded = load_package_directory(root.path(), TARGET).unwrap();
    assert_eq!(loaded.ui.views, vec![UiView::Settings]);
}
