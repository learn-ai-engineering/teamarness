# 02-system/08-observability — Observability Requirements

Prefix: `TEAM.SYS.OBS`

Observability covers stable error reporting, secret-safe diagnostics, and Package failure visibility. Structured logging, metrics, and diagnostic tooling are not yet specified.

## TEAM.SYS.OBS-R01 — Stable Error Contracts

Errors that cross the Shell bridge, the Package Process Protocol, or a Package UI contract SHALL carry a stable machine-readable code and retryability, and SHALL NOT serialize internal error text.

## TEAM.SYS.OBS-R02 — No Secrets in Diagnostics

Logs, diagnostics, and verification notes SHALL NOT contain private keys, raw secret or content-key bytes, Workspace Invitation values, plaintext Package records, or private database contents.

## TEAM.SYS.OBS-R03 — Package Failure Visibility

The Package Host SHALL expose each Package Instance's lifecycle state, including `failed` and `crash_loop`, through its snapshot so the Shell can present failures and recovery.
