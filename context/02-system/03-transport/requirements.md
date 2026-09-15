# 02-system/03-transport — Transport Requirements

Prefix: `TEAM.SYS.TRANS`

The Core Runtime transport layer uses Iroh for QUIC connectivity, hole-punching, relay fallback, and peer discovery. Each Installation binds an Iroh `Endpoint` from its `SecretKey`. Phase 2 uses that endpoint for Workspace onboarding. Later generic Synchronization Capabilities use Core-owned transport without exposing raw endpoint handles to Packages. The first capability is the generic immutable-record log; it owns replication, retry, bounded sessions, and backpressure while Packages provide canonical domain bytes.

## TEAM.SYS.TRANS-R01 — Iroh Endpoint Management

The core runtime SHALL create and manage the Iroh `Endpoint`. The endpoint SHALL use the installation's `SecretKey`. The runtime SHALL bind the endpoint on startup and close it on shutdown.

## TEAM.SYS.TRANS-R02 — Relay Configuration

The system SHALL use Iroh's default public relays (`RelayMode::Default`) for NAT traversal fallback. Phase 2 SHALL use that default configuration. A later phase SHALL let teams configure custom relay servers. All relayed traffic SHALL be end-to-end encrypted; relays SHALL NOT have access to plaintext.

## TEAM.SYS.TRANS-R03 — Package Transport Access

Packages SHALL access Teamarness-owned peer transport through versioned, typed Semantic Capabilities. Future Synchronization Capabilities SHALL own reusable Iroh connection, replication, convergence, retry, and backpressure machinery for Package-defined records. The Core Runtime SHALL NOT expose raw `Endpoint` handles, `SecretKey` material, or relay configuration to Packages. A future custom-transport capability MAY provide a Package-scoped escape hatch without exposing those resources.

## TEAM.SYS.TRANS-R04 — Copyable Workspace Invitation

Phase 2 SHALL let a member copy one transfer value that contains a single-use workspace invitation and enough public connection information for another installation to reach the inviter through Iroh. The transfer value SHALL NOT contain private key material or make a central service authoritative for admission.

## TEAM.SYS.TRANS-R05 — Workspace Bootstrap Transfer

After accepting an invitation, the joining installation SHALL receive the authenticated workspace identity, genesis, canonical membership history needed to validate the workspace, and the operation admitting that installation. It SHALL validate the received state before storing the workspace locally. After a successful join, inviter and joiner SHALL report the same workspace identity and current member set. An interrupted or retried transfer SHALL NOT create a false local join or a duplicate admission. If the admitting installation commits an admission before its response is lost, a retry by the same joining installation SHALL recover that result without making the invitation available to another installation.