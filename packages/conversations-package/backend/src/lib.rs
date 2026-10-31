#![forbid(unsafe_code)]

//! The Conversations Package domain and durable projection.  This crate deliberately
//! depends only on the language-neutral Package SDK; transport and key custody stay in Core.

mod backend;
pub(crate) mod domain;
mod guide;
pub(crate) mod store;

pub use backend::{ConversationsActive, router};
pub use domain::{DomainError, DomainRecord, GENERAL_ID, PAGE_SIZE, decode_id};
pub use store::{
    ChannelView, ConversationStore, IngestResult, MessageView, StoreError, StoredRecord,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MAX_MARKDOWN, hex_id};
    use crate::store::StoreError;
    use rusqlite::Connection;
    use std::collections::BTreeMap;

    #[test]
    fn canonical_roundtrip_and_bounds() {
        let x = DomainRecord::message_post([1; 16], "author", "**hello**").unwrap();
        assert_eq!(DomainRecord::decode(&x.encode()).unwrap(), x);
        assert!(DomainRecord::channel_create_with_id([2; 16], "a", "Bad Slug").is_err());
        assert!(DomainRecord::channel_create_with_id([2; 16], "a", "bad_slug").is_err());
        assert!(DomainRecord::message_post([1; 16], "a", "x".repeat(MAX_MARKDOWN + 1)).is_err());
    }
    #[test]
    fn general_channel_accepts_messages_but_cannot_be_hidden() {
        let message = DomainRecord::message_post(GENERAL_ID, "author", "hello").unwrap();
        assert_eq!(DomainRecord::decode(&message.encode()).unwrap(), message);
        assert!(DomainRecord::channel_hide(GENERAL_ID, "author").is_err());
    }

    #[test]
    fn general_and_duplicate_slug_order_are_stable() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let a = DomainRecord::channel_create_with_id([1; 16], "author", "team").unwrap();
        let b = DomainRecord::channel_create_with_id([2; 16], "author", "team").unwrap();
        let c = DomainRecord::channel_create_with_id([3; 16], "author", "team-02020202").unwrap();
        for (d, o) in [(&a, "01"), (&b, "02"), (&c, "03")] {
            let c = d.encode();
            assert_eq!(
                s.ingest(
                    &hex_id(d.id()),
                    o,
                    "author",
                    d.kind(),
                    "sig",
                    &c,
                    b"sealed",
                    None
                )
                .unwrap(),
                IngestResult::Applied
            )
        }
        let channels = s.channels().unwrap();
        assert!(channels.iter().any(|channel| channel.slug == "team"));
        assert!(
            channels
                .iter()
                .any(|channel| channel.slug == "team-0202020202020202")
        );
        assert!(
            channels
                .iter()
                .any(|channel| channel.slug == "team-02020202")
        );

        let reversed = ConversationStore::open(d.path().join("reversed.sqlite")).unwrap();
        for (record, order) in [(&c, "03"), (&b, "02"), (&a, "01")] {
            reversed
                .ingest(
                    &hex_id(record.id()),
                    order,
                    "author",
                    record.kind(),
                    "sig",
                    &record.encode(),
                    b"sealed",
                    None,
                )
                .unwrap();
        }
        let presentation = |store: &ConversationStore| {
            store
                .channels()
                .unwrap()
                .into_iter()
                .filter(|channel| channel.slug.starts_with("team"))
                .map(|channel| (channel.id, channel.slug))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(presentation(&s), presentation(&reversed));
    }
    #[test]
    fn duplicate_slug_suffix_stays_within_slug_limit() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let base = "a".repeat(48);
        let first = DomainRecord::channel_create_with_id([4; 16], "author", &base).unwrap();
        let second = DomainRecord::channel_create_with_id([5; 16], "author", &base).unwrap();
        for (record, order_key) in [(&first, "01"), (&second, "02")] {
            assert_eq!(
                store
                    .ingest(
                        &hex_id(record.id()),
                        order_key,
                        "author",
                        record.kind(),
                        "signature",
                        &record.encode(),
                        b"sealed",
                        None,
                    )
                    .unwrap(),
                IngestResult::Applied
            );
        }

        let slugs = store
            .channels()
            .unwrap()
            .into_iter()
            .filter(|channel| channel.id != hex_id(GENERAL_ID))
            .map(|channel| channel.slug)
            .collect::<Vec<_>>();
        assert_eq!(slugs.len(), 2);
        assert!(slugs.iter().all(|slug| slug.len() <= 48));
        assert!(slugs.contains(&base));
        assert!(
            slugs
                .iter()
                .any(|slug| slug.len() == 48 && slug.ends_with("-05050505"))
        );
    }

    /// Every slug the duplicate-slug path synthesizes travels in the Package's own
    /// `conversations.snapshot` response, and a response the Package UI contract refuses fails the
    /// Mount for good. So the projection's slugs obey the contract grammar even when truncating to
    /// fit the length bound lands on a hyphen.
    // Verifies: TEAM.SYS.CONV-R05
    #[test]
    fn duplicate_slug_suffix_obeys_the_contract_grammar() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        // 40 characters, contract-valid, whose 39th character is the hyphen the eight-character
        // channel-id suffix truncates onto.
        let base = format!("{}-b", "a".repeat(38));
        let first = DomainRecord::channel_create_with_id([4; 16], "author", &base).unwrap();
        let second = DomainRecord::channel_create_with_id([5; 16], "author", &base).unwrap();
        for (record, order_key) in [(&first, "01"), (&second, "02")] {
            assert_eq!(
                store
                    .ingest(
                        &hex_id(record.id()),
                        order_key,
                        "author",
                        record.kind(),
                        "signature",
                        &record.encode(),
                        b"sealed",
                        None,
                    )
                    .unwrap(),
                IngestResult::Applied
            );
        }

        let slugs = store
            .channels()
            .unwrap()
            .into_iter()
            .map(|channel| channel.slug)
            .collect::<Vec<_>>();
        assert_eq!(slugs.len(), 3);
        for slug in &slugs {
            assert!(
                crate::domain::validate_slug(slug).is_ok(),
                "the projection presented {slug:?}, which its own decoder and the UI contract \
                 pattern ^[a-z0-9]+(-[a-z0-9]+)*$ both refuse"
            );
        }
        assert_eq!(slugs.iter().filter(|slug| **slug == base).count(), 1);
    }

    #[test]
    fn persistence_contains_sealed_bytes_and_opaque_indexes_only() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.sqlite");
        let s = ConversationStore::open(&path).unwrap();
        let c = DomainRecord::channel_create_with_id([3; 16], "alice", "private").unwrap();
        let canonical = c.encode();
        s.ingest(
            "aabb",
            "0001",
            "alice",
            c.kind(),
            "signature",
            &canonical,
            b"ciphertext",
            Some("cursor"),
        )
        .unwrap();
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes
                .windows(b"ciphertext".len())
                .any(|w| w == b"ciphertext")
        );
        assert!(!bytes.windows(b"private".len()).any(|w| w == b"private"));
        assert!(!bytes.windows(b"TCONV".len()).any(|w| w == b"TCONV"));
        let reopened = ConversationStore::open(&path).unwrap();
        assert_eq!(reopened.cursor().unwrap().as_deref(), Some("cursor"));
        assert_eq!(
            reopened
                .channels()
                .unwrap()
                .iter()
                .find(|x| x.id == hex_id([3; 16]))
                .unwrap()
                .slug,
            "channel-03030303"
        );
    }

    #[test]
    fn message_before_channel_is_deferred_then_projected() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let message = DomainRecord::message_post([8; 16], "alice", "hello").unwrap();
        assert_eq!(
            store
                .ingest(
                    "message-record",
                    "01",
                    "alice",
                    message.kind(),
                    "s",
                    &message.encode(),
                    b"sealed-message",
                    None,
                )
                .unwrap(),
            IngestResult::Deferred("waiting_for_channel".into())
        );
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );

        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel-record",
                "02",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"sealed-channel",
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deferred_message_recovery_uses_reopened_sealed_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("x.sqlite");
        let message = DomainRecord::message_post([8; 16], "alice", "restart secret").unwrap();
        {
            let store = ConversationStore::open(&path).unwrap();
            store
                .ingest(
                    "message-record",
                    "01",
                    "alice",
                    message.kind(),
                    "s",
                    &message.encode(),
                    b"sealed-message",
                    None,
                )
                .unwrap();
        }
        let store = ConversationStore::open(&path).unwrap();
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel-record",
                "02",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"sealed-channel",
                None,
            )
            .unwrap();
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );
        let deferred = store.deferred_records().unwrap();
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].sealed_record, b"sealed-message");
        assert_eq!(
            store
                .retry_deferred("message-record", &message.encode())
                .unwrap(),
            IngestResult::Applied
        );
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        let bytes = std::fs::read(path).unwrap();
        assert!(
            !bytes
                .windows(b"restart secret".len())
                .any(|window| window == b"restart secret")
        );
    }

    #[test]
    fn earliest_hide_removes_later_messages_but_keeps_earlier_history() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel",
                "01",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"c",
                None,
            )
            .unwrap();
        let later = DomainRecord::message_post([8; 16], "bob", "later").unwrap();
        store
            .ingest(
                "later",
                "04",
                "bob",
                later.kind(),
                "s",
                &later.encode(),
                b"l",
                None,
            )
            .unwrap();
        let hide = DomainRecord::channel_hide([8; 16], "alice").unwrap();
        store
            .ingest(
                "hide",
                "03",
                "alice",
                hide.kind(),
                "s",
                &hide.encode(),
                b"h",
                None,
            )
            .unwrap();
        assert!(!store.channel_is_active(&hex_id([8; 16])).unwrap());
        assert!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .is_empty()
        );

        let earlier = DomainRecord::message_post([8; 16], "bob", "earlier").unwrap();
        assert_eq!(
            store
                .ingest(
                    "earlier",
                    "02",
                    "bob",
                    earlier.kind(),
                    "s",
                    &earlier.encode(),
                    b"e",
                    None
                )
                .unwrap(),
            IngestResult::Applied
        );
        assert_eq!(
            store
                .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
        let after = DomainRecord::message_post([8; 16], "bob", "after").unwrap();
        assert_eq!(
            store
                .ingest(
                    "after",
                    "05",
                    "bob",
                    after.kind(),
                    "s",
                    &after.encode(),
                    b"a",
                    None
                )
                .unwrap(),
            IngestResult::Quarantined("channel_hidden".into())
        );
    }

    #[test]
    fn deferred_hide_and_messages_replay_in_order_after_channel_arrives() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let earlier = DomainRecord::message_post([8; 16], "bob", "earlier").unwrap();
        let hide = DomainRecord::channel_hide([8; 16], "alice").unwrap();
        let later = DomainRecord::message_post([8; 16], "bob", "later").unwrap();
        for (record_id, order, author, record) in [
            ("later", "04", "bob", &later),
            ("hide", "03", "alice", &hide),
            ("earlier", "02", "bob", &earlier),
        ] {
            assert!(matches!(
                store
                    .ingest(
                        record_id,
                        order,
                        author,
                        record.kind(),
                        "s",
                        &record.encode(),
                        record_id.as_bytes(),
                        None,
                    )
                    .unwrap(),
                IngestResult::Deferred(_)
            ));
        }
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "channel",
                "05",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"channel",
                None,
            )
            .unwrap();

        let rows = store
            .message_rows(&hex_id([8; 16]), None, PAGE_SIZE)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, hex_id(earlier.id()));
        assert!(!store.channel_is_active(&hex_id([8; 16])).unwrap());
    }

    #[test]
    fn scale_fixture_keeps_channel_and_page_queries_bounded() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("scale.sqlite")).unwrap();
        let mut inner = s.inner.lock().unwrap();
        let tx = inner.conn.transaction().unwrap();
        for channel in 1..=100_u32 {
            let channel_id = format!("{channel:032x}");
            tx.execute(
                "INSERT INTO channels(channel_id,creator,create_order,hidden_order) VALUES(?1,'author',?2,NULL)",
                rusqlite::params![channel_id, format!("{channel:082x}")],
            ).unwrap();
        }
        for message in 0..100_000_u32 {
            let channel = (message % 100) + 1;
            let channel_id = format!("{channel:032x}");
            let message_id = format!("{message:032x}");
            let record_id = format!("{message:064x}");
            let order = format!("{message:082x}");
            tx.execute(
                "INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES(?1,?2,'author','message.post','signature',X'7365616c6564',NULL,?3,NULL)",
                rusqlite::params![record_id, order, i64::from(message) + 1],
            ).unwrap();
            tx.execute(
                "INSERT INTO messages(message_id,channel_id,order_key,author,record_id,ingestion_ordinal) VALUES(?1,?2,?3,'author',?4,?5)",
                rusqlite::params![message_id, channel_id, order, record_id, i64::from(message) + 1],
            ).unwrap();
            tx.execute(
                "INSERT INTO history_changes(revision,channel_id,message_id,order_key,kind) VALUES(?1,?2,?3,?4,'insert')",
                rusqlite::params![i64::from(message) + 1, channel_id, message_id, order],
            ).unwrap();
        }
        tx.execute("UPDATE history_state SET revision=100000", [])
            .unwrap();
        tx.commit().unwrap();
        let detail: String = inner.conn.query_row(
            "EXPLAIN QUERY PLAN SELECT revision,channel_id,message_id,order_key,kind FROM history_changes WHERE channel_id=?1 AND revision>?2 AND revision<=?3 ORDER BY revision LIMIT ?4",
            rusqlite::params!["00000000000000000000000000000001", 0_i64, 100_000_i64, 33_i64],
            |row| row.get(3),
        ).unwrap();
        assert!(
            detail.contains("history_changes_channel_revision"),
            "{detail}"
        );
        drop(inner);
        let (epoch, revision) = s.history_head().unwrap();
        assert_eq!(revision, 100_000);
        let channel_id = "00000000000000000000000000000001";
        let mut after = 0;
        let mut seen = 0;
        loop {
            let batch = s
                .history_changes(channel_id, &epoch, after, 32)
                .unwrap()
                .unwrap();
            assert!(batch.changes.len() <= 32);
            assert_eq!(batch.high_water, 100_000);
            if batch.changes.is_empty() {
                break;
            }
            assert!(
                batch
                    .changes
                    .iter()
                    .all(|change| change.channel_id == channel_id)
            );
            assert!(batch.continuation > after);
            seen += batch.changes.len();
            after = batch.continuation;
        }
        assert_eq!(seen, 1_000);
        assert_eq!(s.channels().unwrap().len(), 101);
        for channel in 1..=100_u32 {
            let rows = s
                .message_rows(&format!("{channel:032x}"), None, PAGE_SIZE)
                .unwrap();
            assert_eq!(rows.len(), PAGE_SIZE);
        }
    }

    #[test]
    fn unread_defaults_to_all_messages_before_first_read() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let message = DomainRecord::message_post(GENERAL_ID, "alice", "one").unwrap();
        s.ingest(
            &hex_id(message.id()),
            "01",
            "alice",
            message.kind(),
            "s",
            &message.encode(),
            b"x",
            None,
        )
        .unwrap();

        assert_eq!(
            s.channels()
                .unwrap()
                .iter()
                .find(|channel| channel.id == hex_id(GENERAL_ID))
                .unwrap()
                .unread_count,
            1
        );
    }

    #[test]
    fn unread_uses_ingestion_not_sort_order() {
        let d = tempfile::tempdir().unwrap();
        let s = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let c = DomainRecord::channel_create_with_id([1; 16], "a", "team").unwrap();
        let cb = c.encode();
        s.ingest(&hex_id(c.id()), "10", "a", c.kind(), "s", &cb, b"x", None)
            .unwrap();
        let m = DomainRecord::message_post([1; 16], "a", "one").unwrap();
        let mb = m.encode();
        s.ingest(&hex_id(m.id()), "01", "a", m.kind(), "s", &mb, b"y", None)
            .unwrap();
        s.mark_read(&hex_id([1; 16])).unwrap();
        let m2 = DomainRecord::message_post([1; 16], "b", "two").unwrap();
        let mb = m2.encode();
        s.ingest(&hex_id(m2.id()), "00", "b", m2.kind(), "s", &mb, b"z", None)
            .unwrap();
        assert_eq!(
            s.channels()
                .unwrap()
                .iter()
                .find(|x| x.id == hex_id([1; 16]))
                .unwrap()
                .unread_count,
            1
        );
    }

    #[test]
    fn projection_changes_track_late_and_deferred_messages_in_projection_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("changes.sqlite")).unwrap();
        let channel = hex_id([8; 16]);
        let initial = store.history_head().unwrap();
        let deferred = DomainRecord::message_post([8; 16], "alice", "private body").unwrap();
        store
            .ingest(
                "deferred",
                "01",
                "alice",
                deferred.kind(),
                "s",
                &deferred.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        assert_eq!(store.history_head().unwrap(), initial);
        let create = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "create",
                "10",
                "alice",
                create.kind(),
                "s",
                &create.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        let late =
            DomainRecord::message_post([8; 16], "alice", "not stored in the journal").unwrap();
        store
            .ingest(
                "late",
                "02",
                "alice",
                late.kind(),
                "s",
                &late.encode(),
                b"sealed",
                None,
            )
            .unwrap();
        let batch = store
            .history_changes(&channel, &initial.0, initial.1, 32)
            .unwrap()
            .unwrap();
        assert_eq!(
            batch
                .changes
                .iter()
                .map(|change| (
                    &change.message_id,
                    change.order_key.as_str(),
                    change.kind.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (&hex_id(deferred.id()), "01", "insert"),
                (&hex_id(late.id()), "02", "insert")
            ]
        );
        assert_eq!(batch.high_water, 2);
        assert_eq!(batch.continuation, 2);
        assert!(
            store
                .history_changes(&channel, "wrong epoch", 0, 32)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .history_changes(&channel, &initial.0, 3, 32)
                .unwrap()
                .is_none()
        );
        drop(store);
        let reopened = ConversationStore::open(directory.path().join("changes.sqlite")).unwrap();
        assert_eq!(reopened.history_head().unwrap(), (initial.0.clone(), 2));
        assert!(
            reopened
                .history_changes(&channel, &initial.0, 2, 32)
                .unwrap()
                .unwrap()
                .changes
                .is_empty()
        );
    }

    #[test]
    fn hide_replays_removals_without_replaying_duplicate_ingests() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("hide.sqlite")).unwrap();
        let channel_id = hex_id([8; 16]);
        let channel = DomainRecord::channel_create_with_id([8; 16], "alice", "known").unwrap();
        store
            .ingest(
                "create",
                "01",
                "alice",
                channel.kind(),
                "s",
                &channel.encode(),
                b"c",
                None,
            )
            .unwrap();
        let message = DomainRecord::message_post([8; 16], "alice", "sensitive").unwrap();
        let canonical = message.encode();
        store
            .ingest(
                "message",
                "04",
                "alice",
                message.kind(),
                "s",
                &canonical,
                b"m",
                None,
            )
            .unwrap();
        let (epoch, revision) = store.history_head().unwrap();
        assert_eq!(revision, 1);
        assert_eq!(
            store
                .ingest(
                    "message",
                    "04",
                    "alice",
                    message.kind(),
                    "s",
                    &canonical,
                    b"m",
                    None
                )
                .unwrap(),
            IngestResult::Duplicate
        );
        assert_eq!(store.history_head().unwrap().1, revision);
        let hide = DomainRecord::channel_hide([8; 16], "alice").unwrap();
        store
            .ingest(
                "hide",
                "03",
                "alice",
                hide.kind(),
                "s",
                &hide.encode(),
                b"h",
                None,
            )
            .unwrap();
        let batch = store
            .history_changes(&channel_id, &epoch, 0, 1)
            .unwrap()
            .unwrap();
        assert_eq!(batch.changes[0].kind, "insert");
        assert_eq!((batch.continuation, batch.high_water), (1, 2));
        let remaining = store
            .history_changes(&channel_id, &epoch, batch.continuation, 1)
            .unwrap()
            .unwrap();
        assert_eq!(remaining.changes[0].kind, "remove");
        assert_eq!(remaining.changes[0].message_id, hex_id(message.id()));
        assert_eq!(remaining.continuation, 2);
        assert!(
            store
                .message_rows(&channel_id, None, 32)
                .unwrap()
                .is_empty()
        );
        let raw = std::fs::read(directory.path().join("hide.sqlite")).unwrap();
        assert!(
            !raw.windows(b"sensitive".len())
                .any(|bytes| bytes == b"sensitive")
        );
    }

    #[test]
    fn failed_journal_write_rolls_back_message_and_revision() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(directory.path().join("rollback.sqlite")).unwrap();
        store.inner.lock().unwrap().conn.execute_batch(
            "CREATE TRIGGER reject_history BEFORE INSERT ON history_changes BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
        ).unwrap();
        let message = DomainRecord::message_post(GENERAL_ID, "alice", "hello").unwrap();
        assert!(
            store
                .ingest(
                    "failed",
                    "01",
                    "alice",
                    message.kind(),
                    "s",
                    &message.encode(),
                    b"sealed",
                    None
                )
                .is_err()
        );
        assert_eq!(store.history_head().unwrap().1, 0);
        assert!(
            store
                .message_rows(&hex_id(GENERAL_ID), None, 32)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.record_count().unwrap(), 0);
    }

    #[test]
    fn new_store_applies_schema_v2_and_reopens() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("fresh.sqlite");
        {
            let s = ConversationStore::open(&path).unwrap();
            assert!(s.channels().is_ok());
        }
        // Reopen, migration is idempotent.
        let s = ConversationStore::open(&path).unwrap();
        assert!(!s.channels().unwrap().is_empty());
    }

    #[test]
    fn v1_upgrade_leaves_prior_messages_as_the_new_history_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("v1.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_conversations.sql"))
            .unwrap();
        conn.execute("INSERT INTO records(record_id,order_key,author,purpose,signature,sealed_record,cursor,ingestion_ordinal,quarantine_code) VALUES('old','01','alice','message.post','signature',X'7365616c6564',NULL,1,NULL)", []).unwrap();
        conn.execute("INSERT INTO messages(message_id,channel_id,order_key,author,record_id,ingestion_ordinal) VALUES('old','00000000000000000000000000000000','01','alice','old',1)", []).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);
        let store = ConversationStore::open(&path).unwrap();
        let (epoch, revision) = store.history_head().unwrap();
        assert_eq!(revision, 0);
        assert!(
            store
                .history_changes(&hex_id(GENERAL_ID), &epoch, 0, 32)
                .unwrap()
                .unwrap()
                .changes
                .is_empty()
        );
        assert_eq!(
            store
                .message_rows(&hex_id(GENERAL_ID), None, 32)
                .unwrap()
                .len(),
            1
        );
        drop(store);
        assert_eq!(
            ConversationStore::open(&path)
                .unwrap()
                .history_head()
                .unwrap(),
            (epoch, 0)
        );
    }

    #[test]
    fn legacy_v0_store_is_adopted_without_projection_loss() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("legacy.sqlite");
        // Manually create a v0-format database with all current tables.
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE records(record_id TEXT PRIMARY KEY, order_key TEXT NOT NULL, author TEXT NOT NULL, purpose TEXT NOT NULL, signature TEXT NOT NULL, sealed_record BLOB NOT NULL, cursor TEXT, ingestion_ordinal INTEGER NOT NULL, quarantine_code TEXT);
                 CREATE TABLE channels(channel_id TEXT PRIMARY KEY, creator TEXT NOT NULL, create_order TEXT NOT NULL, hidden_order TEXT);
                 CREATE TABLE messages(message_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, order_key TEXT NOT NULL, author TEXT NOT NULL, record_id TEXT NOT NULL, ingestion_ordinal INTEGER NOT NULL);
                 CREATE TABLE unread(channel_id TEXT PRIMARY KEY, last_read_ingestion_ordinal INTEGER NOT NULL);
                 CREATE TABLE metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1), cursor TEXT, next_ingestion_ordinal INTEGER NOT NULL);
                 INSERT INTO metadata(singleton,cursor,next_ingestion_ordinal) VALUES(1,NULL,0);
                 INSERT INTO channels(channel_id,creator,create_order,hidden_order) VALUES('00000000000000000000000000000000','system','',NULL);",
            )
            .unwrap();
        }
        // Adoption succeeds.
        let s = ConversationStore::open(&path).unwrap();
        assert!(s.channels().unwrap().iter().any(|c| c.slug == "general"));
        // Idempotent reopen.
        let s2 = ConversationStore::open(&path).unwrap();
        assert!(s2.channels().unwrap().iter().any(|c| c.slug == "general"));
    }

    #[test]
    fn unknown_schema_version_fails_closed() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("future.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
            c.execute_batch(
                "CREATE TABLE records(record_id TEXT PRIMARY KEY, order_key TEXT NOT NULL, author TEXT NOT NULL, purpose TEXT NOT NULL, signature TEXT NOT NULL, sealed_record BLOB NOT NULL, cursor TEXT, ingestion_ordinal INTEGER NOT NULL, quarantine_code TEXT);
                 CREATE TABLE channels(channel_id TEXT PRIMARY KEY, creator TEXT NOT NULL, create_order TEXT NOT NULL, hidden_order TEXT);
                 CREATE TABLE messages(message_id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, order_key TEXT NOT NULL, author TEXT NOT NULL, record_id TEXT NOT NULL, ingestion_ordinal INTEGER NOT NULL);
                 CREATE TABLE unread(channel_id TEXT PRIMARY KEY, last_read_ingestion_ordinal INTEGER NOT NULL);
                 CREATE TABLE metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1), cursor TEXT, next_ingestion_ordinal INTEGER NOT NULL);",
            )
            .unwrap();
        }
        let err = ConversationStore::open(&path).unwrap_err();
        assert!(matches!(err, StoreError::UnsupportedSchema));
    }

    #[test]
    fn malformed_legacy_schema_fails_closed() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("broken.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            // v0 with only one table — validation must reject it.
            c.execute_batch("CREATE TABLE records(record_id TEXT PRIMARY KEY);")
                .unwrap();
        }
        let err = ConversationStore::open(&path).unwrap_err();
        match err {
            StoreError::LegacyValidation(_) => {}
            other => panic!("expected LegacyValidation, got {other:?}"),
        }
    }
}
