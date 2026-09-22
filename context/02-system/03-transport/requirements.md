# 02-system/03-transport — Transport Requirements

Prefix: `TEAM.SYS.TRANS`

The Core Runtime transport layer uses Iroh for QUIC connectivity, hole-punching, relay fallback, and peer discovery. Each Installation binds an Iroh `Endpoint` from its `SecretKey`. Phase 2 uses that endpoint for Workspace onboarding. Later generic Synchronization Capabilities use Core-owned transport without exposing raw endpoint handles to Packages. The Package-facing Synchronization Capability contract is owned by `TEAM.SYS.PKG-R10`.

## TEAM.SYS.TRANS-R01 — Iroh Endpoint Management

The core runtime SHALL create and manage the Iroh `Endpoint`. The endpoint SHALL use the installation's `SecretKey`. The runtime SHALL bind the endpoint on startup and close it on shutdown.

## TEAM.SYS.TRANS-R02 — Relay Configuration

The system SHALL use Iroh's default public relays (`RelayMode::Default`) for NAT traversal fallback. Custom relay configuration is unscheduled; see `roadmap.md` Later (unscheduled). All relayed traffic SHALL be end-to-end encrypted; relays SHALL NOT have access to plaintext.

## TEAM.SYS.TRANS-R03 — Package Transport Access

Packages SHALL access Teamarness-owned peer transport through versioned, typed Semantic Capabilities. The generic immutable-record capability SHALL use finite production LogSync sessions over a Core-owned durable operation projection, with continuation recorded as durable wakes. Invalid or out-of-order operations SHALL be isolated without partially committing Teamarness records. The Core Runtime SHALL NOT expose raw `Endpoint` handles, `SecretKey` material, relay configuration, P2Panda types, or SQLite handles to Packages through any Teamarness contract (see PKG-R11). A future custom-transport capability MAY provide a Package-scoped escape hatch without exposing those resources.

## TEAM.SYS.TRANS-R04 — Copyable Workspace Invitation

Phase 2 SHALL let a member copy one transfer value that contains a single-use workspace invitation and enough public connection information for another installation to reach the inviter through Iroh. The transfer value SHALL NOT contain private key material or make a central service authoritative for admission.

## TEAM.SYS.TRANS-R05 — Workspace Bootstrap Transfer

After accepting an invitation, the joining installation SHALL receive the authenticated workspace identity, genesis, canonical membership history needed to validate the workspace, and the operation admitting that installation. It SHALL validate the received state before storing the workspace locally. After a successful join, inviter and joiner SHALL report the same workspace identity and current member set. An interrupted or retried transfer SHALL NOT create a false local join or a duplicate admission. If the admitting installation commits an admission before its response is lost, a retry by the same joining installation SHALL recover that result without making the invitation available to another installation.

## TEAM.SYS.TRANS-R06 — Synchronization Peer Authorization

Every Core synchronization protocol SHALL identify the remote peer by its authenticated Iroh `EndpointId`, which is its Installation `PublicIdentity`. A session that serves Package records, the Workspace author list, or content-key grants SHALL start only after a bounded opening message names the Workspace and only when both the local Installation and the remote peer are current Members of that Workspace in the local accepted membership; otherwise the peer SHALL receive an explicit refusal and nothing else. A refused peer that may be newly admitted SHALL be able to prove its admission through membership replication and retry. Accepting a revocation SHALL cancel sessions with the revoked Installation. One unacceptable received operation SHALL NOT end a session: it SHALL be rejected, or, when its author's admission or the scope's Content Key is not yet known, parked within fixed per-scope and per-peer bounds and applied when that state arrives. A first Content Key for a scope SHALL be accepted only with the Workspace creator's signed origin for that key.

## TEAM.SYS.TRANS-R07 — Bounded Synchronization Resources

Core synchronization SHALL bound the memory and concurrency of every session. A responder SHALL limit concurrent synchronization sessions globally and per peer before reading a peer's opening message, and SHALL wait a bounded time for that message. Every frame and every session SHALL have fixed size and count bounds, and a peer that exceeds one SHALL be disconnected without losing operations already applied. Outgoing operations SHALL be read from storage incrementally and sent with transport backpressure, and a session that stops at its bound SHALL be continued by a later session. Record delivery to Packages SHALL hold no per-subscriber queue in Core. Storage and cryptographic work SHALL NOT run on asynchronous executor threads, and Package capability-call deadlines SHALL take effect while Core is busy. Scheduling SHALL back off only after failures and only for the failing peer, and SHALL NOT delay other peers or shutdown. A corrupt local record or scope SHALL be isolated without preventing the Core Runtime or other scopes from operating.
