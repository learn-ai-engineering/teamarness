//! The normative fixtures in `contracts/packages/v1` classify the way the Host classifies them.

use std::fs;
use std::path::PathBuf;

use teamarness_package_contract::ContractErrorCode;
use teamarness_package_contract::manifest::parse_manifest;
use teamarness_package_contract::ui_contract::compile_ui_contract;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/packages/v1/fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

#[test]
fn valid_contract_fixtures_compile() {
    let contract = compile_ui_contract(&fixture("ui-contract.valid.json")).unwrap();
    assert_eq!(
        contract.request_operations().collect::<Vec<_>>(),
        ["reference.read"]
    );
    assert_eq!(
        contract.subscription_operations().collect::<Vec<_>>(),
        ["reference.changes"]
    );

    let manifest = parse_manifest(&fixture("manifest.valid.json")).unwrap();
    assert_eq!(manifest.identity, "dev.teamarness.reference");
    assert_eq!(manifest.version.to_string(), "1.0.0");
    assert_eq!(manifest.ui_contract_path(), "ui/contract.json");
}

#[test]
fn remote_reference_fixture_reports_its_rule() {
    let error = compile_ui_contract(&fixture("ui-contract.invalid-remote.json")).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::UiContractViolation);
    assert_eq!(error.rule, "ui_contract.reference_not_local");
    assert_eq!(error.pointer.as_deref(), Some("/$defs/remote/$ref"));
}

#[test]
fn unknown_manifest_field_fixture_is_malformed() {
    let error = parse_manifest(&fixture("manifest.invalid-unknown-field.json")).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::ManifestMalformed);
    assert_eq!(error.rule, "manifest.malformed");
}
