import type { TextEvent, TextOp } from "./contract.generated";

/** A minimal common-prefix/suffix delta from `before` to `after`, in Unicode scalar positions. */
export function diffOps(before: string, after: string): TextOp[] {
  const b = Array.from(before);
  const a = Array.from(after);
  let prefix = 0;
  while (prefix < b.length && prefix < a.length && b[prefix] === a[prefix]) prefix += 1;
  let suffix = 0;
  while (
    suffix < b.length - prefix &&
    suffix < a.length - prefix &&
    b[b.length - 1 - suffix] === a[a.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  const deleted = b.length - prefix - suffix;
  const inserted = a.slice(prefix, a.length - suffix).join("");
  const ops: TextOp[] = [];
  if (deleted === 0 && inserted.length === 0) return ops;
  if (prefix > 0) ops.push({ retain: prefix });
  if (deleted > 0) ops.push({ delete: deleted });
  if (inserted.length > 0) ops.push({ insert: inserted });
  return ops;
}

/** Applies a delta to `text`; positions are Unicode scalar positions. */
export function applyOps(text: string, ops: readonly TextOp[]): string {
  let chars = Array.from(text);
  let position = 0;
  for (const op of ops) {
    if ("retain" in op) {
      position += op.retain;
    } else if ("insert" in op) {
      const inserted = Array.from(op.insert);
      chars = chars.slice(0, position).concat(inserted, chars.slice(position));
      position += inserted.length;
    } else {
      chars.splice(position, op.delete);
    }
  }
  return chars.join("");
}

/** Moves a caret (a Unicode scalar position) across a delta someone else applied. */
export function transformCaret(caret: number, ops: readonly TextOp[]): number {
  let position = 0;
  let result = caret;
  for (const op of ops) {
    if ("retain" in op) {
      position += op.retain;
    } else if ("insert" in op) {
      const length = Array.from(op.insert).length;
      if (position <= caret) result += length;
      position += length;
    } else {
      if (position < caret) result -= Math.min(op.delete, caret - position);
    }
    if (position > caret) break;
  }
  return Math.max(0, result);
}

/** UTF-16 offset (what a textarea reports) to a Unicode scalar position. */
export function utf16ToScalar(text: string, offset: number): number {
  return Array.from(text.slice(0, offset)).length;
}

/** Unicode scalar position to a UTF-16 offset for a textarea. */
export function scalarToUtf16(text: string, position: number): number {
  return Array.from(text).slice(0, position).join("").length;
}

export type EditorState = Readonly<{
  documentId: string;
  /** The text the Backend last acknowledged or pushed; edits are diffed against it. */
  text: string;
  revision: string;
  editable: boolean;
  /** An edit request is in flight; further changes wait for its acknowledgement. */
  sending: boolean;
  /** The Backend refused the last edit with a message to show. */
  error: string | null;
}>;

/**
 * Tracks one open Document: what the Backend knows, what the Member has typed since, and the
 * single in-flight edit. A pushed delta is applied to the local draft and the caret so a remote
 * change never loses a keystroke.
 */
export class DocumentEditor {
  private state: EditorState;
  private draft: string;
  private retry: { base: string; ops: TextOp[]; text: string } | null = null;
  private inFlightText: string | null = null;
  private savedRevision: string | null = null;
  private transfer: { revision: string; editable: boolean; chunks: number; parts: string[] } | null = null;
  private abandonedRevision: string | null = null;
  private readonly resync: () => void;
  private readonly send: (documentId: string, baseRevision: string, ops: TextOp[]) => Promise<{ revision: string }>;
  private readonly announce: () => void;

  constructor(
    initial: TextEvent,
    send: (documentId: string, baseRevision: string, ops: TextOp[]) => Promise<{ revision: string }>,
    announce: () => void,
    resync: () => void = () => {},
  ) {
    this.state = {
      documentId: initial.documentId,
      text: initial.text ?? "",
      revision: initial.revision,
      editable: initial.editable,
      sending: false,
      error: null,
    };
    this.draft = this.state.text;
    this.send = send;
    this.announce = announce;
    this.resync = resync;
  }

  get current(): EditorState {
    return this.state;
  }

  get value(): string {
    return this.draft;
  }

  get assembling(): boolean {
    return this.transfer !== null;
  }

  /** The Member typed: record the draft and send the difference when nothing is in flight. */
  input(value: string): void {
    this.draft = value;
    void this.flush();
  }

  /** A Backend event for this Document: a full text or a delta, local echo or remote. */
  receive(event: TextEvent, caret: { start: number; end: number } | null): { start: number; end: number } | null {
    if (event.documentId !== this.state.documentId) return caret;
    if (Number(event.revision) < Number(this.state.revision)) return caret;
    if (event.transfer === "begin") {
      if (this.transfer !== null && this.transfer.revision !== event.revision) this.requestResync(this.transfer.revision);
      this.abandonedRevision = null;
      this.transfer = event.chunks !== undefined && event.chunks > 0
        ? { revision: event.revision, editable: event.editable, chunks: event.chunks, parts: [] } : null;
      if (this.transfer === null) this.requestResync(event.revision);
      return caret;
    }
    if (event.transfer === "chunk" || event.transfer === "end") {
      const transfer = this.transfer;
      if (transfer === null) {
        if (Number(event.revision) > Number(this.state.revision)) this.requestResync(event.revision);
        return caret;
      }
      if (transfer.revision !== event.revision) return caret;
      if (event.transfer === "chunk") {
        if (event.chunkIndex !== transfer.parts.length || event.chunk === undefined || transfer.parts.length >= transfer.chunks) {
          this.transfer = null;
          this.requestResync(event.revision);
        } else transfer.parts.push(event.chunk);
        return caret;
      }
      this.transfer = null;
      if (transfer.parts.length !== transfer.chunks) { this.requestResync(event.revision); return caret; }
      return this.receive({ documentId: event.documentId, revision: event.revision,
        editable: transfer.editable, text: transfer.parts.join(""), savedRevision: event.savedRevision }, caret);
    }
    if (event.text === undefined && Number(event.revision) > Number(this.state.revision) + 1) {
      this.transfer = null;
      this.requestResync(event.revision);
      return caret;
    }
    if (event.text !== undefined) {
      this.transfer = null;
      this.abandonedRevision = null;
      const base = this.state.text;
      const previousDraft = this.draft;
      // A lagged full recovery can already contain the in-flight edit even when its echo
      // was missed. In that case only the typing *after* that edit needs rebasing.
      const pendingBase = this.inFlightText !== null && event.text.includes(this.inFlightText)
        ? this.inFlightText : base;
      const applied = diffOps(pendingBase, event.text);
      this.state = { ...this.state, text: event.text, revision: event.revision, editable: event.editable };
      this.draft = previousDraft === pendingBase || previousDraft === event.text
        ? event.text : applyOps(event.text, rebase(diffOps(pendingBase, previousDraft), applied));
      if (event.savedRevision !== undefined) this.savedRevision = event.savedRevision;
      if (this.retry !== null && event.savedRevision === String(Number(this.retry.base) + 1)) {
        this.retry = null;
        this.inFlightText = null;
        this.state = { ...this.state, error: null };
      }
      this.announce();
      if (this.retry === null || this.state.error === null) void this.flush();
      return caret === null ? null : { start: Math.min(caret.start, this.draft.length), end: Math.min(caret.end, this.draft.length) };
    }
    const delta = event.delta ?? [];
    const base = this.state.text;
    const nextBase = applyOps(base, delta);
    this.state = { ...this.state, text: nextBase, revision: event.revision, editable: event.editable };
    if (event.origin === "local") {
      // Our own change coming back: the draft already contains it.
      this.announce();
      return caret;
    }
    // Carry the unsent local difference over the remote change, then send it again.
    // Positions are rebased by caret transformation, which is exact unless both sides edited
    // the same characters inside one coalescing window; the Backend's text stays authoritative.
    const pending = diffOps(base, this.draft);
    const previousDraft = this.draft;
    this.draft = applyOps(nextBase, rebase(pending, delta));
    let next = caret;
    if (caret !== null) {
      const start = scalarToUtf16(this.draft, transformCaret(utf16ToScalar(previousDraft, caret.start), delta));
      const end = scalarToUtf16(this.draft, transformCaret(utf16ToScalar(previousDraft, caret.end), delta));
      next = { start, end };
    }
    this.announce();
    void this.flush();
    return next;
  }

  private requestResync(revision: string): void {
    if (this.abandonedRevision === revision) return;
    this.abandonedRevision = revision;
    this.resync();
  }

  private async flush(): Promise<void> {
    if (this.state.sending || !this.state.editable) return;
    const ops = this.retry?.ops ?? diffOps(this.state.text, this.draft);
    if (ops.length === 0) return;
    const base = this.retry?.base ?? this.state.revision;
    const sent = this.retry?.text ?? this.draft;
    this.state = { ...this.state, sending: true, error: null };
    this.inFlightText = sent;
    this.announce();
    try {
      const { revision } = await this.send(this.state.documentId, base, ops);
      this.retry = null;
      this.inFlightText = null;
      this.state = Number(this.state.revision) > Number(revision)
        ? { ...this.state, sending: false }
        : { ...this.state, text: sent, revision, sending: false };
    } catch (error) {
      if (this.savedRevision === String(Number(base) + 1)) {
        this.retry = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false, error: null };
        this.announce();
        void this.flush();
        return;
      }
      const code = (error as { code?: unknown } | null)?.code;
      if (code === "documents.stale_revision") {
        // A delta is on its way; it will re-flush once applied.
        this.state = { ...this.state, sending: false };
        return;
      }
      this.retry = { base, ops, text: sent };
      this.state = { ...this.state, sending: false, error: describe(code) };
      this.announce();
      return;
    }
    this.announce();
    void this.flush();
  }
}

function describe(code: unknown): string {
  switch (code) {
    case "documents.document_too_large":
      return "This document has reached its size limit.";
    case "documents.document_pending":
      return "Waiting for this document's history before edits can be made.";
    default:
      return "The edit could not be saved.";
  }
}

/**
 * Rewrites `pending` (made against the old base) so it applies to the base after `applied`.
 * Retains and deletes skip characters of the new base; each old position maps through the
 * caret transformation of `applied`.
 */
function rebase(pending: readonly TextOp[], applied: readonly TextOp[]): TextOp[] {
  const out: TextOp[] = [];
  let oldPosition = 0;
  let newPosition = 0;
  for (const op of pending) {
    if ("retain" in op) {
      oldPosition += op.retain;
      const target = transformCaret(oldPosition, applied);
      if (target > newPosition) out.push({ retain: target - newPosition });
      newPosition = Math.max(newPosition, target);
    } else if ("insert" in op) {
      out.push(op);
    } else {
      oldPosition += op.delete;
      const target = transformCaret(oldPosition, applied);
      if (target > newPosition) out.push({ delete: target - newPosition });
      newPosition = Math.max(newPosition, target);
    }
  }
  return out;
}
