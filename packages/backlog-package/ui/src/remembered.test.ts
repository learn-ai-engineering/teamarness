// @vitest-environment happy-dom

import { createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { typedPackageClient } from "@teamarness/package-ui-sdk";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BacklogContract, Draft, RememberedState, Ticket } from "./contract.generated";
import { BacklogController } from "./controller";
import { mount } from "./index";
import { MAX_DRAFTS, REMEMBERED_BUDGET, RememberedStateSync, boundState, draftOf, restoreEdit } from "./remembered";
import { createFakeBacklog, order, recordId } from "./test-support/fake-backend";
import type { FakeBacklog } from "./test-support/fake-backend";

const ALPHA = recordId(1);
const BETA = recordId(2);

function seed() {
  return {
    epics: [
      { epicId: ALPHA, title: "Alpha", archived: false, createdOrder: order(1) },
      { epicId: BETA, title: "Beta", archived: false, createdOrder: order(2) },
    ],
    tickets: [
      { epicId: ALPHA, title: "Write the guide", body: "Old body", priority: "high" as const, status: "ready" as const },
      { epicId: ALPHA, title: "Fix the build", body: "Build body", priority: "high" as const, status: "ready" as const },
      { epicId: BETA, title: "Beta work", body: "", priority: "low" as const },
    ],
  };
}

let teardown: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const step of teardown) await step();
  teardown = [];
  vi.useRealTimers();
});

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

async function settle(): Promise<void> {
  for (let round = 0; round < 4; round += 1) await tick();
}

function ticketId(backlog: FakeBacklog, title: string): string {
  return [...backlog.tickets.values()].find((ticket) => ticket.title === title)!.ticketId;
}

/** Mounts the Backlog against `backlog` and delivers its first snapshot. */
async function mounted(backlog: FakeBacklog) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = host.attachShadow({ mode: "open" });
  const unmount = await mount(createTestMountContext({ client: backlog.client, root, displayName: "Backlog" }));
  let unmounted = false;
  const close = async () => {
    if (unmounted) return;
    unmounted = true;
    await unmount();
    host.remove();
  };
  teardown.push(close);
  await backlog.publish();
  await settle();
  const q = <E extends Element = HTMLElement>(selector: string) => root.querySelector(selector) as E;
  const labelled = <E extends Element = HTMLButtonElement>(label: string) => q<E>(`[aria-label="${label}"]`);
  const choose = (name: string, label: string) => {
    labelled(name).click();
    const popup = root.getElementById(labelled(name).getAttribute("aria-controls")!)!;
    const option = [...popup.querySelectorAll<HTMLElement>('[role="option"]')].find((item) => item.textContent === label);
    if (!option) throw new Error(`no option ${label} in ${name}`);
    option.click();
  };
  const select = async (title: string) => {
    [...root.querySelectorAll<HTMLButtonElement>(".ticket-title")].find((b) => b.textContent?.startsWith(title))!.click();
    await settle();
  };
  const epicToggle = (title: string) => [...root.querySelectorAll<HTMLButtonElement>(".epic-button")]
    .find((b) => b.querySelector(".epic-title")?.textContent === title)!;
  const button = (text: string) => [...q(".ticket-detail").querySelectorAll<HTMLButtonElement>("button")]
    .find((b) => b.textContent === text) ?? null;
  const type = (element: HTMLInputElement | HTMLTextAreaElement, value: string) => {
    element.value = value;
    element.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const area = () => q<HTMLTextAreaElement>("textarea.ticket-body-input");
  const current = () => q(".ticket-title[aria-current=true]")?.textContent ?? null;
  const operations = () => backlog.fake.calls.map((call) => call.operation);
  return { root, q, labelled, choose, select, epicToggle, button, type, area, current, operations, unmount: close };
}

describe("remembered state across a Package switch", () => {
  it("restores selection, edit mode, draft text, filters, show-archived and expansion", async () => {
    const backlog = createFakeBacklog(seed());
    const first = await mounted(backlog);
    first.choose("Priority filter", "High");
    first.choose("Status filter", "Ready");
    first.labelled("Show archived").click();
    first.epicToggle("Beta").click();
    await first.select("Write the guide");
    first.button("Edit")!.click();
    first.type(first.area(), "Half-written body");
    // Unmounting before the debounce elapses still sends the state.
    await first.unmount();

    const id = ticketId(backlog, "Write the guide");
    const old = backlog.tickets.get(id)!;
    expect(backlog.remembered).toEqual({
      selectedTicketId: id,
      editing: true,
      filters: { priority: "high", status: "ready" },
      showArchived: true,
      expandedEpicIds: [ALPHA],
      drafts: { [id]: { body: "Half-written body", baseVersion: old.version } },
    });

    const second = await mounted(backlog);
    expect(second.labelled("Priority filter").textContent).toBe("High");
    expect(second.labelled("Status filter").textContent).toBe("Ready");
    expect(second.labelled("Hide archived").getAttribute("aria-pressed")).toBe("true");
    expect(second.epicToggle("Alpha").getAttribute("aria-expanded")).toBe("true");
    expect(second.epicToggle("Beta").getAttribute("aria-expanded")).toBe("false");
    expect(second.current()).toContain("Write the guide");
    expect(second.area().value).toBe("Half-written body");
    expect(second.button("Save")?.hidden).toBe(false);
    expect(second.q(".ticket-stale").hidden).toBe(true);
    // Save sends only the remembered change, against the version the draft started from.
    second.button("Save")!.click();
    await settle();
    expect(backlog.tickets.get(id)!.body).toBe("Half-written body");
    expect(backlog.tickets.get(id)!.title).toBe("Write the guide");
  });

  it("restores view mode with a remembered selection and no draft", async () => {
    const backlog = createFakeBacklog(seed());
    const first = await mounted(backlog);
    await first.select("Fix the build");
    await first.unmount();
    const second = await mounted(backlog);
    expect(second.current()).toContain("Fix the build");
    expect(second.area()).toBeNull();
    expect(second.q(".ticket-body").textContent).toContain("Build body");
  });

  it("shows the stale notice when the Ticket changed while the Person was away, keeping the draft", async () => {
    const backlog = createFakeBacklog(seed());
    const first = await mounted(backlog);
    await first.select("Write the guide");
    first.button("Edit")!.click();
    first.type(first.area(), "My body");
    await first.unmount();

    const id = ticketId(backlog, "Write the guide");
    backlog.remoteChange(id, { title: "Renamed elsewhere", status: "done" });
    const second = await mounted(backlog);
    expect(second.area().value).toBe("My body");
    expect(second.q(".ticket-stale").hidden).toBe(false);
    second.button("Use latest")!.click();
    await settle();
    expect(second.q(".ticket-stale").hidden).toBe(true);
    expect(second.area().value).toBe("My body");
    second.button("Save")!.click();
    await settle();
    // The other change stays; only the Person's body is written.
    expect(backlog.tickets.get(id)).toMatchObject({ title: "Renamed elsewhere", status: "done", body: "My body" });
  });

  it("keeps a draft per Ticket when the Person switches Tickets", async () => {
    const backlog = createFakeBacklog(seed());
    const view = await mounted(backlog);
    await view.select("Write the guide");
    view.button("Edit")!.click();
    view.type(view.area(), "Guide draft");
    await view.select("Fix the build");
    expect(view.area()).toBeNull();
    await view.select("Write the guide");
    expect(view.area().value).toBe("Guide draft");
  });
});

describe("remembered state is sent debounced", () => {
  it("coalesces rapid typing into one put and flushes a pending put on unmount", async () => {
    const backlog = createFakeBacklog(seed());
    const view = await mounted(backlog);
    await view.select("Write the guide");
    // Let the selection's own put go out first.
    await new Promise((resolve) => setTimeout(resolve, 350));
    vi.useFakeTimers();
    const puts = () => view.operations().filter((operation) => operation === "backlog.draft.put").length;
    const before = puts();
    view.button("Edit")!.click();
    for (const text of ["H", "He", "Hel", "Hell", "Hello"]) {
      view.type(view.area(), text);
      await vi.advanceTimersByTimeAsync(100);
    }
    expect(puts()).toBe(before);
    await vi.advanceTimersByTimeAsync(300);
    expect(puts()).toBe(before + 1);
    expect(Object.values(backlog.remembered!.drafts)[0]?.body).toBe("Hello");

    view.type(view.area(), "Hello there");
    expect(puts()).toBe(before + 1);
    vi.useRealTimers();
    await view.unmount();
    expect(puts()).toBe(before + 2);
    expect(Object.values(backlog.remembered!.drafts)[0]?.body).toBe("Hello there");
  });
});

describe("Save and Cancel clear the Ticket's draft only", () => {
  async function twoDrafts() {
    const backlog = createFakeBacklog(seed());
    const view = await mounted(backlog);
    const guide = ticketId(backlog, "Write the guide");
    const build = ticketId(backlog, "Fix the build");
    await view.select("Fix the build");
    view.button("Edit")!.click();
    view.type(view.area(), "Build draft");
    await view.select("Write the guide");
    view.button("Edit")!.click();
    view.type(view.area(), "Guide draft");
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(Object.keys(backlog.remembered!.drafts).sort()).toEqual([guide, build].sort());
    return { backlog, view, guide, build };
  }

  it("Cancel clears the open Ticket's draft and keeps the other", async () => {
    const { backlog, view, guide, build } = await twoDrafts();
    view.button("Cancel")!.click();
    await settle();
    const clears = backlog.fake.calls.filter((call) => call.operation === "backlog.draft.clear");
    expect(clears.map((call) => call.payload)).toEqual([{ key: guide }]);
    expect(Object.keys(backlog.remembered!.drafts)).toEqual([build]);
    await view.unmount();
    expect(Object.keys(backlog.remembered!.drafts)).toEqual([build]);
    expect(backlog.remembered!.editing).toBe(false);
  });

  it("Save clears the saved Ticket's draft and keeps the other", async () => {
    const { backlog, view, guide, build } = await twoDrafts();
    view.button("Save")!.click();
    await settle();
    const clears = backlog.fake.calls.filter((call) => call.operation === "backlog.draft.clear");
    expect(clears.map((call) => call.payload)).toEqual([{ key: guide }]);
    expect(Object.keys(backlog.remembered!.drafts)).toEqual([build]);
    expect(backlog.tickets.get(guide)!.body).toBe("Guide draft");
    await view.unmount();
    expect(Object.keys(backlog.remembered!.drafts)).toEqual([build]);
  });

  it("a stale refusal keeps the draft remembered", async () => {
    const backlog = createFakeBacklog(seed());
    const view = await mounted(backlog);
    const guide = ticketId(backlog, "Write the guide");
    await view.select("Write the guide");
    view.button("Edit")!.click();
    view.type(view.area(), "Mine");
    backlog.remoteChange(guide, { status: "done" });
    view.button("Save")!.click();
    await settle();
    expect(backlog.fake.calls.some((call) => call.operation === "backlog.draft.clear")).toBe(false);
    await view.unmount();
    expect(backlog.remembered!.drafts[guide]?.body).toBe("Mine");
  });
});

describe("remembered helpers", () => {
  const ticket: Ticket = {
    ticketId: recordId(9), epicId: ALPHA, title: "T", body: "B", status: "ready", priority: "high", assignee: null,
    archived: false, version: order(9), creator: "c", createdAt: 1, lastEditor: "c", lastAt: 1,
  };

  it("remembers only changed fields, including a cleared assignee", () => {
    const base = { ...ticket, assignee: "a".repeat(32) };
    expect(draftOf({ base, draft: { title: "T", body: "New", status: "ready", priority: "low", assignee: null }, stale: null }))
      .toEqual({ baseVersion: order(9), body: "New", priority: "low", assignee: null });
  });

  it("restores onto the current Ticket, stale only when it changed and the draft has changes", () => {
    const stored: Draft = { baseVersion: order(9), body: "Mine" };
    expect(restoreEdit(stored, ticket)).toEqual({ base: ticket, draft: { title: "T", body: "Mine", status: "ready", priority: "high", assignee: null }, stale: null });
    const latest = { ...ticket, title: "Other", version: order(10) };
    const restored = restoreEdit(stored, latest);
    expect(restored.stale).toBe(latest);
    expect(restored.base.version).toBe(order(9));
    expect(restored.draft).toMatchObject({ title: "Other", body: "Mine" });
    expect(restoreEdit({ baseVersion: order(9) }, latest).stale).toBeNull();
  });

  it("drops the oldest drafts first to stay within the count and size bounds, keeping the open one", () => {
    const drafts: Record<string, Draft> = {};
    for (let n = 1; n <= MAX_DRAFTS + 2; n += 1) drafts[recordId(n)] = { baseVersion: order(n), body: "x" };
    const state: RememberedState = { filters: { priority: null, status: null }, showArchived: false, expandedEpicIds: [], drafts };
    const counted = boundState(state, recordId(1));
    expect(Object.keys(counted.drafts)).toHaveLength(MAX_DRAFTS);
    expect(counted.drafts[recordId(1)]).toBeDefined();
    expect(counted.drafts[recordId(2)]).toBeUndefined();
    expect(counted.drafts[recordId(3)]).toBeUndefined();

    const big = "é".repeat(30_000);
    const large: Record<string, Draft> = {};
    for (let n = 1; n <= 8; n += 1) large[recordId(n)] = { baseVersion: order(n), body: big, title: "t".repeat(300) };
    const sized = boundState({ ...state, drafts: large }, recordId(8));
    expect(new TextEncoder().encode(JSON.stringify(sized)).length).toBeLessThanOrEqual(REMEMBERED_BUDGET);
    expect(sized.drafts[recordId(8)]).toBeDefined();
    expect(sized.drafts[recordId(1)]).toBeUndefined();
    expect([...sized.drafts[recordId(8)]!.title!]).toHaveLength(256);
  });

  it("restoring before the first listing keeps the remembered expansion", () => {
    const backlog = createFakeBacklog(seed());
    const controller = new BacklogController(typedPackageClient<BacklogContract>(backlog.client), () => undefined);
    controller.restore({ filters: { priority: null, status: null }, showArchived: false, expandedEpicIds: [BETA], drafts: {} });
    controller.applySnapshot(backlog.snapshot());
    expect([...controller.state.expandedEpicIds]).toEqual([BETA]);
  });

  it("a closed sync sends nothing more", async () => {
    const backlog = createFakeBacklog(seed());
    const state: RememberedState = { filters: { priority: null, status: null }, showArchived: true, expandedEpicIds: [], drafts: {} };
    const sync = new RememberedStateSync(typedPackageClient<BacklogContract>(backlog.client), () => ({ state, keep: null }));
    await sync.close();
    sync.changed();
    sync.clear(recordId(1));
    await sync.flush();
    expect(backlog.fake.calls).toEqual([]);
  });
});
