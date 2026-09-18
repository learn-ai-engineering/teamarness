# Teamarness — Requirements

Root requirements for the Teamarness product. Subsystem requirements live in `01-product/` and `02-system/`. Prefix: `TEAM`.

## TEAM-R01 — Installation Identity

Owned by `TEAM.SYS.ID-R01`. This ID is retained and not reused.

## TEAM-R02 — Workspace Creation

A user SHALL be able to create a named workspace. Creation SHALL produce an immutable workspace identity, a canonical membership log seeded with the creator, and a recoverable staged protocol that survives interruption.

## TEAM-R03 — Invitations

Owned by `TEAM.SYS.MEM-R03`. This ID is retained and not reused.

## TEAM-R04 — Canonical Membership

Owned by `TEAM.SYS.MEM-R01` and `TEAM.SYS.MEM-R04`. This ID is retained and not reused.

## TEAM-R05 — Revocation and Epochs

Revoking a member SHALL end their current membership interval. Every committed membership operation SHALL advance a workspace-local monotonic epoch and publish the resulting content-derived `MembershipVersion`. Membership changes SHALL replicate to every Member, and synchronization SHALL follow current membership. The revoked member SHALL retain past data. In version 1 the synchronization gate is the only protection for future content; Content Key rotation on revocation is roadmap Phase 5 work. See `TEAM.SYS.MEM-R05`, `TEAM.SYS.MEM-R06`, `TEAM.SYS.MEM-R10`, and `TEAM.SYS.TRANS-R06`.

## TEAM-R06 — Package Model

All product-domain capability SHALL live in Packages. Host-level Workspace controls SHALL remain in the Shell. A Package SHALL own its domain model, records, schemas, projections, policy, UI, and Package-specific membership.

Detailed by: `TEAM.SYS-R01`, `TEAM.SYS.PKG-R01`–`R11`.

## TEAM-R07 — Semantic Capabilities

Teamarness SHALL fulfill capabilities for Teamarness-owned authority and generic behavior through typed, secret-free, versioned Semantic Capabilities. The Shell SHALL NOT hard-code product-specific capability branches.

Detailed by: `TEAM.SYS.PKG-R03`, `TEAM.SYS.PKG-R10`, `TEAM.SYS.TRANS-R03`, `TEAM.SYS.SHELL-R05`.

## TEAM-R08 — Shell

The Tauri Shell SHALL own Package UI Mounts, Package navigation, Workspace controls, Workspace-level theming, and the desktop window lifecycle. It SHALL NOT own product-domain behavior.

Detailed by: `TEAM.SYS.SHELL-R01`–`R06`.

## TEAM-R09 — Conversations Package

Owned by `TEAM.SYS.CONV-R01`–`R16`. This ID is retained and not reused.

## TEAM-R10 — Local-First Operation

The system SHALL work offline. Locally authored content SHALL persist and synchronize when peers become reachable via Iroh. No operation SHALL require a central server.

Detailed by: `TEAM.SYS.TRANS-R01`.

## TEAM-R11 — P2P Transport

The system SHALL use Iroh for all peer-to-peer connectivity. All peer traffic SHALL be end-to-end encrypted, and relays SHALL carry no content authority.

Detailed by: `TEAM.SYS.TRANS-R01`–`R03`.

## TEAM-R12 — Git Integration

Owned by `TEAM.SYS.REPO-R07` and `TEAM.SYS.REPO-R10`. This ID is retained and not reused.

## TEAM-R13 — Technology Stack

Owned by `TEAM.SYS-R02`. This ID is retained and not reused.