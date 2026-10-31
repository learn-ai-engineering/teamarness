# 0037 — `uiHostVersion` also counts additive mount-contract changes

**Status:** accepted (2026-10-25)
**Refines:** ADR 0018 (§1, "`uiHostVersion` … (1 today)" and "the private frame bridge protocol … (2 today)"; §4, which lists a `uiHostVersion` bump among breaking changes), ADR 0015 (Consequences, "`uiHostVersion` stays `1`")

## Context

`uiHostVersion` is the version of the author-visible mount contract: the `context` object a Package UI's `mount(context)` receives. It is one integer, not a major and minor pair. ADR 0018 lists a `uiHostVersion` bump among the breaking changes that need a deprecation window.

A Package can now declare an optional settings view in `ui.views`. The mount context gains a `view` field, `"main"` or `"settings"`. A UI that does not read `view` works unchanged, because it always gets the main view. The private frame bridge protocol also changes, because the mount message now carries the view.

The change is additive, but authors need a way to know whether the Host supplies `context.view`.

## Decision

1. `uiHostVersion` is 2. Version 2 adds `context.view`. Every version 1 field keeps its meaning, so a UI written for version 1 keeps working.
2. `uiHostVersion` increases for every author-visible mount-contract change. An additive change, one that a UI written for the previous version can ignore, increases it without a deprecation window. A breaking change also increases it and follows the ADR 0018 §4 window.
3. The author documentation states for each version what it adds and whether earlier UIs keep working.
4. The private frame bridge protocol (`FRAME_BRIDGE_PROTOCOL`) is 3. It is not author surface and ships with the Shell, as ADR 0018 already says.

## Rejected alternatives

- **Keep `uiHostVersion` at 1 and add `view` silently.** A UI could not tell a Host that supplies `view` from one that does not, and the version would stop describing the contract.
- **Split `uiHostVersion` into a major and a minor.** It matches the capability rule, but it changes the type of a field every SDK and test fixture reads, for one additive field. A single integer with a stated rule is enough while there is no install base.
- **Treat the bump as breaking and build the deprecation window now.** Nothing breaks: every version 1 UI keeps working.

## Consequences

- A version number alone does not tell an author whether a change breaks them. The author guide and the UI SDK README say so for each version.
- The first breaking mount-contract change still needs the ADR 0018 §4 window, and an ADR of its own.
- The Host, the Shell DTOs, the UI SDK (`PACKAGE_UI_HOST_VERSION`), and the template provenance all report 2.

## Evidence

- `crates/teamarness-package-host/src/host/ui_mounts.rs`: `UI_HOST_VERSION = 2`, sent in every UI loader descriptor.
- `apps/teamarness-desktop/src/package-frame/protocol.ts`: `FRAME_BRIDGE_PROTOCOL` 3 and the mount message's required `view`.
- `packages/teamarness-package-ui-sdk/src/index.ts`: `PACKAGE_UI_HOST_VERSION = 2`, `PackageMountContext.view`.
- `templates/package-foundation/v1/teamarness-authoring.json`: provenance `uiHostVersion` 2.
- `docs/package-author-guide.md`, "What can change under you": version 2 adds `context.view`, and a version 1 UI keeps working.
- `crates/teamarness-package-host/tests/ui_resources.rs` `a_mount_shows_only_a_declared_view`: a main mount reports `ui_host_version` 2 and `view: main`.
- **Weaknesses:** nothing checks a UI against a `uiHostVersion`, so the "keeps working" claim rests on the existing UI tests passing unchanged apart from fixtures. No Package outside this repository was tested.

### Provenance (non-versioned)

- `~/.myflow/repositories/github.com/learn-ai-engineering/teamarness/workstreams/packagement/{design,plan,implement}/`
