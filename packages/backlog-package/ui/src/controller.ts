import { describePackageError, isAbortError, packageErrorCode } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { BacklogContract, Epic, Priority, Snapshot, Status, TicketSummary } from "./contract.generated";

export type Filters = Readonly<{ priority: Priority | null; status: Status | null }>;

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
    busy: false,
    error: null,
  };
  private disposed = false;
  /** An Epic this Installation just created, expanded once its record is listed. */
  private pendingEpicId: string | null = null;
  /** A Ticket this Installation just created, selected once its record is listed. */
  private pendingTicketId: string | null = null;

  constructor(client: TypedPackageClient<BacklogContract>, announce: () => void) {
    this.client = client;
    this.announce = announce;
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
    // The first listing opens every Epic so Ticket titles show at once.
    if (!this._state.loaded) for (const epic of snapshot.epics) expanded.add(epic.epicId);
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
    this.update({ snapshot, loaded: true, expandedEpicIds: expanded, selectedTicketId });
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

  selectTicket(ticketId: string | null): void {
    this.update({ selectedTicketId: ticketId, error: null });
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
    this._state = { ...this._state, ...changes };
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
