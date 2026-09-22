# 02-system — System Requirements

Prefix: `TEAM.SYS`

## TEAM.SYS-R01 — Architecture Boundary

The system SHALL keep product-domain Package behavior separate from Core Runtime, Package Host, and Shell responsibilities. No product-domain behavior SHALL live in the Core Runtime, Package Host, or Shell. A Package SHALL use declared Semantic Capabilities rather than Core Runtime internals.

## TEAM.SYS-R02 — Technology Stack

The Core Runtime and Shell SHALL be written in Rust. The desktop Shell SHALL use Tauri v2. Teamarness SHALL provide a Rust Package Backend SDK; the Package Process Protocol SHALL be language-neutral. Package UIs SHALL use TypeScript. Iroh SHALL be the Core Runtime's P2P transport layer.

## TEAM.SYS-R03 — Local-First

Owned by `TEAM-R10`. This ID is retained and not reused.

## TEAM.SYS-R04 — P2P Communication

Owned by `TEAM-R11` and `TEAM.SYS.TRANS-R02`. This ID is retained and not reused.