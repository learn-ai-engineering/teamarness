//! The Markdown corpus: canonical documents are byte-stable, and agent-style Markdown
//! normalises once to its recorded canonical form and then stays fixed.
//!
//! `MARKDOWN_CORPUS_BLESS=1` rewrites the `*.expected.md` files after a reviewed change.

use std::fs;
use std::path::{Path, PathBuf};

use loro::LoroDoc;
use teamarness_documents_package::markdown::testing::parse;
use teamarness_documents_package::markdown::{export, merge, write_tree};

mod common;
use common::AGENT;

const EXPECTED: &str = ".expected.md";

fn corpus(kind: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown-corpus")
        .join(kind);
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "md")
                && !path.to_string_lossy().ends_with(EXPECTED)
        })
        .collect();
    files.sort();
    files
}

fn expected_path(input: &Path) -> PathBuf {
    let name = input.file_stem().unwrap().to_string_lossy();
    input.with_file_name(format!("{name}{EXPECTED}"))
}

/// Canonical Markdown of `markdown`, through the public merge into a new Document's genesis.
fn revise_genesis(markdown: &str) -> String {
    let doc = LoroDoc::new();
    write_tree(&doc, &[]).unwrap();
    let merged = merge(&doc, &doc.oplog_frontiers(), markdown, AGENT).unwrap();
    doc.import(&merged.update).unwrap();
    export(&doc).unwrap()
}

fn export_of(markdown: &str) -> String {
    let doc = LoroDoc::new();
    write_tree(&doc, &parse(markdown)).unwrap();
    export(&doc).unwrap()
}

#[test]
fn the_corpus_has_its_recorded_size() {
    assert_eq!(corpus("canonical").len(), 17);
    assert_eq!(corpus("agent-style").len(), 22);
}

#[test]
fn canonical_markdown_is_byte_stable() {
    let mut failures = Vec::new();
    for path in corpus("canonical") {
        let case = fs::read_to_string(&path).unwrap();
        for (route, out) in [
            ("export", export_of(&case)),
            ("merge", revise_genesis(&case)),
        ] {
            if out != case {
                failures.push(format!(
                    "{} ({route})\n--- in\n{case}--- out\n{out}",
                    path.display()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn agent_style_markdown_normalises_once() {
    let bless = std::env::var_os("MARKDOWN_CORPUS_BLESS").is_some();
    let mut failures = Vec::new();
    for path in corpus("agent-style") {
        let case = fs::read_to_string(&path).unwrap();
        let first = parse(&case);
        let exported = export_of(&case);
        let second = parse(&exported);
        if first != second || export_of(&exported) != exported {
            failures.push(format!(
                "{} is not fixed after one step\n--- out\n{exported}--- first\n{first:?}\n--- second\n{second:?}",
                path.display()
            ));
        }
        if revise_genesis(&case) != exported {
            failures.push(format!("{}: merge and export disagree", path.display()));
        }
        let expected = expected_path(&path);
        if bless {
            fs::write(&expected, &exported).unwrap();
        } else if fs::read_to_string(&expected).ok().as_deref() != Some(exported.as_str()) {
            failures.push(format!(
                "{} differs from {}\n--- out\n{exported}",
                path.display(),
                expected.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
