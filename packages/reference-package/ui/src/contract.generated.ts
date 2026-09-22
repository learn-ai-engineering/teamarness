// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Empty = Record<string, never>;

export type Snapshot = {
  readonly "backgroundMarker": number;
  readonly "counter": number;
  readonly "membershipVersion": string;
  readonly "packageVersion": string;
  readonly "processGeneration": string;
  readonly "signingVerified": boolean;
};

export type ReferenceContract = {
  readonly requests: {
    readonly "reference.fail": { readonly request: Empty; readonly response: Empty };
    readonly "reference.increment": { readonly request: Empty; readonly response: Snapshot };
    readonly "reference.read": { readonly request: Empty; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "reference.changes": { readonly request: Empty; readonly event: Snapshot };
  };
};
