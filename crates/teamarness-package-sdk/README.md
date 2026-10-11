# teamarness-package-sdk

`teamarness-package-sdk` is the supported Rust SDK for Package Backend executables. The Package Process Protocol remains language-neutral. This crate is one adapter and does not make Rust part of the host contract.

## The router

A Backend registers typed handlers and lets the SDK own the protocol:

```rust,no_run
use teamarness_package_sdk::{Context, Empty, Router};

struct State { greeting: String }

#[derive(serde::Serialize)]
struct Read { message: String }

pub fn router() -> Router<State> {
    Router::new(|activation| {
        let directory = activation.data_directory().to_owned();
        async move {
            let _ = directory; // open your storage here
            Ok(State { greeting: "hello".into() })
        }
    })
    .request("example.read", |context: Context<State>, _: Empty| async move {
        Ok(Read { message: context.state().greeting.clone() })
    })
}

# async fn run() -> std::process::ExitCode {
router().run().await
# }
```

A request payload is deserialized into the handler's type and its answer serialized back, so a Backend never shapes JSON by hand. `Router::operations()` lists the registered names for comparison against `ui-contract.json`.

| Hook | When it runs | What it owns |
|---|---|---|
| activation | `lifecycle.activate`, and `lifecycle.recover` for a replacement generation | builds the state every handler of that activation shares |
| `on_suspend` | before the Instance is suspended | stops background work; the state is kept |
| `on_resume` | when a suspended Instance is recovered in the same process | restarts what suspend stopped |
| `on_dispose` | when the state is discarded, by dispose or by a replacing activation | releases what the state holds |

An `initialize` frame the host cannot have sent — a missing field, a malformed value — ends the session before any activation hook runs, so no half-typed `Initialization` reaches the Package. The session's end is what a Backend observes; do not depend on which end code it carries.

`Activation` carries the typed `Initialization`, the capability client, and the activation's cancellation token. `Context` carries the state, the same initialization and capabilities, this request's cancellation and deadline, and `blocking`.

## Cancellation, deadlines, and blocking work

`CancellationToken` fires when the host cancels the request, when its deadline passes, and when the Instance is suspended or disposed. `Context::blocking` moves a closure off the async runtime, gives it that token, and returns `cancelled` as soon as the token fires even while the closure is still finishing:

```rust,ignore
let rows = context.blocking(move |cancel| {
    for path in paths {
        if cancel.is_cancelled() { return Err(interrupted()); }
        // ... blocking work
    }
    Ok(found)
}).await?;
```

Anything that blocks — files, SQLite, hashing, a directory walk — belongs there. A handler that blocks the runtime stalls every other request. Framing, dispatch and health run on their own control-plane thread, so a `ping` is answered even while every Package worker is busy.

## Errors

`BackendError` carries a stable code, `retryable`, and a bounded details object, and all three reach the Package UI unchanged. Name a Package code after the Package (`emojart.storage_failed`), and mark it retryable only when repeating the same request may succeed.

The SDK owns the codes that are not about a product: `package.not_active`, `package.unknown_operation`, `package.unknown_subscription`, `package.invalid_payload`, `package.invalid_response`, `package.blocking_task_failed`, `package.capability_not_granted`, `package.invalid_capability_request`, `package.invalid_capability_response`, and the unprefixed `cancelled`.

## Storage helpers

`storage::write_atomic` writes through a temporary file, `fsync`s it, renames it, and `fsync`s the directory, so an interrupted write never leaves a half-file. `storage::JsonDocumentStore` keeps one JSON document that way: it writes before it adopts a new value, so a failed write leaves both the file and the in-memory value unchanged, and reports unparsable bytes as `StorageError::Corrupt` rather than losing them.

## Semantic Capabilities

`capabilities::{workspace_context, membership, person_names, signing, record_log}` are typed clients over the granted capabilities; each is built from the `Initialization` and the capability client. They decode the capability's own shapes, and answer `package.invalid_capability_response` rather than guessing when a value is not one the contract describes.

`person_names::PersonNames` requires a declared `teamarness.person-names` 1.0 grant. `current` returns at most 1,024 current Workspace Persons with Installation identity, opaque Person ID, optional name and fingerprint. `lookup` resolves one current Installation; `subscribe(after_cursor)` yields coalesced snapshots with a separate durable profile cursor. A missing name is not a membership failure. Use the cursor to refetch or resume after reconnection; do not store names in signed Package records or use them for authorization.

`record_log::run_session` drives a synchronized record log for a Package: it subscribes from the cursor the sink has committed, ingests each record, and only then returns credit, so a record whose ingest fails is redelivered rather than skipped. It resets the cursor once on `cursor_invalid`, reports state and warning events to the sink, and backs off on a retryable failure until its cancellation token fires.

`CapabilityClient` calls a negotiated grant and method. The caller supplies only method fields, not Package, Workspace, Installation, Core signing context, or transport selectors. `CapabilitySubscription::recv` validates contiguous decimal sequences and returns one credit after consumption; `recv_with_credit` hands the credit to the caller so it can be released after the work is committed.

## Testing a Backend without frames

The `testing` feature adds a harness that serves a Backend over in-memory pipes and speaks the protocol for the test:

```rust,ignore
let mut harness = HarnessBuilder::new(router().into_backend())
    .package_identity("dev.example.demo")
    .data_directory(directory.path())
    .capability(record_log::IDENTITY, scripted)
    .start()
    .await;
harness.activate().await?;
let answer = harness.request("example.read", json!({})).await?;
harness.shutdown().await?;
```

`ScriptedCapability` answers capability calls and streams with credit, `PendingRequest` cancels an in-flight request, and `assert_router_matches_contract` fails when the router and `ui-contract.json` do not declare the same operations.

## What the SDK depends on

The SDK depends only on `teamarness-package-protocol`, Tokio, serde and base64. It does not depend on Tauri, the Package Host, Core Runtime, Iroh, SQLite, or a host-managed language runtime. A Package chooses its own storage dependencies and uses the granted Package Data Directory directly.

The SDK owns framed stdin and stdout, strict initialization, concurrent request dispatch, lifecycle barriers, capability correlation, control-frame priority, cancellation, event credit, graceful shutdown, and the parent-stdin lifeline. Package diagnostics go to stderr. Package code must not write protocol data there or write non-protocol data to stdout.

Run the SDK suite from the repository root:

```sh
cargo test -p teamarness-package-sdk --all-features --locked
```
