import type {
  InspectedPackage,
  PackageHostSnapshot,
  PackageManagementBridge,
  PackageRegistration,
} from "./package-contracts";

/** The risks a person accepts for `trusted_native_code_v1`, exactly as the Host checks them. */
export const TRUSTED_NATIVE_CODE_RISKS = ["filesystem", "network", "process"] as const;

export const INSTALLED_COPY = "Installed on this device. It appears in all your Workspaces.";
export const REMOVE_KEEPS_DATA_COPY = "Its data stays on this device, so installing it again resumes where it left off.";

/** One step of the Add Personal Package flow. */
export type PackageInstallStep =
  | Readonly<{ kind: "closed" }>
  | Readonly<{ kind: "choose"; pasting: boolean; refusal: string | null }>
  | Readonly<{ kind: "working"; message: string }>
  | Readonly<{ kind: "preview"; inspected: InspectedPackage }>
  | Readonly<{ kind: "consent"; inspected: InspectedPackage }>
  | Readonly<{ kind: "override"; inspected: InspectedPackage }>
  | Readonly<{ kind: "installed"; name: string }>
  | Readonly<{ kind: "restart"; message: string; refusal: string | null }>;

/** What the person did last to one registration row, and how it went. */
export type RegistrationFeedback = Readonly<{
  identity: string;
  message: string;
  failed: boolean;
  restart: boolean;
}>;

/** The registration actions' state for one identity. */
export type RegistrationState = Readonly<{
  confirming: boolean;
  working: boolean;
  feedback: RegistrationFeedback | null;
}>;

type ManagementHooks = Readonly<{
  /** Re-renders the Packages section. */
  changed(): void;
  /** Reads the Package Host snapshot again; the Shell renders it. */
  refresh(): Promise<void>;
}>;

export type PackageInstallAction =
  | "open" | "choose" | "paste" | "continue" | "trust" | "override" | "restart" | "back" | "cancel" | "done";

export type PackageRegistrationAction = "reload" | "remove" | "confirm-remove" | "cancel-remove" | "restart";

/**
 * Owns the Add Personal Package flow and the actions on registrations that
 * need attention. It never installs before the person accepts the risks, and
 * it writes an override only after a second, separate confirmation.
 */
export class PackageManagementController {
  private step: PackageInstallStep = { kind: "closed" };
  private confirmingRemoval: string | null = null;
  private workingRegistration: string | null = null;
  private feedback: RegistrationFeedback | null = null;
  private focusRequested = false;

  public constructor(
    private readonly bridge: PackageManagementBridge,
    private readonly hooks: ManagementHooks,
  ) {}

  public state(): PackageInstallStep { return this.step; }

  /** Whether the flow changed step since the last call, so focus moves to the new step. */
  public takeFocusRequest(): boolean {
    const requested = this.focusRequested;
    this.focusRequested = false;
    return requested;
  }

  public async installAction(action: string | undefined): Promise<void> {
    const step = this.step;
    switch (action) {
      case "open":
        if (step.kind === "closed") this.go({ kind: "choose", pasting: false, refusal: null });
        return;
      case "paste":
        if (step.kind === "choose") this.go({ ...step, pasting: true });
        return;
      case "choose":
        if (step.kind === "choose") await this.chooseFolder();
        return;
      case "continue":
        if (step.kind === "preview") this.go({ kind: "consent", inspected: step.inspected });
        return;
      case "trust":
        if (step.kind !== "consent") return;
        if (step.inspected.kind === "overrides_team") this.go({ kind: "override", inspected: step.inspected });
        else await this.install(step.inspected);
        return;
      case "override":
        if (step.kind === "override") await this.writeOverride(step.inspected);
        return;
      case "restart":
        if (step.kind === "restart") await this.restart();
        return;
      case "back":
        if (step.kind === "preview" || step.kind === "consent" || step.kind === "override") {
          this.go({ kind: "choose", pasting: false, refusal: null });
        }
        return;
      case "cancel":
      case "done":
        if (step.kind !== "working") this.go({ kind: "closed" });
        return;
    }
  }

  public async inspectPath(value: string): Promise<void> {
    if (this.step.kind !== "choose") return;
    const directory = value.trim();
    if (!directory.startsWith("/")) {
      this.go({ kind: "choose", pasting: true, refusal: "Enter the full path of the Package folder, starting with /." });
      return;
    }
    await this.inspect(directory, true);
  }

  public async registrationAction(action: string | undefined, identity: string): Promise<void> {
    if (this.workingRegistration) return;
    switch (action) {
      case "remove":
        this.confirmingRemoval = identity;
        this.feedback = null;
        this.hooks.changed();
        return;
      case "cancel-remove":
        this.confirmingRemoval = null;
        this.hooks.changed();
        return;
      case "confirm-remove":
        if (this.confirmingRemoval !== identity) return;
        this.confirmingRemoval = null;
        await this.runRegistration(identity, () => this.bridge.remove(identity), (change) => change.restartRequired
          ? REMOVED_FOR_RESTART_COPY
          : `Removed. ${REMOVE_KEEPS_DATA_COPY}`);
        return;
      case "reload":
        await this.runRegistration(identity, () => this.bridge.reload(identity), () => "Reloaded.");
        return;
      case "restart":
        await this.restartApplication(identity);
        return;
    }
  }

  /** Restarts Teamarness after a removed override, from the section-level result. */
  public async restartAfterChange(): Promise<void> {
    const feedback = this.feedback;
    if (feedback?.restart) await this.restartApplication(feedback.identity);
  }

  /** Restarts Teamarness so a change that waits for a restart applies. A failure is shown for `identity`. */
  private async restartApplication(identity: string): Promise<void> {
    try {
      await this.bridge.restart();
    } catch (error: unknown) {
      this.feedback = { identity, message: refusalMessage(error), failed: true, restart: true };
      this.hooks.changed();
    }
  }

  /** The inline add flow, or nothing while it is closed. */
  public renderPanel(snapshot: PackageHostSnapshot | null): string {
    return renderInstallStep(this.step, snapshot);
  }

  /** Personal registrations that failed to admit, Team Package overrides, and entries waiting for a restart. */
  public renderRegistrations(snapshot: PackageHostSnapshot | null): string {
    const rows = attentionRegistrations(snapshot);
    const feedback = this.feedback;
    // A removed registration leaves the list, so its result is shown for the section.
    const detached = feedback && !rows.some(({ expectedIdentity }) => expectedIdentity === feedback.identity)
      ? `<div class="${feedback.failed ? "package-lifecycle-feedback" : "package-action-feedback"}" role="${feedback.failed ? "alert" : "status"}"><p>${escape(feedback.message)}</p>${feedback.restart ? '<button type="button" data-package-change-restart>Restart Teamarness</button>' : ""}</div>`
      : "";
    if (rows.length === 0) return detached;
    const items = rows.map((registration, index) => renderRegistration(
      registration,
      index,
      snapshot,
      this.registrationState(registration.expectedIdentity),
    )).join("");
    return `<section class="package-registrations" aria-labelledby="package-registrations-heading"><h3 id="package-registrations-heading">Needs attention on this device</h3>${detached}<div class="package-instance-list">${items}</div></section>`;
  }

  /** Whether Remove waits for confirmation, an action runs, and how the last one went, for one identity. */
  public registrationState(identity: string): RegistrationState {
    return {
      confirming: this.confirmingRemoval === identity,
      working: this.workingRegistration === identity,
      feedback: this.feedback?.identity === identity ? this.feedback : null,
    };
  }

  /** Forgets the last registration result and any pending Remove confirmation. */
  public dismissFeedback(): void {
    if (this.workingRegistration) return;
    this.feedback = null;
    this.confirmingRemoval = null;
  }

  /** The identity behind a rendered registration row. */
  public registrationIdentity(snapshot: PackageHostSnapshot | null, index: number): string | null {
    return attentionRegistrations(snapshot)[index]?.expectedIdentity ?? null;
  }

  private async chooseFolder(): Promise<void> {
    let directory: string | null;
    try {
      directory = await this.bridge.chooseDirectory();
    } catch (error: unknown) {
      this.go({ kind: "choose", pasting: false, refusal: refusalMessage(error) });
      return;
    }
    if (directory !== null) await this.inspect(directory, false);
  }

  private async inspect(directory: string, pasting: boolean): Promise<void> {
    this.go({ kind: "working", message: "Reading the Package folder…" });
    try {
      const inspected = await this.bridge.inspectDirectory(directory);
      this.go({ kind: "preview", inspected });
    } catch (error: unknown) {
      this.go({ kind: "choose", pasting, refusal: refusalMessage(error) });
    }
  }

  private async install(inspected: InspectedPackage): Promise<void> {
    this.go({ kind: "working", message: `Installing ${inspected.displayName}…` });
    try {
      await this.bridge.install({
        directory: inspected.directory,
        expectedIdentity: inspected.identity,
        acceptedRisks: TRUSTED_NATIVE_CODE_RISKS,
      });
    } catch (error: unknown) {
      await this.refreshQuietly();
      this.go({ kind: "choose", pasting: false, refusal: refusalMessage(error) });
      return;
    }
    await this.refreshQuietly();
    this.go({ kind: "installed", name: inspected.displayName });
  }

  private async writeOverride(inspected: InspectedPackage): Promise<void> {
    this.go({ kind: "working", message: `Saving the override for ${inspected.displayName}…` });
    try {
      await this.bridge.writeOverride({
        directory: inspected.directory,
        expectedIdentity: inspected.identity,
        acceptedRisks: TRUSTED_NATIVE_CODE_RISKS,
      });
    } catch (error: unknown) {
      this.go({ kind: "choose", pasting: false, refusal: refusalMessage(error) });
      return;
    }
    // The Host now lists the override as waiting for a restart.
    await this.refreshQuietly();
    this.go({
      kind: "restart",
      message: `The override is saved. ${inspected.displayName} from this folder replaces the Team Package after Teamarness restarts.`,
      refusal: null,
    });
  }

  private async restart(): Promise<void> {
    const step = this.step;
    if (step.kind !== "restart") return;
    try {
      await this.bridge.restart();
    } catch (error: unknown) {
      this.go({ ...step, refusal: refusalMessage(error) });
    }
  }

  private async runRegistration(
    identity: string,
    operation: () => Promise<Readonly<{ restartRequired: boolean }>>,
    success: (change: Readonly<{ restartRequired: boolean }>) => string,
  ): Promise<void> {
    this.workingRegistration = identity;
    this.feedback = null;
    this.hooks.changed();
    let feedback: RegistrationFeedback;
    try {
      const change = await operation();
      feedback = { identity, message: success(change), failed: false, restart: change.restartRequired };
    } catch (error: unknown) {
      feedback = { identity, message: refusalMessage(error), failed: true, restart: false };
    }
    // After a refusal the Host may have changed the registration's status, so read it again either way.
    await this.refreshQuietly();
    this.workingRegistration = null;
    this.feedback = feedback;
    this.hooks.changed();
  }

  private async refreshQuietly(): Promise<void> {
    try {
      await this.hooks.refresh();
    } catch {
      // The Shell owns snapshot failure presentation.
    }
  }

  private go(step: PackageInstallStep): void {
    this.step = step;
    this.focusRequested = true;
    this.hooks.changed();
  }
}

/** A readable message for a Package Host refusal. It names the rule that failed. */
export function packageRefusalMessage(code: string): string {
  return REFUSALS[code] ?? `Teamarness refused this Package (${code}).`;
}

const REFUSALS: Readonly<Record<string, string>> = {
  invalid_registration: "Choose a Package folder by its full path. A file or a link to a folder cannot be used.",
  source_not_found: "That folder does not exist.",
  manifest_malformed: "The folder's teamarness.package.json is missing, cannot be read, or breaks a manifest rule. Run teamarness-package-validate on the folder to see which rule.",
  manifest_unsupported: "This Package needs a newer version of Teamarness.",
  invalid_package_identity: "The Package Identity is not valid.",
  identity_mismatch: "The folder now holds a different Package than the preview showed. Choose the folder again.",
  duplicate_identity: "Two registrations use this Package Identity.",
  override_required: "A Team Package already uses this Package Identity. It can only be replaced by an override.",
  trust_evidence_missing: "The Package was not installed because its risks were not accepted.",
  target_unsupported: "This Package has no Backend built for this computer.",
  artifact_missing: "A file the manifest lists is missing. Rebuild the Package.",
  artifact_changed: "A file changed after the manifest was written. Rebuild the Package.",
  registration_file_invalid: "The Package registration file on this device cannot be read safely, so Teamarness will not change it. Repair or remove it, then try again.",
  not_registered: "This Package is not registered on this device.",
  port_provider_conflict: "Another Package already provides a Port contract that this Package provides.",
  port_contract_missing: "A Port contract this Package requires is not available.",
  port_missing: "A Port this Package requires is not available.",
  port_version_mismatch: "A Port this Package requires is not available in a compatible version.",
  port_dependency_cycle: "This Package's required Ports depend on each other in a cycle.",
  port_contract_violation: "This Package's Port contract is invalid. Rebuild the Package.",
  port_graph_search_limit: "Teamarness could not resolve this Package's Ports.",
  capability_unavailable: "This Package asks for a capability Teamarness does not offer.",
  capability_version_unsupported: "This Package asks for a capability version Teamarness does not offer.",
  ui_entrypoint_missing: "The Package UI entrypoint is missing. Rebuild the Package.",
  ui_contract_violation: "The Package UI contract is invalid. Rebuild the Package.",
  ui_view_not_declared: "This Package does not offer that view.",
  data_directory_unavailable: "Teamarness could not write its Package data on this device.",
  data_directory_permission: "Teamarness Package data on this device has unsafe permissions.",
  spawn_failed: "The Package Backend could not start.",
  handshake_timeout: "The Package Backend did not answer when it started.",
  handshake_mismatch: "The Package Backend speaks a protocol version Teamarness does not support.",
  shutting_down: "Teamarness is shutting down. Try again after reopening it.",
  invalid_lifecycle_state: "This Package cannot do that in its current state. Wait a moment, then try again.",
};

function refusalMessage(error: unknown): string {
  const code = typeof error === "object" && error !== null && "code" in error && typeof error.code === "string"
    ? error.code
    : null;
  return code ? packageRefusalMessage(code) : "Teamarness could not complete this. Try again.";
}

function attentionRegistrations(snapshot: PackageHostSnapshot | null): readonly PackageRegistration[] {
  return snapshot?.registrations.filter((registration) =>
    registration.override || registration.status.kind !== "admitted") ?? [];
}

/**
 * Whether a registration is a file entry for a Team Package identity: an
 * override, or an entry written by hand without one, which the Host refuses
 * with `override_required`. Such an entry changes only after a restart, so it
 * offers Remove but never Reload.
 */
export function isTeamIdentityEntry(registration: PackageRegistration): boolean {
  return registration.override
    || registration.tier === "team"
    || registration.status.kind === "pending_restart"
    || (registration.status.kind === "refused" && registration.status.code === "override_required");
}

/** The file change a registration waits to apply at the next start, if any. */
export function pendingRestartChange(registration: PackageRegistration): "written" | "removed" | null {
  return registration.status.kind === "pending_restart" ? registration.status.change : null;
}

/** What a registration waiting for a restart says. */
export function pendingRestartCopy(change: "written" | "removed", name: string): string {
  return change === "written"
    ? `Applies after restart: this override replaces the Team Package ${name} when Teamarness restarts.`
    : `Applies after restart: removed from this device. The Team Package ${name} returns when Teamarness restarts. Data is kept.`;
}

/** The question before removing a Team Package identity's file entry. */
export function teamEntryRemoveQuestion(registration: PackageRegistration, name: string): string {
  return registration.override
    ? `Remove the override? ${name} returns to the Team Package after Teamarness restarts. Data is kept.`
    : `Remove this entry? The Team Package ${name} returns after Teamarness restarts. Data is kept.`;
}

export const REMOVED_FOR_RESTART_COPY = "Removed from this device. Restart Teamarness to use the Team Package again.";

function teamPackageName(snapshot: PackageHostSnapshot | null, identity: string): string {
  return snapshot?.packages.find((entry) => entry.identity === identity)?.displayName ?? identity;
}

const RISK_COPY: Readonly<Record<(typeof TRUSTED_NATIVE_CODE_RISKS)[number], string>> = {
  filesystem: "Filesystem: read, change, and delete your files.",
  network: "Network: connect to any computer on your network or the internet.",
  process: "Process: start other programs on this device.",
};

function renderInstallStep(step: PackageInstallStep, snapshot: PackageHostSnapshot | null): string {
  const panel = (heading: string, body: string): string =>
    `<section class="package-install" aria-labelledby="package-install-heading"><h3 id="package-install-heading" tabindex="-1" data-package-install-focus>${escape(heading)}</h3>${body}</section>`;
  const button = (action: PackageInstallAction, label: string, secondary = false): string =>
    `<button type="button"${secondary ? ' class="secondary"' : ""} data-package-install-action="${action}">${escape(label)}</button>`;
  const actions = (...buttons: string[]): string => `<div class="package-instance-actions">${buttons.join("")}</div>`;
  switch (step.kind) {
    case "closed":
      return "";
    case "choose": {
      const refusal = step.refusal ? `<p class="error" role="alert">${escape(step.refusal)}</p>` : "";
      const paste = step.pasting
        ? `<form id="package-install-path-form"><label for="package-install-path">Package folder path</label><input id="package-install-path" required autocapitalize="off" autocorrect="off" spellcheck="false" placeholder="/Users/you/Packages/my-package"><button>Read folder</button></form>`
        : "";
      return panel("Add Personal Package", `<p>Choose a built Package folder. Teamarness reads it first and shows what it contains. Nothing runs until you trust it.</p>${refusal}${actions(button("choose", "Choose folder"), step.pasting ? "" : button("paste", "Paste a path", true), button("cancel", "Cancel", true))}${paste}`);
    }
    case "working":
      return panel("Add Personal Package", `<p role="status" aria-live="polite">${escape(step.message)}</p>`);
    case "preview":
      return panel(`Preview: ${step.inspected.displayName}`, `${renderPreview(step.inspected, snapshot)}${actions(button("continue", "Continue"), button("back", "Choose another folder", true), button("cancel", "Cancel", true))}`);
    case "consent": {
      const risks = TRUSTED_NATIVE_CODE_RISKS.map((risk) => `<li>${escape(RISK_COPY[risk])}</li>`).join("");
      const label = step.inspected.kind === "overrides_team" ? "Trust and continue" : "Trust and install";
      return panel(`Trust ${step.inspected.displayName}?`, `<p>${escape(step.inspected.displayName)} runs as native code on this device, with your permissions. Teamarness does not sandbox it. It can use:</p><ul class="package-risks">${risks}</ul><p>Install it only if you trust where it came from.</p>${actions(button("trust", label), button("cancel", "Cancel", true))}`);
    }
    case "override": {
      const team = teamPackageName(snapshot, step.inspected.identity);
      return panel(`Replace ${team}?`, `<p>This is a second, separate confirmation. ${escape(step.inspected.displayName)} from this folder replaces the Team Package ${escape(team)} on this device, in all your Workspaces. The change applies after Teamarness restarts. Data is kept.</p><p>To return to the Team Package later, remove the override and restart.</p>${actions(button("override", `Replace ${team} after restart`), button("cancel", "Cancel", true))}`);
    }
    case "installed":
      return panel(`${step.name} is installed`, `<p role="status">${escape(INSTALLED_COPY)}</p>${actions(button("done", "Done"))}`);
    case "restart": {
      const refusal = step.refusal ? `<p class="error" role="alert">${escape(step.refusal)}</p>` : "";
      return panel("Restart to apply the override", `<p role="status">${escape(step.message)}</p>${refusal}${actions(button("restart", "Restart Teamarness"), button("done", "Later", true))}`);
    }
  }
}

function renderPreview(inspected: InspectedPackage, snapshot: PackageHostSnapshot | null): string {
  const row = (term: string, value: string | undefined): string =>
    value === undefined ? "" : `<div><dt>${escape(term)}</dt><dd>${escape(value)}</dd></div>`;
  const list = (items: readonly string[]): string =>
    items.length === 0 ? "None" : items.join(", ");
  const capabilities = list(inspected.capabilities.map(({ identity, major }) => `${identity} ${major}`));
  const provided = list(inspected.provided.map(({ identity, major, minor }) => `${identity} ${major}.${minor}`));
  const consumed = list(inspected.consumed.map(({ identity, major, required }) =>
    `${identity} ${major}${required ? "" : " (optional)"}`));
  const team = teamPackageName(snapshot, inspected.identity);
  const notes: Readonly<Record<InspectedPackage["kind"], string>> = {
    new: "",
    reload: "This folder is already registered. Installing it again restarts it with the current build.",
    replaces_registration: "This Package is already registered from another folder. Installing replaces that registration.",
    overrides_team: `This Package has the same identity as the Team Package ${team}. Installing it replaces ${team} on this device after Teamarness restarts. You confirm that separately.`,
  };
  const note = notes[inspected.kind] ? `<p class="package-install-note">${escape(notes[inspected.kind])}</p>` : "";
  return `${note}<dl class="package-preview">${[
    row("Name", inspected.displayName),
    row("Identity", inspected.identity),
    row("Version", inspected.version),
    row("Description", inspected.description),
    row("Author", inspected.author),
    row("Homepage", inspected.homepage),
    row("Folder", inspected.directory),
    row("Capabilities", capabilities),
    row("Provides Ports", provided),
    row("Uses Ports", consumed),
  ].join("")}</dl>`;
}

function renderRegistration(
  registration: PackageRegistration,
  index: number,
  snapshot: PackageHostSnapshot | null,
  state: RegistrationState,
): string {
  const identity = registration.expectedIdentity;
  const team = isTeamIdentityEntry(registration);
  const pending = pendingRestartChange(registration);
  const name = team ? teamPackageName(snapshot, identity) : identity;
  const action = (value: PackageRegistrationAction, label: string, secondary = false): string =>
    `<button type="button"${secondary ? ' class="secondary"' : ""} data-package-registration-action="${value}" data-package-registration-index="${index}" aria-label="${escape(`${label} ${name}`)}"${state.working ? " disabled" : ""}>${escape(label)}</button>`;
  const refused = registration.status.kind === "refused"
    ? `<p class="error">${escape(packageRefusalMessage(registration.status.code))}</p>`
    : "";
  const summary = registration.override
    ? `<p>Override of a Team Package, from ${escape(registration.directory)}. Changes to an override apply after Teamarness restarts.</p>`
    : team
      ? `<p>Entry for the Team Package identity ${escape(identity)}, from ${escape(registration.directory)}, without an override. The Team Package stays unavailable until you remove it and restart Teamarness.</p>`
      : `<p>Personal Package from ${escape(registration.directory)}. It could not start.</p>`;
  const pendingNote = pending ? `<p class="package-install-note" role="status">${escape(pendingRestartCopy(pending, name))}</p>` : "";
  const working = state.working ? '<p role="status" aria-live="polite">Working…</p>' : "";
  // A row that waits for a restart has its own Restart Teamarness control.
  const feedback = state.feedback
    ? `<div class="${state.feedback.failed ? "package-lifecycle-feedback" : "package-action-feedback"}" role="${state.feedback.failed ? "alert" : "status"}"><p>${escape(state.feedback.message)}</p>${state.feedback.restart && !pending ? action("restart", "Restart Teamarness") : ""}</div>`
    : "";
  const removeLabel = registration.override ? "Remove override" : "Remove";
  const buttons = [
    pending ? action("restart", "Restart Teamarness") : "",
    team ? "" : action("reload", "Reload"),
    // A removed entry is already gone from the file.
    pending === "removed" ? "" : action("remove", removeLabel, true),
  ].join("");
  const controls = state.confirming && pending !== "removed"
    ? `<div class="package-remove-confirmation" role="group" aria-label="${escape(`Confirm removing ${name}`)}"><p>${escape(team
      ? teamEntryRemoveQuestion(registration, name)
      : `Remove ${name} from this device? ${REMOVE_KEEPS_DATA_COPY}`)}</p><div class="package-instance-actions">${action("confirm-remove", removeLabel)}${action("cancel-remove", "Keep it", true)}</div></div>`
    : `<div class="package-instance-actions">${buttons}</div>`;
  return `<article class="package-instance" aria-labelledby="package-registration-name-${index}"><div><h4 id="package-registration-name-${index}"><button type="button" class="package-instance-link" data-package-registration-page="${index}" aria-label="${escape(`About ${name}`)}">${escape(name)}</button></h4>${summary}${refused}${pendingNote}</div>${working}${feedback}${controls}</article>`;
}

function escape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
