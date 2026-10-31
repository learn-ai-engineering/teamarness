#![forbid(unsafe_code)]

//! The Documents Package: collaborative Markdown documents, Collections, and Comments.
//!
//! Every change is a record on the immutable record log; document text is rebuilt with Loro.
//! This crate depends only on the language-neutral Package SDK; transport and key custody
//! stay in Core.

mod backend;
pub(crate) mod body;
pub(crate) mod domain;
pub mod markdown;
pub(crate) mod store;

pub use backend::{DocumentsActive, router};
pub use domain::{DomainError, DomainRecord};
pub use store::{
    CollectionView, DocumentStore, DocumentView, IngestResult, StoreError, StoredRecord,
};

#[cfg(test)]
mod store_tests {
    use super::*;

    const ALICE: &str = "alice";
    const BOB: &str = "bob";

    fn hex(length: usize, seed: u64) -> String {
        format!("{seed:0length$x}")
    }

    fn genesis() -> Vec<u8> {
        crate::body::genesis()
    }

    /// An identified edit whose request ID no assertion names; each call gets a fresh one.
    fn unidentified_edit(
        author: &str,
        document: &str,
        update: Vec<u8>,
    ) -> Result<DomainRecord, DomainError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = format!(
            "e{:031x}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        DomainRecord::identified_document_edit(author, document, &id, &[0xee; 32], update)
    }

    struct Fixture {
        store: DocumentStore,
        _directory: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::open(directory.path().join("documents.sqlite3")).unwrap();
        Fixture {
            store,
            _directory: directory,
        }
    }

    /// Ingests `record` as record `seed`, authored by `author`, with the cursor for `seed`.
    fn ingest(
        store: &DocumentStore,
        seed: u64,
        author: &str,
        record: &DomainRecord,
    ) -> IngestResult {
        store
            .ingest(
                &hex(64, seed),
                &hex(82, seed),
                author,
                record.kind(),
                "sig",
                &record.encode(),
                b"sealed",
                Some(&hex(50, seed)),
            )
            .unwrap()
    }

    fn names(store: &DocumentStore) -> Vec<(String, Option<String>, bool)> {
        store
            .collections()
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.name, c.archived))
            .collect()
    }

    #[test]
    fn replay_qualification_requires_every_previously_projected_record_after_cursor_reset() {
        let f = fixture();
        let create = DomainRecord::collection_create(ALICE, "Plans").unwrap();
        let rename = DomainRecord::collection_rename(ALICE, &hex(64, 1), "Roadmap").unwrap();
        assert_eq!(ingest(&f.store, 1, ALICE, &create), IngestResult::Applied);
        assert_eq!(ingest(&f.store, 2, ALICE, &rename), IngestResult::Applied);
        f.store.begin_replay_qualification().unwrap();
        f.store.reset_cursor().unwrap();
        assert!(!f.store.replayed_previous_projection().unwrap());
        assert_eq!(ingest(&f.store, 1, ALICE, &create), IngestResult::Duplicate);
        f.store.note_replayed(&hex(64, 1)).unwrap();
        assert!(
            !f.store.replayed_previous_projection().unwrap(),
            "a changed journal with a missing old row cannot prove absence"
        );
        assert_eq!(ingest(&f.store, 2, ALICE, &rename), IngestResult::Duplicate);
        f.store.note_replayed(&hex(64, 2)).unwrap();
        assert!(f.store.replayed_previous_projection().unwrap());
        // A new activation takes a fresh prefix; old proof is never reused.
        f.store.begin_replay_qualification().unwrap();
        assert!(!f.store.replayed_previous_projection().unwrap());
    }

    #[test]
    fn v4_identified_edits_rebuild_exact_index_without_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let doc = hex(64, 2);
        let id = hex(32, 9);
        let edit =
            DomainRecord::identified_document_edit(ALICE, &doc, &id, &[7; 32], vec![42]).unwrap();
        let first_id = hex(32, 8);
        let first =
            DomainRecord::identified_document_edit(ALICE, &doc, &first_id, &[6; 32], vec![1])
                .unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            ingest(
                &store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
            );
            ingest(
                &store,
                2,
                ALICE,
                &DomainRecord::document_create(ALICE, &hex(64, 1), "Notes", genesis()).unwrap(),
            );
            ingest(&store, 3, ALICE, &first);
            ingest(&store, 4, ALICE, &edit);
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; DROP TABLE comment_history_state; DROP TABLE comment_history_changes; PRAGMA user_version=4;")
            .unwrap();
        drop(connection);
        let store = DocumentStore::open(&path).unwrap();
        assert_eq!(
            store.edit_identity(&id).unwrap(),
            None,
            "migration alone is not proof"
        );
        let old = store.edit_records(None).unwrap();
        assert_eq!(old.len(), 2);
        assert!(store.index_opened_edit(&old[0], &first.encode()).unwrap());
        assert_eq!(
            store.edit_identity(&id).unwrap(),
            None,
            "only the opened record is indexed"
        );
        assert!(store.index_opened_edit(&old[1], &edit.encode()).unwrap());
        assert_eq!(
            store.edit_identity(&id).unwrap(),
            Some((hex(64, 4), doc.clone(), ALICE.into()))
        );
        assert_eq!(
            store.indexed_edit_record(&id).unwrap().unwrap().record_id,
            hex(64, 4)
        );
        drop(store);
        let store = DocumentStore::open(&path).unwrap();
        assert!(store.index_opened_edit(&old[1], &edit.encode()).unwrap());
        let connection = rusqlite::Connection::open(&path).unwrap();
        let columns = connection
            .prepare("PRAGMA table_info(edit_identities)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            columns,
            ["request_id", "record_id", "document_id", "author"]
        );
        assert_eq!(store.edit_identity(&id).unwrap().unwrap().0, hex(64, 4));
    }

    #[test]
    fn opposite_arrival_orders_quarantine_both_documents_and_preserve_unrelated_delivery() {
        let id = hex(32, 77);
        let a = hex(64, 2);
        let b = hex(64, 3);
        for reversed in [false, true] {
            let f = fixture();
            ingest(
                &f.store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
            );
            for (seed, title) in [(2, "A"), (3, "B")] {
                ingest(
                    &f.store,
                    seed,
                    ALICE,
                    &DomainRecord::document_create(ALICE, &hex(64, 1), title, genesis()).unwrap(),
                );
            }
            let mut records = vec![(4, ALICE, &a, vec![1]), (5, BOB, &b, vec![2])];
            if reversed {
                records.reverse();
            }
            for (seed, author, target, update) in records {
                ingest(
                    &f.store,
                    seed,
                    author,
                    &DomainRecord::identified_document_edit(
                        author,
                        target,
                        &id,
                        &[seed as u8; 32],
                        update,
                    )
                    .unwrap(),
                );
            }
            for target in [&a, &b] {
                assert!(f.store.identity_conflicted(target).unwrap());
                let later = unidentified_edit(ALICE, target, vec![3]).unwrap();
                assert_eq!(
                    ingest(&f.store, 10 + target.eq(&b) as u64, ALICE, &later),
                    IngestResult::Quarantined("identity_conflict".into())
                );
                assert!(f.store.edit_records(Some(target)).unwrap().is_empty());
            }
            assert_eq!(
                ingest(
                    &f.store,
                    12,
                    ALICE,
                    &unidentified_edit(ALICE, &hex(64, 99), vec![4]).unwrap()
                ),
                IngestResult::Deferred("waiting_for_target".into())
            );
            assert_eq!(f.store.cursor().unwrap(), Some(hex(50, 12)));
        }
    }

    #[test]
    fn exact_index_quarantines_collisions_and_delivers_later_edits_after_eviction() {
        let f = fixture();
        let doc = hex(64, 2);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &hex(64, 1), "Notes", genesis()).unwrap(),
        );
        let first = hex(32, 10);
        let make = |id: &str, author: &str, document: &str| {
            DomainRecord::identified_document_edit(author, document, id, &[1; 32], vec![42])
                .unwrap()
        };
        assert_eq!(
            ingest(&f.store, 3, ALICE, &make(&first, ALICE, &doc)),
            IngestResult::Applied
        );
        for n in 0..=crate::body::MAX_RECENT_IDENTITIES {
            assert_eq!(
                ingest(
                    &f.store,
                    100 + n as u64,
                    ALICE,
                    &make(&hex(32, 100 + n as u64), ALICE, &doc)
                ),
                IngestResult::Applied
            );
        }
        for (seed, author, document) in [
            (500, ALICE, doc.clone()),
            (501, BOB, doc.clone()),
            (502, ALICE, hex(64, 99)),
        ] {
            assert_eq!(
                ingest(&f.store, seed, author, &make(&first, author, &document)),
                IngestResult::IdentityConflict(document)
            );
            assert_eq!(f.store.cursor().unwrap(), Some(hex(50, seed)));
        }
        assert_eq!(
            f.store.edit_identity(&first).unwrap().unwrap().0,
            hex(64, 3)
        );
        assert!(f.store.has_projection_warnings().unwrap());
        assert!(f.store.identity_conflicted(&doc).unwrap());
        assert!(f.store.identity_conflicted(&hex(64, 99)).unwrap());
        assert!(f.store.edit_records(None).unwrap().is_empty());
        assert_eq!(
            ingest(&f.store, 503, ALICE, &make(&hex(32, 503), ALICE, &doc)),
            IngestResult::Quarantined("identity_conflict".into())
        );
        assert!(f.store.edit_records(None).unwrap().is_empty());
    }

    #[test]
    fn v4_applied_and_deferred_collision_is_hidden_in_both_rebuild_orders_and_restart() {
        for reverse in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("documents.sqlite3");
            let document = hex(64, 2);
            let id = hex(32, 17);
            let a =
                DomainRecord::identified_document_edit(ALICE, &document, &id, &[1; 32], vec![1])
                    .unwrap();
            let b = DomainRecord::identified_document_edit(BOB, &document, &id, &[2; 32], vec![2])
                .unwrap();
            {
                let store = DocumentStore::open(&path).unwrap();
                ingest(
                    &store,
                    1,
                    ALICE,
                    &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
                );
                ingest(
                    &store,
                    2,
                    ALICE,
                    &DomainRecord::document_create(ALICE, &hex(64, 1), "Notes", genesis()).unwrap(),
                );
                assert_eq!(ingest(&store, 3, ALICE, &a), IngestResult::Applied);
                assert_eq!(
                    ingest(&store, 4, BOB, &b),
                    IngestResult::IdentityConflict(document.clone())
                );
                store.inner.lock().unwrap().conn.execute(
                    "UPDATE records SET quarantine_code='waiting_for_target',target=?2 WHERE record_id=?1",
                    rusqlite::params![hex(64, 4), document],
                ).unwrap();
            }
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; DROP TABLE comment_history_state; DROP TABLE comment_history_changes; PRAGMA user_version=4;").unwrap();
            drop(connection);
            let store = DocumentStore::open(&path).unwrap();
            let mut rows = store.identity_candidates().unwrap();
            if reverse {
                rows.reverse();
            }
            for row in rows {
                let canonical = if row.record_id == hex(64, 3) {
                    a.encode()
                } else {
                    b.encode()
                };
                store.index_opened_edit(&row, &canonical).unwrap();
            }
            assert!(store.identity_conflicted(&document).unwrap());
            assert!(store.edit_records(Some(&document)).unwrap().is_empty());
            drop(store);
            let reopened = DocumentStore::open(&path).unwrap();
            assert!(reopened.identity_conflicted(&document).unwrap());
            assert!(reopened.edit_records(Some(&document)).unwrap().is_empty());
        }
    }

    #[test]
    fn v4_deferred_identified_edits_index_before_collision_and_never_apply_quarantine() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let document = hex(64, 9);
        let id = hex(32, 17);
        let old = DomainRecord::identified_document_edit(ALICE, &document, &id, &[1; 32], vec![1])
            .unwrap();
        let conflicting =
            DomainRecord::identified_document_edit(BOB, &document, &id, &[2; 32], vec![2]).unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            assert!(matches!(
                ingest(&store, 1, ALICE, &old),
                IngestResult::Deferred(_)
            ));
            assert!(matches!(
                ingest(&store, 2, BOB, &conflicting),
                IngestResult::IdentityConflict(_)
            ));
            // Simulate a v4 projection that stored both authenticated deferred records,
            // before the opaque table existed to reject the second ID.
            store
                .inner
                .lock()
                .unwrap()
                .conn
                .execute(
                    "UPDATE records SET quarantine_code='waiting_for_target',target=?2 WHERE record_id=?1",
                    rusqlite::params![hex(64, 2), document],
                )
                .unwrap();
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE edit_identities; DROP TABLE document_identity_conflicts; DROP TABLE document_import_failures; DROP TABLE comment_history_state; DROP TABLE comment_history_changes; PRAGMA user_version=4;")
            .unwrap();
        drop(connection);
        let store = DocumentStore::open(&path).unwrap();
        let rows = store.deferred_records().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(store.edit_identity(&id).unwrap().is_none());
        assert!(store.index_opened_edit(&rows[0], &old.encode()).unwrap());
        assert!(matches!(
            store
                .retry_deferred(&hex(64, 1), &old.encode())
                .unwrap()
                .result,
            IngestResult::Deferred(_)
        ));
        assert!(
            !store
                .index_opened_edit(&rows[1], &conflicting.encode())
                .unwrap()
        );
        assert_eq!(store.edit_identity(&id).unwrap().unwrap().0, hex(64, 1));
        assert!(matches!(
            store
                .retry_deferred(&hex(64, 2), &conflicting.encode())
                .unwrap()
                .result,
            IngestResult::Quarantined(_)
        ));
        // A collision keeps the target unavailable even after its create arrives.
        assert_eq!(
            ingest(&store, 3, BOB, &conflicting),
            IngestResult::IdentityConflict(document.clone())
        );
        assert!(matches!(
            ingest(
                &store,
                4,
                ALICE,
                &DomainRecord::identified_document_edit(
                    ALICE,
                    &document,
                    &hex(32, 99),
                    &[3; 32],
                    vec![3]
                )
                .unwrap()
            ),
            IngestResult::Quarantined(ref code) if code == "identity_conflict"
        ));
        ingest(
            &store,
            8,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
        );
        let created = store
            .ingest_with_commit(
                &document,
                &hex(82, 9),
                ALICE,
                "document.create",
                "sig",
                &DomainRecord::document_create(ALICE, &hex(64, 8), "Notes", genesis())
                    .unwrap()
                    .encode(),
                b"sealed",
                None,
                &|| {},
            )
            .unwrap();
        assert_eq!(
            created.edits.len(),
            0,
            "a conflicted target cannot import even its original deferred edit"
        );
        assert_eq!(store.cursor().unwrap(), Some(hex(50, 8)));
    }

    #[test]
    fn import_failure_marker_rolls_back_with_sealed_row_identity_and_cursor() {
        let f = fixture();
        let document = hex(64, 2);
        let id = hex(32, 9);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &hex(64, 1), "Doc", genesis()).unwrap(),
        );
        let cursor = f.store.cursor().unwrap();
        let edit = DomainRecord::identified_document_edit(
            ALICE,
            &document,
            &id,
            &[7; 32],
            b"not loro".to_vec(),
        )
        .unwrap();
        f.store.inner.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER deny_import_failure BEFORE INSERT ON document_import_failures BEGIN SELECT RAISE(FAIL, 'injected'); END;"
        ).unwrap();
        assert!(
            f.store
                .ingest_with_import(
                    &hex(64, 3),
                    &hex(82, 3),
                    ALICE,
                    edit.kind(),
                    "sig",
                    &edit.encode(),
                    b"sealed",
                    Some(&hex(50, 3)),
                    &|| {},
                    &|_, _, _, _, _, _| Some("decode_or_checksum")
                )
                .is_err()
        );
        assert_eq!(f.store.cursor().unwrap(), cursor);
        assert_eq!(f.store.record_count().unwrap(), 2);
        assert!(f.store.edit_identity(&id).unwrap().is_none());
        assert!(f.store.import_failure(&document).unwrap().is_none());
        f.store
            .inner
            .lock()
            .unwrap()
            .conn
            .execute_batch("DROP TRIGGER deny_import_failure;")
            .unwrap();
        let ingested = f
            .store
            .ingest_with_import(
                &hex(64, 3),
                &hex(82, 3),
                ALICE,
                edit.kind(),
                "sig",
                &edit.encode(),
                b"sealed",
                Some(&hex(50, 3)),
                &|| {},
                &|_, _, _, _, _, _| Some("decode_or_checksum"),
            )
            .unwrap();
        assert_eq!(
            ingested.result,
            IngestResult::ImportFailure(document.clone())
        );
        assert_eq!(f.store.cursor().unwrap(), Some(hex(50, 3)));
        assert!(f.store.edit_identity(&id).unwrap().is_some());
        assert_eq!(
            f.store.import_failure(&document).unwrap(),
            Some((hex(64, 3), "decode_or_checksum".into()))
        );
    }

    #[test]
    fn conflict_marker_failure_rolls_back_both_quarantine_and_cursor() {
        let f = fixture();
        let document = hex(64, 2);
        let id = hex(32, 1);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &hex(64, 1), "Notes", genesis()).unwrap(),
        );
        let first =
            DomainRecord::identified_document_edit(ALICE, &document, &id, &[1; 32], vec![1])
                .unwrap();
        let second =
            DomainRecord::identified_document_edit(BOB, &document, &id, &[2; 32], vec![2]).unwrap();
        ingest(&f.store, 3, ALICE, &first);
        let cursor = f.store.cursor().unwrap();
        f.store.inner.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_conflict BEFORE INSERT ON document_identity_conflicts BEGIN SELECT RAISE(ABORT, 'fault'); END;"
        ).unwrap();
        assert!(
            f.store
                .ingest(
                    &hex(64, 4),
                    &hex(82, 4),
                    BOB,
                    second.kind(),
                    "sig",
                    &second.encode(),
                    b"sealed",
                    Some(&hex(50, 4))
                )
                .is_err()
        );
        assert_eq!(f.store.cursor().unwrap(), cursor);
        assert_eq!(f.store.record_count().unwrap(), 3);
        assert!(!f.store.identity_conflicted(&document).unwrap());
        assert_eq!(f.store.edit_records(Some(&document)).unwrap().len(), 1);
    }

    #[test]
    fn index_write_failure_rolls_back_sealed_row_and_cursor() {
        let f = fixture();
        let doc = hex(64, 2);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Plan").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &hex(64, 1), "Notes", genesis()).unwrap(),
        );
        let cursor = f.store.cursor().unwrap();
        let edit =
            DomainRecord::identified_document_edit(ALICE, &doc, &hex(32, 7), &[1; 32], vec![1])
                .unwrap();
        f.store.inner.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_identity BEFORE INSERT ON edit_identities BEGIN SELECT RAISE(ABORT, 'fault'); END;"
        ).unwrap();
        assert!(
            f.store
                .ingest(
                    &hex(64, 3),
                    &hex(82, 3),
                    ALICE,
                    edit.kind(),
                    "sig",
                    &edit.encode(),
                    b"sealed",
                    Some(&hex(50, 3))
                )
                .is_err()
        );
        assert_eq!(f.store.cursor().unwrap(), cursor);
        assert_eq!(f.store.record_count().unwrap(), 2);
        assert!(f.store.edit_identity(&hex(32, 7)).unwrap().is_none());
    }

    // Verifies: AC4 Collections and Documents converge regardless of arrival order.
    #[test]
    fn metadata_converges_in_any_arrival_order() {
        let create = DomainRecord::collection_create(ALICE, "Planning").unwrap();
        let collection = hex(64, 1);
        let rename_early = DomainRecord::collection_rename(BOB, &collection, "Plans").unwrap();
        let rename_late = DomainRecord::collection_rename(ALICE, &collection, "Roadmap").unwrap();
        let archive = DomainRecord::collection_archive(ALICE, &collection).unwrap();
        let restore = DomainRecord::collection_restore(BOB, &collection).unwrap();
        let document = DomainRecord::document_create(BOB, &collection, "Q4", genesis()).unwrap();
        let steps: Vec<(u64, &str, &DomainRecord)> = vec![
            (1, ALICE, &create),
            (2, BOB, &rename_early),
            (3, ALICE, &rename_late),
            (4, ALICE, &archive),
            (5, BOB, &restore),
            (6, BOB, &document),
        ];

        let forward = fixture();
        for (seed, author, record) in &steps {
            assert_eq!(
                ingest(&forward.store, *seed, author, record),
                IngestResult::Applied
            );
        }
        let reversed = fixture();
        let mut deferred = 0;
        for (seed, author, record) in steps.iter().rev() {
            if matches!(
                ingest(&reversed.store, *seed, author, record),
                IngestResult::Deferred(_)
            ) {
                deferred += 1;
            }
        }
        assert_eq!(deferred, 5, "everything but the create waits on it");
        let expected = vec![(collection.clone(), Some("Roadmap".to_owned()), false)];
        assert_eq!(names(&forward.store), expected);
        assert_eq!(names(&reversed.store), expected);
        for store in [&forward.store, &reversed.store] {
            let documents = store.documents().unwrap();
            assert_eq!(documents.len(), 1);
            assert_eq!(documents[0].id, hex(64, 6));
            assert_eq!(documents[0].collection_id, collection);
            assert_eq!(documents[0].title.as_deref(), Some("Q4"));
            assert!(store.deferred_records().unwrap().is_empty());
            assert!(!store.has_projection_warnings().unwrap());
        }
        // The delivery cursor advanced to the last ingested record in both orders.
        assert_eq!(
            forward.store.cursor().unwrap().as_deref(),
            Some(hex(50, 6).as_str())
        );
        assert_eq!(
            reversed.store.cursor().unwrap().as_deref(),
            Some(hex(50, 1).as_str())
        );
    }

    /// A late record that sorts before an applied one never overrides it (ADR 0008).
    #[test]
    fn late_earlier_sorting_records_do_not_override_later_state() {
        let f = fixture();
        let collection = hex(64, 1);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "A").unwrap(),
        );
        ingest(
            &f.store,
            9,
            ALICE,
            &DomainRecord::collection_rename(ALICE, &collection, "Nine").unwrap(),
        );
        ingest(
            &f.store,
            9,
            ALICE,
            &DomainRecord::collection_rename(ALICE, &collection, "Nine").unwrap(),
        );
        ingest(
            &f.store,
            8,
            BOB,
            &DomainRecord::collection_rename(BOB, &collection, "Eight").unwrap(),
        );
        ingest(
            &f.store,
            7,
            BOB,
            &DomainRecord::collection_archive(BOB, &collection).unwrap(),
        );
        ingest(
            &f.store,
            6,
            ALICE,
            &DomainRecord::collection_restore(ALICE, &collection).unwrap(),
        );
        assert_eq!(
            names(&f.store),
            vec![(collection, Some("Nine".to_owned()), true)]
        );
    }

    #[test]
    fn duplicates_mismatches_and_malformed_records_are_classified() {
        let f = fixture();
        let create = DomainRecord::collection_create(ALICE, "A").unwrap();
        assert_eq!(ingest(&f.store, 1, ALICE, &create), IngestResult::Applied);
        assert_eq!(ingest(&f.store, 1, ALICE, &create), IngestResult::Duplicate);
        let mismatch = f
            .store
            .ingest(
                &hex(64, 1),
                &hex(82, 2),
                ALICE,
                create.kind(),
                "sig",
                &create.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        assert_eq!(
            mismatch,
            IngestResult::Quarantined("immutable_field_mismatch".into())
        );
        let malformed = f
            .store
            .ingest(
                &hex(64, 3),
                &hex(82, 3),
                ALICE,
                "collection.create",
                "sig",
                b"garbage",
                b"sealed",
                None,
            )
            .unwrap();
        assert_eq!(
            malformed,
            IngestResult::Quarantined("malformed_record".into())
        );
        let forged = DomainRecord::collection_create(BOB, "B").unwrap();
        assert_eq!(
            ingest(&f.store, 4, ALICE, &forged),
            IngestResult::Quarantined("signer_mismatch".into())
        );
        let wrong_purpose = f
            .store
            .ingest(
                &hex(64, 5),
                &hex(82, 5),
                ALICE,
                "document.create",
                "sig",
                &create.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        assert_eq!(
            wrong_purpose,
            IngestResult::Quarantined("purpose_mismatch".into())
        );
        assert!(f.store.has_projection_warnings().unwrap());
        assert_eq!(f.store.collections().unwrap().len(), 1);
        assert_eq!(
            f.store.record_count().unwrap(),
            4,
            "a mismatched duplicate adds no row"
        );
    }

    /// A move waits on both its Document and its destination Collection.
    #[test]
    fn a_move_waits_for_its_destination_collection() {
        let f = fixture();
        let source = hex(64, 1);
        let document = hex(64, 2);
        let destination = hex(64, 3);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Source").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &source, "Doc", genesis()).unwrap(),
        );
        let moved = DomainRecord::document_move(ALICE, &document, &destination).unwrap();
        assert!(matches!(
            ingest(&f.store, 4, ALICE, &moved),
            IngestResult::Deferred(_)
        ));
        assert_eq!(f.store.documents().unwrap()[0].collection_id, source);
        ingest(
            &f.store,
            3,
            ALICE,
            &DomainRecord::collection_create(ALICE, "Destination").unwrap(),
        );
        assert_eq!(f.store.documents().unwrap()[0].collection_id, destination);
        assert!(f.store.deferred_records().unwrap().is_empty());
    }

    /// Names live only in memory: a reopened store knows ordering and archive state, and
    /// recovers names from the records it is allowed to open.
    #[test]
    fn restart_keeps_structure_and_rehydrates_names_from_opened_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let collection = hex(64, 1);
        let create = DomainRecord::collection_create(ALICE, "Planning").unwrap();
        let rename = DomainRecord::collection_rename(ALICE, &collection, "Plans").unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            ingest(&store, 1, ALICE, &create);
            ingest(&store, 2, ALICE, &rename);
        }
        let store = DocumentStore::open(&path).unwrap();
        assert_eq!(names(&store), vec![(collection.clone(), None, false)]);
        let control = store.control_records().unwrap();
        assert_eq!(control.len(), 2);
        // Rehydrate in reverse to prove last-writer-wins does not depend on replay order.
        store
            .remember_opened(&hex(64, 2), &hex(82, 2), &rename.encode())
            .unwrap();
        store
            .remember_opened(&hex(64, 1), &hex(82, 1), &create.encode())
            .unwrap();
        assert_eq!(
            names(&store),
            vec![(collection, Some("Plans".to_owned()), false)]
        );
        assert_eq!(
            store.cursor().unwrap().as_deref(),
            Some(hex(50, 2).as_str())
        );
        store.reset_cursor().unwrap();
        assert_eq!(store.cursor().unwrap(), None);
    }

    /// A deferred record re-opened after restart is retried through the same path.
    #[test]
    fn deferred_records_retry_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let collection = hex(64, 1);
        let document = DomainRecord::document_create(ALICE, &collection, "Doc", genesis()).unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            assert!(matches!(
                ingest(&store, 2, ALICE, &document),
                IngestResult::Deferred(_)
            ));
        }
        let store = DocumentStore::open(&path).unwrap();
        assert_eq!(store.deferred_records().unwrap().len(), 1);
        assert!(matches!(
            store
                .retry_deferred(&hex(64, 2), &document.encode())
                .unwrap()
                .result,
            IngestResult::Deferred(_)
        ));
        ingest(
            &store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "C").unwrap(),
        );
        assert!(matches!(
            store
                .retry_deferred(&hex(64, 2), &document.encode())
                .unwrap()
                .result,
            IngestResult::Applied | IngestResult::Duplicate
        ));
        assert_eq!(store.documents().unwrap().len(), 1);
    }

    /// A body edit that arrives before its Document waits, then surfaces once the Document's
    /// create record is applied; the edit rows are findable per Document afterwards.
    #[test]
    fn edits_wait_for_their_document_and_surface_when_it_arrives() {
        let f = fixture();
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let edit = unidentified_edit(BOB, &document, vec![1, 2, 3]).unwrap();
        assert!(matches!(
            ingest(&f.store, 3, BOB, &edit),
            IngestResult::Deferred(_)
        ));
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "C").unwrap(),
        );
        let genesis = genesis();
        let ingested = f
            .store
            .ingest_with_commit(
                &hex(64, 2),
                &hex(82, 2),
                ALICE,
                "document.create",
                "sig",
                &DomainRecord::document_create(ALICE, &collection, "Doc", genesis.clone())
                    .unwrap()
                    .encode(),
                b"sealed",
                Some(&hex(50, 2)),
                &|| {},
            )
            .unwrap();
        assert_eq!(ingested.result, IngestResult::Applied);
        assert_eq!(
            ingested.edits,
            vec![
                (document.clone(), genesis),
                (document.clone(), vec![1, 2, 3])
            ],
            "the genesis is the body's first update, before the edit it unblocked"
        );
        assert_eq!(f.store.edit_records(Some(&document)).unwrap().len(), 1);
        let body = f.store.body_records(Some(&document)).unwrap();
        assert_eq!(
            body.iter()
                .map(|row| (row.purpose.as_str(), row.target.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("document.create", Some(document.as_str())),
                ("document.edit", Some(document.as_str()))
            ]
        );
        assert_eq!(f.store.body_records(None).unwrap().len(), 2);
        assert_eq!(
            f.store.edit_records(None).unwrap()[0].target.as_deref(),
            Some(document.as_str())
        );
        assert!(f.store.edit_records(Some("not-an-id")).is_err());
        let direct = unidentified_edit(ALICE, &document, vec![9]).unwrap();
        let ingested = f
            .store
            .ingest_with_commit(
                &hex(64, 4),
                &hex(82, 4),
                ALICE,
                "document.edit",
                "sig",
                &direct.encode(),
                b"sealed",
                None,
                &|| {},
            )
            .unwrap();
        assert_eq!(ingested.edits, vec![(document.clone(), vec![9])]);
        assert_eq!(f.store.edit_records(Some(&document)).unwrap().len(), 2);
    }

    // Verifies: AC5 — v3 databases migrate with nullable author time; old records stay
    // readable after restart and new signed times survive projection and replay.
    #[test]
    fn comment_time_migrates_v3_and_replays_old_and_new_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(concat!(
                    include_str!("../migrations/0001_documents.sql"),
                    include_str!("../migrations/0002_edit_targets.sql"),
                    include_str!("../migrations/0003_comments.sql")
                ))
                .unwrap();
            connection.pragma_update(None, "user_version", 3).unwrap();
        }
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let old = DomainRecord::comment_add(ALICE, &document, None, None, "Old").unwrap();
        let new =
            DomainRecord::comment_add_at(BOB, &document, None, None, "New", Some("1730000000000"))
                .unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            ingest(
                &store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "C").unwrap(),
            );
            ingest(
                &store,
                2,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "D", genesis()).unwrap(),
            );
            ingest(&store, 3, ALICE, &old);
            ingest(&store, 4, BOB, &new);
            assert_eq!(store.comments(&document).unwrap()[0].created_at, None);
            assert_eq!(
                store.comments(&document).unwrap()[1].created_at.as_deref(),
                Some("1730000000000")
            );
        }
        let store = DocumentStore::open(&path).unwrap();
        assert_eq!(store.comments(&document).unwrap()[0].created_at, None);
        assert_eq!(
            store.comments(&document).unwrap()[1].created_at.as_deref(),
            Some("1730000000000")
        );
        for (seed, record) in [(3, old), (4, new)] {
            store
                .remember_opened(&hex(64, seed), &hex(82, seed), &record.encode())
                .unwrap();
        }
        assert_eq!(
            store.comments(&document).unwrap()[0].markdown.as_deref(),
            Some("Old")
        );
        assert_eq!(
            store.comments(&document).unwrap()[1].markdown.as_deref(),
            Some("New")
        );
    }

    // Verifies: AC5 — comments thread, wait for their parent, resolve last-writer-wins, and
    // rehydrate their text from opened records after restart.
    #[test]
    fn comments_thread_defer_and_rehydrate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let root = hex(64, 3);
        let reply = DomainRecord::comment_add(BOB, &document, Some(&root), Some("quoted"), "Reply")
            .unwrap();
        let add = DomainRecord::comment_add(ALICE, &document, None, None, "Root").unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            ingest(
                &store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "C").unwrap(),
            );
            ingest(
                &store,
                2,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "D", genesis()).unwrap(),
            );
            assert!(matches!(
                ingest(&store, 4, BOB, &reply),
                IngestResult::Deferred(_)
            ));
            assert_eq!(ingest(&store, 3, ALICE, &add), IngestResult::Applied);
            let comments = store.comments(&document).unwrap();
            assert_eq!(comments.len(), 2);
            assert_eq!(comments[0].markdown.as_deref(), Some("Root"));
            assert_eq!(comments[1].parent_id.as_deref(), Some(root.as_str()));
            assert_eq!(comments[1].quote.as_deref(), Some("quoted"));
            ingest(
                &store,
                6,
                BOB,
                &DomainRecord::comment_resolve(BOB, &root).unwrap(),
            );
            ingest(
                &store,
                5,
                ALICE,
                &DomainRecord::comment_reopen(ALICE, &root).unwrap(),
            );
            assert!(
                store.comments(&document).unwrap()[0].resolved,
                "the later resolve wins"
            );
            let other = hex(64, 9);
            ingest(
                &store,
                9,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "Other", genesis()).unwrap(),
            );
            let cross =
                DomainRecord::comment_add(ALICE, &other, Some(&root), None, "wrong").unwrap();
            assert_eq!(
                ingest(&store, 10, ALICE, &cross),
                IngestResult::Quarantined("parent_on_other_document".into())
            );
        }
        let store = DocumentStore::open(&path).unwrap();
        let comments = store.comments(&document).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].markdown, None, "text is not persisted");
        store
            .remember_opened(&hex(64, 3), &hex(82, 3), &add.encode())
            .unwrap();
        store
            .remember_opened(&hex(64, 4), &hex(82, 4), &reply.encode())
            .unwrap();
        let comments = store.comments(&document).unwrap();
        assert_eq!(comments[0].markdown.as_deref(), Some("Root"));
        assert_eq!(comments[1].quote.as_deref(), Some("quoted"));
        assert!(
            store
                .control_records()
                .unwrap()
                .iter()
                .any(|r| r.purpose == "comment.add")
        );
    }

    fn kinds(changes: &store::CommentChanges) -> Vec<(u64, String, String)> {
        changes
            .changes
            .iter()
            .map(|change| {
                (
                    change.revision,
                    change.kind.clone(),
                    change.comment_id.clone(),
                )
            })
            .collect()
    }

    // Verifies: TEAM.SYS.DOCS-R07 — every projected Comment add, resolve or reopen appends
    // exactly one revision, including a reply that waited for its parent; duplicates, stale
    // state records and quarantined records append none. Changes page under one epoch.
    #[test]
    fn comment_history_counts_each_projected_change_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let root = hex(64, 3);
        let reply_id = hex(64, 4);
        let add =
            DomainRecord::comment_add(ALICE, &document, None, Some("Q"), "@Scout look").unwrap();
        let reply = DomainRecord::comment_add(BOB, &document, Some(&root), None, "Reply").unwrap();
        let epoch;
        {
            let store = DocumentStore::open(&path).unwrap();
            let (head_epoch, head) = store.comment_head().unwrap();
            assert_eq!(head, 0, "a new history starts at revision 0");
            assert_eq!(head_epoch.len(), 32);
            assert!(head_epoch.bytes().all(|b| b.is_ascii_hexdigit()));
            epoch = head_epoch;
            ingest(
                &store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "C").unwrap(),
            );
            ingest(
                &store,
                2,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "D", genesis()).unwrap(),
            );
            assert!(matches!(
                ingest(&store, 4, BOB, &reply),
                IngestResult::Deferred(_)
            ));
            assert_eq!(store.comment_head().unwrap().1, 0, "a deferred reply waits");
            assert_eq!(ingest(&store, 3, ALICE, &add), IngestResult::Applied);
            assert_eq!(
                store.comment_head().unwrap().1,
                2,
                "the root, then the reply it unblocked"
            );
            assert_eq!(ingest(&store, 3, ALICE, &add), IngestResult::Duplicate);
            ingest(
                &store,
                6,
                BOB,
                &DomainRecord::comment_resolve(BOB, &root).unwrap(),
            );
            // An earlier-sorting reopen changes nothing, so it records nothing.
            ingest(
                &store,
                5,
                ALICE,
                &DomainRecord::comment_reopen(ALICE, &root).unwrap(),
            );
            ingest(
                &store,
                7,
                ALICE,
                &DomainRecord::comment_reopen(ALICE, &root).unwrap(),
            );
            ingest(
                &store,
                9,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "Other", genesis()).unwrap(),
            );
            let cross =
                DomainRecord::comment_add(ALICE, &hex(64, 9), Some(&root), None, "x").unwrap();
            assert!(matches!(
                ingest(&store, 10, ALICE, &cross),
                IngestResult::Quarantined(_)
            ));
            let all = store.comment_changes(&epoch, 0, 32).unwrap();
            assert!(!all.resync_required);
            assert_eq!(all.epoch, epoch);
            assert_eq!(
                kinds(&all),
                [
                    (1, "add".to_owned(), root.clone()),
                    (2, "add".to_owned(), reply_id.clone()),
                    (3, "resolve".to_owned(), root.clone()),
                    (4, "reopen".to_owned(), root.clone()),
                ]
            );
            assert_eq!((all.continuation, all.high_water), (4, 4));
            let first = all.changes[0].comment.as_ref().unwrap();
            assert_eq!(first.document_id, document);
            assert_eq!(first.author, ALICE);
            assert_eq!(first.markdown.as_deref(), Some("@Scout look"));
            assert_eq!(first.quote.as_deref(), Some("Q"));
            let second = all.changes[1].comment.as_ref().unwrap();
            assert_eq!(second.parent_id.as_deref(), Some(root.as_str()));

            // Paging: a limit stops at a revision, and the next page continues from it.
            let page = store.comment_changes(&epoch, 0, 2).unwrap();
            assert_eq!(page.changes.len(), 2);
            assert_eq!((page.continuation, page.high_water), (2, 4));
            let rest = store.comment_changes(&epoch, 2, 2).unwrap();
            assert_eq!(kinds(&rest)[0].0, 3);
            assert_eq!((rest.continuation, rest.high_water), (4, 4));
            let none = store.comment_changes(&epoch, 4, 32).unwrap();
            assert!(none.changes.is_empty());
            assert_eq!((none.continuation, none.high_water), (4, 4));
            assert_eq!(
                store
                    .comment_changes(&epoch, 0, 1000)
                    .unwrap()
                    .changes
                    .len(),
                4,
                "the limit is clamped, not refused"
            );

            // A foreign epoch or a cursor past the head asks for a resync from the head.
            for (asked, after) in [(hex(32, 0xe), 0), (epoch.clone(), 5)] {
                let resync = store.comment_changes(&asked, after, 32).unwrap();
                assert!(resync.resync_required);
                assert!(resync.changes.is_empty());
                assert_eq!(resync.epoch, epoch);
                assert_eq!((resync.continuation, resync.high_water), (4, 4));
            }
        }
        // The history survives restart; texts are pending until their records are reopened.
        let store = DocumentStore::open(&path).unwrap();
        assert_eq!(store.comment_head().unwrap(), (epoch.clone(), 4));
        let after_restart = store.comment_changes(&epoch, 0, 1).unwrap();
        assert_eq!(
            after_restart.changes[0].comment.as_ref().unwrap().markdown,
            None
        );
    }

    // Verifies: TEAM.SYS.DOCS-R07 — a reply deferred before restart is counted when it is
    // actually projected after restart, once.
    #[test]
    fn comment_history_counts_a_reply_rehydrated_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let root = hex(64, 3);
        let reply = DomainRecord::comment_add(BOB, &document, Some(&root), None, "Reply").unwrap();
        {
            let store = DocumentStore::open(&path).unwrap();
            ingest(
                &store,
                1,
                ALICE,
                &DomainRecord::collection_create(ALICE, "C").unwrap(),
            );
            ingest(
                &store,
                2,
                ALICE,
                &DomainRecord::document_create(ALICE, &collection, "D", genesis()).unwrap(),
            );
            assert!(matches!(
                ingest(&store, 4, BOB, &reply),
                IngestResult::Deferred(_)
            ));
        }
        let store = DocumentStore::open(&path).unwrap();
        let (epoch, _) = store.comment_head().unwrap();
        ingest(
            &store,
            3,
            ALICE,
            &DomainRecord::comment_add(ALICE, &document, None, None, "Root").unwrap(),
        );
        assert_eq!(store.comment_head().unwrap().1, 1);
        for _ in 0..2 {
            store.retry_deferred(&hex(64, 4), &reply.encode()).unwrap();
        }
        let changes = store.comment_changes(&epoch, 0, 32).unwrap();
        assert_eq!(
            kinds(&changes),
            [
                (1, "add".to_owned(), root),
                (2, "add".to_owned(), hex(64, 4)),
            ]
        );
        assert_eq!(
            changes.changes[1]
                .comment
                .as_ref()
                .unwrap()
                .markdown
                .as_deref(),
            Some("Reply")
        );
    }

    // Verifies: TEAM.SYS.DOCS-R07 — a v7 database gains an empty Comment history with a fresh
    // epoch.
    #[test]
    fn comment_history_migrates_v7_with_a_fresh_epoch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documents.sqlite3");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(concat!(
                    include_str!("../migrations/0001_documents.sql"),
                    include_str!("../migrations/0002_edit_targets.sql"),
                    include_str!("../migrations/0003_comments.sql"),
                    include_str!("../migrations/0004_comment_time.sql"),
                    include_str!("../migrations/0005_edit_identity_index.sql"),
                    include_str!("../migrations/0006_import_failure.sql"),
                    include_str!("../migrations/0007_schema_violation.sql"),
                ))
                .unwrap();
            connection.pragma_update(None, "user_version", 7).unwrap();
        }
        let first = DocumentStore::open(&path).unwrap().comment_head().unwrap();
        assert_eq!(first.1, 0);
        let other = tempfile::tempdir().unwrap();
        let second = DocumentStore::open(other.path().join("documents.sqlite3"))
            .unwrap()
            .comment_head()
            .unwrap();
        assert_ne!(first.0, second.0, "each history has its own epoch");
        assert_eq!(
            DocumentStore::open(&path).unwrap().comment_head().unwrap(),
            first,
            "reopening keeps the epoch"
        );
    }

    // Verifies: TEAM.SYS.DOCS-R07 — a thread is its root and every nested reply in creation
    // order, with its Document's ID and title, from any Comment in it.
    #[test]
    fn comment_thread_is_the_root_and_nested_replies() {
        let f = fixture();
        let collection = hex(64, 1);
        let document = hex(64, 2);
        ingest(
            &f.store,
            1,
            ALICE,
            &DomainRecord::collection_create(ALICE, "C").unwrap(),
        );
        ingest(
            &f.store,
            2,
            ALICE,
            &DomainRecord::document_create(ALICE, &collection, "Plan", genesis()).unwrap(),
        );
        let comment = |parent: Option<u64>, text: &str| {
            DomainRecord::comment_add(
                ALICE,
                &document,
                parent.map(|p| hex(64, p)).as_deref(),
                None,
                text,
            )
            .unwrap()
        };
        ingest(&f.store, 3, ALICE, &comment(None, "Root"));
        ingest(&f.store, 4, ALICE, &comment(None, "Other thread"));
        ingest(&f.store, 5, ALICE, &comment(Some(3), "First reply"));
        ingest(&f.store, 6, ALICE, &comment(Some(5), "Nested reply"));
        ingest(&f.store, 7, ALICE, &comment(Some(4), "Other reply"));
        ingest(&f.store, 8, ALICE, &comment(Some(3), "Second reply"));
        for from in [3, 5, 6, 8] {
            let thread = f.store.comment_thread(&hex(64, from)).unwrap().unwrap();
            assert_eq!(thread.document_id, document);
            assert_eq!(thread.title.as_deref(), Some("Plan"));
            let texts: Vec<_> = thread
                .comments
                .iter()
                .map(|c| c.markdown.as_deref().unwrap())
                .collect();
            assert_eq!(
                texts,
                ["Root", "First reply", "Nested reply", "Second reply"]
            );
        }
        assert!(f.store.comment_thread(&hex(64, 0xdead)).unwrap().is_none());
    }
}
