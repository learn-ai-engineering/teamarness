//! The in-memory body of one Document: a Loro tree of the shared schema, the Canonical
//! Markdown it currently renders, and the bookkeeping that validates an update before it
//! becomes a record.
//!
//! Nothing here is persisted. A body is rebuilt at activation by importing the genesis of its
//! opened `document.create` record and every opened `document.edit` record; the Package SQLite
//! never holds plaintext.
//!
//! The backend never edits a body as a Loro peer of its own. Every operation comes from a peer
//! that [`DocumentBody::allocate_peer`] handed out: one per genesis, per Markdown merge and (from
//! the editor session seam) per UI session. Peers are random, absent from the Document's
//! version vector, never reused in this process, and never derived from the Installation.
//!
//! An editor session loads the body from [`DocumentBody::snapshot`] and then follows the
//! operations each change adds ([`DocumentBody::updates_since`]); it never replays records.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, PoisonError};

use loro::{ExportMode, Frontiers, LoroDoc, LoroError, VersionVector};

use crate::domain::MAX_UPDATE;
use crate::markdown::{self, Block, Inline, MergeError, SchemaViolation};

/// Local growth budget, in UTF-8 bytes of text content. Valid remote merges can exceed it, and
/// then only edits that do not grow the body are accepted until it is back under budget.
pub(crate) const MAX_TEXT_BYTES: usize = 192 * 1024;
/// Only recent IDs stay in the plaintext in-memory projection; old IDs are opened lazily.
pub(crate) const MAX_RECENT_IDENTITIES: usize = 256;

/// Every peer this process has handed out, so a peer is never reused even across Documents.
static ALLOCATED_PEERS: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Mutex::default);

/// Why the body refused an update or a revision. Nothing was staged or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BodyError {
    /// The update holds operations from a peer other than the submitting session's own.
    ForeignPeer,
    /// The update, or a merged revision, does not fit one record ([`MAX_UPDATE`]).
    RecordTooLarge,
    /// The resulting tree is not a body under the shared schema.
    Schema(SchemaViolation),
    /// The update would grow the body beyond [`MAX_TEXT_BYTES`], or grow one already above it.
    TooLarge,
    /// The bytes are not a Loro update, add no operation, or depend on history the body lacks.
    InvalidUpdate,
    /// A revision's base version is not in the body's history.
    BaseUnavailable,
    /// The body still waits for history, or an earlier accepted update is not yet appended.
    NotReady,
    /// A request ID already names a different author or payload.
    IdentityMismatch,
    Loro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportFailureCategory {
    IncompatibleEncoding,
    DecodeOrChecksum,
    /// The record imports, but leaves a tree the shared schema does not allow.
    SchemaViolation,
    Other,
}

impl ImportFailureCategory {
    pub(crate) fn from_error(error: &LoroError) -> Self {
        match error {
            LoroError::IncompatibleFutureEncodingError(_) => Self::IncompatibleEncoding,
            LoroError::DecodeVersionVectorError
            | LoroError::DecodeError(_)
            | LoroError::DecodeDataCorruptionError
            | LoroError::DecodeChecksumMismatchError => Self::DecodeOrChecksum,
            _ => Self::Other,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::IncompatibleEncoding => "incompatible_encoding",
            Self::DecodeOrChecksum => "decode_or_checksum",
            Self::SchemaViolation => "schema_violation",
            Self::Other => "other",
        }
    }
}

#[derive(Clone)]
pub(crate) struct EditIdentity {
    pub(crate) author: String,
    pub(crate) digest: [u8; 32],
    pub(crate) revision: u64,
    pub(crate) durable: bool,
    pub(crate) projection_pending: bool,
}

/// The body as the record log alone makes it: what stays visible, read-only, when a later
/// record cannot be imported.
struct AppliedBaseline {
    doc: LoroDoc,
    text: String,
    pending_updates: Vec<Vec<u8>>,
}

impl Clone for AppliedBaseline {
    fn clone(&self) -> Self {
        Self {
            doc: self.doc.fork(),
            text: self.text.clone(),
            pending_updates: self.pending_updates.clone(),
        }
    }
}

impl AppliedBaseline {
    fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
            text: String::new(),
            pending_updates: Vec::new(),
        }
    }

    fn import_candidate(&self, update: &[u8]) -> Result<Self, ImportFailureCategory> {
        let mut candidate = self.clone();
        let pending = import_into(&candidate.doc, &candidate.pending_updates, update)?;
        candidate.pending_updates = pending;
        if candidate.pending_updates.is_empty() {
            markdown::read_tree(&candidate.doc)
                .map_err(|_| ImportFailureCategory::SchemaViolation)?;
        }
        candidate.text = render(&candidate.doc);
        Ok(candidate)
    }
}

pub(crate) struct DocumentBody {
    /// The applied baseline plus the one accepted local update not yet appended, if any.
    doc: LoroDoc,
    text: String,
    applied: AppliedBaseline,
    /// Increments on every change the UI can observe, local or remote.
    revision: u64,
    /// The accepted local update awaiting its append, exactly as it becomes the record payload.
    staged: Option<Vec<u8>>,
    /// Body records known locally whose sealed bytes have not been opened yet.
    unopened: usize,
    /// A sealed deferred edit targeting this body has not yet been opened.
    waiting_deferred: bool,
    /// Loro has received a sealed update whose causal prerequisites are still missing.
    pending_history: bool,
    /// Forks omit Loro's buffered, causally pending updates; replay them into each candidate.
    pending_updates: Vec<Vec<u8>>,
    pending_request_id: Option<String>,
    identities: HashMap<String, EditIdentity>,
    identity_order: VecDeque<String>,
    /// Peers whose operations reached this body in records another Installation wrote. A UI
    /// session never resumes as one of them.
    foreign_peers: HashSet<u64>,
}

impl DocumentBody {
    pub(crate) fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
            text: String::new(),
            applied: AppliedBaseline::new(),
            revision: 0,
            staged: None,
            unopened: 0,
            waiting_deferred: false,
            pending_history: false,
            pending_updates: Vec::new(),
            pending_request_id: None,
            identities: HashMap::new(),
            identity_order: VecDeque::new(),
            foreign_peers: HashSet::new(),
        }
    }

    /// An isolated copy whose documents are deep forks, so a refused candidate changes nothing.
    fn fork(&self) -> Self {
        Self {
            doc: self.doc.fork(),
            text: self.text.clone(),
            applied: self.applied.clone(),
            revision: self.revision,
            staged: self.staged.clone(),
            unopened: self.unopened,
            waiting_deferred: self.waiting_deferred,
            pending_history: self.pending_history,
            pending_updates: self.pending_updates.clone(),
            pending_request_id: self.pending_request_id.clone(),
            identities: self.identities.clone(),
            identity_order: self.identity_order.clone(),
            foreign_peers: self.foreign_peers.clone(),
        }
    }

    /// The Canonical Markdown of the body, including an accepted update not yet appended.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    /// The Canonical Markdown of the body as the record log alone makes it.
    pub(crate) fn applied_text(&self) -> &str {
        &self.applied.text
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    /// The body's current version, a base for [`Self::merge_markdown`].
    pub(crate) fn frontiers(&self) -> Frontiers {
        self.doc.oplog_frontiers()
    }
    /// Every operation the body holds, including an accepted update not yet appended.
    pub(crate) fn version_vector(&self) -> VersionVector {
        self.doc.oplog_vv()
    }
    /// The operations the body gained since `since`, as one update, or `None` when it gained
    /// none. Updates still waiting for causal history are not operations yet.
    pub(crate) fn updates_since(&self, since: &VersionVector) -> Option<Vec<u8>> {
        if self.doc.oplog_vv() == *since {
            return None;
        }
        self.doc.export(ExportMode::updates(since)).ok()
    }
    /// The whole body as one Loro snapshot, the only thing an editor session loads.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }
    /// The body as the record log alone makes it, as one Loro snapshot: what stays visible,
    /// read-only, after an import failure.
    pub(crate) fn applied_snapshot(&self) -> Vec<u8> {
        self.applied
            .doc
            .export(ExportMode::Snapshot)
            .unwrap_or_default()
    }
    /// Remembers the peers of `update`, a record another Installation wrote.
    pub(crate) fn note_foreign_record(&mut self, update: &[u8]) {
        if let Ok(meta) = LoroDoc::decode_import_blob_meta(update, false) {
            self.foreign_peers
                .extend(meta.partial_end_vv.keys().copied());
        }
    }
    /// Whether a UI session that held `peer` before may resume as it: never a peer another
    /// Installation writes with, so no two writers ever share one.
    pub(crate) fn may_resume(&self, peer: u64) -> bool {
        peer != u64::MAX && !self.foreign_peers.contains(&peer)
    }
    pub(crate) fn unopened(&self) -> usize {
        self.unopened
    }
    pub(crate) fn set_unopened(&mut self, count: usize) {
        self.unopened = count;
    }
    pub(crate) fn set_waiting_deferred(&mut self, waiting: bool) -> bool {
        let changed = self.waiting_deferred != waiting;
        self.waiting_deferred = waiting;
        changed
    }
    /// A body is editable once every locally known record has been imported, so an update is
    /// validated against the whole history this Installation holds.
    pub(crate) fn editable(&self) -> bool {
        self.unopened == 0 && !self.waiting_deferred && !self.pending_history
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.staged.is_some()
    }

    pub(crate) fn identity(&self, id: &str) -> Option<&EditIdentity> {
        self.identities.get(id)
    }

    pub(crate) fn set_projection_pending(&mut self, id: &str, pending: bool) {
        if let Some(known) = self.identities.get_mut(id) {
            known.projection_pending = pending;
        }
    }

    pub(crate) fn remember_identity(
        &mut self,
        id: String,
        author: String,
        digest: [u8; 32],
        durable: bool,
    ) -> Result<(), BodyError> {
        if let Some(known) = self.identities.get_mut(&id) {
            if known.author != author || known.digest != digest {
                return Err(BodyError::IdentityMismatch);
            }
            known.durable |= durable;
        } else {
            if self.identity_order.len() == MAX_RECENT_IDENTITIES {
                if self.identity_order.front() == self.pending_request_id.as_ref() {
                    let active = self
                        .identity_order
                        .pop_front()
                        .expect("full cache has an ID");
                    self.identity_order.push_back(active);
                }
                let oldest = self
                    .identity_order
                    .pop_front()
                    .expect("full identity cache has an oldest ID");
                self.identities.remove(&oldest);
            }
            self.identity_order.push_back(id.clone());
            self.identities.insert(
                id,
                EditIdentity {
                    author,
                    digest,
                    revision: self.revision,
                    durable,
                    projection_pending: false,
                },
            );
        }
        Ok(())
    }

    /// A fresh Loro peer for one writer of this body: random, absent from the body's version
    /// vector, and never handed out before in this process.
    pub(crate) fn allocate_peer(&self) -> u64 {
        self.allocate_peer_from(system_random)
    }

    fn allocate_peer_from(&self, draw: impl FnMut() -> u64) -> u64 {
        allocate(&[&self.doc.oplog_vv(), &self.applied.doc.oplog_vv()], draw)
    }

    /// Validates one local update on a fork, then stages it as the next record.
    ///
    /// The update must hold operations only from `session_peer`, fit one record, leave a tree
    /// valid under the shared schema, and respect the growth budget. Only then is it staged
    /// for exactly one `document.edit` append and adopted as the visible body.
    pub(crate) fn accept_local_update(
        &mut self,
        session_peer: u64,
        request_id: String,
        update: &[u8],
    ) -> Result<(), BodyError> {
        if !self.editable() || self.has_pending() {
            return Err(BodyError::NotReady);
        }
        if update.len() > MAX_UPDATE {
            return Err(BodyError::RecordTooLarge);
        }
        let mut candidate = self.fork();
        let before = candidate.doc.oplog_vv();
        let status = candidate
            .doc
            .import(update)
            .map_err(|_| BodyError::InvalidUpdate)?;
        if status.pending.is_some() {
            return Err(BodyError::InvalidUpdate);
        }
        let after = candidate.doc.oplog_vv();
        let mut added = false;
        for (peer, end) in after.iter() {
            if before.get(peer).copied().unwrap_or(0) != *end {
                if *peer != session_peer {
                    return Err(BodyError::ForeignPeer);
                }
                added = true;
            }
        }
        if !added {
            return Err(BodyError::InvalidUpdate);
        }
        let blocks = markdown::read_tree(&candidate.doc).map_err(BodyError::Schema)?;
        let next = content_bytes(&blocks);
        if next > MAX_TEXT_BYTES && next > self.content_bytes() {
            return Err(BodyError::TooLarge);
        }
        let text = render(&candidate.doc);
        if text != candidate.text {
            candidate.revision += 1;
        }
        candidate.text = text;
        candidate.staged = Some(update.to_vec());
        candidate.pending_request_id = Some(request_id);
        *self = candidate;
        Ok(())
    }

    /// Merges a Markdown revision made against `base` as one update from a fresh peer, and
    /// stages it through [`Self::accept_local_update`].
    ///
    /// Returns `false`, with nothing staged, when the revision changes nothing. A merge whose
    /// update does not fit one record is refused, and nothing is staged.
    pub(crate) fn merge_markdown(
        &mut self,
        base: &Frontiers,
        revision: &str,
        request_id: String,
    ) -> Result<bool, BodyError> {
        if !self.editable() || self.has_pending() {
            return Err(BodyError::NotReady);
        }
        let peer = self.allocate_peer();
        let merged =
            markdown::merge(&self.doc, base, revision, peer).map_err(|error| match error {
                MergeError::BaseUnavailable => BodyError::BaseUnavailable,
                MergeError::Schema(violation) => BodyError::Schema(violation),
                MergeError::PeerInUse | MergeError::Loro(_) => BodyError::Loro,
            })?;
        if merged.ops == 0 {
            return Ok(false);
        }
        if merged.update.len() > MAX_UPDATE {
            return Err(BodyError::RecordTooLarge);
        }
        self.accept_local_update(peer, request_id, &merged.update)?;
        Ok(true)
    }

    /// The ID of the one staged identified request, if present.
    pub(crate) fn pending_request_id(&self) -> Option<&str> {
        self.pending_request_id.as_deref()
    }

    /// The staged update to append next, if any.
    pub(crate) fn staged(&self) -> Option<Vec<u8>> {
        self.staged.clone()
    }

    /// Confirm the staged update only after Core accepted its append.
    pub(crate) fn append_succeeded(&mut self) {
        if self.staged.take().is_some() {
            self.pending_request_id = None;
        }
    }

    /// Build an isolated candidate; the caller publishes it only after the sealed row and
    /// its import disposition commit. A failed import leaves the live body untouched.
    ///
    /// The second value says whether the Markdown the UI shows changed. A record whose causal
    /// history is complete must leave a schema-valid tree; one still missing history waits.
    pub(crate) fn import_candidate(
        &self,
        update: &[u8],
    ) -> Result<(Self, bool), ImportFailureCategory> {
        let mut candidate = self.fork();
        candidate.pending_updates =
            import_into(&candidate.doc, &candidate.pending_updates, update)?;
        candidate.pending_history = !candidate.pending_updates.is_empty();
        candidate.applied = self.applied.import_candidate(update)?;
        if !candidate.pending_history {
            markdown::read_tree(&candidate.doc)
                .map_err(|_| ImportFailureCategory::SchemaViolation)?;
        }
        let text = render(&candidate.doc);
        let changed = text != candidate.text;
        if changed {
            candidate.revision += 1;
        }
        candidate.text = text;
        Ok((candidate, changed))
    }

    /// Imports one update from a record, as the record-log paths do after commit.
    #[cfg(test)]
    pub(crate) fn import(&mut self, update: &[u8]) -> Result<bool, ImportFailureCategory> {
        let (candidate, changed) = self.import_candidate(update)?;
        *self = candidate;
        Ok(changed)
    }

    /// What a UI session on this body would submit: the body's Markdown revised to
    /// `revision`, as one update from `peer`, without changing the body.
    #[cfg(test)]
    pub(crate) fn revision_update(&self, peer: u64, revision: &str) -> Vec<u8> {
        markdown::merge(&self.doc, &self.frontiers(), revision, peer)
            .expect("the test revision merges")
            .update
    }

    /// An independent copy that another writer can edit, with nothing staged.
    #[cfg(test)]
    pub(crate) fn writer_copy(&self) -> Self {
        let mut copy = self.fork();
        copy.staged = None;
        copy.pending_request_id = None;
        copy
    }

    fn content_bytes(&self) -> usize {
        markdown::read_tree(&self.doc)
            .map(|blocks| content_bytes(&blocks))
            .unwrap_or(0)
    }
}

/// A body genesis: one update from a freshly allocated peer holding the root map, its
/// attributes, the `children` list and one empty paragraph.
pub(crate) fn genesis() -> Vec<u8> {
    let doc = LoroDoc::new();
    doc.set_peer_id(allocate(&[], system_random))
        .expect("an allocated peer is a valid peer id");
    markdown::write_tree(&doc, &[]).expect("an empty body is a valid tree");
    doc.export(ExportMode::all_updates())
        .expect("a fresh document exports its updates")
}

/// Whether `update` is exactly a body genesis: self-contained, from one peer, and holding only
/// the empty body.
pub(crate) fn is_valid_genesis(update: &[u8]) -> bool {
    let doc = LoroDoc::new();
    let Ok(status) = doc.import(update) else {
        return false;
    };
    status.pending.is_none()
        && doc.oplog_vv().iter().count() == 1
        && markdown::read_tree(&doc).is_ok_and(|blocks| blocks == [Block::Paragraph(Vec::new())])
}

/// Imports `update` into `doc`, replaying the buffered updates a fork does not carry, and
/// returns the updates that still wait for causal history.
fn import_into(
    doc: &LoroDoc,
    buffered: &[Vec<u8>],
    update: &[u8],
) -> Result<Vec<Vec<u8>>, ImportFailureCategory> {
    // Classify the actual import error, not a failed envelope decode or pending status.
    for earlier in buffered {
        doc.import(earlier)
            .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
    }
    let status = doc
        .import(update)
        .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
    let mut pending = Vec::new();
    for earlier in buffered {
        if doc
            .import(earlier)
            .map_err(|cause| ImportFailureCategory::from_error(&cause))?
            .pending
            .is_some()
        {
            pending.push(earlier.clone());
        }
    }
    if status.pending.is_some() {
        pending.push(update.to_vec());
    }
    Ok(pending)
}

/// The Markdown a body shows; a tree that cannot be read yet (no genesis) shows nothing.
fn render(doc: &LoroDoc) -> String {
    markdown::export(doc).unwrap_or_default()
}

/// UTF-8 bytes of text content, the measure of the growth budget.
fn content_bytes(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => content
                .iter()
                .map(|inline| match inline {
                    Inline::Text(text, _) => text.len(),
                    Inline::Break => 1,
                })
                .sum(),
            Block::Blockquote(children) => content_bytes(children),
            Block::Rule => 0,
            Block::Code { text, .. } => text.len(),
            Block::Bullet { items } | Block::Ordered { items, .. } => {
                items.iter().map(|item| content_bytes(item)).sum()
            }
        })
        .sum()
}

fn system_random() -> u64 {
    getrandom::u64().expect("the system random source is available")
}

/// Records `peer` as handed out in this process, as a resumed session does. Returns `false`
/// when this process already handed it out.
pub(crate) fn claim_peer(peer: u64) -> bool {
    ALLOCATED_PEERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(peer)
}

/// Draws until a peer is absent from every `known` version vector and from this process's
/// earlier allocations, then records it. Loro reserves `u64::MAX`.
fn allocate(known: &[&VersionVector], mut draw: impl FnMut() -> u64) -> u64 {
    let mut allocated = ALLOCATED_PEERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    loop {
        let peer = draw();
        if peer == u64::MAX
            || allocated.contains(&peer)
            || known.iter().any(|vv| vv.get(&peer).is_some())
        {
            continue;
        }
        allocated.insert(peer);
        return peer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body that has imported `genesis`, as every Installation's body of one Document does.
    fn body_from(genesis: &[u8]) -> DocumentBody {
        let mut body = DocumentBody::new();
        body.import(genesis).unwrap();
        body
    }

    /// What a UI session submits: a revision of the body's Markdown, as one update from the
    /// session's own peer, made on the body's current version.
    fn session_update(body: &DocumentBody, peer: u64, revision: &str) -> Vec<u8> {
        markdown::merge(&body.doc, &body.frontiers(), revision, peer)
            .unwrap()
            .update
    }

    /// Accepts `revision` from a fresh session peer and returns the record payload it staged,
    /// as the append path would, then confirms the append.
    fn edit(body: &mut DocumentBody, revision: &str) -> Vec<u8> {
        let peer = body.allocate_peer();
        let update = session_update(body, peer, revision);
        body.accept_local_update(peer, request_id(), &update)
            .unwrap();
        let staged = body.staged().unwrap();
        assert_eq!(
            staged, update,
            "the record payload is the session's exact bytes"
        );
        body.append_succeeded();
        body.import(&staged).unwrap();
        staged
    }

    fn request_id() -> String {
        format!("{:032x}", u128::from(system_random()))
    }

    /// A deterministic permutation, so shuffled delivery is reproducible.
    fn shuffled<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
        let mut out = items.to_vec();
        let mut state = seed;
        for index in (1..out.len()).rev() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.swap(index, (state >> 33) as usize % (index + 1));
        }
        out
    }

    fn junk_update(body: &DocumentBody, peer: u64) -> Vec<u8> {
        let fork = body.doc.fork();
        fork.set_peer_id(peer).unwrap();
        let before = fork.oplog_vv();
        let children = match fork.get_map("doc").get("children") {
            Some(loro::ValueOrContainer::Container(loro::Container::List(list))) => list,
            _ => panic!("the genesis has a children list"),
        };
        let node = children.insert_container(0, loro::LoroMap::new()).unwrap();
        node.insert("nodeName", "table").unwrap();
        fork.commit();
        fork.export(ExportMode::updates(&before)).unwrap()
    }

    // Verifies: AC2 — two Installations converge whatever order and delay their records have.
    #[test]
    fn two_bodies_converge_under_shuffled_and_delayed_delivery() {
        let genesis = genesis();
        let mut alice = body_from(&genesis);
        let mut bob = body_from(&genesis);
        let mut from_alice = Vec::new();
        let mut from_bob = Vec::new();
        let mut alice_doc = String::new();
        let mut bob_doc = String::new();
        for round in 0..6 {
            alice_doc.push_str(&format!("# Alice {round}\n\nalice says *{round}*\n\n"));
            from_alice.push(edit(&mut alice, &alice_doc));
            bob_doc.push_str(&format!("- bob item **{round}**\n"));
            from_bob.push(edit(&mut bob, &bob_doc));
            // Bob sees only some of Alice's records before editing again, and late.
            if round % 2 == 1 {
                for update in shuffled(&from_alice, round) {
                    bob.import(&update).unwrap();
                }
                bob_doc = bob.text().to_owned();
            }
        }
        for update in shuffled(&from_bob, 7) {
            alice.import(&update).unwrap();
        }
        for update in shuffled(&from_alice, 11).iter().rev() {
            bob.import(update).unwrap();
        }
        assert_eq!(alice.text(), bob.text());
        assert!(alice.editable() && bob.editable());
        let mut reader = body_from(&genesis);
        let mut everything = from_alice.clone();
        everything.extend(from_bob.iter().cloned());
        for update in shuffled(&everything, 3).iter().rev() {
            reader.import(update).unwrap();
        }
        assert_eq!(reader.text(), alice.text());
        assert!(!reader.pending_history);
        assert!(alice.text().contains("Alice 5") && alice.text().contains("bob item"));
    }

    // Verifies: AC2 — S7 cannot recur: both Installations start from one genesis and keep
    // their concurrent text.
    #[test]
    fn concurrent_first_edits_after_one_genesis_keep_both_sides_text() {
        let genesis = genesis();
        assert!(is_valid_genesis(&genesis));
        let mut alice = body_from(&genesis);
        let mut bob = body_from(&genesis);
        assert_eq!(alice.text(), "");
        let a = edit(&mut alice, "alice wrote this\n");
        let b = edit(&mut bob, "bob wrote that\n");
        alice.import(&b).unwrap();
        bob.import(&a).unwrap();
        assert_eq!(alice.text(), bob.text());
        assert!(alice.text().contains("alice wrote this"));
        assert!(alice.text().contains("bob wrote that"));
        // A second genesis, written independently, is a different body: it is never imported
        // into this one, because each Document has exactly one create record.
        assert_ne!(super::genesis(), genesis);
    }

    #[test]
    fn a_refused_update_stages_nothing_and_changes_nothing() {
        let genesis = genesis();
        let mut body = body_from(&genesis);
        edit(&mut body, "keep me\n");
        let before = (body.text().to_owned(), body.revision());
        let assert_unchanged = |body: &DocumentBody| {
            assert_eq!((body.text().to_owned(), body.revision()), before);
            assert!(!body.has_pending());
            assert!(body.pending_request_id().is_none());
        };

        // A foreign peer's ops.
        let session = body.allocate_peer();
        let other = body.allocate_peer();
        let foreign = session_update(&body, other, "keep me, changed\n");
        assert_eq!(
            body.accept_local_update(session, request_id(), &foreign),
            Err(BodyError::ForeignPeer)
        );
        assert_unchanged(&body);

        // An update over the record bound. Large text arrives as new blocks, as a paste does.
        let peer = body.allocate_peer();
        let huge = session_update(
            &body,
            peer,
            &format!("keep me\n\n{}\n", unique_text(170 * 1024)),
        );
        assert!(huge.len() > MAX_UPDATE, "update is {} bytes", huge.len());
        assert_eq!(
            body.accept_local_update(peer, request_id(), &huge),
            Err(BodyError::RecordTooLarge)
        );
        assert_unchanged(&body);
        assert_eq!(
            body.merge_markdown(
                &body.frontiers(),
                &format!("keep me\n\n{}\n", unique_text(170 * 1024)),
                request_id()
            ),
            Err(BodyError::RecordTooLarge)
        );
        assert_unchanged(&body);

        // A schema-invalid tree.
        let peer = body.allocate_peer();
        let junk = junk_update(&body, peer);
        assert_eq!(
            body.accept_local_update(peer, request_id(), &junk),
            Err(BodyError::Schema(SchemaViolation::UnknownNode(
                "table".into()
            )))
        );
        assert_unchanged(&body);

        // Growth over budget.
        let peer = body.allocate_peer();
        let first = session_update(
            &body,
            peer,
            &format!("keep me\n\n{}\n", unique_text(150 * 1024)),
        );
        body.accept_local_update(peer, request_id(), &first)
            .unwrap();
        body.append_succeeded();
        let grown = (body.text().to_owned(), body.revision());
        let peer = body.allocate_peer();
        let more = session_update(
            &body,
            peer,
            &format!("{}\n\n{}\n", body.text().trim_end(), unique_text(50 * 1024)),
        );
        assert!(more.len() <= MAX_UPDATE);
        assert_eq!(
            body.accept_local_update(peer, request_id(), &more),
            Err(BodyError::TooLarge)
        );
        assert_eq!((body.text().to_owned(), body.revision()), grown);
        assert!(!body.has_pending());

        // Garbage and empty updates are refused too.
        assert_eq!(
            body.accept_local_update(peer, request_id(), b"not loro"),
            Err(BodyError::InvalidUpdate)
        );
    }

    /// Text that Loro cannot shrink much: every line differs.
    fn unique_text(bytes: usize) -> String {
        let mut text = String::new();
        let mut index = 0u64;
        while text.len() < bytes {
            text.push_str(&format!(
                "{:016x} ",
                index.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ));
            index += 1;
        }
        text.truncate(bytes);
        text.trim_end().to_owned()
    }

    #[test]
    fn concurrent_overflow_converges_and_only_non_growing_edits_are_allowed() {
        let genesis = genesis();
        let mut alice = body_from(&genesis);
        let mut bob = body_from(&genesis);
        let base = edit(&mut alice, "first\n\nend\n");
        bob.import(&base).unwrap();
        let a = edit(
            &mut alice,
            &format!("first\n\n{}\n\nend\n", unique_text(110 * 1024)),
        );
        let b = edit(
            &mut bob,
            &format!("first\n\n{}\n\nend\n", "b".repeat(110 * 1024)),
        );
        alice.import(&b).unwrap();
        bob.import(&a).unwrap();
        assert_eq!(alice.text(), bob.text());
        assert!(alice.content_bytes() > MAX_TEXT_BYTES);
        let merged = alice.text().to_owned();
        let peer = alice.allocate_peer();
        let grow = session_update(&alice, peer, &merged.replace("\n\nend\n", "\n\nend!\n"));
        assert_eq!(
            alice.accept_local_update(peer, request_id(), &grow),
            Err(BodyError::TooLarge)
        );
        let peer = alice.allocate_peer();
        let shrink = session_update(&alice, peer, &merged.replace("\n\nend\n", "\n\nen\n"));
        alice
            .accept_local_update(peer, request_id(), &shrink)
            .expect("a shrinking edit stays allowed while over budget");
    }

    #[test]
    fn merge_markdown_stages_one_update_and_an_unchanged_export_changes_nothing() {
        let genesis = genesis();
        let mut body = body_from(&genesis);
        edit(&mut body, "# Plan\n\nfirst\n");
        let base = body.frontiers();
        let unchanged = body.text().to_owned();
        assert_eq!(
            body.merge_markdown(&base, &unchanged, request_id()),
            Ok(false)
        );
        assert!(!body.has_pending());
        assert!(
            body.merge_markdown(&base, "# Plan\n\nsecond\n", request_id())
                .unwrap()
        );
        assert!(body.has_pending());
        assert_eq!(body.text(), "# Plan\n\nsecond\n");
        assert_eq!(
            body.merge_markdown(&base, "again\n", request_id()),
            Err(BodyError::NotReady),
            "one staged request at a time"
        );
        let staged = body.staged().unwrap();
        body.append_succeeded();
        let mut reader = body_from(&genesis);
        reader.import(&staged).unwrap();
        assert!(reader.pending_history, "the first edit is still missing");
        let unknown: Frontiers = loro::ID::new(7, 3).into();
        assert_eq!(
            body.merge_markdown(&unknown, "x\n", request_id()),
            Err(BodyError::BaseUnavailable)
        );
    }

    /// A later record that arrives before its predecessor is held and applied once the
    /// predecessor arrives; the text never shows a partial state.
    #[test]
    fn a_dependency_missing_update_waits_for_its_predecessor() {
        let genesis = genesis();
        let mut writer = body_from(&genesis);
        let first = edit(&mut writer, "one\n");
        let second = edit(&mut writer, "one two\n");
        let mut reader = body_from(&genesis);
        assert!(!reader.import(&second).unwrap());
        assert!(!reader.editable(), "missing history keeps the body pending");
        assert_eq!(reader.text(), "");
        assert_eq!(reader.applied_text(), "");
        assert!(reader.import(&first).unwrap());
        assert_eq!(reader.text(), "one two\n");
        assert_eq!(reader.applied_text(), "one two\n");
        assert!(reader.editable());
        assert_eq!(reader.revision(), 1);
    }

    #[test]
    fn an_edit_before_its_genesis_waits_instead_of_failing_validation() {
        let genesis = genesis();
        let mut writer = body_from(&genesis);
        let update = edit(&mut writer, "after genesis\n");
        let mut reader = DocumentBody::new();
        assert!(!reader.import(&update).unwrap());
        assert!(!reader.editable());
        assert!(reader.import(&genesis).unwrap());
        assert_eq!(reader.text(), "after genesis\n");
    }

    #[test]
    fn a_schema_invalid_or_undecodable_record_is_an_import_failure() {
        let genesis = genesis();
        let mut body = body_from(&genesis);
        edit(&mut body, "keep me\n");
        let junk = junk_update(&body, 4242);
        assert!(matches!(
            body.import_candidate(&junk),
            Err(ImportFailureCategory::SchemaViolation)
        ));
        assert_eq!(
            ImportFailureCategory::SchemaViolation.as_str(),
            "schema_violation"
        );
        assert!(matches!(
            body.import_candidate(b"not loro"),
            Err(ImportFailureCategory::DecodeOrChecksum)
        ));
        assert_eq!(body.text(), "keep me\n");
        assert_eq!(body.applied_text(), "keep me\n");
    }

    #[test]
    fn a_failed_append_keeps_the_same_bytes_for_retry() {
        let genesis = genesis();
        let mut body = body_from(&genesis);
        let peer = body.allocate_peer();
        let update = session_update(&body, peer, "durable\n");
        body.accept_local_update(peer, request_id(), &update)
            .unwrap();
        assert_eq!(body.staged(), Some(update.clone()));
        assert_eq!(
            body.staged(),
            Some(update.clone()),
            "a failure must not consume it"
        );
        assert_eq!(body.text(), "durable\n");
        assert_eq!(
            body.applied_text(),
            "",
            "only records make the applied baseline"
        );
        body.append_succeeded();
        assert!(!body.has_pending());
        let mut restarted = body_from(&genesis);
        restarted.import(&update).unwrap();
        assert_eq!(restarted.text(), "durable\n");
    }

    // Verifies: AC4 — peers never collide with the history or with each other.
    #[test]
    fn allocated_peers_avoid_the_version_vector_and_earlier_allocations() {
        let genesis = genesis();
        let mut body = body_from(&genesis);
        edit(&mut body, "one\n");
        edit(&mut body, "two\n");
        let known = body.doc.oplog_vv();
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            let peer = body.allocate_peer();
            assert!(known.get(&peer).is_none());
            assert!(seen.insert(peer), "peer {peer} was handed out twice");
        }
        // A draw that repeats known peers is skipped until a fresh one comes up.
        let history: Vec<u64> = known.keys().copied().collect();
        let earlier = *seen.iter().next().unwrap();
        let mut draws = history
            .iter()
            .copied()
            .chain([earlier, u64::MAX, earlier])
            .chain(std::iter::once(0x5eed));
        let fresh = body.allocate_peer_from(|| draws.next().unwrap_or_else(system_random));
        assert!(!history.contains(&fresh) && fresh != earlier && fresh != u64::MAX);
    }

    #[test]
    fn recent_identity_cache_is_bounded_and_pins_an_unacknowledged_request() {
        let mut body = DocumentBody::new();
        body.pending_request_id = Some("active".into());
        body.remember_identity("active".into(), "alice".into(), [1; 32], false)
            .unwrap();
        for index in 0..(MAX_RECENT_IDENTITIES * 3) {
            body.remember_identity(index.to_string(), "alice".into(), [2; 32], true)
                .unwrap();
            assert!(body.identities.len() <= MAX_RECENT_IDENTITIES);
            assert!(body.identity("active").is_some());
        }
        assert!(body.identity("0").is_none());
        assert_eq!(body.identity_order.len(), body.identities.len());
        assert_eq!(
            body.remember_identity("active".into(), "bob".into(), [1; 32], true),
            Err(BodyError::IdentityMismatch)
        );
    }
}
