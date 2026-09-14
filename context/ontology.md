# Teamarness — Ontology

Canonical terminology for the Teamarness domain. When a term appears in requirements, specs, code, UI, or conversation, it carries the meaning defined here. Divergence from this file in another surface is a bug.

## Top-level

| Term | Definition |
|---|---|
| **Teamarness** | The application: a local-first, P2P, package-extensible team workspace |
| **Installation** | One running instance of Teamarness on one device. Identified by an Ed25519 keypair that doubles as an Iroh `EndpointId` |
| **Workspace** | A named team context with canonical membership. Contains packages, conversations, and shared context. A single installation may belong to multiple workspaces |
| **Package** | A vertical module that owns one product domain, including its model, records, schemas, projections, policy, UI, and Package-specific membership. A Package consumes Semantic Capabilities for Teamarness-owned authority and generic services |
| **Core Runtime** | The application kernel that owns Installation identity, Workspace creation and invitations, Canonical Membership, Package lifecycle, and Teamarness implementations of Semantic Capabilities |
| **Shell** | The Tauri desktop host that owns Package navigation and UI mounting, Workspace controls, Workspace-level theming, and the desktop window lifecycle |

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
| **Package Host** | The Teamarness boundary that operates Package Instances and routes Package Process, data-directory, Semantic Capability, and UI contracts without product-specific branches |
| **Semantic Capability** | A versioned, typed interface through which a Package uses Teamarness-owned authority or generic behavior without receiving host secrets or resources |
| **Synchronization Capability** | A Package-scoped Semantic Capability that uses Core Runtime-owned replication and transport machinery for Package-defined records. The Package still owns its schemas, projections, and domain policy |
| **Package Identity** | The stable identifier of one Package across versions, Package Sources, distributions, and Package Instances |
| **Package Manifest** | The Package-owned declaration that binds a Package Identity to its compatibility requirements, entrypoints, and consumed Semantic Capabilities |
| **Package Backend** | The part of a Package that executes its domain behavior, policy, projections, and background work |
| **Package UI** | The part of a Package that presents its domain and communicates through the Package's declared contracts |
| **Package Instance** | One activation of a Package Identity for one Workspace on one Installation. Its Backend lifecycle does not depend on whether its UI is visible |
| **Package Process** | One supervised operating-system process running a Package Backend for one Package Instance |
| **Package Process Protocol** | The versioned, language-neutral executable contract between the Package Host and a Package Process |
| **Package UI Mount** | The Shell-owned attachment of a Package UI to the visible Package pane for one Package Instance. Mounting and visibility do not define Backend lifetime |
| **Package Data Directory** | The durable local directory assigned to one Package Identity, Workspace, and Installation. The Package owns its contents and local representation |
| **Package Artifact** | A built Package file selected for a specific target and entrypoint |
| **Package Directory** | A directory containing one Package Manifest and the Package Artifacts it identifies. A configured local Package path identifies this directory |
| **Package Source** | The concrete Package Directory from which an Installation discovers and validates a Package. A Package Source is `bundled` or `local` in Phase 3 |
| **Package Distribution** | The means by which a Package Directory reaches an Installation. Distribution does not determine Package Identity, Package Trust, or runtime behavior |
| **Package Registration** | A binding that makes one Package Source available to Teamarness under a stated Package Registration Scope |
| **Package Registration Scope** | The authority boundary for a Package Registration: `application`, `installation`, or `workspace`. Phase 3 uses application registrations for bundled sources and installation registrations for configured local sources |
| **Package Admission** | The Teamarness decision to accept a validated Package Source for execution as a Package Identity |
| **Package Trust** | The authority Teamarness grants during Package Admission. A Package Manifest cannot grant trust to its own Package |

## Conversations

| Term | Definition |
|---|---|
| **Channel** | A named, append-only message stream within a workspace. Has an owning creator and a lifecycle (active, archived) |
| **Message** | An attributed, authenticated, encrypted Markdown record appended immutably to a channel. Transported over Iroh QUIC streams between package instances |
| **Channel Membership** | Package-owned membership determining who can read and post in a specific channel. Bounded by canonical workspace membership |
| **Conversation Synchronization** | Replication of Conversation records between peers. The Conversations Package owns the records and domain policy; a generic Synchronization Capability owns the reusable Iroh transport and replication machinery |

## Transport (Iroh)

| Term | Definition |
|---|---|
| **Endpoint** | An Iroh network node. Created from a `SecretKey`, bound to a local socket, connected to a home relay. The core runtime creates one endpoint per installation |
| **EndpointId** | The Ed25519 public key identifying an Iroh endpoint. Used as the peer identity for connections. Doubles as the installation's public identity |
| **Relay** | An Iroh relay server that forwards encrypted traffic between endpoints that cannot establish direct connections. Carries no content authority — traffic is end-to-end encrypted |
| **ALPN** | Application-Layer Protocol Negotiation, a protocol identifier used when establishing Iroh connections |

## Repository and Files

| Term | Definition |
|---|---|
| **Repository Registration** | Binding a local Git clone to a workspace so packages can read it. Teamarness does not replicate repository files |
| **Workspace Files** | Collaborative documents owned by the workspace-files package. Filesystem-first authority with signed operations, transported as Iroh blobs |