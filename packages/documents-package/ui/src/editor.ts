import type { TextEvent } from "./contract.generated";

export type EditorState = Readonly<{
  documentId: string;
  /** The Canonical Markdown the Backend last published for the body. */
  text: string;
  revision: string;
  /** Every locally known record of the body has been opened. */
  editable: boolean;
  reason?: "identity_conflict" | "import_failure";
  diagnosticRecordId?: string;
}>;

/**
 * Follows one open Document's body as the Backend publishes it: full Canonical Markdown, or a
 * chunked transfer of it. The body is read-only here; the rendered editor replaces this seam.
 */
export class DocumentEditor {
  private state: EditorState;
  private transfer: { revision: string; editable: boolean; chunks: number; parts: string[]; reason?: TextEvent["reason"]; diagnosticRecordId?: string } | null = null;
  private abandonedRevision: string | null = null;
  private readonly resync: () => void;
  private readonly announce: () => void;

  constructor(initial: TextEvent, announce: () => void, resync: () => void = () => {}) {
    this.state = {
      documentId: initial.documentId,
      text: initial.text ?? "",
      revision: initial.revision,
      editable: initial.editable,
      reason: initial.reason,
      diagnosticRecordId: initial.diagnosticRecordId,
    };
    this.announce = announce;
    this.resync = resync;
  }

  get current(): EditorState {
    return this.state;
  }

  get value(): string {
    return this.state.text;
  }

  get assembling(): boolean {
    return this.transfer !== null;
  }

  /** A Backend event for this Document: a full text, or one part of a chunked transfer. */
  receive(event: TextEvent): void {
    if (event.documentId !== this.state.documentId) return;
    if (event.reason === "identity_conflict") {
      this.transfer = null;
      this.state = { ...this.state, text: "", revision: event.revision, editable: false, reason: "identity_conflict" };
      this.announce();
      return;
    }
    if (this.state.reason === "identity_conflict") return;
    if (this.state.reason === "import_failure" && event.reason !== "import_failure") return;
    if (Number(event.revision) < Number(this.state.revision)) return;
    if (event.transfer === "begin") {
      if (this.transfer !== null && this.transfer.revision !== event.revision) this.requestResync(this.transfer.revision);
      this.abandonedRevision = null;
      this.transfer = event.chunks !== undefined && event.chunks > 0
        ? { revision: event.revision, editable: event.editable, chunks: event.chunks, parts: [],
          reason: event.reason, diagnosticRecordId: event.diagnosticRecordId } : null;
      if (this.transfer === null) this.requestResync(event.revision);
      return;
    }
    if (event.transfer === "chunk" || event.transfer === "end") {
      const transfer = this.transfer;
      if (transfer === null) {
        if (Number(event.revision) > Number(this.state.revision)) this.requestResync(event.revision);
        return;
      }
      if (transfer.revision !== event.revision) return;
      if (event.transfer === "chunk") {
        if (event.chunkIndex !== transfer.parts.length || event.chunk === undefined || transfer.parts.length >= transfer.chunks) {
          this.transfer = null;
          this.requestResync(event.revision);
        } else transfer.parts.push(event.chunk);
        return;
      }
      this.transfer = null;
      if (transfer.parts.length !== transfer.chunks) { this.requestResync(event.revision); return; }
      this.receive({ documentId: event.documentId, revision: event.revision, editable: transfer.editable,
        text: transfer.parts.join(""), reason: transfer.reason, diagnosticRecordId: transfer.diagnosticRecordId });
      return;
    }
    if (event.text === undefined) return;
    this.transfer = null;
    this.abandonedRevision = null;
    this.state = {
      ...this.state,
      text: event.text,
      revision: event.revision,
      editable: event.editable,
      ...(event.reason === "import_failure" ? { reason: "import_failure", diagnosticRecordId: event.diagnosticRecordId } : {}),
    };
    this.announce();
  }

  private requestResync(revision: string): void {
    if (this.abandonedRevision === revision) return;
    this.abandonedRevision = revision;
    this.resync();
  }
}
