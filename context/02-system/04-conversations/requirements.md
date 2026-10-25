# 02-system/04-conversations — Conversations Requirements

Prefix: `TEAM.SYS.CONV`

The Conversations Package (`dev.teamarness.conversations`) provides Workspace Channels and Messages.

## TEAM.SYS.CONV-R01 — Package Boundary

The Conversations Package SHALL own Channels, Messages, their canonical record encoding and validation, local projections, local read state, and its UI. It SHALL use only the standard Package Manifest, Package Process Protocol, lifecycle, Package Data Directory, UI contract, and declared Semantic Capabilities. The Core Runtime, Package Host, and Shell SHALL contain no Conversations-specific behavior.

## TEAM.SYS.CONV-R02 — Record Replication

The Conversations Package SHALL replicate all Channel, hide, and Message records through one `teamarness.synchronization.immutable-record-log` 1.x scope per Package and Workspace. The Core Runtime SHALL treat Conversations canonical bytes as opaque and SHALL NOT parse Conversations records or decide Channel policy.

## TEAM.SYS.CONV-R03 — Membership Eligibility

Only current Canonical Members SHALL be able to append or receive Conversations records. Every current Workspace Member SHALL be able to read and post in every active Channel, and the Package SHALL keep no separate Channel Membership records. The Package SHALL consume Canonical Membership as an eligibility ceiling and SHALL NOT mutate it.

## TEAM.SYS.CONV-R04 — General Channel

Every Workspace SHALL have a permanent `#general` Channel that exists without any record, accepts Messages, and cannot be hidden. No created Channel SHALL use the `#general` identity.

## TEAM.SYS.CONV-R05 — Channel Creation and Names

Any current Member SHALL be able to create a Channel with a slug of 1 to 48 lowercase ASCII letters and digits, optionally separated by single hyphens. Channels that share a slug after convergence SHALL all be retained. Every Installation SHALL present the same unique slugs: the earliest-created Channel keeps the base slug, and the others receive a deterministic suffix derived from their Channel identity.

## TEAM.SYS.CONV-R06 — Permanent Channel Hide

The creator of a non-general Channel SHALL be able to hide it after explicit confirmation. Hiding SHALL be permanent with no restore path. After convergence the Channel SHALL be hidden for every Member, Messages ordered after the earliest accepted hide SHALL be excluded, and new posts to it SHALL be rejected. A hide authored by anyone other than the creator SHALL be rejected. Hidden records SHALL be retained, not deleted.

## TEAM.SYS.CONV-R07 — Messages

A current Member SHALL be able to post an immutable Message of 1 byte to 32 KiB of UTF-8 Markdown to an active Channel. Version 1 SHALL NOT support Message edit, delete, reply, reaction, attachment, or embed.

## TEAM.SYS.CONV-R08 — Convergent Order and History Paging

After convergence every Installation SHALL show each Channel's Messages in the same order, derived from Core-provided order keys rather than wall-clock time. The newest Message SHALL appear at the bottom of the Channel, with older Messages above it. Opening a Channel SHALL show its newest page of at most 32 Messages, scrolled to the bottom. Older pages SHALL load automatically as the Member approaches the top of the Message history, using a raw-row cursor that advances even when projected Messages are temporarily unreadable. The UI SHALL preserve the visible reading position across older loads, interior projection changes, and reflow. A projected Message that arrives inside the loaded range SHALL appear in order without requiring a Channel reopen; an older insert below that range SHALL remain discoverable through paging. A hide SHALL remove its affected projected Messages. A non-ready row SHALL remain identified as pending or unavailable rather than becoming fake Message content or a false beginning-of-history result. A Member SHALL be able to retry pending history explicitly when readability changes without a notification.

The Package UI history contract SHALL expose the projection epoch and captured revision with a Channel open or page. A bounded `channel.changes` request SHALL replay insert and removal revisions after a caller's acknowledged revision, and a bounded `channel.resolve` request SHALL reopen specified pending identities. A stale epoch SHALL require reconstruction of the loaded raw range. Failed head, older-page, change-feed, and pending-resolution operations SHALL keep independent explicit retry states; unrelated notifications SHALL NOT automatically retry a failed operation.

## TEAM.SYS.CONV-R09 — Out-of-Order and Invalid Records

A record that refers to a Channel not yet known SHALL be deferred and projected once that Channel arrives, including after restart. A record that is malformed, whose purpose does not match its content, or whose declared author or creator differs from the Core-authenticated author SHALL be quarantined, excluded from projections, and reported through a non-sensitive warning. It SHALL NOT block other records.

## TEAM.SYS.CONV-R10 — Offline Authoring and Local Durability

A Member SHALL be able to create Channels and post Messages while no peer is reachable. A local record SHALL appear only after Core has committed it and the Package has stored it durably. Replaying a record SHALL NOT create a duplicate. After reconnection Installations SHALL converge without lost or duplicate Channels or Messages. Projections and read state SHALL survive Backend recovery and application restart. A local store with an unknown newer schema version SHALL fail closed without modification.

## TEAM.SYS.CONV-R11 — Record Protection and Key Custody (Version 1)

Conversations records SHALL be sealed by the Core Runtime, which retains raw content-key custody and never exposes raw keys to the Package. Package persistence SHALL contain sealed record bytes and non-content indexes only (identifiers, order keys, author Public Identities, cursors, and local read positions). Plaintext SHALL be held only in memory. Version 1 SHALL use one content key per Package and Workspace, with creator-only first-key bootstrap and grants from current key holders. It has no rotation, so revocation does not remove key material the revoked Member already holds.

## TEAM.SYS.CONV-R12 — History for Newly Admitted Members

A newly admitted Member SHALL receive the existing Conversations history once a current key holder has granted access. Subscription-driven acquisition of missing history access, including an empty `#general`, is owned by TEAM.SYS.TRANS-R08.

## TEAM.SYS.CONV-R13 — Local Read State

Each Installation SHALL keep a local, unsynchronized last-read position per Channel, based on local arrival order. Opening a Channel SHALL mark its current Messages read. The Package SHALL expose an unread Message count per Channel in its own projection and UI contract. Presenting that count is a Package UI choice.

## TEAM.SYS.CONV-R14 — Synchronization Availability

The Package SHALL report Package-level availability of `waiting_for_history_access`, `offline`, `catching_up`, or `current` as reported by the Synchronization Capability, and SHALL NOT claim per-peer delivery or read receipts.

## TEAM.SYS.CONV-R15 — Safe Message Rendering

The Package UI SHALL render only a limited Markdown subset (paragraphs, emphasis, headings, lists, quotations, code, horizontal rules, and links). It SHALL NOT render raw HTML, images, or other fetchable media. Only `http`, `https`, `mailto`, `tel`, and relative links SHALL be allowed, and they SHALL open with no opener or referrer.

## TEAM.SYS.CONV-R17 — Workspace Principal Author Presentation

The Conversations Backend SHALL declare and consume the generic `teamarness.principals` 1.0 capability for current Workspace Principals and bounded, resumable profile updates. The typed Package UI contract SHALL forward Principal snapshots independently of immutable Message records. The UI SHALL resolve each Message author by its signed author key, SHALL label it with the Principal's current name, and SHALL present a Workspace Agent author with a compact information icon beside the author name. Its tooltip SHALL identify the author as an Agent and show its Steward's name on pointer hover and keyboard focus. Escape SHALL dismiss the tooltip without moving focus. The Message payload and the declared-author check (`TEAM.SYS.CONV-R09`) SHALL be unchanged. The UI SHALL relabel already loaded remote Messages after rename without modifying their signed author identity; own Messages SHALL keep the stable `You` label. Missing, duplicate, or unavailable profile data SHALL retain a visible distinguishing author identity fallback, and the full identity SHALL remain inspectable. A profile stream failure SHALL NOT prevent Channel or Message use. The Core Runtime, Host and Shell SHALL contain no Conversations-specific name logic (ADR 0023).

## TEAM.SYS.CONV-R18 — Agent-Authored Posts and Channel Discovery (Port Contract 1.1)

The Conversations Port Contract `dev.teamarness.conversations` 1.1 SHALL be additive over 1.0: `message.post`, `message.page`, and `messages.changed` SHALL keep their kinds and schemas. On `message.post` the Package SHALL accept the Host-derived actor only: a Person actor SHALL be refused unless it is this Installation; an Agent actor SHALL be accepted as given, the Message SHALL name the Agent's signing key as its author, and the Package SHALL pass the actor's Principal ID as `principal` to the record-log 1.1 append so that the Core Runtime signs with the Agent's key under the standing local Agent authority rule (ADR 0030) or refuses the post. The Package SHALL NOT select a signer from request content, SHALL NOT store Agent Bindings, and SHALL keep the Message payload and declared-author check (`TEAM.SYS.CONV-R09`) unchanged, so an Agent-authored Message is presented through `TEAM.SYS.CONV-R17` without a UI change. A `channel.list` query SHALL return the Channels a caller may post to as `{channels:[{id, name}]}`, at most 64, with the same visibility and order as the Package's own Channel snapshot (`#general` first, hidden Channels excluded).

## TEAM.SYS.CONV-R19 — Revision-Based Port Recovery (Port Contract 1.2)

The Conversations Port Contract 1.2 SHALL add a read-only `message.changes` query without changing existing Port kinds or schemas. The query SHALL reuse the Package's projection-change journal and accept `channelId`, `epoch`, decimal-string `after` revision, and an optional limit of 1 to 32. It SHALL return at most 32 insert/removal events in revision order with a bounded continuation and captured high-water revision, independently of Message display order. Insert events SHALL identify ready, pending, or unavailable content. A changed epoch or ahead cursor SHALL report `resyncRequired` rather than replaying an unrelated history. Consumers SHALL retain unresolved pending work before acknowledging its revision and MAY safely retry the query. The query SHALL NOT mark a Channel read. The existing `messages.changed` event SHALL remain a coalescible, live-only invalidation; Consumers SHALL query changes after notification or reconnection rather than treating each invalidation as one Message.

## TEAM.SYS.CONV-R20 — Runtime Port Guidance (Port Contract 1.3)

The Conversations Port Contract 1.3 SHALL add an ordinary read-only `runtime.guide` query with a closed empty request and a closed response requiring a bounded `markdown` string. The actual encoded response SHALL be at most 32 KiB. Guidance SHALL derive numeric limits from enforced Backend constants and explain Channel discovery, bounded pages and change batches, the UTF-8-byte post limit, subscribe-before-read, initial epoch/revision, high-water and continuation handling, acknowledged prefixes, pending and unavailable content, and explicit changed-epoch reconstruction. It SHALL state that Port reads do not mark Channels read, authorship is Host-derived, hidden Channels refuse posts, Provider errors retain their envelopes, reads are repeatable, and uncertain posts must never be retried automatically. The five existing operation kinds and schemas SHALL remain unchanged. A conforming guide SHALL make bound-Agent access default-denied under Packages R13 until the calling harness explicitly enables the contract. No new UI behavior or domain command follows from this query.

## TEAM.SYS.CONV-R16 — Delivery Qualification

On an ordinary supported development machine, a reachable peer SHALL show a new Message within 5 seconds, and a reconnecting peer SHALL catch up on 1,000 queued records within 30 seconds. Delivery SHALL meet these targets in a Workspace of 25 Members. Channel and history queries SHALL stay bounded with 100 Channels and 100,000 Messages in one Workspace.
