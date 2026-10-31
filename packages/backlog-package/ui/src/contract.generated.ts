// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Availability = "waiting_for_history_access" | "offline" | "catching_up" | "current";

export type Body = string;

export type Draft = {
  readonly "assignee"?: PrincipalId | null;
  readonly "baseVersion": Version | "";
  readonly "body"?: Body;
  readonly "priority"?: Priority;
  readonly "status"?: Status;
  readonly "title"?: string;
};

export type DraftKey = string;

export type Empty = Record<string, never>;

export type Epic = {
  readonly "archived": boolean;
  readonly "createdOrder": Version;
  readonly "epicId": RecordId;
  readonly "title": string;
};

export type EpicRequest = {
  readonly "epicId": RecordId;
};

export type Identity = string;

export type Names = {
  readonly "available": boolean;
  readonly "cursor": Revision;
  readonly "part"?: number;
  readonly "parts"?: number;
  readonly "principals": readonly Principal[];
};

export type Principal = {
  readonly "fingerprint": string;
  readonly "kind": "person" | "agent";
  readonly "name": string | null;
  readonly "principalId": PrincipalId;
};

export type PrincipalId = string;

export type Priority = "high" | "medium" | "low";

export type RecordId = string;

export type RememberedState = {
  readonly "drafts": {
    readonly [key: string]: Draft;
  };
  readonly "editing"?: boolean;
  readonly "expandedEpicIds": readonly RecordId[];
  readonly "filters": {
    readonly "priority": Priority | null;
    readonly "status": Status | null;
  };
  readonly "selectedTicketId"?: RecordId;
  readonly "showArchived": boolean;
};

export type Revision = string;

export type Snapshot = {
  readonly "availability": Availability;
  readonly "epics": readonly Epic[];
  readonly "tickets": readonly TicketSummary[];
  readonly "warnings": readonly string[];
};

export type Status = "not_ready" | "ready" | "in_progress" | "done";

export type Ticket = {
  readonly "archived": boolean;
  readonly "assignee": PrincipalId | null;
  readonly "body": Body;
  readonly "createdAt": Time;
  readonly "creator": Identity;
  readonly "epicId": RecordId;
  readonly "lastAt": Time;
  readonly "lastEditor": Identity;
  readonly "priority": Priority;
  readonly "status": Status;
  readonly "ticketId": RecordId;
  readonly "title": string;
  readonly "version": Version;
};

export type TicketBaseRequest = {
  readonly "baseVersion": Version;
  readonly "ticketId": RecordId;
};

export type TicketRequest = {
  readonly "ticketId": RecordId;
};

export type TicketSummary = {
  readonly "archived": boolean;
  readonly "assignee": PrincipalId | null;
  readonly "createdOrder": Version;
  readonly "epicId": RecordId;
  readonly "priority": Priority;
  readonly "status": Status;
  readonly "ticketId": RecordId;
  readonly "title": string;
  readonly "version": Version;
};

export type Time = number;

export type Title = string;

export type Version = string;

export type VersionResult = {
  readonly "version": Version;
};

export type BacklogContract = {
  readonly requests: {
    readonly "backlog.draft.clear": { readonly request: {
        readonly "key": DraftKey;
      }; readonly response: Empty };
    readonly "backlog.draft.get": { readonly request: Empty; readonly response: {
        readonly "state": RememberedState | null;
      } };
    readonly "backlog.draft.put": { readonly request: {
        readonly "state": RememberedState;
      }; readonly response: Empty };
    readonly "backlog.epic.archive": { readonly request: EpicRequest; readonly response: Empty };
    readonly "backlog.epic.create": { readonly request: {
        readonly "title": Title;
      }; readonly response: {
        readonly "epicId": RecordId;
      } };
    readonly "backlog.epic.rename": { readonly request: {
        readonly "epicId": RecordId;
        readonly "title": Title;
      }; readonly response: Empty };
    readonly "backlog.epic.restore": { readonly request: EpicRequest; readonly response: Empty };
    readonly "backlog.ticket.archive": { readonly request: TicketBaseRequest; readonly response: VersionResult };
    readonly "backlog.ticket.assign": { readonly request: {
        readonly "assignee": PrincipalId | null;
        readonly "baseVersion": Version;
        readonly "ticketId": RecordId;
      }; readonly response: VersionResult };
    readonly "backlog.ticket.create": { readonly request: {
        readonly "body"?: Body;
        readonly "epicId": RecordId;
        readonly "priority"?: Priority;
        readonly "status"?: Status;
        readonly "title": Title;
      }; readonly response: {
        readonly "ticketId": RecordId;
        readonly "version": Version;
      } };
    readonly "backlog.ticket.get": { readonly request: TicketRequest; readonly response: Ticket };
    readonly "backlog.ticket.move": { readonly request: {
        readonly "baseVersion": Version;
        readonly "epicId": RecordId;
        readonly "ticketId": RecordId;
      }; readonly response: VersionResult };
    readonly "backlog.ticket.restore": { readonly request: TicketBaseRequest; readonly response: VersionResult };
    readonly "backlog.ticket.update": { readonly request: {
        readonly "baseVersion": Version;
        readonly "body"?: Body;
        readonly "priority"?: Priority;
        readonly "status"?: Status;
        readonly "ticketId": RecordId;
        readonly "title"?: Title;
      }; readonly response: VersionResult };
  };
  readonly subscriptions: {
    readonly "backlog.changes": { readonly request: Record<string, never>; readonly event: Snapshot };
    readonly "backlog.names": { readonly request: Record<string, never>; readonly event: Names };
  };
};
