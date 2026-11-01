import { typedPackageClient } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import type { BacklogContract, Epic, TicketSummary } from "./contract.generated";
import { BacklogController, describeError, sortEpics, sortTickets } from "./controller";
import { createFakeBacklog, order, recordId } from "./test-support/fake-backend";

function epic(n: number, title: string, archived = false): Epic {
  return { epicId: recordId(n), title, archived, createdOrder: order(n) };
}

function ticket(n: number, fields: Partial<TicketSummary>): TicketSummary {
  return {
    ticketId: recordId(n), epicId: recordId(1), title: `T${n}`, status: "not_ready", priority: "medium",
    assignee: null, archived: false, version: order(n), createdOrder: order(n), ...fields,
  };
}

describe("tree ordering", () => {
  it("orders Epics alphabetically ignoring case, ties by creation order", () => {
    const sorted = sortEpics([epic(5, "beta"), epic(3, "Alpha"), epic(4, "alpha"), epic(1, "Gamma"), epic(2, "ALPHA")]);
    expect(sorted.map((e) => `${e.title}@${parseInt(e.createdOrder, 16)}`))
      .toEqual(["ALPHA@2", "Alpha@3", "alpha@4", "beta@5", "Gamma@1"]);
  });

  it("orders Tickets by priority, then status order, then creation order", () => {
    const sorted = sortTickets([
      ticket(1, { title: "low ready", priority: "low", status: "ready" }),
      ticket(2, { title: "med done", priority: "medium", status: "done" }),
      ticket(3, { title: "high in progress", priority: "high", status: "in_progress" }),
      ticket(4, { title: "med not ready later", priority: "medium", status: "not_ready" }),
      ticket(5, { title: "high not ready", priority: "high", status: "not_ready" }),
      ticket(0, { title: "med not ready first", priority: "medium", status: "not_ready" }),
      ticket(6, { title: "med ready", priority: "medium", status: "ready" }),
    ]);
    expect(sorted.map((t) => t.title)).toEqual([
      "high not ready", "high in progress",
      "med not ready first", "med not ready later", "med ready", "med done",
      "low ready",
    ]);
  });
});

/** A controller fed from the in-memory Backend, with the first snapshot applied. */
function setup(seed: Parameters<typeof createFakeBacklog>[0]) {
  const backlog = createFakeBacklog(seed);
  const controller = new BacklogController(typedPackageClient<BacklogContract>(backlog.client), () => undefined);
  controller.applySnapshot(backlog.snapshot());
  return { backlog, controller };
}

const A = recordId(1);
const B = recordId(2);
const C = recordId(3);

const seed = () => ({
  epics: [epic(1, "Alpha"), epic(2, "beta"), epic(3, "Closed", true)],
  tickets: [
    { epicId: A, title: "A high ready", priority: "high" as const, status: "ready" as const },
    { epicId: A, title: "A low ready", priority: "low" as const, status: "ready" as const },
    { epicId: A, title: "A high done", priority: "high" as const, status: "done" as const },
    { epicId: A, title: "A archived high ready", priority: "high" as const, status: "ready" as const, archived: true },
    { epicId: B, title: "B medium not ready" },
    { epicId: C, title: "C high ready", priority: "high" as const, status: "ready" as const },
  ],
});

describe("BacklogController", () => {
  it("hides archived Epics and Tickets until show archived is on", () => {
    const { controller } = setup(seed());
    expect(controller.visibleEpics.map((e) => e.title)).toEqual(["Alpha", "beta"]);
    expect(controller.visibleTickets(A).map((t) => t.title)).toEqual(["A high ready", "A high done", "A low ready"]);
    controller.toggleArchived();
    expect(controller.visibleEpics.map((e) => e.title)).toEqual(["Alpha", "beta", "Closed"]);
    expect(controller.visibleTickets(A).map((t) => t.title))
      .toEqual(["A high ready", "A archived high ready", "A high done", "A low ready"]);
  });

  it("expands every Epic on the first snapshot only", () => {
    const { backlog, controller } = setup(seed());
    expect([...controller.state.expandedEpicIds].sort()).toEqual([A, B, C]);
    controller.toggleEpic(B);
    backlog.epics.set(recordId(99), epic(99, "Later"));
    controller.applySnapshot(backlog.snapshot());
    expect(controller.state.expandedEpicIds.has(B)).toBe(false);
    expect(controller.state.expandedEpicIds.has(recordId(99))).toBe(false);
  });

  it("combines filters, keeps every Epic and clears each filter on its own", () => {
    const { controller } = setup(seed());
    controller.setPriorityFilter("high");
    expect(controller.visibleEpics.map((e) => e.title)).toEqual(["Alpha", "beta"]);
    expect(controller.visibleTickets(A).map((t) => t.title)).toEqual(["A high ready", "A high done"]);
    expect(controller.visibleTickets(B)).toEqual([]);
    controller.setStatusFilter("ready");
    expect(controller.visibleTickets(A).map((t) => t.title)).toEqual(["A high ready"]);
    controller.setPriorityFilter(null);
    expect(controller.state.filters).toEqual({ priority: null, status: "ready" });
    expect(controller.visibleTickets(A).map((t) => t.title)).toEqual(["A high ready", "A low ready"]);
    controller.setStatusFilter(null);
    expect(controller.visibleTickets(A)).toHaveLength(3);
  });

  it("sends each Ticket's current version as baseVersion", async () => {
    const { backlog, controller } = setup(seed());
    const target = controller.visibleTickets(A)[0]!;
    await controller.moveTicket(target.ticketId, B);
    expect(backlog.requests().at(-1)).toEqual({
      operation: "backlog.ticket.move", payload: { ticketId: target.ticketId, baseVersion: target.version, epicId: B },
    });
    const movedVersion = backlog.tickets.get(target.ticketId)!.version;
    expect(movedVersion).not.toBe(target.version);
    controller.applySnapshot(backlog.snapshot());
    await controller.archiveTicket(target.ticketId);
    expect(backlog.requests().at(-1)).toEqual({
      operation: "backlog.ticket.archive", payload: { ticketId: target.ticketId, baseVersion: movedVersion },
    });
    controller.applySnapshot(backlog.snapshot());
    await controller.restoreTicket(target.ticketId);
    expect(backlog.requests().at(-1)?.operation).toBe("backlog.ticket.restore");
    expect(controller.state.error).toBeNull();
  });

  it("shows a stale refusal as an error", async () => {
    const { backlog, controller } = setup(seed());
    const target = controller.visibleTickets(A)[0]!;
    backlog.remoteChange(target.ticketId, { status: "done" });
    expect(await controller.archiveTicket(target.ticketId)).toBe(false);
    expect(controller.state.error).toMatch(/changed since you saw it/);
  });

  it("sends Epic requests with trimmed titles and expands a new Epic once listed", async () => {
    const { backlog, controller } = setup(seed());
    expect(await controller.createEpic("  Roadmap ")).toBe(true);
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.create", payload: { title: "Roadmap" } });
    controller.applySnapshot(backlog.snapshot());
    const created = controller.visibleEpics.find((e) => e.title === "Roadmap")!;
    expect(controller.state.expandedEpicIds.has(created.epicId)).toBe(true);
    await controller.renameEpic(A, " Apex ");
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.rename", payload: { epicId: A, title: "Apex" } });
    await controller.archiveEpic(A);
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.archive", payload: { epicId: A } });
    await controller.restoreEpic(A);
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.restore", payload: { epicId: A } });
  });

  it("creates a Ticket with defaults and selects it once listed", async () => {
    const { backlog, controller } = setup(seed());
    expect(await controller.createTicket(B, " Write docs ")).toBe(true);
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.ticket.create", payload: { epicId: B, title: "Write docs" } });
    expect(controller.state.selectedTicketId).toBeNull();
    controller.applySnapshot(backlog.snapshot());
    expect(controller.selectedTicket?.title).toBe("Write docs");
    expect(controller.selectedTicket?.status).toBe("not_ready");
    expect(controller.selectedTicket?.priority).toBe("medium");
  });

  it("offers every other non-archived Epic as a move destination", () => {
    const { controller } = setup(seed());
    const target = controller.visibleTickets(A)[0]!;
    expect(controller.moveDestinations(target).map((e) => e.epicId)).toEqual([B]);
  });

  it("describes Backlog error codes", () => {
    const error = (code: string) => ({ code });
    expect(describeError(error("backlog.epic_archived"))).toMatch(/archived/);
    expect(describeError(error("backlog.not_found"))).toMatch(/no longer available/);
    expect(describeError(error("backlog.invalid"))).toMatch(/1 to 256/);
  });
});
