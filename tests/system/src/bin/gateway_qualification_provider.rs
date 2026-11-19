#![forbid(unsafe_code)]
//! Source-built only for the explicit gateway proof; control requests never ship.
use serde_json::{Value, json};
use std::{
    process::ExitCode,
    sync::Mutex,
    sync::atomic::{AtomicUsize, Ordering},
};
use teamarness_package_sdk::{
    Activation, BackendError, Context, Empty, EventSubscription, Router, event_channel,
    ports::PortActor,
};
use tokio::sync::{Semaphore, broadcast};

#[derive(Clone)]
enum Signal {
    Value(Value),
    End(bool),
}

struct Provider {
    guide: Mutex<String>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    waiting: AtomicUsize,
    release: Semaphore,
    setup_waiting: AtomicUsize,
    command_waiting: AtomicUsize,
    command_release: Semaphore,
    rows: Mutex<Vec<Value>>,
    signals: broadcast::Sender<Signal>,
}
#[tokio::main]
async fn main() -> ExitCode {
    let mut router = Router::new(|_: Activation| async {
        Ok(Provider {
            guide: Mutex::new("# Gateway qualification\nRead the sample with offset seven. Never replay an uncertain command automatically.".into()),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            waiting: AtomicUsize::new(0),
            release: Semaphore::new(0),
            setup_waiting: AtomicUsize::new(0),
            command_waiting: AtomicUsize::new(0),
            command_release: Semaphore::new(0),
            rows: Mutex::new(Vec::new()),
            signals: broadcast::channel(32).0,
        })
    })
    .request("proof.status", |c: Context<Provider>, _: Empty| async move {
        Ok::<_, BackendError>(json!({
            "reads": c.state().reads.load(Ordering::SeqCst),
            "writes": c.state().writes.load(Ordering::SeqCst),
            "waiting": c.state().waiting.load(Ordering::SeqCst),
            "setupWaiting": c.state().setup_waiting.load(Ordering::SeqCst),
            "commandWaiting": c.state().command_waiting.load(Ordering::SeqCst),
            "rows": *c.state().rows.lock().unwrap(),
        }))
    })
    .request("proof.release", |c: Context<Provider>, _: Empty| async move {
        c.state().release.add_permits(12);
        Ok::<_, BackendError>(json!({}))
    });
    router = router
        .request(
            "proof.emit",
            |c: Context<Provider>, request: Value| async move {
                let receivers = c
                    .state()
                    .signals
                    .send(Signal::Value(request["value"].clone()))
                    .unwrap_or(0);
                Ok::<_, BackendError>(json!({"receivers":receivers}))
            },
        )
        .request(
            "proof.end",
            |c: Context<Provider>, request: Value| async move {
                let _ = c
                    .state()
                    .signals
                    .send(Signal::End(request["error"] == true));
                Ok::<_, BackendError>(json!({}))
            },
        )
        .request(
            "proof.command-release",
            |c: Context<Provider>, _: Empty| async move {
                c.state().command_release.add_permits(1);
                Ok::<_, BackendError>(json!({}))
            },
        );
    router = router.request(
        "proof.guide",
        |c: Context<Provider>, request: Value| async move {
            *c.state().guide.lock().unwrap() = request["markdown"].as_str().unwrap().to_owned();
            Ok::<_, BackendError>(json!({}))
        },
    );
    for contract in [
        "dev.teamarness.qualification",
        "dev.example.second",
        "dev.example.guide-less",
        "dev.teamarness.conversations",
    ] {
        router = router
            .port_request(
                contract,
                "runtime.guide",
                "query",
                |c: Context<Provider>, _: PortActor, _: Empty| async move {
                    Ok::<_, BackendError>(json!({"markdown":*c.state().guide.lock().unwrap()}))
                },
            )
            .port_request(
                contract,
                "sample.read",
                "query",
                |c: Context<Provider>, actor: PortActor, request: Value| async move {
                    c.state().reads.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, BackendError>(
                        json!({"answer":request["offset"].as_u64().unwrap()+35,"actor":actor}),
                    )
                },
            )
            .port_request(
                contract,
                "sample.raw",
                "query",
                |_: Context<Provider>, _: PortActor, _: Empty| async {
                    Ok::<_, BackendError>(json!({"large":9007199254740993_u64,"maximum":u64::MAX,"nested":[null,false,{"version":"opaque:v7/+=not-a-counter"}]}))
                },
            )
            .port_request(
                contract,
                "sample.refuse",
                "command",
                |_: Context<Provider>, _: PortActor, _: Empty| async {
                    Err::<Value, _>(BackendError::new("proof.precondition", true, json!({"version":"opaque:v7/+=not-a-counter","nested":{"large":9007199254740993_u64},"reasons":["not-ready",null]})))
                },
            )
            .port_request(
                contract,
                "added.read",
                "query",
                |_: Context<Provider>, _: PortActor, _: Empty| async {
                    Ok::<_, BackendError>(json!({"added":true}))
                },
            )
            .port_request(
                contract,
                "sample.wait",
                "query",
                |c: Context<Provider>, _: PortActor, _: Empty| async move {
                    c.state().waiting.fetch_add(1, Ordering::SeqCst);
                    let permit = c.state().release.acquire().await.unwrap();
                    permit.forget();
                    Ok::<_, BackendError>(json!({"released":true}))
                },
            )
            .port_request(
                contract,
                "sample.write",
                "command",
                |c: Context<Provider>, actor: PortActor, request: Value| async move {
                    c.state().writes.fetch_add(1, Ordering::SeqCst);
                    if request["value"] == "held" {
                        c.state().command_waiting.fetch_add(1, Ordering::SeqCst);
                        c.state().command_release.acquire().await.unwrap().forget();
                    }
                    c.state()
                        .rows
                        .lock()
                        .unwrap()
                        .push(json!({"actor":actor,"value":request["value"]}));
                    Ok::<_, BackendError>(json!({"written":true}))
                },
            )
            .port_event(
                contract,
                "sample.events",
                |c: Context<Provider>, _: PortActor| async move {
                    c.state().setup_waiting.fetch_add(1, Ordering::SeqCst);
                    c.state().release.acquire().await.unwrap().forget();
                    Ok(listen(&c))
                },
            )
            .port_live_query(
                contract,
                "sample.stream",
                |c: Context<Provider>, _: PortActor, _: Empty| async move { Ok(listen(&c)) },
            );
    }
    router.run().await
}

// The receiver is registered before returning, as required by confirmed setup.
fn listen(context: &Context<Provider>) -> EventSubscription {
    let mut receiver = context.state().signals.subscribe();
    let (sender, subscription) = event_channel(2);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = sender.closed() => break,
                signal = receiver.recv() => match signal {
                    Ok(Signal::Value(value)) => if sender.send_port_value(value).await.is_err() { break; },
                    Ok(Signal::End(error)) => {
                        if error { let _ = sender.fail(BackendError::new("proof.stream_failed", false, json!({"reason":"controlled"}))).await; }
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
    });
    subscription
}
