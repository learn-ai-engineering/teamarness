# 0001 — VRS intent tree as the context authority

**Status:** accepted (2026-09-12)

## Context

The project needs an intent layer: a single source of truth for what Teamarness is, what it should do, what decisions have been made, what questions remain open, and where the implementation diverges from the spec. Without a canonical intent layer, documentation, code, and other surfaces drift into competing sources of truth.

LiveStore (`livestorejs/livestore`) uses a VRS (Versioned Requirements Specification) intent tree rooted at `context/`. This decision adopts that pattern for this project.

## Decision

### Root at `context/`

The `context/` directory is the product-scoped VRS root. It is the canonical, settled intent for the Teamarness product. All other surfaces — documentation, code comments, README — derive from `context/`.

### Structure

```
context/
  vision.md           — what this is and why
  ontology.md         — canonical domain terminology
  requirements.md     — root product requirements (TEAM prefix)
  spec.md             — branch table and cross-cutting rules
  roadmap.md          — phased delivery sequence
  open-questions.md   — unresolved design questions (TEAM-DQ prefix)
  .decisions/         — accepted architectural decisions (numbered, immutable)
  .delta/             — known divergence from spec (DELTA prefix)
  01-product/         — product requirements and success criteria
  02-system/          — system architecture by domain
    NN-domain/        — requirements, spec, and decisions per subsystem
  03-delivery/        — build, release, CI/CD
  04-contributing/    — development setup, standards, workflow

```

### Rules

1. **VRS is canonical.** `context/` is the settled intent. RFCs propose; VRS records settled decisions. When an RFC is accepted, its durable content folds into the owning VRS nodes and the RFC becomes a historical record.
2. **Decisions are numbered and immutable.** `context/.decisions/NNNN-title.md`. An accepted decision is never edited — it is superseded by a later decision that cites it. Decisions record the context, options, chosen option, evidence, and consequences.
3. **Deltas track divergence.** `context/.delta/DELTA-NNN-title.md` records known gaps between spec and implementation. Each delta owns its resolution plan. Deltas are temporary — they either resolve into implementation or are promoted to requirements.
4. **Ontology is the term authority.** Every domain term in requirements, specs, code, and UI derives its meaning from `context/ontology.md`. Divergence is a bug.
5. **One node owns every line of intent.** No requirement, spec clause, or decision lives in two places. Cross-references cite the owning node by prefix.
6. **Open questions block dependent work.** Each entry in `open-questions.md` names the subsystem it blocks and what would resolve it.
7. **Roadmap is the delivery order.** `roadmap.md` records what we build and in what sequence. It does not replace requirements — it sequences them.

## Evidence

- LiveStore's VRS tree (`livestorejs/livestore/context/`) demonstrates this pattern in production use.
- LiveStore decisions 0001 (root VRS at context) and 0002 (single intent layer) establish the rationale for a product-scoped canonical tree.
- The existing Teamarness project used a simpler `context/` layout without the full VRS discipline. This project adopts the stricter VRS rules from the start.

## Rejected alternatives

### RFCs as the primary intent layer

Rejected. RFCs are proposals, not settled intent. A repository where RFCs are the source of truth has no single place to answer "what does this system actually do?" After acceptance, RFC content must fold into VRS.

### Docs-site as the intent layer

Rejected. Documentation teaches users; it does not define requirements. Mixing normative intent with user-facing narrative creates ambiguity about what is a requirement versus an explanation.

### Separate spec repository

Rejected. Keeping the spec in a separate repository guarantees drift. The spec lives with the code it describes.

### Informal conventions (no structure)

Rejected. Without an enforced structure, intent scatters across issues, PR descriptions, code comments, and Slack messages. The VRS tree provides one place to look.

## Consequences

- Every design discussion and code review can cite a specific VRS node.
- The tree is public in the repository. Intent is visible to all contributors.
- Writing requirement or spec text for a subsystem is a deliberate act with a known home.
- Deltas make technical debt visible and trackable. Nothing hides.