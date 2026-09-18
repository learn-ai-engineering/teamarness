// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type PackageContract = {
  readonly requests: {
    readonly "__REQUEST_NAMESPACE__.read": { readonly request: Record<string, never>; readonly response: {
        readonly "displayName": string;
        readonly "message": string;
        readonly "readCount": number;
      } };
  };
  readonly subscriptions: Record<string, never>;
};
