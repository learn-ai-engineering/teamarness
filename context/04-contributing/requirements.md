# 04-contributing — Contributing

Prefix: `TEAM.CONTRIB`

## TEAM.CONTRIB-R01 — Development Setup

A contributor SHALL be able to clone, install dependencies, and run the application. The development environment SHALL require Rust (installed via rustup) and Node.js. Setup SHALL be documented in a top-level README.

## TEAM.CONTRIB-R02 — Code Standards

All Rust code SHALL pass `cargo clippy` and `cargo test` before merge. All TypeScript code SHALL pass linting and type-checking. Commit messages SHALL follow Conventional Commits. Architecture decisions SHALL be recorded in `context/.decisions/`.

## TEAM.CONTRIB-R03 — Architecture Discipline

Changes that affect the core runtime boundary, package contracts, or membership model SHALL update the relevant VRS node (requirements, spec, or decision) in the same change. The `context/` tree SHALL remain the canonical intent for the product.