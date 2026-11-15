#![forbid(unsafe_code)]
//! Source-built only for the explicit gateway proof; control requests never ship.
use serde_json::{Value, json};
use std::{
    process::ExitCode,
    sync::Mutex,
    sync::atomic::{AtomicUsize, Ordering},
};
use teamarness_package_sdk::{Activation, BackendError, Context, Empty, Router, ports::PortActor};
use tokio::sync::Semaphore;

struct Provider {
    guide: Mutex<String>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    waiting: AtomicUsize,
    release: Semaphore,
}
#[tokio::main]
async fn main() -> ExitCode {
    let mut router = Router::new(|_: Activation| async { Ok(Provider {
            guide: Mutex::new("# Gateway qualification\nRead the sample with offset seven. Never retry a timed-out read automatically.".into()),
        reads: AtomicUsize::new(0), writes: AtomicUsize::new(0), waiting: AtomicUsize::new(0), release: Semaphore::new(0),
    }) })
    .request("proof.status", |c: Context<Provider>, _: Empty| async move {
        Ok::<_, BackendError>(json!({"reads":c.state().reads.load(Ordering::SeqCst),"writes":c.state().writes.load(Ordering::SeqCst),"waiting":c.state().waiting.load(Ordering::SeqCst)}))
    })
    .request("proof.release", |c: Context<Provider>, _: Empty| async move {
        c.state().release.add_permits(12); Ok::<_, BackendError>(json!({}))
    });
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
                |c: Context<Provider>, _: PortActor, _: Value| async move {
                    c.state().writes.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, BackendError>(json!({}))
                },
            );
    }
    router.run().await
}
