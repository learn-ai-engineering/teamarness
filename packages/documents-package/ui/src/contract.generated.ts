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

export type Comment = {
  readonly "author": string;
  readonly "documentId": RecordId;
  readonly "id": RecordId;
  readonly "isLocal": boolean;
  readonly "markdown"?: string;
  readonly "parentId"?: RecordId;
  readonly "position": string;
  readonly "quote"?: string;
  readonly "resolved": boolean;
};

export type CommentRequest = {
  readonly "commentId": RecordId;
};

export type Comments = {
  readonly "comments": readonly Comment[];
  readonly "truncated"?: boolean;
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

export type Edited = {
  readonly "ok": true;
  readonly "revision": Revision;
};

export type Name = string;

export type Names = {
  readonly "available": boolean;
  readonly "cursor": Revision;
  readonly "part"?: number;
  readonly "parts"?: number;
  readonly "persons": readonly PersonName[];
};

export type PersonName = {
  readonly "fingerprint": string;
  readonly "identity": string;
  readonly "name": string | null;
  readonly "personId": string;
};

export type RecordId = string;

export type Revision = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "collections": readonly Collection[];
  readonly "documents": readonly Document[];
  readonly "warnings"?: readonly string[];
};

export type TextEvent = {
  readonly "chunk"?: string;
  readonly "chunkIndex"?: number;
  readonly "chunks"?: number;
  readonly "delta"?: readonly TextOp[];
  readonly "documentId": RecordId;
  readonly "editable": boolean;
  readonly "origin"?: "local" | "remote";
  readonly "revision": Revision;
  readonly "text"?: string;
  readonly "transfer"?: "begin" | "chunk" | "end";
};

export type TextOp = {
  readonly "retain": number;
} | {
  readonly "insert": string;
} | {
  readonly "delete": number;
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
    readonly "documents.comment.add": { readonly request: {
        readonly "documentId": RecordId;
        readonly "markdown": string;
        readonly "parentId"?: RecordId;
        readonly "quote"?: string;
      }; readonly response: Appended };
    readonly "documents.comment.reopen": { readonly request: CommentRequest; readonly response: Appended };
    readonly "documents.comment.resolve": { readonly request: CommentRequest; readonly response: Appended };
    readonly "documents.comments": { readonly request: DocumentRequest; readonly response: Comments };
    readonly "documents.document.archive": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.document.create": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.edit": { readonly request: {
        readonly "baseRevision": Revision;
        readonly "documentId": RecordId;
        readonly "ops": readonly TextOp[];
      }; readonly response: Edited };
    readonly "documents.document.move": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "documentId": RecordId;
      }; readonly response: Appended };
    readonly "documents.document.open": { readonly request: DocumentRequest; readonly response: TextEvent };
    readonly "documents.document.rename": { readonly request: {
        readonly "documentId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.restore": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.snapshot": { readonly request: Record<string, never>; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "documents.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
    readonly "documents.document.text": { readonly request: DocumentRequest; readonly event: TextEvent };
    readonly "documents.names": { readonly request: Record<string, never>; readonly event: Names };
  };
};
