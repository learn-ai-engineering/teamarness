// @vitest-environment happy-dom

import { createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import { afterEach, describe, expect, it } from "vitest";
import { mount } from "./index";
import { createFakeBacklog, order, recordId } from "./test-support/fake-backend";
import type { FakeBacklog } from "./test-support/fake-backend";

const EPIC = recordId(1);
const ALICE = { principalId: "a".repeat(32), identity: "1".repeat(64), kind: "person", name: "Alice", fingerprint: "111111111111" };
const BOB = { principalId: "b".repeat(32), identity: "2".repeat(64), kind: "person", name: "Bob", fingerprint: "222222222222" };
const HELPER = { principalId: "c".repeat(32), identity: "3".repeat(64), kind: "agent", name: "Helper", fingerprint: "333333333333" };
const FORMER = "f0e1d2c3b4a5968778695a4b3c2d1e0f";
const CREATED = Date.UTC(2026, 9, 30, 9, 15);
const EDITED = Date.UTC(2026, 9, 31, 16, 40);

function seed() {
  return {
    epics: [{ epicId: EPIC, title: "Launch", archived: false, createdOrder: order(1) }],
    tickets: [
      {
        epicId: EPIC, title: "Write the guide", body: "# Steps\n\n- [x] outline\n- [ ] draft\n\n| A | B |\n| - | - |\n| 1 | 2 |",
        status: "ready" as const, priority: "high" as const, assignee: ALICE.principalId,
        creator: ALICE.identity, createdAt: CREATED, lastEditor: HELPER.identity, lastAt: EDITED,
      },
      { epicId: EPIC, title: "Old work", assignee: FORMER, creator: "9".repeat(64), lastEditor: "9".repeat(64) },
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

async function settle(): Promise<void> {
  for (let round = 0; round < 4; round += 1) await tick();
}

/** Mounts the Backlog, delivers its first snapshot and a roster, and selects `title`. */
async function opened(title: string, backlog: FakeBacklog = createFakeBacklog(seed()), roster = [ALICE, BOB, HELPER]) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = host.attachShadow({ mode: "open" });
  const unmount = await mount(createTestMountContext({ client: backlog.client, root, displayName: "Backlog" }));
  teardown.push(async () => { await unmount(); host.remove(); });
  await backlog.publish();
  await backlog.fake.emit("backlog.names", { available: true, cursor: "3", part: 0, parts: 1, principals: roster } as unknown as PackageValue);
  const q = <E extends Element = HTMLElement>(selector: string) => root.querySelector(selector) as E;
  const labelled = <E extends Element = HTMLButtonElement>(label: string) => q<E>(`[aria-label="${label}"]`);
  const detail = () => q(".ticket-detail");
  const ticket = [...backlog.tickets.values()].find((candidate) => candidate.title === title)!;
  [...root.querySelectorAll<HTMLButtonElement>(".ticket-title")].find((button) => button.textContent?.startsWith(title))!.click();
  await settle();
  const options = (name: string) => {
    const popup = root.getElementById(labelled(name).getAttribute("aria-controls")!)!;
    return [...popup.querySelectorAll<HTMLElement>('[role="option"]')];
  };
  const choose = (name: string, label: string) => {
    labelled(name).click();
    const option = options(name).find((item) => item.textContent === label);
    if (!option) throw new Error(`no option ${label} in ${name}`);
    option.click();
  };
  const button = (text: string) => [...detail().querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === text) ?? null;
  const type = (element: HTMLInputElement | HTMLTextAreaElement, value: string) => {
    element.value = value;
    element.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const writes = () => backlog.requests().filter((call) => call.operation === "backlog.ticket.update"
    || call.operation === "backlog.ticket.assign");
  return { root, backlog, q, labelled, detail, ticket, options, choose, button, type, writes };
}

describe("Ticket detail", () => {
  it("shows every field: title, rendered body, status, priority, assignee and attribution", async () => {
    const { q, labelled, detail, backlog, ticket } = await opened("Write the guide");
    expect(backlog.reads()).toContain(ticket.ticketId);
    expect(q(".ticket-detail-title").textContent).toBe("Write the guide");
    expect(labelled("Status").textContent).toBe("Ready");
    expect(labelled("Priority").textContent).toBe("High");
    expect(labelled("Assignee").textContent).toBe("Alice");
    const body = q(".ticket-body");
    expect(body.querySelector("h1")?.textContent).toBe("Steps");
    expect(body.querySelector("table")).not.toBeNull();
    const boxes = [...body.querySelectorAll("input")];
    expect(boxes.map((box) => [box.type, box.disabled, box.checked])).toEqual([["checkbox", true, true], ["checkbox", true, false]]);
    const created = q(".ticket-created");
    expect(created.textContent).toContain("Created by Alice");
    expect(created.querySelector("time")?.getAttribute("datetime")).toBe(new Date(CREATED).toISOString());
    const changed = q(".ticket-changed");
    expect(changed.textContent).toContain("Last changed by Helper");
    expect(changed.querySelector("time")?.getAttribute("datetime")).toBe(new Date(EDITED).toISOString());
    // Viewing shows no editor.
    expect(detail().querySelector("textarea")).toBeNull();
    expect(detail().querySelector("select")).toBeNull();
  });

  it("swaps in a raw Markdown text area on Edit, and Cancel discards the draft", async () => {
    const { q, labelled, detail, button, type, writes, ticket } = await opened("Write the guide");
    button("Edit")!.click();
    const area = q<HTMLTextAreaElement>("textarea.ticket-body-input");
    expect(area.classList.contains("teamarness-input")).toBe(true);
    expect(area.value).toBe(ticket.body);
    expect(q(".ticket-body")).toBeNull();
    expect(labelled<HTMLInputElement>("Ticket title").value).toBe("Write the guide");
    type(area, "Changed body");
    button("Cancel")!.click();
    expect(detail().querySelector("textarea")).toBeNull();
    expect(q(".ticket-body").textContent).toContain("Steps");
    expect(writes()).toEqual([]);
    // Edit starts again from the stored body.
    button("Edit")!.click();
    expect(q<HTMLTextAreaElement>("textarea.ticket-body-input").value).toBe(ticket.body);
  });

  it("saves only the changed fields, then the assignee, each with its base version", async () => {
    const { q, labelled, button, type, choose, writes, ticket, detail } = await opened("Write the guide");
    const base = ticket.version;
    button("Edit")!.click();
    type(q<HTMLTextAreaElement>("textarea.ticket-body-input"), "New body with **bold**");
    choose("Status", "In progress");
    choose("Assignee", "Bob");
    button("Save")!.click();
    await settle();
    const [update, assign] = writes();
    expect(update).toEqual({ operation: "backlog.ticket.update",
      payload: { ticketId: ticket.ticketId, baseVersion: base, body: "New body with **bold**", status: "in_progress" } });
    expect(assign?.operation).toBe("backlog.ticket.assign");
    const afterUpdate = (assign!.payload as { baseVersion: string }).baseVersion;
    expect(afterUpdate > base).toBe(true);
    expect(assign!.payload).toEqual({ ticketId: ticket.ticketId, baseVersion: afterUpdate, assignee: BOB.principalId });
    expect(writes()).toHaveLength(2);
    expect(detail().querySelector("textarea")).toBeNull();
    expect(q(".ticket-body strong")?.textContent).toBe("bold");
    expect(labelled("Status").textContent).toBe("In progress");
    expect(labelled("Assignee").textContent).toBe("Bob");
  });

  it("changes title and priority alone, trimming the title, and sends no assign", async () => {
    const { labelled, button, type, choose, writes, ticket } = await opened("Write the guide");
    // Choosing from a dropdown starts an edit.
    choose("Priority", "Low");
    expect(button("Save")).not.toBeNull();
    type(labelled<HTMLInputElement>("Ticket title"), "  Write the user guide ");
    button("Save")!.click();
    await settle();
    expect(writes()).toEqual([{ operation: "backlog.ticket.update",
      payload: { ticketId: ticket.ticketId, baseVersion: expect.any(String), title: "Write the user guide", priority: "low" } }]);
  });

  it("saves nothing when nothing changed, and refuses an empty title", async () => {
    const { labelled, button, type, writes, root, detail } = await opened("Write the guide");
    button("Edit")!.click();
    button("Save")!.click();
    await settle();
    expect(writes()).toEqual([]);
    expect(detail().querySelector("textarea")).toBeNull();
    button("Edit")!.click();
    type(labelled<HTMLInputElement>("Ticket title"), "   ");
    button("Save")!.click();
    await settle();
    expect(writes()).toEqual([]);
    expect((root.querySelector("[role=alert]") as HTMLElement).hidden).toBe(false);
    expect(detail().querySelector("textarea")).not.toBeNull();
  });
});

describe("Live changes", () => {
  it("refresh the pane while viewing and keep a draft while editing", async () => {
    const { q, button, type, backlog, ticket, labelled } = await opened("Write the guide");
    backlog.remoteChange(ticket.ticketId, { body: "Remote **body**", lastEditor: BOB.identity });
    await backlog.publish();
    await settle();
    expect(q(".ticket-body strong")?.textContent).toBe("body");
    expect(q(".ticket-changed").textContent).toContain("Last changed by Bob");

    button("Edit")!.click();
    type(q<HTMLTextAreaElement>("textarea.ticket-body-input"), "Draft text");
    backlog.remoteChange(ticket.ticketId, { priority: "low", lastEditor: ALICE.identity });
    await backlog.publish();
    await settle();
    expect(q<HTMLTextAreaElement>("textarea.ticket-body-input").value).toBe("Draft text");
    // The draft still shows the values it started from; attribution follows the latest read.
    expect(labelled("Priority").textContent).toBe("High");
    expect(q(".ticket-changed").textContent).toContain("Last changed by Alice");
  });
});

describe("Ticket assignee", () => {
  it("offers Unassigned and current Persons only, never an Agent", async () => {
    const { options, labelled } = await opened("Write the guide");
    labelled("Assignee").click();
    expect(options("Assignee").map((option) => option.textContent)).toEqual(["Unassigned", "Alice", "Bob"]);
  });

  it("shows a missing assignee as a former member and lets it be reassigned", async () => {
    const { labelled, options, choose, button, writes, ticket, q } = await opened("Old work");
    const base = ticket.version;
    expect(labelled("Assignee").textContent).toBe(`Former member · ${FORMER.slice(0, 12)}`);
    labelled("Assignee").click();
    expect(options("Assignee").map((option) => option.textContent))
      .toEqual(["Unassigned", "Alice", "Bob", `Former member · ${FORMER.slice(0, 12)}`]);
    labelled("Assignee").click();
    // An unknown signer is also shown by its short identity.
    expect(q(".ticket-created").textContent).toContain(`Former member · ${"9".repeat(12)}`);
    choose("Assignee", "Unassigned");
    button("Save")!.click();
    await settle();
    expect(writes()).toEqual([{ operation: "backlog.ticket.assign",
      payload: { ticketId: ticket.ticketId, baseVersion: base, assignee: null } }]);
    expect(labelled("Assignee").textContent).toBe("Unassigned");
  });

  it("reassembles a roster sent in parts", async () => {
    const backlog = createFakeBacklog(seed());
    const { labelled, backlog: fake } = await opened("Write the guide", backlog, []);
    expect(labelled("Assignee").textContent).toBe(`Former member · ${ALICE.principalId.slice(0, 12)}`);
    await fake.fake.emit("backlog.names", { available: true, cursor: "4", part: 0, parts: 2, principals: [BOB] } as unknown as PackageValue);
    expect(labelled("Assignee").textContent).toBe(`Former member · ${ALICE.principalId.slice(0, 12)}`);
    await fake.fake.emit("backlog.names", { available: true, cursor: "4", part: 1, parts: 2, principals: [ALICE] } as unknown as PackageValue);
    expect(labelled("Assignee").textContent).toBe("Alice");
  });
});

describe("Stale saves", () => {
  it("keep the draft, show the current values, and Use latest reapplies the draft on the new version", async () => {
    const { q, labelled, button, type, writes, backlog, ticket, detail } = await opened("Write the guide");
    button("Edit")!.click();
    type(q<HTMLTextAreaElement>("textarea.ticket-body-input"), "My careful rewrite");
    // Another Installation (or an Agent) changes the status first.
    backlog.remoteChange(ticket.ticketId, { status: "done", title: "Write the guide v2" });
    const remoteVersion = ticket.version;
    button("Save")!.click();
    await settle();
    expect(writes()).toHaveLength(1);
    const notice = q(".ticket-stale");
    expect(notice.hidden).toBe(false);
    expect(notice.textContent).toContain("changed since you started editing");
    expect(notice.textContent).toContain("Write the guide v2");
    expect(notice.textContent).toContain("Done");
    // The draft is kept.
    expect(q<HTMLTextAreaElement>("textarea.ticket-body-input").value).toBe("My careful rewrite");
    expect((q("[role=alert]")).hidden).toBe(true);

    button("Use latest")!.click();
    await settle();
    expect(notice.hidden).toBe(true);
    expect(q<HTMLTextAreaElement>("textarea.ticket-body-input").value).toBe("My careful rewrite");
    // Fields the Person did not change follow the latest Ticket.
    expect(labelled<HTMLInputElement>("Ticket title").value).toBe("Write the guide v2");
    expect(labelled("Status").textContent).toBe("Done");
    button("Save")!.click();
    await settle();
    expect(writes().at(-1)).toEqual({ operation: "backlog.ticket.update",
      payload: { ticketId: ticket.ticketId, baseVersion: remoteVersion, body: "My careful rewrite" } });
    expect(detail().querySelector("textarea")).toBeNull();
    expect(backlog.tickets.get(ticket.ticketId)?.status).toBe("done");
    expect(backlog.tickets.get(ticket.ticketId)?.body).toBe("My careful rewrite");
  });

  it("reads the body when the refusal left it out", async () => {
    const { q, button, type, backlog, ticket } = await opened("Write the guide");
    backlog.omitStaleBody = true;
    button("Edit")!.click();
    type(q<HTMLTextAreaElement>("textarea.ticket-body-input"), "Mine");
    backlog.remoteChange(ticket.ticketId, { body: "Theirs" });
    button("Save")!.click();
    await settle();
    const notice = q(".ticket-stale");
    expect(notice.hidden).toBe(false);
    expect(notice.querySelector(".ticket-stale-body")?.textContent).toBe("Theirs");
  });
});

/**
 * Types `text` one character at a time at the caret, as a browser does: insert, move the caret,
 * then fire `input`. After each key, checks focus, value and caret, and that no blur happened.
 */
function typeKeys(root: ShadowRoot, element: HTMLInputElement | HTMLTextAreaElement, text: string): void {
  let blurs = 0;
  const onBlur = () => { blurs += 1; };
  element.addEventListener("blur", onBlur);
  try {
    for (const key of text) {
      const at = element.selectionStart ?? element.value.length;
      const expected = element.value.slice(0, at) + key + element.value.slice(element.selectionEnd ?? at);
      element.value = expected;
      element.setSelectionRange(at + 1, at + 1);
      element.dispatchEvent(new Event("input", { bubbles: true }));
      expect(root.activeElement).toBe(element);
      expect(blurs).toBe(0);
      expect(element.isConnected).toBe(true);
      expect(element.value).toBe(expected);
      expect([element.selectionStart, element.selectionEnd]).toEqual([at + 1, at + 1]);
    }
  } finally {
    element.removeEventListener("blur", onBlur);
  }
}

describe("Typing in edit mode", () => {
  it("keeps focus, text and caret in the body text area on every key", async () => {
    const { root, q, button, ticket } = await opened("Write the guide");
    button("Edit")!.click();
    const area = q<HTMLTextAreaElement>("textarea.ticket-body-input");
    expect(root.activeElement).toBe(area);
    // Type in the middle of the body, so a moved caret would show.
    area.setSelectionRange(2, 2);
    typeKeys(root, area, "abc");
    expect(area.value).toBe(`${ticket.body.slice(0, 2)}abc${ticket.body.slice(2)}`);
  });

  it("keeps focus, text and caret in the title input on every key", async () => {
    const { root, labelled, button } = await opened("Write the guide");
    button("Edit")!.click();
    const title = labelled<HTMLInputElement>("Ticket title");
    title.focus();
    title.setSelectionRange(5, 5);
    typeKeys(root, title, "xyz");
    expect(title.value).toBe("Writexyz the guide");
  });

  it("keeps focus, text and caret when a remote change arrives mid-typing", async () => {
    const { root, q, button, backlog, ticket } = await opened("Write the guide");
    button("Edit")!.click();
    const area = q<HTMLTextAreaElement>("textarea.ticket-body-input");
    area.setSelectionRange(0, 0);
    typeKeys(root, area, "ab");
    let blurs = 0;
    area.addEventListener("blur", () => { blurs += 1; });
    backlog.remoteChange(ticket.ticketId, { priority: "low" });
    await backlog.publish();
    await settle();
    expect(root.activeElement).toBe(area);
    expect(blurs).toBe(0);
    expect([area.selectionStart, area.selectionEnd]).toEqual([2, 2]);
    typeKeys(root, area, "c");
    expect(area.value).toBe(`abc${ticket.body}`);
  });
});
