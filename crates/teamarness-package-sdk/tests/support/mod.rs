//! Frame helpers shared by the raw-framing SDK tests.

#![allow(dead_code)]

use teamarness_package_protocol::{Frame, decode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The `initialize` frame the Host sends, with placeholder digests.
pub const INITIALIZE: &str = r#"{"kind":"initialize","nonce":"bm9uY2U","package":{"identity":"dev.teamarness.fixture","version":"1.0.0","manifestDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","backendDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"uiContractDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","processGeneration":"1","workspace":{"id":"workspace1","name":"Workspace"},"installation":{"identity":"installation1","displayName":"Installation"},"dataDirectory":{"path":"/tmp/package","continuityId":"Y29udGludWl0eQ"},"protocol":{"major":1,"minor":0},"capabilities":[],"limits":{"maxFrameBytes":1048576,"maxInFlightEachDirection":32,"maxWriterFrames":64,"maxWriterBytes":8388608,"initialStreamCredit":32}}"#;

pub async fn write_json(stream: &mut tokio::io::DuplexStream, value: &str) {
    let payload = value.as_bytes();
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    stream.write_all(payload).await.unwrap();
}

pub async fn read_frame(stream: &mut tokio::io::DuplexStream) -> Frame {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await.unwrap();
    let mut bytes = prefix.to_vec();
    bytes.extend(payload);
    decode_frame(&bytes).unwrap()
}
