// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type Collection = {
  readonly "archived": boolean;
  readonly "id": RecordId;
  readonly "name": string;
};

export type Document = {
  readonly "archived": boolean;
  readonly "collectionId": RecordId;
  readonly "id": RecordId;
  readonly "title": string;
};

export type RecordId = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "collections": readonly Collection[];
  readonly "documents": readonly Document[];
  readonly "warnings"?: readonly string[];
};

export type DocumentsContract = {
  readonly requests: {
    readonly "documents.snapshot": { readonly request: Record<string, never>; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "documents.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
  };
};
