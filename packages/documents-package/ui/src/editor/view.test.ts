// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LoroDoc } from "loro-crdt";
import { baseKeymap } from "prosemirror-commands";
import { keymap } from "prosemirror-keymap";
import type { Node } from "prosemirror-model";
import { AllSelection, EditorState, TextSelection } from "prosemirror-state";
import type { Command } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GuardHost } from "./guard";
import { MARK_EXPAND, schema } from "./schema";
import {
  DocumentEditor, MAX_QUOTE, acceptableHref, boundedQuote, editorCommands, editorInputRules, editorKeymap,
  editorPlugins, hasBody, initialDocument, selectionQuote, setLink,
} from "./view";

const GENESIS = join(import.meta.dirname, "../../../fixtures/body-trees/from-rust/00-genesis.loro");
const mac = /Mac|iP(hone|[oa]d)/.test(globalThis.navigator?.platform ?? "");

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

function paragraphs(...texts: string[]): Node {
  return schema.node("doc", null, texts.map((text) => schema.node("paragraph", null, text ? [schema.text(text)] : [])));
}

/** A plain editor (no Loro) with the editor's input rules and keymap. */
function plainView(doc: Node = paragraphs(""), openLink: () => boolean = () => true): EditorView {
  const state = EditorState.create({
    schema, doc,
    plugins: [editorInputRules(), keymap(editorKeymap(openLink)), keymap(baseKeymap)],
  });
  const view = new EditorView(document.createElement("div"), { state });
  views.push(view);
  return view;
}

/** Types `text` as the browser would: each character goes through the input rules first. */
function type(view: EditorView, text: string): void {
  for (const character of text) {
    const { from, to } = view.state.selection;
    const handled = view.someProp("handleTextInput", (handler) => handler(view, from, to, character, () => view.state.tr));
    if (!handled) view.dispatch(view.state.tr.insertText(character, from, to));
  }
}

/** Presses a key chord such as `Mod-b` or `Shift-Ctrl-1` through the keymaps. */
function press(view: EditorView, chord: string): boolean {
  const parts = chord.split("-");
  let key = parts.pop()!;
  const has = (name: string) => parts.includes(name);
  const shift = has("Shift");
  if (shift && /^[0-9]$/.test(key)) key = ")!@#$%^&*("[Number(key)]!;
  else if (shift && key.length === 1) key = key.toUpperCase();
  const event = new KeyboardEvent("keydown", {
    key,
    shiftKey: shift,
    altKey: has("Alt"),
    ctrlKey: has("Ctrl") || (has("Mod") && !mac),
    metaKey: has("Mod") && mac,
    keyCode: /^[0-9]$/.test(chord.slice(-1)) ? 48 + Number(chord.slice(-1)) : key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0,
  });
  return view.someProp("handleKeyDown", (handler) => handler(view, event)) ?? false;
}

function select(view: EditorView, from: number, to = from): void {
  view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, from, to)));
}

function run(view: EditorView, command: Command): boolean {
  return command(view.state, view.dispatch, view);
}

function blockTypes(doc: Node): string[] {
  const types: string[] = [];
  doc.descendants((node) => { if (!node.isText) types.push(node.type.name); });
  return types;
}

function markedText(doc: Node, mark: string): string {
  let text = "";
  doc.descendants((node) => { if (node.isText && node.marks.some((m) => m.type.name === mark)) text += node.text; });
  return text;
}

describe("toolbar commands", () => {
  it("create every block node", () => {
    const view = plainView(paragraphs("alpha", "bravo"));
    select(view, 2);
    expect(run(view, editorCommands.heading(3))).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("heading");
    expect(view.state.doc.firstChild?.attrs.level).toBe(3);
    expect(run(view, editorCommands.paragraph)).toBe(true);
    expect(run(view, editorCommands.codeBlock)).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("code_block");
    run(view, editorCommands.paragraph);
    expect(run(view, editorCommands.bulletList)).toBe(true);
    expect(blockTypes(view.state.doc).slice(0, 3)).toEqual(["bullet_list", "list_item", "paragraph"]);
    expect(run(view, editorCommands.bulletList)).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("paragraph");
    expect(run(view, editorCommands.orderedList)).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("ordered_list");
    run(view, editorCommands.orderedList);
    expect(run(view, editorCommands.blockquote)).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("blockquote");
    expect(run(view, editorCommands.blockquote)).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("paragraph");
    select(view, 4);
    expect(run(view, editorCommands.hardBreak)).toBe(true);
    expect(blockTypes(view.state.doc)).toContain("hard_break");
    select(view, view.state.doc.content.size - 1);
    expect(run(view, editorCommands.horizontalRule)).toBe(true);
    expect(blockTypes(view.state.doc)).toContain("horizontal_rule");
    expect(() => view.state.doc.check()).not.toThrow();
  });

  it("toggle every mark and set and remove a link", () => {
    const view = plainView(paragraphs("alpha bravo"));
    for (const name of ["strong", "em", "strikethrough", "code"] as const) {
      select(view, 1, 6);
      expect(run(view, editorCommands[name])).toBe(true);
      expect(markedText(view.state.doc, name)).toBe("alpha");
      run(view, editorCommands[name]);
      expect(markedText(view.state.doc, name)).toBe("");
    }
    select(view, 7, 12);
    expect(run(view, setLink("https://example.com/x"))).toBe(true);
    expect(markedText(view.state.doc, "link")).toBe("bravo");
    // With the cursor inside the link, the whole link is the target.
    select(view, 9);
    expect(run(view, setLink("https://example.com/y"))).toBe(true);
    expect(view.state.doc.firstChild?.child(1).marks[0]?.attrs.href).toBe("https://example.com/y");
    expect(run(view, setLink(null))).toBe(true);
    expect(markedText(view.state.doc, "link")).toBe("");
    select(view, 7, 12);
    expect(run(view, setLink("javascript:alert(1)"))).toBe(false);
  });

  it("accept only bounded web, mail and relative links", () => {
    expect(acceptableHref("https://example.com/a?b#c")).toBe(true);
    expect(acceptableHref("mailto:a@example.com")).toBe(true);
    expect(acceptableHref("../other")).toBe(true);
    expect(acceptableHref("#section")).toBe(true);
    expect(acceptableHref("javascript:alert(1)")).toBe(false);
    expect(acceptableHref("data:text/html,x")).toBe(false);
    expect(acceptableHref("https://example.com/" + "a".repeat(2048))).toBe(false);
    expect(acceptableHref("")).toBe(false);
  });
});

describe("keyboard shortcuts", () => {
  it("create every mark", () => {
    const view = plainView(paragraphs("alpha"));
    for (const [chord, name] of [["Mod-b", "strong"], ["Mod-i", "em"], ["Shift-Mod-s", "strikethrough"], ["Mod-e", "code"]] as const) {
      select(view, 1, 6);
      expect(press(view, chord), chord).toBe(true);
      expect(markedText(view.state.doc, name), chord).toBe("alpha");
      press(view, chord);
    }
    let opened = 0;
    const linking = plainView(paragraphs("alpha"), () => { opened += 1; return true; });
    select(linking, 1, 6);
    expect(press(linking, "Mod-k")).toBe(true);
    expect(opened).toBe(1);
  });

  it("create every block node", () => {
    const view = plainView(paragraphs("alpha"));
    select(view, 2);
    for (let level = 1; level <= 6; level += 1) {
      expect(press(view, `Shift-Ctrl-${level}`)).toBe(true);
      expect(view.state.doc.firstChild?.attrs.level).toBe(level);
    }
    expect(press(view, "Shift-Ctrl-0")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("paragraph");
    expect(press(view, "Shift-Ctrl-\\")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("code_block");
    press(view, "Shift-Ctrl-0");
    expect(press(view, "Shift-Ctrl-8")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("bullet_list");
    press(view, "Shift-Ctrl-8");
    expect(press(view, "Shift-Ctrl-9")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("ordered_list");
    press(view, "Shift-Ctrl-9");
    expect(press(view, "Ctrl->")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("blockquote");
    press(view, "Ctrl->");
    select(view, 3);
    expect(press(view, "Shift-Enter")).toBe(true);
    expect(blockTypes(view.state.doc)).toContain("hard_break");
    expect(press(view, "Mod-_")).toBe(true);
    expect(blockTypes(view.state.doc)).toContain("horizontal_rule");
  });

  it("continues after a rule inserted at the end of the document", () => {
    const view = plainView(paragraphs("alpha"));
    select(view, 6);
    expect(press(view, "Mod-_")).toBe(true);
    type(view, "next");
    expect(blockTypes(view.state.doc)).toEqual(["paragraph", "horizontal_rule", "paragraph"]);
    expect(view.state.doc.lastChild?.textContent).toBe("next");
  });

  it("turns a code fence paragraph into a code block on Enter", () => {
    const view = plainView(paragraphs("```rust"));
    select(view, 8);
    expect(press(view, "Enter")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("code_block");
    expect(view.state.doc.firstChild?.attrs.params).toBe("rust");
    expect(view.state.doc.firstChild?.textContent).toBe("");
  });
});

describe("input rules", () => {
  it.each([
    ["# ", "heading", 1], ["### ", "heading", 3], ["###### ", "heading", 6],
    ["> ", "blockquote", undefined], ["- ", "bullet_list", undefined], ["* ", "bullet_list", undefined],
    ["1. ", "ordered_list", undefined], ["```js ", "code_block", undefined],
  ])("%j makes a %s", (input, node, level) => {
    const view = plainView();
    type(view, input);
    const first = view.state.doc.firstChild!;
    expect(first.type.name).toBe(node);
    if (level !== undefined) expect(first.attrs.level).toBe(level);
    expect(view.state.doc.textContent).toBe("");
  });

  it("keeps an ordered list's start number", () => {
    const view = plainView();
    type(view, "3. ");
    expect(view.state.doc.firstChild?.attrs.order).toBe(3);
  });

  it.each(["---", "***", "___"])("%j makes a horizontal rule", (input) => {
    const view = plainView();
    type(view, input);
    expect(blockTypes(view.state.doc)).toEqual(["horizontal_rule", "paragraph"]);
    type(view, "next");
    expect(view.state.doc.lastChild?.textContent).toBe("next");
  });

  it.each([
    ["**bold** ", "strong", "bold"], ["__bold__ ", "strong", "bold"], ["*it* ", "em", "it"], ["_it_ ", "em", "it"],
    ["~~gone~~ ", "strikethrough", "gone"], ["`x = 1` ", "code", "x = 1"],
  ])("%j makes %s and removes the delimiters", (input, mark, text) => {
    const view = plainView();
    type(view, `a ${input}after`);
    expect(markedText(view.state.doc, mark)).toBe(text);
    expect(view.state.doc.textContent).toBe(`a ${text} after`);
  });

  it("keeps Markdown characters as text inside a code block", () => {
    const view = plainView(schema.node("doc", null, [schema.node("code_block")]));
    type(view, "**not bold** ");
    expect(view.state.doc.textContent).toBe("**not bold** ");
  });

  it("undoes an input rule with Backspace", () => {
    const view = plainView();
    type(view, "# ");
    expect(view.state.doc.firstChild?.type.name).toBe("heading");
    expect(press(view, "Backspace")).toBe(true);
    expect(view.state.doc.firstChild?.type.name).toBe("paragraph");
    expect(view.state.doc.textContent).toBe("# ");
  });
});

describe("comment quotes", () => {
  it("are the selection's plain text", () => {
    const view = plainView(schema.node("doc", null, [
      schema.node("paragraph", null, [schema.text("one "), schema.text("bold", [schema.marks.strong.create()])]),
      schema.node("paragraph", null, [schema.text("two")]),
    ]));
    expect(selectionQuote(view.state)).toBeNull();
    select(view, 1, view.state.doc.content.size - 1);
    expect(selectionQuote(view.state)).toBe("one bold\ntwo");
  });

  it("are cut to 2 KiB on a character boundary", () => {
    expect(boundedQuote("a".repeat(MAX_QUOTE + 10))).toBe("a".repeat(MAX_QUOTE));
    const wide = "é".repeat(MAX_QUOTE);
    const quote = boundedQuote(`  ${wide}  `)!;
    expect(new TextEncoder().encode(quote).length).toBe(MAX_QUOTE);
    expect(quote).toBe("é".repeat(MAX_QUOTE / 2));
    expect(boundedQuote(" \n ")).toBeNull();
  });
});

/** A Loro peer holding the Rust genesis, with the binding's mark configuration. */
function genesisPeer(peer: bigint): LoroDoc {
  const doc = new LoroDoc();
  doc.import(new Uint8Array(readFileSync(GENESIS)));
  doc.setPeerId(peer);
  doc.configTextStyle(MARK_EXPAND);
  return doc;
}

const idleGuard: GuardHost = { unsentBytes: () => 0, flush: () => {}, refused: () => {} };

function loroView(doc: LoroDoc): EditorView {
  const view = new EditorView(document.createElement("div"), {
    state: EditorState.create({ schema, doc: initialDocument(doc), plugins: editorPlugins(doc, idleGuard, () => true) }),
  });
  views.push(view);
  return view;
}

function nextTask(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("per-person undo", () => {
  it("undoes only this peer's edit and leaves the remote edit in place", async () => {
    const a = genesisPeer(11n);
    const b = genesisPeer(22n);
    expect(hasBody(a)).toBe(true);
    a.subscribeLocalUpdates((update) => { b.import(update); });
    b.subscribeLocalUpdates((update) => { a.import(update); });
    const viewA = loroView(a);
    const viewB = loroView(b);
    await nextTask();
    await nextTask();
    viewB.dispatch(viewB.state.tr.insertText("base text", 1));
    await nextTask();
    expect(viewA.state.doc.textContent).toBe("base text");
    viewA.dispatch(viewA.state.tr.insertText("mine ", 1));
    await nextTask();
    viewB.dispatch(viewB.state.tr.insertText(" theirs", viewB.state.doc.content.size - 1));
    await nextTask();
    expect(viewA.state.doc.textContent).toBe("mine base text theirs");
    expect(press(viewA, "Mod-z")).toBe(true);
    await nextTask();
    await nextTask();
    expect(viewA.state.doc.textContent).toBe("base text theirs");
    expect(viewB.state.doc.textContent).toBe("base text theirs");
    // B never had A's edit in its own history: its undo removes only its own text.
    expect(press(viewB, "Mod-z")).toBe(true);
    await nextTask();
    await nextTask();
    expect(viewA.state.doc.textContent).toBe(viewB.state.doc.textContent);
    expect(viewB.state.doc.textContent).not.toContain("mine");
    expect(press(viewA, "Shift-Mod-z")).toBe(true);
    await nextTask();
    await nextTask();
    expect(viewA.state.doc.textContent).toContain("mine");
    expect(press(viewA, "Mod-y")).toBe(false);
  });
});

describe("the rendered editor", () => {
  it("binds to the session's doc, follows its replacement, and never shows Markdown", async () => {
    let flushed = 0;
    const editor = new DocumentEditor({
      guard: { unsentBytes: () => 0, flush: () => { flushed += 1; } },
      onCopyMarkdown: () => {}, onComment: () => {},
    });
    document.body.append(editor.element);
    const first = genesisPeer(33n);
    editor.bind(first, true);
    await nextTask();
    const view = editor.editorView!;
    view.dispatch(view.state.tr.insertText("Hello"));
    select(view, 1, 6);
    expect(editor.quote()).toBe("Hello");
    const bold = editor.element.querySelector<HTMLButtonElement>('[aria-label="Bold"]')!;
    expect(bold.disabled).toBe(false);
    bold.click();
    expect(bold.getAttribute("aria-pressed")).toBe("true");
    expect(editor.element.querySelector(".ProseMirror strong")?.textContent).toBe("Hello");
    expect(editor.element.textContent).not.toMatch(/\*\*|__|~~|```/);
    editor.collapseSelection();
    expect(editor.quote()).toBeNull();
    // Select-all collapses into the last text block, not onto the document node.
    const warnings = vi.spyOn(console, "warn");
    view.dispatch(view.state.tr.setSelection(new AllSelection(view.state.doc)));
    editor.collapseSelection();
    expect(view.state.selection.empty).toBe(true);
    expect(warnings).not.toHaveBeenCalled();
    warnings.mockRestore();
    // Read only: every formatting control is disabled.
    editor.bind(first, false);
    expect(bold.disabled).toBe(true);
    expect(view.editable).toBe(false);
    // A resync hands over a new doc: the editor shows it in a new view.
    const second = genesisPeer(44n);
    editor.bind(second, true);
    await nextTask();
    expect(editor.editorView).not.toBe(view);
    expect(editor.editorView!.state.doc.textContent).toBe("");
    editor.bind(null, false);
    expect(editor.editorView).toBeNull();
    expect(flushed).toBe(0);
    editor.destroy();
  });

  it("links the selection through its bounded prompt", async () => {
    const editor = new DocumentEditor({ guard: { unsentBytes: () => 0, flush: () => {} }, onCopyMarkdown: () => {}, onComment: () => {} });
    document.body.append(editor.element);
    editor.bind(genesisPeer(55n), true);
    await nextTask();
    const view = editor.editorView!;
    view.dispatch(view.state.tr.insertText("site"));
    select(view, 1, 5);
    editor.element.querySelector<HTMLButtonElement>('[aria-label="Link"]')!.click();
    const form = editor.element.querySelector<HTMLFormElement>(".editor-link-prompt")!;
    const input = form.querySelector("input")!;
    expect(form.hidden).toBe(false);
    expect(input.maxLength).toBe(2048);
    input.value = "javascript:alert(1)";
    form.requestSubmit();
    expect(form.hidden).toBe(false);
    expect(form.querySelector(".editor-link-error")?.textContent).not.toBe("");
    input.value = "https://example.com/";
    form.requestSubmit();
    expect(form.hidden).toBe(true);
    expect(editor.element.querySelector(".ProseMirror a")?.getAttribute("href")).toBe("https://example.com/");
    editor.destroy();
  });
});
