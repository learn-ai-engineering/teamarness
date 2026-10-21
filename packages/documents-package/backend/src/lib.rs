#![forbid(unsafe_code)]

//! The Documents Package: collaborative Markdown documents, Collections, and Comments.
//!
//! Every change is a record on the immutable record log; document text is rebuilt with Loro.
//! This crate depends only on the language-neutral Package SDK; transport and key custody
//! stay in Core.

mod backend;
pub(crate) mod body;
pub(crate) mod domain;
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

    // Verifies: AC4 Collections and Documents converge regardless of arrival order.
    #[test]
    fn metadata_converges_in_any_arrival_order() {
        let create = DomainRecord::collection_create(ALICE, "Planning").unwrap();
        let collection = hex(64, 1);
        let rename_early = DomainRecord::collection_rename(BOB, &collection, "Plans").unwrap();
        let rename_late = DomainRecord::collection_rename(ALICE, &collection, "Roadmap").unwrap();
        let archive = DomainRecord::collection_archive(ALICE, &collection).unwrap();
        let restore = DomainRecord::collection_restore(BOB, &collection).unwrap();
        let document = DomainRecord::document_create(BOB, &collection, "Q4").unwrap();
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
            &DomainRecord::document_create(ALICE, &source, "Doc").unwrap(),
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
        let document = DomainRecord::document_create(ALICE, &collection, "Doc").unwrap();
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
        assert_eq!(
            store
                .retry_deferred(&hex(64, 2), &document.encode())
                .unwrap()
                .result,
            IngestResult::Applied
        );
        assert_eq!(store.documents().unwrap().len(), 1);
    }

    /// A body edit that arrives before its Document waits, then surfaces once the Document's
    /// create record is applied; the edit rows are findable per Document afterwards.
    #[test]
    fn edits_wait_for_their_document_and_surface_when_it_arrives() {
        let f = fixture();
        let collection = hex(64, 1);
        let document = hex(64, 2);
        let edit = DomainRecord::document_edit(BOB, &document, vec![1, 2, 3]).unwrap();
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
        let ingested = f
            .store
            .ingest_with_commit(
                &hex(64, 2),
                &hex(82, 2),
                ALICE,
                "document.create",
                "sig",
                &DomainRecord::document_create(ALICE, &collection, "Doc")
                    .unwrap()
                    .encode(),
                b"sealed",
                Some(&hex(50, 2)),
                &|| {},
            )
            .unwrap();
        assert_eq!(ingested.result, IngestResult::Applied);
        assert_eq!(ingested.edits, vec![(document.clone(), vec![1, 2, 3])]);
        assert_eq!(f.store.edit_records(Some(&document)).unwrap().len(), 1);
        assert_eq!(
            f.store.edit_records(None).unwrap()[0].target.as_deref(),
            Some(document.as_str())
        );
        assert!(f.store.edit_records(Some("not-an-id")).is_err());
        let direct = DomainRecord::document_edit(ALICE, &document, vec![9]).unwrap();
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
                &DomainRecord::document_create(ALICE, &collection, "D").unwrap(),
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
                &DomainRecord::document_create(ALICE, &collection, "Other").unwrap(),
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
}
