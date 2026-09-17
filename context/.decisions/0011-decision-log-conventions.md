# 0011 — Decision log numbering, format, phase labels, and evidence

**Status:** accepted (2026-09-17)

## Context

The decision log had drifted from the rules in ADR 0001:

- **Duplicate number.** Two accepted decisions used `0008`: the immutable-record capability contract and the vendored Package authoring foundation. "ADR 0008" was ambiguous in every citation.
- **Three header styles.** Titles used `# 000N — Title`, `# 000N: Title`, `# 000N - Title`, and `# ADR 000N: Title`. Status appeared as a `**Status:**` line, a bulleted `- Status:` and `- Date:` list, or a `## Status` section.
- **Missing sections.** ADR 0002 and ADR 0003 have no Evidence section. The vendored authoring decision has no Context. ADR 0008 has no Rejected alternatives. ADR 0009 has neither Context nor Rejected alternatives.
- **Mixed phase numbering.** Some ADRs use "Phase N" for a step inside a workstream plan, while others use it for a roadmap phase. ADR 0009 mixes both.
- **Unreachable evidence.** ADRs 0005, 0006, 0007, and 0008, and open questions TEAM-DQ6 and TEAM-DQ7, justified themselves by citing local `.myflow` working artifacts. Those files are not versioned, so a reader of the repository cannot reach or check the justification. ADR 0001 says the tree is public.

## Decision

### 1. Renumber the vendored authoring decision

The vendored Package authoring decision is renumbered to `0010` (`0010-vendored-package-authoring.md`). Its decision text is unchanged; only its header was normalised so that it identifies itself by its new number. `0008` means the immutable-record capability contract and protection boundary.

The index in `context/.decisions/README.md` records the former file name so older citations still resolve.

### 2. One template from 0011 onward

ADRs numbered `0011` and later use the template in `context/.decisions/README.md`. Existing ADRs are not reformatted, apart from the `0010` header above. Accepted ADRs are immutable (ADR 0001 rule 2); the index shows every status in one uniform format.

### 3. Phase labels

In an ADR, "Phase N" means a phase in `context/roadmap.md`. A step inside a workstream plan is called a "workstream step", never a "Phase".

Clarifications for earlier ADRs:

- ADR 0008 "Phase 1's bounded Encryption gate" was step 1 of the Conversations Package workstream plan, inside roadmap Phase 4.
- ADR 0009's status line "Phase 5, 2026-09-15" was step 5 of that workstream's recovery plan. The decision belongs to roadmap Phase 4. The references in ADR 0009's Consequences ("Phase 5 key rotation", "Phase 4 relies on") are roadmap phases and are correct.
- ADR 0004 "Shared package-key generation and distributed finality remain Phase 4 work" and "Phase 4 must design shared key rotation": roadmap Phase 4 chose one content key without rotation (ADR 0008). Rotation is roadmap Phase 5 work.

### 4. Evidence policy

An ADR's Evidence section is self-sufficient in versioned text. It may cite tracked repository paths, and it states known weaknesses. Local, non-versioned working artifacts (for example files under `.myflow`) may appear only under `### Provenance (non-versioned)`. They are never required to understand or check the decision.

The Rationale of an open question in `context/open-questions.md` follows the same rule.

## Evidence summaries for earlier decisions

These summaries make ADRs 0005–0008 stand on their own. They supplement, and do not change, those decisions.

### ADR 0005 — Iroh workspace onboarding

- Research on 2026-09-12 confirmed `iroh` 1.2.0 (Rust ≥ 1.91) and `iroh-tickets` 1.0.0 against `iroh-base` 1.2.0. A throwaway probe compiled with exact pins.
- A second probe opened two relay-disabled endpoints, round-tripped an `EndpointTicket`, checked both authenticated remote IDs, exchanged one request and response on a bidirectional stream, and closed cleanly.
- Iroh documents that `online()` has no timeout, which is why readiness is capped at 5 s, and that 0-RTT data can be replayed, which is why 0-RTT is excluded.
- A code review found two defects. Invitation expiry was checked before prior use, so a response lost after the issuer commit could never be recovered; the fix is to check for a spent nonce before expiry. Startup needs one transition row per operation; the fix is to ship transition order.
- Tauri 2.11.5 `app_data_dir()` depends only on the bundle identifier, which is why `--data-dir` exists.
- Three design passes compared a minimal interface, storing the exact response, and regenerating from current state. The final review found that only current-state completion keeps membership matching when it changes before a retry, and a follow-up review passed.
- **Weaknesses:** the probes ran on loopback without relays. Nothing was implemented, relay-tested, or bundle-verified at decision time. The choice between regenerating and storing rests on review, not a test. The review records themselves were not kept; the two review identifiers cited by ADR 0005 cannot be resolved to stored review content.

### ADR 0006 — Rust sidecar Package Backends

- One fixture ran against a native `cdylib` (C ABI), a `wasm32-wasip2` component (Wasmtime 48.0.1), and a Rust sidecar (length-prefixed JSON over stdio). Each passed 20/20 checks, including restart persistence.
- On macOS, the ad-hoc-signed hardened host refused a separately built local dylib ("different Team IDs") unless `com.apple.security.cs.disable-library-validation` was set. It launched the local sidecar normally.
- **Failure probes.** Native: an abort killed the host, and an infinite loop stalled it. Sidecar: on abort the host saw EOF and started a replacement; an infinite loop hit the 2 s deadline, was killed, and was replaced. Wasm: traps and fuel limits recovered.
- Wasm guest calls were synchronous, and its host adapter needed 225 lines. The transport stand-in proved only begin and cancel, not Tokio or Iroh background work.
- Throughput and startup did not decide the choice. Calls per second: sidecar 5,844, Wasm 9,634, native 8,704, in one run.
- **Weaknesses:** one noisy run, ad-hoc (not Developer ID) signing, Apple silicon only. Orphan cleanup after the host dies was not proven. The sidecar did not prove Iroh background work either. ADR 0007 later moved Iroh into Core, which weakens the objection to Wasm. WebAssembly remains deferred, not rejected.

### ADR 0007 — Language-neutral Package host capabilities

- This is an ownership and product-scope decision taken with the developer, not a measured result.
- Four independent architecture passes compared generic dispatch, explicit lifecycle, Workspace-oriented orchestration, and private ports.
- A domain model tested the vocabulary against three scenarios: a non-Rust executable passing conformance, a hidden Package staying active, and an unavailable capability version failing negotiation with no raw-transport fallback.
- The rationale for Package-owned storage relies on ADR 0006's ambient-authority finding.
- **Weaknesses:** no measurements. The non-Rust conformance fixture (`contracts/package-process/v1/fixtures/node-package-process.mjs`) came later.

### ADR 0008 — Immutable record capability contract

- **P2Panda Spaces rejected:** it adds a second auth and space model, and it rotates the group secret on removal (`EncryptionGroup::remove`). Its crate warns that the interface is not production-stable before 1.0.
- **P2Panda Net rejected:** it would compete with Core's Endpoint and Router ownership. `p2panda-sync` accepts a caller-supplied sink and stream, and Iroh's discovery already resolves member identities.
- P2Panda Encryption 0.7.1 encrypts with XChaCha20-Poly1305. Its data function takes no associated data, so Core re-checks the sealed-prefix digest after decryption.
- The creator is the only first-key authority. A "lowest current Member" rule diverges under incomplete views, and "first valid key wins" strands ciphertext. `remove`, `update`, and rotation are refused before P2Panda is called.
- The 512 KiB `open` bound comes from the 1 MiB frame limit after base64url and JSON overhead.
- **Weakness:** the Encryption gate's group state machine was never run in a test. The contract-freeze tests round-trip only the direct XChaCha20 primitive, tamper rejection, and scope rejection. The direct-data fallback was selected by default, not because the state machine failed.

## Rejected alternatives

### Leave both 0008 files and disambiguate in an index

Rejected. "ADR 0008" would stay ambiguous in every future citation.

### Renumber the immutable-record decision instead

Rejected. Although the vendored authoring decision was committed first, "ADR 0008" already means the immutable-record contract in architecture reviews and in the remediation work that will keep citing it. The vendored decision had one tracked reference.

### Copy the evidence artifacts into `context/.evidence/`

Rejected. The artifacts are long working documents with local absolute paths, run identifiers, and superseded recommendations. Copying them would create a second owner of intent (ADR 0001 rule 5), and the ADR 0001 tree has no evidence directory.

### Reformat existing ADRs

Rejected. Accepted ADRs are immutable, and header style is cosmetic. The index gives a uniform view.

## Consequences

- Older non-versioned citations of `0008-vendored-package-authoring.md` resolve through the index in `context/.decisions/README.md`.
- `scripts/package-foundation-qualification.test.mjs` reads `context/.decisions/0010-vendored-package-authoring.md`.
- New ADRs follow the template and keep Evidence self-sufficient.
- ADR numbers are unique; the index gives the check to run before adding one.
- TEAM-DQ6 and TEAM-DQ7 summarise their evidence inline.

## Evidence

- `context/.decisions/0001-vrs-intent-tree.md` (rules 2 and 5)
- `context/.decisions/0004-p2panda-membership-dag.md`
- `context/.decisions/0005-iroh-workspace-onboarding.md` to `context/.decisions/0010-vendored-package-authoring.md`
- `context/open-questions.md` (TEAM-DQ6, TEAM-DQ7)
- `scripts/package-foundation-qualification.test.mjs`

### Provenance (non-versioned)

Local working artifacts cited by earlier decisions and open questions, listed so a reader with the working notes can still find them. They are not required to understand or check any decision.

- ADR 0005 and TEAM-DQ7: `.myflow/workstreams/shell-package-lifecycle/design/20260913T002055Z_workspace-onboarding-protocol.md`
- ADR 0005: `.myflow/workstreams/shell-package-lifecycle/research/20260912T231307Z_iroh-workspace-onboarding-protocol.md`
- ADR 0006 and ADR 0007: `.myflow/workstreams/package-foundation/prototype/20260913T131901Z_runtime-execution-model.md`
- ADR 0006: `.myflow/workstreams/package-foundation/prototype/runtime-model/results/summary.md`
- ADR 0006: `.myflow/workstreams/package-foundation/research/20260913T123921Z_runtime-loading-models.md`
- ADR 0007: `.myflow/workstreams/package-foundation/scope/20260913T212911Z_package-host-and-capability-direction.md`
- ADR 0007: `.myflow/workstreams/package-foundation/domain-model/20260913T214230Z_package-host-capabilities-and-ui-lifecycle.md`
- ADR 0008: `.myflow/workstreams/conversations-package/design/20260915T051922Z_conversations-package.md`
- TEAM-DQ6: `.myflow/workstreams/shell-package-lifecycle/research/20260912T221751Z_tauri-package-execution-storage-and-core-boundary.md`
- The vendored authoring decision (now `0010`) was cited by its former file name in `.myflow/workstreams/external-package-authoring/` plan and handoff notes.
