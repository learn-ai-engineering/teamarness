//! One test per UI contract profile rule, each pinning the exact code and rule name.

use serde_json::{Value, json};
use teamarness_package_contract::ui_contract::compile_ui_contract;
use teamarness_package_contract::{ContractError, ContractErrorCode};

fn reject(contract: &Value) -> ContractError {
    let error = compile_ui_contract(&serde_json::to_vec(contract).unwrap()).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::UiContractViolation);
    error
}

fn contract_with_defs(definitions: Value) -> Value {
    json!({
        "schemaVersion": 1,
        "$defs": definitions,
        "requests": {},
        "subscriptions": {},
    })
}

#[test]
fn rejects_dynamic_references() {
    for keyword in [
        "$dynamicRef",
        "$dynamicAnchor",
        "$recursiveRef",
        "$recursiveAnchor",
    ] {
        let error = reject(&contract_with_defs(json!({"node": {keyword: "#node"}})));
        assert_eq!(error.rule, "ui_contract.dynamic_reference", "{keyword}");
        assert_eq!(
            error.pointer.as_deref(),
            Some(format!("/$defs/node/{keyword}").as_str())
        );
    }
}

#[test]
fn rejects_missing_definitions() {
    let error = reject(&contract_with_defs(
        json!({"node": {"$ref": "#/$defs/absent"}}),
    ));
    assert_eq!(error.rule, "ui_contract.reference_missing");
    assert_eq!(error.pointer.as_deref(), Some("/$defs/node/$ref"));
}

#[test]
fn rejects_remote_and_non_local_references() {
    for reference in [
        json!("https://example.com/x.json"),
        json!("#/definitions/node"),
        json!("#/$defs/nested/deep"),
        json!(7),
    ] {
        let error = reject(&contract_with_defs(json!({"node": {"$ref": reference}})));
        assert_eq!(error.rule, "ui_contract.reference_not_local", "{reference}");
    }
}

#[test]
fn rejects_reference_cycles() {
    let error = reject(&contract_with_defs(json!({
        "left": {"$ref": "#/$defs/right"},
        "right": {"$ref": "#/$defs/left"},
    })));
    assert_eq!(error.rule, "ui_contract.reference_cycle");
}

#[test]
fn rejects_schemas_deeper_than_64() {
    let mut schema = json!({"type": "string"});
    for _ in 0..40 {
        schema = json!({"properties": {"next": schema}});
    }
    assert!(
        compile_ui_contract(
            &serde_json::to_vec(&contract_with_defs(json!({"node": schema.clone()}))).unwrap()
        )
        .is_err()
    );

    let mut shallow = json!({"type": "string"});
    for _ in 0..30 {
        shallow = json!({"properties": {"next": shallow}});
    }
    compile_ui_contract(
        &serde_json::to_vec(&contract_with_defs(json!({"node": shallow}))).unwrap(),
    )
    .unwrap();

    let error = reject(&contract_with_defs(json!({"node": schema})));
    assert_eq!(error.rule, "ui_contract.schema_too_deep");
}

#[test]
fn rejects_more_than_16384_nodes() {
    let wide: Vec<Value> = (0..20_000).map(|index| json!({"const": index})).collect();
    let error = reject(&contract_with_defs(json!({"node": {"anyOf": wide}})));
    assert_eq!(error.rule, "ui_contract.too_many_nodes");
}

#[test]
fn rejects_more_than_128_requests() {
    let mut requests = serde_json::Map::new();
    for index in 0..129 {
        requests.insert(
            format!("fixture.op-{index}"),
            json!({"request": {"type": "object"}, "response": {"type": "object"}}),
        );
    }
    let error = reject(&json!({
        "schemaVersion": 1,
        "$defs": {},
        "requests": requests,
        "subscriptions": {},
    }));
    assert_eq!(error.rule, "ui_contract.too_many_operations");
}

#[test]
fn rejects_invalid_operation_names() {
    for name in [
        "Fixture.read",
        "fixture..read",
        "fixture.Read",
        "9fixture.read",
        "",
    ] {
        let error = reject(&json!({
            "schemaVersion": 1,
            "$defs": {},
            "requests": {name: {"request": {"type": "object"}, "response": {"type": "object"}}},
            "subscriptions": {},
        }));
        assert_eq!(error.rule, "ui_contract.operation_name", "{name}");
    }
}

#[test]
fn rejects_contracts_over_one_mebibyte() {
    let mut bytes = serde_json::to_vec(&contract_with_defs(json!({}))).unwrap();
    bytes.resize(1024 * 1024 + 1, b' ');
    let error = compile_ui_contract(&bytes).unwrap_err();
    assert_eq!(error.code, ContractErrorCode::UiContractViolation);
    assert_eq!(error.rule, "ui_contract.too_large");
}

#[test]
fn rejects_unsupported_schema_versions_and_malformed_documents() {
    let error = reject(&json!({
        "schemaVersion": 2, "$defs": {}, "requests": {}, "subscriptions": {}
    }));
    assert_eq!(error.rule, "ui_contract.schema_version");

    let error = compile_ui_contract(br#"{"schemaVersion":1,"schemaVersion":1}"#).unwrap_err();
    assert_eq!(error.rule, "ui_contract.malformed");
}

#[test]
fn validates_payload_bounds_and_schema() {
    let contract = compile_ui_contract(
        &serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "$defs": {"item": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"], "additionalProperties": false}},
            "requests": {"fixture.read": {"request": {"$ref": "#/$defs/item"}, "response": {"$ref": "#/$defs/item"}}},
            "subscriptions": {"fixture.watch": {"request": {"$ref": "#/$defs/item"}, "event": {"$ref": "#/$defs/item"}}},
        }))
        .unwrap(),
    )
    .unwrap();

    contract
        .validate_request("fixture.read", &json!({"value": "ok"}))
        .unwrap();
    contract
        .validate_response("fixture.read", &json!({"value": "ok"}))
        .unwrap();
    contract
        .validate_subscription("fixture.watch", &json!({"value": "ok"}))
        .unwrap();
    contract
        .validate_event("fixture.watch", &json!({"value": "ok"}))
        .unwrap();

    let unknown = contract
        .validate_request("fixture.absent", &json!({"value": "ok"}))
        .unwrap_err();
    assert_eq!(unknown.rule, "ui_contract.unknown_operation");

    let invalid = contract
        .validate_request("fixture.read", &json!({"value": 7}))
        .unwrap_err();
    assert_eq!(invalid.rule, "ui_contract.payload_invalid");

    let oversized = contract
        .validate_request("fixture.read", &json!({"value": "x".repeat(300 * 1024)}))
        .unwrap_err();
    assert_eq!(oversized.rule, "ui_contract.payload_too_large");

    let mut deep = json!("leaf");
    for _ in 0..70 {
        deep = json!([deep]);
    }
    let too_deep = contract
        .validate_request("fixture.read", &deep)
        .unwrap_err();
    assert_eq!(too_deep.rule, "ui_contract.payload_too_large");
}
