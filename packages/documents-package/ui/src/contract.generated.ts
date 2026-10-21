// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Appended = {
  readonly "id": RecordId;
  readonly "ok": true;
};

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type Collection = {
  readonly "archived": boolean;
  readonly "id": RecordId;
  readonly "name": string;
};

export type CollectionRequest = {
  readonly "collectionId": RecordId;
};

export type Document = {
  readonly "archived": boolean;
  readonly "collectionId": RecordId;
  readonly "id": RecordId;
  readonly "title": string;
};

export type DocumentRequest = {
  readonly "documentId": RecordId;
};

export type Name = string;

export type RecordId = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "collections": readonly Collection[];
  readonly "documents": readonly Document[];
  readonly "warnings"?: readonly string[];
};

export type Title = string;

export type DocumentsContract = {
  readonly requests: {
    readonly "documents.collection.archive": { readonly request: CollectionRequest; readonly response: Appended };
    readonly "documents.collection.create": { readonly request: {
        readonly "name": Name;
      }; readonly response: Appended };
    readonly "documents.collection.rename": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "name": Name;
      }; readonly response: Appended };
    readonly "documents.collection.restore": { readonly request: CollectionRequest; readonly response: Appended };
    readonly "documents.document.archive": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.document.create": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.move": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "documentId": RecordId;
      }; readonly response: Appended };
    readonly "documents.document.rename": { readonly request: {
        readonly "documentId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.restore": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.snapshot": { readonly request: Record<string, never>; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "documents.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
  };
};
