// @vitest-environment happy-dom
/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LoroDoc, VersionVector } from "loro-crdt";
import { canRedo, canUndo, updateLoroToPmState } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import { Fragment, Slice } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { AllSelection, EditorState, TextSelection } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { afterEach, describe, expect, it } from "vitest";
import { RECORD_LIMIT_BYTES } from "./guard";
import type { GuardHost, GuardRefusal } from "./guard";
import { MARK_EXPAND, schema } from "./schema";
import { EXPORT_BUDGET_BYTES } from "./session";
import { editorCommands, editorPlugins, initialDocument } from "./view";

const GENESIS = join(import.meta.dirname, "../../../fixtures/body-trees/from-rust/00-genesis.loro");

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

function nextTask(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function paragraph(text: string): Node {
  return schema.node("paragraph", null, text ? [schema.text(text)] : []);
}

/** `count` paragraphs of `each` bytes of text. */
function paragraphs(count: number, each: number): Node[] {
  return Array.from({ length: count }, (_, index) => paragraph(`p${index} `.padEnd(each, "x")));
}

/**
 * An editor bound to a Loro body as a session holds it: every export is recorded, the way the
 * session sends it, so a test can see each update's size. `content` is written into the body by
 * another peer first, so it is not in this peer's undo history.
 */
async function editor(content: Node[] = []) {
  const doc = new LoroDoc();
  doc.import(new Uint8Array(readFileSync(GENESIS)));
  doc.configTextStyle(MARK_EXPAND);
  if (content.length > 0) {
    doc.setPeerId(5n);
    const state = EditorState.create({ schema, doc: schema.node("doc", null, content) });
    updateLoroToPmState(doc as unknown as LoroDocType, new Map(), state);
  }
  doc.setPeerId(77n);
  let sent = doc.oplogVersion();
  let unsent = 0;
  const exports: number[] = [];
  const refusals: GuardRefusal[] = [];
  const flush = () => {
    if (unsent === 0) return;
    exports.push(doc.export({ mode: "update", from: new VersionVector(sent.toJSON()) }).byteLength);
    sent = doc.oplogVersion();
    unsent = 0;
  };
  doc.subscribeLocalUpdates((bytes) => {
    unsent += bytes.byteLength;
    if (unsent >= EXPORT_BUDGET_BYTES) flush();
  });
  const host: GuardHost = { unsentBytes: () => unsent, flush, refused: (reason) => refusals.push(reason) };
  const view = new EditorView(document.createElement("div"), {
    state: EditorState.create({ schema, doc: initialDocument(doc), plugins: editorPlugins(doc, host, () => true) }),
  });
  views.push(view);
  await nextTask();
  await nextTask();
  return { doc, view, exports, refusals, flush };
}

async function step(view: EditorView, command: "undo" | "redo", flush: () => void): Promise<boolean> {
  const handled = editorCommands[command](view.state, view.dispatch, view);
  await nextTask();
  await nextTask();
  flush();
  return handled;
}

function pasteInto(view: EditorView, slice: Slice): void {
  const handled = view.someProp("handlePaste", (handler) => handler(view, new Event("paste") as ClipboardEvent, slice));
  if (!handled) view.dispatch(view.state.tr.replaceSelection(slice));
}

describe("guarded undo and redo", () => {
  it("refuses a redo whose update cannot fit one record, before anything is committed", async () => {
    const { doc, view, exports, refusals, flush } = await editor();
    // A 150 KB paste is split into records, but the undo manager merges the pieces into one step.
    pasteInto(view, new Slice(Fragment.fromArray(paragraphs(610, 245)), 1, 1));
    flush();
    expect(exports.length).toBeGreaterThanOrEqual(3);
    expect(Math.max(...exports)).toBeLessThanOrEqual(RECORD_LIMIT_BYTES);
    exports.length = 0;
    const pasted = view.state.doc;

    // Undoing the paste only deletes: it fits.
    expect(await step(view, "undo", flush)).toBe(true);
    expect(view.state.doc.textContent).toBe("");
    expect(exports.every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
    exports.length = 0;

    // Redoing it would re-insert 150 KB in one update: refused, nothing changes, and it stays available.
    const version = doc.oplogVersion();
    const before = view.state.doc;
    expect(await step(view, "redo", flush)).toBe(true);
    expect(refusals).toEqual(["step_too_large"]);
    expect(view.state.doc.eq(before)).toBe(true);
    expect(doc.oplogVersion().compare(version)).toBe(0);
    expect(exports).toEqual([]);
    expect(canRedo(view.state)).toBe(true);
    expect(pasted.textContent.length).toBeGreaterThan(140_000);
  });

  it("refuses an undo of a delete-all on a 180 KiB body, leaving the step to undo", async () => {
    const { doc, view, exports, refusals, flush } = await editor(paragraphs(186, 1000));
    expect(view.state.doc.textContent.length).toBeGreaterThan(180 * 1024);
    view.dispatch(view.state.tr.setSelection(new AllSelection(view.state.doc)).deleteSelection());
    flush();
    expect(view.state.doc.textContent).toBe("");
    expect(Math.max(...exports)).toBeLessThanOrEqual(RECORD_LIMIT_BYTES);
    exports.length = 0;

    const version = doc.oplogVersion();
    expect(await step(view, "undo", flush)).toBe(true);
    expect(refusals).toEqual(["step_too_large"]);
    expect(view.state.doc.textContent).toBe("");
    expect(doc.oplogVersion().compare(version)).toBe(0);
    expect(exports).toEqual([]);
    expect(canUndo(view.state)).toBe(true);
  });

  it("still undoes and redoes ordinary steps, each within one record", async () => {
    const { view, exports, refusals, flush } = await editor(paragraphs(60, 1000));
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, 20_000)).deleteSelection());
    flush();
    const deleted = view.state.doc;
    expect(await step(view, "undo", flush)).toBe(true);
    expect(view.state.doc.textContent.length).toBeGreaterThan(deleted.textContent.length);
    expect(await step(view, "redo", flush)).toBe(true);
    expect(view.state.doc.textContent).toBe(deleted.textContent);
    expect(refusals).toEqual([]);
    expect(exports.every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
  });
});
