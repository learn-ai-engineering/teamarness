# 02-system/08-observability — Observability Requirements

Prefix: `TEAM.SYS.OBS`

Observability covers stable error reporting, secret-safe diagnostics, and Package failure visibility. A bounded local Package Port observation seam is available for developers. A persisted telemetry store, metrics, and in-app diagnostic tooling are not specified.

## TEAM.SYS.OBS-R01 — Stable Error Contracts

Errors that cross the Shell bridge, the Package Process Protocol, or a Package UI contract SHALL carry a stable machine-readable code and retryability, and SHALL NOT serialize internal error text.

## TEAM.SYS.OBS-R02 — No Secrets in Diagnostics

Logs, diagnostics, and verification notes SHALL NOT contain private keys, raw secret or content-key bytes, Workspace Invitation values, plaintext Package records, or private database contents.

## TEAM.SYS.OBS-R03 — Package Failure Visibility

The Package Host SHALL expose each Package Instance's lifecycle state, including `failed` and `crash_loop`, through its snapshot so the Shell can present failures and recovery.

## TEAM.SYS.OBS-R04 — Local Port Diagnosis

An opt-in Host or SDK Port observer SHALL contain only process-local route IDs, contract and Port names, scoped generation, stage, stable outcome code, and uncertainty. It SHALL not contain a Message, canonical or sealed record, grant token, actor identity, invitation, key, or full request or response. A slow observer SHALL report lag without blocking routing. A missing terminal observation SHALL not imply success. Conversations MAY write these fields in developer mode to the existing private, rotated Backend stderr files. Those files SHALL not be presented as a Shell diagnostic API.
