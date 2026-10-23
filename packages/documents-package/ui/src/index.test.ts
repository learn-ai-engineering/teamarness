// @vitest-environment happy-dom

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import type { DocumentsSnapshot } from "./index";
import { mount } from "./index";

const planning = "1".repeat(64);
const archive = "2".repeat(64);
const doc = "3".repeat(64);

function snapshot(): DocumentsSnapshot {
  return {
    availability: "current",
    warnings: [],
    collections: [
      { id: planning, name: "Planning", archived: false },
      { id: archive, name: "Old", archived: true },
    ],
    documents: [{ id: doc, collectionId: planning, title: "Q4 plan", archived: false }],
  };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function backend(current: () => DocumentsSnapshot) {
  const calls: { operation: string; payload: PackageValue }[] = [];
  const record = (operation: string) => (payload: PackageValue) => {
    calls.push({ operation, payload });
    return { ok: true as const, id: "9".repeat(64) };
  };
  const fake = createFakePackageClient({
    requests: {
      "documents.snapshot": () => current(),
      "documents.collection.create": record("documents.collection.create"),
      "documents.collection.rename": record("documents.collection.rename"),
      "documents.collection.archive": record("documents.collection.archive"),
      "documents.document.create": record("documents.document.create"),
      "documents.document.rename": record("documents.document.rename"),
      "documents.document.archive": record("documents.document.archive"),
      "documents.document.move": record("documents.document.move"),
    },
    subscriptions: { "documents.changes": () => undefined },
  });
  return { fake, calls };
}

function texts(root: ParentNode, selector: string): string[] {
  return [...root.querySelectorAll(selector)].map((element) => element.textContent ?? "");
}

describe("Documents mount", () => {
  it("lists Collections and Documents, hides archived until asked, and unmounts cleanly", async () => {
    const { fake } = backend(snapshot);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Documents" }));
    expect(root.querySelector("h1")?.textContent).toBe("Documents");
    expect(root.querySelector("[role=status]")?.textContent).toBe("Up to date");
    expect(texts(root, ".collection-button")).toEqual(["Planning"]);
    expect(root.querySelector(".collection-button")?.getAttribute("aria-current")).toBe("page");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan"]);
    const toggle = root.querySelector(".archived-toggle input") as HTMLInputElement;
    toggle.checked = true;
    toggle.dispatchEvent(new Event("change"));
    expect(texts(root, ".collection-button")).toEqual(["Planning", "Old"]);
    await unmount();
    expect(root.childElementCount).toBe(0);
  });

  it("sends create, rename, archive, and move requests with the selected Collection", async () => {
    const { fake, calls } = backend(snapshot);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));

    const collectionInput = root.querySelector(".collection-create input") as HTMLInputElement;
    collectionInput.value = "  Roadmap ";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.collection.create", payload: { name: "Roadmap" } });
    expect(collectionInput.value).toBe("");

    const documentInput = root.querySelector(".document-create input") as HTMLInputElement;
    documentInput.value = "Hiring plan";
    (root.querySelector(".document-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({
      operation: "documents.document.create",
      payload: { collectionId: planning, title: "Hiring plan" },
    });

    const renameButton = [...root.querySelectorAll(".document-actions button")].find((b) => b.textContent === "Rename") as HTMLButtonElement;
    renameButton.click();
    const renameInput = root.querySelector(".document-rename input") as HTMLInputElement;
    expect(renameInput.value).toBe("Q4 plan");
    renameInput.value = "Q4 plan v2";
    (root.querySelector(".document-rename") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.document.rename", payload: { documentId: doc, title: "Q4 plan v2" } });
    expect(root.querySelector(".document-rename")).toBeNull();

    const archiveButton = [...root.querySelectorAll(".document-actions button")].find((b) => b.textContent === "Archive") as HTMLButtonElement;
    archiveButton.click();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.document.archive", payload: { documentId: doc } });

    // Only non-archived other Collections are move destinations; "Old" is archived, so none.
    expect(root.querySelector(".document-move")).toBeNull();
    await unmount();
  });

  it("applies a pushed snapshot and keeps the selection when it survives", async () => {
    let current = snapshot();
    const { fake } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    current = {
      ...current,
      collections: [...current.collections, { id: "4".repeat(64), name: "Research", archived: false }],
      documents: [...current.documents, { id: "5".repeat(64), collectionId: planning, title: "OKRs", archived: false }],
    };
    await fake.emit("documents.changes", current);
    expect(texts(root, ".collection-button")).toEqual(["Planning", "Research"]);
    expect(root.querySelector(".collection-button")?.getAttribute("aria-current")).toBe("page");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "OKRs"]);
    expect(root.querySelector(".document-move")).not.toBeNull();
    // A Collection this Installation created is selected once its record is listed.
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "Mine";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Planning");
    current = { ...current, collections: [...current.collections, { id: "9".repeat(64), name: "Mine", archived: false }] };
    await fake.emit("documents.changes", current);
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Mine");
    expect(texts(root, ".document-title")).toEqual([]);
    expect(texts(root, ".documents-empty")).toEqual(["No documents yet."]);
    await unmount();
  });

  it("hides a selected archived Collection and its Documents until Archived is shown", async () => {
    const research = "4".repeat(64);
    let current = { ...snapshot(),
      collections: [...snapshot().collections, { id: research, name: "Research", archived: false }],
      documents: [...snapshot().documents, { id: "5".repeat(64), collectionId: research, title: "New plan", archived: false }],
    };
    const { fake } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    current = { ...current, collections: current.collections.map((collection) =>
      collection.id === planning ? { ...collection, archived: true } : collection) };
    await fake.emit("documents.changes", current);
    expect(texts(root, ".collection-button")).toEqual(["Research"]);
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Research");
    expect(texts(root, ".document-title")).toEqual(["New plan"]);
    const toggle = root.querySelector(".archived-toggle input") as HTMLInputElement;
    toggle.checked = true;
    toggle.dispatchEvent(new Event("change"));
    expect(texts(root, ".collection-button")).toContain("Planning");
    ([...root.querySelectorAll(".collection-button")].find((button) => button.textContent === "Planning") as HTMLButtonElement).click();
    expect(texts(root, ".document-title")).toEqual(["Q4 plan"]);
    await unmount();
  });

  it("opens a Document, sends typed edits, and applies a remote delta with the caret moved", async () => {
    const edits: { base: string; ops: unknown }[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
        "documents.document.edit": (payload) => {
          const { baseRevision, ops } = payload as { baseRevision: string; ops: unknown };
          edits.push({ base: baseRevision, ops });
          return { ok: true as const, revision: String(Number(baseRevision) + 1) };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    const pane = root.querySelector(".editor-pane") as HTMLElement;
    expect(pane.hidden).toBe(false);
    expect(root.querySelector(".editor-title")?.textContent).toBe("Q4 plan");
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: true, text: "# Q4\n" });
    const textarea = root.querySelector("textarea") as HTMLTextAreaElement;
    expect(textarea.value).toBe("# Q4\n");
    expect(textarea.disabled).toBe(false);
    textarea.value = "# Q4\n- hire\n";
    textarea.dispatchEvent(new Event("input"));
    await tick();
    expect(edits).toEqual([{ base: "0", ops: [{ retain: 5 }, { insert: "- hire\n" }] }]);
    await fake.emit("documents.document.text", { documentId: doc, revision: "1", editable: true, origin: "local", delta: [{ retain: 5 }, { insert: "- hire\n" }] });
    expect(textarea.value).toBe("# Q4\n- hire\n");
    textarea.focus();
    textarea.setSelectionRange(12, 12);
    await fake.emit("documents.document.text", { documentId: doc, revision: "2", editable: true, origin: "remote", delta: [{ insert: "> intro\n" }] });
    expect(textarea.value).toBe("> intro\n# Q4\n- hire\n");
    expect(textarea.selectionStart).toBe(20);
    ([...root.querySelectorAll(".editor-header button")].find((b) => b.textContent === "Close") as HTMLButtonElement).click();
    await tick();
    expect(pane.hidden).toBe(true);
    await unmount();
  });

  it("shows a terminal conflict warning without retrying or rendering quarantined text", async () => {
    let opens = 0;
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => { opens++; return { documentId: doc, revision: "0", editable: true, text: "secret" }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: true, text: "secret" });
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: false, reason: "identity_conflict", text: "" });
    expect(root.querySelector(".editor-status")?.textContent).toContain("Conflicting edit history");
    expect((root.querySelector(".editor-textarea") as HTMLTextAreaElement).disabled).toBe(true);
    expect((root.querySelector(".editor-textarea") as HTMLTextAreaElement).value).toBe("");
    ([...root.querySelectorAll(".editor-header button")].find((button) => button.textContent === "Preview") as HTMLButtonElement).click();
    expect(root.querySelector(".editor-preview")?.innerHTML).toBe("");
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(1);
    await unmount();
  });

  it("shows last-good text read-only with an import failure reference without retrying", async () => {
    let opens = 0;
    const record = "e".repeat(64);
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => { opens++; return { documentId: doc, revision: "1", editable: false,
          reason: "import_failure", diagnosticRecordId: record, text: "last good" }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await fake.emit("documents.document.text", { documentId: doc, revision: "1", editable: false,
      reason: "import_failure", diagnosticRecordId: record, text: "last good" });
    const textarea = root.querySelector(".editor-textarea") as HTMLTextAreaElement;
    expect(textarea.disabled).toBe(true);
    expect(textarea.value).toBe("last good");
    expect(root.querySelector(".editor-status")?.textContent).toContain("Incomplete edit history");
    expect(root.querySelector(".editor-status")?.textContent).toContain(record);
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(1);
    await unmount();
  });

  it("retries pending history after availability changes and enables the existing editor", async () => {
    let opens = 0;
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => ({ ...snapshot(), availability: "waiting_for_history_access" }),
        "documents.document.open": () => { opens++; return { documentId: doc, revision: "0", editable: opens > 1, text: "" }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: false, text: "" });
    expect((root.querySelector(".editor-textarea") as HTMLTextAreaElement).disabled).toBe(true);
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(2);
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: true, text: "" });
    expect((root.querySelector(".editor-textarea") as HTMLTextAreaElement).disabled).toBe(false);
    await unmount();
  });

  it("lists threaded comments for the open Document and adds one with a quote", async () => {
    const root = "a".repeat(64);
    const reply = "b".repeat(64);
    const comments: unknown[] = [
      { id: root, documentId: doc, author: "c".repeat(64), position: "01", resolved: false, isLocal: false, quote: "# Q4", markdown: "Needs **dates**" },
      { id: reply, documentId: doc, parentId: root, author: "me", position: "02", resolved: true, isLocal: true, markdown: "Added" },
    ];
    const added: unknown[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
        "documents.comments": () => ({ comments } as never),
        "documents.comment.add": (payload) => { added.push(payload); return { ok: true as const, id: "d".repeat(64) }; },
        "documents.comment.resolve": () => ({ ok: true as const, id: root }),
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined, "documents.names": () => undefined },
    });
    const rootNode = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root: rootNode }));
    (rootNode.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await fake.emit("documents.document.text", { documentId: doc, revision: "0", editable: true, text: "# Q4\nbody" });
    const items = [...rootNode.querySelectorAll(".comment")];
    expect(items).toHaveLength(2);
    expect(items[0]?.querySelector(".comment-quote")?.textContent).toBe("# Q4");
    expect(items[0]?.querySelector(".comment-body strong")?.textContent).toBe("dates");
    expect(items[1]?.querySelector(".comment-author")?.textContent).toBe("You");
    expect(items[0]?.querySelector(".comment-author")?.textContent).toBe("cccccccc…cccccc");
    const person = { identity: "c".repeat(64), personId: "c".repeat(64), name: "Carol", fingerprint: "cccccccccccc" };
    await fake.emit("documents.names", { available: true, cursor: "1", persons: [person], part: 0, parts: 1 });
    expect(rootNode.querySelector(".comment-author")?.textContent).toBe("Carol");
    await fake.emit("documents.names", { available: true, cursor: "2", persons: [{ ...person, name: "Carol R" }], part: 0, parts: 1 });
    expect(rootNode.querySelector(".comment-author")?.textContent).toBe("Carol R");
    // Preview renders the current draft and hides the editor.
    ([...rootNode.querySelectorAll(".editor-header button")].find((b) => b.textContent === "Preview") as HTMLButtonElement).click();
    expect((rootNode.querySelector(".editor-textarea") as HTMLElement).hidden).toBe(true);
    expect(rootNode.querySelector(".editor-preview h1")?.textContent).toBe("Q4");
    ([...rootNode.querySelectorAll(".editor-header button")].find((b) => b.textContent === "Edit") as HTMLButtonElement).click();
    expect((rootNode.querySelector(".editor-textarea") as HTMLElement).hidden).toBe(false);
    expect(items[1]?.classList.contains("is-resolved")).toBe(true);
    expect((items[1] as HTMLElement).style.getPropertyValue("--comment-depth")).toBe("1");

    const textarea = rootNode.querySelector(".editor-textarea") as HTMLTextAreaElement;
    textarea.setSelectionRange(0, 4);
    ([...rootNode.querySelectorAll(".comment-compose-actions button")].find((b) => b.textContent === "Quote selection") as HTMLButtonElement).click();
    expect(rootNode.querySelector(".comment-quote--draft")?.textContent).toContain("# Q4");
    const input = rootNode.querySelector(".comment-input") as HTMLTextAreaElement;
    input.value = "Agree";
    (rootNode.querySelector(".comment-compose") as HTMLFormElement).requestSubmit();
    await tick();
    expect(added).toEqual([{ documentId: doc, markdown: "Agree", quote: "# Q4" }]);
    expect(input.value).toBe("");
    await unmount();
  });

  it("loads older comment pages without stranding replies and displays author time", async () => {
    const parent = "4".repeat(64);
    const child = "5".repeat(64);
    const requests: unknown[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
        "documents.comments": (payload) => {
          requests.push(payload);
          return requests.length === 1
            ? { comments: [{ id: child, documentId: doc, parentId: parent, author: "bob", position: "02", resolved: false, isLocal: false, markdown: "Reply", createdAt: "1730000000000" }], nextCursor: "older", hasMore: true }
            : { comments: [{ id: parent, documentId: doc, author: "alice", position: "01", resolved: false, isLocal: false, markdown: "Parent" }], nextCursor: null, hasMore: false };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    expect(texts(root, ".comment-body").map((text) => text.trim())).toEqual(["Reply"]);
    expect(texts(root, ".comment-time")).toEqual([new Date(1730000000000).toLocaleString()]);
    (root.querySelector(".comments-load-older") as HTMLButtonElement).click();
    await tick();
    expect(requests).toEqual([{ documentId: doc }, { documentId: doc, cursor: "older" }]);
    expect(texts(root, ".comment-body").map((text) => text.trim())).toEqual(["Parent", "Reply"]);
    expect(texts(root, ".comment-time")[0]).toBe("Time unavailable");
    await unmount();
  });

  it("refreshes loaded older Comments after a remote resolve and reopen", async () => {
    const older = "4".repeat(64);
    const newest = "5".repeat(64);
    let resolved = false;
    const requests: unknown[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
        "documents.comments": (payload) => {
          requests.push(payload);
          return "cursor" in (payload as object)
            ? { comments: [{ id: older, documentId: doc, author: "bob", position: "01", resolved, isLocal: false, markdown: "Older" }], nextCursor: null, hasMore: false }
            : { comments: [{ id: newest, documentId: doc, author: "bob", position: "02", resolved: false, isLocal: false, markdown: "Newest" }], nextCursor: "older", hasMore: true };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    (root.querySelector(".comments-load-older") as HTMLButtonElement).click();
    await tick();
    expect(root.querySelector(".comment.is-resolved")).toBeNull();
    resolved = true;
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(texts(root, ".comment-body").map((text) => text.trim())).toEqual(["Older", "Newest"]);
    expect(root.querySelector(".comment.is-resolved .comment-body")?.textContent).toContain("Older");
    resolved = false;
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(root.querySelector(".comment.is-resolved")).toBeNull();
    expect(requests.filter((payload) => "cursor" in (payload as object))).toHaveLength(3);
    await unmount();
  });

  it("surfaces a request error without losing the listing", async () => {
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.collection.create": () => {
          throw Object.assign(new Error("invalid"), { kind: "teamarness.package-request-error", code: "documents.invalid_name" });
        },
      },
      subscriptions: { "documents.changes": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "x";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector("[role=alert]")?.textContent).toMatch(/Collection names/);
    expect(texts(root, ".collection-button")).toEqual(["Planning"]);
    await unmount();
  });
});
