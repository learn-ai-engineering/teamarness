# Teamarness — Specification

Branch table for the Teamarness product VRS. Every requirement, spec clause, decision, and delta is owned by exactly one branch node. This file is the root; subsystem branches delegate to their own `spec.md` or `requirements.md`.

## Branch table

| Prefix | Node | Owns |
|---|---|---|
| `TEAM` | Root (`context/`) | Product-wide requirements that span subsystems |
| `TEAM.PROD` | `01-product/` | Product requirements, vision, roadmap, success criteria |
| `TEAM.SYS.ID` | `02-system/01-identity/` | Installation identity, Iroh endpoint binding, key custody, public identity |
| `TEAM.SYS.MEM` | `02-system/02-membership/` | Canonical membership log, intervals, invitations, revocation, epochs |
| `TEAM.SYS.TRANS` | `02-system/03-transport/` | Iroh endpoint management, relay configuration, Core synchronization transport (LogSync sessions, durable wakes), and Package access to transport |
| `TEAM.SYS.CONV` | `02-system/04-conversations/` | The Conversations Package: Channels, Messages, Channel participation policy, record validation and projections, local read state, and its UI. Consumes the immutable-record Synchronization Capability |
| `TEAM.SYS.PKG` | `02-system/05-packages/` | Package Manifests, Sources, Registrations, Admission, Package Host, Process Protocol, lifecycle, Semantic Capabilities, SDKs, local data, trust, Synchronization Capability contracts, Content Views, the Exhibits Package, and the reference and tutorial example Packages |
| `TEAM.SYS.REPO` | `02-system/06-repos/` | The Repo-aware Package: repository registration, local read-only Git inspection, and repository views. No repository replication |
| `TEAM.SYS.SHELL` | `02-system/07-shell/` | Tauri shell, package frame mounting, workspace controls, theming |
| `TEAM.SYS.OBS` | `02-system/08-observability/` | Logging, metrics, error reporting, diagnostics |
| `TEAM.SYS.DOCS` | `02-system/09-documents/` | The Documents Package: Collections, Documents with a collaborative Markdown body, Comments, record validation and projections, and its UI. Consumes the immutable-record Synchronization Capability (ADR 0027) |
| `TEAM.DEL` | `03-delivery/` | Build, release, CI/CD, update distribution, code signing |
| `TEAM.CONTRIB` | `04-contributing/` | Development setup, contribution workflow, code standards |

## Cross-cutting rules

1. **VRS is canonical.** `context/` is the settled intent for the product. RFCs propose; VRS records settled decisions. Docs derive from VRS.
2. **Decisions are numbered and immutable.** `context/.decisions/NNNN-title.md`. Once accepted, a decision is never edited — it is superseded by a later decision.
3. **Deltas are tracked divergence.** `context/.delta/DELTA-NNN-title.md` records known gaps between spec and implementation. Each delta must own its resolution plan.
4. **Ontology is the term authority.** `context/ontology.md` defines every domain term. Other surfaces that diverge from ontology definitions are bugs.
5. **Open questions block dependent work.** `context/open-questions.md` records unresolved design questions. Each entry names what it blocks and what would resolve it.
6. **One node owns every line of intent.** No requirement, spec clause, or decision lives in two places. Cross-references cite the owning node.

## Current state

The Context Foundation, Core Runtime, Desktop Shell with Workspace onboarding, Package Foundation, Conversations Package, Repository Integration, Shell and Package UI polish, architecture review, and Workspace Principals workstream are complete. The Agent Package workstream (ADR 0030) is complete and verified locally, including revision-based recovery, standalone Personal distribution, dispatch ownership and Host generation safeguards, and incoming-main integration. Automated checks and focused independent review pass; developer observations cover a limited real-endpoint and restart/offline flow, not the full manual matrix. The developer approved local integration only; remote delivery and publication require separate decisions. Phase 5 Membership Management and Hardening remains next for the core roadmap. See `roadmap.md` for workstream status and `open-questions.md` for unresolved design questions.