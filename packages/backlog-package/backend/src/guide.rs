//! Runtime advice for the Backlog Ports (1.0). Statuses, priorities, defaults and limits come
//! from the constants that enforce them.

use std::fmt::Write as _;

use serde_json::{Value as JsonValue, json};

use super::ports::{MAX_PAGE, PORT_PAYLOAD_BYTES};
use crate::domain::{DEFAULT_PRIORITY, DEFAULT_STATUS, MAX_BODY, MAX_TITLE, Priority, Status};

/// What each status means, in listing order.
pub(super) fn status_meaning(status: Status) -> &'static str {
    match status {
        Status::NotReady => "not yet specified enough to start",
        Status::Ready => "specified, can be started",
        Status::InProgress => "someone is working on it",
        Status::Done => "finished",
    }
}

pub(super) fn response() -> JsonValue {
    let mut statuses = String::new();
    for status in Status::ALL {
        let _ = writeln!(
            statuses,
            "- `{}`: {}.",
            status.as_str(),
            status_meaning(status)
        );
    }
    let priorities = Priority::ALL
        .iter()
        .map(|priority| format!("`{}`", priority.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let markdown = format!(
        r#"# Backlog Port guide

## What this is

The Backlog holds Epics, each a titled group of Tickets. A Ticket is a unit of work with a title, a Markdown body, a status, a priority and an optional assignee. Every Ticket belongs to exactly one Epic. Epic and Ticket IDs are 64 lowercase hexadecimal characters: use an ID that ticket.list returned, never a guessed title.

## Statuses

{statuses}
## Priorities

The priorities, from most to least urgent, are {priorities}. New Tickets default to `{default_status}` / `{default_priority}`.

## What you can do

- ticket.list returns Epics and Ticket summaries without bodies: Epics first, then Tickets, each in creation order, at most {MAX_PAGE} of each per page. A page can be shorter to fit the {payload_kib} KiB Port value bound. While nextCursor is present, pass it as cursor, with the same filters, for the next page. Filter with epicId, status or priority. Archived Epics, archived Tickets and the Tickets of archived Epics are omitted unless includeArchived is true. An unknown epicId is refused with backlog.not_found, and a cursor ticket.list did not return with backlog.invalid_cursor.
- ticket.get takes ticketId and returns the whole Ticket: title, body, status, priority, assignee (a Principal ID or null), archived, version, creator, createdAt, lastEditor and lastAt. creator and lastEditor are signing keys; times are Unix milliseconds.
- ticket.create takes epicId, title and optional body, status and priority, and returns ticketId and version. The Epic must exist (backlog.not_found) and must not be archived (backlog.epic_archived).
- ticket.update changes title, body, status or priority.
- backlog.changed is an empty, live-only notice that may coalesce, with no initial event or replay. List or read again on receipt and after reconnecting.

You cannot create, rename, archive or restore Epics, and you cannot assign, move, archive or restore Tickets. Ask a Person to do that in the Backlog.

Your records are signed as you: the Host-derived actor is the Agent the calling Package is bound to, or the local Person. Never supply an actor, author or Principal. A Person from another Installation is refused with backlog.actor_unauthorized.

## Saving

Always call ticket.get first and pass its `version` as `baseVersion`; send only the fields you change, at least one. If someone changed the Ticket after you read it, ticket.update is refused with backlog.stale_version and nothing is written. Its details hold the current Ticket as `ticket`; with `bodyOmitted: true` the body did not fit, so read it with ticket.get. On `backlog.stale_version`, re-read the returned Ticket, re-apply your change to it and retry once; tell the Person if it fails again. The returned `version` is the base for your next update.

A refusal arrives as provider_error with details.provider.code and details.provider.details; inspect outcomeUncertain as well. backlog.unavailable means the Backlog cannot write or read now; retry later. ticket.list, ticket.get and runtime.guide are read-only and repeatable. Never automatically retry an uncertain ticket.create: every accepted create makes a new Ticket, so list and look for it first. After an uncertain ticket.update, read the Ticket and decide from what it holds.

## Limits

A title has 1 to {MAX_TITLE} characters, with no control characters and no leading or trailing whitespace. A body is at most {MAX_BODY} bytes of UTF-8 Markdown and may be empty; tables and `- [ ]` task lists render. A field over its limit is refused with backlog.too_large and its limit; any other invalid field with backlog.invalid and the field name.
"#,
        default_status = DEFAULT_STATUS.as_str(),
        default_priority = DEFAULT_PRIORITY.as_str(),
        payload_kib = PORT_PAYLOAD_BYTES / 1024,
    );
    json!({ "markdown": markdown })
}
