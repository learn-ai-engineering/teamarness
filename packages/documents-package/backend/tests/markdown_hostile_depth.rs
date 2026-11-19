//! Deeply nested text must not overflow the stack on the import, export or merge paths. A stack
//! overflow aborts the process and cannot be caught, so every case here runs on a thread with the
//! stack of a Tokio blocking thread, where the Backend does this work. A regression kills the test
//! binary.
//!
//! The Rust core is the only Markdown authority, so these cases go through its public interface.

use loro::LoroDoc;
use teamarness_documents_package::markdown::{
    Block, Inline, Marks, PreservedKind, canonical, export, merge, read_tree, write_tree,
};

/// Tokio's default stack for a blocking thread.
const BLOCKING_STACK: usize = 2 * 1024 * 1024;

const DEPTH: usize = 20_000;

fn on_blocking_stack(case: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(BLOCKING_STACK)
        .spawn(case)
        .unwrap()
        .join()
        .unwrap();
}

fn body_of(blocks: &[Block]) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(7).unwrap();
    write_tree(&doc, blocks).unwrap();
    doc.commit();
    doc
}

fn count(text: &str, wanted: char) -> usize {
    text.chars().filter(|c| *c == wanted).count()
}

fn plain(text: &str) -> Block {
    Block::Paragraph(vec![Inline::Text(text.to_owned(), Marks::default())])
}

/// A source that reads as nesting far beyond any limit exports as the literal paragraph of a
/// source that is not sound: every character stays, and the export is a fixed point.
#[test]
fn a_stored_source_that_reads_as_deep_nesting_exports_as_literal_text() {
    on_blocking_stack(|| {
        let shapes = [
            (">".repeat(DEPTH), '>'),
            (format!("{}x", "- ".repeat(DEPTH)), '-'),
            (format!("{}x", "1. ".repeat(DEPTH)), '1'),
            (format!("{}x", "> - ".repeat(DEPTH)), '>'),
            (
                format!("{}x{}", "*a ".repeat(5_000), " a*".repeat(5_000)),
                '*',
            ),
            (
                format!("{}x{}", "[".repeat(5_000), "](u)".repeat(5_000)),
                '[',
            ),
        ];
        for (source, marker) in shapes {
            let expected = count(&source, marker);
            let doc = body_of(&[Block::Preserved {
                kind: PreservedKind::Html,
                source,
            }]);
            let exported = export(&doc).unwrap();
            assert_eq!(count(&exported, marker), expected, "no character is lost");
            assert_eq!(canonical(&exported).unwrap(), exported, "a fixed point");
        }
    });
}

/// Paragraph text whose code span cannot hold its line break exports on one line.
#[test]
fn deep_text_in_a_code_mark_keeps_its_text() {
    on_blocking_stack(|| {
        let doc = body_of(&[Block::Paragraph(vec![Inline::Text(
            format!("a\n{}", ">".repeat(DEPTH)),
            Marks {
                code: true,
                ..Marks::default()
            },
        )])]);
        let exported = export(&doc).unwrap();
        assert_eq!(count(&exported, '>'), DEPTH, "no character is lost");
        assert_eq!(canonical(&exported).unwrap(), exported, "a fixed point");
    });
}

/// A merge reads the current body block by block, so a stored block must not abort it, whatever
/// the revision. Unchanged blocks stay untouched.
#[test]
fn a_merge_over_deeply_nested_stored_blocks_does_not_abort() {
    on_blocking_stack(|| {
        let deep = ">".repeat(DEPTH);
        let code = Marks {
            code: true,
            ..Marks::default()
        };
        let stored = [
            Block::Preserved {
                kind: PreservedKind::Html,
                source: deep.clone(),
            },
            Block::Paragraph(vec![Inline::Text(format!("a\n{deep}"), code)]),
        ];
        for block in stored {
            let doc = body_of(&[block, plain("keep")]);
            let base = doc.oplog_frontiers();
            let merged = merge(&doc, &base, "changed\n", 9).unwrap();
            doc.import(&merged.update).unwrap();
            assert_eq!(export(&doc).unwrap(), "changed\n");
        }
    });
}

/// The table fallback imports a table's lines again with no table extension.
#[test]
fn a_table_that_falls_back_with_deep_lines_keeps_its_text() {
    on_blocking_stack(|| {
        let deep = ">".repeat(DEPTH);
        for markdown in [
            // A final row that trimming would turn into a blockquote. This one stays a table.
            format!("a|b\n-|-\n\u{3000}{deep} \n"),
            // A tab across a column boundary cannot be removed as a container prefix, so this
            // one falls back, and the trimmed row reads as a blockquote without the bound.
            format!("- a|b\n\t-|-\n  \u{3000}{deep}\n"),
        ] {
            let exported = canonical(&markdown).unwrap();
            assert_eq!(count(&exported, '>'), DEPTH, "no character is lost");
            assert_eq!(canonical(&exported).unwrap(), exported, "a fixed point");
        }
    });
}

/// Two valid revisions, each of an HTML block, can merge into a source with no HTML opening left.
#[test]
fn two_valid_revisions_can_merge_into_a_deep_source_without_abort() {
    on_blocking_stack(|| {
        let emphasis = format!("{}x{}", "*a ".repeat(5_000), " a*".repeat(5_000));
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        write_tree(&doc, &[]).unwrap();
        let empty = doc.oplog_frontiers();
        let created = merge(&doc, &empty, &format!("<div>\n<p>\n{emphasis}\n"), 2).unwrap();
        doc.import(&created.update).unwrap();
        assert!(matches!(
            read_tree(&doc).unwrap().as_slice(),
            [Block::Preserved { .. }]
        ));
        let base = doc.oplog_frontiers();
        let first = merge(&doc, &base, &format!("<p>\n{emphasis}\n"), 3).unwrap();
        let second = merge(&doc, &base, &format!("<div>\n{emphasis}\n"), 4).unwrap();
        doc.import(&first.update).unwrap();
        doc.import(&second.update).unwrap();

        let exported = export(&doc).unwrap();
        assert_eq!(count(&exported, '*'), count(&emphasis, '*'));
        assert_eq!(canonical(&exported).unwrap(), exported, "a fixed point");
    });
}
