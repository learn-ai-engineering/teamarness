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
export function renderApplicationUpdate(view: ApplicationUpdateView): string {
  const { snapshot } = view;
  const button = (action: string, label: string): string => `<button type="button" data-update-action="${action}">${label}</button>`;
  if (view.updating) {
    if (snapshot?.lifecycle.phase === "failed") {
      return `<section aria-labelledby="application-update-heading"><h1 id="application-update-heading">Update recovery</h1><p role="status">Application stopped: ${escape(snapshot.lifecycle.failure ?? "uncertain")}.</p><p>Quit and reinstall Teamarness from a trusted disk image. Do not retry installation in this session. If replacement succeeded but relaunch did not, open Teamarness manually.</p>${snapshot.lifecycle.previousApp ? `<p>Preserved previous application: <span>${escape(snapshot.lifecycle.previousApp)}</span></p>` : ""}${button("quit", "Quit")}</section>`;
    }
    return `<section aria-labelledby="application-update-heading"><h1 id="application-update-heading">Updating Teamarness</h1><p role="status">${escape(snapshot?.lifecycle.phase === "running" ? "Requesting restart" : snapshot?.lifecycle.phase ?? "Stopping")}…</p><p>Workspace and Package controls are unavailable. Please wait. If relaunch does not complete, open Teamarness manually.</p>${view.unavailable ? '<p role="status">Unable to read application status. Controls remain disabled until status is known.</p>' : ""}</section>`;
  }
  if (!snapshot || snapshot.update.state === "disabled") return view.unavailable ? '<p role="status">Application update status unavailable.</p>' : "";
  const update = snapshot.update;
  if (update.state === "ready") {
    if (view.dismissed) return button("show", "Update ready");
    return `<section aria-label="Application update"><p role="status">Teamarness ${escape(update.candidateVersion ?? "")} is ready.</p><p class="application-update-notes">${escape(update.notes ?? "")}</p><p>Unsaved Package edits may be lost on restart.</p>${button("restart", "Restart now")}${button("later", "Later")}</section>`;
  }
  const status = update.error ? `Update check failed (${update.error}). Normal use is available.`
    : update.state === "downloading" ? `Downloading update: ${update.downloadedBytes} bytes (limit ${update.archiveLimit} bytes).`
    : update.state === "checking" ? "Checking for an application update…"
    : update.state === "verifying" ? "Verifying application update…" : "Teamarness application updates";
  return `<section aria-label="Application update"><p role="status">${escape(status)}</p>${button("check", "Check again")}</section>`;
}

export class BrowserApplicationUpdateView {
  private updating = false;
  public constructor(private readonly notice: HTMLElement, private readonly terminal: HTMLElement, private readonly fence: (blocked: boolean) => void) {}
  public render(view: ApplicationUpdateView, actions: ApplicationUpdateController): void {
    const entering = view.updating && !this.updating;
    this.updating = view.updating;
    this.fence(view.updating);
    this.notice.hidden = view.updating;
    this.terminal.hidden = !view.updating;
    const target = view.updating ? this.terminal : this.notice;
    target.innerHTML = renderApplicationUpdate(view);
    if (entering) this.terminal.focus();
    target.querySelectorAll<HTMLButtonElement>("[data-update-action]").forEach((button) => {
      button.addEventListener("click", () => {
        switch (button.dataset.updateAction) {
          case "restart": void actions.restart(); break;
          case "later": actions.later(); this.notice.querySelector<HTMLButtonElement>('[data-update-action="show"]')?.focus(); break;
          case "show": actions.showReady(); this.notice.querySelector<HTMLButtonElement>('[data-update-action="restart"]')?.focus(); break;
          case "check": void actions.check(); break;
          case "quit": void actions.quit(); break;
        }
      });
    });
  }
}
