# 02-system/10-backlog — Backlog Requirements

Prefix: `TEAM.SYS.BACKLOG`

The Backlog Package (`dev.teamarness.backlog`) provides Workspace Epics and Tickets. It owns its records, codec, validation, projection, UI, and Port Contract. The Core Runtime, Package Host and Shell contain no Backlog-specific behavior. See ADR 0027, ADR 0039 and ADR 0048.

## TEAM.SYS.BACKLOG-R01 — Epics

A Person SHALL be able to create an Epic with a title, rename it, archive it, and restore it. Only a Person, through the Backlog UI, SHALL change Epics. An Epic SHALL be identified by the Record ID of its create record and SHALL never be deleted.

## TEAM.SYS.BACKLOG-R02 — Tickets

A Ticket SHALL belong to exactly one Epic. A Person SHALL be able to create a Ticket in an Epic, move it to another Epic, archive it, and restore it. A Ticket SHALL be identified by the Record ID of its create record and SHALL never be deleted. Archived Epics and Tickets SHALL be hidden unless "show archived" is on.

## TEAM.SYS.BACKLOG-R03 — Replication and Convergence

The Backlog SHALL replicate every Epic and Ticket record through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace, with a Package-owned canonical codec (`TBLOG\x01`). Every record SHALL carry its author and an author-clock time; a record whose author is not its signer SHALL be quarantined and never applied. Each Ticket field SHALL converge on its own: a record changes a field only when its Order Key sorts after the order that last set that field, so every Installation shows the same state whatever the arrival order. A `ticket.update` SHALL carry only the fields its saver changed. A record whose Epic or Ticket has not arrived SHALL wait and SHALL be applied when the target arrives.

## TEAM.SYS.BACKLOG-R04 — Ticket Version and Stale Saves

A Ticket's version SHALL be the greatest Order Key of any record applied to it. Every Ticket-changing request, from a Person or an Agent, SHALL carry the version it was based on. The Backend SHALL refuse a request whose base differs from the version this Installation has applied, with `backlog.stale_version` and the current Ticket. Saves made concurrently on Installations that have not seen each other's records SHALL NOT be refused; they converge under R03 (ADR 0039).

## TEAM.SYS.BACKLOG-R05 — Ticket Detail

The Ticket detail SHALL show the title, the body rendered as sanitized Markdown (with tables and read-only task lists), the status, priority and assignee, the creator with the creation time, and the last editor with the time of the last change. Edit SHALL swap in a plain text area holding the raw Markdown; Save SHALL store only the changed fields, and Cancel SHALL discard the edit. A title SHALL hold 1 to 256 characters without control characters. A body SHALL hold at most 65,536 bytes of UTF-8 and MAY hold tabs and line breaks but no other control characters. Status SHALL be one of `not_ready`, `ready`, `in_progress`, `done`, `cancelled`, where `cancelled` means closed and will not be done (ADR 0048); priority SHALL be one of `high`, `medium`, `low`. A new Ticket SHALL default to `not_ready` and `medium`.

## TEAM.SYS.BACKLOG-R06 — Assignee

A Ticket's assignee SHALL be a current Workspace Person, identified by Principal ID, or none. Agents SHALL never be offered or accepted as assignees. When the stored assignee is no longer a current Principal, the UI SHALL show "Former member" and the first 12 characters of the Principal ID, and the Ticket SHALL remain reassignable.

## TEAM.SYS.BACKLOG-R07 — Remembered UI State

Unsaved edits, the selected Ticket, edit mode, filters, "show archived", expanded Epics and the left pane's width SHALL survive a Package switch on the same Installation. The Backend SHALL hold them in memory only; they SHALL NOT be persisted or replicated and MAY be lost on app restart.

## TEAM.SYS.BACKLOG-R08 — Navigation

The left pane SHALL list Epics alphabetically (case-insensitive, ties by creation order), each collapsible and showing its Ticket titles. Tickets SHALL be ordered by priority, then status, then creation. A priority filter (All or one) and a status filter (Not done, All or one) SHALL combine; **Not done** SHALL match `not_ready`, `ready` and `in_progress`. With no remembered filters, the status filter SHALL be Not done and the priority filter All. Every Epic SHALL stay visible while only matching Tickets show, and each filter SHALL clear to All in one action. A single control SHALL expand every listed Epic, or collapse them all when every one is expanded. The left pane's right edge SHALL widen it by drag or arrow keys, never below its default width, and the width SHALL be remembered under R07. Selecting a Ticket SHALL NOT rebuild the left pane or move its scroll position. Every dropdown SHALL be the Package's custom listbox, never a native `<select>`.

## TEAM.SYS.BACKLOG-R09 — Backlog Port Contract

The Backlog SHALL provide Port Contract `dev.teamarness.backlog` 1.1 with `runtime.guide`, `ticket.list`, `ticket.get`, `ticket.create`, `ticket.update`, and the payload-free `backlog.changed` event. 1.1 is an additive minor over 1.0: Ticket summaries and Tickets carry `ref` (R11), `ticket.get` takes `ticketId` or `ref`, and status gains `cancelled` (ADR 0048). Port commands from a bound Workspace Agent SHALL be signed with the Agent's key. The contract SHALL offer no Epic, assign, move, archive or restore operation.

## TEAM.SYS.BACKLOG-R10 — No Plaintext at Rest

Ticket and Epic titles, Ticket bodies and unsaved drafts SHALL NOT be stored in Package SQLite or any other Package file. Titles and bodies SHALL live in Backend memory, rebuilt from sealed records on activation (ADR 0027).

## TEAM.SYS.BACKLOG-R11 — Ticket References

A Ticket's reference SHALL be the shortest prefix of its Ticket ID, at least four characters long, that no other Ticket known to this Installation shares, archived Tickets included. It SHALL be derived from the projection and never stored or carried in a record. The UI SHALL show it with a leading `#` in the Ticket's row and in the Ticket detail, where it can be copied. `ticket.get` SHALL accept a reference of 4 to 64 hexadecimal characters, with an optional `#` and in any case: one match returns the Ticket, none is `backlog.not_found`, and several are `backlog.ambiguous_ref` listing at most 16 candidates with a flag when more exist. Writes SHALL take the full Ticket ID (ADR 0048).

## TEAM.SYS.BACKLOG-R12 — Re-evaluation After a Codec Upgrade

The store SHALL record the codec generation that last re-evaluated its records quarantined as `malformed_record`. On activation, a store below the current generation SHALL open each such record and project those the current codec decodes, as if just delivered; a record that still fails SHALL stay quarantined. The generation SHALL be recorded only once every such record could be opened (ADR 0048).
