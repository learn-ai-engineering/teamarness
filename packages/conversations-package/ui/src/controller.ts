import { isPackageRequestError, packageErrorCode, type TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { Availability, ConversationsContract } from "./contract.generated";
import type {
  ConversationChannel,
  ConversationMessage,
  ConversationSnapshot,
} from "./index";

/** The client every Conversations request goes through: one contract, one set of shapes. */
export type ConversationsClient = TypedPackageClient<ConversationsContract>;

// ---------------------------------------------------------------------------
// immutable presentable state
// ---------------------------------------------------------------------------

export interface ControllerState {
  readonly channels: readonly ConversationChannel[];
  readonly selectedChannelId: string | null;
  readonly selectedSlug: string | null;
  readonly selectedCanHide: boolean;
  readonly messages: readonly ConversationMessage[];
  readonly hasMore: boolean;
  readonly nextBefore: string | null;
  readonly loading: boolean;
  readonly loadingOlder: boolean;
  readonly olderError: boolean;
  readonly availability: Availability;
  readonly warning: string | null;
  readonly warnings: readonly string[];
  readonly creating: boolean;
  readonly confirmingHide: boolean;
}

export const PAGE_SIZE = 32;

/**
 * The Backend pages a Channel newest Message first; the view reads top to bottom.
 *
 * Reversing here is what puts the newest Message at the bottom of the Channel, as
 * `TEAM.SYS.CONV-R08` requires, and keeps every page the controller holds in one order.
 */
function oldestFirst(
  messages: readonly ConversationMessage[],
): readonly ConversationMessage[] {
  return [...messages].reverse();
}

// ---------------------------------------------------------------------------
// safe error classification — never exposes raw codes to the user
// ---------------------------------------------------------------------------

interface ErrorInfo {
  code: string;
  retryable: boolean;
}

/** The slug grammar the Backend's `validate_slug` enforces, mirrored so an invalid name fails without a round trip. */
const MAX_SLUG_LEN = 48;

function isValidSlug(slug: string): boolean {
  return (
    slug.length >= 1
    && slug.length <= MAX_SLUG_LEN
    && /^[a-z0-9-]*$/.test(slug)
    && !slug.startsWith("-")
    && !slug.endsWith("-")
    && !slug.includes("--")
  );
}

function classify(error: unknown): ErrorInfo {
  // The Package's own code, when it has one; otherwise the host's code and retryability.
  const packageCode = packageErrorCode(error);
  if (packageCode !== null) {
    return { code: packageCode, retryable: isPackageRequestError(error) && error.retryable };
  }
  if (typeof error === "object" && error !== null) {
    const e = error as Record<string, unknown>;
    if (typeof e.code === "string" && e.code.length > 0) {
      return { code: e.code, retryable: e.retryable === true };
    }
  }
  return { code: "unknown_error", retryable: false };
}

function errorMessage(
  operation: string,
  info: ErrorInfo,
  availability: Availability,
): string {
  // Only log structured metadata — never raw errors or codes in user text.
  console.debug("Conversations operation failed", {
    operation,
    code: info.code,
    retryable: info.retryable,
  });

  if (availability === "waiting_for_history_access") {
    return "Posting is unavailable until a current key holder shares history access.";
  }
  if (availability === "catching_up") {
    return "Posting is unavailable while Conversations catches up. Please try again.";
  }
  if (availability === "offline" && info.retryable) {
    return "You are offline. Your message is saved and will sync when a member is reachable.";
  }
  if (info.retryable) {
    return "Unable to complete the action. Please try again.";
  }
  switch (info.code) {
    case "conversations.channel_hidden":
      return "This Channel has been permanently hidden.";
    case "conversations.invalid_slug":
      return "Channel names use lowercase letters, numbers, and hyphens.";
    case "conversations.invalid_markdown":
      return "Message content is too long or unsupported.";
    case "conversations.confirmation_required":
      return "Please confirm this action before proceeding.";
    default:
      return "Something went wrong. Please try again.";
  }
}

// ---------------------------------------------------------------------------
// controller
// ---------------------------------------------------------------------------

/**
 * Owns the Conversations UI state machine. Every mutation produces a new
 * immutable snapshot and notifies the view. Per-request generations guard
 * against stale channel-switch responses; disposal guards prevent state
 * changes after unmount.
 */
export class ConversationsController {
  private _state: ControllerState;
  private generation = 0;
  private disposed = false;
  private headRequest: Promise<void> | null = null;
  private headQueued = false;
  private headRevision = 0;
  private headFailed = false;
  private readonly client: ConversationsClient;
  private readonly onChange: () => void;

  /** Sticky draft value that survives re-renders. */
  draft = "";

  constructor(client: ConversationsClient, onChange: () => void) {
    this.client = client;
    this.onChange = onChange;
    this._state = {
      channels: [],
      selectedChannelId: null,
      selectedSlug: null,
      selectedCanHide: false,
      messages: [],
      hasMore: false,
      nextBefore: null,
      loading: true,
      loadingOlder: false,
      olderError: false,
      availability: "catching_up",
      warning: null,
      warnings: [],
      creating: false,
      confirmingHide: false,
    };
  }

  /** Returns an immutable snapshot of the current state. */
  get state(): ControllerState {
    return this._state;
  }

  // ------------------------------------------------------------------
  // lifecycle
  // ------------------------------------------------------------------

  /** Fetch the initial snapshot and load the first channel's messages. */
  async initialize(): Promise<void> {
    if (this.disposed) return;
    // Fetch snapshot first, then load messages for the default channel.
    try {
      const snapshot = await this.client.request("conversations.snapshot", {});
      if (this.disposed) return;
      this.applySnapshotSync(snapshot);
    } catch {
      if (this.disposed) return;
      this._state = {
        ...this._state,
        loading: false,
        availability: "offline",
        warning: "Conversations is temporarily unavailable.",
      };
      this.notify();
      return;
    }

    const selectedId = this._state.selectedChannelId;
    const gen = this.generation;
    if (selectedId) {
      try {
        const page = await this.client.request(
          "conversations.channel.open",
          { channelId: selectedId, limit: PAGE_SIZE },
        );
        if (this.disposed || this.generation !== gen) return;
        this._state = {
          ...this._state,
          messages: oldestFirst(page.messages),
          hasMore: page.nextBefore !== undefined,
          nextBefore: page.nextBefore ?? null,
          loading: false,
        };
      } catch {
        if (this.disposed || this.generation !== gen) return;
        this._state = {
          ...this._state,
          loading: false,
          warning: "Conversation history is temporarily unavailable.",
        };
      } finally {
        if (!this.disposed && this.generation === gen) this.notify();
      }
    }
  }

  /** Apply snapshot without triggering an async channel load. Used during
   *  live subscription updates; during initialization use initialize(). */
  private applySnapshotSync(snapshot: ConversationSnapshot): void {
    const channels = snapshot.channels;
    const availability = snapshot.availability;
    const general = channels.find((c) => c.slug === "general");
    const selectedId =
      general?.id ?? channels[0]?.id ?? null;
    this._state = {
      ...this._state,
      channels,
      availability,
      warnings: snapshot.warnings ?? [],
      selectedChannelId: selectedId,
      loading: selectedId !== null && this._state.messages.length === 0,
    };
  }

  /** Dispose all pending work. Safe to call multiple times. */
  dispose(): void {
    this.disposed = true;
    ++this.generation;
  }

  // ------------------------------------------------------------------
  // actions
  // ------------------------------------------------------------------

  /**
   * Switch to a different channel. Late responses from a previous channel
   * selection are discarded via a per-request generation counter.
   */
  async selectChannel(channelId: string): Promise<void> {
    if (this.disposed) return;
    if (this._state.selectedChannelId === channelId) return;

    const gen = ++this.generation;
    this.headFailed = false;
    this._state = {
      ...this._state,
      selectedChannelId: channelId,
      messages: [],
      hasMore: false,
      nextBefore: null,
      loading: true,
      loadingOlder: false,
      olderError: false,
      warning: null,
      confirmingHide: false,
    };
    this.notify();

    try {
      const page = await this.client.request(
        "conversations.channel.open",
        { channelId, limit: PAGE_SIZE },
      );
      if (this.disposed || this.generation !== gen) return;
      this._state = {
        ...this._state,
        messages: oldestFirst(page.messages),
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
        loading: false,
      };
    } catch (error) {
      if (this.disposed || this.generation !== gen) return;
      const info = classify(error);
      this._state = {
        ...this._state,
        loading: false,
        warning: errorMessage("channel.open", info, this._state.availability),
      };
    } finally {
      if (!this.disposed && this.generation === gen) this.notify();
    }
  }

  /** Load the next page of older messages. */
  async loadOlder(): Promise<void> {
    if (this.disposed) return;
    const { hasMore, nextBefore, loadingOlder, selectedChannelId, olderError } =
      this._state;
    if (!hasMore || !nextBefore || loadingOlder || !selectedChannelId || olderError) return;
    const gen = this.generation;

    this._state = { ...this._state, loadingOlder: true, warning: null };
    this.notify();

    try {
      const page = await this.client.request(
        "conversations.channel.page",
        { channelId: selectedChannelId, before: nextBefore, limit: PAGE_SIZE },
      );
      if (this.disposed || this.generation !== gen) return;
      if (page.nextBefore === nextBefore) throw new Error("Non-advancing history cursor");
      // Deduplicate by message id. Older Messages belong above the ones already shown.
      const existing = new Set(this._state.messages.map((m) => m.id));
      const fresh = page.messages.filter((message) => {
        if (existing.has(message.id)) return false;
        existing.add(message.id);
        return true;
      });
      this._state = {
        ...this._state,
        messages: [...oldestFirst(fresh), ...this._state.messages],
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
        loadingOlder: false,
      };
    } catch {
      if (this.disposed || this.generation !== gen) return;
      this._state = {
        ...this._state,
        loadingOlder: false,
        olderError: true,
        warning: "Older history could not be loaded.",
      };
    } finally {
      if (!this.disposed && this.generation === gen) this.notify();
    }
  }

  /** A failed request is retried only by an explicit action, never by scrolling. */
  async retryOlder(): Promise<void> {
    if (!this._state.olderError || this.disposed) return;
    this._state = { ...this._state, olderError: false, warning: null };
    this.notify();
    if (this.headFailed) {
      this.headFailed = false;
      await this.reloadCurrentChannel();
    } else {
      await this.loadOlder();
    }
  }

  /** Create a new channel. */
  async createChannel(slug: string): Promise<void> {
    if (this.disposed) return;
    if (!isValidSlug(slug)) {
      // The Backend would refuse this slug; fail locally so a valid name is the only thing that can reach it.
      this._state = {
        ...this._state,
        warning: "Channel names use lowercase letters, numbers, and hyphens.",
      };
      this.notify();
      return;
    }
    this._state = { ...this._state, loading: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.channel.create", { slug });
      if (this.disposed) return;
      const snapshot = await this.client.request("conversations.snapshot", {});
      if (this.disposed) return;
      this._state = { ...this._state, loading: false, creating: false };
      this.applySnapshot(snapshot);
    } catch (error) {
      if (this.disposed) return;
      const info = classify(error);
      this._state = {
        ...this._state,
        loading: false,
        warning: errorMessage("channel.create", info, this._state.availability),
      };
      this.notify();
    }
  }

  /** Hide the selected channel permanently. */
  async hideChannel(): Promise<void> {
    if (this.disposed) return;
    const channelId = this._state.selectedChannelId;
    if (!channelId) return;
    this._state = { ...this._state, loading: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.channel.hide", {
        channelId,
        confirm: true,
      });
      if (this.disposed) return;
      const snapshot = await this.client.request("conversations.snapshot", {});
      if (this.disposed) return;
      this._state = {
        ...this._state,
        loading: false,
        confirmingHide: false,
      };
      this.applySnapshot(snapshot);
    } catch {
      if (this.disposed) return;
      this._state = {
        ...this._state,
        loading: false,
        warning:
          "Only the Channel creator can permanently hide this Channel.",
      };
      this.notify();
    }
  }

  /**
   * Post a message to the selected channel. The draft is cleared only by
   * the view after a successful acknowledgement.
   */
  async postMessage(markdown: string): Promise<void> {
    if (this.disposed) return;
    const channelId = this._state.selectedChannelId;
    if (!channelId || !markdown.trim()) return;
    const gen = this.generation;
    this._state = { ...this._state, loading: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.message.post", { channelId, markdown });
      if (this.disposed || this.generation !== gen) return;
      // A committed post can complete while a bounded head backfill continues.
      // Keep the composer responsive instead of awaiting every intervening page.
      this._state = { ...this._state, loading: false };
      void this.reloadCurrentChannel();
    } catch (error) {
      if (this.disposed || this.generation !== gen) return;
      const info = classify(error);
      this._state = {
        ...this._state,
        loading: false,
        warning: errorMessage("message.post", info, this._state.availability),
      };
    } finally {
      if (!this.disposed && this.generation === gen) this.notify();
    }
  }

  // ------------------------------------------------------------------
  // live updates
  // ------------------------------------------------------------------

  /**
   * Apply an incoming subscription snapshot. Preserves the current channel
   * selection unless the channel was removed, in which case it falls back
   * to the first available channel.
   */
  applySnapshot(snapshot: ConversationSnapshot): void | Promise<void> {
    if (this.disposed) return;
    const channels = snapshot.channels;
    const availability = snapshot.availability;
    const general = channels.find((c) => c.slug === "general");
    const selectedExists =
      this._state.selectedChannelId !== null &&
      channels.some((c) => c.id === this._state.selectedChannelId);
    const selectedId = selectedExists
      ? this._state.selectedChannelId
      : (general?.id ?? channels[0]?.id ?? null);

    if (selectedId !== this._state.selectedChannelId) {
      this._state = { ...this._state, channels, availability, warnings: snapshot.warnings ?? [] };
      if (selectedId) return this.selectChannel(selectedId);
      ++this.generation;
      this.headFailed = false;
      this._state = { ...this._state, selectedChannelId: null, messages: [], hasMore: false,
        nextBefore: null, loading: false, loadingOlder: false, olderError: false };
      this.notify();
      return;
    }

    // Same channel — reload messages to catch live delivery.
    this._state = {
      ...this._state,
      channels,
      availability,
      warnings: snapshot.warnings ?? [],
    };
    this.notify();
    // Return the promise so callers can await the reload.
    return this.reloadCurrentChannel();
  }

  private reloadCurrentChannel(): Promise<void> {
    if (!this._state.selectedChannelId || this.disposed) return Promise.resolve();
    ++this.headRevision;
    if (this.headRequest) {
      this.headQueued = true;
      return this.headRequest;
    }
    const request = this.reconcileHead();
    this.headRequest = request;
    void request.finally(() => {
      if (this.headRequest === request) this.headRequest = null;
    });
    return request;
  }

  private async reconcileHead(): Promise<void> {
    do {
      this.headQueued = false;
      const gen = this.generation;
      const revision = this.headRevision;
      const channelId = this._state.selectedChannelId;
      if (!channelId) return;
      try {
        let page = await this.client.request("conversations.channel.open", { channelId, limit: PAGE_SIZE });
        if (this.disposed || this.generation !== gen) return;
        const newest = [...oldestFirst(page.messages)];
        const known = new Set(this._state.messages.map((m) => m.id));
        const cursors = new Set<string>();
        while (newest.length > 0 && !newest.some((m) => known.has(m.id)) && page.nextBefore) {
          if (this.disposed || this.generation !== gen || revision !== this.headRevision) break;
          if (cursors.has(page.nextBefore)) throw new Error("Non-advancing history cursor");
          cursors.add(page.nextBefore);
          page = await this.client.request("conversations.channel.page", {
            channelId, before: page.nextBefore, limit: PAGE_SIZE,
          });
          if (this.disposed || this.generation !== gen) return;
          newest.unshift(...oldestFirst(page.messages));
        }
        if (revision !== this.headRevision) continue;
        const current = this._state.messages;
        const overlap = newest.findIndex((m) => known.has(m.id));
        const retained = overlap < 0 ? current : current.slice(0, current.findIndex((m) => m.id === newest[overlap]!.id));
        const ids = new Set(retained.map((m) => m.id));
        const messages = [...retained, ...newest.filter((message) => {
          if (ids.has(message.id)) return false;
          ids.add(message.id);
          return true;
        })];
        this._state = {
          ...this._state, messages,
          hasMore: current.length ? this._state.hasMore : page.nextBefore !== undefined,
          nextBefore: current.length ? this._state.nextBefore : page.nextBefore ?? null,
          olderError: this.headFailed ? false : this._state.olderError,
          warning: this.headFailed ? null : this._state.warning,
        };
        this.headFailed = false;
        this.notify();
      } catch {
        if (this.disposed || this.generation !== gen) return;
        if (revision !== this.headRevision) continue;
        this.headFailed = true;
        this._state = { ...this._state, olderError: true, warning: "Conversation history could not be refreshed." };
        this.notify();
      }
    } while (this.headQueued && !this.disposed);
  }

  // ------------------------------------------------------------------
  // UI toggles
  // ------------------------------------------------------------------

  cancelConfirm(): void {
    if (this.disposed) return;
    this._state = { ...this._state, confirmingHide: false };
    this.notify();
  }

  showConfirmHide(): void {
    if (this.disposed) return;
    this._state = { ...this._state, confirmingHide: true };
    this.notify();
  }

  toggleCreate(): void {
    if (this.disposed) return;
    this._state = { ...this._state, creating: !this._state.creating };
    this.notify();
  }

  // ------------------------------------------------------------------
  // internals
  // ------------------------------------------------------------------

  private notify(): void {
    this.onChange();
  }
}