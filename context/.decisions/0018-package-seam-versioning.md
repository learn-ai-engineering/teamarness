# 0018 — Package seam versioning

**Status:** accepted (2026-09-19)
**Refines:** ADR 0007 (the Package boundary), ADR 0008 (the immutable-record capability contract)

## Context

The Package boundary is the product's most important interface and the one most likely to change. Three things cross it: the Package Process Protocol, the Semantic Capability contracts, and the Shell↔Package UI bridge. Today's synchronization capability is an immutable-record log. Other Packages will need other shapes, such as document-oriented synchronization.

A Package is written outside this repository. Its author needs to know what can change under them, what cannot, and how they will be told.

## Decision

### 1. What carries a version

Three independent version lines, each with its own compatibility rule.

- The **Package Process Protocol** (`{major, minor}`, 1.0 today): the frame set, the lifecycle operations, and the `initialize` handshake. It is negotiated once per process generation, at `initialize`.
- Each **Semantic Capability family** (an identity that names the family, plus `{major, minor}`): its request and event shapes, bounds, vocabularies, and error codes. A Package declares a family and an accepted version range in its Package Manifest; the Package Host grants one version per activation.
- The **Shell↔Package UI bridge**: `uiHostVersion` for the author-visible mount contract (1 today), and the private frame bridge protocol for the Shell-internal transport (2 today). The private protocol is not author surface and may change whenever the Shell and its frame runtime ship together.

The Package Manifest schema and the UI contract profile version separately, and neither is negotiated at run time: a Package Directory is admitted or refused.

### 2. How versions are negotiated

The Host offers a family with a supported major and a minor range; the Package declares a required major and a minor range; the highest common minor is selected and written into the grant and into the `initialize` capability entry. A major mismatch, or an empty minor intersection, refuses activation with a capability version error. An unknown family is an unsupported capability, not a version error.

Within a major, a minor may only add: new optional request fields, new event kinds a Package may ignore, and wider bounds. Nothing is removed or narrowed within a major.

**A closed vocabulary belongs to its major.** A capability's enumerated values — the record log's `availability` states, its `open` statuses — are part of the contract's shape, not a bound. Adding a value to one is a new major of that family, not a minor. The typed SDK clients therefore refuse a value outside the schema as `package.invalid_capability_response` rather than mapping it to an "unknown" placeholder: a Package that mis-reports a state it does not understand is worse than a Package that stops and says so. A Host that needs a new state either takes a new major, with the deprecation window below, or defines a new family.

### 3. How a new capability family is added

A new family is a new contract file, a new Host descriptor, and a new typed SDK client. It changes no existing family, no lifecycle operation, no Package Process Protocol version, and no UI contract. A Package that does not declare it is unaffected; a Package that declares it and runs on a Host that does not provide it fails activation with a clear unsupported-capability error.

Document-oriented synchronization is the worked example: it is a family beside the immutable-record log, not a version of it.

### 4. Deprecation window (intended policy, not implemented)

When a breaking change eventually lands — a new capability major, a Package Process Protocol major, or a `uiHostVersion` bump — the Host supports the previous major alongside the new one for one release cycle; the Package Catalog reports which Packages still use a superseded major; and the author documentation and the Package Validator warn before the Host refuses.

None of that exists today and none of it is needed: there is no install base, no published SDK, and no external Package. This clause records the intent so the first breaking change starts from a decided position.

## Rejected alternatives

- **One global "Package API version" covering all three lines.** It couples unrelated changes and forces lockstep releases: a frame-bridge fix would renegotiate every capability.
- **Versioning capabilities by the identity string alone.** An author cannot express an accepted range, and the Host cannot add a compatible minor.
- **Adding document-oriented synchronization as a version of the record log.** Two incompatible shapes behind one family, with every Package forced to discriminate.
- **Tolerating an unknown vocabulary value within a minor, mapped to "unknown".** It buys additive vocabulary growth at the price of a Package that silently reports a state it cannot interpret, in the one capability whose whole purpose is telling a person whether their data is current. The refusal is loud, bounded to the subscription, and retried with backoff.
- **Implementing a deprecation window now.** There is nothing to deprecate.

## Consequences

- Every capability grant names a family and a version. A new family is additive, so the record log's successors cost existing Packages nothing.
- Author-visible breaks are limited to the three lines named here, and each is recorded as an ADR.
- A new `availability` state or `open` status for the immutable record log is a major version of that family. The SDK needs no change for this decision; its refusal of an unknown value is the behaviour this ADR ratifies.
- The deprecation window is a promise about the first breaking change, not a mechanism that exists. Whoever lands that change builds it.

## Evidence

- `crates/teamarness-package-host/src/capability.rs` — capability descriptors, families, and negotiation; `negotiates_the_highest_common_minor`, `unknown_capability_family_is_not_a_version_error`, `adding_a_capability_family_does_not_change_lifecycle_or_ui_contracts`, `capability_limits_match_contract_schemas`.
- `crates/teamarness-package-contract/src/manifest.rs` — `CapabilityRequest`'s declared family and accepted minor range, and the `manifest.capability_range` rule.
- `contracts/package-capabilities/v1/README.md` — "Families and versions", the normative statement of the additive-minor rule.
- `contracts/package-process/v1/README.md` — the protocol's own version and limits.
- `apps/teamarness-desktop/src/package-frame/protocol.ts` — `FRAME_BRIDGE_PROTOCOL`, the private bridge protocol version, and `uiHostVersion` in the mount snapshot.
- `crates/teamarness-package-sdk/src/capabilities/record_log.rs` — the typed client that refuses a vocabulary value outside the schema, with `state_events_drive_sync_status_for_every_availability` in `packages/conversations-package/backend/src/backend.rs`.
- **Weaknesses.** Nothing enforces the additive-minor rule mechanically: a minor that narrowed a bound would be caught only by review and by the schema-pinning test. No test exercises two majors of one family side by side, because only one major exists.

### Provenance (non-versioned)

`.myflow/workstreams/architecture-review/plan/20260918T000000+1200_phase-6-package-contract-authoring.md` and its design, and `.myflow/workstreams/architecture-review/decisions/20260918_developer-answers.md` amendment A6.
