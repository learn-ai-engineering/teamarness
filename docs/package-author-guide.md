# Package author guide

A Teamarness Package is a directory of files, not a Rust library or a dynamic ABI. Build the complete Package Directory in your own repository, test it as an installation-local Package, then give the same declared bytes to the Teamarness application build. For a supported starter project, run `npm run create:package -- --output <directory> --identity <reverse-dns-id> --display-name <name>` from a Teamarness checkout. The generator creates a foundation outside the Teamarness workspaces and vendors the current Rust and TypeScript SDK source with exact lockfiles. Generated SDK copies have no supported in-place upgrade yet.

Teamarness supports `aarch64-apple-darwin`, and ships a Rust Backend SDK and a TypeScript UI SDK. The Package Process Protocol is language-neutral, but no other production Backend SDK exists yet.

## Package Directory

The directory root contains `teamarness.package.json`. The manifest points to one target Backend, one ESM UI entrypoint, optional UI assets, and one UI contract. The reference Package has this shape:

```text
teamarness.package.json
backend/
  teamarness-reference-package-aarch64-apple-darwin
ui/
  index.js
  contract.json
  probe.svg
```

You own executable packaging. Ship a self-contained executable for every declared target. Teamarness does not install Node, Python, shared libraries, language runtimes, or Package dependencies. It starts the exact admitted Backend path without a command shell and uses the Package Data Directory as its working directory.

Do not use symlinks. Every manifest path must be normalized relative UTF-8 with no empty, `.`, `..`, absolute, or backslash segment. Compute each declared byte size and lowercase SHA-256 digest after the final build. Any later byte change invalidates admission until Teamarness restarts and admits the directory again.

## Package Manifest

The normative schema is [`contracts/packages/v1/schemas/manifest.schema.json`](../contracts/packages/v1/schemas/manifest.schema.json). Admission also applies stricter semantic checks described in [`contracts/packages/v1/README.md`](../contracts/packages/v1/README.md).

A version 1 manifest declares:

- `identity`, a lowercase reverse-DNS Package Identity;
- a strict Semantic Version in `version`;
- `presentation.displayName`;
- one Package Process Protocol major and minor range;
- Backend files with exact target, path, size, and digest;
- the ESM entrypoint, assets, and UI contract;
- every required versioned Semantic Capability;
- optional Package-owned metadata under reverse-DNS `extensions` keys.

Source, trust, override policy, installation paths, and data paths do not belong in the manifest. Keep registration metadata outside the Package Directory.

The current application selects exactly one `aarch64-apple-darwin` Backend. The Backend executable implements [`contracts/package-process/v1`](../contracts/package-process/v1/README.md). It must keep reading control frames while work runs, honor lifecycle requests, and exit when stdin closes. Rust authors can use [`teamarness-package-sdk`](../crates/teamarness-package-sdk/README.md).

## UI contract and module

The UI contract is a closed JSON document:

```json
{
  "schemaVersion": 1,
  "$defs": {},
  "requests": {
    "example.read": {
      "request": { "type": "object", "additionalProperties": false },
      "response": { "type": "object" }
    }
  },
  "subscriptions": {
    "example.changes": {
      "request": { "type": "object", "additionalProperties": false },
      "event": { "type": "object" }
    }
  }
}
```

Every request has separate request and response schemas. Every subscription has separate request and event schemas. Operation names use lowercase dotted segments. Only local `#/$defs/...` references are accepted. Remote references, dynamic references, cycles, unknown descriptor fields, schemas deeper than 64 levels, and contracts over 16,384 schema nodes fail admission.

The contract file may use at most 1 MiB. Each generic request, response, subscription request, or event may use at most 256 KiB of encoded JSON and 64 levels. A contract may declare at most 128 requests, 128 subscriptions, and 256 operations total.

Build one UTF-8 ESM entrypoint of at most 8 MiB. Declare at most 64 static assets, each at most 8 MiB. The entrypoint, assets, and contract may total at most 32 MiB. Do not emit undeclared chunks or remote imports.

Export `mount(context)`. The context provides a fresh Shadow Root inside the Package UI Frame, Package and Workspace presentation, theme token names, the UI host version, and a mount-scoped typed client. Return an idempotent unmount function. `context.client` gives requests, cancellation, ordered subscriptions, and post-listener acknowledgement; `@teamarness/package-ui-sdk` supplies the types and the optional CSS kit.

### Package UI frame restrictions

The Shell runs each Package UI Mount in its own sandboxed frame with an opaque origin. Inside it:

- `localStorage`, `sessionStorage`, IndexedDB, and cookies are unavailable.
- `fetch`, XHR, and WebSocket are denied by the frame's policy. All data goes through `context.client`.
- Tauri commands, Tauri globals, the Shell's DOM, and other Packages are unreachable.
- There are no popups, no `window.open`, and no `alert`, `confirm`, or `prompt`.
- A `position: fixed` overlay covers the Package pane, not the window.
- Navigating the frame away faults the mount.
- Any uncaught error or unhandled rejection anywhere in the frame is a UI fault, not only one attributed to your module.
- The `__teamarness/` path of the resource URL is reserved for the Shell-owned frame document, stylesheet, and runtime. A declared asset under that path is shadowed.
- WebAssembly can run: the frame's policy allows `'wasm-unsafe-eval'` and nothing more ([ADR 0031](../context/.decisions/0031-package-ui-frames-run-webassembly.md)). Ship the module's bytes in your entrypoint or a declared asset; you cannot fetch it. `eval` and `new Function` stay refused, and workers stay denied. A runaway module freezes the Shell as runaway JavaScript does.
- Some controls misbehave under the frame policy ([DELTA-014](../context/.delta/DELTA-014-package-ui-frame-form-and-select-behaviour.md)). Creating a `<select>` element makes WebKit report a `style-src-attr` violation, so use buttons or a listbox built from other elements. Chromium does not fire `submit` in the sandboxed frame, so make every form act on its button's click and keep `submit` only as a fallback.

### Theme and Package-owned composition

The UI SDK's [Package UI theme and CSS kit guide](../packages/teamarness-package-ui-sdk/README.md) is authoritative for the exact 12 semantic token names and the optional class-based kit. The Shell sends token values across the frame bridge and the frame runtime defines them on the frame document root. Use the tokens inside your Shadow Root so System, Light, and Dark changes apply without a Package event.

A Package owns its complete internal composition, including its Package-owned header, secondary navigation, forms, state feedback, and domain content. The Shell owns Workspace and Package selection, the mount, inherited token values, and generic recovery; do not copy Shell navigation into a Package. The optional `installPackageUiKit(root)` helper supplies only common controls, fields, validation, notices, panels, flow, and visible focus. It uses a constructed stylesheet in the Shadow Root's `adoptedStyleSheets`. Retain its cleanup callback and invoke it from your idempotent unmount function.

The kit also includes opt-in page-structure classes whose values mirror the Shell's screens: `teamarness-page`, `teamarness-page--readable`, `teamarness-page-header`, `teamarness-eyebrow`, `teamarness-page-title`, `teamarness-page-subtitle`, `teamarness-section`, and `teamarness-section-title`. Use them when a Package page should read like a Shell screen:

```html
<main class="teamarness-page teamarness-page--readable">
  <header class="teamarness-page-header">
    <h1 class="teamarness-page-title">Milestones</h1>
    <p class="teamarness-page-subtitle">Track what the team is shipping next.</p>
  </header>
  <section class="teamarness-section">
    <h2 class="teamarness-section-title">Current milestone</h2>
  </section>
</main>
```

The frame document policy rejects runtime inline `<style>` elements and `style` attributes. Package-owned styling must respect that policy too. Use a separate constructed `CSSStyleSheet` in the Package Shadow Root, and remove only that sheet during unmount. Do not fall back to inline styles, nonces, hashes, `data:` styles, or `blob:` styles if constructed stylesheets are unavailable.

The Package UI Frame loads admitted resources through `teamarness-package://`. The loader accepts only `GET` and `HEAD` for exact admitted paths. It rejects traversal, encoded separators, query selectors, stale tokens, undeclared files, changed cache bytes, and MIME changes. Responses disable sniffing and storage. Package code never receives the mount token or a source, cache, or data path.

## Validate without the GUI

`teamarness-package-validate` applies Package Admission's Package Manifest and UI contract rules, and answers with the same verdict the Package Host reaches. A generated project runs it as `npm run validate` for the project it is in, and `npm run validate:dist` for the assembled Package Directory, where `--smoke` also launches the Backend through handshake, a ping, activation and disposal.

```sh
# The project you are editing: placeholders and unbuilt artifacts are tolerated.
cargo run -p teamarness-package-contract --bin teamarness-package-validate -- source .
# An assembled Package Directory: exact bytes, sizes, digests, and a Backend smoke launch.
cargo run -p teamarness-package-contract --bin teamarness-package-validate -- directory dist/<identity> --smoke
```

A failure names the rule that failed and where, for example `error[ui_contract_violation/ui_contract.reference_not_local] ui-contract.json#/requests/...`. `--json` prints the same verdict as data. Exit 0 is admitted, 1 is refused, 2 is a usage error. In this repository, `npm run validate:packages` validates every bundled Package's source and staged directory.

## Package conventions

A Rust Backend registers typed handlers on a `Router` and lets the SDK own the protocol:

| Hook | When it runs | What it owns |
|---|---|---|
| activation | `lifecycle.activate`, and `lifecycle.recover` for a replacement generation | builds the state every handler shares |
| `on_suspend` | before the Instance is suspended | stops background work; the state is kept |
| `on_resume` | when a suspended Instance is recovered in the same process | restarts what suspend stopped |
| `on_dispose` | when the state is discarded | releases what the state holds |

`main.rs` stays thin: `#[tokio::main] async fn main() -> ExitCode { my_package::router().run().await }`.

Name a Package error code after your Package — `emojart.storage_failed` — and mark it `retryable` only when repeating the same request may succeed. The SDK owns the codes that are not about your product: `package.not_active`, `package.unknown_operation`, `package.unknown_subscription`, `package.invalid_payload`, `package.blocking_task_failed`, `package.capability_not_granted`, and `cancelled`.

Anything that blocks — files, SQLite, hashing, a directory walk — belongs in `ctx.blocking(...)`, which moves it off the async runtime and hands the closure a cancellation token. The token fires when the caller cancels, when the request deadline passes, and when the Instance is suspended or disposed. A handler that blocks the runtime stalls every other request, including the host's health probe.

`backend/tests/` drives the Backend through `teamarness_package_sdk::testing`: activate, request, subscribe, suspend, recover, dispose and shutdown against a real temporary Package Data Directory, with no hand-built frames. `assert_router_matches_contract` proves the router and `ui-contract.json` declare the same operations.

In the UI, `adoptPackageStyles(context.root, css)` adopts one Package-owned stylesheet and returns the cleanup your unmount calls.

## Package errors in the UI

A Package Backend's error reaches the Package UI unchanged: its stable code, its retryability, and up to 4 KiB of bounded `details`, beside the Teamarness code that says what Teamarness did with the failure.

```ts
import { describePackageError, isAbortError, packageErrorCode } from "@teamarness/package-ui-sdk";

try {
  await client.request("emojart.save", { recipe });
} catch (error) {
  if (isAbortError(error)) return;                  // the mount went away
  if (packageErrorCode(error) === "emojart.storage_failed") retryLater();
  else show(describePackageError(error));           // never includes details
}
```

`isPackageRequestError(error)` narrows an unknown rejection; `error.packageError` is `null` when Teamarness owns the failure. A subscription's `onTerminated` is called once with the Package's terminal error, or `null` when the Package completed the stream; neither recovers the mount by itself. Show a code, not raw `details`: details are your data, not display text.

Generate the UI's request types from the contract with `teamarness-contract-types --contract ui-contract.json --out ui/src/contract.generated.ts --type-name PackageContract`, commit the result, and call through `typedPackageClient<PackageContract>(context.client)`. The UI then cannot name an operation, or build a payload, the contract does not describe, and a stale generated module fails `npm run typecheck`.

## Consume a Package Port

Declare `teamarness.package-ports` 1.0 and each contract identity, version range, Port name, and kind in `consumedPorts`. A Provider declares its independently versioned `portContract` file and SHA-256 digest. The [Port declaration reference](../contracts/package-ports/v1/README.md) lists the bounds and schemas. The Host resolves one Provider per contract at admission, limits each grant to the declared ports in the calling Workspace, and does not let you select an actor or signer. A persisted Provider stays selected when it participates in a feasible graph, irrespective of discovery order or recent calls. A dense contested dependency component can instead be refused with `port_graph_search_limit` after 4,096 search states; inspect the authoring catalog, fix invalid declarations or reduce competition, and reopen the Host. Unrelated Packages continue.

In Rust, find the granted token through `activation.initialization().capability("teamarness.package-ports")`, then construct `ports::PortClient` with `activation.capabilities().clone()` and that token. Use `command`, `query`, or `subscribe_event` for the declared kind. Subscribe explicitly before querying your initial page. Treat `messages.changed` from Conversations as a live, coalescible invalidation with an empty payload. A display can refresh `message.page` after an event. A Consumer that must process each newly delivered Message should declare Conversations 1.2 `message.changes`: save the initial page's epoch/revision, then query `{channelId, epoch, after, limit?}` after notification or reconnection. Process revision-ordered changes and acknowledge only an examined prefix; continue bounded batches when behind, and retain unresolved pending content for safe query retry. Do not treat a Message's display position or timestamp as delivery progress. A changed epoch reports `resyncRequired`; choose an explicit recovery policy rather than replaying side effects. The Agent Package stops on invalid saved continuity and never retries an already settled reply.

Its `message.post` command (Conversations 1.1) signs as the Host-derived actor: the local Installation Person, or the Workspace Agent your Package is bound to through `teamarness.agent-binding` (see [Act as a Workspace Agent](#act-as-a-workspace-agent)). Its `channel.list` query returns the Channels you may post to. Documents 1.1 `document.revise` accepts the same two actors, so a bound Agent's revision is signed with the Agent's key; Core's refusal surfaces as `documents.principal_not_bound`. The version a revise returns may include concurrent edits, so read again before revising. For a non-object Provider event, use `EventSender::send_port_value` rather than `send`; the Port router wraps every Provider response/event for Package Process 1.0 and the Host validates the original JSON value, including an object that resembles the reserved transport shape. The SDK Port client returns the original response or event value. A non-Rust Provider or Consumer must handle the reserved Port transport envelope documented in [Package Process 1.0](../contracts/package-process/v1/README.md). A Port Contract major can exceed 1 without changing Package Process or capability version 1.0. If your command durably commits before a later local step fails, return `BackendError::new(...).with_outcome_uncertain(true)`; the SDK sends `outcomeUncertain:true` and disables retry. Never retry an `outcomeUncertain` command automatically. The test-only SDK Consumer in `tests/system/src/bin/conversations_port_consumer.rs` exercises this sequence without a UI mount.

For local diagnosis, `PackageHost::observe_ports()` and `Package::observe_ports()` provide bounded receivers. Acquire the Host receiver before making calls. Match a Consumer process scope and `p:` ID to the Host's assigned Provider `h:` ID; a shared event upstream has multiple Consumer scopes. A slow receiver reports `Lagged` and does not block routing. Neither receiver carries payloads or proves a write committed. To log the Conversations Provider's SDK observations during development, start its Backend with `TEAMARNESS_CONVERSATIONS_PORT_DIAGNOSTICS=1`. The resulting structured lines go to the Host's private rotated stderr files under `packages/logs/`; normal operation emits no Port observations. These files are not an in-app diagnostics API. Do not copy Message text, actor identity, grants, keys, or records into your own diagnostics.

## Act as a Workspace Agent

A Workspace Agent is a Principal with a Core-custodied signing key and a Steward (ADR 0023). A Package acts as one through an Agent Binding: Installation-local Core state, keyed by Workspace and Package Identity, that names the one Agent this Package Instance acts as. Declare `teamarness.agent-binding` 1.0 and, to notice removals and renames, `teamarness.principals` 1.0. The [capability reference](../contracts/package-capabilities/v1/README.md) lists the methods `current`, `candidates`, `bind`, and `unbind`. The grant fixes your Package and Workspace, so a request never names a Package and you cannot read or change another Package's binding. `candidates` lists only the Agents this Installation stewards; `bind` on another Person's Agent fails with `agent_not_stewarded`, and `bind` on an unknown Principal fails with `agent_not_found`.

In Rust, build `capabilities::agent_binding::AgentBinding::from_initialization(activation.initialization(), activation.capabilities().clone())` and call `bind(principal_id)`. Once bound, two things change (ADR 0030):

- Every Port command and live query your Package makes carries `actor:{kind:"agent",principal,identity}` instead of the Person actor. The Host reads the binding at each dispatch; event subscriptions carry the Person. A Provider that accepts the Agent kind, such as Conversations 1.1 `message.post`, appends with that `principal`, so the record is signed with the Agent's key and every Installation attributes it to the Agent. A Provider that serves only Persons refuses the Agent kind with its own error code.
- Core signs a record-log 1.1 append that carries the Agent's `principal` for any Package Instance on this Installation, not only yours. The rule is: the Agent is a current Workspace Principal, its Steward's key group contains this Installation's key, and the Agent has at least one binding here. Anything else is `principal_not_bound`.

Core clears the binding when this Installation removes the Agent, and `current` answers `null` once the Agent is no longer current, including after a peer removes it. Re-read `current` on each `teamarness.principals` snapshot to notice removal and rename; the Agent's current name is also how Members address it. Bindings are never replicated, and no Shell control binds a Package. The optional standalone `dev.teamarness.agent` Personal Package follows this pattern: Package-owned settings, a key file with mode `0600` in the Package Data Directory, and a mention-triggered reply loop over the Conversations Ports.

For a Provider, accept `ports::PortActor::Agent { principal, identity }` where your contract allows it and pass `principal` to `RecordLog::append_with_principal`; refuse the kind with your own code where it does not. Do not store bindings and do not select a signer from request content.

## Capabilities and local data

Declare only the capabilities you call. Teamarness provides Workspace context, canonical membership with membership transitions and historical membership checks, Workspace Principals, the Agent Binding, domain-separated signing, and the immutable record log for Package-defined synchronized records. Each is a capability **family** with a version: a Package declares a family and the versions it accepts, and the Host grants one. Grants bind one Package Identity, Workspace, Installation, and process generation. A Package cannot select another scope. Private keys, Core database handles, Core types, raw secret bytes, and Iroh handles never cross this interface.

The `initialize` frame grants an absolute Package Data Directory, which the SDK gives an activation hook as `Activation::data_directory()`. Your Backend owns that directory's files, schema, and migration policy. The directory remains bound to Installation, Workspace, and Package Identity across Package source changes, versions, process generations, disposal, and application restarts. Teamarness does not inspect, migrate, back up, or delete Package-owned data.

## What can change under you

Three things cross the Package boundary, and each carries its own version.

| Line | Version today | What it covers |
|---|---|---|
| Package Process Protocol | 1.0 | the frame set, the lifecycle operations, the `initialize` handshake |
| Semantic Capability family | one per family, for example the record log's | that family's request and event shapes, bounds, vocabularies, and error codes |
| Package UI bridge | `uiHostVersion` 1 | the mount contract your `mount(context)` is given |

A capability is negotiated at activation: you declare a family and the versions of it you accept, the Host offers a major and a range of minors, and the highest minor both accept is granted. Within a major, a minor only adds — new optional request fields, new event kinds you may ignore, wider bounds. Nothing is removed or narrowed, and a value added to a closed vocabulary such as the record log's `availability` states is a new major, not a minor, so your Package never has to interpret a state it does not know.

A capability family the Host does not provide is an unavailable capability, not a version error. A **new** family — document-oriented synchronization, say — is added beside the existing ones. It changes no existing family, no lifecycle operation, no protocol version, and no UI contract, so a Package that does not declare it is unaffected.

When a breaking change eventually lands, it comes with a deprecation window: the Host supports the previous major alongside the new one for one release cycle, the Package Catalog reports which Packages still use a superseded major, and these docs and `npm run validate` warn you before the Host refuses. No such window exists yet, because nothing has broken yet. See ADR 0018, Package seam versioning.

## Trust limits

A Package Backend is trusted native code. It has the current user's ambient filesystem, network, and process authority. Process supervision handles ordinary exits, deadlines, graceful shutdown, and conservative orphan cleanup. It is not a sandbox and does not contain hostile code or deliberately evasive descendants.

A Package UI runs in a sandboxed frame with an opaque origin, so it reaches neither the Shell's DOM nor any Tauri command, and it has no storage and no network. Frame isolation separates realms, DOM, storage, and Tauri access; it does not separate threads, so a Package UI that blocks its thread freezes the Shell, and it places no limit on what the Package Backend can do. Review both Backend and UI code before trusting a Package.

## Bundled and local Package Source kinds

`packages/tutorial/` is checked-in Tutorial source. The application builder turns it into the bundled `dev.teamarness.tutorial` Package Directory under `packages/dev.teamarness.tutorial/`. A generated learner project such as `dev.teamarness.emojart` remains installation-local and is admitted through the owner-only registration file. Package Source values `bundled` and `local` remain canonical technical source data; the Shell presents them as `Team` and `Personal`. `Team` and `Personal` are UI aliases, not trust verdicts, and do not define registration scope or imply Workspace enablement, synchronization, sharing, networking, or collaboration.

## Local development loop

1. Run `npm run validate` after every contract or manifest change.
2. Build the final Backend executable, ESM entrypoint, declared assets, and UI contract in the Package repository.
3. Assemble the Package Directory and write final sizes and SHA-256 digests into `teamarness.package.json`. `npm run build` does both and validates the result.
4. Follow [Local Package registration](local-package-registration.md). Use the manifest identity as `expectedIdentity`, accept the native-code risks, and restart Teamarness.
5. Confirm the Package UI's display-safe readiness labels and exercise requests, subscriptions, restart, and data continuity. Workspace settings offers Recover Backend and Dispose only for a Package Instance that needs recovery. Use development logs for protocol diagnostics rather than adding identifiers, trust enums, lifecycle enums, or process generations to normal Package UI.
6. Rebuild in place when needed, then restart. There is no source watch or hot reload for Backend bytes.

## Promotion to application distribution

Promotion changes distribution metadata, not Package-owned code or contracts. Give the release Package Directory to an application maintainer. The maintainer adds one reviewed entry (source directory and Cargo Backend package) to `packages/bundled-packages.json`, the single list of bundled Packages, and runs `node scripts/bundled-packages.mjs --write` to regenerate the matching Tauri resource and external-binary configuration; `npm test` fails if they diverge. The builder remains the only writer of `packages/application-packages.v1.json`; it stages and verifies every declared byte, size, and digest. Package authors do not edit that generated index or copy Tauri resources by hand. Run `npm run build:application-packages`, then remove the installation override and restart to confirm the bundled source uses the same Package Data Directory.

Signing and notarization, Gatekeeper remediation, other operating systems, remote Package fetching, Package updates, hot reload, enablement policy, and synchronization engines remain outside the current delivery claims.
