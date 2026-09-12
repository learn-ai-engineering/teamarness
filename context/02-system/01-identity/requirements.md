# 02-system/01-identity — Identity Requirements

Prefix: `RES.SYS.ID`

## RES.SYS.ID-R01 — Installation Identity

The system SHALL generate a permanent Ed25519 keypair on first launch. The `SecretKey` SHALL serve as the Iroh endpoint secret. The `PublicKey` SHALL serve as both the Iroh `EndpointId` and the installation's public identity. The private key SHALL never leave the runtime.

## RES.SYS.ID-R02 — Iroh Endpoint Binding

The system SHALL create an Iroh `Endpoint` from the installation's `SecretKey` on startup. The endpoint SHALL bind to the default Iroh relay configuration. The `EndpointId` SHALL be the installation's network identity for all P2P communication.

## RES.SYS.ID-R03 — Public Identity

The system SHALL expose the installation's public identity as a validated `PublicIdentity` value equivalent to the Iroh `EndpointId`. `PublicIdentity` SHALL support equality comparison, display formatting, and serialization. It SHALL NOT expose raw key bytes to packages.

## RES.SYS.ID-R04 — Domain-Separated Signing

The system SHALL provide domain-separated signing operations to packages. Each signing context SHALL include a domain tag to prevent cross-domain signature reuse. The signing interface SHALL return signatures without exposing the private key.

## RES.SYS.ID-R05 — Identity Display

The system SHALL support a human-readable display name for the local installation. The display name SHALL be bounded in length and validated against a defined character set. It SHALL be changeable by the user.

## RES.SYS.ID-R06 — Device vs Human Identity

The membership log SHALL accommodate both device-scoped and future human-scoped identity. The initial implementation SHALL treat each device as a first-class member. The schema SHALL reserve space for a human identity layer that groups multiple device identities.