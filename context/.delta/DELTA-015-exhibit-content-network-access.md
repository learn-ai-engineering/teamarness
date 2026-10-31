# DELTA-015 — Exhibit content frames can use the network

**Status:** open (2026-10-25). Owner: a network-control workstream after Exhibits slice 1. Accepted as a non-goal of Exhibits milestone 1.

## Divergence

The Exhibits guidance tells authors that an Exhibit bundles every file and uses no network, and the Package UI frame rule is that Package-facing web content loads no network resource. Content frames do not fully meet that rule. The Shell's content policy (ADR 0034) sets `connect-src teamarness-content:`, which denies `fetch`, XHR and WebSocket to other schemes, and denies workers, plugins, forms and nested frames. It does not control top-level or frame navigation to a network URL, and it does not deny WebRTC. Content that an Agent writes can therefore send data to, or load a page from, a network host.

Network behaviour from content frames is not tested. The packaged-app smoke proves isolation from the Shell, the Package UI and Tauri commands (`TEAM.SYS.PKG-R17`); it makes no network claim.

## Impact

An Exhibit is less trusted than a Package UI, because an Agent writes it. A malicious or mistaken Exhibit can disclose what it contains, or what a viewer types into it, to a network host. It cannot read the Package UI, the Shell, other Packages or origin-scoped storage. Slice 1 Drafts stay on one Installation. Exhibits slice 2 brings content from other Members, which raises the impact.

## Follow-up

Tighten network denial in one place: the content policy served by `apps/teamarness-desktop/src-tauri/src/content_protocol.rs`, together with the content-frame handling around it. The planned work is:

- keep `connect-src teamarness-content:`;
- add a navigation handler that refuses content-frame navigation outside `teamarness-content:`;
- deny WebRTC in content frames.

Add a packaged-app probe that proves each denial. Close this delta when that probe passes. Decide in Exhibits slice 2 planning whether sharing content between Members waits for this work.

## Tracking

- ADR 0034, `TEAM.SYS.PKG-R17`, `context/roadmap.md` (Exhibits).
- `exhibits-package` workstream: plan `plan/20261024T222308Z_exhibits-slice-1.md`, Explicit Exclusions.
