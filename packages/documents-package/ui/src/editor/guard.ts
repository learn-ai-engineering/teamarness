import { Fragment, Slice } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { Plugin, PluginKey, TextSelection } from "prosemirror-state";
import type { EditorState, Transaction } from "prosemirror-state";
import { AddMarkStep, RemoveMarkStep, ReplaceAroundStep, ReplaceStep } from "prosemirror-transform";
import type { Step } from "prosemirror-transform";
import type { EditorView } from "prosemirror-view";
import { loroSyncPluginKey, updateLoroToPmState } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import type { LoroDoc } from "loro-crdt";
import { MARK_EXPAND } from "./schema";
import { EXPORT_BUDGET_BYTES } from "./session";

/** One record carries at most this much update (the Backend's `MAX_UPDATE`). */
export const RECORD_LIMIT_BYTES = 160 * 1024;
/**
 * A single block whose estimated export exceeds this is refused before commit. The margin below
 * {@link RECORD_LIMIT_BYTES} covers what the estimate does not model exactly.
 */
export const BLOCK_BUDGET_BYTES = RECORD_LIMIT_BYTES - 8 * 1024;
/** Local growth stops at this much UTF-8 text content (the Backend's `MAX_TEXT_BYTES`). */
export const TEXT_BUDGET_BYTES = 192 * 1024;

/**
 * What the estimate adds for each node: its Loro map, `attributes` map and `children` list. The
 * spike measured about 76 bytes per pasted paragraph beyond its text.
 */
const NODE_OVERHEAD_BYTES = 96;
/** What the estimate adds for each mark on a text run. */
const MARK_OVERHEAD_BYTES = 24;
/**
 * A transaction estimated above this is measured exactly before it commits: the binding's own
 * write is run on a fork of the body and its update exported. Below it, the estimate cannot be
 * far enough off to reach a record.
 */
export const MEASURE_THRESHOLD_BYTES = EXPORT_BUDGET_BYTES / 2;

/**
 * Why the guard refused a change. Nothing was committed.
 *
 * - `block_too_large`: one change, or one block, cannot fit one record.
 * - `document_too_large`: the change would grow the body past its text budget.
 * - `step_too_large`: the change to undo or redo is too large to undo or redo as one record.
 * - `step_unknown`: the change to undo or redo could not be measured, so it is not undone or redone.
 */
export type GuardRefusal = "block_too_large" | "document_too_large" | "step_too_large" | "step_unknown";

/** What the guard needs from the editor session. */
export interface GuardHost {
  /** Local update bytes not yet exported. */
  unsentBytes(): number;
  /** Exports and queues the local changes now. */
  flush(): void;
  /** Called when a change is refused. */
  refused(reason: GuardRefusal): void;
}

const encoder = new TextEncoder();
const textCache = new WeakMap<Node, number>();
const exportCache = new WeakMap<Node, number>();

function utf8(text: string): number {
  // Fast path: ASCII text is one byte per code unit.
  // eslint-disable-next-line no-control-regex
  return /^[\x00-\x7f]*$/.test(text) ? text.length : encoder.encode(text).length;
}

/**
 * The UTF-8 bytes of text content in `node`, as the Backend counts them: every text run, and one
 * byte for each hard break. Persistent nodes are cached, so an edit re-counts only what changed.
 */
export function textBytes(node: Node): number {
  const cached = textCache.get(node);
  if (cached !== undefined) return cached;
  let total: number;
  if (node.isText) total = utf8(node.text ?? "");
  else if (node.type.name === "hard_break") total = 1;
  else {
    total = 0;
    node.forEach((child) => { total += textBytes(child); });
  }
  textCache.set(node, total);
  return total;
}

/** An upper estimate of the update bytes `node` adds to the body when it is inserted. */
export function estimateNodeBytes(node: Node): number {
  const cached = exportCache.get(node);
  if (cached !== undefined) return cached;
  let total: number;
  if (node.isText) total = utf8(node.text ?? "") + node.marks.length * MARK_OVERHEAD_BYTES;
  else {
    total = NODE_OVERHEAD_BYTES;
    node.forEach((child) => { total += estimateNodeBytes(child); });
  }
  exportCache.set(node, total);
  return total;
}

export function estimateFragmentBytes(fragment: Fragment): number {
  let total = 0;
  fragment.forEach((child) => { total += estimateNodeBytes(child); });
  return total;
}

/**
 * An upper estimate of the update bytes a transaction adds: everything its steps insert, and
 * everything a structural step makes the binding rewrite. The binding replaces a node whose type
 * changes, and a parent whose children are wrapped or lifted, with a full copy, so a step that
 * is not confined to one text block counts every top-level block it touches.
 */
export function estimateTransactionBytes(tr: Transaction): number {
  let total = 0;
  tr.steps.forEach((step, index) => {
    total += estimateStepBytes(step, tr.docs[index]!, tr.docs[index + 1] ?? tr.doc);
  });
  return total;
}

function estimateStepBytes(step: Step, before: Node, after: Node): number {
  if (step instanceof AddMarkStep || step instanceof RemoveMarkStep) {
    // A mark is one range operation in each text run it covers.
    let runs = 0;
    before.nodesBetween(step.from, step.to, (node) => { if (node.isTextblock) runs += 1; });
    return Math.max(1, runs) * MARK_OVERHEAD_BYTES;
  }
  if (!(step instanceof ReplaceStep || step instanceof ReplaceAroundStep)) return NODE_OVERHEAD_BYTES;
  const inserted = estimateFragmentBytes(step.slice.content);
  if (step instanceof ReplaceStep && withinTextblock(before, step.from, step.to)) {
    const map = step.getMap();
    if (withinTextblock(after, map.map(step.from, -1), map.map(step.to, 1))) return inserted;
  }
  return inserted + touchedBlockBytes(before, step.from, step.to);
}

/** Whether `from` and `to` lie in one text block of `doc`. */
function withinTextblock(doc: Node, from: number, to: number): boolean {
  const $from = doc.resolve(from);
  return $from.parent.isTextblock && $from.sameParent(doc.resolve(to));
}

/**
 * An upper estimate of the update bytes undoing a transaction writes: undoing an insertion only
 * deletes, while undoing a removal re-creates what was removed, and undoing a structural step
 * re-creates every top-level block it touched.
 */
export function estimateRevertBytes(tr: Transaction): number {
  let total = 0;
  tr.steps.forEach((step, index) => {
    total += estimateStepRevertBytes(step, tr.docs[index]!, tr.docs[index + 1] ?? tr.doc);
  });
  return total;
}

function estimateStepRevertBytes(step: Step, before: Node, after: Node): number {
  if (step instanceof AddMarkStep || step instanceof RemoveMarkStep) return estimateStepBytes(step, before, after);
  if (!(step instanceof ReplaceStep || step instanceof ReplaceAroundStep)) return NODE_OVERHEAD_BYTES;
  const removed = NODE_OVERHEAD_BYTES + estimateFragmentBytes(before.slice(step.from, step.to).content);
  if (step instanceof ReplaceStep && withinTextblock(before, step.from, step.to)) {
    const map = step.getMap();
    if (withinTextblock(after, map.map(step.from, -1), map.map(step.to, 1))) return removed;
  }
  return removed + touchedBlockBytes(before, step.from, step.to);
}

/** The estimate of every top-level block of `doc` that the range from `from` to `to` touches. */
function touchedBlockBytes(doc: Node, from: number, to: number): number {
  const $from = doc.resolve(from);
  const $to = doc.resolve(to);
  const start = $from.index(0);
  const end = $to.depth === 0 ? $to.index(0) : $to.index(0) + 1;
  let total = 0;
  for (let index = start; index < end && index < doc.childCount; index += 1) total += estimateNodeBytes(doc.child(index));
  return total;
}

/** What a transaction writes (`bytes`), and what undoing it would write (`revertBytes`). */
export type TransactionCost = Readonly<{ bytes: number; revertBytes: number }>;

const measured = new WeakMap<Transaction, TransactionCost | null>();

/**
 * The exact update bytes the binding would write for `tr`, and those its undo would write: the
 * binding's write runs on a fork of the body, as this peer, the fork's new operations are
 * exported, and then the fork is returned to where it was and that update is exported too. `null`
 * when the editor is not bound or the measurement fails. Each transaction is measured once.
 */
export function measureTransaction(state: EditorState, tr: Transaction): TransactionCost | null {
  if (measured.has(tr)) return measured.get(tr) ?? null;
  let cost: TransactionCost | null = null;
  const sync = loroSyncPluginKey.getState(state) as { doc?: LoroDoc; mapping?: Map<unknown, unknown> } | undefined;
  const doc = sync?.doc;
  if (doc !== undefined && sync?.mapping !== undefined) {
    try {
      const fork = doc.fork();
      fork.setPeerId(doc.peerIdStr);
      fork.configTextStyle(MARK_EXPAND);
      const before = fork.oplogVersion();
      const start = fork.frontiers();
      const mapping = new Map(sync.mapping) as Parameters<typeof updateLoroToPmState>[1];
      // The binding remembers, per ProseMirror node, the container it wrote it to. A copy of the
      // document keeps the fork's containers off the editor's own nodes.
      const copy = tr.doc.type.schema.nodeFromJSON(tr.doc.toJSON());
      updateLoroToPmState(fork as unknown as LoroDocType, mapping, { doc: copy } as EditorState);
      const bytes = fork.export({ mode: "update", from: before }).byteLength;
      const written = fork.oplogVersion();
      const end = fork.frontiers();
      fork.applyDiff(fork.diff(end, start, false));
      fork.commit();
      cost = { bytes, revertBytes: fork.export({ mode: "update", from: written }).byteLength };
    } catch {
      cost = null;
    }
  }
  measured.set(tr, cost);
  return cost;
}

/** A paste too large for one export, cut at top-level block boundaries. */
export type PastePlan =
  | Readonly<{ kind: "whole" }>
  | Readonly<{ kind: "split"; pieces: readonly Fragment[] }>
  | Readonly<{ kind: "refused"; reason: GuardRefusal }>;

/**
 * Decides how a paste is applied. A paste within {@link EXPORT_BUDGET_BYTES} applies whole. A
 * larger one is cut at block boundaries into pieces of at most that budget (a block larger than
 * the budget is a piece of its own). A paste is refused when any one block cannot fit a record,
 * or when it would grow the body beyond {@link TEXT_BUDGET_BYTES}; `replacedBytes` is the text
 * the paste replaces.
 */
export function planPaste(doc: Node, slice: Slice, replacedBytes = 0): PastePlan {
  const content = slice.content;
  const growth = fragmentTextBytes(content) - replacedBytes;
  if (growth > 0 && textBytes(doc) + growth > TEXT_BUDGET_BYTES) return { kind: "refused", reason: "document_too_large" };
  let oversized = false;
  content.forEach((child) => { if (estimateNodeBytes(child) > BLOCK_BUDGET_BYTES) oversized = true; });
  if (oversized) return { kind: "refused", reason: "block_too_large" };
  if (estimateFragmentBytes(content) <= EXPORT_BUDGET_BYTES) return { kind: "whole" };
  const pieces: Fragment[] = [];
  let current: Node[] = [];
  let size = 0;
  content.forEach((child) => {
    const bytes = estimateNodeBytes(child);
    if (current.length > 0 && size + bytes > EXPORT_BUDGET_BYTES) {
      pieces.push(Fragment.fromArray(current));
      current = [];
      size = 0;
    }
    current.push(child);
    size += bytes;
  });
  if (current.length > 0) pieces.push(Fragment.fromArray(current));
  return pieces.length === 1 ? { kind: "whole" } : { kind: "split", pieces };
}

function fragmentTextBytes(fragment: Fragment): number {
  let total = 0;
  fragment.forEach((child) => { total += textBytes(child); });
  return total;
}

/**
 * Applies a split paste: the first piece replaces the selection (keeping the paste's open start),
 * and each later piece is inserted after the block the previous one ended in. `afterEach` runs
 * after every piece, so each is exported and queued before the next is applied.
 */
export function applySplitPaste(view: EditorView, slice: Slice, pieces: readonly Fragment[], afterEach: () => void): void {
  pieces.forEach((piece, index) => {
    const last = index === pieces.length - 1;
    const state = view.state;
    let tr: Transaction;
    if (index === 0) {
      tr = state.tr.replaceSelection(new Slice(piece, slice.openStart, last ? slice.openEnd : 0));
    } else {
      const $to = state.selection.$to;
      const at = $to.depth > 0 ? $to.after(1) : $to.pos;
      tr = state.tr.replace(at, at, new Slice(piece, 0, last ? slice.openEnd : 0));
      const end = tr.mapping.map(at, 1);
      tr.setSelection(TextSelection.near(tr.doc.resolve(Math.min(end, tr.doc.content.size)), -1));
    }
    view.dispatch(tr.setMeta("paste", true).setMeta("uiEvent", "paste").setMeta(guardKey, "split").scrollIntoView());
    afterEach();
  });
}

const guardKey = new PluginKey<GuardHost>("documentsGuard");

export type GuardOptions = Readonly<{
  /** Runs the pieces of a split paste, so that the undo history records them as one step. */
  batched?: (apply: () => void) => void;
}>;

/** The guard host of an editor state, for the guarded undo and redo commands. */
export function guardHostOf(state: EditorState): GuardHost | undefined {
  return guardKey.getState(state);
}

/** Whether a transaction is the binding replaying the Loro document, which the guard never filters. */
export function isBindingReplay(tr: Transaction): boolean {
  return tr.getMeta(loroSyncPluginKey) !== undefined;
}

/**
 * The pre-commit size guard.
 *
 * - Before a transaction applies, its growth is estimated ({@link estimateTransactionBytes}), and
 *   measured exactly on a fork when the estimate is above {@link MEASURE_THRESHOLD_BYTES}; when
 *   the pending export would then exceed {@link EXPORT_BUDGET_BYTES}, the session flushes first.
 * - A transaction whose update would exceed {@link BLOCK_BUDGET_BYTES} is refused, including a
 *   structural edit that makes the binding rewrite large blocks.
 * - A transaction that grows text content beyond {@link TEXT_BUDGET_BYTES} is refused; one that
 *   shrinks, or keeps, the size is always allowed.
 * - A paste larger than the export budget is split at block boundaries ({@link planPaste}); its
 *   pieces are applied inside `options.batched`, which makes them one undo step.
 */
export function guardPlugin(host: GuardHost, options: GuardOptions = {}): Plugin {
  const batched = options.batched ?? ((apply: () => void) => apply());
  return new Plugin({
    key: guardKey,
    state: { init: () => host, apply: (_tr, value) => value },
    filterTransaction(tr: Transaction, state: EditorState): boolean {
      if (!tr.docChanged || isBindingReplay(tr)) return true;
      const before = textBytes(state.doc);
      const after = textBytes(tr.doc);
      if (after > TEXT_BUDGET_BYTES && after > before) {
        host.refused("document_too_large");
        return false;
      }
      let growth = estimateTransactionBytes(tr);
      if (growth > MEASURE_THRESHOLD_BYTES) growth = measureTransaction(state, tr)?.bytes ?? growth;
      if (growth > BLOCK_BUDGET_BYTES) {
        host.refused("block_too_large");
        return false;
      }
      if (host.unsentBytes() > 0 && host.unsentBytes() + growth > EXPORT_BUDGET_BYTES) host.flush();
      return true;
    },
    props: {
      handlePaste(view, _event, slice) {
        const plan = planPaste(view.state.doc, slice, fragmentTextBytes(view.state.selection.content().content));
        if (plan.kind === "whole") return false;
        if (plan.kind === "refused") {
          host.refused(plan.reason);
          return true;
        }
        host.flush();
        batched(() => applySplitPaste(view, slice, plan.pieces, () => host.flush()));
        return true;
      },
    },
  });
}
