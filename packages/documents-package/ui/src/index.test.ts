// @vitest-environment happy-dom
/// <reference types="node" />
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import type { DocumentsSnapshot } from "./index";
import { mount } from "./index";
import documentsCss from "./styles.css?raw";

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
      "documents.collection.restore": record("documents.collection.restore"),
      "documents.document.create": record("documents.document.create"),
      "documents.document.rename": record("documents.document.rename"),
      "documents.document.archive": record("documents.document.archive"),
      "documents.document.restore": record("documents.document.restore"),
      "documents.document.move": record("documents.document.move"),
    },
    subscriptions: { "documents.changes": () => undefined },
  });
  return { fake, calls };
}

/** The Rust-written body holding every node and mark. */
const ALL_NODES = new Uint8Array(readFileSync(join(import.meta.dirname, "../../fixtures/body-trees/from-rust/01-all-nodes.loro")));
const mac = /Mac|iP(hone|[oa]d)/.test(globalThis.navigator?.platform ?? "");

/** The Backend opens the editor session of `doc`: `begin`, the snapshot (none by default) and `end`. */
async function opened(fake: ReturnType<typeof createFakePackageClient>, state: {
  editable?: boolean; reason?: "identity_conflict" | "import_failure"; diagnosticRecordId?: string; snapshot?: Uint8Array;
} = {}): Promise<void> {
  const chunks = state.snapshot ? [state.snapshot] : [];
  await fake.emit("documents.document.session", {
    event: "begin", documentId: doc, sessionPeer: "7", version: "0", chunks: chunks.length, editable: state.editable ?? true,
    ...(state.reason ? { reason: state.reason } : {}),
    ...(state.diagnosticRecordId ? { diagnosticRecordId: state.diagnosticRecordId } : {}),
  });
  for (const [chunkIndex, chunk] of chunks.entries()) {
    await fake.emit("documents.document.session", { event: "chunk", documentId: doc, chunkIndex, chunk: Buffer.from(chunk).toString("base64") });
  }
  await fake.emit("documents.document.session", { event: "end", documentId: doc });
}

/** Waits, a task at a time, until `done` holds. */
async function settle(done: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 200 && !done(); attempt += 1) await tick();
  expect(done()).toBe(true);
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
    expect(root.querySelector(".collection-button")?.getAttribute("data-selected")).toBe("true");
    expect(root.querySelector(".collection-list .document-title")?.textContent).toBe("Q4 plan");
    expect(root.querySelector(".documents-panel")).toBeNull();
    expect(texts(root, ".document-title")).toEqual(["Q4 plan"]);
    expect(root.querySelector('.collection-create')).toBeNull();
    expect(root.querySelector('.document-create')).toBeNull();
    const toggle = root.querySelector(".archived-toggle") as HTMLButtonElement;
    expect(toggle.getAttribute('aria-label')).toBe('Show archived');
    expect(toggle.getAttribute('aria-pressed')).toBe('false');
    expect(toggle.title).toBe('Show archived');
    expect(toggle.querySelector('[role=tooltip]')?.textContent).toBe('Show archived');
    const hiddenGraphic = toggle.querySelector('path')?.getAttribute('d');
    toggle.click();
    expect(toggle.getAttribute('aria-label')).toBe('Hide archived');
    expect(toggle.getAttribute('aria-pressed')).toBe('true');
    expect(toggle.querySelector('path')?.getAttribute('d')).not.toBe(hiddenGraphic);
    expect(toggle.querySelector('[role=tooltip]')?.textContent).toBe('Hide archived');
    expect(root.querySelector('.collection-rail-header')?.lastElementChild?.getAttribute('aria-label')).toBe('Add Collection');
    expect(texts(root, ".collection-button")).toEqual(["Planning", "Old"]);
    await unmount();
    expect(root.childElementCount).toBe(0);
  });

  it("sends create, rename, archive, and move requests with the selected Collection", async () => {
    const { fake, calls } = backend(snapshot);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));

    (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
    const collectionInput = root.querySelector(".collection-create input") as HTMLInputElement;
    collectionInput.value = "  Roadmap ";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.collection.create", payload: { name: "Roadmap" } });
    expect(collectionInput.value).toBe("");

    (root.querySelector('[aria-label="Add Document to Planning"]') as HTMLButtonElement).click();
    const documentInput = root.querySelector(".document-create input") as HTMLInputElement;
    documentInput.value = "Hiring plan";
    (root.querySelector(".document-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({
      operation: "documents.document.create",
      payload: { collectionId: planning, title: "Hiring plan" },
    });

    (root.querySelector('[aria-label="Actions for Q4 plan"]') as HTMLButtonElement).click();
    const renameButton = [...root.querySelectorAll(".document-actions button")].find((b) => b.textContent === "Rename") as HTMLButtonElement;
    renameButton.click();
    const renameInput = root.querySelector(".document-rename input") as HTMLInputElement;
    expect(renameInput.value).toBe("Q4 plan");
    renameInput.value = "Q4 plan v2";
    (root.querySelector(".document-rename") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.document.rename", payload: { documentId: doc, title: "Q4 plan v2" } });
    expect(root.querySelector(".document-rename")).toBeNull();

    (root.querySelector('[aria-label="Actions for Q4 plan"]') as HTMLButtonElement).click();
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
    expect(root.querySelector(".collection-button")?.getAttribute("data-selected")).toBe("true");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "OKRs"]);
    (root.querySelector('[aria-label="Actions for Q4 plan"]') as HTMLButtonElement).click();
    expect(root.querySelector(".document-move")).not.toBeNull();
    // A Collection this Installation created is selected once its record is listed.
    (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "Mine";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Planning");
    current = { ...current, collections: [...current.collections, { id: "9".repeat(64), name: "Mine", archived: false }] };
    await fake.emit("documents.changes", current);
    expect(root.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Mine");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "OKRs"]);
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
    expect(root.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Research");
    expect(texts(root, ".document-title")).toEqual(["New plan"]);
    const toggle = root.querySelector(".archived-toggle") as HTMLButtonElement;
    toggle.click();
    expect(texts(root, ".collection-button")).toContain("Planning");
    ([...root.querySelectorAll(".collection-button")].find((button) => button.textContent === "Planning") as HTMLButtonElement).click();
    expect(texts(root, ".document-title")).toEqual(["New plan"]);
    expect(root.querySelector('.collection-button')?.getAttribute('aria-expanded')).toBe('false');
    (root.querySelector('.collection-button') as HTMLButtonElement).click();
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "New plan"]);
    await unmount();
  });

  it("shows the editor placeholder only while a Document is open", async () => {
    const fake = createFakePackageClient({ requests: { "documents.snapshot": () => snapshot() },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const placeholder = root.querySelector(".editor-placeholder") as HTMLElement;
    expect(placeholder.hidden).toBe(true);
    expect(root.querySelector(".editor-header button")).toBeNull();
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    expect(placeholder.hidden).toBe(false);
    expect(placeholder.textContent).toBe("Opening…");
    expect(root.querySelector("textarea.editor-textarea")).toBeNull();
    await unmount();
  });
  it("opens a Document through its editor session and renders its body without Markdown", async () => {
    const fake = createFakePackageClient({
      requests: { "documents.snapshot": () => snapshot() },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    expect(fake.calls.filter((call) => call.operation === "documents.document.session"))
      .toEqual([{ operation: "documents.document.session", payload: { documentId: doc } }]);
    const pane = root.querySelector(".editor-pane") as HTMLElement;
    expect(pane.hidden).toBe(false);
    expect(root.querySelector(".editor-title")?.textContent).toBe("Q4 plan");
    expect(root.querySelector(".editor-status")?.textContent).toBe("Opening…");
    await opened(fake, { snapshot: ALL_NODES });
    await tick();
    expect(root.querySelector(".editor-status")?.textContent).toBe("");
    expect((root.querySelector(".editor-placeholder") as HTMLElement).hidden).toBe(true);
    const body = root.querySelector(".ProseMirror") as HTMLElement;
    expect(body.getAttribute("contenteditable")).toBe("true");
    for (const selector of ["h1", "h6", "strong", "em", "s", "code", "a", "blockquote", "ol", "ul", "hr", "pre", "br"]) {
      expect(body.querySelector(selector), selector).not.toBeNull();
    }
    expect(body.querySelector("a")?.getAttribute("href")).toBe("https://example.com/a");
    // The rendered body and the whole page show no Markdown syntax.
    expect(root.textContent).not.toMatch(/\*\*|~~|```|\]\(|^#+ /m);
    expect(root.querySelector("textarea.editor-textarea")).toBeNull();
    await unmount();
    expect(fake.calls.some((call) => call.operation === "documents.document.open")).toBe(false);
  });

  it("quotes the rendered selection when commenting on it", async () => {
    const added: PackageValue[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.comments": () => ({ comments: [] }),
        "documents.comment.add": (payload) => { added.push(payload); return { ok: true as const, id: "d".repeat(64) }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await opened(fake, { snapshot: ALL_NODES });
    await tick();
    const body = root.querySelector(".ProseMirror") as HTMLElement;
    body.dispatchEvent(new KeyboardEvent("keydown", { key: "a", ctrlKey: !mac, metaKey: mac, bubbles: true }));
    (root.querySelector(".editor-comment") as HTMLButtonElement).click();
    const draft = root.querySelector(".comment-quote--draft") as HTMLElement;
    expect(draft.hidden).toBe(false);
    const excerpt = draft.querySelector(".comment-quote-excerpt")?.textContent ?? "";
    expect(excerpt.startsWith("One\nTwo")).toBe(true);
    expect(excerpt).toContain("Plain strong em strike code link bare both and a hard\nbreak.");
    expect(excerpt).not.toContain("**");
    const input = root.querySelector(".comment-input") as HTMLTextAreaElement;
    input.value = "On all of it";
    (root.querySelector(".comment-compose") as HTMLFormElement).requestSubmit();
    await tick();
    expect(added).toEqual([{ documentId: doc, markdown: "On all of it", quote: excerpt }]);
    expect(draft.hidden).toBe(true);
    await unmount();
  });

  it("copies the Document as Markdown, falling back to a visible copy panel when the clipboard is refused", async () => {
    const markdown = "# Title\n\n" + "Body text.\n\n".repeat(3000);
    const parts: string[] = [];
    for (let start = 0; start < markdown.length; start += 24 * 1024) parts.push(markdown.slice(start, start + 24 * 1024));
    const digest = createHash("sha256").update(markdown).digest("hex");
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.comments": () => ({ comments: [] }),
        "documents.document.markdown": (payload) => {
          const chunkIndex = (payload as { chunkIndex: number }).chunkIndex;
          return { documentId: doc, digest, chunks: parts.length, chunkIndex, chunk: parts[chunkIndex]! };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const written: string[] = [];
    let allow = true;
    const original = Object.getOwnPropertyDescriptor(globalThis.navigator, "clipboard");
    Object.defineProperty(globalThis.navigator, "clipboard", {
      configurable: true,
      value: {
        write: async (items: ClipboardItem[]) => {
          if (!allow) throw Object.assign(new Error("denied"), { name: "NotAllowedError" });
          written.push(await (await items[0]!.getType("text/plain")).text());
        },
      },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    try {
      (root.querySelector(".document-title") as HTMLButtonElement).click();
      await tick();
      await opened(fake, { snapshot: ALL_NODES });
      await tick();
      const copy = root.querySelector(".editor-copy-markdown") as HTMLButtonElement;
      copy.click();
      await settle(() => written.length === 1);
      expect(written).toEqual([markdown]);
      expect(fake.calls.filter((call) => call.operation === "documents.document.markdown").map((call) => (call.payload as { chunkIndex: number }).chunkIndex))
        .toEqual(parts.map((_, index) => index));
      expect(root.querySelector(".editor-copy-status")?.textContent).toBe("Copied as Markdown");
      expect((root.querySelector(".markdown-copy") as HTMLElement).hidden).toBe(true);

      allow = false;
      copy.click();
      const panel = root.querySelector(".markdown-copy") as HTMLElement;
      await settle(() => !panel.hidden);
      expect((panel.querySelector("textarea") as HTMLTextAreaElement).value).toBe(markdown);
      [...panel.querySelectorAll("button")].find((button) => button.textContent === "Close")!.click();
      expect(panel.hidden).toBe(true);
    } finally {
      await unmount();
      if (original) Object.defineProperty(globalThis.navigator, "clipboard", original);
      else delete (globalThis.navigator as { clipboard?: unknown }).clipboard;
    }
  });
  it("shows a terminal conflict warning without retrying", async () => {
    let opens = 0;
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => { opens++; return { documentId: doc, editable: false, reason: "identity_conflict" }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await opened(fake, { editable: false, reason: "identity_conflict" });
    expect(root.querySelector(".editor-status")?.textContent).toContain("Conflicting edit history");
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(0);
    await unmount();
  });
  it("shows an import failure reference read-only without retrying", async () => {
    let opens = 0;
    const record = "e".repeat(64);
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => { opens++; return { documentId: doc, editable: false, reason: "import_failure", diagnosticRecordId: record }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await opened(fake, { editable: false, reason: "import_failure", diagnosticRecordId: record });
    expect(root.querySelector(".editor-status")?.textContent).toContain("Incomplete edit history");
    expect(root.querySelector(".editor-status")?.textContent).toContain(record);
    expect(root.querySelector(".editor-status")?.classList.contains("editor-status-failure")).toBe(true);
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(0);
    await unmount();
  });
  it("retries pending history after availability changes and reloads once the body is editable", async () => {
    let opens = 0;
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => ({ ...snapshot(), availability: "waiting_for_history_access" }),
        "documents.document.open": () => { opens++; return { documentId: doc, editable: opens > 1 }; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await opened(fake, { editable: false });
    expect(root.querySelector(".editor-status")?.textContent).toBe("Waiting for history");
    await fake.emit("documents.changes", snapshot());
    await tick();
    expect(opens).toBe(1);
    // The Backend tells the pending session to reload; the new session opens editable.
    await fake.emit("documents.document.session", { event: "resync", documentId: doc });
    await tick();
    expect(fake.calls.filter((call) => call.operation === "documents.document.session")).toHaveLength(2);
    await opened(fake);
    expect(root.querySelector(".editor-status")?.textContent).toBe("");
    await unmount();
  });
  it("lists threaded comments for the open Document and adds one", async () => {
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
        "documents.document.open": () => ({ documentId: doc, editable: true }),
        "documents.comments": () => ({ comments } as never),
        "documents.comment.add": (payload) => { added.push(payload); return { ok: true as const, id: "d".repeat(64) }; },
        "documents.comment.resolve": () => ({ ok: true as const, id: root }),
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined, "documents.names": () => undefined },
    });
    const rootNode = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root: rootNode }));
    (rootNode.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    await opened(fake);
    const items = [...rootNode.querySelectorAll(".comment")];
    expect(items).toHaveLength(2);
    expect(items[0]?.querySelector(".comment-quote")?.textContent).toBe("# Q4");
    const graphics = ['Reply', 'Resolve', 'Reopen'].map((label) => {
      const action = rootNode.querySelector(`.comment-actions [aria-label="${label}"]`) as HTMLButtonElement;
      expect(action?.title).toBe(label);
      expect(action.querySelector('[role=tooltip]')?.textContent).toBe(label);
      return action.querySelector('svg path')?.getAttribute('d');
    });
    expect(new Set(graphics).size).toBe(3);
    expect(rootNode.querySelector('.comment-actions')?.querySelectorAll('button')).toHaveLength(2);
    expect(items[0]?.querySelector(".comment-body strong")?.textContent).toBe("dates");
    expect(items[1]?.querySelector(".comment-author")?.textContent).toBe("You");
    expect(items[0]?.querySelector(".comment-author")?.textContent).toBe("cccccccc…cccccc");
    const person = { identity: "c".repeat(64), personId: "c".repeat(64), name: "Carol", fingerprint: "cccccccccccc" };
    await fake.emit("documents.names", { available: true, cursor: "1", persons: [person], part: 0, parts: 1 });
    expect(rootNode.querySelector(".comment-author")?.textContent).toBe("Carol");
    await fake.emit("documents.names", { available: true, cursor: "2", persons: [{ ...person, name: "Carol R" }], part: 0, parts: 1 });
    expect(rootNode.querySelector(".comment-author")?.textContent).toBe("Carol R");
    expect(items[1]?.classList.contains("is-resolved")).toBe(true);
    expect((items[1] as HTMLElement).style.getPropertyValue("--comment-depth")).toBe("1");

    const input = rootNode.querySelector(".comment-input") as HTMLTextAreaElement;
    input.dispatchEvent(new Event('focus'));
    expect((rootNode.querySelector(".comment-quote--draft") as HTMLElement).hidden).toBe(true);
    expect(rootNode.querySelector('.comment-compose-actions button')?.textContent).toBe('Comment');
    input.value = "Agree";
    (rootNode.querySelector(".comment-compose") as HTMLFormElement).requestSubmit();
    await tick();
    expect(added).toEqual([{ documentId: doc, markdown: "Agree" }]);
    expect(input.value).toBe("");
    await unmount();
  });

  it("keeps icon comment actions and keyboard-focused time operable", async () => {
    const commentId = 'a'.repeat(64);
    let resolved = false;
    const calls: Array<{ operation: string; payload: PackageValue }> = [];
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, editable: true }),
      'documents.comments': () => ({ comments: [{ id: commentId, documentId: doc, author: 'me',
        position: '01', resolved, isLocal: true, markdown: 'Parent', createdAt: '1730000000000' }] }),
      'documents.comment.resolve': (payload) => { calls.push({ operation: 'resolve', payload }); resolved = true; return { ok: true as const, id: commentId }; },
      'documents.comment.reopen': (payload) => { calls.push({ operation: 'reopen', payload }); resolved = false; return { ok: true as const, id: commentId }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.session': () => undefined } });
    const host = document.createElement('div');
    document.body.append(host);
    const root = host.attachShadow({ mode: 'open' });
    try {
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      (root.querySelector('.document-title') as HTMLButtonElement).click();
      await tick();
      const time = root.querySelector('.comment-time') as HTMLElement;
      time.focus();
      expect(root.activeElement).toBe(time);
      expect(time.querySelector('[role=tooltip]')?.textContent).toBe(new Date(1730000000000).toLocaleString());
      await fake.emit('documents.changes', snapshot());
      await tick();
      expect(root.activeElement).toBe(root.querySelector('.comment-time'));
      (root.querySelector('[aria-label=Reply]') as HTMLButtonElement).click();
      expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Replying to a comment');
      (root.querySelector('[aria-label=Resolve]') as HTMLButtonElement).click();
      await tick();
      expect(root.querySelector('.comment.is-resolved [aria-label=Reopen]')).not.toBeNull();
      (root.querySelector('[aria-label=Reopen]') as HTMLButtonElement).click();
      await tick();
      expect(root.querySelector('.comment.is-resolved')).toBeNull();
      expect(calls).toEqual([{ operation: 'resolve', payload: { commentId } },
        { operation: 'reopen', payload: { commentId } }]);
      await unmount();
    } finally { host.remove(); }
  });

  it("clears a reply on Document switch, but retains it and the composer draft after a failed post", async () => {
    const other = '4'.repeat(64);
    const parent = '5'.repeat(64);
    const current = { ...snapshot(), documents: [...snapshot().documents,
      { id: other, collectionId: planning, title: 'Other', archived: false }] };
    const added: unknown[] = [];
    let fail = true;
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => current,
      "documents.document.open": (payload) => ({ documentId: (payload as { documentId: string }).documentId, editable: true }),
      "documents.comments": (payload) => ({ comments: (payload as { documentId: string }).documentId === doc
        ? [{ id: parent, documentId: doc, author: 'me', position: '01', resolved: false, isLocal: true, markdown: 'Parent' }] : [] }),
      "documents.comment.add": (payload) => { added.push(payload); if (fail) throw Object.assign(new Error('failed'),
        { kind: 'teamarness.package-request-error', code: 'documents.invalid_markdown' });
        return { ok: true as const, id: 'd'.repeat(64) }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.session': () => undefined } });
    const root = document.createElement('div').attachShadow({ mode: 'open' });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    await opened(fake);
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    (root.querySelector('.comment button') as HTMLButtonElement).click();
    composer.dispatchEvent(new Event('focus'));
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Replying to a comment');
    composer.value = 'Still writing';
    (root.querySelector('.comment-compose') as HTMLFormElement).requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: doc, markdown: 'Still writing', parentId: parent });
    expect(root.querySelector('.comment-input')).toBe(composer);
    expect(composer.value).toBe('Still writing');
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Replying to a comment');
    (root.querySelectorAll('.document-title')[1] as HTMLButtonElement).click();
    await tick();
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    expect(root.querySelector('.comment-input')).toBe(composer);
    composer.dispatchEvent(new Event('focus'));
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    fail = false;
    (root.querySelector('.comment-compose') as HTMLFormElement).requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: other, markdown: 'Still writing' });
    await unmount();
  });

  it("loads older comment pages without stranding replies and displays author time", async () => {
    const parent = "4".repeat(64);
    const child = "5".repeat(64);
    const requests: unknown[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.document.open": () => ({ documentId: doc, editable: true }),
        "documents.comments": (payload) => {
          requests.push(payload);
          return requests.length === 1
            ? { comments: [{ id: child, documentId: doc, parentId: parent, author: "bob", position: "02", resolved: false, isLocal: false, markdown: "Reply", createdAt: "1730000000000" }], nextCursor: "older", hasMore: true }
            : { comments: [{ id: parent, documentId: doc, author: "alice", position: "01", resolved: false, isLocal: false, markdown: "Parent" }], nextCursor: null, hasMore: false };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector(".document-title") as HTMLButtonElement).click();
    await tick();
    expect(texts(root, ".comment-body").map((text) => text.trim())).toEqual(["Reply"]);
    const clock = root.querySelector('.comment-time') as HTMLElement;
    expect(clock.getAttribute('aria-label')).toBe(new Date(1730000000000).toLocaleString());
    expect(clock.querySelector('svg path')?.getAttribute('d')).toBeTruthy();
    expect(clock.querySelector('[role=tooltip]')?.textContent).toBe(clock.getAttribute('aria-label'));
    expect(clock.title).toBe(clock.getAttribute('aria-label'));
    expect(root.querySelector('.comment-time')?.getAttribute('datetime')).toBe(new Date(1730000000000).toISOString());
    expect(clock.tabIndex).toBe(0);
    expect(clock.matches('.documents-tooltip-control')).toBe(true);
    expect(documentsCss).toMatch(/\.documents-tooltip-control:hover \.control-tooltip, \.documents-tooltip-control:focus-visible \.control-tooltip\s*\{[^}]*visibility:\s*visible/);
    (root.querySelector(".comments-load-older") as HTMLButtonElement).click();
    await tick();
    expect(requests).toEqual([{ documentId: doc }, { documentId: doc, cursor: "older" }]);
    expect(texts(root, ".comment-body").map((text) => text.trim())).toEqual(["Parent", "Reply"]);
    expect(root.querySelector(".comment-time")?.getAttribute("aria-label")).toBe("Time unavailable");
    expect(root.querySelector('.comment-time [role=tooltip]')?.textContent).toBe('Time unavailable');
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
        "documents.document.open": () => ({ documentId: doc, editable: true }),
        "documents.comments": (payload) => {
          requests.push(payload);
          return "cursor" in (payload as object)
            ? { comments: [{ id: older, documentId: doc, author: "bob", position: "01", resolved, isLocal: false, markdown: "Older" }], nextCursor: null, hasMore: false }
            : { comments: [{ id: newest, documentId: doc, author: "bob", position: "02", resolved: false, isLocal: false, markdown: "Newest" }], nextCursor: "older", hasMore: true };
        },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
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

  it("uses the bold Collection name as its only expansion control without closing the editor", async () => {
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => snapshot(),
      "documents.document.open": () => ({ documentId: doc, editable: true }),
    }, subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const name = () => root.querySelector('.collection-button') as HTMLButtonElement;
    expect(root.querySelectorAll('.tree-row button')).toHaveLength(3);
    expect(name().getAttribute('aria-expanded')).toBe('true');
    expect(name().getAttribute('data-selected')).toBe('true');
    expect(name().hasAttribute('aria-current')).toBe(false);
    expect(documentsCss).toMatch(/\.collection-button\s*\{[^}]*font-weight:\s*700/);
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    const editor = root.querySelector('.editor-placeholder');
    name().click();
    expect(name().getAttribute('aria-expanded')).toBe('false');
    expect(name().getAttribute('data-selected')).toBe('true');
    expect(name().hasAttribute('aria-current')).toBe(false);
    expect(root.querySelector('.document-title')).toBeNull();
    expect(root.querySelector('.editor-placeholder')).toBe(editor);
    expect(root.querySelector('.editor-title')?.textContent).toBe('Q4 plan');
    name().click();
    expect(name().getAttribute('aria-expanded')).toBe('true');
    await unmount();
  });

  it("expands independent Collection branches and opens a nested Document with its Collection selected", async () => {
    const research = "4".repeat(64);
    const other = "5".repeat(64);
    const current = { ...snapshot(), collections: [...snapshot().collections, { id: research, name: "Research", archived: false }],
      documents: [...snapshot().documents, { id: other, collectionId: research, title: "Notes", archived: false }] };
    const { fake } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    expect(texts(root, ".document-title")).toEqual(["Q4 plan"]);
    ([...root.querySelectorAll(".collection-button")].find((b) => b.textContent === "Research") as HTMLButtonElement).click();
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "Notes"]);
    (root.querySelector(".collection-button") as HTMLButtonElement).click();
    expect(texts(root, ".document-title")).toEqual(["Notes"]);
    (root.querySelector('[aria-label="Add Document to Planning"]') as HTMLButtonElement).click();
    expect(root.querySelector('.collection-button[data-selected="true"]')?.textContent).toBe("Planning");
    expect(root.querySelector('.document-create')).not.toBeNull();
    (root.querySelector('.document-create button[aria-label="Cancel"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.document-title')].find((b) => b.textContent === 'Notes') as HTMLButtonElement).click();
    await tick();
    expect(root.querySelector('.collection-button[data-selected="true"]')?.textContent).toBe("Research");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "Notes"]);
    await unmount();
  });

  it("dismisses menus, preserves rename input on snapshot, and routes restore and move", async () => {
    const research = "4".repeat(64);
    const current = { ...snapshot(), collections: [...snapshot().collections, { id: research, name: "Research", archived: false }],
      documents: [...snapshot().documents, { id: "5".repeat(64), collectionId: planning, title: "Past", archived: true }] };
    const { fake, calls } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const actions = () => root.querySelector('[aria-label="Actions for Q4 plan"]') as HTMLButtonElement;
    actions().click();
    expect(actions().getAttribute('aria-haspopup')).toBeNull();
    expect(root.querySelector('.document-actions')?.getAttribute('role')).toBe('group');
    expect(root.querySelector('.document-actions')?.getAttribute('aria-label')).toBe('Actions for Q4 plan');
    expect(root.querySelector('.document-actions')).not.toBeNull();
    root.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(root.querySelector('.document-actions')).toBeNull();
    expect(actions().getAttribute('aria-expanded')).toBe('false');
    actions().click();
    document.body.click();
    expect(root.querySelector('.document-actions')).toBeNull();
    actions().click();
    (root.querySelector('.document-actions button') as HTMLButtonElement).click();
    const input = root.querySelector('.document-rename input') as HTMLInputElement;
    expect(input.value).toBe('Q4 plan');
    input.value = 'Still typing';
    await fake.emit('documents.changes', current);
    expect(root.querySelector('.document-rename input')).toBe(input);
    expect(input.value).toBe('Still typing');
    (root.querySelector('.document-rename button[aria-label="Cancel"]') as HTMLButtonElement).click();
    expect(root.querySelector('.document-rename')).toBeNull();
    actions().click();
    const move = root.querySelector('.document-move') as HTMLSelectElement;
    move.value = research;
    move.dispatchEvent(new Event('change'));
    await tick();
    expect(calls.at(-1)).toEqual({ operation: 'documents.document.move', payload: { documentId: doc, collectionId: research } });
    const toggle = root.querySelector('.archived-toggle') as HTMLButtonElement;
    toggle.click();
    (root.querySelector('[aria-label="Actions for Past"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.document-row.is-archived .document-actions button')].find((b) => b.textContent === 'Restore') as HTMLButtonElement).click();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: 'documents.document.restore', payload: { documentId: '5'.repeat(64) } });
    (root.querySelector('[aria-label="Actions for Old"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.collection-actions button')].find((b) => b.textContent === 'Restore') as HTMLButtonElement).click();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: 'documents.collection.restore', payload: { collectionId: archive } });
    (root.querySelector('[aria-label="Actions for Planning"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.collection-actions button')].find((b) => b.textContent === 'Rename') as HTMLButtonElement).click();
    const collectionName = root.querySelector('.collection-rename input') as HTMLInputElement;
    expect(collectionName.value).toBe('Planning');
    collectionName.value = 'Editing';
    await fake.emit('documents.changes', current);
    expect(root.querySelector('.collection-rename input')).toBe(collectionName);
    (root.querySelector('.collection-rename button[aria-label="Cancel"]') as HTMLButtonElement).click();
    expect(root.querySelector('.collection-rename')).toBeNull();
    (root.querySelector('[aria-label="Actions for Planning"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.collection-actions button')].find((b) => b.textContent === 'Archive') as HTMLButtonElement).click();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: 'documents.collection.archive', payload: { collectionId: planning } });
    await unmount();
  });

  it("toggles and resizes both rails without replacing the editor or composer", async () => {
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => snapshot(),
      "documents.document.open": () => ({ documentId: doc, editable: true }),
    }, subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    await opened(fake);
    const editor = root.querySelector('.editor-placeholder') as HTMLElement;
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    composer.value = 'Pending comment';
    expect(root.querySelectorAll('.comments-toggle')).toHaveLength(1);
    expect(root.querySelector('.editor-status')?.textContent).toBe('');
    expect(root.querySelector('.editor-pane .comments-panel')).toBeNull();
    (root.querySelector('[aria-label="Hide navigation"]') as HTMLButtonElement).click();
    expect((root.querySelector('.collection-rail') as HTMLElement).hidden).toBe(true);
    (root.querySelector('[aria-label="Show navigation"]') as HTMLButtonElement).click();
    (root.querySelector('[aria-label="Hide Comments"]') as HTMLButtonElement).click();
    expect((root.querySelector('.comments-panel') as HTMLElement).hidden).toBe(true);
    (root.querySelector('[aria-label="Show Comments"]') as HTMLButtonElement).click();
    const shell = root.querySelector('.documents-shell') as HTMLElement;
    Object.defineProperty(shell, 'clientWidth', { configurable: true, value: 1000 });
    await fake.emit('documents.changes', snapshot());
    const handle = root.querySelector('.left-resizer') as HTMLElement;
    expect([handle.getAttribute('aria-valuemin'), handle.getAttribute('aria-valuemax'), handle.getAttribute('aria-valuenow')]).toEqual(['190', '320', '250']);
    handle.setPointerCapture = () => { handle.hasPointerCapture = () => true; };
    handle.releasePointerCapture = () => { handle.hasPointerCapture = () => false; };
    handle.dispatchEvent(new PointerEvent('pointerdown', { pointerId: 1, clientX: 200, bubbles: true }));
    handle.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, clientX: 240, bubbles: true }));
    handle.dispatchEvent(new PointerEvent('pointerup', { pointerId: 1, bubbles: true }));
    expect((root.querySelector('.documents-shell') as HTMLElement).style.getPropertyValue('--left-width')).toBe('290px');
    expect(handle.getAttribute('aria-valuenow')).toBe('290');
    handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    expect(shell.style.getPropertyValue('--left-width')).toBe('320px');
    expect(handle.getAttribute('aria-valuenow')).toBe('320');
    const rightHandle = root.querySelector('.right-resizer') as HTMLElement;
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    expect((root.querySelector('.documents-shell') as HTMLElement).style.getPropertyValue('--right-width')).toBe('320px');
    expect([rightHandle.getAttribute('aria-valuemin'), rightHandle.getAttribute('aria-valuemax'), rightHandle.getAttribute('aria-valuenow')]).toEqual(['190', '360', '320']);
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
    expect(shell.style.getPropertyValue('--right-width')).toBe('360px');
    expect(rightHandle.getAttribute('aria-valuenow')).toBe('360');
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    rightHandle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    expect((root.querySelector('.documents-shell') as HTMLElement).style.getPropertyValue('--right-width')).toBe('190px');
    await fake.emit('documents.changes', snapshot());
    expect(root.querySelector('.editor-placeholder')).toBe(editor);
    expect(root.querySelector('.comment-input')).toBe(composer);
    expect(composer.value).toBe('Pending comment');
    await unmount();
  });

  it("does not close or clear a newer create draft when an older create request completes", async () => {
    const research = '4'.repeat(64);
    const current = { ...snapshot(), collections: [...snapshot().collections,
      { id: research, name: 'Research', archived: false }] };
    for (const operation of ['documents.document.create', 'documents.collection.create']) {
      let complete!: (result: { ok: true; id: string }) => void;
      const pending = new Promise<{ ok: true; id: string }>((resolve) => { complete = resolve; });
      const calls: PackageValue[] = [];
      const fake = createFakePackageClient({ requests: {
        'documents.snapshot': () => current,
        [operation]: (payload) => { calls.push(payload); return pending; },
      }, subscriptions: { 'documents.changes': () => undefined } });
      const root = document.createElement('div').attachShadow({ mode: 'open' });
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      if (operation === 'documents.document.create') {
        (root.querySelector('[aria-label="Add Document to Planning"]') as HTMLButtonElement).click();
      } else (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
      const selector = operation === 'documents.document.create' ? '.document-create' : '.collection-create';
      const input = root.querySelector(`${selector} input`) as HTMLInputElement;
      input.value = 'Earlier';
      (root.querySelector(selector) as HTMLFormElement).requestSubmit();
      await tick();
      if (operation === 'documents.document.create') {
        (root.querySelector('[aria-label="Add Document to Research"]') as HTMLButtonElement).click();
      } else {
        (root.querySelector(`${selector} [aria-label="Cancel"]`) as HTMLButtonElement).click();
        (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
      }
      expect(root.querySelector(`${selector} input`)).toBe(input);
      input.value = 'Later draft';
      input.dispatchEvent(new Event('input'));
      complete({ ok: true, id: '9'.repeat(64) });
      await tick();
      expect(root.querySelector(`${selector} input`)).toBe(input);
      expect(input.value).toBe('Later draft');
      expect(calls).toEqual([operation === 'documents.document.create'
        ? { collectionId: planning, title: 'Earlier' } : { name: 'Earlier' }]);
      await unmount();
    }
  });

  it("dismisses Add Document when its selected Collection changes before submission", async () => {
    const research = "4".repeat(64);
    const current = { ...snapshot(), collections: [...snapshot().collections,
      { id: research, name: "Research", archived: false }] };
    const { fake, calls } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('[aria-label="Add Document to Planning"]') as HTMLButtonElement).click();
    const oldForm = root.querySelector('.document-create') as HTMLFormElement;
    (oldForm.querySelector('input') as HTMLInputElement).value = 'For Planning';
    ([...root.querySelectorAll('.collection-button')].find((b) => b.textContent === 'Research') as HTMLButtonElement).click();
    expect(root.querySelector('.document-create')).toBeNull();
    oldForm.requestSubmit();
    await tick();
    expect(calls.filter((call) => call.operation === 'documents.document.create')).toEqual([]);
    (root.querySelector('[aria-label="Add Document to Research"]') as HTMLButtonElement).click();
    expect((root.querySelector('.document-create input') as HTMLInputElement).value).toBe('');
    (root.querySelector('.document-create input') as HTMLInputElement).value = 'For Research';
    (root.querySelector('.document-create') as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: 'documents.document.create',
      payload: { collectionId: research, title: 'For Research' } });
    await unmount();
  });

  it("keeps a focused tree form or row control usable across a pushed snapshot", async () => {
    const current = snapshot();
    const { fake } = backend(() => current);
    const host = document.createElement('div');
    document.body.append(host);
    const root = host.attachShadow({ mode: 'open' });
    try {
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      (root.querySelector('[aria-label="Add Document to Planning"]') as HTMLButtonElement).click();
      const input = root.querySelector('.document-create input') as HTMLInputElement;
      input.value = 'Draft';
      input.focus();
      input.setSelectionRange(1, 3);
      expect(root.activeElement).toBe(input);
      await fake.emit('documents.changes', current);
      expect(root.querySelector('.document-create input')).toBe(input);
      expect(root.activeElement).toBe(input);
      expect(input.value).toBe('Draft');
      expect([input.selectionStart, input.selectionEnd]).toEqual([1, 3]);
      const rowControl = root.querySelector('.collection-button') as HTMLButtonElement;
      rowControl.focus();
      expect(root.activeElement).toBe(rowControl);
      await fake.emit('documents.changes', current);
      expect(root.activeElement?.textContent).toBe('Planning');
      (root.activeElement as HTMLButtonElement).click();
      expect(root.querySelector('.collection-button')?.getAttribute('aria-expanded')).toBe('false');
      (root.querySelector('[aria-label="Actions for Planning"]') as HTMLButtonElement).click();
      const menuAction = root.querySelector('.collection-actions button') as HTMLButtonElement;
      menuAction.focus();
      await fake.emit('documents.changes', current);
      expect(root.activeElement?.getAttribute('aria-label')).toBe('Actions for Planning');
      (root.activeElement as HTMLButtonElement).click();
      (root.querySelector('.collection-actions button') as HTMLButtonElement).click();
      const rename = root.querySelector('.collection-rename input') as HTMLInputElement;
      rename.value = 'Renaming';
      rename.focus();
      await fake.emit('documents.changes', current);
      expect(root.activeElement).toBe(rename);
      expect(rename.value).toBe('Renaming');
      (root.querySelector('.collection-rename button[aria-label="Cancel"]') as HTMLButtonElement).click();
      (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
      const collection = root.querySelector('.collection-create input') as HTMLInputElement;
      collection.value = 'A new Collection';
      collection.focus();
      await fake.emit('documents.changes', current);
      expect(root.activeElement).toBe(collection);
      expect(collection.value).toBe('A new Collection');
      await unmount();
    } finally { host.remove(); }
  });

  it('retains focus on a reply cue through a pushed snapshot', async () => {
    const parent = '5'.repeat(64);
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, editable: true }),
      'documents.comments': () => ({ comments: [{ id: parent, documentId: doc, author: 'me', position: '01', resolved: false, isLocal: true, markdown: 'Parent' }] }),
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.session': () => undefined } });
    const host = document.createElement('div');
    document.body.append(host);
    const root = host.attachShadow({ mode: 'open' });
    try {
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      (root.querySelector('.document-title') as HTMLButtonElement).click();
      await tick();
      await opened(fake);
      for (const label of ['Clear reply']) {
        (root.querySelector('.comment button') as HTMLButtonElement).click();
        const control = root.querySelector(`[aria-label="${label}"]`) as HTMLButtonElement;
        control.focus();
        expect(root.activeElement).toBe(control);
        await fake.emit('documents.changes', snapshot());
        expect(root.activeElement).toBe(root.querySelector(`[aria-label="${label}"]`));
        (root.activeElement as HTMLButtonElement).click();
        expect(root.activeElement).toBe(root.querySelector('.comment-input'));
      }
      const hide = root.querySelector('[aria-label="Hide navigation"]') as HTMLButtonElement;
      hide.focus();
      hide.click();
      expect(root.activeElement).toBe(root.querySelector('[aria-label="Show navigation"]'));
      await unmount();
    } finally { host.remove(); }
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
    (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "x";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector("[role=alert]")?.textContent).toMatch(/Collection names/);
    expect(texts(root, ".collection-button")).toEqual(["Planning"]);
    await unmount();
  });
});

/** Waits, with a deadline, until `done` holds. */
async function within(done: () => boolean, milliseconds = 2000): Promise<void> {
  const deadline = Date.now() + milliseconds;
  while (!done() && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 10));
  expect(done()).toBe(true);
}

describe("Documents selection and remembered place", () => {
  const research = "4".repeat(64);
  const notes = "5".repeat(64);
  /** Research lists first, so a mount without a remembered place selects it. */
  function listing(): DocumentsSnapshot {
    const base = snapshot();
    return { ...base, collections: [{ id: research, name: "Research", archived: false }, ...base.collections],
      documents: [...base.documents, { id: notes, collectionId: research, title: "Notes", archived: false }] };
  }

  function placeBackend(place: PackageValue | (() => PackageValue)) {
    const saved: PackageValue[] = [];
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => listing(),
        "documents.comments": () => ({ comments: [] }),
        "documents.place.get": () => (typeof place === "function" ? place() : place),
        "documents.place.set": (payload) => { saved.push(payload); return {}; },
      },
      subscriptions: { "documents.changes": () => undefined, "documents.document.session": () => undefined },
    });
    return { fake, saved };
  }

  const sessions = (fake: ReturnType<typeof createFakePackageClient>) =>
    fake.calls.filter((call) => call.operation === "documents.document.session").map((call) => call.payload);

  it("never marks a Collection current; only the open Document is", async () => {
    const { fake } = placeBackend({});
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    expect(root.querySelector("[aria-current]")).toBeNull();
    ([...root.querySelectorAll(".collection-button")].find((b) => b.textContent === "Planning") as HTMLButtonElement).click();
    ([...root.querySelectorAll(".document-title")].find((b) => b.textContent === "Q4 plan") as HTMLButtonElement).click();
    await tick();
    expect(root.querySelectorAll("[aria-current]")).toHaveLength(1);
    expect(root.querySelector('.document-title[aria-current="true"]')?.textContent).toBe("Q4 plan");
    expect(root.querySelector(".collection-button[aria-current]")).toBeNull();
    // The open Document has the Shell-nav selection style, with no bar and normal weight.
    const selected = /\.document-title:hover,\s*\.document-title\[aria-current="true"\]\s*\{([^}]*)\}/.exec(documentsCss)?.[1] ?? "";
    expect(selected).toMatch(/color:\s*var\(--teamarness-color-accent\)/);
    expect(selected).toMatch(/background:\s*var\(--teamarness-color-background\)/);
    expect(documentsCss).not.toMatch(/collection-button\[aria-current/);
    expect(documentsCss).not.toMatch(/box-shadow:\s*inset 3px/);
    await unmount();
  });

  it("restores a remembered Collection, Document and scroll position on mount", async () => {
    const { fake } = placeBackend({ collectionId: planning, documentId: doc, scrollTop: 240 });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    await within(() => sessions(fake).length === 1);
    expect(sessions(fake)).toEqual([{ documentId: doc }]);
    expect(root.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Planning");
    expect(root.querySelector('.document-title[aria-current="true"]')?.textContent).toBe("Q4 plan");
    const surface = root.querySelector(".editor-surface") as HTMLElement;
    await opened(fake, { snapshot: ALL_NODES });
    await within(() => root.querySelector(".ProseMirror") !== null);
    expect(surface.scrollTop).toBe(240);
    await unmount();
  });

  it("falls back to the first Collection with nothing open when the remembered Document is gone", async () => {
    const { fake, saved } = placeBackend({ collectionId: planning, documentId: "6".repeat(64), scrollTop: 240 });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    await tick();
    expect(sessions(fake)).toEqual([]);
    expect(root.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Planning");
    expect(root.querySelector(".editor-title")?.textContent).toBe("Choose a Document");
    expect((root.querySelector(".documents-page > p.teamarness-notice--danger") as HTMLElement).hidden).toBe(true);
    // A Collection that is gone too falls back to today's first-Collection selection.
    await unmount();
    const gone = placeBackend({ collectionId: "7".repeat(64), documentId: "6".repeat(64) });
    const again = document.createElement("div").attachShadow({ mode: "open" });
    const remount = await mount(createTestMountContext({ client: gone.fake.client, root: again }));
    await tick();
    expect(sessions(gone.fake)).toEqual([]);
    expect(again.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Research");
    await remount();
    // A Backend without a remembered place (or one that fails) still mounts as before.
    const failing = createFakePackageClient({ requests: { "documents.snapshot": () => listing() },
      subscriptions: { "documents.changes": () => undefined } });
    const plain = document.createElement("div").attachShadow({ mode: "open" });
    const unmountPlain = await mount(createTestMountContext({ client: failing.client, root: plain }));
    expect(plain.querySelector(".collection-button[data-selected=true]")?.textContent).toBe("Research");
    await unmountPlain();
    expect(saved.every((place) => (place as { documentId?: string }).documentId === undefined)).toBe(true);
  });

  it("saves the selection and editor scroll, debounced, and a remount returns to them", async () => {
    let remembered: PackageValue = {};
    const { fake, saved } = placeBackend(() => remembered);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    ([...root.querySelectorAll(".collection-button")].find((b) => b.textContent === "Planning") as HTMLButtonElement).click();
    ([...root.querySelectorAll(".document-title")].find((b) => b.textContent === "Q4 plan") as HTMLButtonElement).click();
    await within(() => saved.some((place) => (place as { documentId?: string }).documentId === doc), 1000);
    expect(saved.at(-1)).toEqual({ collectionId: planning, documentId: doc });
    await opened(fake, { snapshot: ALL_NODES });
    await within(() => root.querySelector(".ProseMirror") !== null);
    const surface = root.querySelector(".editor-surface") as HTMLElement;
    const before = saved.length;
    for (const top of [40, 80, 120]) {
      surface.scrollTop = top;
      surface.dispatchEvent(new Event("scroll"));
    }
    await within(() => (saved.at(-1) as { scrollTop?: number } | undefined)?.scrollTop === 120, 1000);
    // A burst of scroll events is saved once.
    expect(saved.length - before).toBe(1);
    expect(saved.at(-1)).toEqual({ collectionId: planning, documentId: doc, scrollTop: 120 });
    remembered = saved.at(-1) as PackageValue;
    await unmount();

    const again = document.createElement("div").attachShadow({ mode: "open" });
    const remount = await mount(createTestMountContext({ client: fake.client, root: again }));
    await within(() => sessions(fake).length === 2);
    await opened(fake, { snapshot: ALL_NODES });
    await within(() => again.querySelector(".ProseMirror") !== null);
    expect((again.querySelector(".editor-surface") as HTMLElement).scrollTop).toBe(120);
    expect(again.querySelector('.document-title[aria-current="true"]')?.textContent).toBe("Q4 plan");
    await remount();
  });
});
