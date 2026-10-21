import { describePackageError, isAbortError } from "@teamarness/package-ui-sdk";
import type { TypedPackageClient } from "@teamarness/package-ui-sdk";
import type { Collection, Document, DocumentsContract, Snapshot } from "./contract.generated";

export type ControllerState = Readonly<{
  snapshot: Snapshot;
  selectedCollectionId: string | null;
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
    showArchived: false,
    busy: false,
    error: null,
  };
  private disposed = false;
  /** A Collection this Installation just created, selected once its record is listed. */
  private pendingSelectionId: string | null = null;

  constructor(client: TypedPackageClient<DocumentsContract>, announce: () => void) {
    this.client = client;
    this.announce = announce;
  }

  get state(): ControllerState {
    return this._state;
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
    const visible = snapshot.collections.filter((c) => this._state.showArchived || !c.archived);
    const pending = this.pendingSelectionId;
    if (pending !== null && snapshot.collections.some((c) => c.id === pending)) {
      this.pendingSelectionId = null;
      this.update({ snapshot, selectedCollectionId: pending });
      return;
    }
    const current = this._state.selectedCollectionId;
    const stillThere = current !== null && snapshot.collections.some((c) => c.id === current);
    const selectedCollectionId = stillThere ? current : (visible[0]?.id ?? null);
    this.update({ snapshot, selectedCollectionId });
  }

  selectCollection(id: string): void {
    if (this._state.snapshot.collections.some((c) => c.id === id)) {
      this.update({ selectedCollectionId: id, error: null });
    }
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
    case "documents.deferred":
      return "Saved locally; it will apply once its Collection arrives.";
    default:
      return describePackageError(error);
  }
}
