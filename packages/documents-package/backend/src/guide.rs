//! Runtime advice for the public Documents Ports (1.2). Numeric limits come from their
//! enforcement.

use serde_json::{Value as JsonValue, json};

use super::ports::{MAX_PAGE, MAX_VERSION_IDS, PORT_PAYLOAD_BYTES};
use crate::body::MAX_TEXT_BYTES;
use crate::domain::{MAX_TITLE, MAX_UPDATE};

pub(super) fn response() -> JsonValue {
    let markdown = format!(
        r#"# Documents Port guide

## Discover and read

document.list with {{}} returns Collections and Documents in Order Key order, at most {MAX_PAGE} entries per page. A page can be shorter to fit the {payload_kib} KiB Port value bound. While next is present, pass it as after to read the following page; a missing next means the listing is complete. With collectionId, the page holds only that Collection's Documents. Each Collection has id, name and archived. Each Document has id, collectionId, title, archived and body: available, pending or unavailable. Only an available body has a version. IDs are 64 lowercase hexadecimal characters. Titles need not be unique, so use an ID returned by document.list, not a guessed title.

document.markdown takes documentId and returns the body as Canonical Markdown with the version it was exported at. documents.pending means this Installation holds records for the Document that it cannot open yet; read again later. documents.unavailable means a conflict or a failed import stopped the body, or its version needs more than {MAX_VERSION_IDS} IDs to carry; it does not mean the Document is empty. documents.too_large means the response would exceed the {payload_kib} KiB Port value bound.

documents.changed is a live-only invalidation that may coalesce, with no initial event or replay. It names at most {MAX_PAGE} changed Documents in documentIds; without documentIds, list again. Read again on receipt and after reconnecting.

## Create

document.create takes only collectionId and title, and creates an empty Document. The title is 1 to {MAX_TITLE} characters, with no control characters and no leading or trailing whitespace. The Collection must exist and must not be archived. The response is documentId and version.

To give the new Document content, call document.revise with that documentId, baseVersion set to the returned version, and the complete Markdown you want. Create then revise is two commands. If the revise is refused, the empty Document remains: revise it again, against a fresh read, rather than create another. A revise straight after a create can report documents.pending until the body is open; read it with document.markdown and revise again.

## Revise against a version

document.revise takes documentId, baseVersion and markdown. markdown is the whole Document as you want it, revised from the Markdown that document.markdown exported at baseVersion. Read first, change that text, and pass the version you read as baseVersion. Version tokens are opaque: never construct or change one.

The Provider merges your revision from its base into the current body. Unchanged blocks are untouched, changed blocks are edited in place, and edits other Members or Agents made since the base survive. The result can therefore differ from your Markdown, so read again before the next revise. changed false means the revision changed nothing and no record was written. normalised true means your Markdown was not Canonical Markdown and was normalised before the merge. The returned version is the one your own revision left the Document at; it may include concurrent edits.

One revise is one record of at most {update_kib} KiB of changes. A larger change is refused with documents.revision_too_large: split it into several revises, each against the version the previous one returned. A body may hold at most {text_kib} KiB of text; a revision that grows it further is refused with documents.document_too_large. documents.schema_violation means the Markdown cannot be held in a Document body. documents.base_unavailable means the base is unknown here or is not a token from this Provider: read again and revise against the new version. documents.unknown_document names a Document that does not exist here.

## Host-derived authorship

document.create and document.revise act as the Host-derived actor: this Installation Person, or the Workspace Agent the calling Package Instance is bound to. Never supply an actor, author, signer or Principal in a request. An Agent's record names the Agent's signing key as its author, and Core signs it with that key or refuses. A Person from another Installation is refused with documents.actor_unauthorized. Queries and the event accept any Host-derived actor.

## Errors and repeatability

The Host validates closed schemas and its own authority before dispatch. A Provider refusal arrives as provider_error with details.provider.code, details.provider.retryable and bounded details.provider.details. Inspect outcomeUncertain as well as the code.

document.create refuses an unknown Collection with documents.unknown_collection, an archived Collection with documents.collection_archived, and an invalid title with documents.invalid_title. documents.principal_not_bound means Core does not sign for this Agent now; nothing was written. These refusals are certain. documents.capability_unavailable carries causeCode in Provider details. documents.storage_failed, documents.deferred and documents.rejected can follow a committed record, so treat them as uncertain when outcomeUncertain is set.

runtime.guide, document.list and document.markdown are read-only and repeatable, but their results can change. Treat document.revise as not repeatable: after an uncertain revise, read the Document and decide from what it holds. document.create is not repeatable: every accepted create makes a new Document, so a retry duplicates it. Never automatically retry an uncertain command, including outcome_unknown after a timeout, Provider exit or an invalid response. List and reconcile first: look for the Document by Collection and title before you create it again, and read it before you revise it again.
"#,
        payload_kib = PORT_PAYLOAD_BYTES / 1024,
        update_kib = MAX_UPDATE / 1024,
        text_kib = MAX_TEXT_BYTES / 1024,
    );
    json!({ "markdown": markdown })
}
