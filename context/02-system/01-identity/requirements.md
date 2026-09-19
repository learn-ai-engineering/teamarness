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

The membership log SHALL accommodate both device-scoped and future human-scoped identity. The initial implementation SHALL treat each device as a first-class member. The schema SHALL reserve space for a human identity layer that groups multiple device identities.

## TEAM.SYS.ID-R07 — Private Data Directory

The Core Runtime SHALL refuse a data directory that is a symbolic link, is owned by another user, or contains unrelated files. It SHALL restrict the directory to its owner and SHALL fail closed on platforms where it cannot. It SHALL keep one stored copy of each Content Key and SHALL clear key buffers it owns after use on a best-effort basis.

## TEAM.SYS.ID-R08 — Recipient-Bound Protection Grants

A Protection Grant SHALL be decryptable only with the recipient Installation's private key. It SHALL bind its kind, scope, Workspace, author, recipient, admission, and Content Key identifier, and its author SHALL sign it through the domain-separated signing interface.
