//! Documents Port Contract 1.1 (`dev.teamarness.documents`, `ports.json`): another Package lists
//! Documents, reads one as Canonical Markdown with a version token, revises it against a base
//! version as the Host-derived actor, and observes changes. This is the agent surface.
//!
//! A revise is one Markdown merge from a fresh peer and exactly one `document.edit` record,
//! through the same validation as an editor session's update. The local Installation Person's
//! revise takes the editor's write path; a bound Agent's (1.1) is appended with its Principal ID
//! before the body adopts it (ADR 0030, ADR 0033).

use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use loro::{Frontiers, ID};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::ports::PortActor;
use teamarness_package_sdk::{BackendError, Context, EventSubscription, event_channel};
use tokio::sync::broadcast;

use super::{
    BodyEvent, BodyHub, DocumentsActive, LocalWrite, append_identified_edit, drain_local_writes,
    error, reopen_unopened_edits, require_collection, require_document, stage_local_write,
    storage_failed,
};
use crate::markdown;

pub(super) const CONTRACT: &str = "dev.teamarness.documents";

/// The most frontier IDs one version token carries: 1 024 base64url characters.
pub(crate) const MAX_VERSION_IDS: usize = 64;
/// One frontier ID: an 8-byte big-endian peer, then a 4-byte big-endian counter.
const ID_BYTES: usize = 12;
/// The most entries one `document.list` page holds, and the most Document IDs one
/// `documents.changed` event names.
const MAX_PAGE: usize = 64;
/// The Host's bound on one Port value, `MAX_UI_PAYLOAD_BYTES` in the Package contract crate,
/// which a test pins this to. A Package depends only on the SDK and the protocol (ARC-10), so
/// the bound is restated here rather than imported.
pub(crate) const PORT_PAYLOAD_BYTES: usize = 256 * 1024;

fn pending() -> BackendError {
    error("documents.pending", true)
}
fn unavailable() -> BackendError {
    error("documents.unavailable", false)
}
fn base_unavailable() -> BackendError {
    error("documents.base_unavailable", false)
}

// ---------------------------------------------------------------- version token

/// The canonical version token of `frontiers`: its IDs sorted by `(peer, counter)`, each as
/// 12 bytes, in base64url without padding. `None` when the frontiers are empty (a body without
/// its genesis) or exceed [`MAX_VERSION_IDS`].
pub(crate) fn encode_version(frontiers: &Frontiers) -> Option<String> {
    let mut ids: Vec<ID> = frontiers.iter().collect();
    if ids.is_empty() || ids.len() > MAX_VERSION_IDS {
        return None;
    }
    ids.sort_by_key(|id| (id.peer, id.counter));
    ids.dedup();
    let mut bytes = Vec::with_capacity(ids.len() * ID_BYTES);
    for id in ids {
        bytes.extend_from_slice(&id.peer.to_be_bytes());
        bytes.extend_from_slice(&u32::try_from(id.counter).ok()?.to_be_bytes());
    }
    Some(URL_SAFE_NO_PAD.encode(bytes))
}

/// Reads a version token strictly: only the exact encoding [`encode_version`] produces is
/// accepted. Anything else is `None`, which the provider answers as `base_unavailable`.
pub(crate) fn decode_version(token: &str) -> Option<Frontiers> {
    let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
    if bytes.is_empty() || bytes.len() % ID_BYTES != 0 || bytes.len() / ID_BYTES > MAX_VERSION_IDS {
        return None;
    }
    let mut ids = Vec::with_capacity(bytes.len() / ID_BYTES);
    for chunk in bytes.as_chunks::<ID_BYTES>().0 {
        let (peer, counter) = chunk.split_at(8);
        let peer = u64::from_be_bytes(peer.try_into().ok()?);
        let counter = i32::try_from(u32::from_be_bytes(counter.try_into().ok()?)).ok()?;
        let id = ID::new(peer, counter);
        // Strictly increasing: sorted, and no ID twice.
        if ids
            .last()
            .is_some_and(|last: &ID| (last.peer, last.counter) >= (id.peer, id.counter))
        {
            return None;
        }
        ids.push(id);
    }
    let frontiers = Frontiers::from(ids);
    // Base64url has one encoding per byte string once padding and trailing bits are refused;
    // re-encoding proves it.
    (encode_version(&frontiers).as_deref() == Some(token)).then_some(frontiers)
}

// ---------------------------------------------------------------- body state

/// What a Consumer can do with a Document's body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BodyState {
    /// Every known record is imported: it can be read and revised.
    Available,
    /// Records this Installation holds cannot be opened yet.
    Pending,
    /// An identity conflict or an import failure stopped the body.
    Unavailable,
}

impl BodyState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Pending => "pending",
            Self::Unavailable => "unavailable",
        }
    }
}

fn body_state(hub: &BodyHub, document: &str) -> Result<BodyState, BackendError> {
    if hub
        .store
        .identity_conflicted(document)
        .map_err(|_| storage_failed())?
        || hub
            .store
            .import_failure(document)
            .map_err(|_| storage_failed())?
            .is_some()
    {
        return Ok(BodyState::Unavailable);
    }
    Ok(hub.with_body(document, |body| {
        if body.editable() && !body.frontiers().is_empty() {
            BodyState::Available
        } else {
            BodyState::Pending
        }
    }))
}

/// Refuses a body that cannot be read or revised now, after one attempt to open its records.
async fn require_available(
    context: &Context<DocumentsActive>,
    document: &str,
) -> Result<(), BackendError> {
    require_document(context, document).await?;
    reopen_unopened_edits(context, document).await?;
    match body_state(&context.state().hub, document)? {
        BodyState::Available => Ok(()),
        BodyState::Pending => Err(pending()),
        BodyState::Unavailable => Err(unavailable()),
    }
}

// ---------------------------------------------------------------- document.list

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListRequest {
    #[serde(default)]
    collection_id: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// One page of Collections and Documents in Order Key order. With `collectionId`, only the
/// Documents in that Collection. `next` names the last entry when more follow.
pub(super) async fn list(
    context: &Context<DocumentsActive>,
    request: ListRequest,
) -> Result<JsonValue, BackendError> {
    if let Some(collection) = &request.collection_id {
        require_collection(context, collection).await?;
    }
    let limit = request.limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE);
    let state = context.state().clone();
    let (collections, documents) = context
        .blocking(move |_| {
            Ok((
                state.store.collections().map_err(|_| storage_failed())?,
                state.store.documents().map_err(|_| storage_failed())?,
            ))
        })
        .await?;
    let hub = &context.state().hub;
    // (cursor, is_document, value)
    let mut entries: Vec<(String, bool, JsonValue)> = Vec::new();
    if request.collection_id.is_none() {
        for collection in collections {
            entries.push((
                format!("{}:{}", collection.order_key, collection.id),
                false,
                json!({
                    "id": collection.id,
                    "name": collection.name.unwrap_or_else(|| "Untitled".into()),
                    "archived": collection.archived,
                }),
            ));
        }
    }
    for document in documents {
        if request
            .collection_id
            .as_ref()
            .is_some_and(|collection| *collection != document.collection_id)
        {
            continue;
        }
        let state = body_state(hub, &document.id)?;
        let mut value = json!({
            "id": document.id,
            "collectionId": document.collection_id,
            "title": document.title.unwrap_or_else(|| "Untitled".into()),
            "archived": document.archived,
            "body": state.as_str(),
        });
        if state == BodyState::Available
            && let Some(version) =
                hub.with_body(&document.id, |body| encode_version(&body.frontiers()))
        {
            value["version"] = json!(version);
        }
        entries.push((
            format!("{}:{}", document.order_key, document.id),
            true,
            value,
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let start = match &request.after {
        None => 0,
        Some(after) => {
            entries
                .iter()
                .position(|(cursor, _, _)| cursor == after)
                .ok_or_else(|| error("documents.invalid_cursor", false))?
                + 1
        }
    };
    let mut end = (start + limit).min(entries.len());
    loop {
        let page = &entries[start..end];
        let mut value = json!({
            "collections": page.iter().filter(|e| !e.1).map(|e| &e.2).collect::<Vec<_>>(),
            "documents": page.iter().filter(|e| e.1).map(|e| &e.2).collect::<Vec<_>>(),
        });
        if end < entries.len()
            && let Some((cursor, _, _)) = page.last()
        {
            value["next"] = json!(cursor);
        }
        // A page shrinks rather than exceed the Port value bound; one entry always fits.
        if end <= start + 1 || value.to_string().len() <= PORT_PAYLOAD_BYTES {
            return Ok(value);
        }
        end -= 1;
    }
}

// ---------------------------------------------------------------- document.markdown

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MarkdownRequest {
    document_id: String,
}

/// The body as Canonical Markdown, with the version token it was exported at.
pub(super) async fn read_markdown(
    context: &Context<DocumentsActive>,
    request: MarkdownRequest,
) -> Result<JsonValue, BackendError> {
    require_available(context, &request.document_id).await?;
    let (markdown, version) = context.state().hub.with_body(&request.document_id, |body| {
        (body.text().to_owned(), encode_version(&body.frontiers()))
    });
    // A version the token cannot carry converges to one ID after the next local edit.
    let version = version.ok_or_else(unavailable)?;
    let value = json!({ "markdown": markdown, "version": version });
    if value.to_string().len() > PORT_PAYLOAD_BYTES {
        return Err(error("documents.too_large", false));
    }
    Ok(value)
}

// ---------------------------------------------------------------- document.revise

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReviseRequest {
    document_id: String,
    base_version: String,
    markdown: String,
}

/// Who a revise is written as: the signing key its record names as author and, for an Agent,
/// the Principal ID Core signs for.
pub(super) struct Signer {
    identity: String,
    /// `None` for the Installation Person, whom Core signs for by default.
    principal: Option<String>,
}

/// Turns the Host-authored actor into the signer a revise is written as.
///
/// A Person must be this Installation. An Agent actor is Host-derived from the calling Package
/// Instance's Agent Binding (ADR 0030), so it is accepted as given: the record names the Agent's
/// key, and Core signs with that key or refuses with `principal_not_bound`.
pub(super) fn authorize_actor(
    context: &Context<DocumentsActive>,
    actor: PortActor,
) -> Result<Signer, BackendError> {
    match actor {
        PortActor::Person { identity } if identity == context.state().installation => Ok(Signer {
            identity,
            principal: None,
        }),
        PortActor::Agent {
            principal,
            identity,
        } => Ok(Signer {
            identity,
            principal: Some(principal),
        }),
        PortActor::Person { .. } => Err(error("documents.actor_unauthorized", false)),
    }
}

/// The Port's name for a refusal from the shared local write path.
fn revise_error(cause: BackendError) -> BackendError {
    let code = match cause.code() {
        "documents.document_pending" => "documents.pending",
        "documents.identity_conflict" | "documents.import_failure" => "documents.unavailable",
        "documents.edit_request_too_large" => "documents.revision_too_large",
        _ => return cause,
    };
    BackendError::new(code, cause.retryable(), json!({}))
        .with_outcome_uncertain(cause.outcome_uncertain())
}

/// Merges a Markdown revision made against `baseVersion` as exactly one record signed as
/// `signer`, or none when it changes nothing, and answers once that record has reached the Core
/// record log.
pub(super) async fn revise(
    context: &Context<DocumentsActive>,
    signer: Signer,
    request: ReviseRequest,
) -> Result<JsonValue, BackendError> {
    require_available(context, &request.document_id).await?;
    let base = decode_version(&request.base_version).ok_or_else(base_unavailable)?;
    let normalised = markdown::canonical(&request.markdown)
        .map_err(|_| error("documents.schema_violation", false))?
        != request.markdown;
    let request_id = format!(
        "{:016x}{:016x}",
        getrandom::u64().map_err(|_| error("documents.edit_failed", true))?,
        getrandom::u64().map_err(|_| error("documents.edit_failed", true))?
    );
    let state = context.state();
    let mut staged = stage_local_write(
        &state.log,
        &state.hub,
        &signer.identity,
        &request.document_id,
        &request_id,
        LocalWrite::Revision {
            base: &base,
            markdown: &request.markdown,
            append_first: signer.principal.is_some(),
        },
    )
    .await
    .map_err(revise_error)?;
    if let Some(edit) = staged.unstaged.take() {
        // Nothing is staged, so nothing is retried: a refusal leaves the body as it was, and a
        // failure that may have committed is reported as uncertain. The body adopts the
        // revision from its appended record, while this Document's append lock is held.
        append_identified_edit(
            &state.log,
            &state.hub,
            &state.changes,
            &signer.identity,
            signer.principal.as_deref(),
            &request.document_id,
            &edit.request_id,
            edit.digest,
            edit.update,
        )
        .await
        .map_err(|cause| {
            if cause.code() == "documents.principal_not_bound" {
                cause
            } else {
                revise_error(cause).with_outcome_uncertain(true)
            }
        })?;
    } else if staged.changed {
        // The staged record is retried until it is appended, so a failure here may still
        // commit: the command's outcome is unknown and must not be replayed automatically.
        drain_local_writes(
            &state.log,
            &state.hub,
            &state.changes,
            &signer.identity,
            &request.document_id,
        )
        .await
        .map_err(|cause| revise_error(cause).with_outcome_uncertain(true))?;
    }
    let mut value = json!({ "changed": staged.changed, "normalised": normalised });
    if let Some(version) = encode_version(&staged.version) {
        value["version"] = json!(version);
    }
    Ok(value)
}

// ---------------------------------------------------------------- documents.changed

/// The listing a Consumer would see apart from bodies, to tell metadata changes from others.
fn listing_fingerprint(state: &DocumentsActive) -> Result<String, BackendError> {
    let collections = state.store.collections().map_err(|_| storage_failed())?;
    let documents = state.store.documents().map_err(|_| storage_failed())?;
    Ok(format!("{collections:?}{documents:?}"))
}

/// Live-only and coalescing: one event waits at a time, and whatever changes meanwhile is
/// folded into the next. A body change names its Document; a listing change, a lagged feed or
/// more than [`MAX_PAGE`] Documents send an event without `documentIds`, and the Consumer
/// re-lists.
pub(super) fn changed(
    context: &Context<DocumentsActive>,
) -> Result<EventSubscription, BackendError> {
    let (sender, subscription) = event_channel(1);
    let mut bodies = context.state().hub.events.subscribe();
    let mut metadata = context.state().changes.subscribe();
    let context = context.clone();
    tokio::spawn(async move {
        let state = context.state().clone();
        let Ok(mut listing) = context.blocking(move |_| listing_fingerprint(&state)).await else {
            return;
        };
        loop {
            let mut documents = BTreeSet::new();
            let mut listing_changed = false;
            let mut relist = false;
            tokio::select! {
                () = sender.closed() => return,
                result = metadata.changed() => {
                    if result.is_err() {
                        return;
                    }
                    listing_changed = true;
                }
                event = bodies.recv() => match event {
                    Ok(event) => note_document(&event, &mut documents),
                    Err(broadcast::error::RecvError::Lagged(_)) => relist = true,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
            loop {
                match bodies.try_recv() {
                    Ok(event) => note_document(&event, &mut documents),
                    Err(broadcast::error::TryRecvError::Lagged(_)) => relist = true,
                    Err(_) => break,
                }
            }
            if listing_changed || metadata.has_changed().unwrap_or(false) {
                metadata.borrow_and_update();
                let state = context.state().clone();
                let Ok(next) = context.blocking(move |_| listing_fingerprint(&state)).await else {
                    return;
                };
                if next != listing {
                    listing = next;
                    relist = true;
                }
            }
            let event = if relist || documents.len() > MAX_PAGE {
                json!({})
            } else if documents.is_empty() {
                continue;
            } else {
                json!({ "documentIds": documents })
            };
            if sender.send(event).await.is_err() {
                return;
            }
        }
    });
    Ok(subscription)
}

fn note_document(event: &BodyEvent, documents: &mut BTreeSet<String>) {
    documents.insert(event.document().to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontiers(ids: &[(u64, i32)]) -> Frontiers {
        Frontiers::from(
            ids.iter()
                .map(|(peer, counter)| ID::new(*peer, *counter))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn version_tokens_round_trip_in_canonical_order() {
        let one = frontiers(&[(0x0102_0304_0506_0708, 9)]);
        let token = encode_version(&one).unwrap();
        assert_eq!(token.len(), 16);
        assert_eq!(decode_version(&token).unwrap(), one);
        // IDs are sorted by (peer, counter), whatever order the frontiers hold them in.
        let forward = frontiers(&[(1, 5), (2, 0), (u64::MAX, i32::MAX)]);
        let backward = frontiers(&[(u64::MAX, i32::MAX), (2, 0), (1, 5)]);
        let token = encode_version(&forward).unwrap();
        assert_eq!(encode_version(&backward).unwrap(), token);
        let mut decoded: Vec<ID> = decode_version(&token).unwrap().iter().collect();
        decoded.sort_by_key(|id| (id.peer, id.counter));
        let mut expected: Vec<ID> = forward.iter().collect();
        expected.sort_by_key(|id| (id.peer, id.counter));
        assert_eq!(decoded, expected);
        let bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        assert_eq!(&bytes[..8], &1u64.to_be_bytes());
        assert_eq!(&bytes[8..12], &5u32.to_be_bytes());
    }

    #[test]
    fn non_canonical_version_tokens_are_refused() {
        let canonical = |ids: &[(u64, u32)]| {
            let mut bytes = Vec::new();
            for (peer, counter) in ids {
                bytes.extend_from_slice(&peer.to_be_bytes());
                bytes.extend_from_slice(&counter.to_be_bytes());
            }
            bytes
        };
        let sorted = canonical(&[(1, 1), (2, 1)]);
        assert!(decode_version(&URL_SAFE_NO_PAD.encode(&sorted)).is_some());
        for bytes in [
            // Out of order, and a repeated ID.
            canonical(&[(2, 1), (1, 1)]),
            canonical(&[(1, 1), (1, 1)]),
            // A counter Loro cannot hold.
            canonical(&[(1, u32::MAX)]),
            // Not a whole number of IDs, and nothing at all.
            sorted[..13].to_vec(),
            Vec::new(),
        ] {
            assert!(
                decode_version(&URL_SAFE_NO_PAD.encode(&bytes)).is_none(),
                "{bytes:?}"
            );
        }
        let token = URL_SAFE_NO_PAD.encode(&sorted);
        for variant in [
            format!("{token}="),
            format!("+{}", &token[1..]),
            format!(" {token}"),
            String::new(),
        ] {
            assert!(decode_version(&variant).is_none(), "{variant}");
        }
    }

    #[test]
    fn version_tokens_are_bounded_at_sixty_four_ids() {
        let ids: Vec<(u64, i32)> = (0..64).map(|peer| (peer, 0)).collect();
        let token = encode_version(&frontiers(&ids)).unwrap();
        assert_eq!(token.len(), 1024);
        assert!(decode_version(&token).is_some());
        let ids: Vec<(u64, i32)> = (0..65).map(|peer| (peer, 0)).collect();
        assert!(encode_version(&frontiers(&ids)).is_none());
        let mut bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        bytes.extend_from_slice(&[0xff; ID_BYTES]);
        assert!(decode_version(&URL_SAFE_NO_PAD.encode(&bytes)).is_none());
        assert!(encode_version(&Frontiers::default()).is_none());
    }

    #[test]
    fn the_port_payload_bound_is_the_hosts() {
        const {
            assert!(
                PORT_PAYLOAD_BYTES
                    == teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES
            )
        };
    }
}
