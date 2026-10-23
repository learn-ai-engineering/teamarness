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
  reason?: "identity_conflict" | "import_failure";
  diagnosticRecordId?: string;
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
  private retry: { id: string; base: string; ops: TextOp[]; text: string } | null = null;
  private activeRequestId: string | null = null;
  private recoveryId: string | null = null;
  private chunkLimit = 8192;
  private inFlightText: string | null = null;
  private inFlightBase: string | null = null;
  private savedRevision: string | null = null;
  private transfer: { revision: string; editable: boolean; chunks: number; parts: string[]; transferId?: string; requestStatuses?: TextEvent["requestStatuses"]; reason?: TextEvent["reason"]; diagnosticRecordId?: string } | null = null;
  private abandonedRevision: string | null = null;
  private readonly resync: () => void;
  private readonly send: (documentId: string, baseRevision: string, ops: TextOp[], requestId: string) => Promise<{ revision: string }>;
  private readonly announce: () => void;

  constructor(
    initial: TextEvent,
    send: (documentId: string, baseRevision: string, ops: TextOp[], requestId: string) => Promise<{ revision: string }>,
    announce: () => void,
    resync: () => void = () => {},
  ) {
    this.state = {
      documentId: initial.documentId,
      text: initial.text ?? "",
      revision: initial.revision,
      editable: initial.editable,
      reason: initial.reason,
      diagnosticRecordId: initial.diagnosticRecordId,
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

  get outstandingIds(): string[] {
    return this.activeRequestId === null ? [] : [this.activeRequestId];
  }

  beginRecovery(transferId: string): void {
    this.recoveryId = transferId;
  }

  /** The Member typed: record the draft and send the difference when nothing is in flight. */
  input(value: string): void {
    this.draft = value;
    void this.flush();
  }

  /** A Backend event for this Document: a full text or a delta, local echo or remote. */
  receive(event: TextEvent, caret: { start: number; end: number } | null): { start: number; end: number } | null {
    if (event.documentId !== this.state.documentId) return caret;
    if (event.reason === "identity_conflict") {
      if (this.draft === this.state.text) this.draft = "";
      this.transfer = null;
      this.retry = null;
      this.activeRequestId = null;
      this.inFlightText = null;
      this.inFlightBase = null;
      this.state = { ...this.state, text: "", revision: event.revision, editable: false,
        reason: "identity_conflict", sending: false, error: null };
      this.announce();
      return caret;
    }
    if (this.state.reason === "identity_conflict") return caret;
    if (this.state.reason === "import_failure" && event.reason !== "import_failure") return caret;
    if (event.reason === "import_failure") {
      this.state = { ...this.state, reason: "import_failure", diagnosticRecordId: event.diagnosticRecordId,
        editable: false, sending: false, error: "This Document has incomplete edit history." };
      this.announce();
    }
    if (Number(event.revision) < Number(this.state.revision)) return caret;
    if (event.text === undefined && event.transfer === undefined && Number(event.revision) <= Number(this.state.revision)) return caret;
    if (event.transfer === "begin") {
      if (this.transfer !== null && this.transfer.revision !== event.revision) this.requestResync(this.transfer.revision);
      this.abandonedRevision = null;
      this.transfer = event.chunks !== undefined && event.chunks > 0
        ? { revision: event.revision, editable: event.editable, chunks: event.chunks, parts: [],
          transferId: event.transferId, requestStatuses: event.requestStatuses,
          reason: event.reason, diagnosticRecordId: event.diagnosticRecordId } : null;
      if (this.transfer === null) this.requestResync(event.revision);
      return caret;
    }
    if (event.transfer === "chunk" || event.transfer === "end") {
      const transfer = this.transfer;
      if (transfer === null) {
        if (Number(event.revision) > Number(this.state.revision)) this.requestResync(event.revision);
        return caret;
      }
      if (transfer.revision !== event.revision || transfer.transferId !== event.transferId) return caret;
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
        editable: transfer.editable, text: transfer.parts.join(""), savedRevision: event.savedRevision,
        transferId: transfer.transferId, requestStatuses: transfer.requestStatuses,
        reason: transfer.reason, diagnosticRecordId: transfer.diagnosticRecordId,
        requestId: event.requestId }, caret);
    }
    if (event.text === undefined && Number(event.revision) > Number(this.state.revision) + 1) {
      this.transfer = null;
      this.requestResync(event.revision);
      return caret;
    }
    if (event.text !== undefined) {
      this.transfer = null;
      this.abandonedRevision = null;
      const active = this.activeRequestId;
      const status = event.transferId === this.recoveryId
        ? event.requestStatuses?.find((entry) => entry.requestId === active) : undefined;
      if (active !== null && status === undefined && event.requestId !== active
        && event.reason !== "import_failure") {
        // A full broadcast from another editor has no provenance for our request.
        // Preserve the draft; only an identified recovery can decide inclusion.
        this.requestResync(event.revision);
        return caret;
      }
      if (active !== null && status?.inclusion === "unresolved" && event.reason !== "import_failure") {
        this.state = { ...this.state, error: "The edit could not be saved." };
        this.announce();
        return caret;
      }
      const base = this.state.text;
      const previousDraft = this.draft;
      const included = active !== null && (status?.inclusion === "included" || event.requestId === active);
      const pendingBase = included && this.inFlightText !== null ? this.inFlightText : base;
      const applied = diffOps(pendingBase, event.text);
      this.state = { ...this.state, text: event.text, revision: event.revision, editable: event.editable };
      this.draft = event.reason === "import_failure" && active !== null && this.inFlightBase !== null
        ? applyOps(event.text, rebase(diffOps(this.inFlightBase, previousDraft),
          diffOps(this.inFlightBase, event.text)))
        : previousDraft === pendingBase || previousDraft === event.text
          ? event.text : applyOps(event.text, rebase(diffOps(pendingBase, previousDraft), applied));
      if (event.savedRevision !== undefined) this.savedRevision = event.savedRevision;
      if (status?.inclusion === "absent") {
        this.retry = null;
        this.activeRequestId = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false, error: null };
      } else if (active !== null && (status?.inclusion === "included" && status.durability === "durable"
        || event.requestId === active && event.savedRevision !== undefined && event.editable)) {
        this.retry = null;
        this.activeRequestId = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false, error: null };
      }
      this.announce();
      if (this.activeRequestId === null) void this.flush();
      return caret === null ? null : { start: Math.min(caret.start, this.draft.length), end: Math.min(caret.end, this.draft.length) };
    }
    const delta = event.delta ?? [];
    const base = this.state.text;
    const nextBase = applyOps(base, delta);
    this.state = { ...this.state, text: nextBase, revision: event.revision, editable: event.editable };
    if (event.requestId !== undefined && event.requestId === this.activeRequestId) {
      const suffix = this.inFlightText === null ? [] : diffOps(this.inFlightText, this.draft);
      this.draft = applyOps(nextBase, rebase(suffix, diffOps(this.inFlightText ?? base, nextBase)));
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
    if (this.activeRequestId !== null && this.retry === null) return;
    const ops = this.retry?.ops ?? boundedOps(diffOps(this.state.text, this.draft), this.chunkLimit);
    if (ops.length === 0) return;
    const id = this.retry?.id ?? newRequestId();
    const base = this.retry?.base ?? this.state.revision;
    const baseText = this.state.text;
    const sent = this.retry?.text ?? applyOps(this.state.text, ops);
    this.state = { ...this.state, sending: true, error: null };
    this.activeRequestId = id;
    this.inFlightText = sent;
    this.inFlightBase = baseText;
    this.announce();
    try {
      const { revision } = await this.send(this.state.documentId, base, ops, id);
      if (this.state.reason === "identity_conflict" || this.state.reason === "import_failure") return;
      this.retry = null;
      this.activeRequestId = null;
      this.inFlightText = null;
      this.state = Number(this.state.revision) > Number(revision)
        ? { ...this.state, sending: false }
        : { ...this.state, text: sent, revision, sending: false };
    } catch (error) {
      if (this.state.reason === "identity_conflict" || this.state.reason === "import_failure") return;
      if (this.activeRequestId === null) {
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
        this.activeRequestId = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false };
        return;
      }
      if (code === "documents.edit_request_too_large" && this.chunkLimit > 256) {
        this.chunkLimit = Math.max(256, Math.floor(this.chunkLimit / 2));
        this.retry = null;
        this.activeRequestId = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false };
        void this.flush();
        return;
      }
      if (code === "documents.document_too_large" || code === "documents.edit_request_too_large") {
        this.retry = null;
        this.activeRequestId = null;
        this.inFlightText = null;
        this.state = { ...this.state, sending: false, error: describe(code) };
        this.announce();
        return;
      }
      this.retry = { id, base, ops, text: sent };
      this.state = { ...this.state, sending: false, error: describe(code) };
      this.announce();
      return;
    }
    this.announce();
    void this.flush();
  }
}

function newRequestId(): string {
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** One request owns at most 8 KiB of inserted UTF-8, leaving room for one Loro update. */
function boundedOps(ops: readonly TextOp[], limit: number): TextOp[] {
  const out: TextOp[] = [];
  let bytes = 0;
  for (const op of ops) {
    if (!("insert" in op)) {
      out.push(op);
      continue;
    }
    let chunk = "";
    for (const scalar of op.insert) {
      const size = new TextEncoder().encode(scalar).length;
      if (bytes + size > limit) break;
      chunk += scalar;
      bytes += size;
    }
    if (chunk) out.push({ insert: chunk });
    if (chunk.length !== op.insert.length || bytes >= limit) break;
  }
  return out;
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

/** Maps original characters, not intervals; an inserted remote character is never deleted. */
function rebase(pending: readonly TextOp[], applied: readonly TextOp[]): TextOp[] {
  const length = pending.reduce((total, op) => total + ("retain" in op ? op.retain : "delete" in op ? op.delete : 0), 0);
  const positions: (number | null)[] = Array(length + 1).fill(null);
  let original = 0;
  let current = 0;
  for (const op of applied) {
    if ("insert" in op) {
      current += Array.from(op.insert).length;
    } else if ("delete" in op) {
      original += op.delete;
    } else {
      for (let index = 0; index < op.retain && original <= length; index++) positions[original++] = current++;
    }
  }
  while (original <= length) positions[original++] = current++;
  const out: TextOp[] = [];
  let oldPosition = 0;
  let newPosition = 0;
  let removed = 0;
  let inserted = 0;
  for (const op of pending) {
    if ("insert" in op) {
      out.push(op);
      inserted += Array.from(op.insert).length;
      newPosition += Array.from(op.insert).length;
      continue;
    }
    const count = "retain" in op ? op.retain : op.delete;
    for (let index = 0; index < count; index++) {
      const mapped = positions[oldPosition++];
      if (mapped === null || mapped === undefined) continue;
      const target = mapped - removed + inserted;
      if (target < newPosition) continue;
      if ("retain" in op) {
        out.push({ retain: target - newPosition + 1 });
        newPosition = target + 1;
      } else {
        if (target > newPosition) out.push({ retain: target - newPosition });
        out.push({ delete: 1 });
        newPosition = target;
        removed++;
      }
    }
    if ("retain" in op && count > 0 && oldPosition === length && positions[length] !== null) {
      // Keep an insertion at the last retained boundary before a subsequent local insert.
      const boundary = (positions[length] ?? newPosition) - removed + inserted;
      if (boundary > newPosition) { out.push({ retain: boundary - newPosition }); newPosition = boundary; }
    }
  }
  return out;
}
