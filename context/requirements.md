# Teamarness — Requirements

Root requirements for the Teamarness product. Subsystem requirements live in `01-product/` and `02-system/`. Prefix: `TEAM`.

## TEAM-R01 — Installation Identity

The system SHALL generate a permanent Ed25519 keypair on first launch. The `SecretKey` SHALL serve as the Iroh endpoint secret. The `PublicKey` SHALL serve as the Iroh `EndpointId` and the installation's public identity. The private key SHALL never leave the runtime.

## TEAM-R02 — Workspace Creation

A user SHALL be able to create a named workspace. Creation SHALL produce an immutable workspace identity, a canonical membership log seeded with the creator, and a recoverable staged protocol that survives interruption.

## TEAM-R03 — Invitations

A Workspace member SHALL be able to generate a single-use, cryptographically signed Invitation. The Invitation SHALL carry the inviter's identity, the Workspace identity, and an expiry. An Installation that presents a valid, unexpired, unused Invitation SHALL be admitted to the Workspace. An expired unused Invitation or an Invitation spent by a different Installation SHALL be rejected. An exact retry by the already admitted Installation MAY recover the same committed admission after expiry and SHALL NOT create another admission.

## TEAM-R04 — Canonical Membership

The system SHALL maintain a signed, causally ordered, append-only membership operation DAG. Every membership change (`admit`, `revoke`) SHALL have a content-derived ID and SHALL link to the causal heads known to its author. One immutable genesis operation SHALL anchor each workspace. The resolved DAG SHALL be the sole authority for workspace membership. Membership SHALL be interval-based with no gaps.

## TEAM-R05 — Revocation and Epochs

Revoking a member SHALL end their current membership interval. Every committed membership operation SHALL advance a workspace-local monotonic epoch and publish the resulting content-derived `MembershipVersion`. Package-specific keys and authorization SHALL rotate in response to versioned membership transitions. The revoked member SHALL retain past data but SHALL NOT read or write future content.

## TEAM-R06 — Package Model

All product-domain capability SHALL live in Packages. Host-level Workspace controls SHALL remain in the Shell. A Package Manifest SHALL declare its Package Identity, compatibility requirements, consumed Semantic Capabilities, and entrypoints. Teamarness SHALL determine Package Source, Package Registration Scope, Package Admission, and Package Trust outside the Package Manifest. The Package Host SHALL activate, suspend, recover, and dispose Package Instances through a standard lifecycle. A Package SHALL own its domain model, records, schemas, projections, policy, UI, and Package-specific membership. Package Backends SHALL run as supervised executables through one language-neutral Package Process Protocol. Phase 3 SHALL ship a Rust Backend SDK and Rust reference Package without requiring all conforming Package Backends to use Rust.

## TEAM-R07 — Semantic Capabilities

A Package SHALL declare consumed, versioned Semantic Capabilities in its Package Manifest. Teamarness SHALL fulfill capabilities for Teamarness-owned authority and generic behavior through typed, secret-free interfaces. Future generic Synchronization Capabilities SHALL keep replication, convergence, retry, and Core-owned Iroh transport out of Package Backends while leaving Package records, schemas, projections, and domain policy Package-owned. Package Backend capability traffic SHALL cross the Package Process Protocol. Package UI traffic SHALL cross Tauri's invoke and event bridge. The Shell SHALL NOT hard-code product-specific capability branches.

## TEAM-R08 — Shell

The Tauri Shell SHALL own Package UI Mounts, Package navigation, Workspace controls, Workspace-level theming, and the desktop window lifecycle. It SHALL route generic Package contracts but SHALL NOT own product-domain behavior. Changing the visible Package or Workspace SHALL NOT itself suspend a Package Backend.

## TEAM-R09 — Conversations Package

The Conversations Package SHALL own Channels, Messages, Channel Membership, conversation keys, domain policy, local projections, unread state, and its UI. It SHALL consume Canonical Membership as an eligibility ceiling and SHALL NOT mutate it. Its peer replication SHALL use a generic Package-scoped Synchronization Capability rather than a Conversations-specific Core Runtime branch.

## TEAM-R10 — Local-First Operation

The system SHALL work offline. Locally authored content SHALL persist and synchronize when peers become reachable via Iroh. No operation SHALL require a central server.

## TEAM-R11 — P2P Transport

The system SHALL use Iroh for all peer-to-peer connectivity: QUIC transport, hole-punching, relay fallback, and peer discovery. Iroh's default public relays SHALL be the initial relay configuration, with custom relay support for teams that need it. All traffic SHALL be end-to-end encrypted; relays SHALL carry no content authority.

## TEAM-R12 — Git Integration

The system SHALL read registered local Git repositories. It SHALL NOT replicate repository files, snapshots, or repository-derived views to devices without a local clone. Git SHALL remain the sole transport for repository content.

## TEAM-R13 — Technology Stack

The Core Runtime and Shell SHALL be written in Rust. The desktop Shell SHALL use Tauri v2. Phase 3 SHALL ship a Rust Package Backend SDK and a Rust reference Package, while the Package Process Protocol SHALL remain language-neutral. Package UIs SHALL be TypeScript. Iroh SHALL be the Core Runtime's P2P transport layer.