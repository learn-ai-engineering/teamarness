# 0046 — Package-owned managed CLI and client credential custody

**Status:** accepted (2026-11-17)
**Refines:** ADR 0007 (Package-owned native filesystem behavior), ADR 0045 (external-client delivery and credential provisioning)

## Context

The Personal External Gateway Package exposes Person-selected Workspace Ports through authenticated loopback HTTP. Its tokens share one exact-tool scope and remain independently revocable. The Host derives Person authority. Token labels are not principals or signed attribution.

An installed user needs a native CLI and portable skill without a source checkout, build, Node runtime, manual bearer copying, or project configuration. Every gateway-targeted command requires an explicit Workspace. The CLI must work even when an agent's PATH does not include a newly installed command.

The Package Manifest identifies one digest-verified native Backend artifact. Package initialization supplies its admitted digest and version, Installation identity, Workspace identity, and Data Directory continuity ID. Trusted Package Backends retain ambient native filesystem and process authority under ADR 0007. The Shell's Package UI remains sandboxed under ADR 0015.

The gateway's authorization store contains token verifiers, not bearer credentials. A CLI needs the bearer to authenticate. Storing that client credential is a separate responsibility from storing server authorization policy.

## Decision

### 1. Deliver the CLI through its owning Package

The External Gateway's admitted Apple Silicon macOS executable has separate Backend, CLI, and native-picker entry modes. Select the mode before starting its runtime. CLI/help must not enter the Host protocol or start a gateway listener. The picker uses its OS main thread. Entry modes do not confer authority.

Keep one manifest-verified artifact, with embedded help and a portable skill template. Do not disguise another executable as a UI asset or add a companion-artifact category for this delivery. The assembler and standalone Package validator must qualify the final artifact's modes.

Install an owned executable copy at `~/.local/bin/tmx`. Before publication, verify the copied bytes against the digest supplied by the running Backend's Host initialization. A changed Package path or a self-declared manifest is not substitute admission evidence. An old Backend whose source bytes no longer match must request reload rather than copy them.

The command has a private ownership receipt under `~/.local/share/teamarness/external-gateway/`. It records the owning Installation, Package identity/version, admitted digest, and installed digest. Initial setup previews the writes and obtains consent for later refresh of recognized managed bytes.

The owning Installation may automatically refresh an intact copy to a strictly newer compatible admitted Package version on activation or reload. Equal version/equal digest is a no-op. Same-version different bytes, a downgrade, another Installation taking update ownership, or an incompatible client format requires explicit app-led preview and repair. Modified or unrelated executable bytes are never silently overwritten, even by repair.

Serialize publication across processes. Journal executable/receipt replacement so recovery recognizes only the recorded old or new bytes. A copied executable does not depend on the Package Directory remaining at the same path. It does not keep a removed or stopped Package running.

The Personal Package owns this mechanism. The desktop updater neither distributes the Package nor updates its command. No remote fetching, global installer, shell-profile edit, or PATH assumption is introduced. The installed skill uses the exact safely quoted executable path.

### 2. Keep client credentials machine-local and separate from policy

Use a private client store at `~/.local/share/teamarness/external-gateway/`, not a project file, skill, Shell store, or Core database. One atomic connection record holds the Workspace ID, last-observed name, numeric loopback port, local origin association, token ID/generation, bearer, and setup operation ID.

The local origin association combines Host-supplied Installation identity and Data Directory continuity with a Package-generated origin nonce. Configure at most one local origin for each Workspace ID. A conflicting development profile is refused rather than becoming an implicit winner. Names are exact-match convenience metadata, not stable identity or authority. No default Workspace, current-directory inference, or folder registry is added.

Private client directories are owned 0700. Credential records and recovery journals are owned 0600 regular single-link files. Use bounded parsing, descriptor-relative no-follow operations, exclusive temporary creation, durable replacement, and cross-process write ownership. Do not chmod unrelated directories, follow output symlinks, or overwrite unknown/future state.

This store intentionally contains plaintext bearer material readable by the OS user. Server authorization policy remains verifier-only. Documentation must state that distinction rather than continue a blanket claim that no bearer is persisted anywhere. Credentials never enter skill content, normal output, setup summaries, or diagnostic headers. Arbitrary trusted same-user native code remains outside this confidentiality boundary.

Construct the authenticated authority internally as `http://127.0.0.1:<saved-port>`. Do not accept arbitrary credential paths, repository-provided endpoints, proxies, redirects, or Provider-supplied route destinations. Before each targeted operation, authenticate to the manifest and verify its Workspace ID. The existing HTTP contract uses separate connections; this check prevents configuration mistakes but is not cryptographic protection against a hostile same-user listener replacement.

### 3. Make setup recoverable without duplicating authority

A Package-owned setup coordinator owns preview, apply, observation, repair, rotation, and disconnect. The existing Policy module remains the only authorization owner. Setup does not edit policy storage directly, infer ownership from token labels, enable tools, or choose an actor.

Apply has an operation ID and retains ownership after UI observation is cancelled. A lost UI response or repeated apply must discover the same operation, not create another token. Preview fixes exact outputs and expected state; apply revalidates them before effects.

Setup has separate durable commit points:

1. Write a private client-side recovery journal with the proposed credential and expected old state.
2. Publish recognized executable and selected skill outputs.
3. Commit the managed verifier through Policy, with the operation association.
4. Atomically publish the Workspace connection record, then remove pending secret material after confirmed completion.

These files do not form one atomic transaction. A failure may leave installed files without configured access, or committed authorization awaiting credential publication. Report those partial results honestly.

Recovery can finish publication only while the exact committed operation/verifier remains current. An uncommitted proposal needs explicit repair and revalidation. A later revocation, rotation, or disconnect invalidates old recovery work. Recovery never recreates revoked authority or restores an old generation.

Managed setup introduces gateway configVersion 2 with a bounded managed-client association and operation marker, never a bearer. Read existing version-1 policy and preserve manual tokens, generations, selections, saved port, and desired listener state. The first managed setup durably migrates through Policy. Invalid/future state remains untouched. Running an older Backend against version 2 is unsupported, not a rollback strategy.

One managed token uses one existing token slot. Rotation replaces its generation rather than overlapping old and new credentials. This can briefly interrupt access between verifier commit and client-record publication. Existing streams terminate under the existing token-generation policy. A raw manual-token regeneration request cannot bypass managed publication; managed rotation uses the coordinator. Manual-token workflows otherwise remain unchanged.

Disconnect revokes first, then removes only the matching connection record. Local file deletion alone is not revocation. Revocation does not undo admitted Provider commands. Removing the Package does not silently delete its retained state, installed command, or skills; instruct the Person to revoke unwanted access before removal.

### 4. Install only selected, recognized skill output

Install `<selected-folder>/teamarness/SKILL.md` with a secret-free ownership receipt. Preview the canonical destination and revalidate it at apply. Replace only recognized, unmodified generated content; refuse unrelated or edited files. Interrupted skill/receipt publication uses the same exact-old/new-digest recovery rule.

Do not keep a durable registry of skill destinations or scan folders on update. The Person selects a folder again to update a skill. The thin skill uses CLI help and live Provider schemas/guides rather than embedding Package contracts or Workspace defaults. Copying the skill does not guarantee discovery, model wakeups, or session cleanup in every harness.

## Rejected alternatives

- **Independent CLI and picker artifacts:** clearer binary separation, but requires additional provenance and packaging contracts that the existing admitted executable avoids.
- **Launcher or symlink into the Package Directory:** follows in-place updates, but moving the directory breaks the command and delegates update selection to a mutable path. A managed copy makes ownership and recovery explicit.
- **Keychain:** provides OS credential storage but adds native access, signing/update behavior, and unattended-agent qualification. Private files fit the current same-user trust model without those dependencies.
- **Project pointers or automatic Workspace selection:** introduce hidden targeting state and untrusted configuration locations. Always-explicit Workspace selection and machine-local records meet the accepted workflow.
- **An apparently atomic setup:** cannot cover policy, executable, credential record, and a user-selected skill directory with one rename. Recorded progress and conditional repair preserve honest outcomes.
- **Overlapping rotation credentials:** reduces outage risk but leaves old authority usable through partial failures. One generation and explicit publication recovery favors revocation clarity.

## Consequences

Most changes stay in the standalone gateway. It gains native entry modes, managed installation, private client storage, setup recovery, policy migration, and a portable skill. Its security documentation must distinguish client credential custody from server verifier persistence.

The separate generic own-Package Configuration navigation change follows ADRs 0015, 0018, and 0037: additive mount contract 3 and private bridge 4, with target identity derived from the calling mount. It adds no gateway-specific Shell route, broader filesystem authority, or new Semantic Capability. The Package-owned picker remains a cancellable helper subscription, not a longer ordinary request.

CLI transport retains ADR 0045's upstream envelopes, shared authorization, live-only subscriptions, and no automatic replay or reconnect. A lost transmitted generic call response is conservatively uncertain; the CLI does not infer harmlessness from stale metadata. Continuous output alone does not guarantee model wakeups.

Verification must exercise crash boundaries, response loss, concurrent setup/update attempts, modified files, stale credentials, rotation/revocation, duplicate local origins, Package path moves, and the actual acquired-artifact onboarding journey. Private temporary fixtures and automated checks do not replace installed macOS and external-agent observations. Signing/quarantine failure remains a delivery blocker, not permission to disable OS protections.

This is accepted architecture intent, not implementation or verification evidence. It authorizes neither source integration, remote publication, normal-profile installation, nor implementation without an accepted Plan.

## Evidence

- `crates/teamarness-package-sdk/src/initialization.rs` exposes `PackageInfo.backend_digest`, `version`, Installation/Workspace information, and Data Directory continuity. These existing inputs support Package-owned provenance checks without a new Host capability.
- `context/02-system/05-packages/requirements.md`, especially R04–R07 and R11, defines native Backend execution, Package data ownership, shared contracts, and the ambient-authority trust model. R13–R14 preserve Person authorization and live-delivery/uncertainty semantics.
- `context/02-system/07-shell/requirements.md` R02 and R05 preserve mount-derived authority. R07 and ADR 0040 keep application-update ownership separate.
- The standalone External Gateway source inspected for this decision declares one native Backend in `teamarness.package.json`. Its `scripts/assemble-package.mjs` copies and hashes that executable, its UI, and its UI contract. Its `backend/src/config.rs` implements version-1 verifier-only state with safe durable file operations; `policy.rs` serializes cancellation-surviving authorization writes. These are existing mechanisms to extend, not evidence that the new transaction already works.
- **Weaknesses:** no implementation, product test, installed CLI, acquired-artifact signing/quarantine result, or independent architecture review establishes this design yet. The selected private-file model does not protect credentials from hostile code running as the same user. Profile clones and listener takeover are not cryptographically distinguished.

### Provenance (non-versioned)

Resolver-selected workstream `external-gateway-onboarding`: accepted Scope completion and its native/UI/CLI checkpoints, and `design/20261117T111100Z_managed-cli-and-onboarding.md`. Don selected one executable, a managed copy, and private files, then accepted the complete design and this ADR direction on 2026-11-17. H inspection revision: `48e5ff438c39fc238117ee3503ba6aac38f26799`. Standalone gateway inspection revision: `5a4ba7ba30c68c10dc3449e202348ab9809fb231`.
