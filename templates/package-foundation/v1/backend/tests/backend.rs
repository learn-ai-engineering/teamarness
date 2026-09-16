use std::sync::Arc;

use teamarness_generated_package_backend::GeneratedBackend;
use teamarness_package_protocol::{Frame, FrameKind, decode_frame};
use teamarness_package_sdk::Package;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

// Token substitution changes the literal width, so rustfmt must leave this declaration stable.
#[rustfmt::skip]
const EXPECTED_DISPLAY_NAME: &str = __PACKAGE_DISPLAY_NAME_RUST__;

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
) -> Frame {
    write_value(
        host_write,
        serde_json::json!({
            "kind": "request",
            "id": id,
            "operation": operation,
            "deadlineUnixMs": "9999999999999",
            "payload": {}
        }),
    )
    .await;
    read_frame(host_read).await
}

#[tokio::test]
async fn foundation_backend_handles_its_declared_request_through_the_sdk() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service =
        tokio::spawn(Package::builder(Arc::new(GeneratedBackend)).serve_io(sdk_read, sdk_write));

    write_value(
        &mut host_write,
        serde_json::json!({
            "kind": "initialize",
            "nonce": "bm9uY2U",
            "package": {
                "identity": "__PACKAGE_IDENTITY__",
                "version": "1.0.0",
                "manifestDigest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "backendDigest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            },
            "uiContractDigest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "processGeneration": "1",
            "workspace": {"id": "workspace1", "name": "Workspace"},
            "installation": {"identity": "installation1", "displayName": "Installation"},
            "dataDirectory": {"path": "/tmp", "continuityId": "Y29udGludWl0eQ"},
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

    let response = request(
        &mut host_write,
        &mut host_read,
        "h:foundation-read",
        "__REQUEST_NAMESPACE__.read",
    )
    .await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(
        response.field("payload").unwrap()["message"],
        "Hello from __REQUEST_NAMESPACE__"
    );
    assert_eq!(
        response.field("payload").unwrap()["displayName"],
        EXPECTED_DISPLAY_NAME
    );

    let unknown = request(
        &mut host_write,
        &mut host_read,
        "h:foundation-unknown",
        "__REQUEST_NAMESPACE__.unknown",
    )
    .await;
    assert_eq!(unknown.kind(), FrameKind::Error);
    assert_eq!(unknown.field("code").unwrap(), "package.unknown_operation");
    assert_eq!(unknown.field("retryable").unwrap(), false);

    write_value(
        &mut host_write,
        serde_json::json!({"kind": "shutdown", "reason": "test_complete"}),
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}
