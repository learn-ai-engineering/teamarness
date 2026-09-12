# 02-system/03-transport — Transport Requirements

Prefix: `RES.SYS.TRANS`

*Requirements for this subsystem will be authored during Phase 1-2 scoping. The transport layer is Iroh: QUIC connectivity, hole-punching, relay fallback, and DNS/pkarr-based peer discovery. Each installation binds an Iroh `Endpoint` from its `SecretKey`. Packages may use Iroh streams via capability-scoped interfaces — they do not receive raw `Endpoint` handles.*

## RES.SYS.TRANS-R01 — Iroh Endpoint Management

The core runtime SHALL create and manage the Iroh `Endpoint`. The endpoint SHALL use the installation's `SecretKey`. The runtime SHALL bind the endpoint on startup and close it on shutdown.

## RES.SYS.TRANS-R02 — Relay Configuration

The system SHALL use Iroh's default public relays (`RelayMode::Default`) for NAT traversal fallback. Teams SHALL have the option to configure custom relay servers. All relayed traffic SHALL be end-to-end encrypted; relays SHALL NOT have access to plaintext.

## RES.SYS.TRANS-R03 — Package Transport Access

Packages SHALL access Iroh transport through typed, capability-scoped interfaces. The runtime SHALL NOT expose raw `Endpoint` handles, `SecretKey` material, or relay configuration to packages. Packages SHALL register ALPN identifiers for their protocol streams.