//! The in-memory body of one Document: a Loro text container, the text it currently renders,
//! and the bookkeeping that turns local edits into bounded records.
//!
//! Nothing here is persisted. A body is rebuilt at activation by importing every opened
//! `document.edit` record; the Package SQLite never holds plaintext.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use loro::{ExportMode, LoroDoc, VersionVector};
use serde::{Deserialize, Serialize};

use crate::domain::MAX_UPDATE;

/// The text container every Document body uses.
const CONTAINER: &str = "body";
/// Local edits are appended once no further edit arrives within this window.
pub(crate) const COALESCE_WINDOW: Duration = Duration::from_millis(300);
/// Pending local edits are appended early once their export reaches this size.
pub(crate) const EXPORT_BUDGET: usize = 64 * 1024;
/// The largest body the UI protocol carries in one event.
pub(crate) const MAX_TEXT_BYTES: usize = 192 * 1024;
/// A single inserted run larger than this is applied in pieces with an export between them,
/// so no one record approaches the canonical bound.
const INSERT_CHUNK: usize = 48 * 1024;
/// Per-operation encoding overhead assumed when estimating a pending export's size.
const OP_OVERHEAD: usize = 32;

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
    /// Applying the ops would exceed [`MAX_TEXT_BYTES`].
    TooLarge,
    Loro,
}

/// Why a flush produced bytes: the caller appends each returned update as one record.
pub(crate) struct Flushed {
    pub(crate) updates: Vec<Vec<u8>>,
}

pub(crate) struct DocumentBody {
    doc: LoroDoc,
    text: String,
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
    /// A conservative estimate of the pending export size, so a keystroke never exports.
    pending_estimate: usize,
    /// Last request identity allows retry after a failed append without reapplying its ops.
    last_request: Option<(u64, Vec<TextOp>, u64)>,
}

impl DocumentBody {
    pub(crate) fn new(peer: u64) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer)
            .expect("a fresh document accepts its peer id");
        Self {
            doc,
            text: String::new(),
            revision: 0,
            appended: VersionVector::default(),
            staged_version: VersionVector::default(),
            pending_since: None,
            staged: VecDeque::new(),
            unopened: 0,
            pending_estimate: 0,
            last_request: None,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
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
    /// A body is editable once every locally known edit has been imported, so this peer's
    /// own earlier operations are present before it creates new ones.
    pub(crate) fn editable(&self) -> bool {
        self.unopened == 0
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pending_since.is_some() || !self.staged.is_empty()
    }

    pub(crate) fn retried_revision(&self, base: u64, ops: &[TextOp]) -> Option<u64> {
        self.last_request
            .as_ref()
            .and_then(|(previous, previous_ops, revision)| {
                (*previous == base && previous_ops == ops).then_some(*revision)
            })
    }

    pub(crate) fn remember_request(&mut self, base: u64, ops: &[TextOp]) {
        self.last_request = Some((base, ops.to_vec(), self.revision));
    }

    /// Applies UI operations as local edits and returns the delta the UI should see echoed.
    pub(crate) fn apply(&mut self, ops: &[TextOp]) -> Result<Vec<TextOp>, BodyError> {
        let mut inserted = 0usize;
        let mut deleted = 0usize;
        let mut cursor = 0usize;
        let length = self.text.chars().count();
        for op in ops {
            match op {
                TextOp::Retain { retain } => {
                    cursor = cursor.checked_add(*retain).ok_or(BodyError::OutOfRange)?
                }
                TextOp::Insert { insert } => inserted += insert.len(),
                TextOp::Delete { delete } => {
                    cursor = cursor.checked_add(*delete).ok_or(BodyError::OutOfRange)?;
                    deleted += delete;
                }
            }
            if cursor > length {
                return Err(BodyError::OutOfRange);
            }
        }
        // Deleted bytes are at least as many as deleted characters; the bound is conservative.
        if self.text.len() + inserted > MAX_TEXT_BYTES + deleted {
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

    /// Imports one update from a record and returns the delta the UI should apply, or `None`
    /// when the text did not change (already known, or its dependencies are still pending).
    pub(crate) fn import(&mut self, update: &[u8]) -> Result<Option<Vec<TextOp>>, BodyError> {
        let before = self.text.clone();
        self.doc.import(update).map_err(|_| BodyError::Loro)?;
        let after = self.doc.get_text(CONTAINER).to_string();
        // A remote update never advances what this peer has appended, but the remote ops are
        // now part of the version the next export is taken from; exclude them explicitly.
        self.appended = merge_remote(&self.appended, &self.doc.oplog_vv(), self.doc.peer_id());
        self.staged_version = merge_remote(
            &self.staged_version,
            &self.doc.oplog_vv(),
            self.doc.peer_id(),
        );
        let delta = diff(&before, &after);
        self.text = after;
        if delta.is_empty() {
            return Ok(None);
        }
        self.revision += 1;
        Ok(Some(delta))
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
        debug_assert!(
            update.len() <= MAX_UPDATE,
            "an export stayed within one record"
        );
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
        assert!(reader.import(&first[0]).unwrap().is_some());
        assert_eq!(reader.text(), "one two");
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
