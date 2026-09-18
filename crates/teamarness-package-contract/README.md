# teamarness-package-contract

One set of Package admission rules, shared by two callers.

- The **Package Host** (`teamarness-package-host`) calls this crate when it admits a Package
  Directory. It is the authority at runtime.
- The **author validator** (`teamarness-package-validate`, in this crate) calls the same code so a
  Package author gets the host's verdict without starting the application, and so a generated
  project can vendor it.

The Host depends on `teamarness-core` (Iroh, p2panda, SQLite) and cannot be vendored. This crate
depends only on `jsonschema`, `mime`, `semver`, `serde`, `serde_json` and `sha2`. There is no Tokio
and no Core here.

## What it holds

| Module | Rules |
|---|---|
| `identity` | the lowercase reverse-DNS Package Identity grammar |
| `json` | strict JSON parsing: duplicate object keys are rejected |
| `manifest` | `teamarness.package.json`: structure, identity, strict semantic version, display name, protocol range, extensions, capabilities, paths, sizes, digests and UI totals |
| `ui_contract` | the UI contract profile: local `$defs` references only, no dynamic references, no cycles, depth 64, 16,384 nodes, 128 operations per direction and 256 in total, Draft 2020-12 |

`manifest::parse_manifest` applies every rule that needs no file access, so a project source whose
assembler has not filled in sizes and digests still validates. `manifest::load_package_directory`
is the Host's admission path: it parses, then reads and verifies every declared file.

## The error shape

```rust
pub struct ContractError { pub code: ContractErrorCode, pub rule: &'static str, pub pointer: Option<String> }
```

- **`code`** is the Host's stable, payload-free vocabulary (`manifest_malformed`,
  `manifest_unsupported`, `invalid_package_identity`, `target_unsupported`, `artifact_missing`,
  `artifact_changed`, `ui_entrypoint_missing`, `ui_contract_violation`, plus `smoke_failed`, which
  only the validator produces).
- **`rule`** is a stable author-facing identifier such as `ui_contract.reference_not_local` or
  `manifest.digest_mismatch`. Each module's documentation lists its rules.
- **`pointer`** is a JSON pointer, or a relative file path, when one is known.

**The Host maps only `code`.** `rule` and `pointer` are author diagnostics; they stop at the
command line and never reach Host diagnostics, which stay payload-free.
