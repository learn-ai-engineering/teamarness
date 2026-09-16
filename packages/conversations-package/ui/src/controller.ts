import type { PackageClient } from "@teamarness/package-ui-sdk";
import type {
  ConversationChannel,
  ConversationMessage,
  ConversationPage,
  ConversationSnapshot,
} from "./index";

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
  readonly availability: string;
  readonly warning: string | null;
  readonly warnings: readonly string[];
  readonly creating: boolean;
  readonly confirmingHide: boolean;
}

export const PAGE_SIZE = 32;

// ---------------------------------------------------------------------------
// safe error classification — never exposes raw codes to the user
// ---------------------------------------------------------------------------

interface ErrorInfo {
  code: string;
  retryable: boolean;
}

function classify(error: unknown): ErrorInfo {
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
  availability: string,
): string {
  // Only log structured metadata — never raw errors or codes in user text.
  console.debug("Conversations operation failed", {
    operation,
    code: info.code,
    retryable: info.retryable,
  });

  if (availability !== "current") {
    return "Conversations is catching up. Your message will be sent soon.";
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
  private readonly client: PackageClient;
  private readonly onChange: () => void;

  /** Sticky draft value that survives re-renders. */
  draft = "";

  constructor(client: PackageClient, onChange: () => void) {
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
      const snapshot =
        await this.client.request<ConversationSnapshot>(
          "conversations.snapshot",
          {},
        );
      this.applySnapshotSync(snapshot);
    } catch {
      this._state = {
        ...this._state,
        loading: false,
        availability: "degraded",
        warning: "Conversations is temporarily unavailable.",
      };
      this.notify();
      return;
    }

    const selectedId = this._state.selectedChannelId;
    if (selectedId) {
      try {
        const page = await this.client.request<ConversationPage>(
          "conversations.channel.open",
          { channelId: selectedId, limit: PAGE_SIZE },
        );
        if (this.disposed) return;
        this._state = {
          ...this._state,
          messages: page.messages,
          hasMore: page.nextBefore !== undefined,
          nextBefore: page.nextBefore ?? null,
          loading: false,
        };
      } catch {
        if (this.disposed) return;
        this._state = {
          ...this._state,
          loading: false,
          warning: "Conversation history is temporarily unavailable.",
        };
      } finally {
        if (!this.disposed) this.notify();
      }
    }
  }

  /** Apply snapshot without triggering an async channel load. Used during
   *  live subscription updates; during initialization use initialize(). */
  private applySnapshotSync(snapshot: ConversationSnapshot): void {
    const channels = snapshot.channels;
    const availability = snapshot.availability ?? "current";
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
    this.generation = 0;
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
    this._state = {
      ...this._state,
      selectedChannelId: channelId,
      messages: [],
      hasMore: false,
      nextBefore: null,
      loading: true,
      loadingOlder: false,
      warning: null,
      confirmingHide: false,
    };
    this.notify();

    try {
      const page = await this.client.request<ConversationPage>(
        "conversations.channel.open",
        { channelId, limit: PAGE_SIZE },
      );
      if (this.disposed || this.generation !== gen) return;
      this._state = {
        ...this._state,
        messages: page.messages,
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
    const { hasMore, nextBefore, loadingOlder, selectedChannelId } =
      this._state;
    if (!hasMore || !nextBefore || loadingOlder || !selectedChannelId) return;

    this._state = { ...this._state, loadingOlder: true, warning: null };
    this.notify();

    try {
      const page = await this.client.request<ConversationPage>(
        "conversations.channel.page",
        { channelId: selectedChannelId, before: nextBefore, limit: PAGE_SIZE },
      );
      if (this.disposed) return;
      // Deduplicate by message id.
      const existing = new Set(this._state.messages.map((m) => m.id));
      const fresh = page.messages.filter((m) => !existing.has(m.id));
      this._state = {
        ...this._state,
        messages: [...this._state.messages, ...fresh],
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
        loadingOlder: false,
      };
    } catch {
      if (this.disposed) return;
      this._state = {
        ...this._state,
        loadingOlder: false,
        warning: "Older history could not be loaded.",
      };
    } finally {
      if (!this.disposed) this.notify();
    }
  }

  /** Create a new channel. */
  async createChannel(slug: string): Promise<void> {
    if (this.disposed) return;
    this._state = { ...this._state, loading: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.channel.create", { slug });
      if (this.disposed) return;
      const snapshot =
        await this.client.request<ConversationSnapshot>(
          "conversations.snapshot",
          {},
        );
      if (this.disposed) return;
      this._state = { ...this._state, loading: false, creating: false };
      this.applySnapshot(snapshot);
    } catch {
      if (this.disposed) return;
      this._state = {
        ...this._state,
        loading: false,
        warning:
          "Channel names use lowercase letters, numbers, and hyphens.",
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
      const snapshot =
        await this.client.request<ConversationSnapshot>(
          "conversations.snapshot",
          {},
        );
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
    this._state = { ...this._state, loading: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.message.post", {
        channelId,
        markdown,
      });
      if (this.disposed) return;
      const page = await this.client.request<ConversationPage>(
        "conversations.channel.open",
        { channelId, limit: PAGE_SIZE },
      );
      if (this.disposed) return;
      this._state = {
        ...this._state,
        messages: page.messages,
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
        loading: false,
      };
    } catch (error) {
      if (this.disposed) return;
      const info = classify(error);
      this._state = {
        ...this._state,
        loading: false,
        warning: errorMessage("message.post", info, this._state.availability),
      };
    } finally {
      if (!this.disposed) this.notify();
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
    const availability = snapshot.availability ?? "current";
    const general = channels.find((c) => c.slug === "general");
    const selectedExists =
      this._state.selectedChannelId !== null &&
      channels.some((c) => c.id === this._state.selectedChannelId);
    const selectedId = selectedExists
      ? this._state.selectedChannelId
      : (general?.id ?? channels[0]?.id ?? null);

    if (selectedId !== this._state.selectedChannelId) {
      // Channel was removed — switch to fallback and reload.
      this._state = {
        ...this._state,
        channels,
        availability,
        warnings: snapshot.warnings ?? [],
        selectedChannelId: selectedId,
        messages: [],
        hasMore: false,
        nextBefore: null,
        loading: true,
        warning: null,
      };
      this.notify();
      if (selectedId) void this.selectChannel(selectedId);
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

  private async reloadCurrentChannel(): Promise<void> {
    const channelId = this._state.selectedChannelId;
    if (!channelId) return;
    try {
      const page = await this.client.request<ConversationPage>(
        "conversations.channel.open",
        { channelId, limit: PAGE_SIZE },
      );
      if (this.disposed) return;
      this._state = {
        ...this._state,
        messages: page.messages,
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
      };
    } catch {
      // Silently ignore reload failures during live updates — the
      // warning state is already managed by the store.
    } finally {
      if (!this.disposed) this.notify();
    }
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