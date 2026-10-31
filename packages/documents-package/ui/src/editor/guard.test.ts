// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LoroDoc } from "loro-crdt";
import { updateLoroToPmState } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import { Fragment, Slice } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { EditorState, TextSelection } from "prosemirror-state";
import type { Transaction } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { afterEach, describe, expect, it } from "vitest";
import {
  BLOCK_BUDGET_BYTES, RECORD_LIMIT_BYTES, TEXT_BUDGET_BYTES, estimateFragmentBytes, estimateNodeBytes,
  estimateTransactionBytes, guardPlugin, planPaste, textBytes,
} from "./guard";
import type { GuardHost, GuardRefusal } from "./guard";
import { MARK_EXPAND, schema } from "./schema";
import { EXPORT_BUDGET_BYTES } from "./session";
import { editorCommands, editorPlugins, initialDocument } from "./view";

const GENESIS = join(import.meta.dirname, "../../../fixtures/body-trees/from-rust/00-genesis.loro");

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

function paragraph(text: string): Node {
  return schema.node("paragraph", null, text ? [schema.text(text)] : []);
}

function doc(...blocks: Node[]): Node {
  return schema.node("doc", null, blocks.length ? blocks : [paragraph("")]);
}

/** Paragraphs of about `bytes` text in total, each `each` bytes. */
function paste(bytes: number, each = 1000): Slice {
  const blocks: Node[] = [];
  for (let made = 0; made < bytes; made += each) blocks.push(paragraph(`${blocks.length} `.padEnd(each, "x")));
  return new Slice(Fragment.fromArray(blocks), 0, 0);
}

class Host implements GuardHost {
  unsent = 0;
  flushes = 0;
  readonly refusals: GuardRefusal[] = [];
  unsentBytes(): number { return this.unsent; }
  flush(): void { this.flushes += 1; this.unsent = 0; }
  refused(reason: GuardRefusal): void { this.refusals.push(reason); }
}

function guarded(host: Host, content: Node = doc()): EditorView {
  const view = new EditorView(document.createElement("div"), {
    state: EditorState.create({ schema, doc: content, plugins: [guardPlugin(host)] }),
  });
  views.push(view);
  return view;
}

function pasteInto(view: EditorView, slice: Slice): boolean {
  return view.someProp("handlePaste", (handler) => handler(view, new Event("paste") as ClipboardEvent, slice)) ?? false;
}

describe("size estimates", () => {
  it("count text as the Backend does: UTF-8 text runs plus a byte per hard break", () => {
    const node = doc(
      schema.node("paragraph", null, [schema.text("ab"), schema.node("hard_break"), schema.text("é", [schema.marks.em.create()])]),
      schema.node("code_block", null, [schema.text("code")]),
      schema.node("horizontal_rule"),
    );
    expect(textBytes(node)).toBe(2 + 1 + 2 + 4);
  });

  it("bound the update a paste really produces", () => {
    const loro = new LoroDoc();
    loro.import(new Uint8Array(readFileSync(GENESIS)));
    loro.setPeerId(7n);
    loro.configTextStyle(MARK_EXPAND);
    const view = new EditorView(document.createElement("div"), {
      state: EditorState.create({ schema, doc: initialDocument(loro), plugins: editorPlugins(loro, new Host(), () => true) }),
    });
    views.push(view);
    const sizes: number[] = [];
    loro.subscribeLocalUpdates((update) => { sizes.push(update.byteLength); });
    const slice = paste(40_000, 250);
    const marked = schema.node("paragraph", null, Array.from({ length: 200 }, (_, index) =>
      schema.text(`w${index} `, index % 2 ? [schema.marks.strong.create()] : [])));
    const content = slice.content.addToEnd(marked);
    view.dispatch(view.state.tr.replaceWith(1, view.state.doc.content.size - 1, content));
    const actual = sizes.reduce((total, size) => total + size, 0);
    expect(actual).toBeGreaterThan(0);
    expect(estimateFragmentBytes(content)).toBeGreaterThanOrEqual(actual);
  });
});

describe("paste plans", () => {
  it("apply a paste within the export budget whole", () => {
    expect(planPaste(doc(), paste(EXPORT_BUDGET_BYTES / 2))).toEqual({ kind: "whole" });
  });

  it("split a large paste at block boundaries into pieces within the budget, in order", () => {
    const slice = paste(150 * 1000);
    const plan = planPaste(doc(), slice);
    expect(plan.kind).toBe("split");
    if (plan.kind !== "split") return;
    expect(plan.pieces.length).toBeGreaterThanOrEqual(3);
    for (const piece of plan.pieces) expect(estimateFragmentBytes(piece)).toBeLessThanOrEqual(EXPORT_BUDGET_BYTES);
    const rejoined = plan.pieces.reduce((all, piece) => all.append(piece), Fragment.empty);
    expect(rejoined.eq(slice.content)).toBe(true);
  });

  it("give a block larger than the export budget a piece of its own", () => {
    const big = paragraph("y".repeat(EXPORT_BUDGET_BYTES + 100));
    const plan = planPaste(doc(), new Slice(Fragment.from([paragraph("a"), big, paragraph("b")]), 0, 0));
    expect(plan.kind).toBe("split");
    if (plan.kind === "split") expect(plan.pieces.map((piece) => piece.childCount)).toEqual([1, 1, 1]);
  });

  it("refuse a single block that cannot fit one record", () => {
    const big = paragraph("z".repeat(BLOCK_BUDGET_BYTES));
    expect(estimateNodeBytes(big)).toBeGreaterThan(BLOCK_BUDGET_BYTES);
    expect(planPaste(doc(), new Slice(Fragment.from([paragraph("a"), big]), 0, 0)))
      .toEqual({ kind: "refused", reason: "block_too_large" });
  });

  it("refuse a paste that grows the body beyond its text budget", () => {
    const current = doc(...Array.from({ length: 150 }, () => paragraph("q".repeat(1000))));
    expect(planPaste(current, paste(60_000))).toEqual({ kind: "refused", reason: "document_too_large" });
    // Replacing more text than it adds is a shrink, which is always allowed.
    expect(planPaste(current, paste(60_000), 70_000).kind).not.toBe("refused");
  });
});

describe("the pre-commit guard", () => {
  it("flushes before a transaction that would take the pending export over the budget", () => {
    const host = new Host();
    const view = guarded(host);
    host.unsent = EXPORT_BUDGET_BYTES - 100;
    view.dispatch(view.state.tr.insertText("small", 1));
    expect(host.flushes).toBe(0);
    host.unsent = EXPORT_BUDGET_BYTES - 100;
    view.dispatch(view.state.tr.insertText("x".repeat(500), 1));
    expect(host.flushes).toBe(1);
    expect(view.state.doc.textContent).toContain("x".repeat(500));
  });

  it("refuses an over-bound block before commit, leaving the document unchanged", () => {
    const host = new Host();
    const view = guarded(host);
    const before = view.state.doc;
    view.dispatch(view.state.tr.insertText("z".repeat(BLOCK_BUDGET_BYTES + 1), 1));
    expect(view.state.doc.eq(before)).toBe(true);
    expect(host.refusals).toEqual(["block_too_large"]);
    expect(pasteInto(view, new Slice(Fragment.from(paragraph("z".repeat(BLOCK_BUDGET_BYTES + 1))), 0, 0))).toBe(true);
    expect(view.state.doc.eq(before)).toBe(true);
    expect(host.refusals).toEqual(["block_too_large", "block_too_large"]);
  });

  it("refuses growth beyond the text budget but allows edits that shrink", () => {
    const host = new Host();
    const blocks = Array.from({ length: 200 }, () => paragraph("q".repeat(1000)));
    const view = guarded(host, doc(...blocks));
    expect(textBytes(view.state.doc)).toBeGreaterThan(TEXT_BUDGET_BYTES);
    const before = view.state.doc;
    view.dispatch(view.state.tr.insertText("more", 1));
    expect(view.state.doc.eq(before)).toBe(true);
    expect(host.refusals).toEqual(["document_too_large"]);
    view.dispatch(view.state.tr.delete(1, 11));
    expect(textBytes(view.state.doc)).toBe(textBytes(before) - 10);
    // Replacing text with less text is a shrink too.
    view.dispatch(view.state.tr.insertText("ab", 1, 6));
    expect(textBytes(view.state.doc)).toBe(textBytes(before) - 13);
    expect(host.refusals).toEqual(["document_too_large"]);
  });

  it("applies a large paste as sequential pieces, flushing after each", () => {
    const host = new Host();
    const view = guarded(host, doc(paragraph("start")));
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 6)));
    // As a browser paste: the first and last paragraphs are open, so the first joins "start".
    const slice = new Slice(paste(150 * 1000).content, 1, 1);
    const flushedAt: number[] = [];
    host.flush = function (this: Host) { flushedAt.push(textBytes(view.state.doc)); this.flushes += 1; };
    expect(pasteInto(view, slice)).toBe(true);
    const plan = planPaste(doc(), slice);
    expect(plan.kind).toBe("split");
    if (plan.kind !== "split") return;
    // One flush before the paste, then one after each piece; each saw the text grow.
    expect(host.flushes).toBe(plan.pieces.length + 1);
    for (let index = 1; index < flushedAt.length; index += 1) expect(flushedAt[index]!).toBeGreaterThan(flushedAt[index - 1]!);
    let text = "start";
    slice.content.forEach((block) => { text += block.textContent; });
    expect(view.state.doc.textContent).toBe(text);
    expect(view.state.doc.childCount).toBe(slice.content.childCount);
    expect(() => view.state.doc.check()).not.toThrow();
    expect(host.refusals).toEqual([]);
  });

  it("leaves a paste within the budget to the default handling", () => {
    const host = new Host();
    const view = guarded(host);
    expect(pasteInto(view, paste(1000))).toBe(false);
  });
});

describe("structural edits", () => {
  /**
   * An editor bound to a Loro body of `blocks`, written by another peer, that records the size of
   * every update it sends.
   */
  async function bound(blocks: Node[]) {
    const loro = new LoroDoc();
    loro.import(new Uint8Array(readFileSync(GENESIS)));
    loro.configTextStyle(MARK_EXPAND);
    loro.setPeerId(5n);
    updateLoroToPmState(loro as unknown as LoroDocType, new Map(), EditorState.create({ schema, doc: doc(...blocks) }));
    loro.setPeerId(77n);
    const sizes: number[] = [];
    loro.subscribeLocalUpdates((update) => { sizes.push(update.byteLength); });
    const host = new Host();
    const view = new EditorView(document.createElement("div"), {
      state: EditorState.create({ schema, doc: initialDocument(loro), plugins: editorPlugins(loro, host, () => true) }),
    });
    views.push(view);
    await new Promise((resolve) => setTimeout(resolve, 0));
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, view.state.doc.content.size - 1)));
    return { loro, view, host, sizes };
  }

  const large = () => Array.from({ length: 170 }, (_, index) => paragraph(`${index} `.padEnd(1000, "x")));

  for (const [name, command] of [
    ["Blockquote", editorCommands.blockquote],
    ["Bullets", editorCommands.bulletList],
    ["Heading", editorCommands.heading(2)],
  ] as const) {
    it(`refuses ${name} over a 166 KB body before commit, since the rewrite cannot fit one record`, async () => {
      const { loro, view, host, sizes } = await bound(large());
      const before = view.state.doc;
      const version = loro.oplogVersion();
      expect(command(view.state, view.dispatch, view)).toBe(true);
      expect(host.refusals).toEqual(["block_too_large"]);
      expect(view.state.doc.eq(before)).toBe(true);
      expect(loro.oplogVersion().compare(version)).toBe(0);
      expect(sizes).toEqual([]);
    });
  }

  it("counts what a structural step rewrites, not only what it inserts", () => {
    const content = doc(...large());
    const state = EditorState.create({ schema, doc: content });
    const select = state.apply(state.tr.setSelection(TextSelection.create(content, 1, content.content.size - 1)));
    let wrapped: Transaction | null = null;
    editorCommands.blockquote(select, (tr) => { wrapped = tr; });
    expect(wrapped).not.toBeNull();
    expect(estimateTransactionBytes(wrapped!)).toBeGreaterThanOrEqual(estimateFragmentBytes(content.content));
  });

  it("allows a structural edit whose real update fits one record", async () => {
    const { view, host, sizes } = await bound(Array.from({ length: 60 }, (_, index) => paragraph(`${index} `.padEnd(1000, "x"))));
    expect(editorCommands.blockquote(view.state, view.dispatch, view)).toBe(true);
    expect(host.refusals).toEqual([]);
    expect(view.state.doc.firstChild?.type.name).toBe("blockquote");
    expect(sizes.length).toBe(1);
    expect(sizes[0]!).toBeGreaterThan(60_000);
    expect(sizes[0]!).toBeLessThanOrEqual(RECORD_LIMIT_BYTES);
  });
});
