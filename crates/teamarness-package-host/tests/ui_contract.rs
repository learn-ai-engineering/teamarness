#![allow(dead_code)]

use std::collections::BTreeMap;

pub use teamarness_package_host::{PackageErrorCode, PackageIdentity, PackageValue};

#[path = "../src/manifest.rs"]
mod manifest;
#[path = "../src/ui.rs"]
mod ui;

fn contract(
    requests: serde_json::Map<String, serde_json::Value>,
    subscriptions: serde_json::Map<String, serde_json::Value>,
) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "$defs": {},
        "requests": requests,
        "subscriptions": subscriptions,
    }))
    .unwrap()
}

fn request(request: serde_json::Value, response: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"request": request, "response": response})
}

fn subscription(request: serde_json::Value, event: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"request": request, "event": event})
}

fn object(entries: impl IntoIterator<Item = (&'static str, PackageValue)>) -> PackageValue {
    PackageValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[test]
fn validates_request_response_subscription_and_event_directions() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "required": ["value"],
        "additionalProperties": false
    });
    let compiled = ui::compile_contract(&contract(
        serde_json::Map::from_iter([(
            "reference.read".into(),
            request(schema.clone(), schema.clone()),
        )]),
        serde_json::Map::from_iter([(
            "reference.changes".into(),
            subscription(schema.clone(), schema),
        )]),
    ))
    .unwrap();
    let valid = object([("value", PackageValue::String("ok".into()))]);
    let invalid = object([("value", PackageValue::Boolean(true))]);

    assert!(compiled.validate_request("reference.read", &valid).is_ok());
    assert!(compiled.validate_response("reference.read", &valid).is_ok());
    assert!(
        compiled
            .validate_subscription("reference.changes", &valid)
            .is_ok()
    );
    assert!(compiled.validate_event("reference.changes", &valid).is_ok());
    assert!(compiled.validate_request("unknown", &valid).is_err());
    assert!(
        compiled
            .validate_response("reference.read", &invalid)
            .is_err()
    );
    assert!(
        compiled
            .validate_subscription("reference.changes", &invalid)
            .is_err()
    );
    assert!(
        compiled
            .validate_event("reference.changes", &invalid)
            .is_err()
    );
}

#[test]
fn contract_document_is_closed_and_requires_every_direction() {
    let invalid = [
        br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{},"extra":{}}"#.as_slice(),
        br#"{"schemaVersion":2,"$defs":{},"requests":{},"subscriptions":{}}"#.as_slice(),
        br#"{"schemaVersion":1,"requests":{},"subscriptions":{}}"#.as_slice(),
        br#"{"schemaVersion":1,"$defs":{},"requests":{"read":{"request":{}}},"subscriptions":{}}"#.as_slice(),
        br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{"changes":{"request":{}}}}"#.as_slice(),
        br#"{"schemaVersion":1,"$defs":{},"requests":{"read":{"request":{},"response":{},"extra":{}}},"subscriptions":{}}"#.as_slice(),
        br#"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{},"requests":{}}"#.as_slice(),
    ];
    for bytes in invalid {
        assert!(
            ui::compile_contract(bytes).is_err(),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn operation_name_and_count_boundaries_are_enforced() {
    for name in [
        "".to_owned(),
        "Upper.read".to_owned(),
        "reference..read".to_owned(),
        "reference/read".to_owned(),
        "a".repeat(129),
    ] {
        let bytes = contract(
            serde_json::Map::from_iter([(
                name.clone(),
                request(serde_json::json!({}), serde_json::json!({})),
            )]),
            serde_json::Map::new(),
        );
        assert!(ui::compile_contract(&bytes).is_err(), "{name}");
    }
    let requests = (0..128)
        .map(|index| {
            (
                format!("request.op_{index}"),
                request(serde_json::json!({}), serde_json::json!({})),
            )
        })
        .collect();
    let subscriptions = (0..128)
        .map(|index| {
            (
                format!("stream.op_{index}"),
                subscription(serde_json::json!({}), serde_json::json!({})),
            )
        })
        .collect();
    assert!(ui::compile_contract(&contract(requests, subscriptions)).is_ok());

    let too_many = (0..129)
        .map(|index| {
            (
                format!("request.op_{index}"),
                request(serde_json::json!({}), serde_json::json!({})),
            )
        })
        .collect();
    assert!(ui::compile_contract(&contract(too_many, serde_json::Map::new())).is_err());
}

#[test]
fn references_cycles_depth_nodes_and_payload_limits_are_enforced() {
    let valid = br##"{"schemaVersion":1,"$defs":{"item":{"type":"string"}},"requests":{"reference.read":{"request":{"$ref":"#/$defs/item"},"response":{}}},"subscriptions":{}}"##;
    assert!(ui::compile_contract(valid).is_ok());
    for bytes in [
        br##"{"schemaVersion":1,"$defs":{},"requests":{"reference.read":{"request":{"$ref":"#/requests/reference.read"},"response":{}}},"subscriptions":{}}"##.as_slice(),
        br##"{"schemaVersion":1,"$defs":{},"requests":{"reference.read":{"request":{"$ref":"#/$defs/missing"},"response":{}}},"subscriptions":{}}"##.as_slice(),
        br##"{"schemaVersion":1,"$defs":{"a":{"$ref":"#/$defs/b"},"b":{"$ref":"#/$defs/a"}},"requests":{},"subscriptions":{}}"##.as_slice(),
        br##"{"schemaVersion":1,"$defs":{"a":{"$dynamicRef":"#/$defs/a"}},"requests":{},"subscriptions":{}}"##.as_slice(),
    ] {
        assert!(ui::compile_contract(bytes).is_err());
    }

    let mut at_limit = serde_json::json!({"type":"string"});
    for _ in 0..32 {
        at_limit = serde_json::json!({"allOf":[at_limit]});
    }
    let mut above_limit = at_limit.clone();
    above_limit = serde_json::json!({"allOf":[above_limit]});
    assert!(
        ui::compile_contract(&contract(
            serde_json::Map::from_iter([(
                "depth.read".into(),
                request(at_limit, serde_json::json!({}))
            )]),
            serde_json::Map::new(),
        ))
        .is_ok()
    );
    assert!(
        ui::compile_contract(&contract(
            serde_json::Map::from_iter([(
                "depth.read".into(),
                request(above_limit, serde_json::json!({}))
            )]),
            serde_json::Map::new(),
        ))
        .is_err()
    );

    let unrestricted = ui::compile_contract(&contract(
        serde_json::Map::from_iter([(
            "payload.read".into(),
            request(serde_json::json!({}), serde_json::json!({})),
        )]),
        serde_json::Map::new(),
    ))
    .unwrap();
    let at_depth = (0..64).fold(PackageValue::Null, |value, _| {
        PackageValue::Array(vec![value])
    });
    let too_deep = PackageValue::Array(vec![at_depth.clone()]);
    let at_size = PackageValue::String("x".repeat(256 * 1024 - 2));
    let too_large = PackageValue::String("x".repeat(256 * 1024 - 1));
    assert!(
        unrestricted
            .validate_request("payload.read", &at_depth)
            .is_ok()
    );
    assert!(
        unrestricted
            .validate_request("payload.read", &too_deep)
            .is_err()
    );
    assert!(
        unrestricted
            .validate_request("payload.read", &at_size)
            .is_ok()
    );
    assert!(
        unrestricted
            .validate_request("payload.read", &too_large)
            .is_err()
    );

    let oversized_nodes = serde_json::json!({"enum": vec![serde_json::Value::Null; 16_384]});
    assert!(
        ui::compile_contract(&contract(
            serde_json::Map::from_iter([(
                "nodes.read".into(),
                request(oversized_nodes, serde_json::json!({}))
            )]),
            serde_json::Map::new(),
        ))
        .is_err()
    );
}

#[test]
fn contract_byte_limit_is_enforced() {
    let mut bytes =
        br##"{"schemaVersion":1,"$defs":{},"requests":{},"subscriptions":{},"#"##.to_vec();
    bytes.resize(1024 * 1024 + 1, b' ');
    assert!(ui::compile_contract(&bytes).is_err());
}
