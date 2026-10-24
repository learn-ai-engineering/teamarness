import { UndoManager } from "loro-crdt";
import type { Frontiers, LoroDoc } from "loro-crdt";
import { createNodeFromLoroObj, redo as bindingRedo, undo as bindingUndo } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import { Plugin, PluginKey } from "prosemirror-state";
import type { Command, EditorState, Transaction } from "prosemirror-state";
import {
  BLOCK_BUDGET_BYTES, MEASURE_THRESHOLD_BYTES, TEXT_BUDGET_BYTES, estimateRevertBytes, guardHostOf, isBindingReplay,
  measureTransaction, textBytes,
} from "./guard";
import type { GuardHost, GuardRefusal } from "./guard";
import { MARK_EXPAND, schema } from "./schema";

/** What undoing or redoing one change would write: its update bytes and the text it adds back. */
export type StepCost = Readonly<{ bytes: number; growth: number }>;

/**
 * The undo history holds at most this many steps; Loro drops the oldest beyond it. Each local
 * transaction is a step of its own (the merge interval is 0), and a split paste is one step.
 */
export const UNDO_DEPTH = 500;

/** The commit origin of a change that is not undoable: the undo manager never records it. */
export const OUTSIDE_HISTORY_ORIGIN = "documents:outside-history";

type Refusal = Extract<GuardRefusal, "step_too_large" | "step_unknown">;

/**
 * One editor's per-person undo history, kept so that no undo or redo can write more than one
 * record, without predicting how Loro's undo manager chooses its steps.
 *
 * - **Undo stack.** Before a local change commits, the update its undo would write is measured
 *   ({@link historyPlugin}). A change whose undo cannot fit one record, or cannot be measured, is
 *   committed with {@link OUTSIDE_HISTORY_ORIGIN}, which the manager excludes, so it never becomes
 *   a step and no undo can reach it, including through Loro's skip over a step that became empty.
 *   The next undo or redo is refused with a notice until the next ordinary change.
 * - **Redo stack.** After each undo, the redo it pushed is measured at once. A redo that cannot fit
 *   one record, or cannot be measured, is removed with the whole redo stack, so none is ever
 *   reachable; a redo is then refused until the next undo or change. After each redo, the undo it
 *   pushed is measured the same way, and the undo stack is cleared if it cannot fit.
 * - **Text budget.** Each stack keeps the largest text any of its steps could add back, from when it
 *   was last empty. An undo or redo that could take the body past its text budget is refused.
 */
export class UndoHistory {
  readonly manager: UndoManager;
  private readonly doc: LoroDoc;
  /** Set by the admission of a change that is not undoable, until the commit that writes it. */
  private outsideNext = false;
  /** The undo cost of the change admitted for the next commit. */
  private pendingBytes = 0;
  private pendingGrowth = 0;
  /** A split paste in progress: its pieces commit as one step, and one piece outside makes all. */
  private batch: { bytes: number; growth: number; outside: boolean } | null = null;
  private undoGrowth = 0;
  private redoGrowth = 0;
  private undoRefusal: Refusal | null = null;
  private redoRefusal: Refusal | null = null;

  constructor(doc: LoroDoc) {
    this.doc = doc;
    this.manager = new UndoManager(doc, {
      mergeInterval: 0,
      maxUndoSteps: UNDO_DEPTH,
      excludeOriginPrefixes: [OUTSIDE_HISTORY_ORIGIN],
    });
    // The binding commits each local transaction itself; a change that is not undoable carries
    // the excluded origin instead of the binding's own.
    const commit = doc.commit.bind(doc);
    doc.commit = (options) => {
      if (!this.outsideNext) {
        commit(options);
        return;
      }
      commit({ ...options, origin: OUTSIDE_HISTORY_ORIGIN });
    };
  }

  /**
   * Admits a local change about to commit: `cost` is what its undo would write, or `null` when it
   * could not be measured. Every change commits; only one whose undo fits one record is undoable.
   */
  admit(cost: StepCost | null): void {
    this.pendingBytes += cost?.bytes ?? Number.POSITIVE_INFINITY;
    this.pendingGrowth += Math.max(0, cost?.growth ?? 0);
    const batch = this.batch;
    const bytes = this.pendingBytes + (batch?.bytes ?? 0);
    if (cost === null || bytes > BLOCK_BUDGET_BYTES || batch?.outside === true || this.outsideNext) {
      this.outsideNext = true;
      if (batch !== null) batch.outside = true;
      this.undoRefusal = cost === null ? "step_unknown" : "step_too_large";
      this.redoRefusal = this.undoRefusal;
      return;
    }
    // An undoable change: Loro clears the redo stack when it records it.
    this.undoGrowth = Math.max(this.undoGrowth, this.pendingGrowth + (batch?.growth ?? 0));
    this.redoGrowth = 0;
    this.undoRefusal = null;
    this.redoRefusal = null;
  }

  /** Called once the binding has written what was admitted. */
  written(): void {
    if (this.batch !== null) {
      this.batch.bytes += this.pendingBytes;
      this.batch.growth += this.pendingGrowth;
    }
    this.outsideNext = false;
    this.pendingBytes = 0;
    this.pendingGrowth = 0;
  }

  /** Runs `apply`, whose transactions commit as one undo step (a split paste). */
  batched(apply: () => void): void {
    let grouped = false;
    try {
      this.manager.groupStart();
      grouped = true;
    } catch {
      // The manager is not ready for a group: each piece is then a step of its own.
    }
    this.batch = { bytes: 0, growth: 0, outside: false };
    try {
      apply();
    } finally {
      this.batch = null;
      if (grouped) this.manager.groupEnd();
    }
  }

  /** Undo or redo, guarded (see the class). */
  step(kind: "undo" | "redo", state: EditorState, dispatch: ((tr: Transaction) => void) | undefined, host: GuardHost): boolean {
    if (!this.manager.canUndo()) this.undoGrowth = 0;
    if (!this.manager.canRedo()) this.redoGrowth = 0;
    const available = kind === "undo" ? this.manager.canUndo() : this.manager.canRedo();
    const refusal = kind === "undo" ? this.undoRefusal : this.redoRefusal;
    if (dispatch === undefined) return available || refusal !== null;
    if (refusal !== null) {
      host.refused(refusal);
      return true;
    }
    if (!available) return false;
    host.flush();
    const growth = kind === "undo" ? this.undoGrowth : this.redoGrowth;
    if (growth > 0 && textBytes(state.doc) + growth > TEXT_BUDGET_BYTES) {
      host.refused("document_too_large");
      return true;
    }
    const before = this.doc.frontiers();
    const handled = (kind === "undo" ? bindingUndo : bindingRedo)(state, dispatch);
    if (sameFrontiers(before, this.doc.frontiers())) return handled;
    // What reverting this step would write is the step the manager just pushed on the other stack.
    const cost = revertCost(this.doc, before);
    const fits = cost !== null && cost.bytes <= BLOCK_BUDGET_BYTES;
    const reason: Refusal = cost === null ? "step_unknown" : "step_too_large";
    if (kind === "undo") {
      if (fits) this.redoGrowth = Math.max(this.redoGrowth, cost.growth);
      else this.manager.clearRedo();
      this.redoRefusal = fits ? null : reason;
    } else {
      if (fits) this.undoGrowth = Math.max(this.undoGrowth, cost.growth);
      else this.manager.clearUndo();
      this.undoRefusal = fits ? null : reason;
    }
    return handled;
  }
}

function sameFrontiers(left: Frontiers, right: Frontiers): boolean {
  return left.length === right.length && left.every((id, index) => id.peer === right[index]!.peer && id.counter === right[index]!.counter);
}

/**
 * What returning `doc` to `back` would write, measured on a fork: the update bytes, and the UTF-8
 * text it adds. `null` when it cannot be measured.
 */
function revertCost(doc: LoroDoc, back: Frontiers): StepCost | null {
  try {
    const at = doc.frontiers();
    const fork = doc.forkAt(at);
    fork.setPeerId(doc.peerIdStr);
    fork.configTextStyle(MARK_EXPAND);
    const version = fork.oplogVersion();
    const textBefore = bodyText(fork);
    fork.applyDiff(fork.diff(at, back, false));
    fork.commit();
    return { bytes: fork.export({ mode: "update", from: version }).byteLength, growth: bodyText(fork) - textBefore };
  } catch {
    return null;
  }
}

/** The body's UTF-8 text content; it throws when the body cannot be read. */
function bodyText(doc: LoroDoc): number {
  const root = (doc as unknown as LoroDocType).getMap("doc");
  return root.size === 0 ? 0 : textBytes(createNodeFromLoroObj(schema, root, new Map()));
}

const historyKey = new PluginKey<UndoHistory>("documentsUndoHistory");

/**
 * Admits each local change to the undo history before it commits. It must come after the
 * guard (which may refuse the change) and after `LoroSyncPlugin` (whose appended transaction
 * writes the change), so its own `appendTransaction` runs once the write is done.
 */
export function historyPlugin(history: UndoHistory): Plugin {
  return new Plugin({
    key: historyKey,
    state: { init: () => history, apply: (_tr, value) => value },
    filterTransaction(tr, state) {
      if (!tr.docChanged || isBindingReplay(tr)) return true;
      const growth = Math.max(0, textBytes(state.doc) - textBytes(tr.doc));
      let bytes: number | null = estimateRevertBytes(tr);
      if (bytes > MEASURE_THRESHOLD_BYTES) bytes = measureTransaction(state, tr)?.revertBytes ?? null;
      history.admit(bytes === null ? null : { bytes, growth });
      return true;
    },
    appendTransaction() {
      history.written();
      return null;
    },
  });
}

/** Undo or redo through the editor's history, guarded like any local change; never unguarded. */
function guardedStep(kind: "undo" | "redo"): Command {
  return (state, dispatch) => {
    const history = historyKey.getState(state);
    const host = guardHostOf(state);
    if (history === undefined || host === undefined) return false;
    return history.step(kind, state, dispatch, host);
  };
}

export const guardedUndo: Command = guardedStep("undo");
export const guardedRedo: Command = guardedStep("redo");
