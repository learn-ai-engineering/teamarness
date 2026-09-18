use std::collections::BTreeMap;
use std::sync::Arc;

use teamarness_package_protocol::{Frame, FrameKind, JsonValue, MAX_IN_FLIGHT_EACH_DIRECTION};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::task::JoinHandle;

use crate::capability::{CapabilityClient, CapabilityStreams, PendingCapabilities};
use crate::priority_writer::PriorityWriter;
use crate::{Backend, BackendError, BackendRequest, Initialization, SdkError, object};

pub(crate) async fn run(
    backend: Arc<dyn Backend>,
    writer: Arc<PriorityWriter>,
    mut frames: mpsc::Receiver<Frame>,
    pending_capabilities: PendingCapabilities,
    capability_streams: CapabilityStreams,
) -> Result<(), SdkError> {
    let initialize = frames.recv().await.ok_or(SdkError::ParentClosed)?;
    if initialize.kind() != FrameKind::Initialize {
        return Err(SdkError::Protocol("handshake_order"));
    }
    let initialization = Initialization::from_frame(&initialize)?;
    writer.control(initialized(&initialize)?).await?;

    let capabilities = CapabilityClient::new(
        writer.clone(),
        pending_capabilities.clone(),
        capability_streams.clone(),
    );
    let lifecycle = Arc::new(Mutex::new(()));
    let (done_tx, mut done_rx) = mpsc::channel::<String>(MAX_IN_FLIGHT_EACH_DIRECTION);
    let mut tasks: BTreeMap<String, JoinHandle<()>> = BTreeMap::new();
    let mut streams: BTreeMap<String, (JoinHandle<()>, Arc<Semaphore>)> = BTreeMap::new();

    loop {
        tokio::select! {
            completed = done_rx.recv() => {
                if let Some(id) = completed { tasks.remove(&id); }
            }
            frame = frames.recv() => {
                let Some(frame) = frame else {
                    crate::parent_lifeline::abort_all(tasks.into_values());
                    crate::parent_lifeline::abort_all(streams.into_values().map(|value| value.0));
                    return Err(SdkError::ParentClosed);
                };
                match frame.kind() {
                    FrameKind::Request => {
                        if tasks.len() >= MAX_IN_FLIGHT_EACH_DIRECTION {
                            return Err(SdkError::Protocol("overloaded"));
                        }
                        let id = string_field(&frame, "id")?;
                        if tasks.contains_key(&id) {
                            return Err(SdkError::Protocol("duplicate_id"));
                        }
                        let request = BackendRequest {
                            operation: string_field(&frame, "operation")?,
                            payload: frame.field("payload").cloned().ok_or(SdkError::Protocol("malformed_frame"))?,
                            initialization: initialization.clone(),
                            capabilities: capabilities.clone(),
                        };
                        let backend = backend.clone();
                        let writer = writer.clone();
                        let done = done_tx.clone();
                        let lifecycle = lifecycle.clone();
                        let task_id = id.clone();
                        let task = tokio::spawn(async move {
                            let result = if crate::LifecycleOperation::parse(&request.operation).is_some() {
                                let _barrier = lifecycle.lock().await;
                                backend.handle(request).await
                            } else {
                                backend.handle(request).await
                            };
                            let _ = writer.control(terminal(&id, result)).await;
                            let _ = done.send(id).await;
                        });
                        tasks.insert(task_id, task);
                    }
                    FrameKind::Cancel => {
                        let target = string_field(&frame, "targetId")?;
                        if let Some(task) = tasks.remove(&target) {
                            task.abort();
                            writer.control(error_frame(
                                &target,
                                &BackendError::new("cancelled", true, empty_object()),
                            )).await?;
                        }
                    }
                    FrameKind::Event => {
                        let id = string_field(&frame, "streamId")?;
                        let sequence = frame
                            .field("sequence")
                            .and_then(JsonValue::as_str)
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or(SdkError::Protocol("event_sequence"))?;
                        let payload = frame
                            .field("payload")
                            .cloned()
                            .ok_or(SdkError::Protocol("malformed_frame"))?;
                        if crate::capability::CapabilityClient::event(
                            &capability_streams,
                            &id,
                            sequence,
                            payload,
                        )
                        .await
                        .is_err()
                        {
                            return Err(SdkError::Protocol("event_sequence"));
                        }
                    }
                    FrameKind::Response | FrameKind::Error => {
                        let id = string_field(&frame, "id")?;
                        let Some(sender) = pending_capabilities.lock().await.remove(&id) else {
                            return Err(SdkError::Protocol("unknown_response"));
                        };
                        let result = if frame.kind() == FrameKind::Response {
                            Ok(frame.field("payload").cloned().unwrap_or_else(empty_object))
                        } else {
                            Err(BackendError::new(
                                frame.field("code").and_then(JsonValue::as_str).unwrap_or("capability_error"),
                                frame.field("retryable").and_then(JsonValue::as_bool).unwrap_or(false),
                                frame.field("details").cloned().unwrap_or_else(empty_object),
                            ))
                        };
                        let _ = sender.send(result);
                    }
                    FrameKind::Ping => {
                        writer.control(object([
                            ("kind", JsonValue::String("pong".into())),
                            ("nonce", frame.field("nonce").cloned().ok_or(SdkError::Protocol("malformed_frame"))?),
                        ])).await?;
                    }
                    FrameKind::StreamOpen => {
                        streams.retain(|_, (task, _)| !task.is_finished());
                        if streams.len() >= MAX_IN_FLIGHT_EACH_DIRECTION {
                            return Err(SdkError::Protocol("overloaded"));
                        }
                        let id = string_field(&frame, "id")?;
                        if streams.contains_key(&id) {
                            return Err(SdkError::Protocol("duplicate_id"));
                        }
                        let request = BackendRequest {
                            operation: string_field(&frame, "topic")?,
                            payload: frame.field("payload").cloned().ok_or(SdkError::Protocol("malformed_frame"))?,
                            initialization: initialization.clone(),
                            capabilities: capabilities.clone(),
                        };
                        let backend = backend.clone();
                        let writer = writer.clone();
                        let stream_id = id.clone();
                        let credit = Arc::new(Semaphore::new(32));
                        let stream_credit = credit.clone();
                        let task = tokio::spawn(async move {
                            let Ok(mut subscription) = backend.subscribe(request).await else {
                                let _ = writer.data(stream_close(&stream_id, "backend_error")).await;
                                return;
                            };
                            let mut sequence = 0_u64;
                            while let Some(event) = subscription.receiver.recv().await {
                                let permit = match stream_credit.acquire().await {
                                    Ok(permit) => permit,
                                    Err(_) => return,
                                };
                                permit.forget();
                                match event {
                                    Ok(payload) => {
                                        sequence += 1;
                                        if writer.data(object([
                                            ("kind", JsonValue::String("event".into())),
                                            ("streamId", JsonValue::String(stream_id.clone())),
                                            ("sequence", JsonValue::String(sequence.to_string())),
                                            ("payload", payload),
                                        ])).await.is_err() { return; }
                                    }
                                    Err(error) => {
                                        let _ = writer.data(stream_close(&stream_id, error.code())).await;
                                        return;
                                    }
                                }
                            }
                            let _ = writer.data(stream_close(&stream_id, "completed")).await;
                        });
                        streams.insert(id, (task, credit));
                    }
                    FrameKind::Credit => {
                        let id = string_field(&frame, "streamId")?;
                        let amount = frame.field("amount").and_then(JsonValue::as_u64)
                            .and_then(|value| usize::try_from(value).ok())
                            .filter(|value| (1..=32).contains(value))
                            .ok_or(SdkError::Protocol("credit_violation"))?;
                        let Some((_, credit)) = streams.get(&id) else {
                            return Err(SdkError::Protocol("unknown_stream"));
                        };
                        if credit.available_permits().saturating_add(amount) > 32 {
                            return Err(SdkError::Protocol("credit_violation"));
                        }
                        credit.add_permits(amount);
                    }
                    FrameKind::StreamClose => {
                        let id = string_field(&frame, "streamId")?;
                        if let Some((task, _)) = streams.remove(&id) {
                            task.abort();
                        } else {
                            let code = frame.field("code").and_then(JsonValue::as_str);
                            let error = code
                                .filter(|code| !matches!(*code, "completed" | "closed" | "dropped"))
                                .map(|code| BackendError::new(code, true, empty_object()));
                            crate::capability::CapabilityClient::close_stream(&capability_streams, &id, error).await;
                        }
                    }
                    FrameKind::Shutdown => {
                        crate::parent_lifeline::abort_all(tasks.into_values());
                        crate::parent_lifeline::abort_all(streams.into_values().map(|value| value.0));
                        writer.control(object([("kind", JsonValue::String("shutdown_ack".into()))])).await?;
                        return Ok(());
                    }
                    _ => return Err(SdkError::Protocol("protocol_violation")),
                }
            }
        }
    }
}

fn initialized(frame: &Frame) -> Result<JsonValue, SdkError> {
    Ok(object([
        ("kind", JsonValue::String("initialized".into())),
        ("nonce", required(frame, "nonce")?),
        ("package", required(frame, "package")?),
        ("uiContractDigest", required(frame, "uiContractDigest")?),
        ("processGeneration", required(frame, "processGeneration")?),
        ("protocol", required(frame, "protocol")?),
    ]))
}

fn required(frame: &Frame, name: &str) -> Result<JsonValue, SdkError> {
    frame
        .field(name)
        .cloned()
        .ok_or(SdkError::Protocol("malformed_frame"))
}

fn string_field(frame: &Frame, name: &str) -> Result<String, SdkError> {
    frame
        .field(name)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or(SdkError::Protocol("malformed_frame"))
}

fn terminal(id: &str, result: Result<JsonValue, BackendError>) -> JsonValue {
    match result {
        Ok(payload) if payload.is_object() => object([
            ("kind", JsonValue::String("response".into())),
            ("id", JsonValue::String(id.into())),
            ("payload", payload),
        ]),
        Ok(_) => error_frame(
            id,
            &BackendError::new("invalid_response", false, empty_object()),
        ),
        Err(error) => error_frame(id, &error),
    }
}

fn error_frame(id: &str, error: &BackendError) -> JsonValue {
    object([
        ("kind", JsonValue::String("error".into())),
        ("id", JsonValue::String(id.into())),
        ("code", JsonValue::String(error.code.clone())),
        ("retryable", JsonValue::Bool(error.retryable)),
        ("details", error.details.clone()),
    ])
}

fn stream_close(stream_id: &str, code: &str) -> JsonValue {
    object([
        ("kind", JsonValue::String("stream_close".into())),
        ("streamId", JsonValue::String(stream_id.into())),
        ("code", JsonValue::String(code.into())),
    ])
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Default::default())
}
