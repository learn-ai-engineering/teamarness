import type {
  PackageAuthoringEntry,
  PackageHostSnapshot,
  PackageInstanceKey,
  PackageManagementBridge,
  PackageUiView,
} from "./package-contracts";
import {
  isTeamIdentityEntry,
  packageRefusalMessage,
  pendingRestartChange,
  pendingRestartCopy,
  REMOVE_KEEPS_DATA_COPY,
  teamEntryRemoveQuestion,
  type PackageManagementController,
  type RegistrationState,
} from "./package-install";
import { lifecycleDescriptor } from "./presentation";

/** The Package a Package page shows, in the Workspace it was opened from. */
export type PackagePageTarget = Readonly<{ workspaceId: string; packageIdentity: string }>;

/** Every control a Package page can offer. Each page offers only the ones its Package allows. */
export type PackagePageAction =
  | "reload" | "restart" | "pause" | "resume" | "start" | "remove" | "confirm-remove" | "cancel-remove"
  | "recover" | "dispose" | "restart-app" | "retry-catalog" | "copy-homepage" | "open-settings";

/** The Instance lifecycle commands; each re-reads the snapshot and throws when the operation failed. */
export type PackagePageLifecycle = Readonly<{
  suspend(instance: PackageInstanceKey): Promise<void>;
  recover(instance: PackageInstanceKey): Promise<void>;
  dispose(instance: PackageInstanceKey): Promise<void>;
}>;

type PackagePageHooks = Readonly<{
  /** Re-renders the page. */
  changed(): void;
  /** Reads the Package Host snapshot again; the Shell renders it. */
  refresh(): Promise<void>;
  /** The Package was removed from this device, so its page has nothing left to show. */
  removed(target: PackagePageTarget): void;
  /** Copies text to the clipboard. */
  copy(value: string): Promise<void>;
  /** Opens one view of this Package's UI in the page's Workspace. */
  openView(target: PackagePageTarget, view: PackageUiView): void;
}>;

type CatalogState =
  | Readonly<{ kind: "loading" }>
  | Readonly<{ kind: "loaded"; entries: readonly PackageAuthoringEntry[] }>
  | Readonly<{ kind: "failed" }>;

type PageFeedback = Readonly<{ message: string; failed: boolean }>;

/** What a Package page shows, built from the snapshot alone. */
export type PackagePagePresentation = Readonly<{
  identity: string;
  name: string;
  version: string | null;
  description: string | null;
  author: string | null;
  homepage: string | null;
  sourceLabel: string;
  trustLabel: string | null;
  personal: boolean;
  override: boolean;
  /** The question before removing this Team Package identity's file entry, when it has one to remove. */
  teamEntryRemoveQuestion: string | null;
  /** What waits for a restart, when the registration file changed while Teamarness ran. */
  pending: string | null;
  /** The registered Package Directory, for Personal Packages only. */
  directory: string | null;
  /** Why the registration could not start, when it was refused. */
  refusal: string | null;
  instance: Readonly<{ availability: string; generation: number | null }> | null;
  diagnostics: readonly string[];
  controls: readonly PackagePageAction[];
  /** Whether the Package declares a settings view and runs in this Workspace, so it can be opened. */
  settings: boolean;
}>;

/**
 * Owns one open Package page: its catalog read, its lifecycle actions, and
 * their results. Reload and Remove run through the Package management
 * controller, so the Workspace page and the Package page share one result.
 */
export class PackagePageController {
  private target: PackagePageTarget | null = null;
  private catalog: CatalogState = { kind: "loading" };
  private catalogRevision: number | null = null;
  private catalogRequest = 0;
  private working: PackagePageAction | null = null;
  private feedback: PageFeedback | null = null;
  private removed = false;
  private focusRequested = false;

  public constructor(
    private readonly bridge: Pick<PackageManagementBridge, "authoringCatalog" | "restartPackage">,
    private readonly lifecycle: PackagePageLifecycle,
    private readonly management: PackageManagementController,
    private readonly hooks: PackagePageHooks,
  ) {}

  public current(): PackagePageTarget | null { return this.target; }

  /** Opens the page for one Package and reads its declared capabilities and Ports. */
  public open(target: PackagePageTarget, snapshot: PackageHostSnapshot | null): void {
    this.target = target;
    this.feedback = null;
    this.working = null;
    this.removed = false;
    this.focusRequested = true;
    this.management.dismissFeedback();
    void this.loadCatalog(snapshot?.revision ?? null);
  }

  /** Leaves the page. A removal result stays for the Workspace page to show. */
  public close(): void {
    if (!this.target) return;
    if (!this.removed) this.management.dismissFeedback();
    this.target = null;
    this.removed = false;
  }

  /** Reads the catalog again when the Host revision moved on. */
  public snapshotChanged(snapshot: PackageHostSnapshot): void {
    if (!this.target || this.catalogRevision === snapshot.revision) return;
    void this.loadCatalog(snapshot.revision);
  }

  /** Whether the page opened since the last call, so focus moves to its heading. */
  public takeFocusRequest(): boolean {
    const requested = this.focusRequested;
    this.focusRequested = false;
    return requested;
  }

  public async action(action: string | undefined, snapshot: PackageHostSnapshot | null): Promise<void> {
    const target = this.target;
    if (!target || this.working) return;
    const presentation = buildPackagePagePresentation(snapshot, target);
    if (!presentation) return;
    const identity = target.packageIdentity;
    const instance: PackageInstanceKey = { packageIdentity: identity, workspaceId: target.workspaceId };
    const allowed = (control: PackagePageAction): boolean => presentation.controls.includes(control);
    switch (action) {
      case "retry-catalog":
        await this.loadCatalog(snapshot?.revision ?? null);
        return;
      case "open-settings":
        if (presentation.settings) this.hooks.openView(target, "settings");
        return;
      case "copy-homepage":
        if (!presentation.homepage) return;
        await this.run(action, () => this.hooks.copy(presentation.homepage!), "Homepage copied.", "Could not copy the homepage.", false);
        return;
      case "reload":
        if (!allowed("reload")) return;
        this.feedback = null;
        await this.management.registrationAction("reload", identity);
        return;
      case "remove":
      case "cancel-remove":
        if (!allowed("remove")) return;
        this.feedback = null;
        await this.management.registrationAction(action, identity);
        return;
      case "confirm-remove": {
        if (!allowed("remove")) return;
        await this.management.registrationAction("confirm-remove", identity);
        const feedback = this.management.registrationState(identity).feedback;
        if (this.target === target && feedback && !feedback.failed && !feedback.restart) {
          this.removed = true;
          this.hooks.removed(target);
        }
        return;
      }
      case "restart-app":
        await this.management.registrationAction("restart", identity);
        return;
      case "restart":
        if (!allowed("restart")) return;
        await this.run(action, async () => {
          try {
            await this.bridge.restartPackage(instance);
          } finally {
            await this.refreshQuietly();
          }
        }, `Restarted ${presentation.name} in this Workspace.`, `Could not restart ${presentation.name}. Try again.`);
        return;
      case "pause":
        if (allowed("pause")) {
          await this.run(action, () => this.lifecycle.suspend(instance), `Paused ${presentation.name}.`, `Could not pause ${presentation.name}. Try again.`);
        }
        return;
      case "resume":
      case "start":
        if (allowed(action)) {
          await this.run(
            action,
            () => this.lifecycle.recover(instance),
            `${action === "start" ? "Started" : "Resumed"} ${presentation.name}.`,
            `Could not ${action} ${presentation.name}. Try again.`,
          );
        }
        return;
      case "recover":
        if (allowed("recover")) {
          await this.run(action, () => this.lifecycle.recover(instance), `Recovered ${presentation.name}.`, `Could not recover ${presentation.name}. Try again.`);
        }
        return;
      case "dispose":
        if (allowed("dispose")) {
          await this.run(action, () => this.lifecycle.dispose(instance), `Disposed of ${presentation.name}.`, `Could not dispose of ${presentation.name}. Try again.`);
        }
        return;
    }
  }

  public render(snapshot: PackageHostSnapshot | null): string {
    const target = this.target;
    if (!target) return "";
    const presentation = buildPackagePagePresentation(snapshot, target);
    if (!presentation) return renderMissing(snapshot === null);
    return renderPackagePage(presentation, {
      catalog: this.catalog,
      working: this.working,
      feedback: this.feedback,
      registration: this.management.registrationState(target.packageIdentity),
      packageNames: new Map(snapshot?.packages.map(({ identity, displayName }) => [identity, displayName]) ?? []),
    });
  }

  private async run(
    action: PackagePageAction,
    operation: () => Promise<void>,
    success: string,
    failure: string,
    showWorking = true,
  ): Promise<void> {
    this.working = action;
    this.feedback = null;
    if (showWorking) this.hooks.changed();
    let feedback: PageFeedback;
    try {
      await operation();
      feedback = { message: success, failed: false };
    } catch (error: unknown) {
      feedback = { message: failureMessage(error, failure), failed: true };
    }
    this.working = null;
    this.feedback = feedback;
    this.hooks.changed();
  }

  private async loadCatalog(revision: number | null): Promise<void> {
    const request = ++this.catalogRequest;
    this.catalogRevision = revision;
    if (this.catalog.kind === "failed") {
      this.catalog = { kind: "loading" };
      this.hooks.changed();
    }
    try {
      const entries = await this.bridge.authoringCatalog();
      if (request !== this.catalogRequest) return;
      this.catalog = { kind: "loaded", entries };
    } catch {
      if (request !== this.catalogRequest) return;
      this.catalog = { kind: "failed" };
      this.catalogRevision = null;
    }
    this.hooks.changed();
  }

  private async refreshQuietly(): Promise<void> {
    try {
      await this.hooks.refresh();
    } catch {
      // The Shell owns snapshot failure presentation.
    }
  }
}

/** What the page shows for one Package, or `null` when this device has no such Package. */
export function buildPackagePagePresentation(
  snapshot: PackageHostSnapshot | null,
  target: PackagePageTarget,
): PackagePagePresentation | null {
  if (!snapshot) return null;
  const identity = target.packageIdentity;
  const entry = snapshot.packages.find((value) => value.identity === identity);
  const registration = snapshot.registrations.find((value) => value.expectedIdentity === identity);
  if (!entry && !registration) return null;
  const override = registration?.override === true;
  const teamEntry = registration !== undefined && isTeamIdentityEntry(registration);
  const pending = registration ? pendingRestartChange(registration) : null;
  const personal = registration !== undefined && registration.tier === "personal" && !teamEntry;
  const name = entry?.displayName ?? identity;
  const instance = snapshot.instances.find(({ key }) =>
    key.packageIdentity === identity && key.workspaceId === target.workspaceId);
  const descriptor = instance ? lifecycleDescriptor(instance.lifecycle) : null;

  const controls: PackagePageAction[] = [];
  if (personal) {
    controls.push("reload");
    switch (instance?.lifecycle) {
      case "active":
        controls.push("pause", "restart");
        break;
      case "failed":
      case "crash_loop":
        controls.push("restart");
        break;
      case "suspended":
        controls.push("resume");
        break;
      case "admitted":
        controls.push("start");
        break;
      default:
        break;
    }
    controls.push("remove");
  } else {
    if (descriptor?.canRecover) {
      controls.push("recover");
      if (descriptor.canDispose) controls.push("dispose");
    }
    if (pending) controls.push("restart-app");
    // A removed entry is already gone from the file.
    if (teamEntry && pending !== "removed") controls.push("remove");
  }

  const diagnostics = [...new Set(snapshot.diagnostics
    .filter((diagnostic) => diagnostic.affectedPackage === identity
      && (diagnostic.affectedInstance === null || diagnostic.affectedInstance.workspaceId === target.workspaceId))
    .map(({ code }) => packageRefusalMessage(code)))];

  return {
    identity,
    name,
    version: entry?.version ?? null,
    description: entry?.description ?? null,
    author: entry?.author ?? null,
    homepage: entry?.homepage ?? null,
    sourceLabel: override ? "Team, replaced on this device by an override" : personal ? "Personal" : "Team",
    trustLabel: entry ? TRUST_LABELS[entry.trust] : null,
    personal,
    override,
    teamEntryRemoveQuestion: teamEntry && pending !== "removed" ? teamEntryRemoveQuestion(registration, name) : null,
    pending: pending ? pendingRestartCopy(pending, name) : null,
    directory: personal ? registration.directory : null,
    refusal: registration?.status.kind === "refused" ? packageRefusalMessage(registration.status.code) : null,
    instance: instance && descriptor
      ? { availability: descriptor.availability, generation: instance.processGeneration }
      : null,
    diagnostics,
    controls,
    settings: instance !== undefined && (entry?.views ?? []).includes("settings"),
  };
}

const TRUST_LABELS: Readonly<Record<PackageHostSnapshot["packages"][number]["trust"], string>> = {
  application_bundled: "Bundled with Teamarness",
  trusted_native_code_v1: "Native code you trusted on this device. Teamarness does not sandbox it.",
};

const ACTION_LABELS: Readonly<Record<PackagePageAction, string>> = {
  reload: "Reload",
  restart: "Restart",
  pause: "Pause",
  resume: "Resume",
  start: "Start",
  remove: "Remove",
  "confirm-remove": "Remove",
  "cancel-remove": "Keep it",
  recover: "Recover Backend",
  dispose: "Dispose",
  "restart-app": "Restart Teamarness",
  "retry-catalog": "Retry",
  "copy-homepage": "Copy",
  "open-settings": "Open settings",
};

type RenderState = Readonly<{
  catalog: CatalogState;
  working: PackagePageAction | null;
  feedback: PageFeedback | null;
  registration: RegistrationState;
  packageNames: ReadonlyMap<string, string>;
}>;

function renderPackagePage(presentation: PackagePagePresentation, state: RenderState): string {
  const busy = state.working !== null || state.registration.working;
  const button = (action: PackagePageAction, secondary = false, label = ACTION_LABELS[action]): string =>
    `<button type="button"${secondary ? ' class="secondary"' : ""} data-package-page-action="${action}" aria-label="${escape(`${label} ${presentation.name}`)}"${busy ? " disabled" : ""}>${escape(label)}</button>`;
  const row = (term: string, value: string | null): string =>
    value === null ? "" : `<div><dt>${escape(term)}</dt><dd>${escape(value)}</dd></div>`;
  const homepage = presentation.homepage === null
    ? ""
    : `<div><dt>Homepage</dt><dd class="package-page-copyable"><input readonly aria-label="Homepage of ${escape(presentation.name)}" value="${escape(presentation.homepage)}">${button("copy-homepage", true)}</dd></div>`;
  const about = `<section class="screen-section settings-panel" aria-labelledby="package-page-about-heading"><h2 id="package-page-about-heading">About</h2><dl>${[
    row("Name", presentation.name),
    row("Package Identity", presentation.identity),
    row("Version", presentation.version),
    row("Description", presentation.description),
    row("Author", presentation.author),
  ].join("")}${homepage}${[
    row("Source", presentation.sourceLabel),
    row("Trust", presentation.trustLabel),
    row("Package folder", presentation.directory),
  ].join("")}</dl></section>`;

  const instance = presentation.instance
    ? `<dl>${row("Availability", presentation.instance.availability)}${row("Process generation", presentation.instance.generation === null ? null : String(presentation.instance.generation))}</dl>`
    : '<p class="empty-state">This Package is not running in this Workspace.</p>';
  const refusal = presentation.refusal
    ? `<p class="error">It could not start: ${escape(presentation.refusal)}</p>`
    : "";
  const diagnostics = presentation.diagnostics.length === 0
    ? ""
    : `<h3>Diagnostics</h3><ul class="package-page-diagnostics">${presentation.diagnostics.map((message) => `<li>${escape(message)}</li>`).join("")}</ul>`;
  const status = `<section class="screen-section" aria-labelledby="package-page-status-heading"><h2 id="package-page-status-heading">In this Workspace</h2>${refusal}${instance}${diagnostics}${renderControls(presentation, state, button)}</section>`;

  return `<section class="shell-screen package-page" aria-labelledby="package-page-heading"><header class="screen-heading"><button type="button" class="package-page-back secondary" data-package-page-back>Back to Workspace</button><p class="eyebrow">${escape(presentation.personal ? "Personal Package" : "Team Package")}</p><h1 id="package-page-heading" tabindex="-1" data-package-page-focus>${escape(presentation.name)}</h1>${presentation.description === null ? "" : `<p>${escape(presentation.description)}</p>`}${renderSettingsEntry(presentation, button)}</header>
    ${about}${status}${renderCatalog(presentation, state, button)}</section>`;
}

/** **Open settings**, offered only when the Package declares a settings view and runs in this Workspace. */
function renderSettingsEntry(
  presentation: PackagePagePresentation,
  button: (action: PackagePageAction, secondary?: boolean, label?: string) => string,
): string {
  return presentation.settings ? `<div class="package-instance-actions">${button("open-settings")}</div>` : "";
}

function renderControls(
  presentation: PackagePagePresentation,
  state: RenderState,
  button: (action: PackagePageAction, secondary?: boolean, label?: string) => string,
): string {
  const registration = state.registration;
  const feedback = state.feedback ?? registration.feedback;
  // A page that waits for a restart already offers Restart Teamarness.
  const restartOffered = presentation.controls.includes("restart-app");
  const feedbackHtml = feedback
    ? `<div class="${feedback.failed ? "package-lifecycle-feedback" : "package-action-feedback"}" role="${feedback.failed ? "alert" : "status"}"><p>${escape(feedback.message)}</p>${registration.feedback?.restart && !state.feedback && !restartOffered ? button("restart-app") : ""}</div>`
    : "";
  const pendingHtml = presentation.pending
    ? `<p class="package-install-note" role="status">${escape(presentation.pending)}</p>`
    : "";
  const working = state.working !== null || registration.working
    ? '<p role="status" aria-live="polite">Working…</p>'
    : "";
  const controls = presentation.controls;
  if (controls.length === 0) return `${pendingHtml}${working}${feedbackHtml}`;
  const removeLabel = presentation.override ? "Remove override" : "Remove";
  if (registration.confirming && controls.includes("remove")) {
    const question = presentation.teamEntryRemoveQuestion
      ?? `Remove ${presentation.name} from this device? It stops in all your Workspaces. ${REMOVE_KEEPS_DATA_COPY}`;
    return `${pendingHtml}${working}${feedbackHtml}<div class="package-remove-confirmation" role="group" aria-label="${escape(`Confirm removing ${presentation.name}`)}"><p>${escape(question)}</p><div class="package-instance-actions">${button("confirm-remove", false, removeLabel)}${button("cancel-remove", true)}</div></div>`;
  }
  const buttons = controls.map((action) =>
    action === "remove" ? button("remove", true, removeLabel) : button(action, action === "dispose")).join("");
  const hint = presentation.personal
    ? '<p class="package-page-hint">Pause asks the Package to stop its work; its process keeps running until you Resume it. Restart starts a new process. Pause, Resume, and Restart apply to this Workspace. Reload and Remove apply in all your Workspaces.</p>'
    : "";
  return `${pendingHtml}${working}${feedbackHtml}<div class="package-instance-actions">${buttons}</div>${hint}`;
}

function renderCatalog(
  presentation: PackagePagePresentation,
  state: RenderState,
  button: (action: PackagePageAction, secondary?: boolean, label?: string) => string,
): string {
  const section = (body: string): string =>
    `<section class="screen-section" aria-labelledby="package-page-ports-heading"><h2 id="package-page-ports-heading">Capabilities and Ports</h2>${body}</section>`;
  if (state.catalog.kind === "loading") return section('<p class="empty-state">Loading capabilities and Ports…</p>');
  if (state.catalog.kind === "failed") {
    return section(`<div class="package-lifecycle-feedback" role="alert"><p>Capabilities and Ports could not be read.</p>${button("retry-catalog", true)}</div>`);
  }
  const entry = state.catalog.entries.find(({ identity }) => identity === presentation.identity);
  if (!entry) return section('<p class="empty-state">Not available, because this Package is not admitted.</p>');
  const list = (items: readonly string[], empty: string): string => items.length === 0
    ? `<p class="empty-state">${escape(empty)}</p>`
    : `<ul>${items.map((item) => `<li>${item}</li>`).join("")}</ul>`;
  const range = (major: number, minMinor: number, maxMinor: number): string =>
    minMinor === maxMinor ? `${major}.${minMinor}` : `${major}.${minMinor}–${major}.${maxMinor}`;
  const capabilities = list(entry.capabilities.map(({ identity, major, minMinor, maxMinor }) =>
    `${escape(identity)} ${range(major, minMinor, maxMinor)}`), "None declared.");
  const provided = list(entry.provided.map(({ identity, major, minor, description }) =>
    `${escape(identity)} ${major}.${minor}${description ? ` — ${escape(description)}` : ""}`), "None.");
  const consumed = list(entry.consumed.map((contract) => {
    const optional = contract.required ? "" : " (optional)";
    const provider = contract.provider === null ? null : state.packageNames.get(contract.provider) ?? contract.provider;
    const availability = contract.unavailable !== null
      ? `Unavailable: ${packageRefusalMessage(contract.unavailable)}`
      : provider !== null
        ? `Available from ${provider}${contract.minor === null ? "" : `, version ${contract.major}.${contract.minor}`}`
        : "Not available.";
    return `${escape(contract.identity)} ${range(contract.major, contract.minMinor, contract.maxMinor)}${optional}<br><small>${escape(availability)}</small>`;
  }), "None.");
  return section(`<h3>Semantic Capabilities</h3>${capabilities}<h3>Provides Ports</h3>${provided}<h3>Uses Ports</h3>${consumed}`);
}

function renderMissing(loading: boolean): string {
  const message = loading
    ? "Package information is loading."
    : "This Package is no longer installed on this device.";
  return `<section class="shell-screen package-page" aria-labelledby="package-page-heading"><header class="screen-heading"><button type="button" class="package-page-back secondary" data-package-page-back>Back to Workspace</button><h1 id="package-page-heading" tabindex="-1" data-package-page-focus>Package</h1><p>${message}</p></header></section>`;
}

/** A Package Host refusal names its rule; anything else gets the action's own message. */
function failureMessage(error: unknown, fallback: string): string {
  const code = typeof error === "object" && error !== null && "code" in error && typeof error.code === "string"
    ? error.code
    : null;
  return code ? packageRefusalMessage(code) : fallback;
}

function escape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
