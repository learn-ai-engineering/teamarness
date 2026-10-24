import { Fragment, Slice } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { Plugin, PluginKey, TextSelection } from "prosemirror-state";
import type { EditorState, Transaction } from "prosemirror-state";
import type { EditorView } from "prosemirror-view";
import { loroSyncPluginKey } from "loro-prosemirror";
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

/** Why the guard refused a change. Nothing was committed. */
export type GuardRefusal = "block_too_large" | "document_too_large";

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

/** An upper estimate of the update bytes a transaction adds: everything its steps insert. */
export function estimateTransactionBytes(tr: Transaction): number {
  let total = 0;
  for (const step of tr.steps) {
    const slice = (step as { slice?: Slice }).slice;
    if (slice !== undefined) total += estimateFragmentBytes(slice.content);
    else total += NODE_OVERHEAD_BYTES;
  }
  return total;
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

const guardKey = new PluginKey("documentsGuard");

/** Whether a transaction is the binding replaying the Loro document, which the guard never filters. */
function isBindingReplay(tr: Transaction): boolean {
  return tr.getMeta(loroSyncPluginKey) !== undefined;
}

/**
 * The pre-commit size guard.
 *
 * - Before a transaction applies, its growth is estimated; when the pending export would then
 *   exceed {@link EXPORT_BUDGET_BYTES}, the session flushes first.
 * - A transaction that inserts more than {@link BLOCK_BUDGET_BYTES} at once is refused.
 * - A transaction that grows text content beyond {@link TEXT_BUDGET_BYTES} is refused; one that
 *   shrinks, or keeps, the size is always allowed.
 * - A paste larger than the export budget is split at block boundaries ({@link planPaste}).
 */
export function guardPlugin(host: GuardHost): Plugin {
  return new Plugin({
    key: guardKey,
    filterTransaction(tr: Transaction, state: EditorState): boolean {
      if (!tr.docChanged || isBindingReplay(tr)) return true;
      const before = textBytes(state.doc);
      const after = textBytes(tr.doc);
      if (after > TEXT_BUDGET_BYTES && after > before) {
        host.refused("document_too_large");
        return false;
      }
      const growth = estimateTransactionBytes(tr);
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
        applySplitPaste(view, slice, plan.pieces, () => host.flush());
        return true;
      },
    },
  });
}
