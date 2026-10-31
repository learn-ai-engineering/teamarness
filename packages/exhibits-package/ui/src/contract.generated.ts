// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Change = {
  readonly "deleted": boolean;
  readonly "exhibitId": ExhibitId;
  readonly "revision": number;
};

export type ExhibitId = string;

export type Opened = {
  readonly "revision": Revision;
  readonly "url": string;
};

export type Page = {
  readonly "exhibits": readonly Summary[];
  readonly "nextBefore"?: string;
};

export type Revision = number;

export type Summary = {
  readonly "id": ExhibitId;
  readonly "kind": "draft";
  readonly "revision": Revision;
  readonly "title": string;
  readonly "updatedAt": string;
};

export type ExhibitsContract = {
  readonly requests: {
    readonly "exhibit.list": { readonly request: {
        readonly "before"?: string;
        readonly "limit"?: number;
      }; readonly response: Page };
    readonly "exhibit.open": { readonly request: {
        readonly "exhibitId": ExhibitId;
      }; readonly response: Opened };
  };
  readonly subscriptions: {
    readonly "exhibits.changed": { readonly request: Record<string, never>; readonly event: Change };
  };
};
