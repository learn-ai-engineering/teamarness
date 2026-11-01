// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Appended = {
  readonly "id": RecordId;
  readonly "ok": true;
};

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type BodyReason = "identity_conflict" | "import_failure";

export type BodyState = {
  readonly "diagnosticRecordId"?: RecordId;
  readonly "documentId": RecordId;
  readonly "editable": boolean;
  readonly "reason"?: BodyReason;
};

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
  readonly "createdAt"?: string;
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
  readonly "hasMore"?: boolean;
  readonly "nextCursor"?: string | null;
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

export type Place = {
  readonly "collectionId"?: RecordId;
  readonly "documentId"?: RecordId;
  readonly "scrollTop"?: number;
};

export type RecordId = string;

export type RequestId = string;

export type RequestStatus = {
  readonly "durability": "durable" | "pending" | "unresolved";
  readonly "inclusion": "included" | "absent" | "unresolved";
  readonly "requestId": RequestId;
};

export type Revision = string;

export type SessionEvent = {
  readonly "chunks": number;
  readonly "diagnosticRecordId"?: RecordId;
  readonly "documentId": RecordId;
  readonly "editable": boolean;
  readonly "event": "begin";
  readonly "reason"?: BodyReason;
  readonly "sessionPeer": SessionPeer;
  readonly "version": Revision;
} | {
  readonly "chunk": string;
  readonly "chunkIndex": number;
  readonly "documentId": RecordId;
  readonly "event": "chunk";
} | {
  readonly "documentId": RecordId;
  readonly "event": "end";
} | {
  readonly "documentId": RecordId;
  readonly "event": "update";
  readonly "update": string;
} | {
  readonly "diagnosticRecordId"?: RecordId;
  readonly "documentId": RecordId;
  readonly "durable": readonly RequestId[];
  readonly "editable": boolean;
  readonly "event": "status";
  readonly "reason"?: BodyReason;
} | {
  readonly "documentId": RecordId;
  readonly "event": "resync";
};

export type SessionPeer = string;

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
    readonly "documents.comment.add": { readonly request: {
        readonly "documentId": RecordId;
        readonly "markdown": string;
        readonly "parentId"?: RecordId;
        readonly "quote"?: string;
      }; readonly response: Appended };
    readonly "documents.comment.reopen": { readonly request: CommentRequest; readonly response: Appended };
    readonly "documents.comment.resolve": { readonly request: CommentRequest; readonly response: Appended };
    readonly "documents.comments": { readonly request: {
        readonly "cursor"?: string;
        readonly "documentId": RecordId;
      }; readonly response: Comments };
    readonly "documents.document.archive": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.document.create": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.markdown": { readonly request: {
        readonly "chunkIndex": number;
        readonly "documentId": RecordId;
      }; readonly response: {
        readonly "chunk": string;
        readonly "chunkIndex": number;
        readonly "chunks": number;
        readonly "digest": string;
        readonly "documentId": RecordId;
      } };
    readonly "documents.document.move": { readonly request: {
        readonly "collectionId": RecordId;
        readonly "documentId": RecordId;
      }; readonly response: Appended };
    readonly "documents.document.open": { readonly request: DocumentRequest; readonly response: BodyState };
    readonly "documents.document.recover": { readonly request: {
        readonly "documentId": RecordId;
        readonly "requestIds": readonly RequestId[];
      }; readonly response: {
        readonly "documentId": RecordId;
        readonly "requestStatuses": readonly RequestStatus[];
      } };
    readonly "documents.document.rename": { readonly request: {
        readonly "documentId": RecordId;
        readonly "title": Title;
      }; readonly response: Appended };
    readonly "documents.document.restore": { readonly request: DocumentRequest; readonly response: Appended };
    readonly "documents.document.update": { readonly request: {
        readonly "documentId": RecordId;
        readonly "requestId": RequestId;
        readonly "sessionPeer": SessionPeer;
        readonly "update": string;
      }; readonly response: {
        readonly "requestId": RequestId;
      } };
    readonly "documents.place.get": { readonly request: Record<string, never>; readonly response: Place };
    readonly "documents.place.set": { readonly request: Place; readonly response: Record<string, never> };
    readonly "documents.snapshot": { readonly request: Record<string, never>; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "documents.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
    readonly "documents.document.session": { readonly request: {
        readonly "documentId": RecordId;
        readonly "resumePeer"?: SessionPeer;
      }; readonly event: SessionEvent };
    readonly "documents.names": { readonly request: Record<string, never>; readonly event: Names };
  };
};
