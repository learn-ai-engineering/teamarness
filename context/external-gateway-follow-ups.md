# External Gateway follow-ups

**Updated:** 2026-11-19. **Status:** open, unscheduled except for the qualification gates below.

This is the repository-owned list of remaining work from External Gateway onboarding. The [roadmap](roadmap.md#external-gateway-maintenance-and-remaining-qualification) links here. MyFlow artifacts retain evidence and history, not the actionable backlog. Each item below names its owner and the condition for taking it up. None authorizes implementation, live setup, cleanup, integration or release.

## Current outcome

Don accepts the setup UI and notification placement. He reports successful **Rotate credentials → Disconnect → Reconnect**, followed by an external-agent smoke test passing all **32 Ports**: Backlog 6, Conversations 6, Documents 10, Exhibits 10. There was no smoke test between rotation and disconnect. Schemas and guides were reportedly unchanged. Main and Package Configuration share gateway/setup controls; Configuration intentionally adds advanced Tokens. The managed token row is the same Workspace credential, not another connection.

Fresh qualification passes on Host `96ff960f55fd5cfdeaf9aa0fa722b13008f1033a` and standalone Gateway `7df368f88f1fb11d48d6054dc00cb8bc64a1f5fd`: 322 Rust, 92 UI, 3 assembly tests and 17/17 real-Host cases. Fresh complete three-lane source review has no retained findings. Native and detailed manual evidence below remains incomplete; no overall Verify pass or delivery approval is implied.

## Smoke-test observations deferred by Don

Don explicitly requests capture for future attention, **not changes or investigation now**. These are external-agent reports, not independently reproduced defects.

| ID | Owner | Observation and future action |
|---|---|---|
| EG-F01 | Gateway maintenance | Back-to-back requests encountered `external_gateway.rate_limited` after about five calls. Refused writes reportedly had no effects. A rerun with eight seconds between calls passed all 32 Ports. Establish actual timing, budget and limit scope before deciding whether policy needs adjustment. Eight seconds is an observed workaround, not a specified required interval. |
| EG-F02 | Gateway error-contract maintenance | Rate-limit refusals reportedly give no wait duration. Consider retry-after guidance after inspecting the actual response and limiter. Preserve command uncertainty and no-automatic-replay rules; this report does not establish that every failed write is safe to retry. |
| EG-F03 | Gateway reliability, if recurrent | One Exhibits guide call failed before the smoke test, then every retry passed. No error was captured. Capture the safe error and timing on recurrence before attributing it to rate limiting or proposing a fix. |
| EG-F04 | Backlog event semantics | Three Ticket writes reportedly produced nine `backlog.changed` events. Capture payloads and the causal sequence before distinguishing separate domain changes from duplicate delivery. No one-event-per-write promise or confirmed defect is established. |

## Existing Refresh epic tickets

Track the existing tickets rather than creating duplicates. Ticket IDs were not supplied with the agent's report.

| Ticket | Reported state | Owner and remaining work |
|---|---|---|
| Update docs/architecture.html to match the current architecture | Medium priority, ready | Architecture documentation maintenance. Also covers two module READMEs that still describe removed Anthropic paths. Verify the current source before rewriting those descriptions. |
| Documents export escapes # in body text | Low priority, not ready | Documents Markdown maintenance. Agent reproduced it on a new Document. Determine whether escaping occurs on write or export; no cause is established. |

A separate existing documentation follow-up is to clarify that Provider guides describe the Host error wrapper, while Gateway HTTP exposes Provider code, flags and details directly under `error`. Owner: the next Gateway documentation change. This is not a request to change the HTTP contract.

## Remaining qualification

Owner: Don and the maintainer. These are incomplete checks, not confirmed source defects. The [single verification procedure](../docs/test-external-gateway-onboarding.md) owns the steps.

- **Packaged native/frame isolation:** remains required before final delivery. The previous driver hit same-name accessibility-process ambiguity; no later native pass exists. Arrange an explicitly authorized unambiguous environment or actual equivalent evidence. Do not restart unattended GUI experiments, stop unrelated apps, change window-manager settings or weaken OS security.
- **Detailed installed and external-agent checks:** unreported parts include chooser cancellation and browsing/lifecycle behavior, current-state edge cases, complete guidance cursors, stale conflicts, stream cancellation/reconciliation and actual harness idle behavior. The reported 32-Port happy-path pass does not certify every subcheck.
- **Credential denial/isolation details:** rotation, disconnect and reconnect worked as reported. Independent old-generation denial, idle stream closure, unrelated-token/Workspace isolation and cleanup-pending handling were not separately reported. Do not infer these from access working after reconnection.
- **Optional extended qualification:** clean-home, acquisition/signing/quarantine, destructive collision/update/ownership/crash scenarios remain separately deferred. They are not newly required just to begin the current-account first pass.

## Historical reliability and dependency maintenance

These observations did not recur in the final passing qualification. Passing reruns do not diagnose or erase them. Investigate on recurrence or as separately prioritized maintenance; do not weaken assertions or deadlines.

| Owner | Retained observation | Next evidence needed |
|---|---|---|
| Host test reliability | `package_ports_access.rs::cancelled_storage_waiter_retains_gate_and_generation_until_actual_work_finishes` once failed with null versus false; the same unchanged full test binary passed on one bounded repeat. | Capture the actual response and timing at failure before a durable fix. This is separate from [DELTA-022](.delta/DELTA-022-host-deadline-qualification-failure.md). |
| Gateway CLI test adapter | Complete frames and exit 0 were followed by a capture EOF timeout. A fixture-only spawn mitigation was used; cause remains unknown. | Reproduce with descriptor/ownership evidence before transport or timeout changes. |
| Desktop build tooling | One nested npm app build aborted with exit 134; unchanged repeat and later bundle checks passed. | Capture an actual recurrence before attributing cause or changing tooling. |
| Gateway HTTP test reliability | One `ConnectionReset` in the unchanged incomplete-header test at `backend/src/http.rs:640`; subsequent complete runs passed. | Capture the socket/error phase on recurrence; no speculative test relaxation. |
| Dependency maintenance | Locks unchanged. Final `npm ci` feed reports 13 toolchain advisories: 11 moderate, 2 high. Earlier feed counts differed. | Assess actual exposure and deliberate compatible updates separately. This is neither an audit pass nor evidence of a production exploit; no blind audit fix. |

## Optional test-data and profile cleanup

Owner: Don. No cleanup has been authorized or performed by the assistant.

- The external agent reports a smoke message in `extg`'s `#general` and a “Port smoke test notes” Document with a Comment and reply. Those Ports cannot delete them; remove them in the app only if desired. The scratch Exhibit is reportedly deleted and the architecture Exhibit unchanged.
- An earlier disposable app profile may retain token authority after local client-store deletion. Identify that exact context before revoking its test access or removing it. Deleting client files is not revocation. Do not remove the whole shared store or revoke unrelated access. Reuse the current recorded test profile rather than creating another one to continue qualification.

## Evidence location

Historical commands, hashes, lane reports and manual observations remain in the resolver-selected `external-gateway-onboarding` workstream. Current reports are `verify/20261119T010430Z_ui-final-{review,validation}.md`; manual results are in `verify/manual-onboarding.md`. These are supporting evidence only. Scheduling and remaining-work decisions use this file and the roadmap, not a search through MyFlow artifacts.
