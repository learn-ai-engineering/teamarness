//! The in-memory body of one Document: a Loro text container, the text it currently renders,
//! and the bookkeeping that turns local edits into bounded records.
//!
//! Nothing here is persisted. A body is rebuilt at activation by importing every opened
//! `document.edit` record; the Package SQLite never holds plaintext.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use loro::{ExportMode, LoroDoc, LoroError, VersionVector};
use serde::{Deserialize, Serialize};

use crate::domain::MAX_UPDATE;

/// The text container every Document body uses.
const CONTAINER: &str = "body";
/// Local edits are appended once no further edit arrives within this window.
pub(crate) const COALESCE_WINDOW: Duration = Duration::from_millis(300);
/// Pending local edits are appended early once their export reaches this size.
pub(crate) const EXPORT_BUDGET: usize = 64 * 1024;
/// Local growth budget; remote CRDT merges can exceed it and are delivered in bounded chunks.
pub(crate) const MAX_TEXT_BYTES: usize = 192 * 1024;
/// A single inserted run larger than this is applied in pieces with an export between them,
/// so no one record approaches the canonical bound.
const INSERT_CHUNK: usize = 48 * 1024;
/// Per-operation encoding overhead assumed when estimating a pending export's size.
const OP_OVERHEAD: usize = 32;
/// Only recent IDs stay in the plaintext in-memory projection; old IDs are opened lazily.
pub(crate) const MAX_RECENT_IDENTITIES: usize = 256;

/// One operation the UI sends, in Unicode scalar positions against a base revision.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum TextOp {
    Retain { retain: usize },
    Insert { insert: String },
    Delete { delete: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BodyError {
    /// The ops run past the end of the text.
    OutOfRange,
    /// The ops would grow a body beyond [`MAX_TEXT_BYTES`], or grow one already above it.
    TooLarge,
    RecordTooLarge,
    Loro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportFailureCategory {
    IncompatibleEncoding,
    DecodeOrChecksum,
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
            Self::Other => "other",
        }
    }
}

/// Why a flush produced bytes: the caller appends each returned update as one record.
pub(crate) struct Flushed {
    pub(crate) updates: Vec<Vec<u8>>,
}

#[derive(Clone)]
pub(crate) struct EditIdentity {
    pub(crate) author: String,
    pub(crate) digest: [u8; 32],
    pub(crate) revision: u64,
    pub(crate) durable: bool,
}

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
        for buffered in &candidate.pending_updates {
            candidate
                .doc
                .import(buffered)
                .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
        }
        let status = candidate
            .doc
            .import(update)
            .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
        let mut pending = Vec::new();
        for buffered in std::mem::take(&mut candidate.pending_updates) {
            if candidate
                .doc
                .import(&buffered)
                .map_err(|cause| ImportFailureCategory::from_error(&cause))?
                .pending
                .is_some()
            {
                pending.push(buffered);
            }
        }
        if status.pending.is_some() {
            pending.push(update.to_vec());
        }
        candidate.pending_updates = pending;
        candidate.text = candidate.doc.get_text(CONTAINER).to_string();
        Ok(candidate)
    }
}

pub(crate) struct DocumentBody {
    doc: LoroDoc,
    text: String,
    applied: AppliedBaseline,
    /// Increments on every change the UI can observe, local or remote.
    revision: u64,
    /// The version already appended as records; exports start here.
    appended: VersionVector,
    /// Version exported into the ordered queue; only successful appends advance `appended`.
    staged_version: VersionVector,
    /// When the oldest unexported local edit was made, if any.
    pending_since: Option<Instant>,
    /// Exports awaiting append, paired with the version each export reaches.
    staged: VecDeque<(Vec<u8>, VersionVector)>,
    /// Edit records known locally whose sealed bytes have not been opened yet.
    unopened: usize,
    /// A sealed deferred edit targeting this body has not yet been opened.
    waiting_deferred: bool,
    /// Loro has received a sealed update whose causal prerequisites are still missing.
    pending_history: bool,
    /// Forks omit Loro's buffered, causally pending updates; replay them into each candidate.
    pending_updates: Vec<Vec<u8>>,
    /// A conservative estimate of the pending export size, so a keystroke never exports.
    pending_estimate: usize,
    /// Last request identity allows retry after a failed append without reapplying its ops.
    last_request: Option<(u64, Vec<TextOp>, u64)>,
    pending_request_id: Option<String>,
    identities: HashMap<String, EditIdentity>,
    identity_order: VecDeque<String>,
}

impl DocumentBody {
    pub(crate) fn new(peer: u64) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)
            .expect("a fresh document accepts its peer id");
        Self {
            doc,
            text: String::new(),
            applied: AppliedBaseline::new(),
            revision: 0,
            appended: VersionVector::default(),
            staged_version: VersionVector::default(),
            pending_since: None,
            staged: VecDeque::new(),
            unopened: 0,
            waiting_deferred: false,
            pending_history: false,
            pending_updates: Vec::new(),
            pending_estimate: 0,
            last_request: None,
            pending_request_id: None,
            identities: HashMap::new(),
            identity_order: VecDeque::new(),
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    pub(crate) fn applied_text(&self) -> &str {
        &self.applied.text
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
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
    /// A body is editable once every locally known edit has been imported, so this peer's
    /// own earlier operations are present before it creates new ones.
    pub(crate) fn editable(&self) -> bool {
        self.unopened == 0 && !self.waiting_deferred && !self.pending_history
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pending_since.is_some() || !self.staged.is_empty()
    }

    pub(crate) fn last_request_revision(&self) -> Option<u64> {
        self.last_request.as_ref().map(|(_, _, revision)| *revision)
    }

    pub(crate) fn remember_request(&mut self, base: u64, ops: &[TextOp]) {
        self.last_request = Some((base, ops.to_vec(), self.revision));
    }

    pub(crate) fn identity(&self, id: &str) -> Option<&EditIdentity> {
        self.identities.get(id)
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
                return Err(BodyError::Loro);
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
                },
            );
        }
        Ok(())
    }

    /// Preflight on a deep fork before changing live Loro: one identified request
    /// must fit one canonical record, not a partially committed group of exports.
    pub(crate) fn apply_identified(
        &mut self,
        ops: &[TextOp],
        request_id: String,
    ) -> Result<Vec<u8>, BodyError> {
        if self.has_pending() {
            return Err(BodyError::Loro);
        }
        let mut candidate = Self {
            doc: self.doc.fork(),
            text: self.text.clone(),
            applied: self.applied.clone(),
            revision: self.revision,
            appended: self.appended.clone(),
            staged_version: self.staged_version.clone(),
            pending_since: None,
            staged: VecDeque::new(),
            unopened: self.unopened,
            waiting_deferred: self.waiting_deferred,
            pending_history: self.pending_history,
            pending_updates: self.pending_updates.clone(),
            pending_estimate: 0,
            last_request: self.last_request.clone(),
            pending_request_id: None,
            identities: self.identities.clone(),
            identity_order: self.identity_order.clone(),
        };
        candidate
            .doc
            .set_peer_id(self.doc.peer_id())
            .map_err(|_| BodyError::Loro)?;
        candidate.apply(ops)?;
        let updates = candidate
            .flush(Instant::now(), true)
            .ok_or(BodyError::Loro)?
            .updates;
        if updates.len() != 1 || updates[0].len() > MAX_UPDATE {
            return Err(BodyError::RecordTooLarge);
        }
        candidate.pending_request_id = Some(request_id);
        let update = updates[0].clone();
        *self = candidate;
        Ok(update)
    }

    /// The ID of the one staged identified request, if present.
    pub(crate) fn pending_request_id(&self) -> Option<&str> {
        self.pending_request_id.as_deref()
    }

    /// Applies UI operations as local edits and returns the delta the UI should see echoed.
    pub(crate) fn apply(&mut self, ops: &[TextOp]) -> Result<Vec<TextOp>, BodyError> {
        let mut inserted = 0usize;
        let mut deleted_bytes = 0usize;
        let mut cursor = 0usize;
        let chars: Vec<char> = self.text.chars().collect();
        let length = chars.len();
        for op in ops {
            match op {
                TextOp::Retain { retain } => {
                    cursor = cursor.checked_add(*retain).ok_or(BodyError::OutOfRange)?
                }
                TextOp::Insert { insert } => inserted += insert.len(),
                TextOp::Delete { delete } => {
                    let end = cursor.checked_add(*delete).ok_or(BodyError::OutOfRange)?;
                    if end > length {
                        return Err(BodyError::OutOfRange);
                    }
                    deleted_bytes += chars[cursor..end]
                        .iter()
                        .map(|ch| ch.len_utf8())
                        .sum::<usize>();
                    cursor = end;
                }
            }
            if cursor > length {
                return Err(BodyError::OutOfRange);
            }
        }
        let next_bytes = self
            .text
            .len()
            .saturating_sub(deleted_bytes)
            .saturating_add(inserted);
        if next_bytes > MAX_TEXT_BYTES && next_bytes > self.text.len() {
            return Err(BodyError::TooLarge);
        }
        let container = self.doc.get_text(CONTAINER);
        let mut position = 0usize;
        for op in ops {
            match op {
                TextOp::Retain { retain } => position += retain,
                TextOp::Insert { insert } => {
                    let mut offset = 0;
                    while offset < insert.len() {
                        let end = chunk_end(insert, offset);
                        container
                            .insert(position, &insert[offset..end])
                            .map_err(|_| BodyError::Loro)?;
                        position += insert[offset..end].chars().count();
                        self.pending_estimate += end - offset + OP_OVERHEAD;
                        offset = end;
                        if end < insert.len() {
                            self.stage_if_large();
                        }
                    }
                }
                TextOp::Delete { delete } => {
                    container
                        .delete(position, *delete)
                        .map_err(|_| BodyError::Loro)?;
                    self.pending_estimate += OP_OVERHEAD;
                }
            }
        }
        self.doc.commit();
        self.text = container.to_string();
        self.revision += 1;
        if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        self.stage_if_large();
        Ok(ops.to_vec())
    }

    /// Build an isolated candidate; the caller publishes it only after the sealed row and
    /// its import disposition commit. A failed import leaves the live body untouched.
    pub(crate) fn import_candidate(
        &self,
        update: &[u8],
    ) -> Result<(Self, Option<Vec<TextOp>>), ImportFailureCategory> {
        let mut candidate = Self {
            doc: self.doc.fork(),
            text: self.text.clone(),
            applied: self.applied.clone(),
            revision: self.revision,
            appended: self.appended.clone(),
            staged_version: self.staged_version.clone(),
            pending_since: self.pending_since,
            staged: self.staged.clone(),
            unopened: self.unopened,
            waiting_deferred: self.waiting_deferred,
            pending_history: self.pending_history,
            pending_updates: self.pending_updates.clone(),
            pending_estimate: self.pending_estimate,
            last_request: self.last_request.clone(),
            pending_request_id: self.pending_request_id.clone(),
            identities: self.identities.clone(),
            identity_order: self.identity_order.clone(),
        };
        candidate
            .doc
            .set_peer_id(self.doc.peer_id())
            .map_err(|_| ImportFailureCategory::Other)?;
        // Classify the actual import error, not a failed envelope decode or pending status.
        let before = candidate.text.clone();
        for buffered in &candidate.pending_updates {
            candidate
                .doc
                .import(buffered)
                .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
        }
        let status = candidate
            .doc
            .import(update)
            .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
        let mut still_pending = Vec::new();
        for buffered in std::mem::take(&mut candidate.pending_updates) {
            let result = candidate
                .doc
                .import(&buffered)
                .map_err(|cause| ImportFailureCategory::from_error(&cause))?;
            if result.pending.is_some() {
                still_pending.push(buffered);
            }
        }
        candidate.pending_updates = still_pending;
        if status.pending.is_some() {
            candidate.pending_updates.push(update.to_vec());
        }
        candidate.pending_history = !candidate.pending_updates.is_empty();
        candidate.applied = self.applied.import_candidate(update)?;
        let after = candidate.doc.get_text(CONTAINER).to_string();
        candidate.appended = merge_remote(
            &candidate.appended,
            &candidate.doc.oplog_vv(),
            candidate.doc.peer_id(),
        );
        candidate.staged_version = merge_remote(
            &candidate.staged_version,
            &candidate.doc.oplog_vv(),
            candidate.doc.peer_id(),
        );
        let delta = diff(&before, &after);
        candidate.text = after;
        if !delta.is_empty() {
            candidate.revision += 1;
        }
        Ok((candidate, (!delta.is_empty()).then_some(delta)))
    }

    #[cfg(test)]
    pub(crate) fn preview_import(&self, update: &[u8]) -> Result<(), ImportFailureCategory> {
        self.import_candidate(update).map(|_| ())
    }

    /// Imports one update from a record and returns the delta the UI should apply, or `None`
    /// when the text did not change (already known, or its dependencies are still pending).
    #[cfg(test)]
    pub(crate) fn import(&mut self, update: &[u8]) -> Result<Option<Vec<TextOp>>, BodyError> {
        let (candidate, delta) = self.import_candidate(update).map_err(|_| BodyError::Loro)?;
        *self = candidate;
        Ok(delta)
    }

    /// Takes the local edits that are due as one or more update payloads, oldest first.
    pub(crate) fn flush(&mut self, now: Instant, force: bool) -> Option<Flushed> {
        let due = match self.pending_since {
            Some(since) => force || now.duration_since(since) >= COALESCE_WINDOW,
            None => false,
        };
        if due {
            self.stage_pending();
        }
        if self.staged.is_empty() {
            return None;
        }
        Some(Flushed {
            updates: self.staged.iter().map(|(bytes, _)| bytes.clone()).collect(),
        })
    }

    /// Confirm the oldest export only after Core accepted its append.
    pub(crate) fn append_succeeded(&mut self) {
        if let Some((_, version)) = self.staged.pop_front() {
            self.appended = merge_remote(&version, &self.doc.oplog_vv(), self.doc.peer_id());
            self.pending_request_id = None;
        }
    }

    fn stage_if_large(&mut self) {
        if self.pending_estimate >= EXPORT_BUDGET {
            self.stage_pending();
        }
    }

    fn stage_pending(&mut self) {
        let update = self
            .doc
            .export(ExportMode::updates(&self.staged_version))
            .unwrap_or_default();
        self.pending_since = None;
        self.pending_estimate = 0;
        if update.is_empty() {
            return;
        }
        self.staged_version = self.doc.oplog_vv();
        self.staged.push_back((update, self.staged_version.clone()));
    }
}

/// The version `appended` plus every peer other than `local` from `known`, so an export from
/// it carries only this peer's unappended operations.
fn merge_remote(appended: &VersionVector, known: &VersionVector, local: u64) -> VersionVector {
    let mut merged = known.clone();
    let local_counter = appended.get(&local).copied().unwrap_or(0);
    merged.set_end(loro::ID::new(local, local_counter));
    merged
}

/// The end of a chunk of at most [`INSERT_CHUNK`] bytes starting at `from`, on a char boundary.
fn chunk_end(text: &str, from: usize) -> usize {
    let mut end = (from + INSERT_CHUNK).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// A minimal common-prefix/suffix delta from `before` to `after`, in Unicode scalar positions.
pub(crate) fn diff(before: &str, after: &str) -> Vec<TextOp> {
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    let mut prefix = 0;
    while prefix < before_chars.len()
        && prefix < after_chars.len()
        && before_chars[prefix] == after_chars[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before_chars.len() - prefix
        && suffix < after_chars.len() - prefix
        && before_chars[before_chars.len() - 1 - suffix]
            == after_chars[after_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let deleted = before_chars.len() - prefix - suffix;
    let inserted: String = after_chars[prefix..after_chars.len() - suffix]
        .iter()
        .collect();
    let mut ops = Vec::new();
    if deleted == 0 && inserted.is_empty() {
        return ops;
    }
    if prefix > 0 {
        ops.push(TextOp::Retain { retain: prefix });
    }
    if deleted > 0 {
        ops.push(TextOp::Delete { delete: deleted });
    }
    if !inserted.is_empty() {
        ops.push(TextOp::Insert { insert: inserted });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_text(body: &mut DocumentBody, before_len: usize, text: &str) {
        let mut ops = Vec::new();
        if before_len > 0 {
            ops.push(TextOp::Retain { retain: before_len });
        }
        ops.push(TextOp::Insert {
            insert: text.into(),
        });
        body.apply(&ops).unwrap();
    }

    fn take_all(body: &mut DocumentBody) -> Vec<Vec<u8>> {
        let updates = body
            .flush(Instant::now(), true)
            .map(|f| f.updates)
            .unwrap_or_default();
        for _ in &updates {
            body.append_succeeded();
        }
        updates
    }

    #[test]
    fn identified_request_stages_exactly_one_bounded_update_without_partial_growth() {
        let mut body = DocumentBody::new(1);
        let delta = [TextOp::Insert {
            insert: "hello".into(),
        }];
        let update = body.apply_identified(&delta, "a".repeat(32)).unwrap();
        assert!(!update.is_empty());
        assert_eq!(body.text(), "hello");
        assert_eq!(
            body.flush(Instant::now(), true).unwrap().updates,
            vec![update]
        );
        body.append_succeeded();
        assert_eq!(body.text(), "hello");
        assert!(body.flush(Instant::now(), true).is_none());
        assert_eq!(
            body.apply_identified(
                &[TextOp::Insert {
                    insert: "x".repeat(MAX_TEXT_BYTES)
                }],
                "b".repeat(32)
            ),
            Err(BodyError::TooLarge)
        );
        assert_eq!(body.text(), "hello");
    }

    #[test]
    fn concurrent_overflow_converges_and_only_shrinking_edits_are_allowed() {
        let mut alice = DocumentBody::new(1);
        let mut bob = DocumentBody::new(2);
        apply_text(&mut alice, 0, &"a".repeat(110 * 1024));
        apply_text(&mut bob, 0, &"b".repeat(110 * 1024));
        let alice_updates = take_all(&mut alice);
        let bob_updates = take_all(&mut bob);
        for update in &bob_updates {
            alice.import(update).unwrap();
        }
        for update in &alice_updates {
            bob.import(update).unwrap();
        }
        assert_eq!(alice.text(), bob.text());
        assert!(alice.text().len() > MAX_TEXT_BYTES);
        assert_eq!(
            alice.apply(&[TextOp::Insert { insert: "!".into() }]),
            Err(BodyError::TooLarge)
        );
        assert!(
            alice
                .apply(&[
                    TextOp::Delete { delete: 1 },
                    TextOp::Insert { insert: "!".into() }
                ])
                .is_ok(),
            "no-growth replacement stays editable"
        );
        assert_eq!(
            alice.apply(&[TextOp::Delete { delete: 1 }]),
            Ok(vec![TextOp::Delete { delete: 1 }])
        );
        assert_eq!(
            alice.apply(&[TextOp::Insert { insert: "!".into() }]),
            Err(BodyError::TooLarge)
        );
    }

    // Verifies: AC3 — two peers converge whatever order their records arrive in.
    #[test]
    fn two_peers_converge_in_any_import_order() {
        let mut alice = DocumentBody::new(1);
        let mut bob = DocumentBody::new(2);
        apply_text(&mut alice, 0, "# Plan\n");
        let a1 = take_all(&mut alice);
        for u in &a1 {
            bob.import(u).unwrap();
        }
        apply_text(&mut bob, 7, "- bob\n");
        apply_text(&mut alice, 7, "- alice\n");
        let b1 = take_all(&mut bob);
        let a2 = take_all(&mut alice);
        let mut everything: Vec<Vec<u8>> = Vec::new();
        everything.extend(a1.iter().cloned());
        everything.extend(b1.iter().cloned());
        everything.extend(a2.iter().cloned());
        let forward = {
            let mut body = DocumentBody::new(3);
            for u in &everything {
                body.import(u).unwrap();
            }
            body.text().to_owned()
        };
        let reversed = {
            let mut body = DocumentBody::new(4);
            for u in everything.iter().rev() {
                body.import(u).unwrap();
            }
            body.text().to_owned()
        };
        for u in &b1 {
            alice.import(u).unwrap();
        }
        for u in &a2 {
            bob.import(u).unwrap();
        }
        assert_eq!(alice.text(), bob.text());
        assert_eq!(forward, alice.text());
        assert_eq!(
            reversed,
            alice.text(),
            "an update whose dependency arrives later still converges"
        );
        assert!(alice.text().contains("- bob\n") && alice.text().contains("- alice\n"));
    }

    /// A later record from one peer that arrives before its predecessor is held and applied
    /// once the predecessor arrives; the text never shows a partial state.
    #[test]
    fn a_dependency_missing_update_waits_for_its_predecessor() {
        let mut alice = DocumentBody::new(1);
        apply_text(&mut alice, 0, "one ");
        let first = take_all(&mut alice);
        apply_text(&mut alice, 4, "two");
        let second = take_all(&mut alice);
        let mut reader = DocumentBody::new(9);
        assert_eq!(reader.import(&second[0]).unwrap(), None);
        assert_eq!(reader.text(), "");
        assert_eq!(reader.applied_text(), "");
        assert!(reader.import(&first[0]).unwrap().is_some());
        assert_eq!(reader.text(), "one two");
        assert_eq!(reader.applied_text(), "one two");
        assert_eq!(reader.revision(), 1);
    }

    /// Exports carry only this peer's unappended operations, never remote ones it imported.
    #[test]
    fn exports_exclude_imported_remote_operations() {
        let mut alice = DocumentBody::new(1);
        let mut bob = DocumentBody::new(2);
        apply_text(&mut alice, 0, "hello");
        let a = take_all(&mut alice);
        bob.import(&a[0]).unwrap();
        apply_text(&mut bob, 5, "!");
        let b = take_all(&mut bob);
        assert_eq!(b.len(), 1);
        assert!(
            b[0].len() < a[0].len() + 40,
            "bob's export is only his own op"
        );
        let mut reader = DocumentBody::new(3);
        assert_eq!(
            reader.import(&b[0]).unwrap(),
            None,
            "bob's op depends on alice's"
        );
        reader.import(&a[0]).unwrap();
        assert_eq!(reader.text(), "hello!");
        assert!(take_all(&mut bob).is_empty(), "nothing left to export");
    }

    #[test]
    fn coalescing_waits_for_the_window_and_the_budget_forces_early_staging() {
        let mut body = DocumentBody::new(1);
        apply_text(&mut body, 0, "a");
        let now = Instant::now();
        assert!(
            body.flush(now, false).is_none(),
            "within the window nothing is due"
        );
        let first = body
            .flush(now + COALESCE_WINDOW, false)
            .expect("due after the window");
        body.append_succeeded();
        assert!(body.flush(now + COALESCE_WINDOW * 2, false).is_none());
        // A large paste is split and each piece stays under the record bound.
        let paste = "x".repeat(150 * 1024);
        apply_text(&mut body, 1, &paste);
        let mut updates = first.updates;
        updates.extend(take_all(&mut body));
        assert!(
            updates.len() >= 3,
            "a large insert yields several records, got {}",
            updates.len()
        );
        assert!(updates.iter().all(|u| u.len() <= MAX_UPDATE));
        let mut reader = DocumentBody::new(2);
        for u in &updates {
            reader.import(u).unwrap();
        }
        assert_eq!(reader.text().len(), 1 + paste.len());
    }

    #[test]
    fn failed_append_retries_the_same_bytes_and_durable_replay_survives_restart() {
        let mut writer = DocumentBody::new(1);
        apply_text(&mut writer, 0, "durable");
        let first = writer.flush(Instant::now(), true).unwrap().updates;
        let retry = writer.flush(Instant::now(), true).unwrap().updates;
        assert_eq!(
            first, retry,
            "an append failure must not consume the export"
        );
        writer.append_succeeded();
        assert!(!writer.has_pending());
        let mut restarted = DocumentBody::new(1);
        restarted.import(&first[0]).unwrap();
        assert_eq!(restarted.text(), "durable");
    }

    #[test]
    fn malformed_import_preserves_nonempty_cached_text() {
        let mut body = DocumentBody::new(1);
        apply_text(&mut body, 0, "keep me");
        assert_eq!(
            body.preview_import(b"not loro"),
            Err(ImportFailureCategory::DecodeOrChecksum)
        );
        assert_eq!(body.import(b"not loro"), Err(BodyError::Loro));
        assert_eq!(body.text(), "keep me");
    }

    #[test]
    fn bounds_are_enforced() {
        let mut body = DocumentBody::new(1);
        assert_eq!(
            body.apply(&[TextOp::Retain { retain: 1 }]).unwrap_err(),
            BodyError::OutOfRange
        );
        assert_eq!(
            body.apply(&[TextOp::Delete { delete: 1 }]).unwrap_err(),
            BodyError::OutOfRange
        );
        let huge = "y".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            body.apply(&[TextOp::Insert { insert: huge }]).unwrap_err(),
            BodyError::TooLarge
        );
        assert_eq!(body.text(), "");
        assert_eq!(body.revision(), 0);
    }

    #[test]
    fn recent_identity_cache_is_bounded_and_pins_an_unacknowledged_request() {
        let mut body = DocumentBody::new(1);
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
    }

    #[test]
    fn diff_is_minimal_and_unicode_aware() {
        assert_eq!(diff("", ""), vec![]);
        assert_eq!(diff("same", "same"), vec![]);
        assert_eq!(
            diff("abc", "abXc"),
            vec![
                TextOp::Retain { retain: 2 },
                TextOp::Insert { insert: "X".into() }
            ]
        );
        assert_eq!(
            diff("abc", "ac"),
            vec![TextOp::Retain { retain: 1 }, TextOp::Delete { delete: 1 }]
        );
        assert_eq!(
            diff("héllo", "hállo"),
            vec![
                TextOp::Retain { retain: 1 },
                TextOp::Delete { delete: 1 },
                TextOp::Insert {
                    insert: "á".into()
                }
            ]
        );
        assert_eq!(
            diff("aaa", "aa"),
            vec![TextOp::Retain { retain: 2 }, TextOp::Delete { delete: 1 }]
        );
    }
}
