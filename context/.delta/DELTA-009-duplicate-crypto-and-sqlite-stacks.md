# DELTA-009 — The dependency graph carries two Ed25519 stacks and two SQLite drivers

**Status:** open (2026-09-19)

## Divergence

Teamarness pins one version of every dependency it names, but two of its upstream dependencies bring their own copies of the same primitives, so the graph holds each twice.

**Ed25519.** Iroh 1.2 builds on `ed25519-dalek` 3, `curve25519-dalek` 5, `sha2` 0.11 and `signature` 3. p2panda 0.7.1 builds on `ed25519-dalek` 2, `curve25519-dalek` 4, `sha2` 0.10 and `signature` 2. `PublicIdentity` is an iroh key; operation authorship is a p2panda key. A key therefore crosses between the stacks as its 32 compressed Edwards bytes, and a signature as its 64 bytes.

**SQLite.** `rusqlite` 0.32.1 is Core's storage driver. `p2panda-store` brings `sqlx` 0.8.6, whose `sqlx-sqlite` links the same `libsqlite3-sys` 0.30.1. `libsqlite3-sys` declares `links = "sqlite3"`, so Cargo permits exactly one version of it in the graph: `rusqlite` cannot move off 0.32 until `p2panda-store` moves to a `sqlx` that links a newer `libsqlite3-sys`.

**Tauri build-time duplicates.** `tauri-build`, `tauri-codegen` and `tauri-utils` pull older `thiserror`, `toml`, `winnow`, `png`, `bitflags`, `indexmap` and `miniz_oxide`. These are build-time only and do not reach a shipped binary.

None of the duplicates is removable by a first-party pin. Every first-party pin already names the newest version present in the graph, so lowering one would add a version rather than remove one.

## Impact

Two Ed25519 implementations mean two sets of parsing and verification rules. If they ever diverge — a key one accepts and the other refuses, or a signature one verifies and the other does not — bytes could cross a boundary and mean something different on the other side.

`crates/teamarness-core/src/synchronization/key_bridge.rs` is the only crossing. `p2panda_verifying_key` and `identity` are the two directions, and a key crosses only when both stacks accept its bytes; a divergence therefore refuses the key rather than trusting one guard. Its tests pin the three properties the crossing rests on: the 32-byte encoding round trips for random keys, a signature made under either stack verifies under the other, and the two `from_bytes` guards accept and reject the same edge encodings — the identity, the order-2 and order-4 points, canonical order-8 points, a non-canonical `y`, and all-ones.

The duplicate SQLite driver costs build time and binary size. Both drivers open Core's database file, but only `rusqlite` writes it.

## Resolution plan

Drop each duplicate when upstream removes the need for it:

- **Ed25519:** a p2panda release built on `ed25519-dalek` 3. Then p2panda and iroh share one stack, and `key_bridge` becomes a type conversion rather than a byte crossing. Keep its tests either way.
- **SQLite:** a `p2panda-store` release on `sqlx` ≥ 0.9, after which `rusqlite` can move past 0.32.1.
- **Tauri build-time:** a Tauri release that refreshes its build dependencies. Tracked by TEAM-DQ6, which pins Tauri 2.11.

There is no `cargo tree -d` ratchet. It would add a slow cargo invocation to `npm test` and none of the duplicates is actionable here.

## Tracking

- `crates/teamarness-core/src/synchronization/key_bridge.rs` and its tests
- TEAM-DQ6 (the Tauri 2.11 pin)
- The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 8 — structure and maintainability
