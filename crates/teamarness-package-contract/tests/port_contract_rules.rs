use serde_json::{Value, json};
use teamarness_package_contract::port_contract::{
    check_additive_minor, check_additive_minor_documents, compile_port_contracts,
};

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
fn minor_comparison_checks_referenced_definitions_in_both_documents() {
    let mut previous: Value = serde_json::from_slice(&fixture("version-1-0")).unwrap();
    previous["$defs"]["payload"] = json!({"type":"string"});
    previous["contracts"][0]["ports"][0]["request"] = json!({"$ref":"#/$defs/payload"});
    let old = compile_port_contracts(&serde_json::to_vec(&previous).unwrap()).unwrap();
    let mut next = previous.clone();
    next["contracts"][0]["minor"] = json!(1);
    next["$defs"]["newUnrelated"] = json!({"type":"boolean"});
    let compatible = compile_port_contracts(&serde_json::to_vec(&next).unwrap()).unwrap();
    assert!(check_additive_minor(&old.contracts[0], &compatible.contracts[0]).is_err());
    check_additive_minor_documents(&old, &compatible, "dev.example.conversations").unwrap();
    next["$defs"]["payload"] = json!({"type":"number"});
    let breaking = compile_port_contracts(&serde_json::to_vec(&next).unwrap()).unwrap();
    assert_eq!(
        check_additive_minor_documents(&old, &breaking, "dev.example.conversations")
            .unwrap_err()
            .rule,
        "port_contract.incompatible_minor"
    );
}

// Verifies: TEAM.SYS.PKG-R12
#[test]
fn unused_definitions_are_validated_even_without_provided_contracts() {
    let empty = json!({"schemaVersion":1,"$defs":{"unused":{"$ref":"https://example.com/remote"}},"contracts":[]});
    assert!(compile_port_contracts(&serde_json::to_vec(&empty).unwrap()).is_err());
}

#[test]
fn guide_classification_resolves_local_shapes_and_warns_on_bad_guides() {
    let mut doc = json!({"schemaVersion":1,"$defs":{
        "empty":{"type":"object","additionalProperties":false},
        "markdown":{"type":"string","maxLength":16384},
        "guide":{"type":"object","properties":{"markdown":{"$ref":"#/$defs/markdown"}},"required":["markdown"],"additionalProperties":false}
    },"contracts":[{"identity":"dev.example.guide","major":1,"minor":0,"description":"Guide","ports":[
        {"name":"runtime.guide","kind":"query","description":"Guidance","request":{"$ref":"#/$defs/empty"},"response":{"$ref":"#/$defs/guide"}}
    ]}]});
    let compile = |doc: &Value| compile_port_contracts(&serde_json::to_vec(doc).unwrap()).unwrap();
    assert!(compile(&doc).has_guide("dev.example.guide"));
    doc["contracts"][0]["ports"][0]["request"] =
        json!({"type":"object","additionalProperties":false});
    assert!(compile(&doc).has_guide("dev.example.guide"));
    let valid = doc.clone();
    let mut impossible = doc.clone();
    impossible["contracts"][0]["ports"][0]["request"] =
        json!({"type":"object","additionalProperties":false,"minProperties":1});
    assert!(!compile(&impossible).has_guide("dev.example.guide"));
    for (field, shape) in [
        ("request", json!({"type":"object"})),
        (
            "request",
            json!({"type":"object","properties":{"input":{"type":"string"}},"additionalProperties":false}),
        ),
        (
            "response",
            json!({"type":"object","properties":{"markdown":{"type":"string","maxLength":100}},"additionalProperties":false}),
        ),
        (
            "response",
            json!({"type":"object","properties":{"markdown":{"type":"number"}},"required":["markdown"],"additionalProperties":false}),
        ),
    ] {
        let mut malformed = valid.clone();
        malformed["contracts"][0]["ports"][0][field] = shape;
        assert!(!compile(&malformed).has_guide("dev.example.guide"));
    }
    let mut literal = valid.clone();
    literal["contracts"][0]["ports"][0]["response"] = json!({"type":"object","properties":{"markdown":{"type":"string","maxLength":100}},"required":["markdown"],"additionalProperties":false});
    assert!(compile(&literal).has_guide("dev.example.guide"));
    doc["$defs"]["markdown"] = json!({"type":"string"});
    let malformed = compile(&doc);
    assert!(!malformed.has_guide("dev.example.guide"));
    assert!(malformed.warnings.iter().any(|w| w.contains("guide_shape")));
    doc["contracts"][0]["ports"][0]["name"] = json!("other.read");
    assert!(!compile(&doc).has_guide("dev.example.guide"));
}

#[test]
fn metadata_projection_is_complete_filtered_and_deterministic_at_size_bound() {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    let mut doc = json!({"schemaVersion":1,"$defs":{
        "outer":{"type":"object","properties":{"text":{"$ref":"#/$defs/inner"}},"additionalProperties":false},
        "inner":{"type":"string"},"unused":{"type":"boolean"}
    },"contracts":[{"identity":"dev.example.metadata","major":1,"minor":2,"description":"Metadata","ports":[
        {"name":"value.read","kind":"query","description":"Read","request":true,"response":{"$ref":"#/$defs/outer"}},
        {"name":"value.watch","kind":"query","description":"Watch","request":true,"event":{"$ref":"#/$defs/inner"}},
        {"name":"value.changed","kind":"event","description":"Changed","event":{"type":"boolean"}}
    ]}]});
    let identity = "dev.example.metadata";
    let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
    let names = BTreeSet::from(["value.read".to_owned()]);
    let metadata = compiled.project_metadata(identity, Some(&names)).unwrap();
    let actual: Value = serde_json::from_slice(&metadata.bytes).unwrap();
    assert_eq!(
        actual["$defs"],
        json!({"outer":doc["$defs"]["outer"],"inner":{"type":"string"}})
    );
    assert_eq!(
        actual["contracts"][0]["ports"],
        json!([doc["contracts"][0]["ports"][0]])
    );
    assert_eq!(
        metadata.sha256,
        Sha256::digest(&metadata.bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        metadata.bytes,
        compiled
            .project_metadata(identity, Some(&names))
            .unwrap()
            .bytes
    );
    let all = compiled.project_metadata(identity, None).unwrap();
    let actual: Value = serde_json::from_slice(&all.bytes).unwrap();
    assert!(
        actual["contracts"][0]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "value.watch")
            .unwrap()
            .get("response")
            .is_none()
    );
    assert!(
        actual["contracts"][0]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "value.changed")
            .unwrap()
            .get("request")
            .is_none()
    );
    let size = serde_json::to_vec(&doc).unwrap().len();
    doc["$defs"]["inner"]["description"] = json!("x".repeat(1024 * 1024 - size - 17));
    let bytes = serde_json::to_vec(&doc).unwrap();
    assert_eq!(bytes.len(), 1024 * 1024);
    let compiled = compile_port_contracts(&bytes).unwrap();
    assert!(
        compiled
            .project_metadata(identity, None)
            .unwrap()
            .bytes
            .len()
            <= bytes.len()
    );
}

#[test]
fn current_capability_wire_examples_are_closed_and_metadata_fixture_is_complete() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/package-capabilities/v1/package-ports-2.0.schema.json"
    ))
    .unwrap();
    for name in [
        "availableCall",
        "availableResult",
        "describeCall",
        "describeResult",
        "enableCall",
        "enableResult",
        "disableCall",
        "disableResult",
        "invokeCall",
        "queryStream",
        "eventStream",
    ] {
        let validator = jsonschema::validator_for(
            &json!({"$defs":schema["$defs"],"$ref":format!("#/$defs/{name}")}),
        )
        .unwrap();
        let example = &schema["$defs"][name]["examples"][0];
        assert!(validator.is_valid(example), "{name}");
        let mut with_selector = example.clone();
        with_selector["workspace"] = json!("chosen-by-caller");
        assert!(!validator.is_valid(&with_selector), "{name} must be closed");
    }
    assert_eq!(
        schema["$defs"]["availableResult"]["properties"]["contracts"]["maxItems"],
        32
    );
    assert_eq!(
        schema["$defs"]["describeResult"]["properties"]["totalBytes"]["maximum"],
        1024 * 1024
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../contracts/package-capabilities/v1/package-ports-2.0.golden.json"
    ))
    .unwrap();
    let document =
        compile_port_contracts(&serde_json::to_vec(&fixture["document"]).unwrap()).unwrap();
    assert_eq!(document.contracts[0].identity, "dev.example.metadata");
    assert_eq!(
        document.contracts[0].ports[0].response,
        Some(json!({"$ref":"#/$defs/text"}))
    );
}
