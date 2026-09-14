use std::{collections::BTreeSet, fs, path::PathBuf};

use serde_json::Value;
use teamarness_package_protocol::{
    ContractState, Direction, FrameDecoder, MAX_FRAME_BYTES, ProtocolError, ProtocolRange,
    ProtocolVersion, decode_frame, decode_payload, negotiate_version,
};

fn contract_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/package-process/v1")
}

fn json_files(directory: &str, extension: &str) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(contract_root().join(directory))
        .expect("contract directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn observe_sequence(value: &Value) -> Result<(), ProtocolError> {
    let frames = value
        .as_array()
        .or_else(|| value.get("frames").and_then(Value::as_array))
        .expect("fixture sequence");
    let mut state = ContractState::default();
    for entry in frames {
        let direction = match entry["direction"].as_str().expect("direction") {
            "host" => Direction::HostToPackage,
            "package" => Direction::PackageToHost,
            other => panic!("unknown direction {other}"),
        };
        let bytes = serde_json::to_vec(&entry["frame"]).expect("frame JSON");
        let frame = decode_payload(&bytes)?;
        state.observe(direction, &frame)?;
    }
    Ok(())
}

#[test]
fn consumes_every_golden_sequence() {
    let paths = json_files("golden", "jsonl");
    assert_eq!(paths.len(), 8, "the v1 golden set is intentional");

    let mut kinds = BTreeSet::new();
    for path in paths {
        let entries = fs::read_to_string(&path)
            .expect("golden fixture")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("fixture JSON"))
            .collect::<Vec<_>>();
        for entry in &entries {
            kinds.insert(entry["frame"]["kind"].as_str().unwrap().to_owned());
        }
        observe_sequence(&Value::Array(entries))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }

    assert_eq!(
        kinds,
        [
            "cancel",
            "credit",
            "error",
            "event",
            "initialize",
            "initialized",
            "ping",
            "pong",
            "request",
            "response",
            "shutdown",
            "shutdown_ack",
            "stream_close",
            "stream_open",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
}

#[test]
fn consumes_every_malformed_fixture_with_its_stable_error() {
    let paths = json_files("invalid", "json");
    assert_eq!(paths.len(), 14, "the v1 malformed set is intentional");

    for path in paths {
        let fixture: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let result = match fixture["mode"].as_str().unwrap() {
            "payload" => decode_payload(fixture["payload"].as_str().unwrap().as_bytes()).map(drop),
            "frame_hex" => decode_frame(&decode_hex(fixture["hex"].as_str().unwrap())).map(drop),
            "length" => {
                let length = u32::try_from(fixture["length"].as_u64().unwrap()).unwrap();
                decode_frame(&length.to_be_bytes()).map(drop)
            }
            "sequence" => observe_sequence(&fixture),
            mode => panic!("unsupported fixture mode {mode}"),
        };
        let error = result.unwrap_err();
        assert_eq!(
            error.code(),
            fixture["expectedError"].as_str().unwrap(),
            "{}",
            path.display()
        );
    }
}

#[test]
fn framing_round_trips_at_the_boundary_and_rejects_truncation() {
    let payload = br#"{"kind":"ping","nonce":"cm91bmRfdHJpcA"}"#;
    let mut encoded = (payload.len() as u32).to_be_bytes().to_vec();
    encoded.extend_from_slice(payload);
    assert_eq!(decode_frame(&encoded).unwrap().kind().as_str(), "ping");

    let truncated = &encoded[..encoded.len() - 1];
    assert_eq!(
        decode_frame(truncated).unwrap_err().code(),
        "truncated_frame"
    );
    assert_eq!(MAX_FRAME_BYTES, 1_048_576);
}

#[test]
fn incremental_decoder_handles_fragmented_and_back_to_back_frames() {
    let frames = [
        br#"{"kind":"ping","nonce":"b25l"}"#.as_slice(),
        br#"{"kind":"pong","nonce":"dHdv"}"#.as_slice(),
    ];
    let mut bytes = Vec::new();
    for payload in frames {
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
    }

    let mut decoder = FrameDecoder::default();
    decoder.push(&bytes[..3]).unwrap();
    assert!(decoder.next_frame().unwrap().is_none());
    decoder.push(&bytes[3..]).unwrap();
    assert_eq!(
        decoder.next_frame().unwrap().unwrap().kind().as_str(),
        "ping"
    );
    assert_eq!(
        decoder.next_frame().unwrap().unwrap().kind().as_str(),
        "pong"
    );
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
fn negotiates_the_highest_shared_minor_with_one_exact_major() {
    let selected = negotiate_version(
        ProtocolRange::new(1, 0, 4).unwrap(),
        ProtocolRange::new(1, 2, 3).unwrap(),
    )
    .unwrap();
    assert_eq!(selected, ProtocolVersion::new(1, 3));

    let error = negotiate_version(
        ProtocolRange::new(1, 0, 1).unwrap(),
        ProtocolRange::new(2, 0, 1).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "incompatible_version");
}

#[test]
fn every_schema_is_local_and_every_frame_kind_has_a_schema() {
    let schemas = json_files("schemas", "json");
    assert_eq!(schemas.len(), 19);
    let frame_schema: Value = serde_json::from_slice(
        &fs::read(contract_root().join("schemas/frame.schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(frame_schema["oneOf"].as_array().unwrap().len(), 14);

    for path in schemas {
        let schema: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema",
            "{}",
            path.display()
        );
        let serialized = serde_json::to_string(&schema).unwrap();
        assert!(!serialized.contains("http://"));
        for reference in references(&schema) {
            assert!(
                reference.starts_with('#') || reference.starts_with("frame.schema.json#"),
                "remote or file retrieval is forbidden in {}: {reference}",
                path.display()
            );
        }
    }
}

#[test]
fn protocol_crate_and_contract_keep_the_package_boundary() {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in [
        "teamarness-core",
        "tauri",
        "iroh",
        "p2panda",
        "rusqlite",
        "jsonschema",
        "teamarness-package-sdk",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "forbidden dependency: {forbidden}"
        );
    }

    for directory in ["schemas", "golden", "invalid", "fixtures"] {
        for entry in fs::read_dir(contract_root().join(directory)).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let text = fs::read_to_string(&path).unwrap_or_default().to_lowercase();
                for forbidden in ["conversations", "synchronization"] {
                    assert!(
                        !text.contains(forbidden),
                        "{forbidden} in {}",
                        path.display()
                    );
                }
            }
        }
    }
}

fn references(value: &Value) -> Vec<&str> {
    match value {
        Value::Object(object) => object
            .iter()
            .flat_map(|(key, value)| {
                let mut found = Vec::new();
                if key == "$ref" {
                    found.push(value.as_str().unwrap());
                }
                found.extend(references(value));
                found
            })
            .collect(),
        Value::Array(values) => values.iter().flat_map(references).collect(),
        _ => Vec::new(),
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}
