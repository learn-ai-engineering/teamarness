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

After convergence every Installation SHALL show each Channel's Messages in the same order, derived from Core-provided order keys rather than wall-clock time. The newest Message SHALL appear at the bottom of the Channel, with older Messages above it. Opening a Channel SHALL show its newest page of at most 32 Messages, scrolled to the bottom, and older pages SHALL load on request.

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

## TEAM.SYS.CONV-R16 — Delivery Qualification

On an ordinary supported development machine, a reachable peer SHALL show a new Message within 5 seconds, and a reconnecting peer SHALL catch up on 1,000 queued records within 30 seconds. Delivery SHALL meet these targets in a Workspace of 25 Members. Channel and history queries SHALL stay bounded with 100 Channels and 100,000 Messages in one Workspace.
