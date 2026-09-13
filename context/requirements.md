# Resonance — Requirements

Root requirements for the Resonance product. Subsystem requirements live in `01-product/` and `02-system/`. Prefix: `RES`.

## RES-R01 — Installation Identity

The system SHALL generate a permanent Ed25519 keypair on first launch. The `SecretKey` SHALL serve as the Iroh endpoint secret. The `PublicKey` SHALL serve as the Iroh `EndpointId` and the installation's public identity. The private key SHALL never leave the runtime.

## RES-R02 — Workspace Creation

A user SHALL be able to create a named workspace. Creation SHALL produce an immutable workspace identity, a canonical membership log seeded with the creator, and a recoverable staged protocol that survives interruption.

## RES-R03 — Invitations

A workspace member SHALL be able to generate a single-use, cryptographically-signed invitation token. The token SHALL carry the inviter's identity, the workspace identity, and an expiry. An installation that presents a valid, unexpired, unused invitation SHALL be admitted to the workspace. Used tokens SHALL NOT be reusable.

## RES-R04 — Canonical Membership

The system SHALL maintain a signed, causally ordered, append-only membership operation DAG. Every membership change (`admit`, `revoke`) SHALL have a content-derived ID and SHALL link to the causal heads known to its author. One immutable genesis operation SHALL anchor each workspace. The resolved DAG SHALL be the sole authority for workspace membership. Membership SHALL be interval-based with no gaps.

## RES-R05 — Revocation and Epochs

Revoking a member SHALL end their current membership interval. Every committed membership operation SHALL advance a workspace-local monotonic epoch and publish the resulting content-derived `MembershipVersion`. Package-specific keys and authorization SHALL rotate in response to versioned membership transitions. The revoked member SHALL retain past data but SHALL NOT read or write future content.

## RES-R06 — Package Model

All product-domain capability SHALL live in packages. Host-level workspace controls SHALL remain in the shell. A package SHALL declare its identity, the semantic capabilities it consumes, its scope, and its entrypoints in a manifest. The runtime SHALL activate, suspend, and dispose packages through a standard lifecycle. A package SHALL own its domain logic, storage, transport, UI, and package-scoped membership. Team and individual packages SHALL use the same programming interface. Package UIs SHALL be TypeScript web content; package backends SHALL be Rust modules.

## RES-R07 — Semantic Capabilities

A package SHALL declare versioned semantic capabilities (e.g., `conversations:v1`) in its manifest. The runtime SHALL fulfill capabilities through typed, secret-free interfaces over Tauri's invoke/event bridge. The shell SHALL NOT hard-code product-specific capability branches.

## RES-R08 — Shell

The Tauri shell SHALL own package mounting in webviews, workspace controls, workspace-level theming, and the desktop window lifecycle. It SHALL supply generic core capabilities (identity, membership) to packages. It SHALL NOT own product-domain behavior.

## RES-R09 — Conversations Package

The conversations package SHALL own channels, messages, channel membership, conversation keys, transport, storage, unread state, and its UI. It SHALL use Iroh for P2P message delivery between package instances. It SHALL consume canonical workspace membership as an eligibility ceiling. It SHALL NOT mutate canonical membership.

## RES-R10 — Local-First Operation

The system SHALL work offline. Locally authored content SHALL persist and synchronize when peers become reachable via Iroh. No operation SHALL require a central server.

## RES-R11 — P2P Transport

The system SHALL use Iroh for all peer-to-peer connectivity: QUIC transport, hole-punching, relay fallback, and peer discovery. Iroh's default public relays SHALL be the initial relay configuration, with custom relay support for teams that need it. All traffic SHALL be end-to-end encrypted; relays SHALL carry no content authority.

## RES-R12 — Git Integration

The system SHALL read registered local Git repositories. It SHALL NOT replicate repository files, snapshots, or repository-derived views to devices without a local clone. Git SHALL remain the sole transport for repository content.

## RES-R13 — Technology Stack

The core runtime, shell, and package backends SHALL be written in Rust. The desktop shell SHALL use Tauri v2. Package UIs SHALL be TypeScript. Iroh SHALL be the P2P transport layer.