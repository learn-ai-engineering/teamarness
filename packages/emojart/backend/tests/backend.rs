use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use teamarness_emojart_package::{EmojartBackend, SeedError, SeedSource};
use teamarness_package_protocol::{Frame, FrameKind, decode_frame};
use teamarness_package_sdk::Package;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

struct FixedSeeds(Mutex<VecDeque<u32>>);

impl FixedSeeds {
    fn new(seeds: impl IntoIterator<Item = u32>) -> Self {
        Self(Mutex::new(seeds.into_iter().collect()))
    }
}

impl SeedSource for FixedSeeds {
    fn next_seed(&self) -> Result<u32, SeedError> {
        self.0.lock().unwrap().pop_front().ok_or(SeedError)
    }
}

async fn write_value(stream: &mut DuplexStream, value: serde_json::Value) {
    let payload = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    stream.write_all(&payload).await.unwrap();
}

async fn read_frame(stream: &mut DuplexStream) -> Frame {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    decode_frame(&bytes).unwrap()
}

async fn request(
    host_write: &mut DuplexStream,
    host_read: &mut DuplexStream,
    id: &str,
    operation: &str,
    payload: serde_json::Value,
) -> Frame {
    write_value(
        host_write,
        serde_json::json!({
            "kind": "request",
            "id": id,
            "operation": operation,
            "deadlineUnixMs": "9999999999999",
            "payload": payload
        }),
    )
    .await;
    read_frame(host_read).await
}

#[tokio::test]
async fn backend_runs_the_request_and_lifecycle_contract_with_stable_errors() {
    let root = tempfile::tempdir().unwrap();
    let backend = EmojartBackend::with_seed_source(FixedSeeds::new([7, 91]));
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(Arc::new(backend)).serve_io(sdk_read, sdk_write));

    write_value(
        &mut host_write,
        serde_json::json!({
            "kind": "initialize",
            "nonce": "bm9uY2U",
            "package": {
                "identity": "dev.teamarness.emojart",
                "version": "1.0.0",
                "manifestDigest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "backendDigest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            },
            "uiContractDigest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "processGeneration": "1",
            "workspace": {"id": "workspace1", "name": "Workspace"},
            "installation": {"identity": "installation1", "displayName": "Installation"},
            "dataDirectory": {"path": root.path(), "continuityId": "Y29udGludWl0eQ"},
            "protocol": {"major": 1, "minor": 0},
            "capabilities": [],
            "limits": {
                "maxFrameBytes": 1048576,
                "maxInFlightEachDirection": 32,
                "maxWriterFrames": 64,
                "maxWriterBytes": 8388608,
                "initialStreamCredit": 32
            }
        }),
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );

    let inactive = request(
        &mut host_write,
        &mut host_read,
        "h:inactive",
        "emojart.read",
        serde_json::json!({}),
    )
    .await;
    assert_error(&inactive, "emojart.not_active", true);

    assert_response(
        &request(
            &mut host_write,
            &mut host_read,
            "h:activate",
            "lifecycle.activate",
            serde_json::json!({}),
        )
        .await,
    );

    let read = request(
        &mut host_write,
        &mut host_read,
        "h:read",
        "emojart.read",
        serde_json::json!({}),
    )
    .await;
    assert_response(&read);
    let read_payload = read.field("payload").unwrap();
    assert_eq!(read_payload["algorithms"].as_array().unwrap().len(), 3);
    assert_eq!(read_payload["palettes"].as_array().unwrap().len(), 4);
    assert_eq!(read_payload["artworks"], serde_json::json!([]));

    let first = request(
        &mut host_write,
        &mut host_read,
        "h:generate-1",
        "emojart.generate",
        serde_json::json!({"algorithm": "waves", "palette": "garden"}),
    )
    .await;
    let second = request(
        &mut host_write,
        &mut host_read,
        "h:generate-2",
        "emojart.generate",
        serde_json::json!({"algorithm": "waves", "palette": "garden"}),
    )
    .await;
    assert_eq!(first.field("payload").unwrap()["recipe"]["seed"], 7);
    assert_eq!(second.field("payload").unwrap()["recipe"]["seed"], 91);
    assert_ne!(
        first.field("payload").unwrap()["cells"],
        second.field("payload").unwrap()["cells"]
    );

    let saved = request(
        &mut host_write,
        &mut host_read,
        "h:save",
        "emojart.save",
        serde_json::json!({"recipe": first.field("payload").unwrap()["recipe"]}),
    )
    .await;
    assert_response(&saved);
    assert_eq!(saved.field("payload").unwrap()["id"], "artwork-1");
    assert_eq!(
        saved.field("payload").unwrap()["cells"],
        first.field("payload").unwrap()["cells"]
    );

    let invalid = request(
        &mut host_write,
        &mut host_read,
        "h:invalid",
        "emojart.generate",
        serde_json::json!({"algorithm": "unknown", "palette": "garden"}),
    )
    .await;
    assert_error(&invalid, "emojart.unknown_algorithm", false);

    let unknown = request(
        &mut host_write,
        &mut host_read,
        "h:unknown",
        "emojart.delete",
        serde_json::json!({}),
    )
    .await;
    assert_error(&unknown, "emojart.unknown_operation", false);

    assert_response(
        &request(
            &mut host_write,
            &mut host_read,
            "h:suspend",
            "lifecycle.suspend",
            serde_json::json!({}),
        )
        .await,
    );
    assert_response(
        &request(
            &mut host_write,
            &mut host_read,
            "h:dispose",
            "lifecycle.dispose",
            serde_json::json!({}),
        )
        .await,
    );
    let disposed = request(
        &mut host_write,
        &mut host_read,
        "h:disposed",
        "emojart.read",
        serde_json::json!({}),
    )
    .await;
    assert_error(&disposed, "emojart.not_active", true);

    assert_response(
        &request(
            &mut host_write,
            &mut host_read,
            "h:recover",
            "lifecycle.recover",
            serde_json::json!({}),
        )
        .await,
    );
    let recovered = request(
        &mut host_write,
        &mut host_read,
        "h:recovered",
        "emojart.read",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        recovered.field("payload").unwrap()["artworks"][0]["id"],
        "artwork-1"
    );

    write_value(
        &mut host_write,
        serde_json::json!({"kind": "shutdown", "reason": "application_shutdown"}),
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}

fn assert_response(frame: &Frame) {
    assert_eq!(frame.kind(), FrameKind::Response);
    assert!(frame.field("payload").unwrap().is_object());
}

fn assert_error(frame: &Frame, code: &str, retryable: bool) {
    assert_eq!(frame.kind(), FrameKind::Error);
    assert_eq!(frame.field("code").unwrap(), code);
    assert_eq!(frame.field("retryable").unwrap(), retryable);
}
