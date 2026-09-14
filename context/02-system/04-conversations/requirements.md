# 02-system/04-conversations — Conversations Requirements

Prefix: `TEAM.SYS.CONV`

*Requirements for this subsystem will be authored during Phase 4 scoping. The Conversations Package owns Channels, signed Markdown Messages, Channel Membership, conversation keys, domain policy, local schemas and projections, offline behavior, and unread state. It consumes Canonical Membership as an eligibility ceiling. Peer replication will use a generic Package-scoped Synchronization Capability supplied by the Core Runtime rather than a Conversations-specific Core Runtime branch. Phase 4 design still owns the capability's replication and authorization semantics.*