// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Acknowledgement = {
  readonly "ok": true;
};

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type Before = string;

export type Change = {
  readonly "id": string;
  readonly "kind": "insert" | "remove";
  readonly "message"?: Message;
  readonly "position": Position;
  readonly "revision": Revision;
  readonly "status"?: "ready" | "pending" | "unavailable";
};

export type Changes = {
  readonly "continuation": Revision;
  readonly "events": readonly Change[];
  readonly "highWater": Revision;
  readonly "resyncRequired"?: boolean;
};

export type Channel = {
  readonly "canHide": boolean;
  readonly "id": ChannelId;
  readonly "slug": Slug;
  readonly "unreadCount": number;
};

export type ChannelId = string;

export type Epoch = string;

export type Message = {
  readonly "author": string;
  readonly "id": string;
  readonly "isLocal": boolean;
  readonly "markdown": string;
  readonly "position": Position;
};

export type Page = {
  readonly "epoch": Epoch;
  readonly "messages": readonly Message[];
  readonly "nextBefore"?: Before;
  readonly "pending"?: readonly PendingRow[];
  readonly "revision": Revision;
};

export type PendingRow = {
  readonly "id": string;
  readonly "position": Position;
  readonly "status": "pending" | "unavailable";
};

export type Position = string;

export type Resolved = {
  readonly "messages": readonly Message[];
  readonly "missing": readonly string[];
  readonly "pending": readonly PendingRow[];
};

export type Revision = string;

export type Slug = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "channels": readonly Channel[];
  readonly "warnings"?: readonly string[];
};

export type ConversationsContract = {
  readonly requests: {
    readonly "conversations.channel.changes": { readonly request: {
        readonly "after": Revision;
        readonly "channelId": ChannelId;
        readonly "epoch": Epoch;
        readonly "limit"?: number;
      }; readonly response: Changes };
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
    readonly "conversations.channel.resolve": { readonly request: {
        readonly "channelId": ChannelId;
        readonly "ids": readonly string[];
      }; readonly response: Resolved };
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
