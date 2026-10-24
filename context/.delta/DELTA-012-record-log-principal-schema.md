# DELTA-012 — Record-log 1.1 request schema omits the Principal selector

**Status:** open (2026-10-24). Owner: Package platform authoring-contract maintenance, before supported SDK publication.

## Divergence

`TEAM.SYS.PKG-R10` and ADR 0023 permit an optional `principal` selector for immutable-record-log 1.1 appends. Host and Rust SDK support the selector, but `contracts/package-capabilities/v1/immutable-record-log-1.1.schema.json` omits it from the closed request schema for `append`. A client that validates requests against that artifact rejects an otherwise supported Agent-selected append.

The mismatch is already present in incoming local main `a947a17`, before the Agent feature integration. The focused integration review identified it as upstream authoring-contract debt. The Agent harness posts through Conversations Ports, not this JSON request-schema artifact. Passing Agent runtime qualification does not resolve the mismatch.

## Follow-up

Align the versioned request schema with the existing Host and SDK contract. Add qualification for accepted minor-1 requests with and without `principal`, including invalid selectors and the unchanged minor-0 boundary. Do not broaden signing authority or expose keys. Close this delta when the contract artifact and qualification agree with supported behavior.

## Tracking

- `TEAM.SYS.PKG-R10`, ADR 0023, `context/roadmap.md` Later.
- `agent-package` workstream: `verify/20261024T085236Z_fidelity.md` and `verify/20261024T092503Z_post-merge-review.md`.
- Separate maintenance scope. No runtime change, SDK publication, merge, or push is authorized by this delta.
