//! Version-specific capability wire contracts. Expectations are independent of Host handlers.
use std::{fs, path::PathBuf};

use serde_json::{Value, json};

fn schema(major: u16) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../contracts/package-capabilities/v1/package-ports-{major}.0.schema.json"
    ));
    serde_json::from_slice(&fs::read(path).expect("versioned capability schema")).unwrap()
}

// Verifies: TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[test]
fn major_three_golden_metadata_is_complete_and_digest_identifies_exact_bytes() {
    use sha2::{Digest, Sha256};
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/package-capabilities/v1/package-ports-3.0.golden.json");
    let fixture: Value = serde_json::from_slice(&fs::read(path).expect("major-3 golden")).unwrap();
    let schema = schema(3);
    for definition in [
        "availableCall",
        "availableResult",
        "describeCall",
        "describeResult",
    ] {
        assert!(
            accepts(&schema, definition, &fixture[definition]),
            "{definition}"
        );
    }
    let result = &fixture["describeResult"];
    let bytes = fixture["compactUtf8"].as_str().unwrap().as_bytes();
    assert_eq!(
        result["data"],
        schema["$defs"]["describeResult"]["examples"][0]["data"]
    );
    assert_eq!(bytes.len(), result["totalBytes"].as_u64().unwrap() as usize);
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(digest, result["metadataSha256"]);
    assert_eq!(
        serde_json::from_slice::<Value>(bytes).unwrap(),
        fixture["document"]
    );
    assert_eq!(
        fixture["availableResult"]["contracts"][0]["metadataSha256"],
        result["metadataSha256"]
    );
    assert_eq!(
        fixture["describeCall"]["metadataSha256"],
        result["metadataSha256"]
    );
    let compiled =
        teamarness_package_contract::port_contract::compile_port_contracts(bytes).unwrap();
    assert_eq!(
        compiled.contracts[0].ports[0].response,
        Some(json!({"$ref":"#/$defs/text"}))
    );
    assert_eq!(
        fixture["availableResult"]["contracts"][0]["guideEligibility"],
        "missing"
    );
}

fn accepts(schema: &Value, definition: &str, value: &Value) -> bool {
    jsonschema::validator_for(
        &json!({"$defs":schema["$defs"],"$ref":format!("#/$defs/{definition}")}),
    )
    .unwrap()
    .is_valid(value)
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R13
#[test]
fn major_two_keeps_person_selection_and_revision_out_of_its_closed_wire() {
    let schema = schema(2);
    for (definition, original) in [
        (
            "availableCall",
            json!({"method":"available","access":"agent"}),
        ),
        (
            "describeCall",
            json!({"method":"describe","access":"declared","contract":"dev.example.metadata","metadataSha256":"a".repeat(64),"offset":0}),
        ),
        (
            "enableCall",
            json!({"method":"enable","contract":"dev.example.metadata","major":1}),
        ),
        (
            "disableCall",
            json!({"method":"disable","contract":"dev.example.metadata"}),
        ),
        (
            "invokeCall",
            json!({"method":"invoke","access":"agent","contract":"dev.example.metadata","major":1,"port":"value.read","kind":"query","request":{}}),
        ),
        (
            "queryStream",
            json!({"grant":"opaque","access":"agent","contract":"dev.example.metadata","major":1,"port":"value.watch","kind":"query","request":{}}),
        ),
        (
            "eventStream",
            json!({"grant":"opaque","access":"agent","contract":"dev.example.metadata","major":1,"port":"value.changed","kind":"event"}),
        ),
    ] {
        assert!(accepts(&schema, definition, &original), "{definition}");
        let mut changed = original.clone();
        changed["access"] = json!("person");
        assert!(
            !accepts(&schema, definition, &changed),
            "{definition}: Person"
        );
        let mut changed = original.clone();
        changed["routeRevision"] = json!("a".repeat(64));
        assert!(
            !accepts(&schema, definition, &changed),
            "{definition}: revision"
        );
        if matches!(definition, "enableCall" | "disableCall") {
            let mut changed = original;
            changed["access"] = json!("agent");
            assert!(
                !accepts(&schema, definition, &changed),
                "{definition}: explicit selection"
            );
        }
    }
}

// Verifies: TEAM.SYS.PKG-R03, TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[test]
fn major_three_has_explicit_person_access_and_closed_scope_bound_requests() {
    let schema = schema(3);
    for definition in [
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
        let examples = schema["$defs"][definition]["examples"].as_array().unwrap();
        assert!(!examples.is_empty());
        for example in examples {
            assert!(
                accepts(&schema, definition, example),
                "{definition}: {example}"
            );
            for selector in ["workspace", "actor", "provider", "grantOverride"] {
                let mut changed = example.clone();
                changed[selector] = json!("caller-selected");
                assert!(
                    !accepts(&schema, definition, &changed),
                    "{definition}: {selector}"
                );
            }
        }
    }
    for access in ["declared", "agent", "person"] {
        assert!(accepts(
            &schema,
            "availableCall",
            &json!({"method":"available","access":access})
        ));
        let mut describe = json!({"method":"describe","access":access,"contract":"dev.example.metadata","major":1,"metadataSha256":"b".repeat(64),"offset":0,"routeRevision":"a".repeat(64)});
        assert!(accepts(&schema, "describeCall", &describe));
        describe.as_object_mut().unwrap().remove("major");
        assert!(!accepts(&schema, "describeCall", &describe));
    }
    for definition in ["enableCall", "disableCall"] {
        let mut request = schema["$defs"][definition]["examples"][0].clone();
        for access in ["agent", "person"] {
            request["access"] = json!(access);
            assert!(accepts(&schema, definition, &request));
        }
        request["access"] = json!("declared");
        assert!(!accepts(&schema, definition, &request));
        request.as_object_mut().unwrap().remove("access");
        assert!(!accepts(&schema, definition, &request));
    }
    let mut disable =
        json!({"method":"disable","access":"person","contract":"dev.example.metadata"});
    assert!(accepts(&schema, "disableCall", &disable));
    disable["routeRevision"] = json!("a".repeat(64));
    assert!(!accepts(&schema, "disableCall", &disable));
    for code in [
        "port_actor_mode_invalid",
        "person_not_current",
        "port_guide_required",
    ] {
        assert!(accepts(&schema, "errorCodes", &json!(code)));
        assert!(!accepts(&self::schema(2), "errorCodes", &json!(code)));
    }
}

// Verifies: TEAM.SYS.PKG-R12, TEAM.SYS.PKG-R13
#[test]
fn major_three_summary_requires_provider_guide_eligibility_and_opaque_revision() {
    let schema = schema(3);
    let row = json!({"contract":"dev.example.metadata","major":1,"minor":0,"description":"Metadata","available":true,"enabled":false,"enabledMajor":null,"metadataSha256":"b".repeat(64),"provider":{"identity":"dev.example.provider","version":"1.2.3","displayName":"Example","description":null},"guideEligibility":"eligible","routeRevision":"a".repeat(64)});
    assert!(accepts(&schema, "summary", &row));
    for field in ["provider", "guideEligibility", "routeRevision"] {
        let mut changed = row.clone();
        changed.as_object_mut().unwrap().remove(field);
        assert!(!accepts(&schema, "summary", &changed), "{field}");
    }
    for revision in [
        "A".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        "g".repeat(64),
    ] {
        let mut changed = row.clone();
        changed["routeRevision"] = json!(revision);
        assert!(!accepts(&schema, "summary", &changed));
    }
    for eligibility in ["missing", "unavailable"] {
        let mut changed = row.clone();
        changed["guideEligibility"] = json!(eligibility);
        assert!(accepts(&schema, "summary", &changed));
    }
    let mut retained = row;
    retained["provider"] = Value::Null;
    retained["routeRevision"] = Value::Null;
    retained["metadataSha256"] = Value::Null;
    retained["minor"] = Value::Null;
    retained["description"] = Value::Null;
    retained["available"] = json!(false);
    retained["enabled"] = json!(true);
    retained["enabledMajor"] = json!(1);
    retained["guideEligibility"] = json!("unavailable");
    assert!(accepts(&schema, "summary", &retained));
}
