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

fn reference_sibling_guide(markdown: Value) -> Value {
    json!({"schemaVersion":1,"$defs":{
        "empty":{"type":"object","additionalProperties":false},
        "text":{"type":"string"},
        "literalBounded":{"type":"string","maxLength":100},
        "bounded":{"$ref":"#/$defs/text","maxLength":100},
        "nested":{"$ref":"#/$defs/bounded","minLength":2},
        "response":{"type":"object","properties":{"markdown":markdown},"required":["markdown"],"additionalProperties":false}
    },"contracts":[{"identity":"dev.example.guide","major":1,"minor":0,"description":"Guide","ports":[
        {"name":"runtime.guide","kind":"query","description":"Guide","request":{"$ref":"#/$defs/empty"},"response":{"$ref":"#/$defs/response"}}
    ]}]})
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_finite_type_sets_preserve_exact_markers_and_runtime_schemas() {
    let scalar = reference_sibling_guide(json!({"type":"string","maxLength":100}));
    let mut cases = vec![("scalar", scalar.clone())];
    for (name, pointer) in [
        ("singleton request", "/$defs/empty/type"),
        ("singleton response", "/$defs/response/type"),
        (
            "singleton markdown",
            "/$defs/response/properties/markdown/type",
        ),
    ] {
        let mut doc = scalar.clone();
        let ty = doc.pointer_mut(pointer).unwrap();
        *ty = json!([ty.clone()]);
        cases.push((name, doc));
    }
    let mut all = scalar.clone();
    all["$defs"]["empty"]["type"] = json!(["object"]);
    all["$defs"]["response"]["type"] = json!(["object"]);
    all["$defs"]["response"]["properties"]["markdown"]["type"] = json!(["string"]);
    cases.push(("all singleton", all));
    let mut singleton_ref =
        reference_sibling_guide(json!({"$ref":"#/$defs/nested","maxLength":100}));
    singleton_ref["$defs"]["text"]["type"] = json!(["string"]);
    cases.push((
        "singleton reference chain with sibling bound",
        singleton_ref,
    ));
    for types in [
        json!(["string", "null"]),
        json!(["null", "string", "boolean", "array"]),
    ] {
        let mut doc = reference_sibling_guide(
            json!({"$ref":"#/$defs/nested","type":["integer","string"],"maxLength":100}),
        );
        doc["$defs"]["text"]["type"] = types;
        doc["$defs"]["nested"]["type"] = json!(["number", "string", "null"]);
        doc["$defs"]["objectTypes"] = json!({"type":["null","object","array"]});
        for name in ["empty", "response"] {
            doc["$defs"][name]["$ref"] = json!("#/$defs/objectTypes");
            doc["$defs"][name]["type"] = json!(["boolean", "object"]);
        }
        cases.push(("finite array intersections through references", doc));
    }
    for (direct, referenced) in [
        (json!("string"), json!(["number", "string"])),
        (json!(["integer", "string"]), json!(["number", "string"])),
    ] {
        for reverse in [false, true] {
            let mut doc = reference_sibling_guide(
                json!({"$ref":"#/$defs/nested","type":direct,"maxLength":100}),
            );
            doc["$defs"]["text"]["type"] = referenced.clone();
            // This intermediate restriction genuinely removes every numeric alternative.
            doc["$defs"]["nested"]["type"] = json!(["string", "null"]);
            for name in ["empty", "response"] {
                doc["$defs"][format!("{name}Types")] = json!({"type":["number","object"]});
                doc["$defs"][name]["$ref"] = json!(format!("#/$defs/{name}Types"));
                doc["$defs"][name]["type"] = json!(["integer", "object"]);
                doc["$defs"][format!("{name}Types")]["$ref"] = json!("#/$defs/objectOnly");
            }
            doc["$defs"]["objectOnly"] = json!({"type":"object"});
            if reverse {
                doc["$defs"]["text"]["type"] = direct.clone();
                doc["$defs"]["response"]["properties"]["markdown"]["type"] = referenced.clone();
                for name in ["empty", "response"] {
                    doc["$defs"][name]["type"] = json!(["number", "object"]);
                    doc["$defs"][format!("{name}Types")]["type"] = json!(["integer", "object"]);
                }
            }
            cases.push((
                "genuine string and object narrowing through reordered reference chains",
                doc,
            ));
        }
    }
    for (name, doc) in cases {
        let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(compiled.has_guide("dev.example.guide"), "{name}");
        assert!(
            !compiled.warnings.iter().any(|w| w.contains("guide_shape")),
            "{name}"
        );
        for (direction, value) in [
            ("request", json!({})),
            ("response", json!({"markdown":"ok"})),
        ] {
            assert!(
                compiled
                    .validate("dev.example.guide", "runtime.guide", direction, &value)
                    .is_ok(),
                "{name}: {value}"
            );
        }
        for (direction, value) in [
            ("request", json!({"extra":true})),
            ("request", json!(null)),
            ("request", json!([])),
            ("response", json!(null)),
            ("response", json!([])),
            ("response", json!({})),
            ("response", json!({"markdown":"ok","extra":true})),
            ("response", json!({"markdown":"x".repeat(101)})),
            ("response", json!({"markdown":null})),
            ("response", json!({"markdown":1})),
        ] {
            assert!(
                compiled
                    .validate("dev.example.guide", "runtime.guide", direction, &value)
                    .is_err(),
                "{name}: {value}"
            );
        }
        let projection = compiled
            .project_metadata("dev.example.guide", None)
            .unwrap();
        let projected: Value = serde_json::from_slice(&projection.bytes).unwrap();
        for (definition, schema) in projected["$defs"].as_object().unwrap() {
            assert_eq!(schema, &doc["$defs"][definition], "{name}: {definition}");
        }
        assert_eq!(projected["contracts"], doc["contracts"], "{name}");
        let rebuilt = compile_port_contracts(&projection.bytes).unwrap();
        assert!(rebuilt.has_guide("dev.example.guide"), "{name}");
        let again = rebuilt.project_metadata("dev.example.guide", None).unwrap();
        assert_eq!(projection.bytes, again.bytes, "{name}");
        assert_eq!(projection.sha256, again.sha256, "{name}");
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_numeric_overlap_preserves_integer_alternatives_at_every_marker() {
    for (definition, marker, direction) in [
        ("markdown", "string", "response"),
        ("empty", "object", "request"),
        ("response", "object", "response"),
    ] {
        for (direct, referenced) in [("number", "integer"), ("integer", "number")] {
            let mut doc = reference_sibling_guide(json!({"$ref":"#/$defs/markdown"}));
            doc["$defs"]["markdown"] = json!({"type":"string","maxLength":100});
            doc["$defs"]["alternative"] = json!({"type":[referenced,marker]});
            doc["$defs"][definition]["$ref"] = json!("#/$defs/alternative");
            doc["$defs"][definition]["type"] = json!([direct, marker]);
            let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
            let value = |v: Value| {
                if definition == "markdown" {
                    json!({"markdown":v})
                } else {
                    v
                }
            };
            for v in [json!(7), json!("ok"), json!(1.5)] {
                let expected = v == json!(7) || (definition == "markdown" && v == json!("ok"));
                assert_eq!(
                    compiled
                        .validate(
                            "dev.example.guide",
                            "runtime.guide",
                            direction,
                            &value(v.clone())
                        )
                        .is_ok(),
                    expected,
                    "{definition}/{direct}: {v}"
                );
            }
            let valid_marker = if direction == "request" {
                json!({})
            } else {
                json!({"markdown":"ok"})
            };
            assert!(
                compiled
                    .validate(
                        "dev.example.guide",
                        "runtime.guide",
                        direction,
                        &valid_marker
                    )
                    .is_ok()
            );
            assert!(
                !compiled.has_guide("dev.example.guide"),
                "{definition}/{direct}"
            );
            assert!(compiled.warnings.iter().any(|w| w.contains("guide_shape")));
            let projection = compiled
                .project_metadata("dev.example.guide", None)
                .unwrap();
            let projected: Value = serde_json::from_slice(&projection.bytes).unwrap();
            for (name, schema) in projected["$defs"].as_object().unwrap() {
                assert_eq!(schema, &doc["$defs"][name]);
            }
            assert_eq!(projected["contracts"], doc["contracts"]);
            let rebuilt = compile_port_contracts(&projection.bytes).unwrap();
            assert!(!rebuilt.has_guide("dev.example.guide"));
            let again = rebuilt.project_metadata("dev.example.guide", None).unwrap();
            assert_eq!(projection.bytes, again.bytes);
            assert_eq!(projection.sha256, again.sha256);
        }
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_primitive_intersections_agree_with_actual_validator_witnesses() {
    // Explicit witnesses cover all seven primitive names and the sole subtype relation.
    // Each clause also admits string. No expected result computes an intersection.
    let samples = [
        json!(null),
        json!(true),
        json!([]),
        json!({}),
        json!(7),
        json!(1.5),
    ];
    for (left, right, accepted) in [
        ("number", "integer", vec![json!(7)]),
        ("number", "number", vec![json!(7), json!(1.5)]),
        ("integer", "integer", vec![json!(7)]),
        ("null", "null", vec![json!(null)]),
        ("boolean", "boolean", vec![json!(true)]),
        ("array", "array", vec![json!([])]),
        ("object", "object", vec![json!({})]),
        ("string", "number", vec![]),
        ("number", "boolean", vec![]),
        ("integer", "null", vec![]),
        ("array", "object", vec![]),
    ] {
        for (direct, referenced) in [(left, right), (right, left)] {
            let mut doc = reference_sibling_guide(
                json!({"$ref":"#/$defs/nested","type":[direct,"string"],"maxLength":100}),
            );
            doc["$defs"]["text"]["type"] = if referenced == "string" {
                json!("string")
            } else {
                json!([referenced, "string"])
            };
            // Avoid a duplicate string type in the direct clause.
            if direct == "string" {
                doc["$defs"]["response"]["properties"]["markdown"]["type"] = json!("string");
            }
            let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
            for sample in &samples {
                assert_eq!(
                    compiled
                        .validate(
                            "dev.example.guide",
                            "runtime.guide",
                            "response",
                            &json!({"markdown":sample})
                        )
                        .is_ok(),
                    accepted.contains(sample),
                    "{direct}/{referenced}: {sample}"
                );
            }
            assert!(
                compiled
                    .validate(
                        "dev.example.guide",
                        "runtime.guide",
                        "response",
                        &json!({"markdown":"ok"})
                    )
                    .is_ok()
            );
            assert_eq!(
                compiled.has_guide("dev.example.guide"),
                accepted.is_empty(),
                "{direct}/{referenced}"
            );
            assert_eq!(
                compiled.warnings.iter().any(|w| w.contains("guide_shape")),
                !accepted.is_empty()
            );
        }
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_finite_type_sets_reject_nullable_multiple_disjoint_and_false_markers() {
    for markdown in [
        json!({"type":["string","null"],"maxLength":100}),
        json!({"type":["boolean","string"],"maxLength":100}),
        json!({"$ref":"#/$defs/nullable","type":["null","string"],"maxLength":100}),
        json!({"$ref":"#/$defs/nullable","type":["integer","boolean"],"maxLength":100}),
        json!({"type":["number","integer"],"maxLength":100}),
        json!({"$ref":"#/$defs/never","type":["string"],"maxLength":100}),
        json!({"maxLength":100}),
    ] {
        let mut doc = reference_sibling_guide(markdown.clone());
        doc["$defs"]["nullable"] = json!({"type":["string","null"]});
        doc["$defs"]["never"] = json!(false);
        let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(!compiled.has_guide("dev.example.guide"), "{markdown}");
        assert!(
            compiled.warnings.iter().any(|w| w.contains("guide_shape")),
            "{markdown}"
        );
    }
    for name in ["empty", "response"] {
        for types in [
            json!(["null", "object"]),
            json!(["array", "object"]),
            json!(["string", "boolean"]),
        ] {
            let mut doc = reference_sibling_guide(json!({"type":["string"],"maxLength":100}));
            doc["$defs"][name]["type"] = types;
            let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
            assert!(!compiled.has_guide("dev.example.guide"), "{name}");
            assert!(compiled.warnings.iter().any(|w| w.contains("guide_shape")));
        }
    }
    let mut extra = reference_sibling_guide(json!({"type":["string"],"maxLength":100}));
    extra["$defs"]["empty"]["type"] = json!(["object"]);
    extra["$defs"]["empty"]["properties"] = json!({"extra":{"type":"string"}});
    let compiled = compile_port_contracts(&serde_json::to_vec(&extra).unwrap()).unwrap();
    assert!(!compiled.has_guide("dev.example.guide"));
    assert!(
        compiled
            .validate(
                "dev.example.guide",
                "runtime.guide",
                "request",
                &json!({"extra":"accepted"})
            )
            .is_ok()
    );
    // Exact finite markers do not replace the actual compiled empty-request check.
    extra["$defs"]["empty"]
        .as_object_mut()
        .unwrap()
        .remove("properties");
    extra["$defs"]["empty"]["minProperties"] = json!(1);
    let compiled = compile_port_contracts(&serde_json::to_vec(&extra).unwrap()).unwrap();
    assert!(!compiled.has_guide("dev.example.guide"));
    assert!(
        compiled
            .validate("dev.example.guide", "runtime.guide", "request", &json!({}))
            .is_err()
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_reference_siblings_preserve_bounds_and_runtime_validation() {
    for markdown in [
        json!({"type":"string","maxLength":100}),
        json!({"$ref":"#/$defs/literalBounded"}),
        json!({"$ref":"#/$defs/bounded"}),
        json!({"$ref":"#/$defs/text","maxLength":100}),
        json!({"$ref":"#/$defs/nested","maxLength":3}),
    ] {
        let doc = reference_sibling_guide(markdown.clone());
        let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(compiled.has_guide("dev.example.guide"), "{markdown}");
        assert!(!compiled.warnings.iter().any(|w| w.contains("guide_shape")));
        assert!(
            compiled
                .validate(
                    "dev.example.guide",
                    "runtime.guide",
                    "response",
                    &json!({"markdown":"ok"})
                )
                .is_ok()
        );
        for value in [json!({"markdown":"x".repeat(101)}), json!({"markdown":7})] {
            assert!(
                compiled
                    .validate("dev.example.guide", "runtime.guide", "response", &value)
                    .is_err()
            );
        }
        let projected = compiled
            .project_metadata("dev.example.guide", None)
            .unwrap();
        let rebuilt = compile_port_contracts(&projected.bytes).unwrap();
        assert!(rebuilt.has_guide("dev.example.guide"));
        assert_eq!(
            projected.bytes,
            rebuilt
                .project_metadata("dev.example.guide", None)
                .unwrap()
                .bytes
        );
        assert_eq!(
            projected.sha256,
            rebuilt
                .project_metadata("dev.example.guide", None)
                .unwrap()
                .sha256
        );
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_reference_siblings_reject_type_boolean_and_length_contradictions() {
    for markdown in [
        json!({"$ref":"#/$defs/literalBounded","type":"integer"}),
        json!({"$ref":"#/$defs/nested","maxLength":1}),
        json!({"$ref":"#/$defs/never","type":"string","maxLength":100}),
    ] {
        let mut doc = reference_sibling_guide(markdown.clone());
        doc["$defs"]["never"] = json!(false);
        let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(!compiled.has_guide("dev.example.guide"), "{markdown}");
        assert!(compiled.warnings.iter().any(|w| w.contains("guide_shape")));
        assert!(
            compiled
                .validate(
                    "dev.example.guide",
                    "runtime.guide",
                    "response",
                    &json!({"markdown":"ok"})
                )
                .is_err()
        );
    }
    for direction in ["request", "response"] {
        let mut doc = reference_sibling_guide(json!({"$ref":"#/$defs/bounded"}));
        let definition = if direction == "request" {
            "empty"
        } else {
            "response"
        };
        doc["contracts"][0]["ports"][0][direction] =
            json!({"$ref":format!("#/$defs/{definition}"),"type":"string"});
        let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert!(!compiled.has_guide("dev.example.guide"), "{direction}");
    }
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_reference_siblings_conjoin_object_properties_required_and_closure() {
    let mut doc = reference_sibling_guide(json!({"$ref":"#/$defs/bounded"}));
    doc["$defs"]["response"]
        .as_object_mut()
        .unwrap()
        .remove("required");
    doc["$defs"]["response"]
        .as_object_mut()
        .unwrap()
        .remove("additionalProperties");
    doc["contracts"][0]["ports"][0]["response"] = json!({"$ref":"#/$defs/response","required":["markdown"],"additionalProperties":false,"properties":{"markdown":{"minLength":2}}});
    // Closure belongs to each schema location: the sibling must declare markdown too.
    let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(compiled.has_guide("dev.example.guide"));
    assert!(
        compiled
            .validate(
                "dev.example.guide",
                "runtime.guide",
                "response",
                &json!({"markdown":"ok"})
            )
            .is_ok()
    );
    assert!(
        compiled
            .validate(
                "dev.example.guide",
                "runtime.guide",
                "response",
                &json!({"markdown":"x"})
            )
            .is_err()
    );
    doc["contracts"][0]["ports"][0]["response"]["properties"]["markdown"]["type"] =
        json!("integer");
    assert!(
        !compile_port_contracts(&serde_json::to_vec(&doc).unwrap())
            .unwrap()
            .has_guide("dev.example.guide")
    );
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_reference_sibling_pattern_properties_does_not_demote_valid_marker() {
    let mut doc = reference_sibling_guide(json!({"$ref":"#/$defs/bounded"}));
    doc["contracts"][0]["ports"][0]["response"] = json!({"$ref":"#/$defs/response","additionalProperties":false,"patternProperties":{"^markdown$":true}});
    let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(
        compiled
            .validate(
                "dev.example.guide",
                "runtime.guide",
                "response",
                &json!({"markdown":"ok"})
            )
            .is_ok()
    );
    assert!(compiled.has_guide("dev.example.guide"));
}

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn guide_reference_siblings_preserve_effectively_empty_request() {
    let mut doc = reference_sibling_guide(json!({"$ref":"#/$defs/bounded"}));
    doc["contracts"][0]["ports"][0]["request"] =
        json!({"$ref":"#/$defs/empty","properties":{"input":{"type":"string"}}});
    let compiled = compile_port_contracts(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(
        compiled
            .validate("dev.example.guide", "runtime.guide", "request", &json!({}))
            .is_ok()
    );
    assert!(
        compiled
            .validate(
                "dev.example.guide",
                "runtime.guide",
                "request",
                &json!({"input":"not accepted"})
            )
            .is_err()
    );
    assert!(compiled.has_guide("dev.example.guide"));
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

// Verifies: TEAM.SYS.PKG-R13
#[test]
fn capability_wire_port_names_match_compiler_ascii_grammar_and_length() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/package-capabilities/v1/package-ports-2.0.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema["$defs"]["port"]).unwrap();
    for (name, expected) in [
        ("message.read-all".to_owned(), true),
        ("a".to_owned(), true),
        (format!("a{}", "-".repeat(127)), true),
        (format!("a{}", "_".repeat(128)), false),
        ("".to_owned(), false),
        ("message..read".to_owned(), false),
        ("message.Read".to_owned(), false),
        ("message.0read".to_owned(), false),
        ("message.-read".to_owned(), false),
        ("message.read.".to_owned(), false),
        ("message.réad".to_owned(), false),
        ("message.read\n".to_owned(), false),
    ] {
        assert_eq!(
            teamarness_package_contract::ui_contract::valid_operation_name(&name),
            expected,
            "compiler {name:?}"
        );
        assert_eq!(validator.is_valid(&json!(name)), expected, "wire {name:?}");
    }
}
