# Resonance — Ontology

Canonical terminology for the Resonance domain. When a term appears in requirements, specs, code, UI, or conversation, it carries the meaning defined here. Divergence from this file in another surface is a bug.

## Top-level

| Term | Definition |
|---|---|
| **Resonance** | The application: a local-first, P2P, package-extensible team workspace |
| **Installation** | One running instance of Resonance on one device. Identified by an Ed25519 keypair that doubles as an Iroh `EndpointId` |
| **Workspace** | A named team context with canonical membership. Contains packages, conversations, and shared context. A single installation may belong to multiple workspaces |
| **Package** | A vertical module owning its domain: backend behavior, UI, storage, transport, and package-scoped membership. All product-domain capability lives in packages; host-level workspace controls live in the shell. UIs are TypeScript web content; backends are Rust modules |
| **Core Runtime** | The application kernel that owns installation identity, workspace creation/invitations, canonical membership, membership intervals, revocation, and the package lifecycle. Written in Rust |
| **Shell** | The Tauri desktop host that owns package mounting in webviews, workspace controls, workspace-level theming, and the desktop window lifecycle |

## Identity and Membership

| Term | Definition |
|---|---|
| **Installation Identity** | A permanent Ed25519 keypair generated on first launch. The `SecretKey` is the Iroh endpoint secret; the `PublicKey` is the Iroh `EndpointId`. This is the device's network identity |
| **Installation Display Name** | User-changeable, validated human-readable metadata for the local Installation. It does not replace the Public Identity and is not currently shared through canonical membership |
| **Public Identity** | The public half of an installation identity (`EndpointId`), shared with workspaces as a member identifier |
| **Member** | An installation identity admitted to a workspace. Membership is canonical and interval-based. A human may have multiple device identities; devices are first-class members |
| **Membership Interval** | A bounded period `[admitted, revoked?)` during which an identity was a workspace member. Intervals are contiguous; a member cannot have gaps |
| **Canonical Membership** | The signed, causally ordered, append-only membership operation DAG that is the sole authority for who is a workspace member. Built above Iroh's transport identity. Iroh connects peers; the membership DAG authorizes them |
| **Invitation** | A single-use, cryptographically signed token that authorizes one Installation to join a Workspace. It carries the inviter identity, Workspace identity, and expiry. An exact retry by the already admitted Installation may recover the same committed admission; recovery is not a second use |
| **Workspace Invitation** | The opaque, copied onboarding value that wraps a signed Invitation and the issuer's public Iroh endpoint ticket. It is sensitive bearer admission material |
| **Revocation** | A signed membership operation that ends a member's current interval. The revoked member retains past data but cannot read or write future content. Triggers key rotation in affected packages |
| **Epoch** | A workspace-local, monotonically increasing cursor for committed membership transitions. It orders local delivery but does not identify distributed membership state |
| **Membership Version** | A content-derived identifier for a workspace's accepted membership operation set. Peers with the same operation set have the same membership version |
| **Membership Operation** | A signed `admit` or `revoke` entry in the canonical membership DAG. Its content-derived ID authenticates the canonical bytes, and its dependency links record causal history |

## Package Architecture

| Term | Definition |
|---|---|
| **Semantic Capability** | A versioned interface a package declares and the runtime fulfills. Example: `conversations:v1`. Typed, secret-free, transport-handle-free. Implemented over Tauri's invoke/event bridge |
| **Package Backend** | The Rust module that owns a package's domain logic, storage, transport, and lifecycle. Runs in the main process with crate-level isolation from core internals |
| **Package UI** | The TypeScript web content a package renders into a Tauri webview. Communicates with its backend through typed semantic capability contracts |
| **Package Scope** | The instance boundary: `bundled` (shipped with the app), `team` (checked into the team's workspace), or `individual` (personal configuration) |
| **Package Manifest** | Declares the package identity, semantic capabilities it consumes, scope, entrypoints, and trust level |

## Conversations

| Term | Definition |
|---|---|
| **Channel** | A named, append-only message stream within a workspace. Has an owning creator and a lifecycle (active, archived) |
| **Message** | An attributed, authenticated, encrypted Markdown record appended immutably to a channel. Transported over Iroh QUIC streams between package instances |
| **Channel Membership** | Package-owned membership determining who can read and post in a specific channel. Bounded by canonical workspace membership |
| **Conversation Transport** | Iroh-based P2P delivery of messages between peers. The conversations package maintains an allow-list of `EndpointId`s derived from canonical workspace membership |

## Transport (Iroh)

| Term | Definition |
|---|---|
| **Endpoint** | An Iroh network node. Created from a `SecretKey`, bound to a local socket, connected to a home relay. The core runtime creates one endpoint per installation |
| **EndpointId** | The Ed25519 public key identifying an Iroh endpoint. Used as the peer identity for connections. Doubles as the installation's public identity |
| **Relay** | An Iroh relay server that forwards encrypted traffic between endpoints that cannot establish direct connections. Carries no content authority — traffic is end-to-end encrypted |
| **ALPN** | Application-Layer Protocol Negotiation — a protocol identifier used when establishing Iroh connections. Each package may register its own ALPN for package-specific streams |

## Repository and Files

| Term | Definition |
|---|---|
| **Repository Registration** | Binding a local Git clone to a workspace so packages can read it. Resonance does not replicate repository files |
| **Workspace Files** | Collaborative documents owned by the workspace-files package. Filesystem-first authority with signed operations, transported as Iroh blobs |