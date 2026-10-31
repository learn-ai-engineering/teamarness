use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use teamarness_package_sdk::{
    Empty, Router,
    ports::{PortAccess, PortClient},
    testing::{HarnessBuilder, ScriptedCapability},
};

const DOCUMENT: &[u8] = br##"{"schemaVersion":1,"$defs":{"text":{"type":"string"}},"contracts":[{"identity":"dev.example.metadata","major":1,"minor":2,"description":"Metadata","ports":[{"name":"value.read","kind":"query","description":"Read","request":true,"response":{"$ref":"#/$defs/text"}}]}]}"##;
const DIGEST: &str = "0de9112f997e9a8350b7bef09d9331815451382033e418f3954c614a42fdbbc7";

async fn describe_with_digest(
    chunks: Vec<Value>,
    digest: &str,
) -> Result<Value, teamarness_package_sdk::BackendError> {
    let expected_digest = digest.to_owned();
    let digest = digest.to_owned();
    let chunks = std::sync::Arc::new(std::sync::Mutex::new(chunks.into_iter()));
    let scripted = ScriptedCapability::new().calls(move |method, payload| {
        assert_eq!(method, "describe");
        assert_eq!(payload["access"], "declared");
        assert_eq!(payload["contract"], "dev.example.metadata");
        assert_eq!(payload["metadataSha256"], expected_digest);
        Ok(chunks.lock().unwrap().next().expect("bounded requests"))
    });
    let router = Router::stateless().request("metadata.read", move |context, _: Empty| { let digest = digest.clone(); async move {
        let client = PortClient::new(context.capabilities().clone(), context.initialization().capability("teamarness.package-ports").unwrap().grant.clone());
        let document = client.describe(PortAccess::Declared, "dev.example.metadata", &digest).await?;
        Ok(json!({"identity":document.contracts[0].identity,"schema":document.contracts[0].ports[0].response,"definitions":document.definitions}))
    }});
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .capability("teamarness.package-ports", scripted)
        .start()
        .await;
    harness.activate().await.unwrap();
    let result = harness.request("metadata.read", json!({})).await;
    harness.shutdown().await.unwrap();
    result
}

async fn describe(chunks: Vec<Value>) -> Result<Value, teamarness_package_sdk::BackendError> {
    describe_with_digest(chunks, DIGEST).await
}

fn chunk(bytes: &[u8], offset: usize, total: usize, next: Option<usize>) -> Value {
    let mut value = json!({"metadataSha256":DIGEST,"offset":offset,"totalBytes":total,"data":STANDARD.encode(bytes)});
    if let Some(next) = next {
        value["nextOffset"] = json!(next);
    }
    value
}

#[tokio::test]
async fn assembles_independent_complete_metadata_and_preserves_definitions() {
    let result = describe(vec![
        chunk(&DOCUMENT[..40], 0, DOCUMENT.len(), Some(40)),
        chunk(&DOCUMENT[40..], 40, DOCUMENT.len(), None),
    ])
    .await
    .unwrap();
    assert_eq!(
        result,
        json!({"identity":"dev.example.metadata","schema":{"$ref":"#/$defs/text"},"definitions":{"text":{"type":"string"}}})
    );
}

#[tokio::test]
async fn rejects_corrupt_chunks_lengths_digests_and_documents() {
    let valid = chunk(DOCUMENT, 0, DOCUMENT.len(), None);
    let mut cases = Vec::new();
    for (key, value) in [
        ("data", json!("!!==")),
        ("offset", json!(1)),
        ("totalBytes", json!(1024 * 1024 + 1)),
        ("totalBytes", json!(DOCUMENT.len() + 1)),
        ("metadataSha256", json!("0".repeat(64))),
        ("nextOffset", json!(0)),
        ("nextOffset", Value::Null),
        ("nextOffset", json!(DOCUMENT.len() + 1)),
    ] {
        let mut bad = valid.clone();
        bad[key] = value;
        cases.push(vec![bad]);
    }
    cases.push(vec![chunk(
        &vec![b' '; 64 * 1024 + 1],
        0,
        64 * 1024 + 1,
        None,
    )]);
    cases.push(vec![chunk(b"not JSON", 0, 8, None)]);
    cases.push(vec![chunk(
        DOCUMENT,
        0,
        DOCUMENT.len(),
        Some(DOCUMENT.len()),
    )]);
    cases.push(vec![
        chunk(&DOCUMENT[..40], 0, DOCUMENT.len(), Some(40)),
        chunk(&DOCUMENT[40..], 41, DOCUMENT.len(), None),
    ]);
    cases.push(vec![
        chunk(&DOCUMENT[..40], 0, DOCUMENT.len(), Some(40)),
        chunk(&DOCUMENT[40..], 40, DOCUMENT.len() + 1, None),
    ]);
    let mut wrong_bytes = DOCUMENT.to_vec();
    wrong_bytes[0] = b' ';
    cases.push(vec![chunk(&wrong_bytes, 0, DOCUMENT.len(), None)]);
    for case in cases {
        let error = describe(case).await.unwrap_err();
        assert_eq!(error.code(), "package.invalid_capability_response");
    }
}

#[tokio::test]
async fn rejects_malformed_documents_and_dangling_references_after_digest_verification() {
    let bytes = br##"not JSON"##;
    let digest = "62b8125a6f6d924ec53345b5fcd58ca3ed3f5e7d51e2e146e5f1346508acce69";
    let mut response = chunk(bytes, 0, bytes.len(), None);
    response["metadataSha256"] = json!(digest);
    assert_eq!(
        describe_with_digest(vec![response], digest)
            .await
            .unwrap_err()
            .code(),
        "package.invalid_capability_response"
    );
    let bytes = br##"{"schemaVersion":1,"$defs":{},"contracts":[{"identity":"dev.example.metadata","major":1,"minor":0,"description":"Metadata","ports":[{"name":"value.read","kind":"query","description":"Read","request":true,"response":{"$ref":"#/$defs/missing"}}]}]}"##;
    let digest = "fc2a44131147e6c4d8c9a683a606c2ef223a7a4203a4817b14267784cdfc34de";
    let mut response = chunk(bytes, 0, bytes.len(), None);
    response["metadataSha256"] = json!(digest);
    assert_eq!(
        describe_with_digest(vec![response], digest)
            .await
            .unwrap_err()
            .code(),
        "package.invalid_capability_response"
    );
}

async fn list(pages: Vec<Value>) -> Result<Value, teamarness_package_sdk::BackendError> {
    let pages = std::sync::Arc::new(std::sync::Mutex::new(pages.into_iter()));
    let scripted = ScriptedCapability::new().calls(move |method, payload| {
        assert_eq!(method, "available");
        assert_eq!(payload["access"], "declared");
        Ok(pages
            .lock()
            .unwrap()
            .next()
            .expect("bounded page traversal"))
    });
    let router = Router::stateless().request("metadata.list", |context, _: Empty| async move {
        let grant = context
            .initialization()
            .capability("teamarness.package-ports")
            .unwrap()
            .grant
            .clone();
        let rows = PortClient::new(context.capabilities().clone(), grant)
            .declared_contracts()
            .await?;
        Ok(json!({"rows":rows}))
    });
    let directory = tempfile::tempdir().unwrap();
    let mut harness = HarnessBuilder::new(router.into_backend())
        .data_directory(directory.path())
        .capability("teamarness.package-ports", scripted)
        .start()
        .await;
    harness.activate().await.unwrap();
    let result = harness.request("metadata.list", json!({})).await;
    harness.shutdown().await.unwrap();
    result
}

#[tokio::test]
async fn follows_declared_pages_and_refuses_incomplete_or_unbounded_summaries() {
    let row = json!({"contract":"dev.example.alpha","major":1,"minor":null,"description":null,"available":false,"enabled":false,"enabledMajor":null,"metadataSha256":null});
    let mut second = row.clone();
    second["contract"] = json!("dev.example.beta");
    let result = list(vec![
        json!({"contracts":[row],"nextCursor":"opaque"}),
        json!({"contracts":[second]}),
    ])
    .await
    .unwrap();
    assert_eq!(result["rows"].as_array().unwrap().len(), 2);
    assert_eq!(result["rows"][1]["contract"], "dev.example.beta");
    let mut incomplete = row.clone();
    incomplete.as_object_mut().unwrap().remove("description");
    for page in [
        json!({"contracts":[incomplete]}),
        json!({"contracts":vec![row.clone();33]}),
        json!({"contracts":[],"nextCursor":"opaque"}),
    ] {
        assert_eq!(
            list(vec![page]).await.unwrap_err().code(),
            "package.invalid_capability_response"
        );
    }
}
