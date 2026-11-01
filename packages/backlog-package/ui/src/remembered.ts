/**
 * Remembered UI state: what the Backlog keeps in the Backend's memory so a Package switch does not
 * lose selection, filters, expansion or an unsaved draft. The whole state is one blob
 * (`backlog.draft.put`); a saved or cancelled draft is also removed at once with `backlog.draft.clear`.
 */
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { BacklogContract, Draft, RememberedState, Ticket } from "./contract.generated";
import type { EditState, TicketFields } from "./controller";

/** How long typing must pause before the state is sent. */
export const REMEMBER_DELAY_MS = 300;
/** Drafts the Backend accepts in one state (`drafts.maxProperties`). */
export const MAX_DRAFTS = 64;
/** Expanded Epics the Backend accepts (`expandedEpicIds.maxItems`). */
export const MAX_EXPANDED = 1024;
/** The Backend's bound on the serialized state, less room for the request envelope. */
export const REMEMBERED_BUDGET = 262_144 - 1024;
/** Draft titles are bounded like Ticket titles (`draft.title.maxLength`). */
const MAX_DRAFT_TITLE = 256;

const FIELDS = ["title", "body", "status", "priority", "assignee"] as const;

/** An edit as remembered: only the fields the Person changed, and the version they started from. */
export function draftOf(edit: EditState): Draft {
  const draft: { -readonly [K in keyof Draft]: Draft[K] } = { baseVersion: edit.base.version };
  for (const key of FIELDS) {
    if (edit.draft[key] !== edit.base[key]) (draft as Record<string, unknown>)[key] = edit.draft[key];
  }
  return draft;
}

/** Whether a remembered draft holds any change. */
export function hasChanges(draft: Draft): boolean {
  return FIELDS.some((key) => draft[key] !== undefined);
}

/**
 * Turns a remembered draft back into an edit on the Ticket as it is now. The Person's changed
 * fields keep their draft values and every other field shows the current value. When the Ticket
 * changed since the draft began, the edit keeps its old version and shows the stale notice, so the
 * Person sees the newer values and chooses "Use latest" before a save can succeed.
 */
export function restoreEdit(stored: Draft, latest: Ticket): EditState {
  const draft: TicketFields = {
    title: stored.title ?? latest.title,
    body: stored.body ?? latest.body,
    status: stored.status ?? latest.status,
    priority: stored.priority ?? latest.priority,
    assignee: stored.assignee !== undefined ? stored.assignee : latest.assignee,
  };
  if (stored.baseVersion === latest.version || !hasChanges(stored)) return { base: latest, draft, stale: null };
  return { base: { ...latest, version: stored.baseVersion }, draft, stale: latest };
}

function byteLength(value: unknown): number {
  return new TextEncoder().encode(JSON.stringify(value)).length;
}

/**
 * Fits a state to the Backend's bounds. The oldest drafts (first in `drafts`) go first, never the
 * one under `keep`; titles longer than a Ticket title can be are cut to that length.
 */
export function boundState(state: RememberedState, keep: string | null): RememberedState {
  const entries = Object.entries(state.drafts).map(([key, draft]): [string, Draft] => [key,
    draft.title !== undefined && [...draft.title].length > MAX_DRAFT_TITLE
      ? { ...draft, title: [...draft.title].slice(0, MAX_DRAFT_TITLE).join("") } : draft]);
  const build = (): RememberedState => ({
    ...state,
    expandedEpicIds: state.expandedEpicIds.slice(0, MAX_EXPANDED),
    drafts: Object.fromEntries(entries),
  });
  let bounded = build();
  while (entries.length > 0 && (entries.length > MAX_DRAFTS || byteLength(bounded) > REMEMBERED_BUDGET)) {
    const oldest = entries.findIndex(([key]) => key !== keep);
    if (oldest < 0) break;
    entries.splice(oldest, 1);
    bounded = build();
  }
  return bounded;
}

/** Reads the remembered state, or `null` when there is none or it cannot be read. */
export async function loadRemembered(client: TypedPackageClient<BacklogContract>): Promise<RememberedState | null> {
  try {
    return (await client.request("backlog.draft.get", {})).state;
  } catch {
    return null;
  }
}

/**
 * Sends remembered state to the Backend: `changed()` coalesces bursts of changes into one put
 * after {@link REMEMBER_DELAY_MS}; `flush()` sends a pending put at once. Puts and clears go out
 * one at a time, in order. Failures are ignored: remembering is a convenience.
 */
export class RememberedStateSync {
  private readonly client: TypedPackageClient<BacklogContract>;
  private readonly read: () => { state: RememberedState; keep: string | null };
  private readonly delayMs: number;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private dirty = false;
  private closed = false;
  private queue: Promise<void> = Promise.resolve();

  constructor(client: TypedPackageClient<BacklogContract>, read: () => { state: RememberedState; keep: string | null },
    delayMs = REMEMBER_DELAY_MS) {
    this.client = client;
    this.read = read;
    this.delayMs = delayMs;
  }

  /** Notes a change; the state is sent once changes pause. */
  changed(): void {
    if (this.closed) return;
    this.dirty = true;
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.flush();
    }, this.delayMs);
  }

  /** Removes one draft from the Backend's copy at once. */
  clear(key: string): void {
    if (this.closed) return;
    this.enqueue(() => this.client.request("backlog.draft.clear", { key }));
  }

  /** Sends a pending change now and resolves once everything sent so far is answered. */
  flush(): Promise<void> {
    if (this.timer !== undefined) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    if (this.dirty) {
      this.dirty = false;
      const { state, keep } = this.read();
      this.enqueue(() => this.client.request("backlog.draft.put", { state: boundState(state, keep) }));
    }
    return this.queue;
  }

  /** Flushes and stops: later changes are not sent. */
  async close(): Promise<void> {
    const flushed = this.flush();
    this.closed = true;
    await flushed;
  }

  private enqueue(send: () => Promise<unknown>): void {
    this.queue = this.queue.then(send).then(() => undefined, () => undefined);
  }
}
