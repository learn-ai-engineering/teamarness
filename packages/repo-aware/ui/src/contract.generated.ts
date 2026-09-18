// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Empty = Record<string, never>;

export type Registration = {
  readonly "addedAt": string;
  readonly "id": string;
  readonly "path": string;
};

export type Registrations = readonly Registration[];

export type Snapshot = {
  readonly "commit": string;
  readonly "fetchedAt": string;
  readonly "githubUrl"?: string | null;
  readonly "head": {
    readonly "kind": "branch";
    readonly "name": string;
  } | {
    readonly "commit": string;
    readonly "kind": "detached";
  };
  readonly "isClean": boolean;
  readonly "originRemote"?: string | null;
  readonly "path": string;
  readonly "registrationId": string;
};

export type RepoAwareContract = {
  readonly requests: {
    readonly "repo_aware.list": { readonly request: Empty; readonly response: {
        readonly "registrations": Registrations;
      } };
    readonly "repo_aware.register": { readonly request: {
        readonly "path": string;
      }; readonly response: {
        readonly "registration": Registration;
      } };
    readonly "repo_aware.remove": { readonly request: {
        readonly "registrationId": string;
      }; readonly response: {
        readonly "removed": true;
      } };
    readonly "repo_aware.snapshot": { readonly request: {
        readonly "registrationId": string;
      }; readonly response: {
        readonly "registrationId": string;
        readonly "snapshot": Snapshot;
      } };
  };
  readonly subscriptions: Record<string, never>;
};
