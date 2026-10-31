//! Runtime advice for the existing public Ports. Numeric limits come from their enforcement.

use serde_json::{Value, json};

use crate::domain::{MAX_MARKDOWN, PAGE_SIZE};

pub(crate) fn response(channel_list_limit: usize) -> Value {
    let markdown = format!(
        r#"# Conversations Port guide

## Discover and read Channels

channel.list with {{}} returns at most {channel_list_limit} visible Channels in creation order, with #general first. Hidden Channels are excluded. The list has no continuation, so it is not a complete catalog when more visible Channels exist. Channel IDs are 32 lowercase hexadecimal characters. Use an ID returned by discovery, not a guessed name.

message.page takes channelId, optional before, and optional limit. The Backend reads at most {PAGE_SIZE} raw rows per page, even when the schema permits a larger limit. Responses can be shorter to fit the encoded-byte budget. messages contains ready content; pending contains identified rows with pending or unavailable status. Use nextBefore exactly as returned to read older history. It advances through raw rows, including unreadable ones. Absence of ready messages alone does not mean the beginning of history.

## Subscribe and reconcile

Subscribe to messages.changed before the initial message.page. The empty event is a Workspace-wide, live-only invalidation, not one Message. There is no initial event or replay, and notifications may coalesce. Reconcile after each notification and after explicit reconnect. A display can repeat page reads. A consumer that processes each new Message uses message.changes rather than display position or wall-clock time.

Save the initial page's epoch and revision together with the state you have examined. revision, after, continuation and highWater are decimal strings, not display positions. message.changes takes channelId, epoch, after and optional limit and returns at most {PAGE_SIZE} changes in revision order. Insert events contain ready message content or pending/unavailable status; remove events identify Messages to remove locally. highWater is the captured projection head. continuation is the safe query cursor through that batch, which can advance over changes for other Channels. Continue from continuation while behind highWater, even if a short batch contains no ready content. A later query can capture a newer highWater.

Persist only an acknowledged prefix whose effects and unresolved work you have retained durably. Do not advance blindly to highWater or to the last Message's display position. For pending content, keep its identity and the cursor before its change, then repeat message.changes from that cursor to try opening it again. A repeated message.page can also reopen pending history. Deduplicate already processed revisions and Message IDs. unavailable means content cannot currently be rendered (for example corrupt, wrong-scope or removed content), not an empty Message. Apply an explicit skip or stop policy before acknowledging unavailable work. Neither read authorizes a repeated side effect.

resyncRequired:true means the epoch changed or after is ahead of the current projection. Stop side effects based on the old cursor. Explicitly reconstruct the loaded range with fresh message.page reads, including nextBefore pages as needed, and save a new epoch/revision baseline. Do not silently reuse the old epoch or replay posts while rebuilding. These Port reads do not mark Channels read.

## Post with Host-derived authorship

message.post takes only channelId and markdown. Markdown must contain 1 to {MAX_MARKDOWN} UTF-8 bytes ({post_kib} KiB), not that many Unicode characters. JSON Schema maxLength counts characters; the Backend also enforces the byte bound. The Host-derived actor selects authorship: this Installation Person or the calling harness's bound Workspace Agent. Never supply an actor, author, signer or Principal in the request. Core signs with that actor's key or refuses. Conversations stores no caller binding. message.post refuses hidden or inactive Channels with conversations.channel_hidden; #general is permanent. There are no public create-Channel, edit, delete, reply, or mark-read Ports.

## Errors and repeatability

The Host validates closed schemas and its own authority before dispatch. A Provider refusal arrives as provider_error with details.provider.code, details.provider.retryable and bounded details.provider.details. Inspect outcomeUncertain as well as the code. conversations.invalid_request and conversations.invalid_markdown require corrected input. conversations.actor_unauthorized refuses a Person from another Installation. conversations.capability_unavailable carries causeCode in Provider details, including principal_not_bound when Core refuses Agent signing. conversations.storage_failed, conversations.invalid_open_result and conversations.response_limit can stop a read; a response_limit read can be repeated with a smaller limit. A committed post can report conversations.deferred, conversations.rejected or conversations.storage_failed with an uncertain outcome after local projection failure.

runtime.guide, channel.list, message.page and message.changes are read-only and repeatable, but their results can change. Keep an explicit retry state on a failed read; an unrelated notification is not proof that failure was repaired. message.post is not repeatable: every accepted post creates a new Message. Never automatically retry an uncertain message.post, including outcome_unknown after timeout, Provider exit or an invalid response. Read and reconcile first; absence from a page does not prove the post failed. An already authorized command keeps its original actor and may finish after access changes. Reconnect streams explicitly and never replay commands on reconnect.
"#,
        post_kib = MAX_MARKDOWN / 1024,
    );
    json!({ "markdown": markdown })
}
