//! One validated Provider event stream fans out into independently bounded Consumer streams.
use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use teamarness_package_protocol::{
    INITIAL_STREAM_CREDIT, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION, MAX_WRITER_BYTES,
};
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore, mpsc, watch};

use super::{PortBroker, PortObservation, PortStage};
use crate::protocol::{HostStreamSender, PortQueryStream, StreamEnd};
use crate::{PackageErrorCode, PackageIdentity, PackageValue, WorkspaceId};

// Subscriber capacity is deliberately smaller than the protocol credit window: the
// upstream never waits for a Consumer that stops returning its credit. Even when
// every accepted event is at the contract's maximum payload size, these permits
// limit actual queued bytes, independently of the writer and UI/lifecycle lanes.
const QUEUED_EVENTS: usize = INITIAL_STREAM_CREDIT / 8;
const SUBSCRIBER_BYTES: usize = QUEUED_EVENTS * MAX_FRAME_BYTES;
pub(super) const EVENT_BYTES: usize = MAX_WRITER_BYTES / 2;
pub(super) const MAX_EVENT_SUBSCRIBERS: usize = MAX_IN_FLIGHT_EACH_DIRECTION / 2;
pub(super) const MAX_EVENT_UPSTREAMS: usize = MAX_IN_FLIGHT_EACH_DIRECTION / 4;
const PROVIDER_SUBSCRIBERS: usize = MAX_EVENT_SUBSCRIBERS / 4;
const PROVIDER_UPSTREAMS: usize = MAX_EVENT_UPSTREAMS / 2;
const PROVIDER_BYTES: usize = EVENT_BYTES / 2;
const _: () = {
    assert!(QUEUED_EVENTS < INITIAL_STREAM_CREDIT);
    assert!(PROVIDER_UPSTREAMS < MAX_IN_FLIGHT_EACH_DIRECTION);
    assert!(MAX_EVENT_UPSTREAMS < MAX_IN_FLIGHT_EACH_DIRECTION);
    assert!(PROVIDER_SUBSCRIBERS < MAX_EVENT_SUBSCRIBERS);
    assert!(EVENT_BYTES <= MAX_WRITER_BYTES);
    assert!(PROVIDER_BYTES >= MAX_FRAME_BYTES);
    assert!(SUBSCRIBER_BYTES >= MAX_FRAME_BYTES);
};

pub(super) type EventKey = (WorkspaceId, PackageIdentity, String, String);

#[derive(Default)]
pub(super) struct EventRegistry {
    next_subscriber: u64,
    streams: BTreeMap<EventKey, Upstream>,
    provider_bytes: BTreeMap<(WorkspaceId, PackageIdentity), Arc<Semaphore>>,
    provider_streams: BTreeMap<(WorkspaceId, PackageIdentity), Arc<Semaphore>>,
    provider_subscribers: BTreeMap<(WorkspaceId, PackageIdentity), Arc<Semaphore>>,
}

struct Upstream {
    generation: u64,
    provider_id: String,
    token: u64,
    subscribers: BTreeMap<u64, Subscriber>,
    stopped: Arc<Notify>,
    done: watch::Sender<bool>,
    closing: bool,
}

struct Subscriber {
    events: mpsc::Sender<QueuedEvent>,
    terminal: watch::Sender<Option<&'static str>>,
    bytes: Arc<Semaphore>,
    lease: Option<Arc<crate::port_access::SubscriptionLease>>,
}

struct QueuedEvent {
    payload: Value,
    _global: OwnedSemaphorePermit,
    _provider: OwnedSemaphorePermit,
    _subscriber: OwnedSemaphorePermit,
}

pub(crate) struct EventSubscription {
    events: mpsc::Receiver<QueuedEvent>,
    terminal: watch::Receiver<Option<&'static str>>,
    key: EventKey,
    subscriber: u64,
    consumer_package: String,
    consumer_generation: u64,
    provider_id: String,
    generation: u64,
    lease: Option<Arc<crate::port_access::SubscriptionLease>>,
    _slot: OwnedSemaphorePermit,
    _provider_slot: OwnedSemaphorePermit,
}

impl PortBroker {
    pub(crate) async fn subscribe_event(
        &self,
        caller: &PackageIdentity,
        workspace: &WorkspaceId,
        installation: &str,
        route: crate::port_access::ResolvedPort,
        input: &Map<String, Value>,
        consumer_generation: u64,
    ) -> Result<EventSubscription, PackageErrorCode> {
        if input.len() != 6
            || !["grant", "access", "major", "contract", "port", "kind"]
                .iter()
                .all(|key| input.contains_key(*key))
        {
            return Err(PackageErrorCode::CapabilityRequestInvalid);
        }
        let contract = input
            .get("contract")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        let port = input
            .get("port")
            .and_then(Value::as_str)
            .ok_or(PackageErrorCode::CapabilityRequestInvalid)?;
        if input.get("kind").and_then(Value::as_str) != Some("event") {
            return Err(PackageErrorCode::PortUngranted);
        }
        let crate::port_access::ResolvedPort {
            provider: actor,
            schemas,
            major,
            minor,
            lease,
            ..
        } = route;
        if let Some(lease) = &lease {
            lease.enqueue(|| ())?;
        }
        let slot = self
            .event_subscribers
            .clone()
            .try_acquire_owned()
            .map_err(|_| PackageErrorCode::Overloaded)?;
        let key = (
            workspace.clone(),
            actor.identity().clone(),
            contract.to_owned(),
            port.to_owned(),
        );
        // Serialize only this event port's first open. Activation can itself
        // call unrelated Ports, so never hold the global fan-out registry over
        // activation or the Provider writer's stream-open send.
        if matches!(
            actor.snapshot().await.lifecycle,
            crate::LifecycleState::Admitted
                | crate::LifecycleState::Activating
                | crate::LifecycleState::Recovering
        ) {
            // A concurrent first subscriber may already be activating the
            // on-demand Provider. Wait on its lifecycle gate rather than
            // treating that in-progress generation as exited.
            actor.activate().await.map_err(|failure| failure.code)?;
        }
        let open_lock = self
            .event_open_locks
            .lock()
            .expect("event open locks poisoned")
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _open = open_lock.lock().await;
        let mut generation = actor
            .snapshot()
            .await
            .process_generation
            .unwrap_or_default();
        let mut registry = self.events.lock().await;
        while let Some(entry) = registry.streams.get_mut(&key) {
            if entry.generation == generation && !entry.closing {
                break;
            }
            if !entry.closing {
                for (_, subscriber) in std::mem::take(&mut entry.subscribers) {
                    subscriber.terminal.send_replace(Some("provider_exited"));
                }
                entry.closing = true;
                entry.stopped.notify_one();
            }
            let mut done = entry.done.subscribe();
            drop(registry);
            if !*done.borrow() {
                let _ = done.changed().await;
            }
            generation = actor
                .snapshot()
                .await
                .process_generation
                .unwrap_or_default();
            registry = self.events.lock().await;
        }
        let provider_slot = registry
            .provider_subscribers
            .entry((workspace.clone(), actor.identity().clone()))
            .or_insert_with(|| Arc::new(Semaphore::new(PROVIDER_SUBSCRIBERS)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| PackageErrorCode::Overloaded)?;
        let provider_count = registry
            .streams
            .iter()
            .filter(|(other, _)| other.0 == *workspace && other.1 == *actor.identity())
            .map(|(_, upstream)| upstream.subscribers.len())
            .sum::<usize>();
        if provider_count >= PROVIDER_SUBSCRIBERS {
            return Err(PackageErrorCode::Overloaded);
        }
        if !registry.streams.contains_key(&key)
            || registry
                .streams
                .get(&key)
                .is_some_and(|entry| entry.generation != generation)
        {
            // A new Provider stream is not opened until an explicit subscription.
            let provider_streams = registry
                .streams
                .iter()
                .filter(|(other, _)| other.0 == *workspace && other.1 == *actor.identity())
                .count();
            if provider_streams >= PROVIDER_UPSTREAMS {
                return Err(PackageErrorCode::Overloaded);
            }
            let stream_slot = self
                .event_streams
                .clone()
                .try_acquire_owned()
                .map_err(|_| PackageErrorCode::Overloaded)?;
            let provider_stream_slot = registry
                .provider_streams
                .entry((workspace.clone(), actor.identity().clone()))
                .or_insert_with(|| Arc::new(Semaphore::new(PROVIDER_UPSTREAMS)))
                .clone()
                .try_acquire_owned()
                .map_err(|_| PackageErrorCode::Overloaded)?;
            let payload: PackageValue = serde_json::from_value(json!({
                "contract":contract,"major":major,"minor":minor,"port":port,
                "actor":{"kind":"person","identity":installation},"request":{}
            }))
            .map_err(|_| PackageErrorCode::PortRequestInvalid)?;
            drop(registry);
            let upstream = actor
                .port_event_stream(payload, schemas, contract.to_owned(), port.to_owned())
                .await
                .map_err(|failure| failure.code)?;
            let generation = actor
                .snapshot()
                .await
                .process_generation
                .unwrap_or_default();
            if upstream.generation != generation {
                let _ = upstream.client.stream_close(&upstream.id).await;
                return Err(PackageErrorCode::ProcessExited);
            }
            let mut reopened = self.events.lock().await;
            let stopped = Arc::new(Notify::new());
            let (done, _) = watch::channel(false);
            let provider_bytes = reopened
                .provider_bytes
                .entry((workspace.clone(), actor.identity().clone()))
                .or_insert_with(|| Arc::new(Semaphore::new(PROVIDER_BYTES)))
                .clone();
            reopened.next_subscriber += 1;
            let token = reopened.next_subscriber;
            let old = reopened.streams.insert(
                key.clone(),
                Upstream {
                    generation,
                    provider_id: upstream.id.clone(),
                    token,
                    subscribers: BTreeMap::new(),
                    stopped: stopped.clone(),
                    done: done.clone(),
                    closing: false,
                },
            );
            if let Some(old) = old {
                for (_, subscriber) in old.subscribers {
                    subscriber.terminal.send_replace(Some("provider_exited"));
                }
                old.stopped.notify_one();
            }
            registry = reopened;
            tokio::spawn(run_upstream(
                key.clone(),
                upstream,
                self.events.clone(),
                self.event_bytes.clone(),
                provider_bytes,
                stopped,
                stream_slot,
                provider_stream_slot,
                done,
                token,
            ));
        }
        registry.next_subscriber += 1;
        let subscriber = registry.next_subscriber;
        let (sender, events) = mpsc::channel(QUEUED_EVENTS);
        let (terminal, terminal_receiver) = watch::channel(None);
        registry
            .streams
            .get_mut(&key)
            .expect("upstream inserted")
            .subscribers
            .insert(
                subscriber,
                Subscriber {
                    events: sender,
                    terminal,
                    bytes: Arc::new(Semaphore::new(SUBSCRIBER_BYTES)),
                    lease: lease.clone(),
                },
            );
        let upstream = registry.streams.get(&key).expect("upstream inserted");
        Ok(EventSubscription {
            events,
            terminal: terminal_receiver,
            key,
            subscriber,
            consumer_package: caller.as_str().to_owned(),
            consumer_generation,
            provider_id: upstream.provider_id.clone(),
            generation: upstream.generation,
            lease,
            _slot: slot,
            _provider_slot: provider_slot,
        })
    }

    pub(crate) fn record_event_subscription(&self, subscription: &EventSubscription, id: &str) {
        self.record(PortObservation {
            kind: "event",
            contract: Some(subscription.key.2.clone()),
            port: Some(subscription.key.3.clone()),
            consumer_id: id.to_owned(),
            consumer_package: subscription.consumer_package.clone(),
            consumer_generation: subscription.consumer_generation,
            provider_id: Some(subscription.provider_id.clone()),
            provider_generation: Some(subscription.generation),
            stage: PortStage::Subscribed,
            code: None,
            outcome_uncertain: false,
        });
    }

    pub(crate) async fn drop_event_subscription(&self, subscription: &EventSubscription) {
        let mut registry = self.events.lock().await;
        if let Some(upstream) = registry.streams.get_mut(&subscription.key)
            && upstream
                .subscribers
                .remove(&subscription.subscriber)
                .is_some()
            && upstream.subscribers.is_empty()
        {
            upstream.closing = true;
            upstream.stopped.notify_one();
        }
    }

    pub(crate) async fn run_event_subscription(
        self: Arc<Self>,
        id: String,
        mut subscription: EventSubscription,
        mut commands: mpsc::Receiver<super::super::capability::StreamCommand>,
        output: HostStreamSender,
        mut cancelled: watch::Receiver<bool>,
    ) {
        use super::super::capability::StreamCommand;
        let mut revocation = subscription.lease.as_ref().map(|lease| lease.watch());
        let mut revoked = false;
        let mut credit = INITIAL_STREAM_CREDIT as u64;
        let mut sequence = 0;
        let observation = |stage, code| {
            self.record(PortObservation {
                kind: "event",
                contract: Some(subscription.key.2.clone()),
                port: Some(subscription.key.3.clone()),
                consumer_id: id.clone(),
                consumer_package: subscription.consumer_package.clone(),
                consumer_generation: subscription.consumer_generation,
                provider_id: Some(subscription.provider_id.clone()),
                provider_generation: Some(subscription.generation),
                stage,
                code,
                outcome_uncertain: false,
            })
        };
        let reason = loop {
            if !revoked && let Some(code) = revocation.as_ref().and_then(|watch| *watch.borrow()) {
                let mut registry = self.events.lock().await;
                if let Some(upstream) = registry.streams.get_mut(&subscription.key) {
                    if let Some(subscriber) = upstream.subscribers.remove(&subscription.subscriber)
                    {
                        subscriber
                            .terminal
                            .send_replace(Some(crate::protocol::capability_code(code)));
                    }
                    if upstream.subscribers.is_empty() {
                        upstream.closing = true;
                        upstream.stopped.notify_one();
                    }
                }
                revoked = true;
            }
            if *cancelled.borrow() {
                break "closed";
            }
            // A revoked consumer cannot keep scarce permits by withholding credit.
            if revoked && credit == 0 {
                break subscription.terminal.borrow().unwrap_or("closed");
            }
            if subscription.events.is_closed() && subscription.events.is_empty() {
                break subscription.terminal.borrow().unwrap_or("provider_exited");
            }
            tokio::select! {
                biased;
                _ = async {
                    if let Some(watch) = &mut revocation { let _ = watch.changed().await; }
                    else { std::future::pending::<()>().await; }
                }, if !revoked => {},
                change = cancelled.changed() => if change.is_err() || *cancelled.borrow() { break "closed"; },
                command = commands.recv() => match command {
                    Some(StreamCommand::Credit(amount)) => credit += amount,
                    Some(StreamCommand::Close) | None => break "closed",
                },
                event = subscription.events.recv(), if credit > 0 => match event {
                    Some(event) => {
                        #[cfg(feature = "test-hooks")]
                        self.pause_port_work(&subscription.consumer_package.parse().expect("admitted consumer"), true).await;
                        sequence += 1;
                        credit -= 1;
                        if output.event(&id, sequence, event.payload).await.is_err() { break "process_exited"; }
                        observation(PortStage::Event, None);
                    }
                    None => break subscription.terminal.borrow().unwrap_or("provider_exited"),
                },
                _ = subscription.terminal.changed(), if subscription.terminal.borrow().is_none() => {
                    if subscription.events.is_empty() { break subscription.terminal.borrow().unwrap_or("provider_exited"); }
                },
            }
        };
        let mut registry = self.events.lock().await;
        if let Some(upstream) = registry.streams.get_mut(&subscription.key)
            && upstream
                .subscribers
                .remove(&subscription.subscriber)
                .is_some()
            && upstream.subscribers.is_empty()
        {
            upstream.closing = true;
            upstream.stopped.notify_one();
        }
        drop(registry);
        output.retire(&id).await;
        observation(PortStage::Closed, Some(reason));
        let _ = output.close(&id, reason).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_upstream(
    key: EventKey,
    mut upstream: PortQueryStream,
    registry: Arc<Mutex<EventRegistry>>,
    global_bytes: Arc<Semaphore>,
    provider_bytes: Arc<Semaphore>,
    stopped: Arc<Notify>,
    _stream_slot: OwnedSemaphorePermit,
    _provider_stream_slot: OwnedSemaphorePermit,
    done: watch::Sender<bool>,
    token: u64,
) {
    let reason = loop {
        tokio::select! {
            _ = stopped.notified() => break "closed",
            event = upstream.receiver.changed() => match event {
                Ok(event) => {
                    let payload = match serde_json::to_value(event.payload) {
                        Ok(payload) => payload,
                        Err(_) => break "port_event_invalid",
                    };
                    let bytes = match serde_json::to_vec(&payload) {
                        Ok(bytes) => bytes.len(),
                        Err(_) => break "port_event_invalid",
                    };
                    let Ok(bytes) = u32::try_from(bytes) else { break "port_event_invalid" };
                    let mut guard = registry.lock().await;
                    if let Some(entry) = guard.streams.get_mut(&key).filter(|entry| entry.token == token) {
                        let mut failed = Vec::new();
                        for (&id, subscriber) in &entry.subscribers {
                            if let Err(reason) = offer_event(subscriber, &payload, bytes, &global_bytes, &provider_bytes) {
                                failed.push((id, reason));
                            }
                        }
                        for (id, reason) in failed {
                            if let Some(subscriber) = entry.subscribers.remove(&id) {
                                subscriber.terminal.send_replace(Some(reason));
                            }
                        }
                        if entry.subscribers.is_empty() {
                            entry.closing = true;
                            stopped.notify_one();
                        }
                    }
                    drop(guard);
                    if upstream.client.credit(&upstream.id, 1).await.is_err() { break "provider_exited"; }
                }
                Err(StreamEnd::Completed) => break "completed",
                Err(StreamEnd::Host(PackageErrorCode::PortEventInvalid)) => break "port_event_invalid",
                Err(_) => break "provider_exited",
            },
        }
    };
    let _ = upstream.client.stream_close(&upstream.id).await;
    let mut guard = registry.lock().await;
    if guard
        .streams
        .get(&key)
        .is_some_and(|entry| entry.token == token)
        && let Some(entry) = guard.streams.remove(&key)
    {
        for (_, subscriber) in entry.subscribers {
            subscriber.terminal.send_replace(Some(reason));
        }
    }
    done.send_replace(true);
}

fn offer_event(
    subscriber: &Subscriber,
    payload: &Value,
    bytes: u32,
    global_bytes: &Arc<Semaphore>,
    provider_bytes: &Arc<Semaphore>,
) -> Result<(), &'static str> {
    if let Some(lease) = &subscriber.lease {
        return lease
            .enqueue(|| enqueue_event(subscriber, payload, bytes, global_bytes, provider_bytes))
            .map_err(crate::protocol::capability_code)?;
    }
    enqueue_event(subscriber, payload, bytes, global_bytes, provider_bytes)
}

fn enqueue_event(
    subscriber: &Subscriber,
    payload: &Value,
    bytes: u32,
    global_bytes: &Arc<Semaphore>,
    provider_bytes: &Arc<Semaphore>,
) -> Result<(), &'static str> {
    let global = global_bytes
        .clone()
        .try_acquire_many_owned(bytes)
        .map_err(|_| "overloaded")?;
    let provider = provider_bytes
        .clone()
        .try_acquire_many_owned(bytes)
        .map_err(|_| "overloaded")?;
    let private = subscriber
        .bytes
        .clone()
        .try_acquire_many_owned(bytes)
        .map_err(|_| "port_subscriber_overflow")?;
    subscriber
        .events
        .try_send(QueuedEvent {
            payload: payload.clone(),
            _global: global,
            _provider: provider,
            _subscriber: private,
        })
        .map_err(|_| "port_subscriber_overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: TEAM.SYS.PKG-R14
    #[test]
    fn worst_case_frames_fit_only_finite_global_provider_and_subscriber_ledgers() {
        assert_eq!(EVENT_BYTES / MAX_FRAME_BYTES, 4);
        assert_eq!(PROVIDER_BYTES / MAX_FRAME_BYTES, 2);
        assert_eq!(SUBSCRIBER_BYTES / MAX_FRAME_BYTES, QUEUED_EVENTS);
        let global = Arc::new(Semaphore::new(EVENT_BYTES));
        let provider = Arc::new(Semaphore::new(PROVIDER_BYTES));
        let subscriber = Arc::new(Semaphore::new(SUBSCRIBER_BYTES));
        let first_global = global
            .clone()
            .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
            .unwrap();
        let first_provider = provider
            .clone()
            .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
            .unwrap();
        let first_subscriber = subscriber
            .clone()
            .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
            .unwrap();
        let second = provider
            .clone()
            .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
            .unwrap();
        assert!(
            provider
                .clone()
                .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
                .is_err()
        );
        drop((first_global, first_provider, first_subscriber, second));
        assert!(
            provider
                .clone()
                .try_acquire_many_owned(MAX_FRAME_BYTES as u32)
                .is_ok()
        );
    }

    // Verifies: TEAM.SYS.PKG-R14
    #[test]
    fn a_full_subscriber_queue_or_shared_byte_ledger_does_not_block_a_peer() {
        let global = Arc::new(Semaphore::new(EVENT_BYTES));
        let provider = Arc::new(Semaphore::new(PROVIDER_BYTES));
        let (slow_sender, mut slow_receiver) = mpsc::channel(QUEUED_EVENTS);
        let (fast_sender, mut fast_receiver) = mpsc::channel(QUEUED_EVENTS);
        let (slow_end, _) = watch::channel(None);
        let (fast_end, _) = watch::channel(None);
        let slow = Subscriber {
            events: slow_sender,
            terminal: slow_end,
            bytes: Arc::new(Semaphore::new(SUBSCRIBER_BYTES)),
            lease: None,
        };
        let fast = Subscriber {
            events: fast_sender,
            terminal: fast_end,
            bytes: Arc::new(Semaphore::new(SUBSCRIBER_BYTES)),
            lease: None,
        };
        let payload = json!({"order":1});
        let actual_bytes = serde_json::to_vec(&payload).unwrap().len() as u32;
        for _ in 0..QUEUED_EVENTS {
            offer_event(&slow, &payload, actual_bytes, &global, &provider).unwrap();
        }
        assert_eq!(
            offer_event(&slow, &payload, actual_bytes, &global, &provider),
            Err("port_subscriber_overflow")
        );
        offer_event(&fast, &payload, actual_bytes, &global, &provider).unwrap();
        assert_eq!(fast_receiver.try_recv().unwrap().payload, payload);
        // Reserve all but one maximum-frame event's bytes globally. Only the
        // next offer that cannot reserve space fails; existing queued events
        // and a peer after release are still valid.
        let occupied = global
            .clone()
            .try_acquire_many_owned((global.available_permits() - MAX_FRAME_BYTES) as u32)
            .unwrap();
        offer_event(&fast, &payload, MAX_FRAME_BYTES as u32, &global, &provider).unwrap();
        assert_eq!(
            offer_event(&slow, &payload, MAX_FRAME_BYTES as u32, &global, &provider),
            Err("overloaded")
        );
        drop(fast_receiver.try_recv().unwrap());
        drop(occupied);
        assert!(offer_event(&fast, &payload, actual_bytes, &global, &provider).is_ok());
        assert_eq!(slow_receiver.try_recv().unwrap().payload, payload);
    }
}
