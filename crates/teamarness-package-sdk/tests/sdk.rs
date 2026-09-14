use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use teamarness_package_protocol::{Frame, FrameKind, JsonValue, decode_frame};
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, EventSubscription, Package, event_channel,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Echo;

impl Backend for Echo {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            if request.operation == "fixture.capability" {
                return request
                    .capabilities
                    .call("Z3JhbnQx", "get", JsonValue::Object(Default::default()))
                    .await;
            }
            if request.operation == "fixture.slow" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            if request.operation == "fixture.error" {
                Err(BackendError::new(
                    "fixture.error",
                    false,
                    JsonValue::Object(Default::default()),
                ))
            } else {
                Ok(request.payload)
            }
        })
    }

    fn subscribe(
        &self,
        _request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async {
            let (sender, subscription) = event_channel(2);
            tokio::spawn(async move {
                let mut first = JsonValue::Object(Default::default());
                first
                    .as_object_mut()
                    .unwrap()
                    .insert("order".into(), JsonValue::from(1));
                let mut second = JsonValue::Object(Default::default());
                second
                    .as_object_mut()
                    .unwrap()
                    .insert("order".into(), JsonValue::from(2));
                sender.send(first).await.unwrap();
                sender.send(second).await.unwrap();
            });
            Ok(subscription)
        })
    }
}

async fn write_json(stream: &mut tokio::io::DuplexStream, value: &str) {
    let payload = value.as_bytes();
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    stream.write_all(payload).await.unwrap();
}

async fn read_frame(stream: &mut tokio::io::DuplexStream) -> Frame {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    decode_frame(&bytes).unwrap()
}

#[tokio::test]
async fn sdk_hides_framing_dispatches_work_and_honors_shutdown() {
    let (mut host_read, sdk_write) = tokio::io::duplex(64 * 1024);
    let (sdk_read, mut host_write) = tokio::io::duplex(64 * 1024);
    let service = tokio::spawn(Package::builder(Arc::new(Echo)).serve_io(sdk_read, sdk_write));
    write_json(&mut host_write, r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#).await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::Initialized
    );

    write_json(&mut host_write, r#"{"kind":"request","id":"h:one","operation":"fixture.echo","deadlineUnixMs":"9999999999999","payload":{"value":1}}"#).await;
    let response = read_frame(&mut host_read).await;
    assert_eq!(response.kind(), FrameKind::Response);
    assert_eq!(response.field("id").unwrap(), "h:one");

    write_json(&mut host_write, r#"{"kind":"request","id":"h:cap","operation":"fixture.capability","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    let capability = read_frame(&mut host_read).await;
    assert_eq!(capability.kind(), FrameKind::Request);
    let capability_id = capability.field("id").unwrap().as_str().unwrap();
    write_json(
        &mut host_write,
        &format!(r#"{{"kind":"response","id":"{capability_id}","payload":{{"ok":true}}}}"#),
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.field("id").unwrap(),
        "h:cap"
    );

    write_json(&mut host_write, r#"{"kind":"stream_open","id":"h:events","topic":"fixture.events","initialCredit":32,"payload":{}}"#).await;
    for sequence in ["1", "2"] {
        let event = read_frame(&mut host_read).await;
        assert_eq!(event.kind(), FrameKind::Event);
        assert_eq!(event.field("sequence").unwrap(), sequence);
    }
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::StreamClose
    );

    write_json(&mut host_write, r#"{"kind":"request","id":"h:slow","operation":"fixture.slow","deadlineUnixMs":"9999999999999","payload":{}}"#).await;
    write_json(&mut host_write, r#"{"kind":"ping","nonce":"cGluZw"}"#).await;
    assert_eq!(read_frame(&mut host_read).await.kind(), FrameKind::Pong);
    assert_eq!(
        read_frame(&mut host_read).await.field("id").unwrap(),
        "h:slow"
    );
    write_json(
        &mut host_write,
        r#"{"kind":"shutdown","reason":"application_shutdown"}"#,
    )
    .await;
    assert_eq!(
        read_frame(&mut host_read).await.kind(),
        FrameKind::ShutdownAck
    );
    drop(host_write);
    service.await.unwrap().unwrap();
}

#[test]
fn sdk_dependency_boundary_excludes_host_and_runtime_internals() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    for forbidden in [
        "teamarness-core",
        "teamarness-package-host",
        "tauri",
        "iroh",
        "p2panda",
        "rusqlite",
    ] {
        assert!(!manifest.contains(forbidden), "SDK depends on {forbidden}");
    }
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    for forbidden in [
        "teamarness_core",
        "teamarness_package_host",
        "rusqlite",
        "iroh",
    ] {
        assert!(!source.contains(forbidden), "SDK source names {forbidden}");
    }
}
