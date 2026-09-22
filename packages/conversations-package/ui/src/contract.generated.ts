// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Acknowledgement = {
  readonly "ok": true;
};

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type Before = string;

export type Channel = {
  readonly "canHide": boolean;
  readonly "id": ChannelId;
  readonly "slug": Slug;
  readonly "unreadCount": number;
};

export type ChannelId = string;

export type Message = {
  readonly "author": string;
  readonly "id": string;
  readonly "isLocal": boolean;
  readonly "markdown": string;
};

export type Page = {
  readonly "messages": readonly Message[];
  readonly "nextBefore"?: Before;
};

export type Slug = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "channels": readonly Channel[];
  readonly "warnings"?: readonly string[];
};

export type ConversationsContract = {
  readonly requests: {
    readonly "conversations.channel.create": { readonly request: {
        readonly "slug": Slug;
      }; readonly response: Acknowledgement };
    readonly "conversations.channel.hide": { readonly request: {
        readonly "channelId": ChannelId;
        readonly "confirm": true;
      }; readonly response: Acknowledgement };
    readonly "conversations.channel.open": { readonly request: {
        readonly "channelId": ChannelId;
        readonly "limit"?: number;
      }; readonly response: Page };
    readonly "conversations.channel.page": { readonly request: {
        readonly "before": Before;
        readonly "channelId": ChannelId;
        readonly "limit"?: number;
      }; readonly response: Page };
    readonly "conversations.message.post": { readonly request: {
        readonly "channelId": ChannelId;
        readonly "markdown": string;
      }; readonly response: Acknowledgement };
    readonly "conversations.snapshot": { readonly request: Record<string, never>; readonly response: Snapshot };
  };
  readonly subscriptions: {
    readonly "conversations.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
  };
};
