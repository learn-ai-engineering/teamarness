/**
 * An in-memory Backlog Backend for UI tests. It answers the same `client.request` and
 * `client.subscribe` calls the real Backend does, with the Phase 2 payloads and error codes, and
 * publishes a full `backlog.changes` snapshot after every change.
 */
import { createFakePackageClient, createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import type { Epic, Priority, RememberedState, Snapshot, Status, Ticket, TicketSummary } from "../contract.generated";

type Payload = { readonly [key: string]: PackageValue };

export type FakeEpic = { epicId: string; title: string; archived: boolean; createdOrder: string };
export type FakeTicket = {
  ticketId: string; epicId: string; title: string; body: string; status: Status; priority: Priority;
  assignee: string | null; archived: boolean; version: string; createdOrder: string;
  creator: string; createdAt: number; lastEditor: string; lastAt: number;
};

/** A 64-hex record ID from a small number. */
export function recordId(n: number): string {
  return n.toString(16).padStart(64, "0");
}

/** An 82-hex OrderKey from a small number; larger numbers sort later. */
export function order(n: number): string {
  return n.toString(16).padStart(82, "0");
}

function failure(code: string, details: { readonly [key: string]: PackageValue } = {}) {
  return createPackageRequestError({ packageError: { code, details } });
}

export function createFakeBacklog(seed: { epics?: FakeEpic[]; tickets?: Partial<FakeTicket>[] } = {}) {
  const epics = new Map<string, FakeEpic>();
  const tickets = new Map<string, FakeTicket>();
  let clock = 1000;
  let remembered: RememberedState | null = null;
  const LOCAL = "local-identity";
  const next = () => ++clock;

  for (const epic of seed.epics ?? []) epics.set(epic.epicId, { ...epic });
  for (const partial of seed.tickets ?? []) {
    const n = next();
    const ticket: FakeTicket = {
      ticketId: recordId(n), epicId: "", title: "Ticket", body: "", status: "not_ready", priority: "medium",
      assignee: null, archived: false, version: order(n), createdOrder: order(n),
      creator: LOCAL, createdAt: n, lastEditor: LOCAL, lastAt: n, ...partial,
    };
    tickets.set(ticket.ticketId, ticket);
  }

  const snapshot = (): Snapshot => ({
    availability: "current",
    warnings: [],
    epics: [...epics.values()].sort((a, b) => a.epicId.localeCompare(b.epicId)).map((epic): Epic => ({ ...epic })),
    tickets: [...tickets.values()].sort((a, b) => a.ticketId.localeCompare(b.ticketId)).map((ticket): TicketSummary => ({
      ticketId: ticket.ticketId, epicId: ticket.epicId, title: ticket.title, status: ticket.status,
      priority: ticket.priority, assignee: ticket.assignee, archived: ticket.archived,
      version: ticket.version, createdOrder: ticket.createdOrder,
    })),
  });

  const full = (ticket: FakeTicket): Ticket => ({
    ticketId: ticket.ticketId, epicId: ticket.epicId, title: ticket.title, body: ticket.body, status: ticket.status,
    priority: ticket.priority, assignee: ticket.assignee, archived: ticket.archived, version: ticket.version,
    creator: ticket.creator, createdAt: ticket.createdAt, lastEditor: ticket.lastEditor, lastAt: ticket.lastAt,
  });

  /** Set by the test to hold every request until released. */
  let gate: Promise<void> | null = null;
  let publish: () => Promise<void> = async () => undefined;

  const epicOf = (payload: Payload): FakeEpic => {
    const epic = epics.get(String(payload.epicId));
    if (!epic) throw failure("backlog.not_found", { id: String(payload.epicId) });
    return epic;
  };
  const changeTicket = (payload: Payload, change: (ticket: FakeTicket) => void): { version: string } => {
    const ticket = tickets.get(String(payload.ticketId));
    if (!ticket) throw failure("backlog.not_found", { id: String(payload.ticketId) });
    if (payload.baseVersion !== ticket.version) {
      throw failure("backlog.stale_version", { ticket: full(ticket) as unknown as PackageValue });
    }
    change(ticket);
    const n = next();
    ticket.version = order(n);
    ticket.lastEditor = LOCAL;
    ticket.lastAt = n;
    return { version: ticket.version };
  };

  const mutate = <T extends PackageValue>(apply: (payload: Payload) => T) => async (payload: PackageValue): Promise<T> => {
    if (gate) await gate;
    const result = apply(payload as Payload);
    await publish();
    return result;
  };

  const fake = createFakePackageClient({
    requests: {
      "backlog.epic.create": mutate((payload) => {
        const n = next();
        const epic = { epicId: recordId(n), title: String(payload.title), archived: false, createdOrder: order(n) };
        epics.set(epic.epicId, epic);
        return { epicId: epic.epicId };
      }),
      "backlog.epic.rename": mutate((payload) => { epicOf(payload).title = String(payload.title); return {}; }),
      "backlog.epic.archive": mutate((payload) => { epicOf(payload).archived = true; return {}; }),
      "backlog.epic.restore": mutate((payload) => { epicOf(payload).archived = false; return {}; }),
      "backlog.ticket.create": mutate((payload) => {
        const epic = epicOf(payload);
        if (epic.archived) throw failure("backlog.epic_archived", { epicId: epic.epicId });
        const n = next();
        const ticket: FakeTicket = {
          ticketId: recordId(n), epicId: epic.epicId, title: String(payload.title), body: String(payload.body ?? ""),
          status: (payload.status as Status | undefined) ?? "not_ready", priority: (payload.priority as Priority | undefined) ?? "medium",
          assignee: null, archived: false, version: order(n), createdOrder: order(n),
          creator: LOCAL, createdAt: n, lastEditor: LOCAL, lastAt: n,
        };
        tickets.set(ticket.ticketId, ticket);
        return { ticketId: ticket.ticketId, version: ticket.version };
      }),
      "backlog.ticket.update": mutate((payload) => changeTicket(payload, (ticket) => {
        if (payload.title !== undefined) ticket.title = String(payload.title);
        if (payload.body !== undefined) ticket.body = String(payload.body);
        if (payload.status !== undefined) ticket.status = payload.status as Status;
        if (payload.priority !== undefined) ticket.priority = payload.priority as Priority;
      })),
      "backlog.ticket.assign": mutate((payload) => changeTicket(payload, (ticket) => {
        ticket.assignee = payload.assignee === null ? null : String(payload.assignee);
      })),
      "backlog.ticket.move": mutate((payload) => changeTicket(payload, (ticket) => {
        const epic = epicOf(payload);
        if (epic.archived) throw failure("backlog.epic_archived", { epicId: epic.epicId });
        ticket.epicId = epic.epicId;
      })),
      "backlog.ticket.archive": mutate((payload) => changeTicket(payload, (ticket) => { ticket.archived = true; })),
      "backlog.ticket.restore": mutate((payload) => changeTicket(payload, (ticket) => { ticket.archived = false; })),
      "backlog.ticket.get": (payload) => {
        const ticket = tickets.get(String((payload as Payload).ticketId));
        if (!ticket) throw failure("backlog.not_found", { id: String((payload as Payload).ticketId) });
        return full(ticket) as unknown as PackageValue;
      },
      "backlog.draft.get": () => ({ state: remembered as unknown as PackageValue }),
      "backlog.draft.put": (payload) => { remembered = (payload as Payload).state as unknown as RememberedState; return {}; },
      "backlog.draft.clear": (payload) => {
        const key = String((payload as Payload).key);
        if (remembered) {
          const drafts = { ...remembered.drafts };
          delete drafts[key];
          remembered = { ...remembered, drafts };
        }
        return {};
      },
    },
    subscriptions: { "backlog.changes": () => undefined, "backlog.names": () => undefined },
  });

  publish = () => fake.emit("backlog.changes", snapshot() as unknown as PackageValue);

  return {
    fake,
    client: fake.client,
    epics,
    tickets,
    snapshot,
    /** Sends the current snapshot to every open `backlog.changes` subscription. */
    publish: () => publish(),
    /** The mutating requests the UI made, in order. */
    requests: () => fake.calls.filter((call) => call.operation.startsWith("backlog.") && call.operation !== "backlog.changes"
      && call.operation !== "backlog.names"),
    /** Changes a Ticket as another Installation would, without the UI asking. */
    remoteChange(ticketId: string, change: Partial<FakeTicket>): void {
      const ticket = tickets.get(ticketId);
      if (!ticket) throw new Error(`no Ticket ${ticketId}`);
      Object.assign(ticket, change, { version: order(next()) });
    },
    /** Holds every mutating request until the returned release is called. */
    hold(): () => void {
      let release!: () => void;
      gate = new Promise((resolve) => { release = () => { gate = null; resolve(); }; });
      return release;
    },
    get remembered() { return remembered; },
  };
}

export type FakeBacklog = ReturnType<typeof createFakeBacklog>;
