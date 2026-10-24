# 02-system/01-identity — Identity Requirements

Prefix: `TEAM.SYS.ID`

## TEAM.SYS.ID-R01 — Installation Identity

The system SHALL generate a permanent Ed25519 keypair on first launch. The `SecretKey` SHALL serve as the Iroh endpoint secret. The `PublicKey` SHALL serve as both the Iroh `EndpointId` and the installation's public identity. The private key SHALL NOT be exposed through any Teamarness API, Semantic Capability, log, or diagnostic. Version 1 stores it unencrypted in the Core Runtime's private data directory, where processes of the same operating-system user, including trusted Package Backends, can read it (ADR 0017).

## TEAM.SYS.ID-R02 — Iroh Endpoint Binding

The system SHALL create an Iroh `Endpoint` from the installation's `SecretKey` on startup. The endpoint SHALL bind to the default Iroh relay configuration. The `EndpointId` SHALL be the installation's network identity for all P2P communication.

## TEAM.SYS.ID-R03 — Public Identity

The system SHALL expose the installation's public identity as a validated `PublicIdentity` value equivalent to the Iroh `EndpointId`. `PublicIdentity` SHALL support equality comparison, display formatting, and serialization. It SHALL NOT expose raw key bytes to packages.

## TEAM.SYS.ID-R04 — Domain-Separated Signing

The system SHALL provide domain-separated signing operations to packages. Each signing context SHALL include a domain tag to prevent cross-domain signature reuse. The signing interface SHALL return signatures without exposing the private key. Every Teamarness-defined signature SHALL use this interface. Uses of the Installation key outside it (Iroh TLS, P2Panda operation headers, and X25519 grant decryption) are limited to those recorded in ADR 0017.

## TEAM.SYS.ID-R05 — Installation Display Name

The system SHALL support an Installation Display Name for the local Installation. It SHALL contain 1 through 64 Unicode alphanumeric characters, spaces, hyphens, underscores, apostrophes, or periods, with no surrounding whitespace. The user SHALL be able to change it. It SHALL NOT replace the Public Identity or become canonical membership authority.

## TEAM.SYS.ID-R06 — Device vs Human Identity

The membership log SHALL accommodate both device-scoped and human-scoped identity. A Workspace Person SHALL be a key group nested in the Workspace group, managed by the Person's Installation key and holding the Person's Agent Identities (ADR 0023). The initial implementation SHALL attach one Installation key to each Person; grouping multiple Installations under one Person SHALL reuse the same key group without changing the Person's Principal ID.

## TEAM.SYS.ID-R09 — Principal Names

Each current Workspace Principal SHALL have a Workspace-local opaque Principal ID, distinct from any key: a Workspace Person's ID SHALL be derived from its accepted key group create operation, and a Workspace Agent's ID from the accepted operation that first added its key, under separate domain tags. A Person's name edit SHALL be signed by that Person's Installation key; an Agent's name edit SHALL be signed by its Steward's Installation key, and no other key SHALL rename a Principal. Names SHALL be non-unique Unicode text (including emoji), nonblank after trimming, without control characters, and at most 64 grapheme clusters and 256 UTF-8 bytes. Missing names SHALL present a fingerprint fallback, not the Installation Display Name. A name change SHALL NOT grant membership authority or advance the membership epoch or version. Current Principals, their names, and a separate durable profile cursor SHALL remain available after restart and peer synchronization without Package activation (ADR 0023).

## TEAM.SYS.ID-R10 — Agent Identity

When a Workspace Person adds a Workspace Agent, the Steward's Installation SHALL generate the Agent's Ed25519 keypair and SHALL custody its secret under the same rules as the Installation secret (`TEAM.SYS.ID-R01`, ADR 0017). The Agent secret SHALL NOT be exposed through any Teamarness API, Semantic Capability, log, diagnostic, Workspace Invitation, or user interface, and SHALL be cleared from memory after use on a best-effort basis. The Agent Identity SHALL sign records only: it SHALL NOT author membership operations or name edits, SHALL NOT open or accept peer connections, and SHALL NOT receive Protection Grants. Core SHALL sign a record with an Agent Identity for any Package Instance on the same Installation only while the Agent is a current Workspace Agent, its Steward's key group contains that Installation's key, and at least one Package Instance on that Installation holds an Agent Binding to it; the calling Package's identity SHALL NOT be part of the rule (ADR 0027).

## TEAM.SYS.ID-R07 — Private Data Directory

The Core Runtime SHALL refuse a data directory that is a symbolic link, is owned by another user, or contains unrelated files. It SHALL restrict the directory to its owner and SHALL fail closed on platforms where it cannot. It SHALL keep one stored copy of each Content Key and SHALL clear key buffers it owns after use on a best-effort basis.

## TEAM.SYS.ID-R08 — Recipient-Bound Protection Grants

A Protection Grant SHALL be decryptable only with the recipient Installation's private key. It SHALL bind its kind, scope, Workspace, author, recipient, admission, and Content Key identifier, and its author SHALL sign it through the domain-separated signing interface.
