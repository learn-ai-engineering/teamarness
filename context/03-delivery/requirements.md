# 03-delivery — Delivery Requirements

Prefix: `TEAM.DEL`

Delivery covers the desktop application bundle, bundled Packages, and repository verification. Code signing, notarization, update distribution, CI/CD, and release management are unscheduled; see `roadmap.md` Later (unscheduled).

## TEAM.DEL-R01 — Desktop Bundle

The release build SHALL produce a Tauri macOS application bundle and disk image for `aarch64-apple-darwin` that contains every Package listed in `packages/bundled-packages.json`, with its Package Directory and Backend executable.

## TEAM.DEL-R02 — Repository Checks

The repository SHALL provide one command that runs the complete ordered check sequence from a fresh checkout, and a packaged-application smoke check that compares every bundled Package Directory with its staged bytes and launches every bundled Backend from the bundle.

## TEAM.DEL-R03 — Honest Distribution Claims

Until code signing, notarization, and update distribution are delivered, documentation SHALL NOT describe built artifacts as signed, notarized, or automatically updated.
