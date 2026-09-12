# Resonance — Specification

Branch table for the Resonance product VRS. Every requirement, spec clause, decision, and delta is owned by exactly one branch node. This file is the root; subsystem branches delegate to their own `spec.md` or `requirements.md`.

## Branch table

| Prefix | Node | Owns |
|---|---|---|
| `RES` | Root (`context/`) | Product-wide requirements that span subsystems |
| `RES.PROD` | `01-product/` | Product requirements, vision, roadmap, success criteria |
| `RES.SYS.ID` | `02-system/01-identity/` | Installation identity, Iroh endpoint binding, key custody, public identity |
| `RES.SYS.MEM` | `02-system/02-membership/` | Canonical membership log, intervals, invitations, revocation, epochs |
| `RES.SYS.TRANS` | `02-system/03-transport/` | Iroh endpoint management, relay configuration, package transport access |
| `RES.SYS.CONV` | `02-system/04-conversations/` | Channels, messages, channel membership, conversation keys, Iroh message delivery |
| `RES.SYS.PKG` | `02-system/05-packages/` | Package manifest, lifecycle, semantic capabilities, SDK, trust model |
| `RES.SYS.REPO` | `02-system/06-repos/` | Repository registration, local Git integration |
| `RES.SYS.SHELL` | `02-system/07-shell/` | Tauri shell, package webview mounting, workspace controls, theming |
| `RES.SYS.OBS` | `02-system/08-observability/` | Logging, metrics, error reporting, diagnostics |
| `RES.DEL` | `03-delivery/` | Build, release, CI/CD, update distribution, code signing |
| `RES.CONTRIB` | `04-contributing/` | Development setup, contribution workflow, code standards |

## Cross-cutting rules

1. **VRS is canonical.** `context/` is the settled intent for the product. RFCs propose; VRS records settled decisions. Docs derive from VRS.
2. **Decisions are numbered and immutable.** `context/.decisions/NNNN-title.md`. Once accepted, a decision is never edited — it is superseded by a later decision.
3. **Deltas are tracked divergence.** `context/.delta/DELTA-NNN-title.md` records known gaps between spec and implementation. Each delta must own its resolution plan.
4. **Ontology is the term authority.** `context/ontology.md` defines every domain term. Other surfaces that diverge from ontology definitions are bugs.
5. **Open questions block dependent work.** `context/open-questions.md` records unresolved design questions. Each entry names what it blocks and what would resolve it.
6. **One node owns every line of intent.** No requirement, spec clause, or decision lives in two places. Cross-references cite the owning node.

## Current state

The VRS is in initial establishment. Subsystem requirements and specs are being authored. See `roadmap.md` for the phased plan and `open-questions.md` for unresolved design questions.