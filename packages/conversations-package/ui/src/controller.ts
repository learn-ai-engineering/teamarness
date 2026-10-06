import { isPackageRequestError, packageErrorCode, type TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { Availability, ConversationsContract } from "./contract.generated";
import type {
  ConversationChannel,
  ConversationMessage,
  ConversationPage,
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
  readonly posting: boolean;
  readonly retryingHead: boolean;
  readonly olderError: boolean;
  readonly feedError: boolean;
  readonly resolveError: boolean;
  readonly pendingHistory: number;
  readonly unavailableHistory: number;
  readonly availability: Availability;
  readonly warning: string | null;
  readonly warnings: readonly string[];
  readonly creating: boolean;
  readonly confirmingHide: boolean;
}

export const PAGE_SIZE = 32;
const MAX_HISTORY_PAGES = 32;

interface HistoryRebuild {
  messages: ConversationMessage[];
  pending: NonNullable<ConversationPage["pending"]>[number][];
  cursor: string | null;
  targetCursor: string | null;
  exhausted: boolean;
  epoch: string;
  revision: string;
  seen: Set<string>;
}

interface ChangeFeed {
  epoch: string;
  after: string;
  status: "idle" | "pending" | "failed";
  pending: Map<string, string>;
  unavailable: Map<string, string>;
  positions: Map<string, string>;
  running: Promise<void> | null;
  dirty: boolean;
  resolveStatus: "idle" | "failed";
  failedResolveIds: Set<string>;
  resolveRunning: Promise<void> | null;
  resolveOffset: number;
  rebuild: "start" | HistoryRebuild | null;
}

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
  private headFailed = false;
  private feed: ChangeFeed | null = null;
  private removedDuringOlder: Set<string> | null = null;
  private olderSkippedInFlight: Set<string> | null = null;
  private openingChannel: number | null = null;
  private openDirty = false;
  private historyGeneration = 0;
  private historyRebuilding = false;
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
      posting: false,
      retryingHead: false,
      olderError: false,
      feedError: false,
      resolveError: false,
      pendingHistory: 0,
      unavailableHistory: 0,
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
        this.installFeed(page);
      } catch {
        if (this.disposed || this.generation !== gen) return;
        this.headFailed = true;
        this._state = {
          ...this._state,
          loading: false,
          olderError: true,
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
    this.feed = null;
    this.removedDuringOlder = null;
    ++this.historyGeneration;
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
    this.feed = null;
    this.removedDuringOlder = null;
    ++this.historyGeneration;
    this.historyRebuilding = false;
    this.openingChannel = gen;
    this.openDirty = false;
    this._state = {
      ...this._state,
      selectedChannelId: channelId,
      messages: [],
      hasMore: false,
      nextBefore: null,
      loading: true,
      loadingOlder: false,
      posting: false,
      retryingHead: false,
      olderError: false,
      feedError: false,
      resolveError: false,
      pendingHistory: 0,
      unavailableHistory: 0,
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
      this.installFeed(page);
      this.openingChannel = null;
      if (this.openDirty) await this.reconcileSignal();
    } catch (error) {
      if (this.disposed || this.generation !== gen) return;
      const info = classify(error);
      this.headFailed = true;
      this._state = {
        ...this._state,
        loading: false,
        olderError: true,
        warning: errorMessage("channel.open", info, this._state.availability),
      };
    } finally {
      if (this.openingChannel === gen) this.openingChannel = null;
      if (!this.disposed && this.generation === gen) this.notify();
    }
  }

  /** Load the next page of older messages. */
  async loadOlder(): Promise<void> {
    if (this.disposed) return;
    const { hasMore, nextBefore, loadingOlder, selectedChannelId, olderError } =
      this._state;
    if (!hasMore || !nextBefore || loadingOlder || !selectedChannelId || olderError || this.historyRebuilding) return;
    const gen = this.generation;
    const historyGen = this.historyGeneration;
    const removed = new Set<string>();
    this.removedDuringOlder = removed;

    this._state = { ...this._state, loadingOlder: true, warning: null };
    this.notify();

    try {
      let cursor = nextBefore;
      const cursors = new Set([cursor]);
      let page: ConversationPage | undefined;
      for (let count = 0; count < MAX_HISTORY_PAGES; count++) {
        this.olderSkippedInFlight = new Set();
        page = await this.client.request(
          "conversations.channel.page",
          { channelId: selectedChannelId, before: cursor, limit: PAGE_SIZE },
        );
        if (this.disposed || this.generation !== gen || this.historyGeneration !== historyGen) return;
        // A feed insert below the old cursor may have arrived after this page selected
        // its rows. Refetch the same raw range before advancing past that insert.
        const returnedBefore = page.nextBefore;
        if ([...(this.olderSkippedInFlight ?? [])].some(position =>
          !returnedBefore || position >= returnedBefore)) {
          if (count === MAX_HISTORY_PAGES - 1) throw new Error("History page changed too often");
          continue;
        }
        if (page.nextBefore && cursors.has(page.nextBefore)) throw new Error("Non-advancing history cursor");
        this.trackPending(page.pending ?? []);
        if (page.messages.length || !page.nextBefore) break;
        cursor = page.nextBefore;
        cursors.add(cursor);
        // Retain progress even when the bounded run or a subsequent request fails.
        this._state = { ...this._state, nextBefore: cursor };
        if (count === MAX_HISTORY_PAGES - 1) throw new Error("History page limit reached");
      }
      if (!page) return;
      // Deduplicate by message id. Older Messages belong above the ones already shown.
      const existing = new Set(this._state.messages.map((m) => m.id));
      const fresh = page.messages.filter((message) => {
        if (existing.has(message.id) || removed.has(message.id)) return false;
        existing.add(message.id);
        return true;
      });
      this._state = {
        ...this._state,
        messages: this.ordered([...oldestFirst(fresh), ...this._state.messages]),
        hasMore: page.nextBefore !== undefined,
        nextBefore: page.nextBefore ?? null,
        loadingOlder: false,
      };
      for (const message of fresh) {
        if (message.position !== undefined) this.feed?.positions.set(message.id, message.position);
      }
    } catch {
      if (this.disposed || this.generation !== gen || this.historyGeneration !== historyGen) return;
      this._state = {
        ...this._state,
        loadingOlder: false,
        olderError: true,
        warning: "Older history could not be loaded.",
      };
    } finally {
      if (this.removedDuringOlder === removed) {
        this.removedDuringOlder = null;
        this.olderSkippedInFlight = null;
      }
      if (!this.disposed && this.generation === gen) this.notify();
    }
  }

  /** Close the gap between initial page selection and live subscription installation. */
  async catchUpAfterSubscribe(): Promise<void> { if (this.feed) await this.refreshFeed(false); }

  /** Change replay and non-ready row resolution have independent retry ownership. */
  async retryFeed(): Promise<void> { await this.refreshFeed(true); }
  async retryPending(): Promise<void> { await this.resolvePending(true); }

  /** A failed request is retried only by an explicit action, never by scrolling. */
  async retryOlder(): Promise<void> {
    if (this.disposed) return;
    if (!this._state.olderError || this.historyRebuilding) return;
    const retryHead = this.headFailed;
    this._state = { ...this._state, olderError: false, warning: null,
      retryingHead: retryHead,
      loading: retryHead && this._state.messages.length === 0 ? true : this._state.loading };
    this.notify();
    if (retryHead) await this.retryHead();
    else await this.loadOlder();
  }

  private async retryHead(): Promise<void> {
    const gen = this.generation;
    const channelId = this._state.selectedChannelId;
    if (!channelId) return;
    try {
      const page = await this.client.request("conversations.channel.open", { channelId, limit: PAGE_SIZE });
      if (this.disposed || this.generation !== gen) return;
      this._state = { ...this._state, messages: oldestFirst(page.messages),
        hasMore: page.nextBefore !== undefined, nextBefore: page.nextBefore ?? null,
        loading: false, retryingHead: false, olderError: false, warning: null };
      this.installFeed(page);
      this.headFailed = false;
      this.notify();
      if (this.feed) await this.refreshFeed(false);
    } catch {
      if (this.disposed || this.generation !== gen) return;
      this.headFailed = true;
      this._state = { ...this._state, loading: false, retryingHead: false, olderError: true,
        warning: "Conversation history could not be refreshed." };
      this.notify();
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
  async postMessage(markdown: string): Promise<boolean> {
    if (this.disposed) return false;
    const channelId = this._state.selectedChannelId;
    if (!channelId || !markdown.trim() || this._state.posting) return false;
    const gen = this.generation;
    this._state = { ...this._state, posting: true, warning: null };
    this.notify();

    try {
      await this.client.request("conversations.message.post", { channelId, markdown });
      if (this.disposed || this.generation !== gen) return false;
      // A committed post does not wait for change replay or resume a failed history request.
      this._state = { ...this._state, posting: false };
      if (this.feed) void this.refreshFeed(false);
      return true;
    } catch (error) {
      if (this.disposed || this.generation !== gen) return false;
      const info = classify(error);
      this._state = {
        ...this._state,
        posting: false,
        warning: errorMessage("message.post", info, this._state.availability),
      };
      return false;
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
   * to the first available channel. During an in-flight open on the same Channel,
   * the return is notification-only: openDirty schedules replay after the open
   * watermark is installed; the caller must await the open to observe it.
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
      ++this.historyGeneration;
      this.historyRebuilding = false;
      this.headFailed = false;
      this.feed = null;
      this._state = { ...this._state, selectedChannelId: null, messages: [], hasMore: false,
        nextBefore: null, loading: false, loadingOlder: false, posting: false, retryingHead: false, olderError: false,
        feedError: false, resolveError: false, pendingHistory: 0, unavailableHistory: 0 };
      this.notify();
      return;
    }

    // Same Channel: replay from the captured projection watermark.
    this._state = {
      ...this._state,
      channels,
      availability,
      warnings: snapshot.warnings ?? [],
    };
    this.notify();
    if (this.feed) return this.reconcileSignal();
    if (this.openingChannel === this.generation) {
      this.openDirty = true;
      return;
    }
    // A failed open resumes only on explicit retry, never on another snapshot.
    return;
  }

  /** A response watermark belongs to the raw row selection, not to `open` completion. */
  private installFeed(page: ConversationPage): void {
    if (!page.epoch || page.revision === undefined) return;
    this.feed = { epoch: page.epoch, after: page.revision, status: "idle",
      pending: new Map(), unavailable: new Map(),
      positions: new Map(this._state.messages.filter(m => m.position !== undefined).map(m => [m.id, m.position!])),
      running: null, dirty: false,
      resolveStatus: "idle", failedResolveIds: new Set(), resolveRunning: null, resolveOffset: 0, rebuild: null };
    this.trackPending(page.pending ?? []);
  }

  private trackPending(rows: readonly { id: string; position: string; status: string }[]): void {
    if (!this.feed) return;
    const ready = new Set(this._state.messages.map(message => message.id));
    for (const row of rows) {
      if (ready.has(row.id) || this.removedDuringOlder?.has(row.id)) continue;
      if (row.status === "pending") this.feed.pending.set(row.id, row.position);
      else if (row.status === "unavailable") this.feed.unavailable.set(row.id, row.position);
    }
    this._state = { ...this._state, pendingHistory: this.feed.pending.size,
      unavailableHistory: this.feed.unavailable.size };
  }

  private insertionIndex(items: readonly ConversationMessage[], position: string): number {
    let lo = 0; let hi = items.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if ((items[mid]!.position ?? "") < position) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  private ordered(messages: readonly ConversationMessage[]): readonly ConversationMessage[] {
    // The Package guarantees that position uses the same bytewise order as SQLite's Order Key.
    if (!messages.every(message => message.position !== undefined)) return messages;
    return [...messages].sort((a, b) => a.position! < b.position! ? -1 : a.position! > b.position! ? 1 : 0);
  }

  private async reconcileSignal(): Promise<void> {
    const feed = this.feed;
    await this.refreshFeed(false);
    if (feed && feed === this.feed && feed.status === "idle") await this.resolvePending(false);
  }

  private refreshFeed(explicit: boolean): Promise<void> {
    const feed = this.feed;
    if (!feed || this.disposed || (feed.status === "failed" && !explicit)) return Promise.resolve();
    if (feed.running) { feed.dirty = true; return feed.running; }
    const gen = this.generation;
    const channelId = this._state.selectedChannelId;
    if (!channelId) return Promise.resolve();
    if (explicit) { feed.status = "idle"; this._state = { ...this._state, feedError: false }; this.notify(); }
    feed.dirty = true;
    const run = async () => {
      let batches = 0;
      while (feed.dirty && batches < MAX_HISTORY_PAGES && !this.disposed && this.generation === gen) {
        feed.dirty = false;
        feed.status = "pending";
        try {
          if (feed.rebuild) {
            await this.rebuildHistory(feed, channelId, gen);
            return;
          }
          const response = await this.client.request("conversations.channel.changes", {
            channelId, epoch: feed.epoch, after: feed.after, limit: PAGE_SIZE,
          });
          if (this.disposed || this.generation !== gen || this.feed !== feed) return;
          if (response.resyncRequired) {
            feed.rebuild = "start";
            ++this.historyGeneration;
            this.historyRebuilding = true;
            this.removedDuringOlder = null;
            this.olderSkippedInFlight = null;
            this._state = { ...this._state, loadingOlder: false };
            await this.rebuildHistory(feed, channelId, gen);
            return;
          }
          if (response.continuation === feed.after && response.events.length > 0) throw new Error("Non-advancing change cursor");
          let messages: ConversationMessage[] | null = null;
          const removeLoaded = (id: string): void => {
            const position = feed.positions.get(id);
            if (position === undefined) return;
            feed.positions.delete(id);
            const items = messages ?? this._state.messages;
            const candidate = this.insertionIndex(items, position);
            const index = items[candidate]?.id === id ? candidate : items.findIndex(item => item.id === id);
            if (index >= 0) {
              messages ??= [...this._state.messages];
              messages.splice(index, 1);
            }
          };
          for (const event of response.events) {
            if (event.kind === "remove") {
              this.removedDuringOlder?.add(event.id);
              removeLoaded(event.id);
              feed.pending.delete(event.id); feed.unavailable.delete(event.id);
            } else if (this._state.hasMore && this._state.nextBefore && event.position < this._state.nextBefore) {
              // An in-flight page could have selected its rows before this insert.
              // Invalidate only that page; a future page will cover the insert otherwise.
              if (this.removedDuringOlder) this.olderSkippedInFlight?.add(event.position);
            } else {
              if (event.message) {
                removeLoaded(event.id);
                messages ??= [...this._state.messages];
                messages.splice(this.insertionIndex(messages, event.position), 0,
                  { ...event.message, position: event.position });
                feed.positions.set(event.id, event.position);
                feed.pending.delete(event.id); feed.unavailable.delete(event.id);
              } else if (event.status === "pending" && !feed.positions.has(event.id)) {
                feed.pending.set(event.id, event.position);
              } else if (event.status === "unavailable" && !feed.positions.has(event.id)) {
                feed.unavailable.set(event.id, event.position);
              }
            }
          }
          for (const id of feed.failedResolveIds) {
            if (!feed.pending.has(id)) feed.failedResolveIds.delete(id);
          }
          if (feed.failedResolveIds.size === 0) feed.resolveStatus = "idle";
          this._state = { ...this._state, messages: messages ?? this._state.messages,
            feedError: false, resolveError: feed.failedResolveIds.size > 0,
            pendingHistory: feed.pending.size, unavailableHistory: feed.unavailable.size };
          feed.after = response.continuation;
          feed.status = "idle";
          if (response.continuation !== response.highWater) feed.dirty = true;
          this.notify();
          batches++;
        } catch {
          if (this.disposed || this.generation !== gen || this.feed !== feed) return;
          feed.status = "failed";
          feed.dirty = false;
          this._state = { ...this._state, feedError: true };
          this.notify();
          return;
        }
      }
      if (feed.dirty && !this.disposed && this.generation === gen) {
        // Resume on the next notification or explicit action; never discard the continuation.
        this._state = { ...this._state, feedError: true };
        feed.status = "failed";
        this.notify();
      }
    };
    feed.running = run().finally(() => {
      feed.running = null;
      if (feed.dirty && feed.status === "idle" && this.feed === feed && !this.disposed) {
        return this.refreshFeed(false);
      }
    });
    return feed.running;
  }

  /** Rebuild the previously loaded raw range before changing epochs. Keep the old view until complete. */
  private async rebuildHistory(feed: ChangeFeed, channelId: string, gen: number): Promise<void> {
    let rebuild = feed.rebuild;
    if (rebuild === "start") {
      const targetCursor = this._state.nextBefore;
      const exhausted = !this._state.hasMore
        && (this._state.messages.length > 0 || this._state.pendingHistory > 0 || this._state.unavailableHistory > 0);
      const head = await this.client.request("conversations.channel.open", { channelId, limit: PAGE_SIZE });
      if (this.disposed || this.generation !== gen || this.feed !== feed) return;
      if (!head.epoch || head.revision === undefined) throw new Error("History resync omitted its watermark");
      rebuild = { messages: [...oldestFirst(head.messages)], pending: [...(head.pending ?? [])],
        cursor: head.nextBefore ?? null, targetCursor, exhausted,
        epoch: head.epoch, revision: head.revision,
        seen: new Set<string>() };
      feed.rebuild = rebuild;
    }
    if (!rebuild) return;
    let count = 0;
    while (rebuild.cursor && (rebuild.exhausted
      || (rebuild.targetCursor !== null && rebuild.cursor > rebuild.targetCursor))) {
      if (count++ >= MAX_HISTORY_PAGES) throw new Error("History rebuild continuation required");
      const cursor = rebuild.cursor;
      if (rebuild.seen.has(cursor)) throw new Error("Non-advancing history cursor");
      rebuild.seen.add(cursor);
      const page = await this.client.request("conversations.channel.page", {
        channelId, before: cursor, limit: PAGE_SIZE,
      });
      if (this.disposed || this.generation !== gen || this.feed !== feed) return;
      rebuild.messages.unshift(...oldestFirst(page.messages));
      rebuild.pending.push(...(page.pending ?? []));
      rebuild.cursor = page.nextBefore ?? null;
    }
    if (this.disposed || this.generation !== gen || this.feed !== feed) return;
    this.headFailed = false;
    const survivingFailedIds = rebuild.pending.filter(row => row.status === "pending"
      && feed.failedResolveIds.has(row.id)).map(row => row.id);
    const preserveResolveFailure = feed.resolveStatus === "failed" && survivingFailedIds.length > 0;
    this._state = { ...this._state, messages: this.ordered([...new Map(rebuild.messages.map(m => [m.id, m])).values()]),
      hasMore: rebuild.cursor !== null, nextBefore: rebuild.cursor,
      feedError: false, resolveError: preserveResolveFailure,
      pendingHistory: 0, unavailableHistory: 0 };
    this.installFeed({ messages: [], pending: rebuild.pending, epoch: rebuild.epoch, revision: rebuild.revision });
    if (preserveResolveFailure && this.feed) {
      this.feed.resolveStatus = "failed";
      this.feed.failedResolveIds = new Set(survivingFailedIds);
    }
    this.historyRebuilding = false;
    this.notify();
    // The open watermark precedes asynchronous page reads. Always replay after installing it,
    // including when a notification arrived during the rebuild.
    await this.refreshFeed(false);
  }

  /** A snapshot signals one bounded readiness probe; explicit retries rotate through all pending IDs. */
  private resolvePending(explicit: boolean): Promise<void> {
    const feed = this.feed;
    const channelId = this._state.selectedChannelId;
    if (!feed || !channelId || feed.pending.size === 0 || (feed.resolveStatus === "failed" && !explicit)) {
      return Promise.resolve();
    }
    if (feed.resolveRunning) return feed.resolveRunning;
    const gen = this.generation;
    const candidates = [...feed.pending.keys()];
    const start = feed.resolveOffset % candidates.length;
    const requested = [...candidates.slice(start), ...candidates.slice(0, start)].slice(0, PAGE_SIZE);
    const requestedIds = new Set(requested);
    if (explicit) {
      feed.resolveStatus = "idle";
      feed.failedResolveIds.clear();
      this._state = { ...this._state, resolveError: false };
      this.notify();
    }
    const run = async () => {
      try {
        const response = await this.client.request("conversations.channel.resolve", {
          channelId, ids: requested,
        });
        if (this.disposed || this.generation !== gen || this.feed !== feed) return;
        let messages: ConversationMessage[] | null = null;
        for (const message of response.messages) {
          if (!requestedIds.has(message.id) || !feed.pending.has(message.id)) continue;
          messages ??= [...this._state.messages];
          if (message.position !== undefined) {
            messages.splice(this.insertionIndex(messages, message.position), 0, message);
            feed.positions.set(message.id, message.position);
          }
          feed.pending.delete(message.id);
        }
        for (const row of response.pending) {
          if (!requestedIds.has(row.id) || !feed.pending.has(row.id)) continue;
          if (row.status === "unavailable") { feed.pending.delete(row.id); feed.unavailable.set(row.id, row.position); }
        }
        for (const id of response.missing) {
          if (requestedIds.has(id)) { feed.pending.delete(id); feed.unavailable.delete(id); }
        }
        feed.resolveOffset = (start + requested.length) % Math.max(feed.pending.size, 1);
        this._state = { ...this._state, messages: messages ?? this._state.messages,
          pendingHistory: feed.pending.size, unavailableHistory: feed.unavailable.size,
          resolveError: false };
        this.notify();
      } catch {
        if (this.disposed || this.generation !== gen || this.feed !== feed) return;
        // Only surviving requested identities own this failure; a new pending
        // Message must not inherit it from a removed one.
        feed.failedResolveIds = new Set([...requestedIds].filter(id => feed.pending.has(id)));
        feed.resolveStatus = feed.failedResolveIds.size === 0 ? "idle" : "failed";
        this._state = { ...this._state, resolveError: feed.failedResolveIds.size > 0 };
        this.notify();
      }
    };
    feed.resolveRunning = run().finally(() => { feed.resolveRunning = null; });
    return feed.resolveRunning;
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