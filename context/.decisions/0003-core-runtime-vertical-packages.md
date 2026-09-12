# 0003 — Core runtime with vertical packages architecture

**Status:** accepted (2026-09-12)

## Context

The reference Resonance implementation (V1, RFC 0011) fused the Conversations domain into the core runtime: membership transitions waited for conversation epoch construction, `ConversationRuntime` held concrete core types, and the workspace session imported conversation-specific records. A post-merge seam assessment (2026-09-12) declared a new direction: the core runtime owns only installation identity and canonical workspace membership, while every product domain is a vertical package that owns its backend, UI, storage, and transport.

This decision adopts that declared direction as the architecture for this project.

## Decision

### Core runtime owns

- Installation identity generation and custody (Ed25519 keypair, using or derived from Iroh's `SecretKey`)
- Workspace creation with immutable workspace identity
- Invitation token generation and validation
- Canonical membership: the signed, ordered, append-only membership log
- Membership intervals: `[admitted, revoked?)` with no gaps
- Revocation that advances the membership epoch
- Membership queries: current snapshot, historical `was_member()` checks
- Domain-separated signing/verification (no raw private key exposure to packages)
- Package lifecycle: activation, suspension, recovery, disposal
- Package-scoped durable storage handles
- The package manifest and semantic capability registry

### Core runtime does NOT own

- Any product-domain behavior (conversations, documents, repos, AI tooling)
- Package-specific membership or roles
- Package cryptography, keys, or transport
- Package storage schema or migrations
- Package UI or theming (beyond workspace-level token propagation)
- Raw Iroh transport handles exposed to packages (packages get semantic operations, not `Endpoint` handles)

### Package owns

- Its domain records, projections, and storage
- Package-specific membership, roles, and authorization
- Package cryptographic keys and key rotation
- Its transport mesh, delivery, acknowledgement, and recovery (using Iroh via package-scoped interfaces)
- Background processing and lifecycle tasks
- Its UI and styles (consuming workspace theme tokens)
- Its manifest declaring consumed semantic capabilities

### Shell owns

- Package mounting and disposal in Tauri webviews
- Workspace controls (create, join, leave, member list, invitations)
- Workspace-level theming (CSS custom properties propagated to mount points)
- Package error containment
- No product-domain behavior, no hard-coded product capability branches

### Membership is the contract

The core-to-package contract is narrow and semantic:
- workspace identity (immutable)
- local installation public identity and display metadata
- domain-separated signing/verification
- current canonical workspace-member snapshot
- canonical membership head/version with ordered transition subscription
- historical `was_member(identity, interval)` queries
- package-scoped durable storage handle
- explicit lifecycle signals: activate, suspend, recover, dispose

Packages consume canonical workspace membership as an eligibility ceiling. A package member or channel member must also be a canonical workspace member, but not every workspace member must belong to every package or channel.

### Iroh's role in this architecture

Iroh provides transport and peer identity, not membership or authorization (per Iroh's own documentation: "it is up to the application to decide if a particular peer is allowed to connect or not"). This is exactly the right seam:

- **Core runtime** uses Iroh's `SecretKey`/`EndpointId` as the installation identity primitive. The membership log is built above Iroh.
- **Conversations package** uses Iroh for peer-to-peer message delivery, authenticated by the `EndpointId`s that the core runtime certifies as workspace members. The package maintains its own allow-list derived from canonical membership.
- **Other packages** may use Iroh for their own transport needs, or use higher-level abstractions (Iroh blobs, Iroh docs) for content-addressed and mutable data.

## Rejected alternatives

### Monolithic runtime (Resonance V1 pattern)

Rejected. The seam assessment demonstrated that fusing Conversations into core makes core membership dependent on Conversations availability and prevents Conversations from becoming optional or replaceable.

### Iroh as the membership authority

Rejected. Iroh provides transport-level peer identity (`EndpointId`) but deliberately does not provide group membership, authorization, or revocation. The application must own these concerns. This is the right separation — Iroh connects peers; the runtime decides which peers are members.

### Free delegation membership (UCAN/Keyhive model)

Deferred. For v1, manager-only membership with strong removal is the safer default. Any current member with appropriate capability may issue invitations, but only explicit admission operations change the canonical log. Free delegation can be added later as a package-specific policy if needed.

## Consequences

- The core runtime is deliberately small. Its value is in what it doesn't do.
- Every product domain must be designed as a package from the start. No domain logic creeps into core.
- The membership epoch is the central coordination mechanism. Packages subscribe to epoch transitions.
- Package isolation is enforced at the crate level: packages cannot depend on core-internal modules. The Tauri webview boundary provides process-level isolation for package UIs.
- Iroh's transport is available to packages through typed, capability-scoped interfaces — not as raw `Endpoint` handles.