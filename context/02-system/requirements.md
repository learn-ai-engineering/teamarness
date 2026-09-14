# 02-system — System Requirements

Prefix: `TEAM.SYS`

## TEAM.SYS-R01 — Architecture Boundary

The system SHALL keep product-domain Package behavior separate from Core Runtime, Package Host, and Shell responsibilities. No product-domain behavior SHALL live in the Core Runtime, Package Host, or Shell. A Package SHALL use declared Semantic Capabilities rather than Core Runtime internals.

## TEAM.SYS-R02 — Technology Stack

The Core Runtime and Shell SHALL be written in Rust. The desktop Shell SHALL use Tauri v2. Phase 3 SHALL provide a Rust Package Backend SDK and Rust reference Package, but the Package Process Protocol SHALL be language-neutral. Package UIs SHALL use TypeScript. Iroh SHALL be the Core Runtime's P2P transport layer. The Phase 3 development stack SHALL require Rust through rustup and Node.js.

## TEAM.SYS-R03 — Local-First

All user data SHALL be stored locally. The system SHALL work without network connectivity. Data SHALL synchronize when peers become reachable via Iroh. No operation SHALL require a central server.

## TEAM.SYS-R04 — P2P Communication

Peers SHALL exchange data over Iroh's encrypted QUIC channels. Iroh relays MAY assist with NAT traversal but SHALL carry no content authority and SHALL NOT have access to plaintext content. The system SHALL use Iroh's default public relays initially, with custom relay configuration as an option.