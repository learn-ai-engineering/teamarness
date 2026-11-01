// @vitest-environment happy-dom

import { createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { afterEach, describe, expect, it } from "vitest";
import { mount } from "./index";
import { createFakeBacklog, order, recordId } from "./test-support/fake-backend";
import type { FakeBacklog } from "./test-support/fake-backend";

const ALPHA = recordId(1);
const BETA = recordId(2);
const CLOSED = recordId(3);
const SAME = recordId(4);

function seed() {
  return {
    epics: [
      { epicId: BETA, title: "beta", archived: false, createdOrder: order(2) },
      { epicId: SAME, title: "ALPHA", archived: false, createdOrder: order(4) },
      { epicId: CLOSED, title: "Closed", archived: true, createdOrder: order(3) },
      { epicId: ALPHA, title: "Alpha", archived: false, createdOrder: order(1) },
    ],
    tickets: [
      { epicId: ALPHA, title: "Low ready", priority: "low" as const, status: "ready" as const },
      { epicId: ALPHA, title: "High done", priority: "high" as const, status: "done" as const },
      { epicId: ALPHA, title: "High ready", priority: "high" as const, status: "ready" as const },
      { epicId: ALPHA, title: "Old high", priority: "high" as const, status: "not_ready" as const, archived: true },
      { epicId: ALPHA, title: "Medium in progress", priority: "medium" as const, status: "in_progress" as const },
      { epicId: BETA, title: "Beta medium", priority: "medium" as const, status: "not_ready" as const },
      { epicId: CLOSED, title: "Closed ticket", priority: "high" as const, status: "ready" as const },
    ],
  };
}

let teardown: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const step of teardown) await step();
  teardown = [];
});

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** Mounts the Backlog against the in-memory Backend and delivers its first snapshot. */
async function mounted(backlog: FakeBacklog = createFakeBacklog(seed())) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = host.attachShadow({ mode: "open" });
  const unmount = await mount(createTestMountContext({ client: backlog.client, root, displayName: "Backlog" }));
  teardown.push(async () => { await unmount(); host.remove(); });
  await backlog.publish();
  const q = <E extends Element = HTMLElement>(selector: string) => root.querySelector(selector) as E;
  const labelled = (label: string) => q<HTMLButtonElement>(`[aria-label="${label}"]`);
  const texts = (selector: string) => [...root.querySelectorAll(selector)].map((element) => element.textContent ?? "");
  const epicTitles = () => texts(".epic-title");
  const ticketsIn = (title: string) => {
    const list = q(`[aria-label="Tickets in ${title}"]`);
    return list === null ? null : [...list.querySelectorAll(".ticket-title-text")].map((element) => element.textContent);
  };
  /** Chooses `label` in the listbox whose button is `name`. */
  const choose = (name: string, label: string) => {
    labelled(name).click();
    const popup = root.getElementById(labelled(name).getAttribute("aria-controls")!)!;
    const option = [...popup.querySelectorAll<HTMLElement>('[role="option"]')].find((item) => item.textContent === label);
    if (!option) throw new Error(`no option ${label} in ${name}`);
    option.click();
  };
  return { root, backlog, q, labelled, texts, epicTitles, ticketsIn, choose, unmount };
}

describe("Backlog tree", () => {
  it("orders Epics alphabetically and Tickets by priority, status and creation", async () => {
    const { q, epicTitles, ticketsIn } = await mounted();
    expect(q("h1").textContent).toBe("Backlog");
    expect(q("[role=status]").textContent).toBe("Up to date");
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta"]);
    expect(ticketsIn("Alpha")).toEqual(["High ready", "High done", "Medium in progress", "Low ready"]);
    expect(ticketsIn("beta")).toEqual(["Beta medium"]);
    expect(q(".ticket-meta").textContent).toBe("High · Ready");
  });

  it("shows archived Epics and Tickets behind the eye toggle", async () => {
    const { q, epicTitles, ticketsIn } = await mounted();
    const toggle = q<HTMLButtonElement>(".archived-toggle");
    expect(toggle.getAttribute("aria-label")).toBe("Show archived");
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    toggle.click();
    expect(toggle.getAttribute("aria-label")).toBe("Hide archived");
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta", "Closed (archived)"]);
    expect(ticketsIn("Alpha")).toEqual(["Old high (archived)", "High ready", "High done", "Medium in progress", "Low ready"]);
    // The archived Epic lists, but takes no new Tickets.
    expect(ticketsIn("Closed")).toEqual(["Closed ticket"]);
    expect(q('[aria-label="Add Ticket to Closed"]')).toBeNull();
    toggle.click();
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta"]);
  });

  it("collapses and expands an Epic", async () => {
    const { root, ticketsIn } = await mounted();
    const toggle = [...root.querySelectorAll<HTMLButtonElement>(".epic-button")].find((b) => b.textContent === "beta")!;
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    toggle.click();
    expect(ticketsIn("beta")).toBeNull();
    const again = [...root.querySelectorAll<HTMLButtonElement>(".epic-button")].find((b) => b.textContent === "beta")!;
    expect(again.getAttribute("aria-expanded")).toBe("false");
    expect(root.activeElement === again || root.activeElement === null).toBe(true);
    again.click();
    expect(ticketsIn("beta")).toEqual(["Beta medium"]);
  });

  it("never renders a native select", async () => {
    const { root, q, labelled, backlog } = await mounted();
    expect(document.querySelector("select")).toBeNull();
    expect(root.querySelector("select")).toBeNull();
    q<HTMLButtonElement>(".archived-toggle").click();
    labelled("Actions for High ready").click();
    labelled("Actions for Alpha").click();
    expect(root.querySelector("select")).toBeNull();
    expect(backlog.snapshot().tickets.length).toBeGreaterThan(0);
  });
});

describe("Backlog filters", () => {
  it("combine, keep every Epic visible and clear in one action each", async () => {
    const { root, labelled, epicTitles, ticketsIn, choose, q } = await mounted();
    expect(labelled("Clear priority filter").hidden).toBe(true);
    expect(labelled("Clear status filter").hidden).toBe(true);
    expect(labelled("Priority filter").textContent).toBe("All priorities");

    choose("Priority filter", "High");
    expect(labelled("Priority filter").textContent).toBe("High");
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta"]);
    expect(ticketsIn("Alpha")).toEqual(["High ready", "High done"]);
    expect(ticketsIn("beta")).toEqual([]);
    expect(q('[aria-label="Tickets in beta"] .backlog-empty').textContent).toBe("No matching Tickets.");

    choose("Status filter", "Ready");
    expect(ticketsIn("Alpha")).toEqual(["High ready"]);
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta"]);

    labelled("Clear priority filter").click();
    expect(labelled("Priority filter").textContent).toBe("All priorities");
    expect(labelled("Clear priority filter").hidden).toBe(true);
    expect(root.activeElement).toBe(labelled("Priority filter"));
    expect(ticketsIn("Alpha")).toEqual(["High ready", "Low ready"]);

    labelled("Clear status filter").click();
    expect(labelled("Status filter").textContent).toBe("All statuses");
    expect(ticketsIn("Alpha")).toEqual(["High ready", "High done", "Medium in progress", "Low ready"]);
  });

  it("chooses All to clear a filter as well", async () => {
    const { choose, ticketsIn } = await mounted();
    choose("Status filter", "Done");
    expect(ticketsIn("Alpha")).toEqual(["High done"]);
    choose("Status filter", "All statuses");
    expect(ticketsIn("Alpha")).toHaveLength(4);
  });

  it("offers every priority and status in order", async () => {
    const { root, labelled } = await mounted();
    const options = (name: string) => {
      labelled(name).click();
      const popup = root.getElementById(labelled(name).getAttribute("aria-controls")!)!;
      const labels = [...popup.querySelectorAll('[role="option"]')].map((item) => item.textContent);
      labelled(name).click();
      return labels;
    };
    expect(options("Priority filter")).toEqual(["All priorities", "High", "Medium", "Low"]);
    expect(options("Status filter")).toEqual(["All statuses", "Not ready", "Ready", "In progress", "Done"]);
  });
});

describe("Backlog actions", () => {
  it("creates an Epic and a Ticket from the inline forms", async () => {
    const { q, labelled, backlog, epicTitles, ticketsIn } = await mounted();
    labelled("New Epic").click();
    const epicInput = q<HTMLInputElement>(".epic-create input");
    epicInput.value = "  Roadmap ";
    q<HTMLFormElement>(".epic-create").requestSubmit();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.create", payload: { title: "Roadmap" } });
    expect(q(".epic-create")).toBeNull();
    expect(epicTitles()).toEqual(["Alpha", "ALPHA", "beta", "Roadmap"]);

    labelled("Add Ticket to Roadmap").click();
    const ticketInput = q<HTMLInputElement>(".ticket-create input");
    ticketInput.value = "First step";
    q<HTMLFormElement>(".ticket-create").requestSubmit();
    await tick();
    const [epicId] = [...backlog.epics.values()].filter((epic) => epic.title === "Roadmap").map((epic) => epic.epicId);
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.ticket.create", payload: { epicId, title: "First step" } });
    expect(ticketsIn("Roadmap")).toEqual(["First step"]);
    // The new Ticket is selected.
    expect(q('.ticket-title[aria-current="true"]').textContent).toContain("First step");
    expect(q(".ticket-detail-title").textContent).toBe("First step");
  });

  it("renames, archives and restores an Epic through its menu", async () => {
    const { q, labelled, backlog } = await mounted();
    labelled("Actions for beta").click();
    expect(labelled("Actions for beta").getAttribute("aria-expanded")).toBe("true");
    [...q(".epic-actions").querySelectorAll("button")].find((b) => b.textContent === "Rename")!.click();
    const input = q<HTMLInputElement>(".epic-rename input");
    expect(input.value).toBe("beta");
    input.value = "Beta two";
    q<HTMLFormElement>(".epic-rename").requestSubmit();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.rename", payload: { epicId: BETA, title: "Beta two" } });

    labelled("Actions for Beta two").click();
    [...q(".epic-actions").querySelectorAll("button")].find((b) => b.textContent === "Archive")!.click();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.archive", payload: { epicId: BETA } });
    expect(labelled("Actions for Beta two")).toBeNull();

    q<HTMLButtonElement>(".archived-toggle").click();
    labelled("Actions for Beta two").click();
    [...q(".epic-actions").querySelectorAll("button")].find((b) => b.textContent === "Restore")!.click();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({ operation: "backlog.epic.restore", payload: { epicId: BETA } });
  });

  it("moves a Ticket through the Move dropdown with its baseVersion", async () => {
    const { root, q, labelled, backlog, choose, ticketsIn } = await mounted();
    const ticket = [...backlog.tickets.values()].find((t) => t.title === "High ready")!;
    const version = ticket.version;
    labelled("Actions for High ready").click();
    const move = labelled("Move High ready to");
    expect(move.getAttribute("role")).toBe("combobox");
    expect(move.textContent).toBe("Move to…");
    expect(root.activeElement).toBe(move);
    move.click();
    const popup = root.getElementById(move.getAttribute("aria-controls")!)!;
    // Every other Epic that is not archived, alphabetically.
    expect([...popup.querySelectorAll('[role="option"]')].map((o) => o.textContent)).toEqual(["ALPHA", "beta"]);
    move.click();
    choose("Move High ready to", "beta");
    await tick();
    expect(backlog.requests().at(-1)).toEqual({
      operation: "backlog.ticket.move", payload: { ticketId: ticket.ticketId, baseVersion: version, epicId: BETA },
    });
    expect(q(".ticket-actions")).toBeNull();
    expect(ticketsIn("beta")).toEqual(["High ready", "Beta medium"]);
  });

  it("archives and restores a Ticket with its current baseVersion", async () => {
    const { q, labelled, backlog, ticketsIn } = await mounted();
    const ticket = [...backlog.tickets.values()].find((t) => t.title === "Low ready")!;
    const first = ticket.version;
    labelled("Actions for Low ready").click();
    [...q(".ticket-actions").querySelectorAll("button")].find((b) => b.textContent === "Archive")!.click();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({
      operation: "backlog.ticket.archive", payload: { ticketId: ticket.ticketId, baseVersion: first },
    });
    expect(ticketsIn("Alpha")).not.toContain("Low ready");
    const second = ticket.version;
    expect(second).not.toBe(first);
    q<HTMLButtonElement>(".archived-toggle").click();
    labelled("Actions for Low ready").click();
    [...q(".ticket-actions").querySelectorAll("button")].find((b) => b.textContent === "Restore")!.click();
    await tick();
    expect(backlog.requests().at(-1)).toEqual({
      operation: "backlog.ticket.restore", payload: { ticketId: ticket.ticketId, baseVersion: second },
    });
  });

  it("closes an action menu on Escape and an outside click", async () => {
    const { root, q, labelled } = await mounted();
    const trigger = labelled("Actions for Alpha");
    trigger.click();
    expect(q(".epic-actions")).not.toBeNull();
    (root.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, composed: true }));
    expect(q(".epic-actions")).toBeNull();
    expect(root.activeElement).toBe(trigger);
    trigger.click();
    document.body.click();
    expect(q(".epic-actions")).toBeNull();
  });

  it("keeps the Ticket menu open when Escape closes its Move dropdown", async () => {
    const { q, labelled } = await mounted();
    labelled("Actions for High ready").click();
    const move = labelled("Move High ready to");
    move.click();
    move.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, composed: true, cancelable: true }));
    expect(move.getAttribute("aria-expanded")).toBe("false");
    expect(q(".ticket-actions")).not.toBeNull();
  });

  it("selects a Ticket and shows it in the detail pane", async () => {
    const { root, q } = await mounted();
    expect(q(".ticket-detail-placeholder").hidden).toBe(false);
    expect(q(".ticket-detail-content").hidden).toBe(true);
    const title = [...root.querySelectorAll<HTMLButtonElement>(".ticket-title")].find((b) => b.textContent?.startsWith("High done"))!;
    title.click();
    expect(q('.ticket-title[aria-current="true"]').textContent).toContain("High done");
    expect(q(".ticket-detail-placeholder").hidden).toBe(true);
    expect(q(".ticket-detail-title").textContent).toBe("High done");
  });

  it("shows a refusal as an alert", async () => {
    const { q, labelled, backlog } = await mounted();
    const ticket = [...backlog.tickets.values()].find((t) => t.title === "Low ready")!;
    backlog.remoteChange(ticket.ticketId, { status: "done" });
    labelled("Actions for Low ready").click();
    [...q(".ticket-actions").querySelectorAll("button")].find((b) => b.textContent === "Archive")!.click();
    await tick();
    expect(q("[role=alert]").hidden).toBe(false);
    expect(q("[role=alert]").textContent).toMatch(/changed since you saw it/);
  });

  it("removes its DOM on unmount", async () => {
    const { root, unmount } = await mounted();
    teardown = [];
    await unmount();
    expect(root.querySelector(".backlog-page")).toBeNull();
  });
});

describe("Typing in inline forms", () => {
  /** Types at the caret one key at a time, then delivers a remote snapshot, checking focus throughout. */
  async function typeThroughRemoteChange(root: ShadowRoot, backlog: FakeBacklog, input: HTMLInputElement, expected: string) {
    input.setSelectionRange(1, 1);
    const ticket = [...backlog.tickets.values()].find((t) => t.title === "Low ready")!;
    for (const [index, key] of [..."xyz"].entries()) {
      const at = input.selectionStart!;
      input.value = input.value.slice(0, at) + key + input.value.slice(at);
      input.setSelectionRange(at + 1, at + 1);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      if (index === 1) {
        backlog.remoteChange(ticket.ticketId, { status: "done" });
        await backlog.publish();
        await tick();
      }
      expect(root.activeElement).toBe(input);
      expect([input.selectionStart, input.selectionEnd]).toEqual([at + 1, at + 1]);
    }
    expect(input.isConnected).toBe(true);
    expect(input.value).toBe(expected);
  }

  it("keep focus, text and caret in New Epic, Add Ticket and Rename", async () => {
    const { root, q, labelled, backlog } = await mounted();
    labelled("New Epic").click();
    const epicInput = q<HTMLInputElement>(".epic-create input");
    expect(root.activeElement).toBe(epicInput);
    epicInput.value = "ab";
    await typeThroughRemoteChange(root, backlog, epicInput, "axyzb");

    labelled("Add Ticket to beta").click();
    const ticketInput = q<HTMLInputElement>(".ticket-create input");
    expect(root.activeElement).toBe(ticketInput);
    ticketInput.value = "ab";
    await typeThroughRemoteChange(root, backlog, ticketInput, "axyzb");

    labelled("Actions for beta").click();
    [...q(".epic-actions").querySelectorAll("button")].find((b) => b.textContent === "Rename")!.click();
    const renameInput = q<HTMLInputElement>(".epic-rename input");
    expect(root.activeElement).toBe(renameInput);
    await typeThroughRemoteChange(root, backlog, renameInput, "bxyzeta");
  });
});
