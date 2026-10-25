use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use super::*;
use crate::domain::{CHUNK_BYTES, LIST_PAGE, MAX_FILE_BYTES, MAX_PATH_BYTES};

const START: i64 = 1_793_000_000_000;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    now: Arc<AtomicI64>,
    store: ExhibitStore,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_owned();
    let now = Arc::new(AtomicI64::new(START));
    let clock_now = now.clone();
    let store =
        ExhibitStore::open(&root, Arc::new(move || clock_now.load(Ordering::SeqCst))).unwrap();
    Fixture {
        _directory: directory,
        root,
        now,
        store,
    }
}

impl Fixture {
    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    /// Uploads `bytes` completely and returns its hash.
    fn upload(&self, bytes: &[u8]) -> String {
        let sha256 = sha(bytes);
        let size = bytes.len() as u64;
        let count = chunk_count(size);
        for index in 0..count {
            let complete = self
                .store
                .put_chunk(&sha256, size, index, count, chunk(bytes, index))
                .unwrap();
            assert_eq!(complete, index + 1 == count);
        }
        sha256
    }

    fn update(
        &self,
        id: &str,
        files: &[(&str, &str)],
        expected: Option<u64>,
    ) -> Result<u64, ExhibitsError> {
        self.store.update_draft(&DraftUpdate {
            author: author(),
            id: id.into(),
            title: format!("Title of {id}"),
            entry: files
                .first()
                .map_or("index.html", |(path, _)| path)
                .to_owned(),
            files: files
                .iter()
                .map(|(path, sha)| ((*path).to_owned(), (*sha).to_owned()))
                .collect(),
            expected_revision: expected,
        })
    }

    fn complete(&self, sha256: &str) -> bool {
        self.root.join(CONTENT_DIRECTORY).join(sha256).is_file()
    }

    fn partial(&self, sha256: &str) -> bool {
        self.root.join("content/partial").join(sha256).exists()
    }
}

fn author() -> Author {
    Author {
        kind: "agent",
        identity: "c".repeat(64),
    }
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn chunk(bytes: &[u8], index: u64) -> &[u8] {
    let start = usize::try_from(index).unwrap() * CHUNK_BYTES;
    &bytes[start..(start + CHUNK_BYTES).min(bytes.len())]
}

fn pattern(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
fn a_repeated_chunk_is_idempotent_and_the_last_chunk_promotes_the_file() {
    let fixture = fixture();
    let bytes = pattern(CHUNK_BYTES * 2 + 10, 1);
    let sha256 = sha(&bytes);
    let size = bytes.len() as u64;
    let store = &fixture.store;
    assert!(
        !store
            .put_chunk(&sha256, size, 0, 3, chunk(&bytes, 0))
            .unwrap()
    );
    assert!(
        !store
            .put_chunk(&sha256, size, 0, 3, chunk(&bytes, 0))
            .unwrap()
    );
    assert_eq!(
        store.missing(std::slice::from_ref(&sha256)).unwrap(),
        vec![(sha256.clone(), Missing::Chunks(vec![1, 2]))]
    );
    // Out of order: the final chunk index arrives before the middle one.
    assert!(
        !store
            .put_chunk(&sha256, size, 2, 3, chunk(&bytes, 2))
            .unwrap()
    );
    assert!(!fixture.complete(&sha256));
    assert!(
        store
            .put_chunk(&sha256, size, 1, 3, chunk(&bytes, 1))
            .unwrap()
    );
    assert!(fixture.complete(&sha256));
    assert!(!fixture.partial(&sha256));
    let mode = fs::metadata(fixture.root.join(CONTENT_DIRECTORY).join(&sha256))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o400);
    assert_eq!(
        fs::read(fixture.root.join(CONTENT_DIRECTORY).join(&sha256)).unwrap(),
        bytes
    );
    // A repeat after completion writes nothing and still reports complete.
    assert!(
        store
            .put_chunk(&sha256, size, 0, 3, chunk(&bytes, 0))
            .unwrap()
    );
    assert!(store.missing(&[sha256]).unwrap().is_empty());
}

#[test]
fn a_corrupted_file_is_not_promoted() {
    let fixture = fixture();
    let bytes = pattern(CHUNK_BYTES + 5, 2);
    let declared = sha(b"something else entirely");
    let size = bytes.len() as u64;
    assert!(
        !fixture
            .store
            .put_chunk(&declared, size, 0, 2, chunk(&bytes, 0))
            .unwrap()
    );
    assert_eq!(
        fixture
            .store
            .put_chunk(&declared, size, 1, 2, chunk(&bytes, 1)),
        Err(ExhibitsError::ContentMismatch)
    );
    assert!(!fixture.complete(&declared));
    assert!(!fixture.partial(&declared));
    assert!(
        read_directory(&fixture.root.join("content/tmp"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .missing(std::slice::from_ref(&declared))
            .unwrap(),
        vec![(declared, Missing::All)]
    );
}

#[test]
fn a_wrong_chunk_size_is_refused() {
    let fixture = fixture();
    let bytes = pattern(CHUNK_BYTES + 5, 3);
    let sha256 = sha(&bytes);
    let size = bytes.len() as u64;
    let short = &bytes[..CHUNK_BYTES - 1];
    assert_eq!(
        fixture.store.put_chunk(&sha256, size, 0, 2, short),
        Err(ExhibitsError::InvalidChunk {
            reason: "chunk_size"
        })
    );
    assert_eq!(
        fixture.store.put_chunk(&sha256, size, 1, 2, &bytes[..6]),
        Err(ExhibitsError::InvalidChunk {
            reason: "chunk_size"
        })
    );
    assert_eq!(
        fixture
            .store
            .put_chunk(&sha256, size, 0, 3, chunk(&bytes, 0)),
        Err(ExhibitsError::InvalidChunk {
            reason: "chunk_count"
        })
    );
    assert!(!fixture.partial(&sha256));
}

#[test]
fn a_file_over_16_mib_is_refused_at_the_first_chunk() {
    let fixture = fixture();
    let size = MAX_FILE_BYTES + 1;
    assert_eq!(
        fixture.store.put_chunk(
            &"a".repeat(64),
            size,
            0,
            chunk_count(size),
            &vec![0; CHUNK_BYTES]
        ),
        Err(ExhibitsError::TooLarge { limit: "file" })
    );
}

#[test]
fn an_empty_file_is_one_empty_chunk() {
    let fixture = fixture();
    let sha256 = fixture.upload(b"");
    assert!(fixture.complete(&sha256));
    assert_eq!(
        fixture.store.read_chunk(&sha256, 0).unwrap(),
        (Vec::new(), 1)
    );
}

#[test]
fn a_new_declared_size_restarts_the_upload() {
    let fixture = fixture();
    let bytes = pattern(CHUNK_BYTES + 5, 4);
    let sha256 = sha(&bytes);
    fixture
        .store
        .put_chunk(&sha256, 3 * CHUNK_BYTES as u64, 0, 3, chunk(&bytes, 0))
        .unwrap();
    assert!(
        fixture
            .store
            .put_chunk(&sha256, bytes.len() as u64, 1, 2, chunk(&bytes, 1))
            .is_ok()
    );
    assert_eq!(
        fixture
            .store
            .missing(std::slice::from_ref(&sha256))
            .unwrap(),
        vec![(sha256, Missing::Chunks(vec![0]))]
    );
}

#[test]
fn files_missing_reports_all_or_chunks_and_bounds_the_query() {
    let fixture = fixture();
    let complete = fixture.upload(b"<p>done</p>");
    let unknown = "f".repeat(64);
    let missing = fixture
        .store
        .missing(&[complete, unknown.clone(), unknown.clone()])
        .unwrap();
    assert_eq!(missing, vec![(unknown, Missing::All)]);
    assert_eq!(
        fixture.store.missing(&["xyz".into()]),
        Err(ExhibitsError::InvalidRequest)
    );
    assert_eq!(
        fixture.store.missing(&vec!["a".repeat(64); 257]),
        Err(ExhibitsError::InvalidRequest)
    );
}

#[test]
fn files_missing_assembles_a_partial_whose_chunks_are_all_present() {
    let fixture = fixture();
    // Every chunk is on disk but assembly never ran, as after a crash during assembly: the
    // last chunk is placed by hand where `file.put` would have put it.
    let stage = |bytes: &[u8]| {
        let sha256 = sha(bytes);
        let size = bytes.len() as u64;
        assert!(
            !fixture
                .store
                .put_chunk(&sha256, size, 0, 2, chunk(bytes, 0))
                .unwrap()
        );
        let directory = fixture.root.join("content/partial").join(&sha256);
        fs::write(directory.join("1"), chunk(bytes, 1)).unwrap();
        sha256
    };
    let good = pattern(CHUNK_BYTES + 3, 11);
    let good_sha = stage(&good);
    // A partial whose chunks are complete but whose bytes do not match the declared hash.
    let mut wrong = pattern(CHUNK_BYTES + 3, 12);
    let wrong_sha = sha(&wrong);
    assert!(
        !fixture
            .store
            .put_chunk(&wrong_sha, wrong.len() as u64, 0, 2, chunk(&wrong, 0))
            .unwrap()
    );
    wrong[CHUNK_BYTES] ^= 0xff;
    fs::write(
        fixture
            .root
            .join("content/partial")
            .join(&wrong_sha)
            .join("1"),
        chunk(&wrong, 1),
    )
    .unwrap();

    let missing = fixture
        .store
        .missing(&[good_sha.clone(), wrong_sha.clone()])
        .unwrap();
    assert_eq!(missing, vec![(wrong_sha.clone(), Missing::All)]);
    assert!(fixture.complete(&good_sha));
    assert!(!fixture.partial(&good_sha));
    assert!(!fixture.complete(&wrong_sha));
    assert!(!fixture.partial(&wrong_sha));
    assert_eq!(
        fixture.store.read_chunk(&good_sha, 1).unwrap().0,
        chunk(&good, 1)
    );
    assert_eq!(
        fixture
            .store
            .missing(std::slice::from_ref(&wrong_sha))
            .unwrap(),
        vec![(wrong_sha, Missing::All)]
    );
}

#[test]
fn file_read_returns_each_chunk_byte_for_byte() {
    let fixture = fixture();
    let bytes = pattern(CHUNK_BYTES * 2 + 1, 5);
    let sha256 = fixture.upload(&bytes);
    let mut read = Vec::new();
    for index in 0..3 {
        let (data, count) = fixture.store.read_chunk(&sha256, index).unwrap();
        assert_eq!(count, 3);
        read.extend(data);
    }
    assert_eq!(read, bytes);
    assert_eq!(
        fixture.store.read_chunk(&sha256, 3),
        Err(ExhibitsError::InvalidChunk {
            reason: "chunk_index"
        })
    );
    assert_eq!(
        fixture.store.read_chunk(&"0".repeat(64), 0),
        Err(ExhibitsError::NotFound)
    );
}

#[test]
fn draft_update_creates_replaces_and_reads_back() {
    let fixture = fixture();
    let page = fixture.upload(b"<h1>one</h1>");
    let style = fixture.upload(b"h1 { color: teal; }");
    assert_eq!(
        fixture.update(
            "plan",
            &[("index.html", &page), ("assets/style.css", &style)],
            None
        ),
        Ok(1)
    );
    let manifest = fixture.store.read("plan", 0).unwrap();
    assert_eq!(manifest.revision, 1);
    assert_eq!(manifest.entry, "index.html");
    assert_eq!(manifest.title, "Title of plan");
    assert_eq!(manifest.updated_by, author());
    assert_eq!(manifest.file_count, 2);
    assert_eq!(manifest.next_offset, None);
    assert_eq!(
        manifest.files,
        vec![
            DraftFile {
                path: "assets/style.css".into(),
                sha256: style.clone(),
                size: 19
            },
            DraftFile {
                path: "index.html".into(),
                sha256: page.clone(),
                size: 12
            },
        ]
    );
    let second = fixture.upload(b"<h1>two</h1>");
    assert_eq!(
        fixture.update("plan", &[("index.html", &second)], Some(1)),
        Ok(2)
    );
    let (entry, revision, files) = fixture.store.view_files("plan").unwrap();
    assert_eq!((entry.as_str(), revision), ("index.html", 2));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].sha256, second);
}

#[test]
fn draft_update_refuses_with_each_error_code() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    let absent = "e".repeat(64);
    assert_eq!(
        fixture.update("Bad_ID", &[("index.html", &page)], None),
        Err(ExhibitsError::InvalidId)
    );
    assert!(matches!(
        fixture.update("ok", &[("../index.html", &page)], None),
        Err(ExhibitsError::InvalidPath {
            reason: "malformed",
            ..
        })
    ));
    assert!(matches!(
        fixture.update("ok", &[("index.html", &page), ("index.html", &page)], None),
        Err(ExhibitsError::InvalidPath {
            reason: "duplicate",
            ..
        })
    ));
    assert_eq!(
        fixture.update("ok", &[("index.html", &page), ("tool.exe", &page)], None),
        Err(ExhibitsError::TypeRefused {
            path: "tool.exe".into()
        })
    );
    assert_eq!(
        fixture.update("ok", &[("index.html", &page), ("photo.PNG", &page)], None),
        Err(ExhibitsError::TypeRefused {
            path: "photo.PNG".into()
        })
    );
    assert_eq!(
        fixture.update("ok", &[("index.html", &page), ("app.js", &absent)], None),
        Err(ExhibitsError::MissingFiles(vec![absent.clone()]))
    );
    let entry_missing = fixture.store.update_draft(&DraftUpdate {
        author: author(),
        id: "ok".into(),
        title: "Title".into(),
        entry: "start.html".into(),
        files: vec![("index.html".into(), page.clone())],
        expected_revision: None,
    });
    assert!(matches!(
        entry_missing,
        Err(ExhibitsError::InvalidPath {
            reason: "entry_not_listed",
            ..
        })
    ));
    let untitled = fixture.store.update_draft(&DraftUpdate {
        author: author(),
        id: "ok".into(),
        title: " ".into(),
        entry: "index.html".into(),
        files: vec![("index.html".into(), page.clone())],
        expected_revision: None,
    });
    assert_eq!(untitled, Err(ExhibitsError::InvalidTitle));
    assert_eq!(
        fixture.update("ok", &[("index.html", &page)], Some(3)),
        Err(ExhibitsError::RevisionConflict { current: 0 })
    );
    assert_eq!(
        fixture.update("ok", &[("index.html", &page)], Some(0)),
        Ok(1)
    );
    assert_eq!(
        fixture.update("ok", &[("index.html", &page)], Some(0)),
        Err(ExhibitsError::RevisionConflict { current: 1 })
    );
    // Nothing a refusal saw was written.
    assert_eq!(fixture.store.list(None, LIST_PAGE).unwrap().drafts.len(), 1);
}

#[test]
fn draft_update_refuses_too_many_files_and_too_large_a_total() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    let names: Vec<String> = (0..=MAX_FILES).map(|i| format!("f{i}.html")).collect();
    let many: Vec<(&str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), page.as_str()))
        .collect();
    assert_eq!(
        fixture.update("many", &many, None),
        Err(ExhibitsError::TooLarge { limit: "count" })
    );
    // Four 16 MiB files in the content store (sparse, so the test writes no 64 MiB), which with
    // the page exceed the 64 MiB total by the page's 8 bytes.
    let mut files = Vec::new();
    for seed in 0..4_u8 {
        let name = format!("{seed:064x}");
        fs::File::create(fixture.root.join(CONTENT_DIRECTORY).join(&name))
            .unwrap()
            .set_len(MAX_FILE_BYTES)
            .unwrap();
        files.push((format!("v{seed}.mp4"), name));
    }
    let mut listed: Vec<(&str, &str)> = vec![("index.html", page.as_str())];
    listed.extend(
        files
            .iter()
            .map(|(path, sha)| (path.as_str(), sha.as_str())),
    );
    assert_eq!(
        fixture.update("big", &listed, None),
        Err(ExhibitsError::TooLarge { limit: "total" })
    );
    assert_eq!(fixture.update("big", &listed[..4], None), Ok(1));
}

#[test]
fn draft_update_accepts_the_largest_paths() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    let longest = format!("{}.html", "p".repeat(MAX_PATH_BYTES - 5));
    assert_eq!(longest.len(), MAX_PATH_BYTES);
    assert_eq!(
        fixture.update("long", &[(longest.as_str(), &page)], None),
        Ok(1)
    );
}

#[test]
fn list_is_newest_first_and_pages_with_a_cursor() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    for i in 0..70 {
        fixture
            .update(&format!("d{i:02}"), &[("index.html", &page)], None)
            .unwrap();
        if i % 3 == 0 {
            fixture.advance(5);
        }
    }
    // Updating an old Draft moves it to the top.
    fixture
        .update("d00", &[("index.html", &page)], Some(1))
        .unwrap();
    let first = fixture.store.list(None, LIST_PAGE).unwrap();
    assert_eq!(first.drafts.len(), LIST_PAGE);
    assert_eq!(first.drafts[0].id, "d00");
    assert_eq!(first.drafts[0].revision, 2);
    assert_eq!(first.drafts[1].id, "d69");
    assert!(
        first
            .drafts
            .windows(2)
            .all(|pair| pair[0].updated_at > pair[1].updated_at)
    );
    let second = fixture
        .store
        .list(first.next_before.as_deref(), LIST_PAGE)
        .unwrap();
    assert_eq!(second.drafts.len(), 6);
    assert_eq!(second.next_before, None);
    let mut ids: Vec<String> = first
        .drafts
        .iter()
        .chain(&second.drafts)
        .map(|d| d.id.clone())
        .collect();
    assert_eq!(ids.last().unwrap(), "d01");
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 70);
    assert_eq!(
        fixture.store.list(Some("nonsense"), 4),
        Err(ExhibitsError::InvalidRequest)
    );
}

#[test]
fn read_pages_large_manifests() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    let names: Vec<String> = (0..MAX_FILES).map(|i| format!("p/{i:04}.html")).collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .map(|name| (name.as_str(), page.as_str()))
        .collect();
    fixture.update("wide", &files, None).unwrap();
    let first = fixture.store.read("wide", 0).unwrap();
    assert_eq!(first.files.len(), READ_PAGE);
    assert_eq!(first.file_count, MAX_FILES);
    assert_eq!(first.next_offset, Some(READ_PAGE));
    let second = fixture.store.read("wide", READ_PAGE).unwrap();
    assert_eq!(second.files.len(), MAX_FILES - READ_PAGE);
    assert_eq!(second.next_offset, None);
    assert_eq!(second.files.last().unwrap().path, "p/0999.html");
    assert_eq!(
        fixture.store.read("absent", 0),
        Err(ExhibitsError::NotFound)
    );
}

#[test]
fn delete_checks_the_revision_and_is_idempotent() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    fixture
        .update("gone", &[("index.html", &page)], None)
        .unwrap();
    assert_eq!(
        fixture.store.delete("gone", Some(7)),
        Err(ExhibitsError::RevisionConflict { current: 1 })
    );
    assert_eq!(fixture.store.delete("gone", Some(1)), Ok(true));
    assert_eq!(fixture.store.delete("gone", None), Ok(false));
    assert_eq!(fixture.store.read("gone", 0), Err(ExhibitsError::NotFound));
    // The bytes stay for the grace period, so recreating needs no upload.
    assert!(fixture.complete(&page));
    assert_eq!(
        fixture.update("gone", &[("index.html", &page)], None),
        Ok(1)
    );
}

#[test]
fn garbage_collection_keeps_referenced_and_recent_content_and_deletes_stale() {
    let fixture = fixture();
    let referenced = fixture.upload(b"<p>kept</p>");
    let released = fixture.upload(b"<p>released later</p>");
    let unreferenced = fixture.upload(b"<p>never used</p>");
    fixture
        .update(
            "keep",
            &[("index.html", &referenced), ("old.html", &released)],
            None,
        )
        .unwrap();
    let stale_partial = pattern(CHUNK_BYTES + 1, 9);
    let stale_sha = sha(&stale_partial);
    fixture
        .store
        .put_chunk(
            &stale_sha,
            stale_partial.len() as u64,
            0,
            2,
            chunk(&stale_partial, 0),
        )
        .unwrap();

    fixture.advance(GC_GRACE_MS / 2);
    // Releasing `released` now starts its grace period now, not at upload.
    fixture
        .update("keep", &[("index.html", &referenced)], Some(1))
        .unwrap();
    let recent_partial = pattern(CHUNK_BYTES + 1, 10);
    let recent_sha = sha(&recent_partial);
    fixture
        .store
        .put_chunk(
            &recent_sha,
            recent_partial.len() as u64,
            0,
            2,
            chunk(&recent_partial, 0),
        )
        .unwrap();
    assert_eq!(
        fixture.store.collect_garbage().unwrap(),
        GcReport::default()
    );

    fixture.advance(GC_GRACE_MS / 2 + 1);
    let report = fixture.store.collect_garbage().unwrap();
    assert_eq!(
        report,
        GcReport {
            partials_removed: 1,
            files_removed: 1
        }
    );
    assert!(fixture.complete(&referenced));
    assert!(fixture.complete(&released));
    assert!(!fixture.complete(&unreferenced));
    assert!(!fixture.partial(&stale_sha));
    assert!(fixture.partial(&recent_sha));

    fixture.advance(GC_GRACE_MS / 2);
    let report = fixture.store.collect_garbage().unwrap();
    assert_eq!(
        report,
        GcReport {
            partials_removed: 1,
            files_removed: 1
        }
    );
    assert!(fixture.complete(&referenced));
    assert!(!fixture.complete(&released));
    assert!(!fixture.partial(&recent_sha));
    assert_eq!(fixture.store.read("keep", 0).unwrap().revision, 2);
}

#[test]
fn garbage_collection_clears_leftovers_of_interrupted_work() {
    let fixture = fixture();
    fs::write(
        fixture.root.join("content/tmp").join("a".repeat(64)),
        b"half",
    )
    .unwrap();
    let orphan = fixture.root.join("content/partial").join("b".repeat(64));
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("0"), b"chunk").unwrap();
    // A complete file the database never recorded starts its grace period at the first pass.
    let adopted = sha(b"adopted");
    fs::write(
        fixture.root.join(CONTENT_DIRECTORY).join(&adopted),
        b"adopted",
    )
    .unwrap();
    let report = fixture.store.collect_garbage().unwrap();
    assert_eq!(report.partials_removed, 1);
    assert!(
        read_directory(&fixture.root.join("content/tmp"))
            .unwrap()
            .is_empty()
    );
    assert!(fixture.complete(&adopted));
    fixture.advance(GC_GRACE_MS + 1);
    assert_eq!(fixture.store.collect_garbage().unwrap().files_removed, 1);
    assert!(!fixture.complete(&adopted));
}

#[test]
fn the_store_reopens_with_its_drafts() {
    let fixture = fixture();
    let page = fixture.upload(b"<p>x</p>");
    fixture
        .update("again", &[("index.html", &page)], None)
        .unwrap();
    let clock = fixture.now.clone();
    let reopened = ExhibitStore::open(
        &fixture.root,
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
    .unwrap();
    assert_eq!(reopened.read("again", 0).unwrap().revision, 1);
}
