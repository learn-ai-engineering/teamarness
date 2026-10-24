// @vitest-environment happy-dom

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
    expect(root.querySelector(".collection-button")?.getAttribute("aria-current")).toBe("page");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "OKRs"]);
    (root.querySelector('[aria-label="Actions for Q4 plan"]') as HTMLButtonElement).click();
    expect(root.querySelector(".document-move")).not.toBeNull();
    // A Collection this Installation created is selected once its record is listed.
    (root.querySelector('[aria-label="Add Collection"]') as HTMLButtonElement).click();
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "Mine";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Planning");
    current = { ...current, collections: [...current.collections, { id: "9".repeat(64), name: "Mine", archived: false }] };
    await fake.emit("documents.changes", current);
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Mine");
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
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Research");
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

  it('does not enable Preview before a Document is open', async () => {
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, revision: '0', editable: true, text: 'Draft' }),
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
    const root = document.createElement('div').attachShadow({ mode: 'open' });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const preview = root.querySelector('.editor-header button') as HTMLButtonElement;
    expect(preview.disabled).toBe(true);
    preview.click();
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text: 'Draft' });
    expect(preview.disabled).toBe(false);
    expect(preview.getAttribute("aria-label")).toBe("Preview");
    expect(preview.title).toBe('Preview');
    expect(preview.querySelector('svg path')?.getAttribute('d')).toBeTruthy();
    expect(preview.querySelector('[role=tooltip]')?.textContent).toBe('Preview');
    expect(preview.getAttribute('aria-pressed')).toBe('false');
    const previewGraphic = preview.querySelector('path')?.getAttribute('d');
    preview.click();
    expect(preview.querySelector('path')?.getAttribute('d')).not.toBe(previewGraphic);
    expect(preview.getAttribute('aria-label')).toBe('Edit');
    expect(preview.querySelector('[role=tooltip]')?.textContent).toBe('Edit');
    expect(preview.getAttribute('aria-pressed')).toBe('true');
    preview.click();
    expect((root.querySelector('.editor-textarea') as HTMLElement).hidden).toBe(false);
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
    expect(texts(root, ".editor-header button")).not.toContain("Close");
    expect(pane.hidden).toBe(false);
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
    ([...root.querySelectorAll(".editor-header button")].find((button) => button.getAttribute("aria-label") === "Preview") as HTMLButtonElement).click();
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
    // Preview renders the current draft and hides the editor.
    ([...rootNode.querySelectorAll(".editor-header button")].find((b) => b.getAttribute("aria-label") === "Preview") as HTMLButtonElement).click();
    expect((rootNode.querySelector(".editor-textarea") as HTMLElement).hidden).toBe(true);
    expect(rootNode.querySelector(".editor-preview h1")?.textContent).toBe("Q4");
    ([...rootNode.querySelectorAll(".editor-header button")].find((b) => b.getAttribute("aria-label") === "Edit") as HTMLButtonElement).click();
    expect((rootNode.querySelector(".editor-textarea") as HTMLElement).hidden).toBe(false);
    expect(items[1]?.classList.contains("is-resolved")).toBe(true);
    expect((items[1] as HTMLElement).style.getPropertyValue("--comment-depth")).toBe("1");

    const textarea = rootNode.querySelector(".editor-textarea") as HTMLTextAreaElement;
    textarea.setSelectionRange(0, 4);
    const input = rootNode.querySelector(".comment-input") as HTMLTextAreaElement;
    input.dispatchEvent(new Event('focus'));
    expect(rootNode.querySelector(".comment-quote--draft")?.textContent).toContain("Commenting on highlighted text");
    expect(rootNode.querySelector(".comment-quote--draft")?.textContent).toContain("# Q4");
    expect(rootNode.querySelector('.comment-compose-actions button')?.textContent).toBe('Comment');
    input.value = "Agree";
    (rootNode.querySelector(".comment-compose") as HTMLFormElement).requestSubmit();
    await tick();
    expect(added).toEqual([{ documentId: doc, markdown: "Agree", quote: "# Q4" }]);
    expect(input.value).toBe("");
    await unmount();
  });

  it("keeps icon comment actions and keyboard-focused time operable", async () => {
    const commentId = 'a'.repeat(64);
    let resolved = false;
    const calls: Array<{ operation: string; payload: PackageValue }> = [];
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, revision: '0', editable: true, text: '' }),
      'documents.comments': () => ({ comments: [{ id: commentId, documentId: doc, author: 'me',
        position: '01', resolved, isLocal: true, markdown: 'Parent', createdAt: '1730000000000' }] }),
      'documents.comment.resolve': (payload) => { calls.push({ operation: 'resolve', payload }); resolved = true; return { ok: true as const, id: commentId }; },
      'documents.comment.reopen': (payload) => { calls.push({ operation: 'reopen', payload }); resolved = false; return { ok: true as const, id: commentId }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
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

  it("captures only a nonempty editor selection on composer focus, caps it, and clears it on deselection or X", async () => {
    const added: unknown[] = [];
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => snapshot(),
      "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
      "documents.comment.add": (payload) => { added.push(payload); return { ok: true as const, id: "d".repeat(64) }; },
    }, subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    const text = `  ${'x'.repeat(2100)}  `;
    await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text });
    const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    const form = root.querySelector('.comment-compose') as HTMLFormElement;
    editor.focus();
    editor.setSelectionRange(0, text.length);
    // happy-dom does not focus controls inside a detached ShadowRoot; dispatch the browser focus event.
    composer.dispatchEvent(new Event('focus'));
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Commenting on highlighted text');
    expect(root.querySelector('.comment-quote--draft span:last-of-type')?.textContent).toBe('x'.repeat(2048));
    expect(root.querySelector('[aria-label="Clear quote"]')).not.toBeNull();
    expect(root.querySelector('[aria-label="Quote selection"]')).toBeNull();
    // The browser may retain an editor selection after focus moves; capture uses the editor, not window selection.
    composer.value = 'Quoted';
    form.requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: doc, markdown: 'Quoted', quote: 'x'.repeat(2048) });
    expect(editor.selectionStart).toBe(editor.selectionEnd);
    composer.dispatchEvent(new Event('focus'));
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    expect((root.querySelector('.comment-input') as HTMLTextAreaElement).value).toBe('');
    editor.focus();
    editor.setSelectionRange(2, 5);
    composer.dispatchEvent(new Event('focus'));
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('xxx');
    editor.focus();
    editor.setSelectionRange(4, 4);
    composer.dispatchEvent(new Event('focus'));
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    composer.value = 'Collapsed';
    form.requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: doc, markdown: 'Collapsed' });
    editor.focus();
    editor.setSelectionRange(2, 5);
    composer.dispatchEvent(new Event('focus'));
    (root.querySelector('[aria-label="Clear quote"]') as HTMLButtonElement).click();
    expect(editor.selectionStart).toBe(editor.selectionEnd);
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    editor.focus();
    composer.dispatchEvent(new Event('focus'));
    composer.value = 'Cleared';
    form.requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: doc, markdown: 'Cleared' });
    await unmount();
  });

  it("clears quote and reply on Document switch, but retains them and composer draft after failed post", async () => {
    const other = '4'.repeat(64);
    const parent = '5'.repeat(64);
    const current = { ...snapshot(), documents: [...snapshot().documents,
      { id: other, collectionId: planning, title: 'Other', archived: false }] };
    const added: unknown[] = [];
    let fail = true;
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => current,
      "documents.document.open": (payload) => ({ documentId: (payload as { documentId: string }).documentId, revision: '0', editable: true, text: 'Selected text' }),
      "documents.comments": (payload) => ({ comments: (payload as { documentId: string }).documentId === doc
        ? [{ id: parent, documentId: doc, author: 'me', position: '01', resolved: false, isLocal: true, markdown: 'Parent' }] : [] }),
      "documents.comment.add": (payload) => { added.push(payload); if (fail) throw Object.assign(new Error('failed'),
        { kind: 'teamarness.package-request-error', code: 'documents.invalid_markdown' });
        return { ok: true as const, id: 'd'.repeat(64) }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
    const root = document.createElement('div').attachShadow({ mode: 'open' });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text: 'Selected text' });
    const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    editor.focus();
    editor.setSelectionRange(0, 8);
    (root.querySelector('.comment button') as HTMLButtonElement).click();
    composer.dispatchEvent(new Event('focus'));
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Replying to a comment');
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Selected');
    composer.value = 'Still writing';
    (root.querySelector('.comment-compose') as HTMLFormElement).requestSubmit();
    await tick();
    expect(added.at(-1)).toEqual({ documentId: doc, markdown: 'Still writing', quote: 'Selected', parentId: parent });
    expect(root.querySelector('.comment-input')).toBe(composer);
    expect(composer.value).toBe('Still writing');
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Selected');
    expect(root.querySelector('.comment-quote--draft')?.textContent).toContain('Replying to a comment');
    (root.querySelectorAll('.document-title')[1] as HTMLButtonElement).click();
    await tick();
    expect((root.querySelector('.comment-quote--draft') as HTMLElement).hidden).toBe(true);
    expect(root.querySelector('.comment-input')).toBe(composer);
    expect(editor.value).toBe('Selected text');
    expect(editor.selectionStart).toBe(editor.selectionEnd);
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

  it("uses the bold Collection name as its only expansion control without closing the editor", async () => {
    const fake = createFakePackageClient({ requests: {
      "documents.snapshot": () => snapshot(),
      "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "Draft" }),
    }, subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const name = () => root.querySelector('.collection-button') as HTMLButtonElement;
    expect(root.querySelectorAll('.tree-row button')).toHaveLength(3);
    expect(name().getAttribute('aria-expanded')).toBe('true');
    expect(name().getAttribute('aria-current')).toBe('page');
    expect(documentsCss).toMatch(/\.collection-button\s*\{[^}]*font-weight:\s*700/);
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    const editor = root.querySelector('.editor-textarea');
    name().click();
    expect(name().getAttribute('aria-expanded')).toBe('false');
    expect(name().getAttribute('aria-current')).toBe('page');
    expect(root.querySelector('.document-title')).toBeNull();
    expect(root.querySelector('.editor-textarea')).toBe(editor);
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
    expect(root.querySelector('.collection-button[aria-current="page"]')?.textContent).toBe("Planning");
    expect(root.querySelector('.document-create')).not.toBeNull();
    (root.querySelector('.document-create button[aria-label="Cancel"]') as HTMLButtonElement).click();
    ([...root.querySelectorAll('.document-title')].find((b) => b.textContent === 'Notes') as HTMLButtonElement).click();
    await tick();
    expect(root.querySelector('.collection-button[aria-current="page"]')?.textContent).toBe("Research");
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
      "documents.document.open": () => ({ documentId: doc, revision: "0", editable: true, text: "" }),
    }, subscriptions: { "documents.changes": () => undefined, "documents.document.text": () => undefined } });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text: 'Draft' });
    const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    composer.value = 'Pending comment';
    expect(root.querySelectorAll('.comments-toggle')).toHaveLength(1);
    expect(root.querySelector('.editor-status')?.textContent).toBe('Saved');
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
    expect(root.querySelector('.editor-textarea')).toBe(editor);
    expect(editor.value).toBe('Draft');
    expect(root.querySelector('.comment-input')).toBe(composer);
    expect(composer.value).toBe('Pending comment');
    (root.querySelector('.editor-header button') as HTMLButtonElement).click();
    expect(root.querySelector('.editor-preview')?.textContent?.trim()).toBe('Draft');
    expect(root.querySelector('.editor-header button')?.getAttribute('aria-label')).toBe('Edit');
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

  it('posts without a quote after editor deselection and a direct Comment button click', async () => {
    const added: PackageValue[] = [];
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, revision: '0', editable: true, text: 'Selected text' }),
      'documents.comment.add': (payload) => { added.push(payload); return { ok: true as const, id: 'd'.repeat(64) }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
    const host = document.createElement('div');
    document.body.append(host);
    const root = host.attachShadow({ mode: 'open' });
    try {
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      (root.querySelector('.document-title') as HTMLButtonElement).click();
      await tick();
      await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text: 'Selected text' });
      const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
      const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
      editor.setSelectionRange(0, 8);
      composer.focus();
      expect(root.querySelector('.comment-quote-excerpt')?.textContent).toBe('Selected');
      editor.focus();
      editor.setSelectionRange(8, 8);
      composer.value = 'Ordinary';
      (root.querySelector('.comment-compose button[type=submit]') as HTMLButtonElement).click();
      await tick();
      expect(added).toEqual([{ documentId: doc, markdown: 'Ordinary' }]);
      await unmount();
    } finally { host.remove(); }
  });

  it('retains focus on a quote or reply cue through a pushed snapshot', async () => {
    const parent = '5'.repeat(64);
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, revision: '0', editable: true, text: 'Selected text' }),
      'documents.comments': () => ({ comments: [{ id: parent, documentId: doc, author: 'me', position: '01', resolved: false, isLocal: true, markdown: 'Parent' }] }),
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
    const host = document.createElement('div');
    document.body.append(host);
    const root = host.attachShadow({ mode: 'open' });
    try {
      const unmount = await mount(createTestMountContext({ client: fake.client, root }));
      (root.querySelector('.document-title') as HTMLButtonElement).click();
      await tick();
      await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text: 'Selected text' });
      const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
      editor.setSelectionRange(0, 8);
      (root.querySelector('.comment-input') as HTMLTextAreaElement).dispatchEvent(new Event('focus'));
      for (const label of ['Clear quote', 'Clear reply']) {
        if (label === 'Clear reply') (root.querySelector('.comment button') as HTMLButtonElement).click();
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

  it("caps a multibyte selected quote to 2048 UTF-8 bytes before posting", async () => {
    const added: unknown[] = [];
    const fake = createFakePackageClient({ requests: {
      'documents.snapshot': () => snapshot(),
      'documents.document.open': () => ({ documentId: doc, revision: '0', editable: true, text: '' }),
      'documents.comment.add': (payload) => { added.push(payload); return { ok: true as const, id: 'd'.repeat(64) }; },
    }, subscriptions: { 'documents.changes': () => undefined, 'documents.document.text': () => undefined } });
    const root = document.createElement('div').attachShadow({ mode: 'open' });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    (root.querySelector('.document-title') as HTMLButtonElement).click();
    await tick();
    const text = `${'é'.repeat(1023)}😀é`;
    await fake.emit('documents.document.text', { documentId: doc, revision: '0', editable: true, text });
    const editor = root.querySelector('.editor-textarea') as HTMLTextAreaElement;
    editor.setSelectionRange(0, text.length);
    const composer = root.querySelector('.comment-input') as HTMLTextAreaElement;
    composer.dispatchEvent(new Event('focus'));
    const expected = 'é'.repeat(1023);
    expect(root.querySelector('.comment-quote-excerpt')?.textContent).toBe(expected);
    composer.value = 'Quoted';
    (root.querySelector('.comment-compose') as HTMLFormElement).requestSubmit();
    await tick();
    expect(added).toEqual([{ documentId: doc, markdown: 'Quoted', quote: expected }]);
    expect(new TextEncoder().encode(expected).length).toBe(2046);
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
