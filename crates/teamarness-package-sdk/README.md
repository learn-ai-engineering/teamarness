# teamarness-package-sdk

`teamarness-package-sdk` is the supported Rust SDK for Package Backend executables. The Package Process Protocol remains language-neutral. This crate is one adapter and does not make Rust part of the host contract.

Implement the `Backend` trait and start the service with `Package::builder`:

```rust,no_run
use std::sync::Arc;
use teamarness_package_sdk::Package;

# async fn run<B: teamarness_package_sdk::Backend>(backend: Arc<B>) {
Package::builder(backend).serve().await.unwrap();
# }
```

`Backend::handle` receives lifecycle and Package UI requests without framing details. `Backend::subscribe` can return a bounded `EventSubscription`. `Initialization` exposes Package, Workspace, Installation, process generation, Package Data Directory, and negotiated opaque capability descriptors for the current generation.

`CapabilityClient` calls a negotiated grant and method. The caller supplies only method fields, not Package, Workspace, Installation, Core signing context, or transport selectors. Grants stop with the process generation.

The SDK owns framed stdin and stdout, strict initialization, concurrent request dispatch, lifecycle barriers, capability correlation, control-frame priority, cancellation, event credit, graceful shutdown, and the parent-stdin lifeline. Package diagnostics go to stderr. Package code must not write protocol data there or write non-protocol data to stdout.

The SDK depends only on `teamarness-package-protocol` and Tokio. It does not depend on Tauri, the Package Host, Core Runtime, Iroh, SQLite, or a host-managed language runtime. A Package may choose its own local storage dependencies and use the granted Package Data Directory directly.

Run the SDK suite from the repository root:

```sh
cargo test -p teamarness-package-sdk --locked
```
