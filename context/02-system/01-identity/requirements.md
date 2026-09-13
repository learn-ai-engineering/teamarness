# 02-system/01-identity — Identity Requirements

Prefix: `TEAM.SYS.ID`

## TEAM.SYS.ID-R01 — Installation Identity

The system SHALL generate a permanent Ed25519 keypair on first launch. The `SecretKey` SHALL serve as the Iroh endpoint secret. The `PublicKey` SHALL serve as both the Iroh `EndpointId` and the installation's public identity. The private key SHALL never leave the runtime.

## TEAM.SYS.ID-R02 — Iroh Endpoint Binding

The system SHALL create an Iroh `Endpoint` from the installation's `SecretKey` on startup. The endpoint SHALL bind to the default Iroh relay configuration. The `EndpointId` SHALL be the installation's network identity for all P2P communication.

## TEAM.SYS.ID-R03 — Public Identity

The system SHALL expose the installation's public identity as a validated `PublicIdentity` value equivalent to the Iroh `EndpointId`. `PublicIdentity` SHALL support equality comparison, display formatting, and serialization. It SHALL NOT expose raw key bytes to packages.

## TEAM.SYS.ID-R04 — Domain-Separated Signing

The system SHALL provide domain-separated signing operations to packages. Each signing context SHALL include a domain tag to prevent cross-domain signature reuse. The signing interface SHALL return signatures without exposing the private key.

## TEAM.SYS.ID-R05 — Installation Display Name

The system SHALL support an Installation Display Name for the local Installation. It SHALL contain 1 through 64 Unicode alphanumeric characters, spaces, hyphens, underscores, apostrophes, or periods, with no surrounding whitespace. The user SHALL be able to change it. It SHALL NOT replace the Public Identity or become canonical membership authority.

## TEAM.SYS.ID-R06 — Device vs Human Identity

The membership log SHALL accommodate both device-scoped and future human-scoped identity. The initial implementation SHALL treat each device as a first-class member. The schema SHALL reserve space for a human identity layer that groups multiple device identities.