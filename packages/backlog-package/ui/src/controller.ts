import { describePackageError, isAbortError, isPackageRequestError, packageErrorCode } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type {
  BacklogContract, Draft, Epic, Names, Principal, Priority, RememberedState, Snapshot, Status, Ticket, TicketSummary,
} from "./contract.generated";
import { MAX_DRAFTS, draftOf, hasChanges, restoreEdit } from "./remembered";

export type Filters = Readonly<{ priority: Priority | null; status: Status | null }>;

/** The Ticket fields a Person edits in the detail pane. */
export type TicketFields = Readonly<{
  title: string;
  body: string;
  status: Status;
  priority: Priority;
  assignee: string | null;
}>;

/** An edit in progress on the selected Ticket. */
export type EditState = Readonly<{
  /** The Ticket the draft starts from. Save sends the fields that differ from it, with its version. */
  base: Ticket;
  draft: TicketFields;
  /** The current Ticket after a stale refusal, shown until the Person chooses "Use latest". */
  stale: Ticket | null;
}>;

/** The selected Ticket as the detail pane shows it. */
export type DetailState = Readonly<{
  ticketId: string;
  /** The full Ticket as last read, or `null` until `backlog.ticket.get` answers. */
  ticket: Ticket | null;
  /** The edit in progress, or `null` while viewing. */
  edit: EditState | null;
}>;

/** The current Workspace roster from `backlog.names`. */
export type NamesState = Readonly<{ available: boolean; cursor: string; principals: readonly Principal[] }>;

export type AssigneeOption = Readonly<{ value: string; label: string }>;

/** Where the controller reports changes to what it remembers across a Package switch. */
export type RememberSink = {
  /** Something remembered changed; the sink reads `rememberedState()` when it sends. */
  changed(): void;
  /** A Ticket's draft was saved or cancelled and must be forgotten at once. */
  clear(key: string): void;
};

/** The value the assignee dropdown uses for "Unassigned". */
export const UNASSIGNED = "";

/** Titles are 1 to 256 characters (Backend `MAX_TITLE`). */
export const MAX_TITLE = 256;

/** Characters of a Principal ID or identity shown when no name is known. */
const SHORT_ID = 12;

export function fieldsOf(ticket: Ticket): TicketFields {
  return { title: ticket.title, body: ticket.body, status: ticket.status, priority: ticket.priority, assignee: ticket.assignee };
}

/** What Save must send: the changed `ticket.update` fields, and the new assignee if it changed. */
export function changedFields(base: Ticket, draft: TicketFields): {
  update: Partial<Pick<TicketFields, "title" | "body" | "status" | "priority">>;
  assignee?: string | null;
} {
  const update: { title?: string; body?: string; status?: Status; priority?: Priority } = {};
  if (draft.title !== base.title) update.title = draft.title;
  if (draft.body !== base.body) update.body = draft.body;
  if (draft.status !== base.status) update.status = draft.status;
  if (draft.priority !== base.priority) update.priority = draft.priority;
  return draft.assignee === base.assignee ? { update } : { update, assignee: draft.assignee };
}

/**
 * Moves a draft onto a newer Ticket: a field the Person changed keeps the draft value, and a field
 * they left alone takes the newer value, so Save never reverts someone else's change by accident.
 */
export function rebaseDraft(draft: TicketFields, oldBase: Ticket, latest: Ticket): TicketFields {
  const keep = <K extends keyof TicketFields>(key: K): TicketFields[K] => draft[key] === oldBase[key] ? latest[key] : draft[key];
  return { title: keep("title"), body: keep("body"), status: keep("status"), priority: keep("priority"), assignee: keep("assignee") };
}

export type ControllerState = Readonly<{
  snapshot: Snapshot;
  /** Whether a first snapshot has arrived. */
  loaded: boolean;
  showArchived: boolean;
  filters: Filters;
  /** Epics whose Tickets are shown in the tree. */
  expandedEpicIds: ReadonlySet<string>;
  /** The Ticket chosen in the tree; the detail pane shows it. */
  selectedTicketId: string | null;
  /** The selected Ticket in full, with any edit in progress. */
  detail: DetailState | null;
  names: NamesState;
  busy: boolean;
  error: string | null;
}>;

export const PRIORITIES: readonly Priority[] = ["high", "medium", "low"];
export const STATUSES: readonly Status[] = ["not_ready", "ready", "in_progress", "done"];

export const PRIORITY_LABEL: Readonly<Record<Priority, string>> = { high: "High", medium: "Medium", low: "Low" };
export const STATUS_LABEL: Readonly<Record<Status, string>> = {
  not_ready: "Not ready", ready: "Ready", in_progress: "In progress", done: "Done",
};

const EMPTY: Snapshot = { availability: "catching_up", warnings: [], epics: [], tickets: [] };

const titleCollator = new Intl.Collator(undefined, { sensitivity: "accent" });

function byText(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Epics alphabetically, ignoring case; equal titles keep creation order. */
export function sortEpics(epics: readonly Epic[]): Epic[] {
  return [...epics].sort((a, b) => titleCollator.compare(a.title, b.title)
    || byText(a.createdOrder, b.createdOrder) || byText(a.epicId, b.epicId));
}

/** Tickets by priority (high first), then status in its listed order, then creation order. */
export function sortTickets(tickets: readonly TicketSummary[]): TicketSummary[] {
  return [...tickets].sort((a, b) => PRIORITIES.indexOf(a.priority) - PRIORITIES.indexOf(b.priority)
    || STATUSES.indexOf(a.status) - STATUSES.indexOf(b.status)
    || byText(a.createdOrder, b.createdOrder) || byText(a.ticketId, b.ticketId));
}

/**
 * Holds the Backlog snapshot the Backend publishes and the UI's own navigation state, and turns
 * user actions into Package requests. Rendering is the view's job; the controller only announces
 * changes.
 */
export class BacklogController {
  private readonly client: TypedPackageClient<BacklogContract>;
  private readonly announce: () => void;
  private _state: ControllerState = {
    snapshot: EMPTY,
    loaded: false,
    showArchived: false,
    filters: { priority: null, status: null },
    expandedEpicIds: new Set(),
    selectedTicketId: null,
    detail: null,
    names: { available: false, cursor: "0", principals: [] },
    busy: false,
    error: null,
  };
  private disposed = false;
  /** An Epic this Installation just created, expanded once its record is listed. */
  private pendingEpicId: string | null = null;
  /** A Ticket this Installation just created, selected once its record is listed. */
  private pendingTicketId: string | null = null;
  /** A roster arriving in parts, held until its last part. */
  private pendingNames: { cursor: string; parts: number; nextPart: number; principals: Principal[] } | null = null;
  /** Orders `backlog.ticket.get` answers; only the latest request may fill the detail pane. */
  private loadSequence = 0;
  /** The listed version the newest `backlog.ticket.get` was sent for. */
  private requestedVersion: string | null = null;
  /** Drafts of Tickets not open in the pane (or not loaded yet), oldest first, by Ticket ID. */
  private readonly drafts = new Map<string, Draft>();
  /** Set once remembered expansion is restored, so the first listing does not open every Epic. */
  private expansionRestored = false;
  private readonly remember: RememberSink | undefined;

  constructor(client: TypedPackageClient<BacklogContract>, announce: () => void, remember?: RememberSink) {
    this.client = client;
    this.announce = announce;
    this.remember = remember;
  }

  /**
   * Restores remembered state: filters, show-archived, expansion, the drafts, and the selection.
   * The selected Ticket's draft reopens in edit mode once the Ticket is read.
   */
  restore(state: RememberedState): void {
    if (this.disposed) return;
    this.drafts.clear();
    for (const [key, draft] of Object.entries(state.drafts)) {
      if (!key.startsWith("new:")) this.drafts.set(key, draft);
    }
    this.expansionRestored = true;
    this.update({
      showArchived: state.showArchived,
      filters: { priority: state.filters.priority, status: state.filters.status },
      expandedEpicIds: new Set(state.expandedEpicIds),
    });
    this.selectTicket(state.selectedTicketId ?? null);
  }

  /**
   * The state to remember, and the draft key that must survive trimming (the open Ticket's). The
   * open edit is remembered even without changes, so edit mode survives; other drafts only exist
   * when they hold changes.
   */
  rememberedState(): { state: RememberedState; keep: string | null } {
    const { detail, filters, showArchived, expandedEpicIds, selectedTicketId } = this._state;
    const drafts: Record<string, Draft> = Object.fromEntries(this.drafts);
    const edit = detail?.edit ?? null;
    if (detail !== null && edit !== null) drafts[detail.ticketId] = draftOf(edit);
    const state: RememberedState = {
      ...(selectedTicketId === null ? {} : { selectedTicketId }),
      editing: edit !== null,
      filters: { priority: filters.priority, status: filters.status },
      showArchived,
      expandedEpicIds: [...expandedEpicIds],
      drafts,
    };
    return { state, keep: selectedTicketId };
  }

  get state(): ControllerState {
    return this._state;
  }

  /** Epics in tree order, hiding archived ones unless asked for. Filters never hide an Epic. */
  get visibleEpics(): readonly Epic[] {
    return sortEpics(this._state.snapshot.epics.filter((epic) => this._state.showArchived || !epic.archived));
  }

  /** The Tickets of one Epic in tree order, after the archive toggle and both filters. */
  visibleTickets(epicId: string): readonly TicketSummary[] {
    const { showArchived, filters } = this._state;
    return sortTickets(this._state.snapshot.tickets.filter((ticket) => ticket.epicId === epicId
      && (showArchived || !ticket.archived)
      && (filters.priority === null || ticket.priority === filters.priority)
      && (filters.status === null || ticket.status === filters.status)));
  }

  get selectedTicket(): TicketSummary | null {
    const id = this._state.selectedTicketId;
    return this._state.snapshot.tickets.find((ticket) => ticket.ticketId === id) ?? null;
  }

  /** Epics a Ticket may move to: every Epic not archived, except its own. */
  moveDestinations(ticket: TicketSummary): readonly Epic[] {
    return sortEpics(this._state.snapshot.epics.filter((epic) => !epic.archived && epic.epicId !== ticket.epicId));
  }

  applySnapshot(snapshot: Snapshot): void {
    if (this.disposed) return;
    const expanded = new Set(this._state.expandedEpicIds);
    // The first listing opens every Epic so Ticket titles show at once, unless expansion was remembered.
    if (!this._state.loaded && !this.expansionRestored) for (const epic of snapshot.epics) expanded.add(epic.epicId);
    if (this.pendingEpicId !== null && snapshot.epics.some((epic) => epic.epicId === this.pendingEpicId)) {
      expanded.add(this.pendingEpicId);
      this.pendingEpicId = null;
    }
    let selectedTicketId = this._state.selectedTicketId;
    const pending = this.pendingTicketId;
    const created = pending === null ? undefined : snapshot.tickets.find((ticket) => ticket.ticketId === pending);
    if (created !== undefined) {
      this.pendingTicketId = null;
      selectedTicketId = created.ticketId;
      expanded.add(created.epicId);
    }
    this.update({ snapshot, loaded: true, expandedEpicIds: expanded });
    if (selectedTicketId !== this._state.selectedTicketId) this.selectTicket(selectedTicketId);
    else this.refreshDetail();
  }

  /** Applies a roster event, reassembling multi-part rosters by cursor and part order. */
  applyNames(names: Names): void {
    if (this.disposed) return;
    if (!names.available) {
      this.pendingNames = null;
      this.update({ names: { available: false, cursor: "0", principals: [] } });
      return;
    }
    if (this._state.names.available && BigInt(names.cursor) < BigInt(this._state.names.cursor)) return;
    let principals: readonly Principal[] = names.principals;
    if (names.part !== undefined && names.parts !== undefined) {
      if (names.part === 0) this.pendingNames = { cursor: names.cursor, parts: names.parts, nextPart: 0, principals: [] };
      const pending = this.pendingNames;
      if (!pending || pending.cursor !== names.cursor || pending.parts !== names.parts || names.part !== pending.nextPart) return;
      this.pendingNames = { ...pending, nextPart: pending.nextPart + 1, principals: [...pending.principals, ...names.principals] };
      if (names.part !== names.parts - 1) return;
      principals = this.pendingNames.principals;
    }
    this.pendingNames = null;
    this.update({ names: { available: true, cursor: names.cursor, principals } });
  }

  /** A Principal's display name: the name, with its fingerprint when another has the same name. */
  private principalLabel(principal: Principal): string {
    if (principal.name === null) return `${principal.kind === "agent" ? "Agent" : "Person"} · ${principal.fingerprint}`;
    const duplicate = this._state.names.principals.filter((other) => other.name === principal.name).length > 1;
    return duplicate ? `${principal.name} · ${principal.fingerprint}` : principal.name;
  }

  /** A label for an ID missing from the roster: "Former member" once the roster is known. */
  private absentLabel(id: string): string {
    const short = id.slice(0, SHORT_ID);
    return this._state.names.available ? `Former member · ${short}` : short;
  }

  /** Who signed a record (`creator`, `lastEditor`), by signing identity. */
  labelForIdentity(identity: string): string {
    const principal = this._state.names.principals.find((candidate) => candidate.identity === identity);
    return principal ? this.principalLabel(principal) : this.absentLabel(identity);
  }

  /** An assignee by Principal ID; one absent from the roster is a former member. */
  labelForAssignee(principalId: string | null): string {
    if (principalId === null) return "Unassigned";
    const principal = this._state.names.principals.find((candidate) => candidate.principalId === principalId);
    return principal ? this.principalLabel(principal) : this.absentLabel(principalId);
  }

  /**
   * The assignee choices: "Unassigned" and every current Workspace Person, never an Agent. A
   * `current` assignee who is not a current Person (a former member) is listed too, so it shows
   * as chosen and can be changed.
   */
  assigneeOptions(current: string | null): readonly AssigneeOption[] {
    const persons = this._state.names.principals.filter((principal) => principal.kind === "person")
      .map((principal) => ({ value: principal.principalId, label: this.principalLabel(principal) }))
      .sort((a, b) => titleCollator.compare(a.label, b.label) || byText(a.value, b.value));
    const options: AssigneeOption[] = [{ value: UNASSIGNED, label: "Unassigned" }, ...persons];
    if (current !== null && !persons.some((option) => option.value === current)) {
      options.push({ value: current, label: this.labelForAssignee(current) });
    }
    return options;
  }

  /** Reports that live updates stopped; the listing shown may be out of date. */
  updatesStopped(): void {
    this.update({ error: "Backlog updates stopped. Switch to another Package and back to reconnect." });
  }

  toggleArchived(): void {
    this.update({ showArchived: !this._state.showArchived });
  }

  setPriorityFilter(priority: Priority | null): void {
    this.update({ filters: { ...this._state.filters, priority } });
  }

  setStatusFilter(status: Status | null): void {
    this.update({ filters: { ...this._state.filters, status } });
  }

  toggleEpic(epicId: string): void {
    const expanded = new Set(this._state.expandedEpicIds);
    if (expanded.has(epicId)) expanded.delete(epicId);
    else expanded.add(epicId);
    this.update({ expandedEpicIds: expanded });
  }

  expandEpic(epicId: string): void {
    if (this._state.expandedEpicIds.has(epicId)) return;
    this.update({ expandedEpicIds: new Set([...this._state.expandedEpicIds, epicId]) });
  }

  /**
   * Selects a Ticket and reads it in full. An unsaved edit with changes on the Ticket left behind
   * is kept as its draft and reopens when that Ticket is selected again.
   */
  selectTicket(ticketId: string | null): void {
    if (ticketId === this._state.selectedTicketId && (ticketId === null || this._state.detail !== null)) {
      this.update({ error: null });
      return;
    }
    this.stashEdit();
    this.loadSequence++;
    this.requestedVersion = null;
    this.update({
      selectedTicketId: ticketId,
      detail: ticketId === null ? null : { ticketId, ticket: null, edit: null },
      error: null,
    });
    if (ticketId !== null) void this.loadTicket(ticketId, this.ticket(ticketId)?.version ?? null);
  }

  /** Starts editing the selected Ticket from its current values. */
  startEdit(): void {
    const detail = this._state.detail;
    if (detail?.ticket == null || detail.edit !== null) return;
    this.update({ detail: { ...detail, edit: { base: detail.ticket, draft: fieldsOf(detail.ticket), stale: null } } });
  }

  /** Changes the draft, starting an edit first if none is in progress. */
  editDraft(changes: Partial<TicketFields>): void {
    this.startEdit();
    const detail = this._state.detail;
    if (detail?.edit == null) return;
    this.update({ detail: { ...detail, edit: { ...detail.edit, draft: { ...detail.edit.draft, ...changes } } } });
  }

  /** Discards the draft, here and in the remembered state. */
  cancelEdit(): void {
    const detail = this._state.detail;
    if (detail?.edit == null) return;
    this.forgetDraft(detail.ticketId);
    this.update({ detail: { ...detail, edit: null }, error: null });
  }

  /**
   * Saves the draft: `backlog.ticket.update` with only the changed fields, then
   * `backlog.ticket.assign` if the assignee changed, each with the version it is based on. A stale
   * refusal keeps the draft and records the current Ticket for the notice.
   */
  async saveEdit(): Promise<boolean> {
    const detail = this._state.detail;
    const edit = detail?.edit;
    if (detail == null || edit == null || this.disposed || this._state.busy) return false;
    const title = edit.draft.title.trim();
    if (title.length === 0 || [...title].length > MAX_TITLE) {
      this.update({ error: describeError({ code: "backlog.invalid" }) });
      return false;
    }
    const { ticketId } = detail;
    const changes = changedFields(edit.base, { ...edit.draft, title });
    this.update({ busy: true, error: null });
    let base = edit.base;
    try {
      if (Object.keys(changes.update).length > 0) {
        const { version } = await this.client.request("backlog.ticket.update", { ticketId, baseVersion: base.version, ...changes.update });
        base = { ...base, ...changes.update, version };
      }
      if (changes.assignee !== undefined) {
        const assignee = changes.assignee;
        const { version } = await this.client.request("backlog.ticket.assign", { ticketId, baseVersion: base.version, assignee });
        base = { ...base, assignee, version };
      }
    } catch (error) {
      if (isAbortError(error) || this.disposed) return false;
      const stale = await this.staleTicket(error, ticketId, base);
      const current = this._state.detail;
      if (current?.ticketId === ticketId && current.edit !== null) {
        this.update({
          detail: { ticketId, ticket: stale ?? current.ticket, edit: { ...current.edit, base, stale } },
          busy: false,
          error: stale === null ? describeError(error) : null,
        });
      } else {
        this.update({ busy: false, error: describeError(error) });
      }
      return false;
    }
    this.forgetDraft(ticketId);
    const current = this._state.detail;
    if (current?.ticketId === ticketId) {
      // `base` now holds the saved values; read the Ticket again for its attribution.
      const fresh = current.ticket !== null && current.ticket.version >= base.version;
      this.update({ detail: { ticketId, ticket: fresh ? current.ticket : base, edit: null }, busy: false });
      if (!fresh) void this.loadTicket(ticketId, base.version);
    } else {
      this.update({ busy: false });
    }
    return true;
  }

  /**
   * After a stale refusal: reads the current Ticket and moves the draft onto it. Fields the Person
   * changed keep the draft value, so the draft text can be saved again.
   */
  async useLatest(): Promise<boolean> {
    const detail = this._state.detail;
    if (detail?.edit == null) return false;
    const { ticketId } = detail;
    const latest = await this.perform(() => this.client.request("backlog.ticket.get", { ticketId }));
    const current = this._state.detail;
    if (latest === null || current?.ticketId !== ticketId || current.edit === null) return false;
    const draft = rebaseDraft(current.edit.draft, current.edit.base, latest);
    this.update({ detail: { ticketId, ticket: latest, edit: { base: latest, draft, stale: null } } });
    return true;
  }

  async createEpic(title: string): Promise<boolean> {
    const created = await this.perform(() => this.client.request("backlog.epic.create", { title: title.trim() }));
    if (created === null) return false;
    if (this._state.snapshot.epics.some((epic) => epic.epicId === created.epicId)) this.expandEpic(created.epicId);
    else this.pendingEpicId = created.epicId;
    return true;
  }

  async renameEpic(epicId: string, title: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("backlog.epic.rename", { epicId, title: title.trim() }))) !== null;
  }

  async archiveEpic(epicId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("backlog.epic.archive", { epicId }))) !== null;
  }

  async restoreEpic(epicId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("backlog.epic.restore", { epicId }))) !== null;
  }

  /** Creates a Ticket with the default status and priority and selects it once it is listed. */
  async createTicket(epicId: string, title: string): Promise<boolean> {
    const created = await this.perform(() => this.client.request("backlog.ticket.create", { epicId, title: title.trim() }));
    if (created === null) return false;
    const listed = this._state.snapshot.tickets.some((ticket) => ticket.ticketId === created.ticketId);
    if (listed) this.selectTicket(created.ticketId);
    else this.pendingTicketId = created.ticketId;
    return true;
  }

  async moveTicket(ticketId: string, epicId: string): Promise<boolean> {
    const ticket = this.ticket(ticketId);
    if (ticket === null) return false;
    return (await this.perform(() => this.client.request("backlog.ticket.move",
      { ticketId, baseVersion: ticket.version, epicId }))) !== null;
  }

  async archiveTicket(ticketId: string): Promise<boolean> {
    const ticket = this.ticket(ticketId);
    if (ticket === null) return false;
    return (await this.perform(() => this.client.request("backlog.ticket.archive",
      { ticketId, baseVersion: ticket.version }))) !== null;
  }

  async restoreTicket(ticketId: string): Promise<boolean> {
    const ticket = this.ticket(ticketId);
    if (ticket === null) return false;
    return (await this.perform(() => this.client.request("backlog.ticket.restore",
      { ticketId, baseVersion: ticket.version }))) !== null;
  }

  dispose(): void {
    this.disposed = true;
  }

  /** Reads the selected Ticket again when the listing shows a version the pane has not read. */
  private refreshDetail(): void {
    const detail = this._state.detail;
    const summary = this.selectedTicket;
    if (detail === null || summary === null || detail.ticketId !== summary.ticketId) return;
    if (detail.ticket?.version === summary.version || this.requestedVersion === summary.version) return;
    void this.loadTicket(detail.ticketId, summary.version);
  }

  private async loadTicket(ticketId: string, listedVersion: string | null = null): Promise<void> {
    const sequence = ++this.loadSequence;
    this.requestedVersion = listedVersion;
    try {
      const ticket = await this.client.request("backlog.ticket.get", { ticketId });
      const detail = this._state.detail;
      if (sequence !== this.loadSequence || detail?.ticketId !== ticketId) return;
      // An answer older than what the pane already shows (a save just landed) changes nothing.
      if (detail.ticket !== null && detail.ticket.version > ticket.version) return;
      const stored = detail.edit === null ? this.drafts.get(ticketId) : undefined;
      if (stored !== undefined) this.drafts.delete(ticketId);
      this.update({ detail: { ...detail, ticket, edit: stored === undefined ? detail.edit : restoreEdit(stored, ticket) } });
    } catch (error) {
      if (isAbortError(error) || this.disposed || sequence !== this.loadSequence) return;
      this.requestedVersion = null;
      this.update({ error: describeError(error) });
    }
  }

  /** The current Ticket a `backlog.stale_version` refusal carries, reading the body if it was left out. */
  private async staleTicket(error: unknown, ticketId: string, base: Ticket): Promise<Ticket | null> {
    if (!isPackageRequestError(error) || error.packageError?.code !== "backlog.stale_version") return null;
    const details = error.packageError.details as unknown as { ticket?: Ticket; bodyOmitted?: boolean };
    const ticket = details.ticket;
    if (ticket === undefined) return null;
    if (details.bodyOmitted !== true) return ticket;
    try {
      return await this.client.request("backlog.ticket.get", { ticketId });
    } catch {
      // Without the body, show the other current values and keep the body the draft started from.
      return { ...ticket, body: base.body };
    }
  }

  /** Keeps the open edit, when it changes anything, as the draft of the Ticket being left. */
  private stashEdit(): void {
    const detail = this._state.detail;
    if (detail?.edit == null) return;
    const draft = draftOf(detail.edit);
    if (!hasChanges(draft)) return;
    this.drafts.delete(detail.ticketId);
    this.drafts.set(detail.ticketId, draft);
    // The newest drafts win; the open Ticket's own draft is added on top when remembered.
    for (const key of this.drafts.keys()) {
      if (this.drafts.size < MAX_DRAFTS) break;
      this.drafts.delete(key);
    }
  }

  private forgetDraft(ticketId: string): void {
    this.drafts.delete(ticketId);
    this.remember?.clear(ticketId);
  }

  private ticket(ticketId: string): TicketSummary | null {
    return this._state.snapshot.tickets.find((ticket) => ticket.ticketId === ticketId) ?? null;
  }

  private async perform<T>(action: () => Promise<T>): Promise<T | null> {
    if (this.disposed || this._state.busy) return null;
    this.update({ busy: true, error: null });
    try {
      const result = await action();
      this.update({ busy: false });
      return result;
    } catch (error) {
      if (isAbortError(error) || this.disposed) return null;
      this.update({ busy: false, error: describeError(error) });
      return null;
    }
  }

  private update(changes: Partial<ControllerState>): void {
    if (this.disposed) return;
    const before = this._state;
    this._state = { ...before, ...changes };
    const after = this._state;
    if (before.showArchived !== after.showArchived || before.filters !== after.filters
      || before.expandedEpicIds !== after.expandedEpicIds || before.selectedTicketId !== after.selectedTicketId
      || before.detail?.edit !== after.detail?.edit) this.remember?.changed();
    this.announce();
  }
}

export function describeError(error: unknown): string {
  const code = packageErrorCode(error) ?? (error as { code?: unknown } | null)?.code;
  switch (code) {
    case "backlog.stale_version":
      return "This Ticket changed since you saw it. Check the current version and try again.";
    case "backlog.not_found":
      return "That Epic or Ticket is no longer available.";
    case "backlog.epic_archived":
      return "That Epic is archived. Restore it first.";
    case "backlog.invalid":
      return "Titles are 1 to 256 characters with no leading or trailing spaces.";
    case "backlog.too_large":
      return "That is too long to save.";
    case "backlog.assignee_invalid":
      return "Only a current Workspace Person can be assigned.";
    case "backlog.unavailable":
      return "The Backlog is unavailable right now. Try again.";
    default:
      return describePackageError(error);
  }
}
