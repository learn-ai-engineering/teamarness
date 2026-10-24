//! The body tree format across languages, pinned by committed fixtures in
//! `packages/documents-package/fixtures/body-trees/`:
//!
//! - `from-rust/` holds trees the Rust core writes (every node and mark). The UI suite
//!   (`ui/src/editor/cross-language.test.ts`) loads them through `loro-crdt` and the
//!   `loro-prosemirror` reader and checks them against the shared ProseMirror schema.
//! - `from-binding/` holds snapshots `loro-prosemirror` writes
//!   (`ui/scripts/write-binding-fixtures.mjs`). This suite reads them with the strict reader and
//!   exports them to their recorded Canonical Markdown.
//!
//! Regenerate after a reviewed format change, in this order:
//!
//! 1. `BODY_TREES_BLESS=1 cargo test -p teamarness-documents-package --test cross_language`
//!    writes `from-rust/`.
//! 2. `node packages/documents-package/ui/scripts/write-binding-fixtures.mjs` writes the binding
//!    snapshots into `from-binding/`, starting from the Rust genesis.
//! 3. `BODY_TREES_BLESS=1 cargo test ...` again records each binding tree's `.md` export.
//!
//! The same suite checks that the Rust `loro` and the npm `loro-crdt` are one exact version.

use std::fs;
use std::path::{Path, PathBuf};

use loro::{ExportMode, LoroDoc};
use teamarness_documents_package::markdown::testing::parse;
use teamarness_documents_package::markdown::{export, read_tree, write_tree};

/// Every node and mark of the schema in one body, beyond what the corpus already covers.
const ALL_NODES: &str = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n\n\
Plain **strong** *em* ~~strike~~ `code` [link](https://example.com/a \"Title\") \
[bare](https://example.com/b) ***both*** and a hard  \nbreak.\n\n\
> A quote\n>\n> - with a list\n\n\
1. first\n2. second\n   - nested **bold**\n\n\
5. five\n6. six\n\n\
---\n\n\
```rust\nfn main() {}\n```\n\n\
```\nplain code\n```\n";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/body-trees")
}

fn blessing() -> bool {
    std::env::var_os("BODY_TREES_BLESS").is_some()
}

/// The Markdown each Rust fixture is written from: a genesis, the canonical corpus, and one body
/// with every node and mark.
fn rust_sources() -> Vec<(String, String)> {
    let corpus =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown-corpus/canonical");
    let mut files: Vec<PathBuf> = fs::read_dir(corpus)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    let mut sources = vec![
        ("00-genesis".to_owned(), String::new()),
        ("01-all-nodes".to_owned(), ALL_NODES.to_owned()),
    ];
    for (index, path) in files.iter().enumerate() {
        let stem = path.file_stem().unwrap().to_string_lossy();
        let name = stem.split_once('-').map_or(&*stem, |(_, rest)| rest);
        sources.push((
            format!("{:02}-corpus-{name}", index + 2),
            fs::read_to_string(path).unwrap(),
        ));
    }
    sources
}

/// The tree the Rust core writes for `markdown`, as one Loro snapshot from a fixed peer.
fn rust_tree(markdown: &str, peer: u64) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer).unwrap();
    write_tree(&doc, &parse(markdown)).unwrap();
    doc
}

fn snapshot_of(path: &Path) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.import(&fs::read(path).unwrap()).unwrap();
    doc
}

fn names(dir: &Path, extension: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn rust_written_trees_are_the_committed_fixtures() {
    let dir = fixtures().join("from-rust");
    let sources = rust_sources();
    if blessing() {
        fs::create_dir_all(&dir).unwrap();
        for entry in fs::read_dir(&dir).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        for (index, (name, markdown)) in sources.iter().enumerate() {
            let doc = rust_tree(markdown, index as u64 + 1);
            fs::write(
                dir.join(format!("{name}.loro")),
                doc.export(ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
            fs::write(dir.join(format!("{name}.md")), export(&doc).unwrap()).unwrap();
        }
    }
    let expected: Vec<String> = sources.iter().map(|(name, _)| name.clone()).collect();
    assert_eq!(
        names(&dir, "loro"),
        expected,
        "regenerate with BODY_TREES_BLESS=1"
    );
    for (index, (name, markdown)) in sources.iter().enumerate() {
        let committed = snapshot_of(&dir.join(format!("{name}.loro")));
        let recorded = fs::read_to_string(dir.join(format!("{name}.md"))).unwrap();
        read_tree(&committed).unwrap_or_else(|violation| panic!("{name}: {violation:?}"));
        assert_eq!(export(&committed).unwrap(), recorded, "{name}");
        assert_eq!(
            export(&rust_tree(markdown, index as u64 + 1)).unwrap(),
            recorded,
            "{name}: the Rust writer no longer writes the committed tree"
        );
    }
}

#[test]
fn binding_written_trees_read_and_export_to_their_recorded_markdown() {
    let dir = fixtures().join("from-binding");
    let trees = names(&dir, "loro");
    assert!(
        trees.len() >= 8,
        "the binding fixtures are present: {trees:?}"
    );
    for name in trees {
        let doc = snapshot_of(&dir.join(format!("{name}.loro")));
        read_tree(&doc).unwrap_or_else(|violation| panic!("{name}: {violation:?}"));
        let markdown = export(&doc).unwrap();
        let path = dir.join(format!("{name}.md"));
        if blessing() {
            fs::write(&path, &markdown).unwrap();
        }
        assert_eq!(
            markdown,
            fs::read_to_string(&path).unwrap_or_default(),
            "{name}: regenerate with BODY_TREES_BLESS=1"
        );
        // A binding tree is already canonical: its export imports to the same export again.
        let again = rust_tree(&markdown, 7);
        assert_eq!(export(&again).unwrap(), markdown, "{name}");
    }
}

/// The version a lockfile resolves for `name`, from `name = "..."` followed by `version = "..."`.
fn cargo_lock_version(lock: &str, name: &str) -> Option<String> {
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == format!("name = \"{name}\"") {
            let version = lines.next()?.trim();
            return Some(
                version
                    .strip_prefix("version = \"")?
                    .strip_suffix('"')?
                    .to_owned(),
            );
        }
    }
    None
}

#[test]
fn rust_loro_and_npm_loro_crdt_are_one_exact_version() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let cargo = cargo_lock_version(
        &fs::read_to_string(root.join("Cargo.lock")).unwrap(),
        "loro",
    )
    .expect("Cargo.lock resolves loro");
    let npm: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("package-lock.json")).unwrap()).unwrap();
    let crdt = npm["packages"]["node_modules/loro-crdt"]["version"]
        .as_str()
        .expect("package-lock.json resolves loro-crdt");
    assert_eq!(
        cargo, crdt,
        "Rust loro and npm loro-crdt must be the same version"
    );
}
