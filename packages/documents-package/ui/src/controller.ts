import { describePackageError, isAbortError, packageErrorCode } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { Collection, Comment, Document, DocumentsContract, Names, Snapshot } from "./contract.generated";
import { EditorSession, clientBridge } from "./editor/session";
import type { LostNotice, SessionState } from "./editor/session";
import { authorLabel } from "./markdown";

export type ControllerState = Readonly<{
  snapshot: Snapshot;
  selectedCollectionId: string | null;
  openDocumentId: string | null;
  comments: readonly Comment[];
  commentsCursor: string | null;
  commentsLoading: boolean;
  names: Names;
  showArchived: boolean;
  busy: boolean;
  error: string | null;
}>;

const EMPTY: Snapshot = { collections: [], documents: [], availability: "catching_up", warnings: [] };

type Place = DocumentsContract["requests"]["documents.place.get"]["response"];

/**
 * How long the remembered place waits for more changes before it is saved. The Frame has no
 * unmount hook to save on, so each change is saved soon after it settles.
 */
export const PLACE_SAVE_DELAY_MS = 150;
const MAX_SCROLL_TOP = 1_000_000_000;

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
    showArchived: false,
    busy: false,
    error: null,
  };
  private disposed = false;
  /** A Collection this Installation just created, selected once its record is listed. */
  private pendingSelectionId: string | null = null;
  /** The editor session of the open Document. */
  private session: EditorSession | null = null;
  private openSequence = 0;
  private commentsSequence = 0;
  private pendingNames: { cursor: string; parts: number; nextPart: number; persons: Names["persons"] } | null = null;
  /** The open Document's scroll offset, as last reported or restored. */
  private scrollTop: number | undefined;
  /** A remembered scroll offset still to apply once its Document's body is shown. */
  private restoredScroll: { documentId: string; scrollTop: number } | null = null;
  private placeTimer: ReturnType<typeof setTimeout> | undefined;
  private lastSavedPlace: string | null = null;

  constructor(client: TypedPackageClient<DocumentsContract>, announce: () => void) {
    this.client = client;
    this.announce = announce;
  }

  get state(): ControllerState {
    return this._state;
  }

  /** The open Document's editor session state, once its session has started. */
  get openSession(): SessionState | null {
    return this.session?.state ?? null;
  }

  /** The open Document's editor session: its Loro document, export cadence and size. */
  get editorSession(): EditorSession | null {
    return this.session;
  }

  /** The open Document as Canonical Markdown, read from the Backend in chunks. */
  async documentMarkdown(): Promise<string> {
    const documentId = this._state.openDocumentId;
    if (documentId === null) throw new Error("No Document is open.");
    try {
      return await readChunkedMarkdown((chunkIndex) =>
        this.client.request("documents.document.markdown", { documentId, chunkIndex }));
    } catch (error) {
      throw new Error(describeError(error), { cause: error });
    }
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
    const snapshot = await this.client.request("documents.snapshot", {});
    const place = await this.readPlace();
    const restored = this.restorable(snapshot, place);
    if (restored.collectionId !== null) {
      // Chosen before the first listing is shown, so the first selection is the remembered one.
      this._state = { ...this._state, selectedCollectionId: restored.collectionId };
    }
    this.lastSavedPlace = JSON.stringify(placeOf(restored.collectionId, restored.documentId, restored.scrollTop));
    this.applySnapshot(snapshot);
    if (restored.documentId !== null) {
      if (restored.scrollTop !== undefined) {
        this.restoredScroll = { documentId: restored.documentId, scrollTop: restored.scrollTop };
      }
      void this.openDocumentById(restored.documentId);
    }
  }

  /** The remembered place, or none when the Backend has none or cannot say. */
  private async readPlace(): Promise<Place> {
    try {
      return await this.client.request("documents.place.get", {});
    } catch {
      return {};
    }
  }

  /** What of a remembered place still exists and is shown in `snapshot`. */
  private restorable(snapshot: Snapshot, place: Place): {
    collectionId: string | null; documentId: string | null; scrollTop: number | undefined;
  } {
    const visible = (id: string | undefined) => id !== undefined
      && snapshot.collections.some((c) => c.id === id && (this._state.showArchived || !c.archived));
    const document_ = snapshot.documents.find((d) => d.id === place.documentId
      && (this._state.showArchived || !d.archived) && visible(d.collectionId));
    if (document_ !== undefined) {
      return { collectionId: document_.collectionId, documentId: document_.id, scrollTop: place.scrollTop };
    }
    return { collectionId: visible(place.collectionId) ? place.collectionId ?? null : null, documentId: null, scrollTop: undefined };
  }

  /**
   * The remembered scroll offset of `documentId`, once: the view applies it when that Document's
   * body is first shown.
   */
  takeRestoredScroll(documentId: string): number | undefined {
    const restored = this.restoredScroll;
    if (restored === null || restored.documentId !== documentId) return undefined;
    this.restoredScroll = null;
    return restored.scrollTop;
  }

  /** Records the open Document's scroll offset, saved with the place soon after it settles. */
  rememberScroll(scrollTop: number): void {
    if (this.disposed || this._state.openDocumentId === null) return;
    const top = Math.min(MAX_SCROLL_TOP, Math.max(0, Math.round(scrollTop)));
    if (top === this.scrollTop) return;
    this.scrollTop = top;
    this.schedulePlaceSave();
  }

  private schedulePlaceSave(): void {
    if (this.placeTimer !== undefined) clearTimeout(this.placeTimer);
    this.placeTimer = setTimeout(() => this.savePlace(), PLACE_SAVE_DELAY_MS);
  }

  /** Saves the current place to the Backend's memory, unless it is already saved. */
  private savePlace(): void {
    if (this.placeTimer !== undefined) clearTimeout(this.placeTimer);
    this.placeTimer = undefined;
    const place = placeOf(this._state.selectedCollectionId, this._state.openDocumentId, this.scrollTop);
    const key = JSON.stringify(place);
    if (key === this.lastSavedPlace) return;
    this.lastSavedPlace = key;
    // Remembering is a convenience: a refused save changes nothing the Member can see.
    this.client.request("documents.place.set", place).catch(() => {
      if (this.lastSavedPlace === key) this.lastSavedPlace = null;
    });
  }

  applySnapshot(snapshot: Snapshot): void {
    if (this.disposed) return;
    // Any committed record may be a Comment on the open Document; refresh its thread.
    void this.refreshComments();
    // A body still waiting for history may open now; its session reloads once it can.
    const session = this.session?.state;
    if (this._state.openDocumentId !== null && session !== undefined && session.phase === "ready"
      && !session.editable && session.reason === undefined) {
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

  /** Opens a Document: starts its editor session, which loads the body from a snapshot. */
  async openDocumentById(documentId: string): Promise<void> {
    if (this.disposed || this._state.openDocumentId === documentId) return;
    // Cleared before the close, so the notice of changes the closing session lost stays shown.
    this.update({ error: null });
    await this.closeDocument();
    const sequence = ++this.openSequence;
    this.update({ openDocumentId: documentId, comments: [], commentsCursor: null, commentsLoading: false });
    void this.refreshComments();
    const session = new EditorSession({
      bridge: clientBridge(this.client),
      documentId,
      onChange: () => { if (!this.disposed && sequence === this.openSequence) this.announce(); },
      // A session closed with unacknowledged changes reports them too, after its Document closed.
      onLost: (notice) => {
        if (!this.disposed) this.update({ error: sequence === this.openSequence ? lostMessage(notice) : CLOSED_LOST_MESSAGE });
      },
    });
    this.session = session;
    await session.start();
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
    const session = this.session;
    this.session = null;
    if (this._state.openDocumentId !== null) this.update({ openDocumentId: null, comments: [], commentsCursor: null, commentsLoading: false });
    await session?.dispose();
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
    if (this.placeTimer !== undefined) this.savePlace();
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
    const before = this._state;
    this._state = { ...this._state, ...changes };
    if (before.openDocumentId !== this._state.openDocumentId) {
      const opened = this._state.openDocumentId;
      if (this.restoredScroll !== null && this.restoredScroll.documentId !== opened) this.restoredScroll = null;
      this.scrollTop = this.restoredScroll?.documentId === opened ? this.restoredScroll?.scrollTop : undefined;
    }
    if (before.openDocumentId !== this._state.openDocumentId
      || before.selectedCollectionId !== this._state.selectedCollectionId) this.schedulePlaceSave();
    this.announce();
  }
}

type MarkdownChunk = DocumentsContract["requests"]["documents.document.markdown"]["response"];

/** How many times a Markdown read starts over when the Document changes while it is read. */
export const MARKDOWN_READ_ATTEMPTS = 3;

/**
 * Reads a chunked Markdown export. Every chunk must carry the first chunk's digest and count;
 * when one does not, the Document changed meanwhile and the read starts over from chunk 0. When
 * the frame can hash, the reassembled text is checked against the digest too.
 */
export async function readChunkedMarkdown(read: (chunkIndex: number) => Promise<MarkdownChunk>): Promise<string> {
  for (let attempt = 0; attempt < MARKDOWN_READ_ATTEMPTS; attempt += 1) {
    const first = await read(0);
    const parts = [first.chunk];
    let consistent = first.chunkIndex === 0;
    for (let index = 1; consistent && index < first.chunks; index += 1) {
      const next = await read(index);
      consistent = next.digest === first.digest && next.chunks === first.chunks && next.chunkIndex === index;
      parts.push(next.chunk);
    }
    if (!consistent) continue;
    const text = parts.join("");
    if (await digestMatches(text, first.digest)) return text;
  }
  throw new Error("The Document kept changing while it was read; try again.");
}

async function digestMatches(text: string, digest: string): Promise<boolean> {
  const subtle = globalThis.crypto?.subtle;
  if (subtle === undefined) return true;
  const hash = new Uint8Array(await subtle.digest("SHA-256", new TextEncoder().encode(text)));
  return [...hash].map((byte) => byte.toString(16).padStart(2, "0")).join("") === digest;
}

function placeOf(collectionId: string | null, documentId: string | null, scrollTop: number | undefined): Place {
  return {
    ...(collectionId !== null ? { collectionId } : {}),
    ...(documentId !== null ? { documentId } : {}),
    ...(documentId !== null && scrollTop !== undefined ? { scrollTop } : {}),
  };
}

const CLOSED_LOST_MESSAGE = "Your last changes to the Document you closed could not be confirmed as saved.";

function lostMessage(notice: LostNotice): string {
  return notice.requests > 0
    ? "The Document was reloaded; your most recent changes could not be saved."
    : "The Document was reloaded; changes not yet sent were discarded.";
}

function describeError(error: unknown): string {
  const code = packageErrorCode(error) ?? (error as { code?: unknown } | null)?.code;
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
    case "documents.document_pending":
      return "This Document is still waiting for its history.";
    case "documents.unavailable":
      return "This Document is unavailable.";
    default:
      return describePackageError(error);
  }
}
