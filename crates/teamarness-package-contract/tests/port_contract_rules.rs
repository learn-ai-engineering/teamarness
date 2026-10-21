use serde_json::{Value, json};
use teamarness_package_contract::port_contract::{check_additive_minor, compile_port_contracts};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../contracts/package-ports/v1/fixtures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn validates_closed_payloads_and_exact_shapes() {
    let contract = compile_port_contracts(&fixture("version-1-0")).unwrap();
    assert_eq!(contract.contracts[0].ports.len(), 3);
    assert!(
        contract
            .validate(
                "dev.example.conversations",
                "conversation.send",
                "request",
                &json!({"text":"hello"})
            )
            .is_ok()
    );
    assert!(
        contract
            .validate(
                "dev.example.conversations",
                "conversation.send",
                "request",
                &json!({"text":"hello","actor":"forged"})
            )
            .is_err()
    );
    assert!(
        contract
            .validate(
                "dev.example.conversations",
                "conversation.sent",
                "event",
                &json!({"id":"1"})
            )
            .is_ok()
    );
    assert!(
        contract
            .validate(
                "dev.example.conversations",
                "conversation.sent",
                "event",
                &json!({"id":3})
            )
            .is_err()
    );
    assert!(!contract.warnings.is_empty()); // grammatical advice does not fail admission
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn rejects_duplicate_ports_bad_schemas_and_invalid_shape() {
    let original: Value = serde_json::from_slice(&fixture("version-1-0")).unwrap();
    for update in [
        ("duplicate", json!(original["contracts"][0]["ports"][0])),
        (
            "unknown",
            json!({"name":"conversation.new","kind":"event","description":"New conversation","event":{"$ref":"https://example.com/remote"}}),
        ),
        (
            "shape",
            json!({"name":"conversation.other","kind":"command","description":"Other","event":true}),
        ),
    ] {
        let mut document = original.clone();
        document["contracts"][0]["ports"]
            .as_array_mut()
            .unwrap()
            .push(update.1);
        assert!(
            compile_port_contracts(&serde_json::to_vec(&document).unwrap()).is_err(),
            "{}",
            update.0
        );
    }
    let mut document = original;
    document["contracts"][0]["ports"][0]["unknown"] = json!(true);
    assert!(compile_port_contracts(&serde_json::to_vec(&document).unwrap()).is_err());
    assert_eq!(
        compile_port_contracts(&vec![b' '; 1024 * 1024 + 1])
            .unwrap_err()
            .rule,
        "port_contract.too_large"
    );
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn minor_comparison_retains_existing_port_shapes() {
    let old = compile_port_contracts(&fixture("version-1-0")).unwrap();
    let new = compile_port_contracts(&fixture("version-1-1")).unwrap();
    check_additive_minor(&old.contracts[0], &new.contracts[0]).unwrap();
    let mut incompatible = new.contracts[0].clone();
    incompatible.ports[0].request = Some(json!(true));
    assert_eq!(
        check_additive_minor(&old.contracts[0], &incompatible)
            .unwrap_err()
            .rule,
        "port_contract.incompatible_minor"
    );
    incompatible = new.contracts[0].clone();
    incompatible.ports.remove(0);
    assert!(check_additive_minor(&old.contracts[0], &incompatible).is_err());
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn unused_definitions_are_validated_even_without_provided_contracts() {
    let empty = json!({"schemaVersion":1,"$defs":{"unused":{"$ref":"https://example.com/remote"}},"contracts":[]});
    assert!(compile_port_contracts(&serde_json::to_vec(&empty).unwrap()).is_err());
}
