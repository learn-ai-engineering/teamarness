# 02-system/04-conversations — Conversations Requirements

Prefix: `RES.SYS.CONV`

*Requirements for this subsystem will be authored during Phase 3 scoping. The conversations package owns channels, signed Markdown messages, channel membership, conversation keys, Iroh-based P2P transport, local persistence, offline queueing, and unread state. It consumes canonical workspace membership as an eligibility ceiling. The package maintains an allow-list of Iroh `EndpointId`s derived from membership epochs.*