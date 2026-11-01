# DELTA-019 — Agent Doc Exhibit findings accepted as tech debt

**Status:** open (2026-11-01). Don accepted these dispositions when he accepted Verify for Agent Doc Exhibit on 2026-11-01. They are deferred, not fixed. Owner: the follow-up named in each group.

## Divergence

The Agent Doc Exhibit workstream (ADR 0038, `TEAM.SYS.DOCS-R12`, `TEAM.SYS.PKG-R18`, Exhibits Port Contract 1.2, and the standalone Agent Package) passed Verify with the review findings below still open. None of them breaks ordinary use. Verify attempt 1 found one AC7 defect, a turn that could fail after a stored write; Verify attempt 2 confirmed it fixed.

### 1. Agent turn bounds (Agent Package)

- **R2-002.** Sometimes a batch crosses the message bound on its own, for example because the model's text sits beside its calls. Every call is then refused with `argumentBytes: 0, remainingBytes: 0`, and the refusal does not name the cause. It should carry a cause field or report the overage.
- **R2-003.** Each call's reserve is `message_bytes / 128`. Under the production bounds that is 768 bytes, which always holds the coded error that replaces an oversized result. Under very small custom bounds it may not. Set a minimum reserve when per-Agent bounds arrive.
- **SM-005.** `next()` sets the refusal code to `agent.tool_arguments_too_large` directly instead of carrying it in `Phase::Refusing`, and `tool_refused` matches the phase twice.

Follow-up: per-Agent turn bounds.

### 2. Agent code and UI tidiness (Agent Package)

- **SM-006.** The skill `Activity` is built the same way in two places in `tools.rs`.
- **SM-007.** The UI checks the view in two ways, `view !== "settings"` and `context.view === "main"`. A third view would get Chat without its header.

Follow-up: the next Agent Package workstream (scratch-space staging).

### 3. Exhibits and Documents (Host)

- **SM-004.** No test ties the `file.text` schema minimum and maximum to `MIN_TEXT_PAGE_BYTES` and `MAX_WRITE_BYTES`, and no schema test sends `maxBytes` 3 or 98305.
- **CR-003.** `document.create` checks for an archived Collection before the append. An archive that lands between the check and the append is not refused. This is a rare race.
- **SM-009.** The three external Agent cases in `tests/system/tests/agent_reply_flow.rs` repeat about 30 lines of setup each. A shared helper would remove it.

Follow-up: Host Port maintenance on `main`.

### 4. Package page scrolling (Host)

- On a Package page, the Configuration tab scrolls inside its frame, but the About tab scrolls the whole page. A Package's settings view is a sandboxed frame, so the Host cannot measure its content. To fix this, the frame runtime, which the Host owns, would report the content height over the frame bridge, and the Shell would size the frame to it. Don deferred this on 2026-11-01.

Follow-up: the roadmap follow-up "sizing a Package's settings view to its content".
