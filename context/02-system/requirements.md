# 02-system — System Requirements

Prefix: `RES.SYS`

## RES.SYS-R01 — Architecture Boundary

The system SHALL separate the core runtime (identity, membership, package lifecycle) from product-domain packages (conversations, documents, repos, AI tooling). No product-domain behavior SHALL live in the core runtime. No package SHALL import core-internal modules directly.

## RES.SYS-R02 — Technology Stack

The core runtime, shell, and package backends SHALL be written in Rust. The desktop shell SHALL use Tauri v2. Package UIs SHALL use TypeScript. Iroh SHALL be the P2P transport layer. The stack SHALL require Rust (via rustup) and Node.js for development.

## RES.SYS-R03 — Local-First

All user data SHALL be stored locally. The system SHALL work without network connectivity. Data SHALL synchronize when peers become reachable via Iroh. No operation SHALL require a central server.

## RES.SYS-R04 — P2P Communication

Peers SHALL exchange data over Iroh's encrypted QUIC channels. Iroh relays MAY assist with NAT traversal but SHALL carry no content authority and SHALL NOT have access to plaintext content. The system SHALL use Iroh's default public relays initially, with custom relay configuration as an option.