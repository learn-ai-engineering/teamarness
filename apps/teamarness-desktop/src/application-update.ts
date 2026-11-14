import type { ApplicationUpdateBridge, ApplicationUpdateSnapshot } from "./application-update-contracts";

export interface ApplicationUpdateView {
  readonly snapshot: ApplicationUpdateSnapshot | null;
  readonly updating: boolean;
  readonly dismissed: boolean;
  readonly unavailable: boolean;
}

export class ApplicationUpdateController {
  private snapshot: ApplicationUpdateSnapshot | null = null;
  private dismissedVersion: string | null = null;
  private terminal = false;
  private requestingRestart = false;
  private unavailable = false;
  private rejectedRestart = false;
  private unlisten: (() => void) | null = null;
  private attachment = 0;

  public constructor(private readonly bridge: ApplicationUpdateBridge, private readonly render: (view: ApplicationUpdateView) => void) {}

  public async attach(): Promise<void> {
    this.detach();
    const attachment = this.attachment;
    try {
      const unlisten = await this.bridge.subscribe((snapshot) => {
        if (attachment === this.attachment) this.accept(snapshot);
      });
      if (attachment !== this.attachment) { unlisten(); return; }
      this.unlisten = unlisten;
      const snapshot = await this.bridge.status();
      if (attachment === this.attachment) this.acceptAuthoritative(snapshot);
    } catch {
      if (attachment === this.attachment) { this.unavailable = true; this.present(); }
    }
  }
  public detach(): void { this.attachment += 1; this.unlisten?.(); this.unlisten = null; }
  private accept(snapshot: ApplicationUpdateSnapshot): void {
    if (this.snapshot && snapshot.revision < this.snapshot.revision) return;
    this.snapshot = snapshot;
    this.terminal ||= snapshot.lifecycle.phase !== "running";
    this.unavailable = false;
    this.present();
  }
  private acceptAuthoritative(snapshot: ApplicationUpdateSnapshot): void {
    this.accept(snapshot);
    if (this.rejectedRestart && !this.terminal && this.snapshot?.lifecycle.phase === "running") {
      this.requestingRestart = false;
      this.rejectedRestart = false;
      this.present();
    }
  }
  private present(): void {
    this.render({ snapshot: this.snapshot, updating: this.terminal || this.requestingRestart, dismissed: this.dismissedVersion !== null && this.dismissedVersion === this.snapshot?.update.candidateVersion, unavailable: this.unavailable });
  }
  public later(): void { this.dismissedVersion = this.snapshot?.update.candidateVersion ?? null; this.present(); }
  public showReady(): void { this.dismissedVersion = null; this.present(); }
  public async check(): Promise<void> {
    if (this.terminal || this.requestingRestart) return;
    try { this.accept(await this.bridge.check()); } catch { this.unavailable = true; this.present(); }
  }
  public async restart(): Promise<void> {
    const generation = this.snapshot?.update.pendingGeneration;
    if (this.terminal || this.requestingRestart || this.snapshot?.update.state !== "ready" || !generation) return;
    this.rejectedRestart = false;
    this.requestingRestart = true;
    // This callback tears down frames and renders eagerly loaded code before invoke.
    this.present();
    try {
      this.accept(await this.bridge.restart(generation));
    } catch {
      this.rejectedRestart = true;
      try {
        const authoritative = await this.bridge.status();
        this.acceptAuthoritative(authoritative);
      } catch {
        // Unknown admission is not permission to restore ordinary controls.
        this.unavailable = true;
      }
      this.present();
    }
  }
  public async quit(): Promise<void> {
    if (this.snapshot?.lifecycle.phase !== "failed") return;
    try { await this.bridge.quit(); } catch { this.unavailable = true; this.present(); }
  }
}

function escape(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]!);
}
function button(action: string, label: string, disabled = false): string {
  return `<button type="button" data-update-action="${action}"${disabled ? " disabled" : ""}>${label}</button>`;
}

function updateStatus(view: ApplicationUpdateView): string {
  const update = view.snapshot?.update;
  if (view.unavailable) return "Application update status unavailable. Normal use is available.";
  if (!update) return "Reading application version…";
  if (update.error) return `Update check failed (${update.error}). Normal use is available.`;
  switch (update.state) {
    case "disabled": return "Updates are disabled in this build.";
    case "checking": return "Checking for an application update…";
    case "downloading": return `Downloading update: ${update.downloadedBytes.toLocaleString()} bytes.`;
    case "verifying": return "Verifying application update…";
    case "ready": return `Update ${update.candidateVersion ?? ""} is ready.`;
    case "stopped": return "Update checking has stopped.";
    case "idle": return update.revision > 0 ? "Up to date." : "Waiting for the first update check.";
  }
}

function errorKey(view: ApplicationUpdateView): string | null {
  if (view.updating) return null;
  const update = view.snapshot?.update;
  if (view.unavailable) return `unavailable:${update?.revision ?? 0}`;
  return update?.error ? `${update.revision}:${update.error}` : null;
}

/** Only actionable errors use the top row; installation always owns its recovery screen. */
export function renderApplicationUpdate(view: ApplicationUpdateView, errorDismissed = false): string {
  const { snapshot } = view;
  if (view.updating) {
    if (snapshot?.lifecycle.phase === "failed") {
      return `<section aria-labelledby="application-update-heading"><h1 id="application-update-heading">Update recovery</h1><p role="status">Application stopped: ${escape(snapshot.lifecycle.failure ?? "uncertain")}.</p><p>Quit and reinstall Teamarness from a trusted disk image. Do not retry installation in this session. If replacement succeeded but relaunch did not, open Teamarness manually.</p>${snapshot.lifecycle.previousApp ? `<p>Preserved previous application: <span>${escape(snapshot.lifecycle.previousApp)}</span></p>` : ""}${button("quit", "Quit")}</section>`;
    }
    return `<section aria-labelledby="application-update-heading"><h1 id="application-update-heading">Updating Teamarness</h1><p role="status">${escape(snapshot?.lifecycle.phase === "running" ? "Requesting restart" : snapshot?.lifecycle.phase ?? "Stopping")}…</p><p>Workspace and Package controls are unavailable. Please wait. If relaunch does not complete, open Teamarness manually.</p>${view.unavailable ? '<p role="status">Unable to read application status. Controls remain disabled until status is known.</p>' : ""}</section>`;
  }
  if (!errorKey(view) || errorDismissed) return "";
  return `<section class="application-update-error" aria-label="Application update error"><p role="status">${escape(updateStatus(view))}</p>${button("check", "Retry")}<button type="button" data-update-action="dismiss-error" aria-label="Dismiss update notification" title="Dismiss">×</button></section>`;
}

const UPDATE_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5"/><path d="M6.1 6.1A8 8 0 0 1 20 12M4 12a8 8 0 0 0 13.9 5.9"/></svg>';

/** Shell-owned disclosure, not a modal or an automatic installation action. */
export function renderApplicationUpdateFooter(view: ApplicationUpdateView, open = false): string {
  if (view.updating) return "";
  const update = view.snapshot?.update;
  const version = update?.installedVersion;
  const ready = update?.state === "ready";
  const busy = update?.state === "downloading" || update?.state === "verifying";
  const label = ready ? `Update ${update.candidateVersion ?? ""} ready. Review and restart.`
    : update?.state === "verifying" ? "Verifying application update" : "Downloading application update";
  const disclosure = `aria-expanded="${open}" aria-controls="application-update-popover"`;
  const indicator = ready || busy ? `<button type="button" class="application-update-indicator${busy ? " is-busy" : ""}" data-update-action="menu-update" ${disclosure} aria-label="${escape(label)}" title="${escape(label)}">${UPDATE_ICON}${ready ? '<span class="application-update-dot" aria-hidden="true"></span>' : ""}</button>` : "";
  const canCheck = view.unavailable || !update || !["disabled", "stopped", "checking", "downloading", "verifying"].includes(update.state);
  const content = ready
    ? `<p role="status">${escape(updateStatus(view))}</p>${update.notes ? `<p class="application-update-notes">${escape(update.notes)}</p>` : ""}<p class="application-update-hint">Save your work before restarting. Unsaved Package edits may be lost.</p><div class="application-update-actions">${button("restart", "Restart now")}${button("later", "Later")}</div>`
    : `<p role="status">${escape(updateStatus(view))}</p>${button("check", "Check now", !canCheck)}`;
  return `${indicator}<button type="button" class="application-version" data-update-action="menu-version" ${disclosure} aria-label="${escape(version ? `Teamarness version ${version}. Application updates` : "Application version and updates")}" title="Application version and updates">${version ? `v${escape(version)}` : "Version"}</button>${open ? `<section id="application-update-popover" class="application-update-popover" aria-label="Application updates"><p class="application-update-version">Teamarness${version ? ` v${escape(version)}` : ""}</p>${content}</section>` : ""}`;
}

export class BrowserApplicationUpdateView {
  private updating = false;
  private view: ApplicationUpdateView | null = null;
  private actions: ApplicationUpdateController | null = null;
  private footer: HTMLElement | null = null;
  private footerMarkup = "";
  private menuOpen = false;
  private menuOrigin = "menu-version";
  private dismissedError: string | null = null;
  private readonly document: Document | undefined;
  private readonly outside = (event: Event): void => {
    if (this.menuOpen && event.target instanceof Node && !this.footer?.contains(event.target)) this.closeMenu(false);
  };
  private readonly keyboard = (event: KeyboardEvent): void => {
    if (this.menuOpen && event.key === "Escape") { event.preventDefault(); this.closeMenu(true); }
  };

  public constructor(private readonly notice: HTMLElement, private readonly terminal: HTMLElement, private readonly fence: (blocked: boolean) => void) {
    this.document = notice.ownerDocument;
    this.bindActions(notice);
    this.bindActions(terminal);
    this.document?.addEventListener("pointerdown", this.outside);
    this.document?.addEventListener("focusin", this.outside);
    this.document?.addEventListener("keydown", this.keyboard);
  }

  /** The Shell calls this again when rebuilding its rail, without losing update state. */
  public mountFooter(root: HTMLElement): void {
    if (this.footer === root) return;
    const restore = this.menuOpen && this.document?.activeElement === this.document?.body;
    this.footer = root;
    this.footerMarkup = "";
    this.bindActions(root);
    this.renderFooter();
    if (restore) this.focusPrimary();
  }

  public render(view: ApplicationUpdateView, actions: ApplicationUpdateController): void {
    const entering = view.updating && !this.updating;
    this.view = view;
    this.actions = actions;
    this.updating = view.updating;
    if (view.updating) this.menuOpen = false;
    this.fence(view.updating);
    const html = renderApplicationUpdate(view, errorKey(view) === this.dismissedError);
    this.notice.hidden = view.updating || !html;
    this.notice.innerHTML = view.updating ? "" : html;
    this.terminal.hidden = !view.updating;
    this.terminal.innerHTML = view.updating ? html : "";
    this.renderFooter();
    if (entering) this.terminal.focus();
  }

  public dispose(): void {
    this.document?.removeEventListener("pointerdown", this.outside);
    this.document?.removeEventListener("focusin", this.outside);
    this.document?.removeEventListener("keydown", this.keyboard);
    this.view = null;
    this.actions = null;
  }

  private bindActions(root: HTMLElement): void {
    root.addEventListener?.("click", (event) => {
      if (!(event.target instanceof Element)) return;
      const target = event.target.closest<HTMLButtonElement>("[data-update-action]");
      if (target && root.contains(target) && !target.disabled) this.act(target.dataset.updateAction ?? "");
    });
  }

  private act(action: string): void {
    if (!this.view || !this.actions || (this.view.updating && action !== "quit")) return;
    switch (action) {
      case "menu-version":
      case "menu-update": {
        this.menuOpen = !this.menuOpen || this.menuOrigin !== action;
        this.menuOrigin = action;
        if (this.menuOpen && this.view.snapshot?.update.state === "ready") this.actions.showReady();
        this.renderFooter();
        if (this.menuOpen) this.focusPrimary(); else this.focusAction(this.menuOrigin);
        break;
      }
      case "later": this.menuOpen = false; this.actions.later(); this.focusAction("menu-update"); break;
      case "check": void this.actions.check(); break;
      case "restart": void this.actions.restart(); break;
      case "quit": void this.actions.quit(); break;
      case "dismiss-error": {
        this.dismissedError = errorKey(this.view);
        this.render(this.view, this.actions);
        this.focusAction("menu-version");
        break;
      }
    }
  }

  private closeMenu(restoreFocus: boolean): void {
    this.menuOpen = false;
    this.renderFooter();
    if (restoreFocus) this.focusAction(this.menuOrigin);
  }

  private focusAction(action: string): void {
    const target = this.footer?.querySelector<HTMLButtonElement>(`[data-update-action="${action}"]`);
    if (target && !target.disabled) target.focus();
    else this.footer?.querySelector<HTMLButtonElement>('[data-update-action="menu-version"]')?.focus();
  }

  private focusPrimary(): void {
    this.focusAction(this.view?.snapshot?.update.state === "ready" ? "restart" : "check");
  }

  private renderFooter(): void {
    if (!this.footer || !this.view) return;
    this.footer.hidden = this.view.updating;
    const html = renderApplicationUpdateFooter(this.view, this.menuOpen);
    if (html === this.footerMarkup) return;
    const active = this.document?.activeElement as HTMLElement | null | undefined;
    const action = active && this.footer.contains(active) ? active.dataset.updateAction : null;
    this.footer.innerHTML = html;
    this.footerMarkup = html;
    if (action) this.focusAction(action);
  }
}
