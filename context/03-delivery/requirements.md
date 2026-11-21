# 03-delivery — Delivery Requirements

Prefix: `TEAM.DEL`

Delivery covers explicit native application builds (ADR 0047), complete bundled Packages, repository checks, and protected desktop stable release/update tooling (ADR 0040). Local automated qualification is separate from hosted publication, service operation, and installed-update evidence. Developer ID signing, notarization, other native targets, public headless publication, and one-hour adoption remain deferred.

## TEAM.DEL-R01 — Desktop Bundle

The release build SHALL produce a Tauri macOS application bundle and disk image for `aarch64-apple-darwin` that contains every Package listed in `packages/bundled-packages.json`, with its Package Directory and Backend executable.

## TEAM.DEL-R02 — Repository Checks

The repository SHALL provide one command that runs the complete ordered check sequence from a fresh checkout, and a packaged-application smoke check that compares every bundled Package Directory with its staged bytes and launches every bundled Backend from the bundle.

## TEAM.DEL-R03 — Honest Distribution Claims

Documentation SHALL distinguish macOS ad-hoc signatures and updater cryptographic signatures from deferred Developer ID signing and notarization. It SHALL NOT claim hosted distribution, automatic update delivery or installed continuity from local fixtures or builds. Unobserved deployment/native gates SHALL remain visibly pending.

## TEAM.DEL-R04 — Protected Stable Publication

Stable releases SHALL use canonical `vMAJOR.MINOR.PATCH` tags agreeing with all desktop versions, from the public `learn-ai-engineering/teamarness` repository and a commit reachable from protected `main`. Native Apple Silicon checks and signed builds SHALL use pinned tools and locked dependencies. Checks and builds SHALL have read-only repository permission; only publication SHALL have write permission. Signing and publication SHALL require the protected `release` environment, with private signing material confined to the signing step.

## TEAM.DEL-R05 — Qualified Immutable Releases

Only a complete qualified draft containing the DMG, updater archive, signature and `latest.json` SHALL become public. All draft bytes SHALL be read back and qualified, including official signed-version verification and every bundled Package. Publication SHALL be serialized, strictly increase the greatest published stable version, and be the final mutation with stable/latest explicitly selected. Same-source draft reruns MAY replace draft assets; wrong-source and published overwrites SHALL fail. A matching published rerun SHALL make no mutations. CI provenance/digest records SHALL NOT be public assets.

## TEAM.DEL-R07 — Explicit Native Application Distributions

Application builds SHALL require explicit application and target selectors, accepting only desktop `aarch64-apple-darwin` or headless `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`. A non-native or unsupported selection SHALL fail before expensive work or output deletion. A selected build SHALL NOT build the other application, cross-compile, or default to build-all. The existing desktop compatibility and release paths SHALL remain independent.

Headless output SHALL be isolated by application and target, contain its executable and complete Team Package Directories from the bundled list, and run independently of cwd using the canonical executable parent for default resources. Final native Backend declarations, digests, sizes, and permissions SHALL match shipped bytes; Mac signing SHALL precede hashing and Linux SHALL NOT invoke codesign. Neither Agent nor External Gateway source, artifact, or registration SHALL be required.

Operator instructions SHALL provide a manually installed, restrictive Linux user-service example, with a 150-second service stop budget above the application's 120-second shutdown budget and mixed cgroup cleanup. Documentation SHALL distinguish native Linux syntax/runtime/service observations from Mac automation and SHALL NOT infer logout/reboot persistence from a static unit check.

## TEAM.DEL-R06 — Operations and Qualification

Maintainer instructions SHALL cover protection, anonymous access, key custody/backup/loss, immutable release reruns, manual bootstrap to a writable application location, terminal recovery and real installed A→B continuity. Hosted permissions, native frame isolation, Gatekeeper and installed continuity SHALL remain explicit human gates until observed. Local fixtures SHALL NOT establish complete updater delivery.
