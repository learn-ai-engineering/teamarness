/** How long a success or information message stays before it hides itself. */
export const STATUS_MESSAGE_HIDE_MS = 5_000;

/** The timer a message slot uses, so tests can drive it without waiting. */
export type StatusMessageTimers = Readonly<{
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}>;

export const browserStatusMessageTimers: StatusMessageTimers = Object.freeze({
  setTimeout: (callback: () => void, ms: number) => setTimeout(callback, ms),
  clearTimeout: (handle: unknown) => { clearTimeout(handle as ReturnType<typeof setTimeout>); },
});

/**
 * One result to show. A failed result is an error; anything else is a success or information
 * message. A persistent message carries the only control for a follow-up the Person still needs,
 * such as Restart, so it stays until it is dismissed or replaced (`TEAM.SYS.SHELL-R08`).
 */
export type StatusMessage = Readonly<{ message: string; failed: boolean; persistent?: boolean }>;

/**
 * Holds the one message a screen shows. A success hides itself after
 * {@link STATUS_MESSAGE_HIDE_MS}; an error or a persistent message stays until
 * it is dismissed or replaced by the next result.
 */
export class StatusMessageSlot<T extends StatusMessage> {
  private value: T | null = null;
  private timer: unknown = null;

  public constructor(
    /** Called when a success hid itself, so the screen renders again. */
    private readonly expired: () => void,
    private readonly timers: StatusMessageTimers = browserStatusMessageTimers,
  ) {}

  public current(): T | null { return this.value; }

  public set(value: T | null): void {
    this.cancelTimer();
    this.value = value;
    if (value === null || value.failed || value.persistent === true) return;
    this.timer = this.timers.setTimeout(() => {
      this.timer = null;
      if (this.value !== value) return;
      this.value = null;
      this.expired();
    }, STATUS_MESSAGE_HIDE_MS);
  }

  public clear(): void { this.set(null); }

  private cancelTimer(): void {
    if (this.timer === null) return;
    this.timers.clearTimeout(this.timer);
    this.timer = null;
  }
}

/**
 * The message area at the top of a screen. It is always rendered and keeps
 * its height when empty, so showing or hiding a message does not move the
 * page. An error or a persistent message carries a close control with the
 * given data attribute.
 */
export function renderStatusMessageSlot(
  value: StatusMessage | null,
  dismissAttribute: string,
  extra = "",
): string {
  if (value === null) {
    return '<div class="status-message-slot"><div class="status-message" role="status" aria-live="polite"></div></div>';
  }
  return `<div class="status-message-slot">${renderStatusMessage(value, dismissAttribute, extra)}</div>`;
}

/** One notification, outside any slot. Errors are alerts; everything else is a polite status. */
export function renderStatusMessage(value: StatusMessage, dismissAttribute: string, extra = ""): string {
  const close = `<button type="button" class="status-message-dismiss" ${dismissAttribute} aria-label="Dismiss message" title="Dismiss message"><span aria-hidden="true">×</span></button>`;
  if (!value.failed) {
    return `<div class="status-message status-message--success" role="status" aria-live="polite"><p>${escape(value.message)}</p>${extra}${value.persistent === true ? close : ""}</div>`;
  }
  return `<div class="status-message status-message--error" role="alert" aria-live="assertive"><p>${escape(value.message)}</p>${extra}${close}</div>`;
}

function escape(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
