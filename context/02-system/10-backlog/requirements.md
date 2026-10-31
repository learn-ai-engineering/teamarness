# 02-system/10-backlog — Backlog Requirements

Prefix: `TEAM.SYS.BACKLOG`

The Backlog Package (`dev.teamarness.backlog`) provides Workspace Epics and Tickets. It owns its records, codec, validation, projection, UI, and Port Contract. The Core Runtime, Package Host and Shell contain no Backlog-specific behavior. See ADR 0027 and ADR 0039.

## TEAM.SYS.BACKLOG-R01 — Epics

A Person SHALL be able to create an Epic with a title, rename it, archive it, and restore it. Only a Person, through the Backlog UI, SHALL change Epics. An Epic SHALL be identified by the Record ID of its create record and SHALL never be deleted.

## TEAM.SYS.BACKLOG-R02 — Tickets

A Ticket SHALL belong to exactly one Epic. A Person SHALL be able to create a Ticket in an Epic, move it to another Epic, archive it, and restore it. A Ticket SHALL be identified by the Record ID of its create record and SHALL never be deleted. Archived Epics and Tickets SHALL be hidden unless "show archived" is on.

## TEAM.SYS.BACKLOG-R03 — Replication and Convergence

The Backlog SHALL replicate every Epic and Ticket record through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace, with a Package-owned canonical codec (`TBLOG\x01`). Every record SHALL carry its author and an author-clock time; a record whose author is not its signer SHALL be quarantined and never applied. Each Ticket field SHALL converge on its own: a record changes a field only when its Order Key sorts after the order that last set that field, so every Installation shows the same state whatever the arrival order. A `ticket.update` SHALL carry only the fields its saver changed. A record whose Epic or Ticket has not arrived SHALL wait and SHALL be applied when the target arrives.

## TEAM.SYS.BACKLOG-R04 — Ticket Version and Stale Saves

A Ticket's version SHALL be the greatest Order Key of any record applied to it. Every Ticket-changing request, from a Person or an Agent, SHALL carry the version it was based on. The Backend SHALL refuse a request whose base differs from the version this Installation has applied, with `backlog.stale_version` and the current Ticket. Saves made concurrently on Installations that have not seen each other's records SHALL NOT be refused; they converge under R03 (ADR 0039).

## TEAM.SYS.BACKLOG-R05 — Ticket Detail

The Ticket detail SHALL show the title, the body rendered as sanitized Markdown (with tables and read-only task lists), the status, priority and assignee, the creator with the creation time, and the last editor with the time of the last change. Edit SHALL swap in a plain text area holding the raw Markdown; Save SHALL store only the changed fields, and Cancel SHALL discard the edit. A title SHALL hold 1 to 256 characters without control characters. A body SHALL hold at most 65,536 bytes of UTF-8 and MAY hold tabs and line breaks but no other control characters. Status SHALL be one of `not_ready`, `ready`, `in_progress`, `done`; priority SHALL be one of `high`, `medium`, `low`. A new Ticket SHALL default to `not_ready` and `medium`.

## TEAM.SYS.BACKLOG-R06 — Assignee

A Ticket's assignee SHALL be a current Workspace Person, identified by Principal ID, or none. Agents SHALL never be offered or accepted as assignees. When the stored assignee is no longer a current Principal, the UI SHALL show "Former member" and the first 12 characters of the Principal ID, and the Ticket SHALL remain reassignable.

## TEAM.SYS.BACKLOG-R07 — Remembered UI State

Unsaved edits, the selected Ticket, edit mode, filters, "show archived" and expanded Epics SHALL survive a Package switch on the same Installation. The Backend SHALL hold them in memory only; they SHALL NOT be persisted or replicated and MAY be lost on app restart.

## TEAM.SYS.BACKLOG-R08 — Navigation

The left pane SHALL list Epics alphabetically (case-insensitive, ties by creation order), each collapsible and showing its Ticket titles. Tickets SHALL be ordered by priority, then status, then creation. A priority filter (All or one) and a status filter (All or one) SHALL combine; every Epic SHALL stay visible while only matching Tickets show, and each filter SHALL clear in one action. Every dropdown SHALL be the Package's custom listbox, never a native `<select>`.

## TEAM.SYS.BACKLOG-R09 — Backlog Port Contract

The Backlog SHALL provide Port Contract `dev.teamarness.backlog` 1.0 with `runtime.guide`, `ticket.list`, `ticket.get`, `ticket.create`, `ticket.update`, and the payload-free `backlog.changed` event. Port commands from a bound Workspace Agent SHALL be signed with the Agent's key. The contract SHALL offer no Epic, assign, move, archive or restore operation.

## TEAM.SYS.BACKLOG-R10 — No Plaintext at Rest

Ticket and Epic titles, Ticket bodies and unsaved drafts SHALL NOT be stored in Package SQLite or any other Package file. Titles and bodies SHALL live in Backend memory, rebuilt from sealed records on activation (ADR 0027).
