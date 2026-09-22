# 0017 — Version 1 key custody and recipient-bound protection grants

**Status:** accepted (2026-09-19)
**Refines:** ADR 0008 (the custody wording of the protection boundary), ADR 0009 (Core retains raw content-key custody)

## Context

The Core Runtime holds two kinds of secret: the Installation's Ed25519 identity key, and one Content Key per Package scope. Both live in `core.sqlite3` inside the Core Runtime's private data directory.

ADR 0007 makes Package Backends trusted native processes that keep the current user's ambient filesystem and network authority. Process separation there is a contract boundary, not a sandbox. A process running as the same operating-system user reaches plaintext content by supported means whatever the keys look like on disk: it can add an installation-local Package Registration with an expected-identity override and call `open` through the capability, read another Package's Package Data Directory, or drive the running application.

Encrypting the stored keys under a key-encryption key held in the macOS login Keychain would therefore defend one attacker class only — someone who obtains a copy of the data directory, through an unencrypted backup, a cloud-synced development profile, or a copied folder. That class is also covered in part by FileVault and by encrypted backups. Against the class the trust model actually admits, the Keychain raises cost and nothing more. It has a real price: the project has no Developer ID signing, so an ad-hoc or linker-signed development binary changes code identity on every rebuild and the legacy-keychain ACL prompts again; the data-protection keychain that avoids those prompts needs an entitlement that requires Team ID signing.

Three further facts constrain this decision.

One Installation key serves four protocols: Teamarness signatures through the domain-separated signing interface, p2panda operation headers, Iroh QUIC/TLS endpoint authentication, and — since ADR 0013 — an X25519 conversion used to open protection controls. Each of the first three is verified by a foreign verifier over its own message framing, so none can be moved behind the signing helper.

The data directory was accepted on trust. `prepare_private_directory` created any path it was given and applied mode 0700, with no check that the path was not a symbolic link, that it belonged to the current user, or that it held Teamarness data rather than the user's home directory. Off Unix it skipped permissions silently.

Each Content Key was stored twice: once in `sync_scopes.content_key`, and again in `protection_state.state_bytes` behind a `teamarness.secret-bundle/v1` prefix that no code ever read.

Requirement text also overstated the guarantee: ID-R01 claimed the private key was confined to the runtime, and two code comments claimed that the protection projection holds no plaintext content key. Neither was true.

## Decision

### 1. Custody

The Core Runtime stores the Installation secret and each Content Key unencrypted in `core.sqlite3`, inside a private data directory.

The data directory must be private: its final path component is not a symbolic link, it is a directory, it is owned by the effective user, and it holds Teamarness data — it carries the marker file `.teamarness-private-directory`, or it is empty, or it holds `core.sqlite3`, which is adopted by writing the marker. The directory is restricted to its owner (0700) and the database and its journal to the owner (0600), and only after the checks pass. On a platform where the Core Runtime cannot apply those restrictions it fails closed and refuses to open.

Every process running as the same operating-system user can read those secrets, including trusted Package Backends (ADR 0007). No Semantic Capability exposes them, and no Teamarness API, log, or diagnostic returns them: that is a supported-contract boundary, not a confidentiality boundary against the user's own code. Confidentiality at rest rests on operating-system account separation, FileVault, and encrypted backups.

### 2. Grants

A Protection Control is sealed to its recipient and signed through the domain-separated signing interface, as ADR 0013 decides and `crates/teamarness-core/src/synchronization/protection_control.rs` implements. This decision does not restate that format.

### 3. Key reuse

The Installation Ed25519 key is used for exactly four things:

- Teamarness signatures, through the domain-separated signing interface;
- p2panda operation headers, verified by p2panda over its own CBOR header framing;
- Iroh QUIC/TLS endpoint authentication;
- conversion to an X25519 secret that opens protection controls.

No other use is permitted without a new decision. The first three are separated by their message framing: no verifier of one will accept a value produced for another. The fourth is signing-plus-key-agreement reuse of one key pair, which is analysed in Thormarker, "On using the same key pair for Ed25519 and an X25519 based KEM" (IACR ePrint 2021/509), and which `ed25519-dalek` cites for the same conversion. A dedicated X25519 pre-key published in Canonical Membership is the alternative, and it is reconsidered together with content-key rotation and multi-device identity in roadmap Phase 5.

### 4. Hygiene

- Each Content Key exists in exactly one column, `sync_scopes.content_key`.
- The Core database runs with `secure_delete` on, so freed pages are zeroed on disk.
- Content Keys and the Installation secret move through Core code inside zeroizing buffers, and a Content Key does not implement serialization and does not print its bytes.

Zeroization is best effort. Rust moves, SQLite's page cache, the operating-system file cache, `rusqlite`'s own copies, the buffers inside the HPKE and AEAD crates, and swap can all leave a copy behind. It shortens how long a secret stays in this process's memory; it does not make the secret unrecoverable.

### 5. Revisit triggers

Platform key custody — a key-encryption key held by the operating-system keystore, with only wrapped secrets in `core.sqlite3` — is reopened when any of these happens:

1. Developer ID signing or notarization ships.
2. Teamarness plans to host Packages that are not fully trusted: Workspace-shared Packages, remote fetch, or any sandbox.
3. A feature exports or uploads the data directory, a diagnostics bundle, or backups.
4. Shared or multi-user Macs, or unmanaged devices without FileVault, become a stated target.
5. Multi-device human identity or content-key rotation is designed (roadmap Phase 5).

Reaching a trigger calls for a new decision and a measurement of prompt behaviour, not for reviving this one. Every key read and write passes through `storage` and the `ContentKey` type, so custody can be moved behind those without touching a caller.

## Rejected alternatives

### Store a key-encryption key in the macOS Keychain now

Rejected. It defends a leaked copy of the data directory and nothing else in the ADR 0007 trust model, and that class is already partly covered by FileVault and encrypted backups. Without Developer ID signing, the login keychain's ACL trusts the creating binary by code signature, so every development rebuild is expected to prompt; the data-protection keychain that avoids that needs a `keychain-access-groups` entitlement and Team ID signing. It also adds failure modes with no security benefit: a moved or renamed development profile orphans its item and makes the Installation unrecoverable, a copied data directory is unusable on another Mac, and test binaries cannot answer prompts, so every automated test would need a second file-custody backend. The triggers above record when to weigh it again.

### Move Package Data Directories out of the Core private tree

Rejected. Under ADR 0007 a trusted Backend reaches either location with the same authority, so the move creates no boundary. It would migrate every host-store mapping to buy nothing.

### Exclude the data directory from backups

Rejected. Marking `core.sqlite3` as excluded would make a restore lose the Installation identity and Canonical Membership, which is a worse outcome than the exposure it avoids. The README tells the user to encrypt backups instead.

### Authenticate `registrations.v1.json`

Rejected. A registration override written by code running as the user is inside the accepted trust model, and signing the file would be checked by the same process the model already trusts.

### Derive a per-column Content Key identifier other than `BLAKE3(content_key)`

Rejected. The key is 256 bits of randomness, so its hash reveals nothing useful, and the identifier is part of the sealed record envelope.

## Consequences

- The Installation secret and every Content Key stay unencrypted in the private data directory, readable by any process running as the user, including trusted Package Backends. This is stated in ID-R01, PKG-R11, the ontology, and the README rather than being implied away.
- A data directory that is a symbolic link, is owned by another user, or holds unrelated files is refused at startup, and the desktop names the reason. `--data-dir $HOME` no longer silently becomes a Teamarness data directory, and is never `chmod`ed.
- Off Unix the Core Runtime refuses to open rather than running without restricted permissions.
- Each Content Key exists in one column, freed pages are zeroed, and key buffers in Core code are zeroized on a best-effort basis. `protection_state` carries only the scope, the Content Key identifier, and the creator's public identity.
- ID-R01, ID-R04, PKG-R11, and TRANS-R03 state the real guarantee, and `TEAM.SYS.ID-R07 — Private Data Directory` is added.
- ADR 0008's protection boundary and ADR 0009's "Core retains raw content-key custody" stand; this decision says what that custody is worth and what it is not.
- DELTA-005 closes: its grant half closed with ADR 0013, and its storage half is accepted here.
- Nothing here defends against malware running as the user, a compromised Package Backend, or an attacker who can read this process's memory. Those are outside the version-1 model and are named, not mitigated.

## Evidence

- `crates/teamarness-core/src/storage/mod.rs`: identity persistence, the private-directory rule and its marker, database and directory permissions, and `secure_delete`.
- `crates/teamarness-core/src/synchronization/content_key.rs`: the `ContentKey` type, its zeroizing buffer, its identifier, and its non-printing `Debug`.
- `crates/teamarness-core/src/synchronization/mod.rs`: the single `sync_scopes.content_key` column and the `protection_state` insert.
- `crates/teamarness-core/migrations/0012_single_content_key_copy.sql` and `crates/teamarness-core/src/storage/migrations.rs`.
- `crates/teamarness-core/tests/identity_persistence.rs`: symlink, foreign-owner and unrelated-contents refusal, marker writing and legacy adoption, and journal permission inheritance.
- `crates/teamarness-core/tests/immutable_record_recovery.rs::content_key_is_stored_exactly_once`: one occurrence of a live Content Key across every table, every column, and the raw database file.
- `crates/teamarness-core/tests/migrations.rs::migration_0012_removes_duplicate_content_key_copy`.
- ADR 0013 for the protection control format, its HPKE sealing, and its domain-separated signature.
- ADR 0007 for the trust model that makes plaintext at rest an accepted risk rather than an oversight.
- Thormarker, "On using the same key pair for Ed25519 and an X25519 based KEM", IACR ePrint 2021/509, for the reuse in clause 3.
- **Weaknesses:** the Keychain prompt behaviour for ad-hoc and linker-signed rebuilds, and the entitlement requirement of the data-protection keychain, come from platform documentation and were not measured in this repository, so the cost side of the rejected option is an estimate. Zeroization is unverifiable from inside the process: no test proves a key is absent from memory, only that Core's own buffers are wrapped. The private-directory rule checks the final path component only, because macOS temporary paths reach through system symbolic links, so a symbolic link earlier in the path is not detected.

### Provenance (non-versioned)

`.myflow/workstreams/architecture-review/design/20260917T233000+1200_phase-7-key-custody.md`, the matching plan, and `.myflow/workstreams/architecture-review/review/security.md` (SEC-02, SEC-08, SEC-10, SEC-11) and `review/synchronization.md` (SYN-02).
