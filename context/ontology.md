# Teamarness — Ontology

Canonical terminology for the Teamarness domain. When a term appears in requirements, specs, code, UI, or conversation, it carries the meaning defined here. Divergence from this file in another surface is a bug.

## Top-level

| Term | Definition |
|---|---|
| **Teamarness** | The application: a local-first, P2P, package-extensible team workspace |
| **Installation** | One running instance of Teamarness on one device. Identified by an Ed25519 keypair that doubles as an Iroh `EndpointId` |
| **Workspace** | A named team context with canonical membership. Contains packages, conversations, and shared context. A single installation may belong to multiple workspaces |
| **Package** | A vertical module that owns one product domain, including its model, records, schemas, projections, policy, UI, and Package-specific membership. A Package consumes Semantic Capabilities for Teamarness-owned authority and generic services |
| **Core Runtime** | The application kernel that owns Installation identity, Workspace creation and invitations, Canonical Membership, Package lifecycle, and the authority and state behind Semantic Capabilities |
| **Shell** | The Tauri desktop host that owns Package navigation and UI mounting in Package UI Frames, Workspace controls, Workspace-level theming, and the desktop window lifecycle |

## Identity and Membership

| Term | Definition |
|---|---|
| **Installation Identity** | A permanent Ed25519 keypair generated on first launch. The `SecretKey` is the Iroh endpoint secret; the `PublicKey` is the Iroh `EndpointId`. This is the device's network identity. It is also converted to an X25519 key to open Protection Grants (ADR 0013) |
| **Installation Display Name** | User-changeable, validated human-readable metadata for the local Installation. It does not replace the Public Identity and is not currently shared through canonical membership |
| **Public Identity** | The public half of an installation identity (`EndpointId`), shared with workspaces as a member identifier |
| **Member** | A signing key currently admitted to a Workspace, either directly (the Workspace creator's Installation key) or through a Workspace Person's key group. Membership is canonical and interval-based. Every Member key belongs to exactly one Workspace Principal; a Workspace Agent's key is a Member with role `Member` |
| **Workspace Principal** | A participant recognized within one Workspace: a Workspace Person or a Workspace Agent. It has a Workspace-local opaque Principal ID and exactly one current signing key. Attribution of any record, Membership Operation or name edit is the signer key resolved to its Principal; nothing carries an acting-Principal claim (ADR 0023) |
| **Workspace Person** | A Workspace Principal that is a person, with a distinct identity in that Workspace. The same human in another Workspace has a separate Workspace Person. In Canonical Membership a Workspace Person is a key group nested in the Workspace group: the Person's Installation key creates and manages the group, a Manager admits or revokes the group, and the group holds the keys of the Person's Workspace Agents. Its Principal ID is derived from the group create operation. A Workspace Person is not, by itself, membership authority |
| **Workspace Agent** | A Workspace Principal that is not a person, for example an AI agent run by a Package. It has exactly one Steward, whose key group holds the Agent's key. Its Principal ID is derived from the Membership Operation that first added its key. An Agent signs records only; it never authors a Membership Operation or a name edit. Removing a Workspace Person removes its Agents |
| **Agent Identity** | The Ed25519 keypair that signs a Workspace Agent's records. The Steward's Installation generates and custodies it beside the Installation Identity under the same rules (ADR 0017) and never exports, logs or displays it. It is not an Iroh endpoint: an Agent Identity never connects as a peer and never receives a Protection Grant. Core signs with it for any Package Instance on that Installation, but only while the Agent is current, its Steward's key group contains that Installation's key, and the Agent has at least one Agent Binding there (ADR 0030) |
| **Steward** | The Workspace Person responsible for a Workspace Agent. The Steward adds, renames and removes the Agent with its own authority; a Manager can also remove an Agent. A Workspace Agent has exactly one Steward |
| **Principal Name** | A changeable, Workspace-specific name of one Workspace Principal for display to Members and Packages. A Workspace Person sets its own name (its Workspace Person Name); a Steward sets its Agents' names. It need not be unique and does not confer membership or authorization; Conversations resolves the current name for an immutable Message author key through `teamarness.principals`, falling back to an inspectable identity when unavailable |
| **Manager** | A Member whose role authorizes admitting and revoking Workspace Persons, removing Workspace Agents, and issuing Invitations. In version 1 the Workspace creator is the only Manager (`MembershipRole::Manager`). Manager assignment and transfer are roadmap Phase 5 work |
| **Membership Interval** | A bounded period `[admitted, revoked?)` during which an identity was a workspace member. Intervals are contiguous; a member cannot have gaps |
| **Canonical Membership** | The signed, causally ordered, append-only membership operation DAG that is the sole authority for who is a workspace member. Built above Iroh's transport identity. Iroh connects peers; the membership DAG authorizes them |
| **Invitation** | A single-use, cryptographically signed token that authorizes one Installation to join a Workspace as a new Workspace Person. It carries the inviter identity, Workspace identity, and expiry. The joiner's claim carries its signed key group create, so the issuer commits the group create and the group admission together; a claim never carries an Agent Identity. An exact retry by the already admitted Installation may recover the same committed admission; recovery is not a second use |
| **Workspace Invitation** | The opaque, copied onboarding value that wraps a signed Invitation and the issuer's public Iroh endpoint ticket. It is sensitive bearer admission material |
| **Revocation** | A signed membership operation that ends a Member's current interval. It replicates to every Member; once an Installation accepts it, that Installation stops synchronizing with the revoked Installation. The revoked Installation keeps data and Content Keys it already received. Version 1 does not rotate Content Keys on revocation; rotation is roadmap Phase 5 work |
| **Epoch** | A workspace-local, monotonically increasing cursor for committed membership transitions, including transitions received through Membership Replication. It orders local delivery but does not identify distributed membership state and is not a Content Key Generation |
| **Membership Version** | A content-derived identifier for a workspace's accepted membership operation set. Peers with the same operation set have the same membership version |
| **Membership Operation** | A signed entry in the canonical membership DAG: `create person`, `admit` or `revoke` (a Person's key group), or `add agent key` or `remove agent key` (inside a Person's key group). Its content-derived ID authenticates the canonical bytes, and its dependency links record causal history |
| **Membership Replication** | Core-owned exchange of accepted Membership Operations between Installations of one Workspace, independent of Package records. It is idempotent, causal, and resumes after disconnection |

## Package Architecture

| Term | Definition |
|---|---|
| **Package Host** | The Teamarness boundary that operates Package Instances and routes Package Process, data-directory, Semantic Capability, and UI contracts without product-specific branches. It owns each Semantic Capability's wire contract, limits, and version negotiation |
| **Semantic Capability** | A versioned, typed interface through which a Package uses Teamarness-owned authority or generic behavior without receiving host secrets or resources |
| **Semantic Capability Family** | The stable name of one Semantic Capability contract shape, such as the immutable record log or `teamarness.principals`. A family evolves by version; a different shape is a new family, declared and negotiated beside the existing ones |
| **Synchronization Capability** | A Package-scoped Semantic Capability that uses Core Runtime-owned replication and transport machinery for Package-defined records. The Package still owns its schemas, projections, and domain policy |
| **Synchronization Projection** | Core's private authenticated P2Panda operation view of a Teamarness semantic record. It supplies bounded LogSync ranges without replacing `RecordId`, `OrderKey`, or the Installation-local `Cursor` |
| **Durable Wake** | A coalesced Core persistence marker that records synchronization work needing a later bounded session, including after restart |
| **Package Identity** | The stable identifier of one Package across versions, Package Sources, distributions, and Package Instances |
| **Package Manifest** | The Package-owned declaration that binds a Package Identity to its compatibility requirements, entrypoints, and consumed Semantic Capabilities |
| **Package Backend** | The part of a Package that executes its domain behavior, policy, projections, and background work |
| **Package UI** | The part of a Package that presents its domain and communicates through the Package's declared contracts |
| **Package Instance** | One activation of a Package Identity for one Workspace on one Installation. Its Backend lifecycle does not depend on whether its UI is visible |
| **Package Process** | One supervised operating-system process running a Package Backend for one Package Instance |
| **Package Process Protocol** | The versioned, language-neutral executable contract between the Package Host and a Package Process |
| **Package UI Mount** | The Shell-owned attachment of a Package UI to the visible Package pane for one Package Instance. Mounting and visibility do not define Backend lifetime |
| **Package UI Frame** | The sandboxed frame, with an opaque origin, in which the Shell runs one Package UI Mount's code, separate from the Shell's JavaScript realm |
| **Package UI Bridge** | The mount-scoped message channel between a Package UI Frame and the Shell. The Shell derives mount identity from the channel, never from message content |
| **Frame Runtime** | The Shell-owned script inside a Package UI Frame that builds the mount context and client and calls the Package UI's `mount` |
| **Package Data Directory** | The durable local directory assigned to one Package Identity, Workspace, and Installation. The Package owns its contents and local representation |
| **Package Artifact** | A built Package file selected for a specific target and entrypoint |
| **Package Directory** | A directory containing one Package Manifest and the Package Artifacts it identifies. A configured local Package path identifies this directory |
| **Package Source** | The concrete Package Directory from which an Installation discovers and validates a Package. A Package Source is `bundled` or `local` in Phase 3 |
| **Package Distribution** | The means by which a Package Directory reaches an Installation. Distribution does not determine Package Identity, Package Trust, or runtime behavior |
| **Package Registration** | A binding that makes one Package Source available to Teamarness under a stated Package Registration Scope |
| **Package Registration Scope** | The authority boundary for a Package Registration: `application`, `installation`, or `workspace`. Application registrations hold bundled sources. Installation registrations hold local sources; the Shell writes them, or a person writes the file by hand. Workspace scope is not implemented |
| **Team tier** | The Package Admission tier that holds every application registration and every installation registration that overrides one (`overrideApplication`). It is admitted at Host open and does not change while the Host runs. Its Packages are Team Packages. A Team Package never routes to a Personal Provider (ADR 0036) |
| **Personal tier** | The Package Admission tier that holds every installation registration without an override. It is admitted after the Team tier, and a person installs, reloads, and removes its Packages while the Host runs. Its Packages are Personal Packages. The tier is a lifecycle boundary, not a trust verdict (ADR 0036) |
| **Package Admission** | The Teamarness decision to accept a validated Package Source for execution as a Package Identity |
| **Package Trust** | The authority Teamarness grants during Package Admission. A Package Manifest cannot grant trust to its own Package |
| **Package Catalog** | The Package Host's list of admitted Packages, each with Package Identity, version, display name, optional description, author, and homepage, declared Package UI Views, Package Source kind, Package Trust kind, and UI contract digest. It excludes filesystem paths and loader URLs |
| **Host Revision** | A monotonic number identifying one authoritative Package Host snapshot. Consumers use it to discard stale updates |
| **Package Instance Lifecycle State** | One of `admitted`, `activating`, `active`, `suspending`, `suspended`, `recovering`, `failed`, `crash_loop`, `disposing`, `disposed`. `crash_loop` means automatic restarts were exhausted; `failed` needs explicit recovery or disposal. Shell UI wording may differ; see the Shell |
| **Package-Owned Error** | A Package Backend's stable error code, retryability, and bounded details, carried to the Package UI beside the Teamarness error code |
| **Package Validator** | The author-runnable command that applies Package Admission's Package Manifest and UI contract rules to a Package project or Package Directory |
| **Capability Grant** | An opaque, per-process-generation token that binds one Package Instance's declared Semantic Capability to its Package Identity, Workspace, and Installation. Grants are implicit in capability calls and are not transferable |
| **Agent Binding** | Installation-local Package configuration, held by Core and never replicated, that maps one Package Instance in one Workspace to at most one Workspace Agent; any number of Packages may bind the same Agent. Only the Steward's Installation binds; a Package reads, sets and clears its own binding through `teamarness.agent-binding`. A binding is cleared when this Installation removes the Agent and is void once the Agent is no longer current (ADR 0030) |
| **Agent Package** | A generic Package that is the harness for one Workspace Agent per Package Instance: the Steward configures it in the Package's own UI, it holds its Agent through an Agent Binding, and it participates in other Packages only through their Ports. The Shell has no Agent-specific controls |
| **Package Source Label** | A presentation alias shown by the Shell: `Team` for Package Source `bundled`, `Personal` for `local`. It is not a trust verdict and does not define Registration Scope, Workspace enablement, or sharing. It follows Package Source, not the admission tier: an override is `local` but belongs to the Team tier, and its Package page names it as a Team Package replaced by an override. The sidebar groups **Team Packages** and **Personal Packages** by tier, so an override is listed under **Team Packages** |
| **Package page** | The Shell screen that describes one Package: name, Package Identity, version, optional metadata, tier, Package Trust, declared Semantic Capabilities, and Ports. It exists for every admitted Package and every refused installation registration. A Personal Package's page also has Reload, Restart, Pause, Resume, Start, and Remove |
| **Package UI View** | A named presentation that one Package UI mounts: `main`, which every Package UI has, or `settings`, which a Package declares in the manifest's `ui.views`. The mount context carries the view. The Package page links to a declared settings view |
| **Port Contract** | A versioned, digest-verified document independent of Package Identity. It names exact commands, queries, and events with closed schemas; the Host admits and grants declared Consumers against one Provider per contract |
| **Port Observation** | An optional, bounded, payload-free, process-local diagnostic joining a Consumer's `p:` route to a Provider's `h:` route. Multiple Consumer streams can share one Provider event stream; missing or lagged observations do not determine domain outcomes |
| **Content View** | A Host-served, read-only set of files from one Package Data Directory, opened through `teamarness.content-views` and loaded in a nested frame at `teamarness-content://localhost/<token>/<entry>`. The Host verifies each listed file's size and SHA-256 and assigns its media type from a shared allowlist. The Shell serves it with an opaque origin and scripts only. A view ends with its process generation, and a changed file needs a new view (ADR 0034) |

## Conversations

| Term | Definition |
|---|---|
| **Channel** | A named, append-only message stream within a workspace. Has an owning creator and a lifecycle (active, permanently hidden); synthetic `#general` is always present and cannot be hidden |
| **Message** | An attributed, authenticated, encrypted Markdown record appended immutably to a Channel through the generic immutable-record capability. Transport details remain Core-owned |
| **Channel Membership** | Package-owned policy determining who can read and post in a specific Channel, bounded by Canonical Membership. Phase 4 has no separate Channel Membership records because every current Workspace Member participates in every Channel |
| **Conversation Synchronization** | Replication of Conversation records between peers. The Conversations Package owns records and domain policy; `teamarness.synchronization.immutable-record-log` owns reusable transport, replication, retry, and backpressure |
| **Message Port invalidation** | The live, coalescible, payload-free `messages.changed` event. It signals a local projection change, not one Message or a replayable history item. Consumers query `message.page` on receipt and after reconnecting |

## Documents

| Term | Definition |
|---|---|
| **Collection** | A flat, named group of Documents within a Workspace, identified by the Record ID of the record that created it. It can be renamed, archived, and restored; it is never deleted |
| **Document** | A collaboratively edited document that belongs to exactly one Collection, identified by the Record ID of its create record. Its title, Collection, and archive state are metadata records. Its body is a structured Loro tree of the shared body schema, edited in rendered form and exchanged as Canonical Markdown. The body is rebuilt from its Body Genesis and Document Edit records and never persisted as plaintext |
| **Body Genesis** | The first update of a Document body, carried in the `document.create` record: one update from a fresh peer that writes the root map, its attributes, the `children` list, and one empty paragraph. Every Installation imports it before any Document Edit, so no editor initialises an empty body |
| **Document Edit** | An identified (kind-11) immutable record carrying one Loro update of at most 160 KiB for one Document's body, with an editor request ID and payload digest. Edits converge in any arrival order; a Document is editable once every locally known Document Edit has been opened |
| **Editor Session** | One open editing view of a Document body (`documents.document.session`). The Backend gives it a freshly allocated Loro peer, loads it from a body snapshot, relays other writers' updates to it, and accepts its updates through `documents.document.update`. An Editor Session that opened with the snapshot of an editable body, or whose resume as its own peer the Backend accepted, holds a doc: any other reset the Backend starts sends it a fresh opening as the same peer, which it merges into the doc it kept, and only an update refusal that retrying cannot cure, or an identity conflict, ends it with a resync that discards its local state, reporting what it could not keep. Closing it reports what is still unacknowledged. A replaced Backend generation, or a Backend that already ended it, lets it resume as the same peer (`resumePeer`) when no open session holds that peer, no other Installation's record carries it, and this process either ended that session or never handed the peer out; every other peer is fresh. Two views of one Document are two Editor Sessions |
| **Canonical Markdown** | The Markdown export of a Document body as Markdown can carry it, with formatting Markdown cannot express dropped before writing, so exporting again gives the same bytes. The Documents Backend is its only producer (`markdown::export`); the normalisations are the Markdown contract |
| **Version Token** | The canonical encoding of a Document body's Loro frontiers in the Documents Port Contract: the frontier IDs sorted by peer then counter, 12 bytes each, in base64url without padding, at most 64 IDs. `document.markdown` returns it and `document.revise` requires it as `baseVersion` |
| **Comment** | An immutable Markdown remark anchored to one Document, optionally quoting an excerpt and optionally replying to a parent Comment on the same Document. Comments can be resolved and reopened and are never deleted |
| **Archive** | Hiding a Collection or Document from the default listing by a record; archived entries remain openable and can be restored. Documents are never purged |

## Exhibits

| Term | Definition |
|---|---|
| **Exhibit** | A named, multi-page, interactive web experience (HTML, CSS, JavaScript, images, media, and WebAssembly, all bundled) that an Agent or a Person authors through the Exhibits Package's Ports and a Person views inside Teamarness in a Content View. Its Exhibit ID has at most 48 characters: runs of lowercase letters and digits separated by single hyphens. Its title is a separate display string of at most 120 characters |
| **Draft** | An Exhibit's revisable, Installation-local state: a title, an entry page and a complete file list, stored in the Exhibits Package Data Directory and never replicated. Each update replaces the whole Draft atomically and increments its revision. A Person and a bound Workspace Agent can both update a Draft |
| **Snapshot** | An immutable, shared copy of an Exhibit that a Person takes from a Draft so other Members can view it and continue from it. Only a Person takes a Snapshot, never an Agent. The term is defined now; Exhibits slice 2 delivers it, and slice 1 has Drafts only |

## Synchronization and Protection

| Term | Definition |
|---|---|
| **Immutable Record** | A Package-defined canonical byte value appended once to a Package-and-Workspace record log through `teamarness.synchronization.immutable-record-log`, authenticated by its author, and never changed |
| **Record ID** | The authenticated content-derived identifier of an Immutable Record (`RecordId`) |
| **Order Key** | Core's versioned, convergent sort position for an Immutable Record (`OrderKey`). Every Installation sorts accepted records the same way. It is not wall-clock time |
| **Cursor** | An Installation-local delivery position in a record subscription (`Cursor`). It is progress, not an order offset, and differs between Installations |
| **Sealed Record** | The Teamarness-owned versioned binary envelope that carries an encrypted Immutable Record. Packages store and pass sealed records but open them only through the capability |
| **Content Key** | The symmetric key that seals one Package-and-Workspace record log. Core holds it and never exposes it to Packages. Version 1 has one Content Key per scope and no rotation |
| **Protection Grant** | Core-to-Core material that gives one current Member the Content Key for a scope. It is sealed to its recipient Installation's key, so only that Installation can open it, and it is signed by its author through the domain-separated signing interface. It carries the Content Key Origin and is distinct from a Capability Grant |
| **Content Key Generation** | The period during which one Content Key seals new records in a Package scope. Version 1 has exactly one generation per scope, started when the Workspace creator's locally activated Package first opens its immutable-record subscription (or on the creator's first append if it has not subscribed). Roadmap Phase 5 starts a new generation on revocation so a revoked Member cannot open records sealed afterwards |
| **Content Key Origin** | The Workspace creator's signed record naming a scope's first Content Key. A Protection Grant carries it, and an Installation accepts a first Content Key only with it |
| **Parked Operation** | A received, authenticated synchronization operation that Core cannot apply yet because its author's admission or its scope's Content Key is not yet known locally. It is kept within fixed bounds and applied when that state arrives |

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
| **Repository Registration** | A Package-owned record binding a local Git working tree, by canonical path, to one Installation and Workspace so that Package can read it. Registrations and repository content are not replicated |
