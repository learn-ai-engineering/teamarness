# 0036 — Two Package admission tiers with a live Personal tier

**Status:** accepted (2026-10-25)
**Refines:** ADR 0003 (Package lifecycle owned by the Host), ADR 0026 (Host-brokered Package Ports: provider selection), `TEAM.SYS.PKG-R09` (clause "Configuration changes MAY require an application restart")

## Context

An installation-local Package Registration is a hand-written `<app-data>/packages/registrations.v1.json` that the Package Host reads only during `PackageHost::open`. Admission is one whole-Installation pass that resolves identity collisions, application overrides and the Port provider graph across every Package. The Host then fixes the result for its lifetime. Installing, rebuilding or removing a Personal Package therefore means editing a private file and restarting the application.

Teamarness has two kinds of Packages. Team Packages ship with the application and are the main product. Personal Packages are add-ons that a person installs, and later builds inside the Shell. People must be able to install, reload and remove Personal Packages without touching files or restarting, and without disturbing the Team Packages they are using.

## Decision

1. **Admission has two tiers.**
   - The **Team tier** is the bundled application registrations plus any installation registration with `overrideApplication: true`. It is admitted at Host open and is immutable for the Host's lifetime.
   - The **Personal tier** is every installation registration without `overrideApplication`. It is admitted after the Team tier and may change while the Host runs.

   The tier is a lifecycle boundary chosen as a product separation. It is not a trust verdict: both tiers keep their existing Package Trust.

2. **Team Packages never route to Personal providers.** Team graph selection considers Team providers only. A Personal Package may consume Team and Personal providers. A Personal Package that claims a Port contract a Team Package provides is refused with `port_provider_conflict`, so a Team provider is never displaced implicitly.

3. **The application writes the installation registration.** The Host crate owns an Installation Registration store that inspects, installs and removes entries. It writes the same v1 file atomically (a `0600` temporary file, fsync, rename, under a `0700` parent), under the existing file policy, and never overwrites a file that is already invalid. Inspect reads the manifest and its digests only and never executes Package code. Install commits only after a preflight with no side effects shows that the Personal tier would admit the candidate against the fixed Team graph.

4. **Personal changes are live and local.** Under one Personal tier lock, the Host does the following:
   - **Install** admits the new Package and creates its Instances.
   - **Reload** disposes the Package's Instances, re-admits its rebuilt bytes, and recreates them. The process generation continues from the previous Instances.
   - **Remove** disposes the Instances and drops the registration.

   Only the target Package and Personal consumers whose selected route changed are restarted. Team Instances, routes and mounts are untouched. Package Data Directories are kept on removal.

5. **Overrides apply on restart.** Writing or removing an override changes the Team tier, so the application writes it and the change takes effect at the next start. The Shell offers a restart.

6. **Consent stays explicit.** Only the Shell webview may install, reload, remove or write overrides. Install lists the three `trusted_native_code_v1` risks and records the same trust evidence as a hand-written file. An override requires a second, separate confirmation. Reload does not ask again, because a registration already trusts future bytes at its canonical directory.

## Rejected alternatives

- **Whole-Host reopen.** It reuses admission unchanged, but every Personal change restarts all Team Packages.
- **Incremental admission for both tiers.** Team routes would have to be re-resolved under running Team consumers, for a tier that is not meant to change live.
- **Personal providers serving Team consumers.** Team routes would depend on the mutable tier.
- **A restart prompt with no live tier.** It fails the build-and-test loop that in-app Package authoring needs.
- **A new registration store.** It would break the documented hand-written format.

## Consequences

- From this decision on, a Team Package cannot use a Port provided only by a Personal Package. Such a route reports the Port as unavailable, with a diagnostic.
- The `PortBroker` route maps become per-caller mutable for Personal callers. Team entries are inserted once and never removed. An in-flight call keeps the provider it resolved.
- Workspace reconciliation creates Instances from the Team templates and the current Personal templates.
- `TEAM.SYS.PKG-R09` no longer permits a restart requirement for Personal registrations. Overrides still require one. `contracts/packages/v1/README.md` drops "admitted on the next Host open, not live" for the Personal tier.
- A Package Backend can still write the registration file itself, through its ambient filesystem authority (`TEAM.SYS.PKG-R11`). This decision adds no boundary against trusted native code.

## Evidence

- Workstream `packagement`: scope, research and design artifacts (see Provenance). Design revision 2 records the developer review that replaced the whole-Host reopen.
- `crates/teamarness-package-host/src/catalog.rs` (file policy), `src/admission.rs` (whole-Installation admission, `finalize` side effects), `src/broker.rs` (per-caller route maps), `src/host/lifecycle.rs` (fixed Host structures, reconciliation), `src/instance.rs` (per-actor generation).
- **Weaknesses:** the per-caller broker mutability and the affected-consumer restart had not been exercised when this decision was accepted. The implementing workstream must test that Team generations and mounts are unchanged across Personal operations.

### Provenance (non-versioned)

- `~/.myflow/repositories/github.com/learn-ai-engineering/teamarness/workstreams/packagement/{scope,research,design}/`
