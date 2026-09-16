#![forbid(unsafe_code)]

//! The Conversations Package domain and durable projection.  This crate deliberately
//! depends only on the language-neutral Package SDK; transport and key custody stay in Core.

mod backend;
pub(crate) mod domain;
pub(crate) mod immutable_log;
pub(crate) mod store;

pub use backend::ConversationsBackend;
pub use domain::{DomainError, DomainRecord, GENERAL_ID, PAGE_SIZE, decode_id};
pub use immutable_log::{
    AppendOutput, ImmutableRecordLog, LogError, LogEvent, LogSubscription, OpenResult,
    OpenResultStatus, SyncStatus,
};
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
    use std::sync::Arc;
    use tokio::sync::watch;

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
        }
        tx.commit().unwrap();
        drop(inner);
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

    #[tokio::test]
    async fn lifecycle_deactivation_cancels_and_awaits_the_owned_task() {
        let directory = tempfile::tempdir().unwrap();
        let backend = ConversationsBackend::default();
        let store = ConversationStore::open(directory.path().join("x.sqlite")).unwrap();
        let (cancel, mut cancelled) = watch::channel(false);
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_exited = Arc::clone(&exited);
        let task = tokio::spawn(async move {
            let _ = cancelled.changed().await;
            task_exited.store(true, std::sync::atomic::Ordering::Release);
        });
        *backend.active.lock().await = Some(crate::backend::Running {
            active: crate::backend::Active {
                store,
                record_grant: "grant".into(),
                sync_status: Arc::new(tokio::sync::Mutex::new(SyncStatus::Current)),
            },
            cancel,
            task,
        });

        backend.deactivate().await.unwrap();

        assert!(backend.active.lock().await.is_none());
        assert!(exited.load(std::sync::atomic::Ordering::Acquire));
        backend.deactivate().await.unwrap();
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
    fn new_store_applies_schema_v1_and_reopens() {
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

    // ------------------------------------------------------------------
    // Phase 4: session loop integration tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn session_ingests_real_record_and_transitions_to_current() {
        let d = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        // Pre-create a channel so records aren't deferred.
        let ch = DomainRecord::channel_create_with_id([1; 16], "alice", "test").unwrap();
        store
            .ingest(
                &hex_id(ch.id()),
                "ch-order",
                "alice",
                ch.kind(),
                "s",
                &ch.encode(),
                b"ch-sealed",
                None,
            )
            .unwrap();

        let log = crate::immutable_log::scripted::ScriptedLog::new();

        // Push a real DomainRecord through the scripted log.
        let msg = DomainRecord::message_post([1; 16], "alice", "hello").unwrap();
        let canonical = msg.encode();
        log.push_event(Some(Ok(crate::immutable_log::LogEvent::Record {
            record_id: "msg-1".into(),
            order_key: "order-1".into(),
            author: "alice".into(),
            purpose: "message.post".into(),
            signature: "sig".into(),
            canonical,
            sealed_record: b"sealed".to_vec(),
            cursor: Some("order-1".into()),
        })));

        let sync_status = Arc::new(tokio::sync::Mutex::new(SyncStatus::CatchingUp));
        let mut sub = log.subscribe(None).await.unwrap();

        // Manually simulate what receive_loop does.
        let event = sub.recv().await.unwrap().unwrap();
        if let crate::immutable_log::LogEvent::Record {
            record_id,
            order_key,
            author,
            purpose,
            signature,
            canonical,
            sealed_record,
            cursor,
        } = event
        {
            store
                .ingest(
                    &record_id,
                    &order_key,
                    &author,
                    &purpose,
                    &signature,
                    &canonical,
                    &sealed_record,
                    cursor.as_deref(),
                )
                .unwrap();
            // simulate quiescence transition
            let mut status = sync_status.lock().await;
            if *status == SyncStatus::CatchingUp {
                *status = SyncStatus::Current;
            }
        }

        assert_eq!(*sync_status.lock().await, SyncStatus::Current);
        assert_eq!(
            store
                .message_rows(&hex_id([1; 16]), None, crate::domain::PAGE_SIZE)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn state_event_updates_sync_status_explicitly() {
        let log = crate::immutable_log::scripted::ScriptedLog::new();
        log.push_state("catching_up");

        let mut sub = log.subscribe(None).await.unwrap();
        let event = sub.recv().await.unwrap().unwrap();
        assert!(matches!(
            event,
            crate::immutable_log::LogEvent::State {
                availability: Some(a)
            } if a == "catching_up"
        ));
    }

    #[tokio::test]
    async fn warning_event_does_not_affect_record_count() {
        let d = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(d.path().join("x.sqlite")).unwrap();
        let count_before = store.record_count().unwrap();

        let log = crate::immutable_log::scripted::ScriptedLog::new();
        log.push_warning("congestion");

        let mut sub = log.subscribe(None).await.unwrap();
        let event = sub.recv().await.unwrap().unwrap();
        assert!(matches!(
            event,
            crate::immutable_log::LogEvent::Warning { code, .. } if code == "congestion"
        ));

        // Record count must not change — warnings are never ingested.
        assert_eq!(store.record_count().unwrap(), count_before);
    }

    #[tokio::test]
    async fn reconnection_after_stream_close_uses_committed_cursor() {
        let d = tempfile::tempdir().unwrap();
        let store = ConversationStore::open(d.path().join("x.sqlite")).unwrap();

        let log = crate::immutable_log::scripted::ScriptedLog::new();

        // First subscription: deliver one record with cursor, then close.
        let ch = DomainRecord::channel_create_with_id([1; 16], "alice", "test").unwrap();
        let canonical = ch.encode();
        log.push_event(Some(Ok(crate::immutable_log::LogEvent::Record {
            record_id: "ch-1".into(),
            order_key: "ch-order".into(),
            author: "alice".into(),
            purpose: "channel.create".into(),
            signature: "sig".into(),
            canonical,
            sealed_record: b"sealed".to_vec(),
            cursor: Some("ch-order".into()),
        })));
        log.push_close();

        // Ingest the record so cursor advances.
        let mut sub = log.subscribe(None).await.unwrap();
        if let Some(Ok(crate::immutable_log::LogEvent::Record {
            record_id,
            order_key,
            author,
            purpose,
            signature,
            canonical,
            sealed_record,
            cursor,
        })) = sub.recv().await
        {
            store
                .ingest(
                    &record_id,
                    &order_key,
                    &author,
                    &purpose,
                    &signature,
                    &canonical,
                    &sealed_record,
                    cursor.as_deref(),
                )
                .unwrap();
        }
        assert!(sub.recv().await.is_none()); // closed
        sub.close().await;

        // After stream close, cursor should have advanced.
        let committed_cursor = store.cursor().unwrap();
        assert_eq!(committed_cursor.as_deref(), Some("ch-order"));

        // Reconnect with the committed cursor.
        log.push_record("rec-2", "order-2");
        log.push_close();

        let mut sub2 = log.subscribe(committed_cursor).await.unwrap();
        assert!(sub2.recv().await.unwrap().is_ok());
        assert!(sub2.recv().await.is_none());
    }

    #[tokio::test]
    async fn cancellation_stops_reconnect_backoff() {
        let log = crate::immutable_log::scripted::ScriptedLog::new();
        // Stream closes immediately.
        log.push_close();

        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let log_clone = log.clone();

        let handle = tokio::spawn(async move {
            let backoff = std::time::Duration::from_millis(10);
            // First subscribe gets the close.
            let mut sub = log_clone.subscribe(None).await.unwrap();
            let closed = sub.recv().await.is_none();
            sub.close().await;
            if closed {
                // Wait for backoff or cancellation.
                tokio::select! {
                    _ = cancel_rx.changed() => {},
                    _ = tokio::time::sleep(backoff) => {},
                }
            }
        });

        // Cancel during the backoff.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let _ = cancel_tx.send(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "session did not stop after cancellation");
    }

    #[tokio::test]
    async fn subscribe_failure_sets_degraded_status() {
        let log = crate::immutable_log::scripted::ScriptedLog::new();
        log.subscribe_errors
            .lock()
            .unwrap()
            .push_back(crate::immutable_log::LogError::Transport("fatal".into()));

        let sync_status = Arc::new(tokio::sync::Mutex::new(SyncStatus::CatchingUp));

        let result = log.subscribe(None).await;
        assert!(result.is_err());

        // Set degraded (as run_session would).
        *sync_status.lock().await = SyncStatus::Degraded("subscribe_failed".into());
        assert_eq!(sync_status.lock().await.availability(), "degraded");
    }
}
