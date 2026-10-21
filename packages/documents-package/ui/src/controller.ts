import { describePackageError, isAbortError } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { Collection, Comment, Document, DocumentsContract, Names, Snapshot, TextEvent } from "./contract.generated";
import { DocumentEditor } from "./editor";
import { authorLabel } from "./markdown";

export type ControllerState = Readonly<{
  snapshot: Snapshot;
  selectedCollectionId: string | null;
  openDocumentId: string | null;
  comments: readonly Comment[];
  commentsCursor: string | null;
  commentsLoading: boolean;
  names: Names;
  /** Document body shown as rendered Markdown instead of the editor. */
  preview: boolean;
  showArchived: boolean;
  busy: boolean;
  error: string | null;
}>;

const EMPTY: Snapshot = { collections: [], documents: [], availability: "catching_up", warnings: [] };

/**
 * Holds the listing the Backend publishes and the UI's own selection, and turns user actions
 * into Package requests. Rendering is the view's job; the controller only announces changes.
 */
export class DocumentsController {
  private readonly client: TypedPackageClient<DocumentsContract>;
  private readonly announce: () => void;
  private _state: ControllerState = {
    snapshot: EMPTY,
    selectedCollectionId: null,
    openDocumentId: null,
    comments: [],
    commentsCursor: null,
    commentsLoading: false,
    names: { available: false, cursor: "0", persons: [] },
    preview: false,
    showArchived: false,
    busy: false,
    error: null,
  };
  private disposed = false;
  /** A Collection this Installation just created, selected once its record is listed. */
  private pendingSelectionId: string | null = null;
  /** The editor for the open Document and its text subscription. */
  private editor: DocumentEditor | null = null;
  private textSubscription: { close(): Promise<void> } | null = null;
  private openSequence = 0;
  private commentsSequence = 0;
  private pendingNames: { cursor: string; parts: number; nextPart: number; persons: Names["persons"] } | null = null;

  constructor(client: TypedPackageClient<DocumentsContract>, announce: () => void) {
    this.client = client;
    this.announce = announce;
  }

  get state(): ControllerState {
    return this._state;
  }

  get openEditor(): DocumentEditor | null {
    return this.editor;
  }

  get openDocument(): Document | null {
    const id = this._state.openDocumentId;
    return this._state.snapshot.documents.find((d) => d.id === id) ?? null;
  }

  /** Collections in listing order, hiding archived ones unless asked for. */
  get visibleCollections(): readonly Collection[] {
    return this._state.snapshot.collections.filter((c) => this._state.showArchived || !c.archived);
  }

  get selectedCollection(): Collection | null {
    const id = this._state.selectedCollectionId;
    return this._state.snapshot.collections.find((c) => c.id === id) ?? null;
  }

  /** Documents of the selected Collection in listing order, hiding archived ones unless asked for. */
  get visibleDocuments(): readonly Document[] {
    const id = this._state.selectedCollectionId;
    return this._state.snapshot.documents.filter(
      (d) => d.collectionId === id && (this._state.showArchived || !d.archived),
    );
  }

  async initialize(): Promise<void> {
    this.applySnapshot(await this.client.request("documents.snapshot", {}));
  }

  applySnapshot(snapshot: Snapshot): void {
    if (this.disposed) return;
    // Any committed record may be a Comment on the open Document; refresh its thread.
    void this.refreshComments();
    if (this._state.openDocumentId !== null && this.editor !== null
      && !this.editor.current.editable && !this.editor.assembling) {
      void this.retryOpen(this._state.openDocumentId, this.openSequence);
    }
    const visible = snapshot.collections.filter((c) => this._state.showArchived || !c.archived);
    const pending = this.pendingSelectionId;
    if (pending !== null && visible.some((c) => c.id === pending)) {
      this.pendingSelectionId = null;
      this.update({ snapshot, selectedCollectionId: pending });
      return;
    }
    const current = this._state.selectedCollectionId;
    const stillThere = current !== null && visible.some((c) => c.id === current);
    const selectedCollectionId = stillThere ? current : (visible[0]?.id ?? null);
    this.update({ snapshot, selectedCollectionId });
  }

  selectCollection(id: string): void {
    if (this.visibleCollections.some((c) => c.id === id)) {
      this.update({ selectedCollectionId: id, error: null });
    }
  }

  /** Opens a Document for editing: fetches its text, then follows its text events. */
  async openDocumentById(documentId: string): Promise<void> {
    if (this.disposed || this._state.openDocumentId === documentId) return;
    await this.closeDocument();
    const sequence = ++this.openSequence;
    this.update({ openDocumentId: documentId, comments: [], commentsCursor: null, commentsLoading: false, error: null });
    void this.refreshComments();
    try {
      const subscription = await this.client.subscribe("documents.document.text", { documentId }, (event) => {
        if (this.disposed || sequence !== this.openSequence) return;
        this.onTextEvent(event);
      });
      if (this.disposed || sequence !== this.openSequence) {
        await subscription.close();
        return;
      }
      this.textSubscription = subscription;
      await this.retryOpen(documentId, sequence);
    } catch (error) {
      if (sequence !== this.openSequence || this.disposed) return;
      this.update({ openDocumentId: null, error: describeError(error) });
    }
  }

  private async retryOpen(documentId: string, sequence: number): Promise<void> {
    try {
      await this.client.request("documents.document.open", { documentId });
    } catch (error) {
      if (!this.disposed && sequence === this.openSequence && !isAbortError(error)) {
        this.update({ error: describeError(error) });
      }
    }
  }

  async closeDocument(): Promise<void> {
    this.openSequence += 1;
    this.commentsSequence += 1;
    const subscription = this.textSubscription;
    this.textSubscription = null;
    this.editor = null;
    if (this._state.openDocumentId !== null) this.update({ openDocumentId: null, comments: [], commentsCursor: null, commentsLoading: false });
    if (subscription) await subscription.close();
  }

  async refreshComments(): Promise<void> {
    const documentId = this._state.openDocumentId;
    if (this.disposed || documentId === null) return;
    const sequence = ++this.commentsSequence;
    try {
      const listed = await this.client.request("documents.comments", { documentId });
      if (this.disposed || this._state.openDocumentId !== documentId || sequence !== this.commentsSequence) return;
      // Re-read the loaded range: an older Comment may have changed state remotely.
      const oldest = this._state.comments.at(-1)?.id;
      const fullyLoaded = this._state.comments.length > 0 && this._state.commentsCursor === null;
      const comments = [...listed.comments];
      let cursor = listed.nextCursor ?? null;
      while (cursor !== null && (fullyLoaded || (oldest !== undefined && !comments.some((comment) => comment.id === oldest)))) {
        const page = await this.client.request("documents.comments", { documentId, cursor });
        if (this.disposed || this._state.openDocumentId !== documentId || sequence !== this.commentsSequence) return;
        comments.push(...page.comments);
        cursor = page.nextCursor ?? null;
      }
      this.update({ comments, commentsCursor: cursor, commentsLoading: false });
    } catch (error) {
      if (isAbortError(error) || this.disposed || sequence !== this.commentsSequence) return;
      this.update({ error: describeError(error), commentsLoading: false });
    }
  }

  async loadOlderComments(): Promise<void> {
    const { openDocumentId: documentId, commentsCursor: cursor, commentsLoading } = this._state;
    if (this.disposed || documentId === null || cursor === null || commentsLoading) return;
    const sequence = this.commentsSequence;
    this.update({ commentsLoading: true });
    try {
      const listed = await this.client.request("documents.comments", { documentId, cursor });
      if (this.disposed || this._state.openDocumentId !== documentId || sequence !== this.commentsSequence) return;
      const ids = new Set(this._state.comments.map((comment) => comment.id));
      this.update({ comments: [...this._state.comments, ...listed.comments.filter((comment) => !ids.has(comment.id))],
        commentsCursor: listed.nextCursor ?? null, commentsLoading: false });
    } catch (error) {
      if (isAbortError(error) || this.disposed || sequence !== this.commentsSequence) return;
      this.update({ commentsLoading: false, error: describeError(error) });
    }
  }

  async addComment(markdown: string, quote: string | null, parentId: string | null): Promise<boolean> {
    const documentId = this._state.openDocumentId;
    if (documentId === null) return false;
    const payload: { documentId: string; markdown: string; quote?: string; parentId?: string } = { documentId, markdown: markdown.trim() };
    if (quote) payload.quote = quote;
    if (parentId) payload.parentId = parentId;
    const added = (await this.perform(() => this.client.request("documents.comment.add", payload))) !== null;
    if (added) await this.refreshComments();
    return added;
  }

  async resolveComment(commentId: string): Promise<boolean> {
    const ok = (await this.perform(() => this.client.request("documents.comment.resolve", { commentId }))) !== null;
    if (ok) await this.refreshComments();
    return ok;
  }

  async reopenComment(commentId: string): Promise<boolean> {
    const ok = (await this.perform(() => this.client.request("documents.comment.reopen", { commentId }))) !== null;
    if (ok) await this.refreshComments();
    return ok;
  }

  /** Called by the view with the caret so a remote change can move it. */
  receiveText(event: TextEvent, caret: { start: number; end: number } | null): { start: number; end: number } | null {
    if (event.documentId !== this._state.openDocumentId) return caret;
    if (this.editor === null || this.editor.current.documentId !== event.documentId) {
      if (event.text === undefined && event.transfer !== "begin") {
        void this.retryOpen(event.documentId, this.openSequence);
        return caret;
      }
      this.editor = new DocumentEditor(
        event.transfer === "begin" ? { documentId: event.documentId, revision: "0", editable: false, text: "" } : event,
        async (documentId, baseRevision, ops) => {
          const { revision } = await this.client.request("documents.document.edit", { documentId, baseRevision, ops });
          return { revision };
        },
        this.announce,
        () => { void this.retryOpen(event.documentId, this.openSequence); },
      );
      this.announce();
      return event.transfer === "begin" ? this.editor.receive(event, caret) : caret;
    }
    return this.editor.receive(event, caret);
  }

  private onTextEvent(event: TextEvent): void {
    this.textListener?.(event);
  }

  /** The view installs this to supply the caret when an event arrives. */
  textListener: ((event: TextEvent) => void) | null = null;

  togglePreview(): void {
    this.update({ preview: !this._state.preview });
  }

  /** The current Workspace Person Name for an author identity, with duplicate detection. */
  labelFor(author: string, isLocal: boolean): string {
    const person = this._state.names.persons.find((p) => p.identity === author);
    const name = person?.name ?? null;
    const duplicate = name !== null && this._state.names.persons.filter((p) => p.name === name).length > 1;
    return authorLabel(author, undefined, isLocal, name, duplicate);
  }

  /** Applies a roster event, reassembling multi-part rosters by cursor and part order. */
  applyNames(names: Names): void {
    if (this.disposed) return;
    if (!names.available) {
      this.pendingNames = null;
      this.update({ names: { available: false, cursor: "0", persons: [] } });
      return;
    }
    if (this._state.names.available && BigInt(names.cursor) < BigInt(this._state.names.cursor)) return;
    if (names.part !== undefined && names.parts !== undefined) {
      if (names.part === 0) this.pendingNames = { cursor: names.cursor, parts: names.parts, nextPart: 0, persons: [] };
      const pending = this.pendingNames;
      if (!pending || pending.cursor !== names.cursor || pending.parts !== names.parts || names.part !== pending.nextPart) return;
      this.pendingNames = { ...pending, nextPart: pending.nextPart + 1, persons: [...pending.persons, ...names.persons] };
      if (names.part !== names.parts - 1) return;
      names = { available: true, cursor: names.cursor, persons: this.pendingNames.persons };
      this.pendingNames = null;
    } else {
      this.pendingNames = null;
    }
    this.update({ names });
  }

  toggleArchived(): void {
    this.update({ showArchived: !this._state.showArchived });
    this.applySnapshot(this._state.snapshot);
  }

  async createCollection(name: string): Promise<boolean> {
    const created = await this.perform(() => this.client.request("documents.collection.create", { name: name.trim() }));
    if (created) this.pendingSelectionId = created.id;
    return created !== null;
  }

  async renameCollection(collectionId: string, name: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.collection.rename", { collectionId, name: name.trim() }))) !== null;
  }

  async archiveCollection(collectionId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.collection.archive", { collectionId }))) !== null;
  }

  async restoreCollection(collectionId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.collection.restore", { collectionId }))) !== null;
  }

  async createDocument(title: string): Promise<boolean> {
    const collectionId = this._state.selectedCollectionId;
    if (collectionId === null) return false;
    return (await this.perform(() => this.client.request("documents.document.create", { collectionId, title: title.trim() }))) !== null;
  }

  async renameDocument(documentId: string, title: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.document.rename", { documentId, title: title.trim() }))) !== null;
  }

  async moveDocument(documentId: string, collectionId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.document.move", { documentId, collectionId }))) !== null;
  }

  async archiveDocument(documentId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.document.archive", { documentId }))) !== null;
  }

  async restoreDocument(documentId: string): Promise<boolean> {
    return (await this.perform(() => this.client.request("documents.document.restore", { documentId }))) !== null;
  }

  dispose(): void {
    this.disposed = true;
    void this.closeDocument();
  }

  private async perform<T>(action: () => Promise<T>): Promise<T | null> {
    if (this.disposed || this._state.busy) return null;
    this.update({ busy: true, error: null });
    try {
      const result = await action();
      this.update({ busy: false });
      return result;
    } catch (error) {
      if (isAbortError(error) || this.disposed) return null;
      this.update({ busy: false, error: describeError(error) });
      return null;
    }
  }

  private update(changes: Partial<ControllerState>): void {
    if (this.disposed) return;
    this._state = { ...this._state, ...changes };
    this.announce();
  }
}

function describeError(error: unknown): string {
  const code = (error as { code?: unknown } | null)?.code;
  switch (code) {
    case "documents.invalid_name":
      return "Collection names are 1 to 128 characters with no leading or trailing spaces.";
    case "documents.invalid_title":
      return "Document titles are 1 to 256 characters with no leading or trailing spaces.";
    case "documents.unknown_collection":
      return "That Collection is no longer available.";
    case "documents.unknown_document":
      return "That Document is no longer available.";
    case "documents.unknown_comment":
      return "That Comment is no longer available.";
    case "documents.invalid_markdown":
      return "Comments are 1 byte to 32 KiB of Markdown.";
    case "documents.invalid_quote":
      return "A quote is at most 2 KiB.";
    case "documents.deferred":
      return "Saved locally; it will apply once its Collection arrives.";
    default:
      return describePackageError(error);
  }
}
