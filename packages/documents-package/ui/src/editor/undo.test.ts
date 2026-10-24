// @vitest-environment happy-dom
/// <reference types="node" />
// Verifies: TEAM.SYS.DOCS-R09, TEAM.SYS.DOCS-R13 — per-person undo, and changes too large to undo.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LoroDoc, UndoManager, VersionVector } from "loro-crdt";
import type { PeerID } from "loro-crdt";
import { canRedo, canUndo, updateLoroToPmState } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import { Fragment, Slice } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { AllSelection, EditorState, TextSelection } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RECORD_LIMIT_BYTES, textBytes } from "./guard";
import type { GuardHost, GuardRefusal } from "./guard";
import { MARK_EXPAND, schema } from "./schema";
import { EXPORT_BUDGET_BYTES } from "./session";
import { editorCommands, editorPlugins, initialDocument } from "./view";

const GENESIS = join(import.meta.dirname, "../../../fixtures/body-trees/from-rust/00-genesis.loro");

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  vi.restoreAllMocks();
});

/** Every checkpoint lands seconds after the last, as a person edits, so time never merges steps. */
function personPace(): void {
  let now = 1_000_000;
  vi.spyOn(Date, "now").mockImplementation(() => (now += 5000));
}

function nextTask(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

async function settled(): Promise<void> {
  await nextTask();
  await nextTask();
}

function paragraph(text: string): Node {
  return schema.node("paragraph", null, text ? [schema.text(text)] : []);
}

/** `count` paragraphs of `each` bytes of text. */
function paragraphs(count: number, each: number, label = "p"): Node[] {
  return Array.from({ length: count }, (_, index) => paragraph(`${label}${index} `.padEnd(each, "x")));
}

/** A body that another peer wrote, so it is in no local undo history. */
function body(content: Node[]): LoroDoc {
  const doc = new LoroDoc();
  doc.import(new Uint8Array(readFileSync(GENESIS)));
  doc.configTextStyle(MARK_EXPAND);
  if (content.length > 0) {
    doc.setPeerId(5n);
    updateLoroToPmState(doc as unknown as LoroDocType, new Map(), EditorState.create({ schema, doc: schema.node("doc", null, content) }));
  }
  return doc;
}

type Peer = Awaited<ReturnType<typeof editor>>;

/**
 * An editor bound to a Loro body as a session holds it. Its own operations are exported the way
 * the session sends them (never another peer's), and every export is recorded, so a test can see
 * each update's size.
 */
async function editor(source: LoroDoc, peer: bigint) {
  const doc = new LoroDoc();
  doc.import(source.export({ mode: "snapshot" }));
  doc.configTextStyle(MARK_EXPAND);
  doc.setPeerId(peer);
  const id = String(peer) as PeerID;
  let sentCounter = doc.oplogVersion().get(id) ?? 0;
  let unsent = 0;
  const exports: number[] = [];
  const refusals: GuardRefusal[] = [];
  const flush = () => {
    const own = doc.oplogVersion().get(id) ?? 0;
    if (own > sentCounter) {
      const from = doc.oplogVersion().toJSON();
      if (sentCounter === 0) from.delete(id);
      else from.set(id, sentCounter);
      exports.push(doc.export({ mode: "update", from: new VersionVector(from) }).byteLength);
      sentCounter = own;
    }
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
  await settled();
  return { doc, view, exports, refusals, flush, text: () => view.state.doc.textContent };
}

/** Each peer imports what the other has that it lacks, as the relay delivers it. */
async function exchange(a: Peer, b: Peer): Promise<void> {
  a.flush();
  b.flush();
  const toB = a.doc.export({ mode: "update", from: b.doc.oplogVersion() });
  const toA = b.doc.export({ mode: "update", from: a.doc.oplogVersion() });
  b.doc.import(toB);
  a.doc.import(toA);
  await settled();
}

async function step(peer: Peer, command: "undo" | "redo"): Promise<boolean> {
  const handled = editorCommands[command](peer.view.state, peer.view.dispatch, peer.view);
  await settled();
  peer.flush();
  return handled;
}

async function edit(peer: Peer, change: (state: EditorState) => EditorState["tr"]): Promise<void> {
  peer.view.dispatch(change(peer.view.state));
  await settled();
  peer.flush();
}

function pasteInto(view: EditorView, slice: Slice): void {
  const handled = view.someProp("handlePaste", (handler) => handler(view, new Event("paste") as ClipboardEvent, slice));
  if (!handled) view.dispatch(view.state.tr.replaceSelection(slice));
}

const selectAll = (state: EditorState) => state.tr.setSelection(new AllSelection(state.doc));

/** The position just after the first occurrence of `text`. */
function after(state: EditorState, text: string): number {
  let found = -1;
  state.doc.descendants((node, at) => {
    if (found < 0 && node.isText && node.text!.includes(text)) found = at + node.text!.indexOf(text) + text.length;
  });
  if (found < 0) throw new Error(`no ${text}`);
  return found;
}

describe("changes too large to undo", () => {
  it("SF-201: a remote edit that empties the top step never lets undo reach a large change", async () => {
    // A builds a large body itself, deletes it all, then types one character; B deletes that
    // character. Loro's undo skips the emptied step and runs the next one down: that must never
    // be the delete, whose undo cannot fit one record.
    personPace();
    const a = await editor(body([]), 77n);
    for (let round = 0; round < 4; round += 1) {
      a.view.dispatch(a.view.state.tr.setSelection(TextSelection.atEnd(a.view.state.doc)));
      pasteInto(a.view, new Slice(Fragment.fromArray(paragraphs(190, 245, `r${round}p`)), 1, 1));
      await settled();
      a.flush();
    }
    expect(textBytes(a.view.state.doc)).toBeGreaterThan(180 * 1024);
    await edit(a, (state) => selectAll(state).deleteSelection());
    await edit(a, (state) => state.tr.insertText("x"));
    const b = await editor(a.doc, 88n);
    await edit(b, (state) => state.tr.delete(after(state, "x") - 1, after(state, "x")));
    await exchange(a, b);
    expect(a.text()).toBe("");

    a.exports.length = 0;
    for (let press = 0; press < 3; press += 1) await step(a, "undo");
    expect(a.exports.every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
    expect(a.text()).toBe("");
    await exchange(a, b);
    expect(b.text()).toBe(a.text());
  }, 120_000);

  it("a large delete is not undoable: undo is refused and the content stays as it is", async () => {
    const a = await editor(body(paragraphs(186, 1000)), 77n);
    expect(textBytes(a.view.state.doc)).toBeGreaterThan(180 * 1024);
    await edit(a, (state) => state.tr.insertText("mine ", 1));
    await edit(a, (state) => selectAll(state).deleteSelection());
    expect(a.text()).toBe("");
    expect(Math.max(...a.exports)).toBeLessThanOrEqual(RECORD_LIMIT_BYTES);
    a.exports.length = 0;
    const version = a.doc.oplogVersion();

    expect(await step(a, "undo")).toBe(true);
    expect(a.refusals).toEqual(["step_too_large"]);
    expect(a.text()).toBe("");
    expect(a.doc.oplogVersion().compare(version)).toBe(0);
    expect(a.exports).toEqual([]);

    // Ordinary undo works again for the next change.
    await edit(a, (state) => state.tr.insertText("after"));
    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("");
    expect(a.exports.every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
  }, 60_000);

  it("a large paste is one undo step: its undo works and its redo is refused", async () => {
    const a = await editor(body([paragraph("Before the paste.")]), 77n);
    a.view.dispatch(a.view.state.tr.setSelection(TextSelection.atEnd(a.view.state.doc)));
    pasteInto(a.view, new Slice(Fragment.fromArray(paragraphs(700, 245)), 1, 1));
    await settled();
    a.flush();
    expect(a.exports.length).toBeGreaterThanOrEqual(3);
    expect(Math.max(...a.exports)).toBeLessThanOrEqual(RECORD_LIMIT_BYTES);
    expect(textBytes(a.view.state.doc)).toBeGreaterThan(165 * 1024);
    a.exports.length = 0;

    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("Before the paste.");
    expect(a.exports.every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
    a.exports.length = 0;

    // Redoing it would re-insert the whole paste in one update.
    const version = a.doc.oplogVersion();
    expect(await step(a, "redo")).toBe(true);
    expect(a.refusals).toEqual(["step_too_large"]);
    expect(a.text()).toBe("Before the paste.");
    expect(a.doc.oplogVersion().compare(version)).toBe(0);
    expect(a.exports).toEqual([]);
  }, 60_000);

  it("ordinary steps undo one at a time while another peer edits elsewhere and in the same paragraph", async () => {
    const source = body([paragraph("shared start"), paragraph("other")]);
    const a = await editor(source, 77n);
    const b = await editor(source, 88n);
    await edit(a, (state) => state.tr.insertText(" one", after(state, "start")));
    await edit(b, (state) => state.tr.insertText("theirs ", 1));
    await exchange(a, b);
    await edit(a, (state) => state.tr.insertText(" two", after(state, "one")));
    await edit(b, (state) => state.tr.insertText(" too", after(state, "other")));
    await exchange(a, b);
    expect(a.text()).toBe("theirs shared start one twoother too");

    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("theirs shared start oneother too");
    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("theirs shared startother too");
    expect(await step(a, "redo")).toBe(true);
    expect(a.text()).toBe("theirs shared start oneother too");
    await exchange(a, b);
    expect(b.text()).toBe(a.text());
    expect(a.refusals).toEqual([]);
    expect([...a.exports, ...b.exports].every((bytes) => bytes <= RECORD_LIMIT_BYTES)).toBe(true);
  });

  it("an ordinary change made within a second of another is still its own step", async () => {
    const a = await editor(body([paragraph("")]), 77n);
    await edit(a, (state) => state.tr.insertText("first"));
    await edit(a, (state) => state.tr.insertText(" second"));
    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("first");
  });

  it("refuses an undo whose step could not be measured, with its own notice", async () => {
    const a = await editor(body([paragraph("text")]), 77n);
    await edit(a, (state) => state.tr.insertText("typed ", 1));
    const fork = vi.spyOn(LoroDoc.prototype, "forkAt").mockImplementation(() => { throw new Error("unavailable"); });
    expect(await step(a, "undo")).toBe(true);
    expect(a.text()).toBe("text");
    fork.mockRestore();
    // The redo pushed by that undo could not be measured, so it is refused.
    const version = a.doc.oplogVersion();
    expect(await step(a, "redo")).toBe(true);
    expect(a.refusals).toEqual(["step_unknown"]);
    expect(a.text()).toBe("text");
    expect(a.doc.oplogVersion().compare(version)).toBe(0);
  });

  it("never runs undo or redo without the guard", async () => {
    const doc = body([paragraph("text")]);
    doc.setPeerId(77n);
    const plugins = editorPlugins(doc, { unsentBytes: () => 0, flush: () => {}, refused: () => {} }, () => true).slice(1);
    const view = new EditorView(document.createElement("div"), { state: EditorState.create({ schema, doc: initialDocument(doc), plugins }) });
    views.push(view);
    await settled();
    view.dispatch(view.state.tr.insertText("typed ", 1));
    await settled();
    expect(canUndo(view.state)).toBe(true);
    expect(editorCommands.undo(view.state, view.dispatch, view)).toBe(false);
    await settled();
    expect(view.state.doc.textContent).toBe("typed text");
  });

  it("an excluded commit is never reached by undo or redo, even through Loro's no-op skip", () => {
    const seed = new LoroDoc();
    seed.setPeerId(9n);
    seed.getText("t").insert(0, "q".repeat(180 * 1024));
    seed.commit();
    const a = new LoroDoc();
    a.setPeerId(1n);
    a.import(seed.export({ mode: "snapshot" }));
    const manager = new UndoManager(a, { mergeInterval: 0, excludeOriginPrefixes: ["documents:outside-history"] });
    const text = a.getText("t");
    text.insert(0, "ab");
    a.commit();
    text.delete(0, text.length);
    a.commit({ origin: "documents:outside-history" });
    text.insert(0, "x");
    a.commit();
    const b = new LoroDoc();
    b.setPeerId(2n);
    b.import(a.export({ mode: "snapshot" }));
    b.getText("t").delete(0, 1);
    b.commit();
    a.import(b.export({ mode: "update", from: a.oplogVersion() }));
    for (let press = 0; press < 3; press += 1) {
      const before = a.oplogVersion();
      manager.undo();
      expect(a.export({ mode: "update", from: before }).byteLength).toBeLessThan(1024);
      expect(text.length).toBe(0);
    }
    while (manager.canRedo()) {
      const before = a.oplogVersion();
      manager.redo();
      expect(a.export({ mode: "update", from: before }).byteLength).toBeLessThan(1024);
    }
    expect(text.length).toBeLessThanOrEqual(2);
  });
});

/** A small seeded generator (mulberry32). */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

const UNDO_SEEDS = Number(process.env.UNDO_SEEDS ?? 24);
const UNDO_OPERATIONS = Number(process.env.UNDO_OPERATIONS ?? 40);

describe("undo under interleaved local and remote edits (seeded)", () => {
  it(`never admits a local step whose update exceeds one record (${UNDO_SEEDS} seeds)`, async () => {
    for (let seed = 1; seed <= UNDO_SEEDS; seed += 1) {
      vi.restoreAllMocks();
      personPace();
      const next = random(seed);
      const pick = (count: number) => Math.floor(next() * count);
      const source = body(paragraphs(40 + pick(120), 1000, "base"));
      const typed: Array<{ by: number; word: string }> = [];
      const peers = [await editor(source, 77n), await editor(source, 88n)];
      const trace: string[] = [];
      const position = (state: EditorState) => TextSelection.near(state.doc.resolve(pick(state.doc.content.size + 1)));
      for (let operation = 0; operation < UNDO_OPERATIONS; operation += 1) {
        const peer = peers[next() < 0.7 ? 0 : 1]!;
        const roll = next();
        const name = peer === peers[0] ? "A" : "B";
        if (roll < 0.14) {
          trace.push(`${name}:type`);
          const word = `w${operation}q`;
          typed.push({ by: peers.indexOf(peer), word });
          await edit(peer, (state) => state.tr.setSelection(position(state)).insertText(word));
        } else if (roll < 0.2) {
          // Erase a word the other peer typed: its own step there becomes empty.
          const theirs = typed.filter(({ by, word }) => by !== peers.indexOf(peer) && peer.text().includes(word)).at(-1);
          trace.push(`${name}:erase-theirs`);
          if (theirs !== undefined) {
            await edit(peer, (state) => state.tr.delete(after(state, theirs.word) - theirs.word.length, after(state, theirs.word)));
          }
        } else if (roll < 0.3) {
          trace.push(`${name}:delete-range`);
          await edit(peer, (state) => {
            const from = pick(state.doc.content.size);
            const to = Math.min(state.doc.content.size, from + 1 + pick(next() < 0.3 ? 200_000 : 2_000));
            return state.tr.setSelection(TextSelection.between(state.doc.resolve(from), state.doc.resolve(to))).deleteSelection();
          });
        } else if (roll < 0.38) {
          trace.push(`${name}:delete-all`);
          await edit(peer, (state) => selectAll(state).deleteSelection());
        } else if (roll < 0.48) {
          const count = 20 + pick(560);
          trace.push(`${name}:paste-${count}`);
          peer.view.dispatch(peer.view.state.tr.setSelection(position(peer.view.state)));
          pasteInto(peer.view, new Slice(Fragment.fromArray(paragraphs(count, 245, `s${seed}o${operation}p`)), 1, 1));
          await settled();
          peer.flush();
        } else if (roll < 0.52) {
          trace.push(`${name}:bold`);
          await edit(peer, (state) => state.tr.addMark(0, state.doc.content.size, schema.marks.strong.create()));
        } else if (roll < 0.72) {
          const presses = 1 + pick(4);
          trace.push(`${name}:undo×${presses}`);
          for (let press = 0; press < presses; press += 1) await step(peer, "undo");
        } else if (roll < 0.84) {
          const presses = 1 + pick(3);
          trace.push(`${name}:redo×${presses}`);
          for (let press = 0; press < presses; press += 1) await step(peer, "redo");
        } else {
          trace.push("exchange");
          await exchange(peers[0]!, peers[1]!);
        }
        for (const each of peers) {
          const largest = Math.max(0, ...each.exports);
          if (largest > RECORD_LIMIT_BYTES) throw new Error(`seed ${seed}: an update of ${largest} bytes after ${trace.join(", ")}`);
        }
      }
      await exchange(peers[0]!, peers[1]!);
      expect(peers[0]!.text(), `seed ${seed}`).toBe(peers[1]!.text());
      for (const view of views.splice(0)) view.destroy();
    }
  }, 600_000);
});

describe("undo depth", () => {
  it("keeps a bounded number of steps and undoes each of them", async () => {
    const { UNDO_DEPTH } = await import("./undo");
    const a = await editor(body([paragraph("")]), 77n);
    for (let index = 0; index < UNDO_DEPTH + 20; index += 1) {
      a.view.dispatch(a.view.state.tr.insertText("a"));
    }
    await settled();
    a.flush();
    let undone = 0;
    while (canUndo(a.view.state)) {
      expect(await step(a, "undo")).toBe(true);
      undone += 1;
    }
    expect(undone).toBe(UNDO_DEPTH);
    expect(a.text()).toBe("a".repeat(20));
    expect(a.refusals).toEqual([]);
    while (canRedo(a.view.state)) await step(a, "redo");
    expect(a.text()).toBe("a".repeat(UNDO_DEPTH + 20));
  }, 120_000);
});
