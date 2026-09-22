use std::fs;

use serde::{Deserialize, Serialize};
use teamarness_package_sdk::storage::{JsonDocumentStore, StorageError, write_atomic};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Document {
    counter: u32,
    label: String,
}

fn document() -> Document {
    Document {
        counter: 0,
        label: "start".to_owned(),
    }
}

#[test]
fn write_atomic_replaces_the_file_and_leaves_no_temp_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("nested/state.json");
    write_atomic(&path, b"first").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"first");
    write_atomic(&path, b"second").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second");
    assert!(!temp.path().join("nested/state.json.tmp").exists());
}

#[test]
fn write_atomic_failure_preserves_the_previous_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("state.json");
    write_atomic(&path, b"kept").unwrap();
    // A directory in the temporary file's place makes the write fail without touching the target.
    fs::create_dir(temp.path().join("state.json.tmp")).unwrap();
    assert!(write_atomic(&path, b"lost").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"kept");
    assert!(temp.path().join("state.json.tmp").is_dir());
}

#[test]
fn json_document_store_creates_reads_updates_and_reopens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("document.json");
    let mut store = JsonDocumentStore::open_or_create(&path, document).unwrap();
    assert_eq!(store.get(), &document());
    assert!(fs::read_to_string(&path).unwrap().ends_with("}\n"));

    store
        .update(|value| {
            value.counter += 1;
            value.label = "changed".to_owned();
            Ok(())
        })
        .unwrap();
    assert_eq!(store.get().counter, 1);

    store
        .replace(Document {
            counter: 7,
            label: "replaced".to_owned(),
        })
        .unwrap();

    let reopened = JsonDocumentStore::<Document>::open_or_create(&path, document).unwrap();
    assert_eq!(reopened.get().counter, 7);
    assert_eq!(reopened.get().label, "replaced");
}

#[test]
fn json_document_store_update_error_leaves_value_and_file_unchanged() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("document.json");
    let mut store = JsonDocumentStore::open_or_create(&path, document).unwrap();
    let before = fs::read(&path).unwrap();

    let error = store
        .update(|value| {
            value.counter = 99;
            Err::<(), _>(StorageError::Corrupt)
        })
        .unwrap_err();
    assert_eq!(error, StorageError::Corrupt);
    assert_eq!(store.get(), &document());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn corrupt_document_is_reported() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("document.json");
    fs::write(&path, b"not json at all").unwrap();
    let error = JsonDocumentStore::<Document>::open_or_create(&path, document).unwrap_err();
    assert_eq!(error, StorageError::Corrupt);
}
