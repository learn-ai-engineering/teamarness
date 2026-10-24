import { UndoManager } from "loro-crdt";
import type { LoroDoc, OpId, Value } from "loro-crdt";
import { createNodeFromLoroObj, loroUndoPluginKey, redo as bindingRedo, undo as bindingUndo } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import type { Command, EditorState } from "prosemirror-state";
import { BLOCK_BUDGET_BYTES, TEXT_BUDGET_BYTES, guardHostOf, textBytes } from "./guard";
import { MARK_EXPAND, schema } from "./schema";

/** What undoing or redoing one step would write: its update bytes and its text growth. */
export type StepCost = Readonly<{ bytes: number; growth: number }>;

type Span = { start: number; end: number };

/**
 * The undo manager's step limit. When Loro trims the oldest step at its limit, the step pushed in
 * that same checkpoint loses its meta value, so it could not be identified or measured. The limit
 * is therefore set beyond any session's reach; a step that is still unknown is refused.
 */
export const UNDO_STEP_LIMIT = 1_000_000;

const tracked = new WeakMap<UndoManager, UndoSteps>();

/**
 * The counter span of this peer's operations that each undo and redo step holds, followed from
 * the manager's own pushes. Each push is tagged with a token in its meta value. A step that a
 * later checkpoint merged into keeps its token, so a push whose token is not on top afterwards
 * extended the top step.
 *
 * A span is kept while its step is on either stack, and only then. Loro reports pops, but it
 * clears the redo stack on every new local checkpoint, and both stacks on a checkout or a peer
 * change, without reporting them; the tokens on each stack are followed here so those are seen.
 */
export class UndoSteps {
  readonly manager: UndoManager;
  private readonly doc: LoroDoc;
  private readonly spans = new Map<number, Span>();
  /** The tokens on each stack, bottom first. */
  private readonly undoTokens: number[] = [];
  private readonly redoTokens: number[] = [];
  private nextToken = 1;
  /** The last checkpoint push whose outcome (a new step, or merged into the top) is unread. */
  private unsettled: number | null = null;
  private performing = false;

  constructor(doc: LoroDoc) {
    this.doc = doc;
    this.manager = new UndoManager(doc, { maxUndoSteps: UNDO_STEP_LIMIT });
    const setOnPush = this.manager.setOnPush.bind(this.manager);
    const setOnPop = this.manager.setOnPop.bind(this.manager);
    type OnPush = NonNullable<Parameters<UndoManager["setOnPush"]>[0]>;
    type OnPop = NonNullable<Parameters<UndoManager["setOnPop"]>[0]>;
    // The binding installs its own callbacks for cursors; every push is still tagged.
    this.manager.setOnPush = (listener?: OnPush) => setOnPush((isUndo, range, event) => {
      const meta = listener?.(isUndo, range, event) ?? { value: null, cursors: [] };
      return { ...meta, value: this.pushed(isUndo, range) };
    });
    this.manager.setOnPop = (listener?: OnPop) => setOnPop((isUndo, meta, range) => {
      if (typeof meta.value === "number") this.popped(isUndo, meta.value);
      listener?.(isUndo, meta, range);
    });
    this.manager.setOnPush();
    this.manager.setOnPop();
    tracked.set(this.manager, this);
  }

  /** The tracker of `manager`, if it is tracked. */
  static of(manager: UndoManager): UndoSteps | undefined {
    return tracked.get(manager);
  }

  /**
   * What undoing (or redoing) the top step would write, measured on a fork: the step's own
   * operations are reverted there exactly as the undo manager computes them, before any later
   * change transforms them, and the fork's new operations are exported. `null` when there is no
   * step, or its span is unknown; the guard refuses such a step.
   */
  measure(kind: "undo" | "redo"): StepCost | null {
    this.settle();
    this.forgetCleared();
    const token = kind === "undo" ? this.manager.topUndoValue() : this.manager.topRedoValue();
    const span = typeof token === "number" ? this.spans.get(token) : undefined;
    if (span === undefined || span.end <= span.start) return null;
    const peer = this.manager.peer();
    const last: OpId = { peer, counter: span.end - 1 };
    const change = this.doc.getChangeAt({ peer, counter: span.start });
    const deps: OpId[] = change.counter === span.start ? change.deps : [{ peer, counter: span.start - 1 }];
    const fork = this.doc.forkAt([last]);
    fork.configTextStyle(MARK_EXPAND);
    const before = fork.oplogVersion();
    const textBefore = bodyText(fork);
    fork.applyDiff(fork.diff([last], deps, false));
    fork.commit();
    return { bytes: fork.export({ mode: "update", from: before }).byteLength, growth: bodyText(fork) - textBefore };
  }

  /** Runs the manager's own undo or redo; its pushes are exact steps, never merges. */
  perform<T>(run: () => T): T {
    this.settle();
    this.performing = true;
    try {
      return run();
    } finally {
      this.performing = false;
    }
  }

  private pushed(isUndo: boolean, range: Span): Value {
    this.settle();
    this.forgetCleared();
    const token = this.nextToken;
    this.nextToken += 1;
    this.spans.set(token, { start: range.start, end: range.end });
    (isUndo ? this.undoTokens : this.redoTokens).push(token);
    if (isUndo && !this.performing) {
      // A local change's checkpoint may merge into the top step, and it clears the redo stack.
      this.unsettled = token;
      this.forget(this.redoTokens);
    }
    return token;
  }

  private popped(isUndo: boolean, token: number): void {
    const stack = isUndo ? this.undoTokens : this.redoTokens;
    const at = stack.lastIndexOf(token);
    if (at >= 0) stack.splice(at, 1);
    this.spans.delete(token);
  }

  /** Forgets the tokens of a stack Loro has emptied without reporting it. */
  private forgetCleared(): void {
    if (!this.manager.canUndo()) this.forget(this.undoTokens);
    if (!this.manager.canRedo()) this.forget(this.redoTokens);
  }

  /** Empties `stack` and forgets its spans. */
  private forget(stack: number[]): void {
    for (const token of stack.splice(0)) {
      if (token !== this.unsettled) this.spans.delete(token);
    }
  }

  /** Reads whether the last checkpoint push became a step of its own or extended the top one. */
  private settle(): void {
    const pushed = this.unsettled;
    if (pushed === null) return;
    this.unsettled = null;
    const top = this.manager.topUndoValue();
    if (typeof top !== "number" || top === pushed) return;
    const into = this.spans.get(top);
    const from = this.spans.get(pushed);
    if (into !== undefined && from !== undefined && into.end === from.start) into.end = from.end;
    this.popped(true, pushed);
  }
}

function bodyText(doc: LoroDoc): number {
  const root = (doc as unknown as LoroDocType).getMap("doc");
  if (root.size === 0) return 0;
  try {
    return textBytes(createNodeFromLoroObj(schema, root, new Map()));
  } catch {
    return 0;
  }
}

/**
 * Undo or redo, guarded like any local change: pending changes are sent first, then the step's
 * update is measured before it is applied. A step that cannot fit one record, or that grows the
 * body past its text budget, is refused with the guard notice and stays on its stack.
 */
function guardedStep(kind: "undo" | "redo"): Command {
  const run = kind === "undo" ? bindingUndo : bindingRedo;
  return (state: EditorState, dispatch) => {
    const undoState = loroUndoPluginKey.getState(state) as { undoManager: UndoManager } | undefined;
    if (undoState === undefined) return false;
    const manager = undoState.undoManager;
    if (dispatch === undefined) return kind === "undo" ? manager.canUndo() : manager.canRedo();
    const steps = UndoSteps.of(manager);
    const host = guardHostOf(state);
    if (steps === undefined || host === undefined) return run(state, dispatch);
    if (!(kind === "undo" ? manager.canUndo() : manager.canRedo())) return false;
    host.flush();
    const cost = steps.measure(kind);
    // A step whose span is unknown cannot be measured, so it is refused: the guard fails closed.
    if (cost === null || cost.bytes > BLOCK_BUDGET_BYTES) {
      host.refused("step_too_large");
      return true;
    }
    if (cost.growth > 0 && textBytes(state.doc) + cost.growth > TEXT_BUDGET_BYTES) {
      host.refused("document_too_large");
      return true;
    }
    return steps.perform(() => run(state, dispatch));
  };
}

export const guardedUndo: Command = guardedStep("undo");
export const guardedRedo: Command = guardedStep("redo");
