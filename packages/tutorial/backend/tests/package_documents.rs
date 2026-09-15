use std::fs;
use std::path::Path;

fn read_json(path: impl AsRef<Path>) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn manifest_template_and_request_only_contract_match_package_schemas() {
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let contract_root = package_root.join("../../contracts/packages/v1/schemas");
    let cases = [
        (
            contract_root.join("manifest.schema.json"),
            package_root.join("teamarness.package.json"),
        ),
        (
            contract_root.join("ui-contract-profile.schema.json"),
            package_root.join("ui-contract.json"),
        ),
    ];

    for (schema_path, document_path) in cases {
        let schema = read_json(schema_path);
        let document = read_json(&document_path);
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        assert!(
            validator.is_valid(&document),
            "{} does not match its package schema",
            document_path.display()
        );
    }

    let manifest = read_json(package_root.join("teamarness.package.json"));
    assert_eq!(manifest["identity"], "dev.teamarness.tutorial");
    assert_eq!(manifest["capabilities"], serde_json::json!([]));
    assert_eq!(manifest["ui"]["assets"], serde_json::json!([]));

    let contract = read_json(package_root.join("ui-contract.json"));
    assert_eq!(
        contract["requests"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["tutorial.generate", "tutorial.read", "tutorial.save"]
    );
    assert_eq!(contract["subscriptions"], serde_json::json!({}));
}
