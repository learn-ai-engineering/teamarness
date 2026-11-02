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
    assert_eq!(corpus("canonical").len(), 22);
    assert_eq!(corpus("agent-style").len(), 40);
}

/// A new Document's genesis, revised to `markdown`.
fn revised_document(markdown: &str) -> LoroDoc {
    let doc = LoroDoc::new();
    write_tree(&doc, &[]).unwrap();
    let merged = merge(&doc, &doc.oplog_frontiers(), markdown, AGENT).unwrap();
    doc.import(&merged.update).unwrap();
    doc
}

/// Revises `doc` to `markdown` against its current version, and gives the operation count.
fn revise(doc: &LoroDoc, markdown: &str, peer: u64) -> usize {
    let merged = merge(doc, &doc.oplog_frontiers(), markdown, peer).unwrap();
    doc.import(&merged.update).unwrap();
    merged.ops
}

/// The corpus documents whose table uses a reference definition, with that definition's lines.
const TABLES_WITH_DEFINITIONS: [(&str, &str); 4] = [
    (
        "36-table-reference-link",
        "[r]: https://x.test/page \"Page\"\n[i]: https://x.test/p.png\n",
    ),
    (
        "37-table-reference-link-in-list",
        "[r]: https://x.test/page\n[i]: https://x.test/p.png\n",
    ),
    (
        "38-table-reference-link-in-quote",
        "[r]: https://x.test/page\n[i]: https://x.test/p.png\n",
    ),
    (
        "39-fallback-table-reference-link",
        "[r]: https://x.test/page\n[i]: https://x.test/p.png\n",
    ),
];

fn agent_style(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown-corpus/agent-style")
            .join(format!("{name}.md")),
    )
    .unwrap()
}

/// SF2-001. The first export holds the target, and it does not change or grow afterwards: not
/// on an unchanged revision, and not when a revision brings the definition back, repeatedly.
#[test]
fn a_table_that_uses_a_reference_definition_keeps_its_target() {
    let target = "(https://x.test/page";
    let image = "(https://x.test/p.png)";
    for (name, definitions) in TABLES_WITH_DEFINITIONS {
        let exported = revise_genesis(&agent_style(name));
        assert!(exported.contains(target), "{name}: link target\n{exported}");
        assert!(exported.contains(image), "{name}: image target\n{exported}");
        assert_eq!(export_of(&exported), exported, "{name}: fixed point");

        let doc = revised_document(&exported);
        assert_eq!(revise(&doc, &exported, 0x10), 0, "{name}: no-op merge");

        // The definition comes back after each export, as an outside editor might keep it.
        let with_definitions = format!("{exported}\n{definitions}");
        for (round, peer) in [0x11, 0x12, 0x13].into_iter().enumerate() {
            assert_eq!(
                revise(&doc, &with_definitions, peer),
                0,
                "{name}: the definition is back, round {round}"
            );
            assert_eq!(export(&doc).unwrap(), exported, "{name}: round {round}");
        }
    }
}

/// The added paragraph is ordinary content: once removed, it stays removed, and the table keeps
/// its lines. Bringing the definition back gives it back once.
#[test]
fn a_revision_can_remove_the_links_paragraph() {
    let table = "| a |\n|---|\n| [x][r] |\n";
    let with_links = format!("{table}\n[x](https://x.test/page)\n");
    let doc = revised_document(&format!("{table}\n[r]: https://x.test/page\n"));
    assert_eq!(export(&doc).unwrap(), with_links);

    revise(&doc, table, 0x10);
    assert_eq!(export(&doc).unwrap(), table);
    assert_eq!(revise(&doc, table, 0x11), 0);

    let definition = format!("{table}\n[r]: https://x.test/page\n");
    revise(&doc, &definition, 0x12);
    assert_eq!(export(&doc).unwrap(), with_links);
    assert_eq!(revise(&doc, &definition, 0x13), 0);
}

/// Another paragraph after the table is not the links paragraph, and it stays. Each table has
/// its own paragraph, and a table's own inline link is not repeated.
#[test]
fn the_links_paragraph_is_added_beside_other_content() {
    assert_eq!(
        export_of(
            "| a |\n|---|\n| [x][r] |\n\nProse.\n\n| b |\n|---|\n| [y](https://x.test/in) [x][r] |\n\n[r]: https://x.test/page\n"
        ),
        "| a |\n|---|\n| [x][r] |\n\n[x](https://x.test/page)\n\nProse.\n\n| b |\n|---|\n| [y](https://x.test/in) [x][r] |\n\n[x](https://x.test/page)\n"
    );
    // The same link twice is two links, in cell order, and the definition may stand first.
    assert_eq!(
        export_of(
            "[r]: https://x.test/page\n\n| a | b |\n|---|---|\n| [x][r] | [x][r] |\n| [r] | [r][] |\n"
        ),
        "| a | b |\n|---|---|\n| [x][r] | [x][r] |\n| [r] | [r][] |\n\n[x](https://x.test/page) [x](https://x.test/page) [r](https://x.test/page) [r](https://x.test/page)\n"
    );
}

/// The known limits the contract states.
#[test]
fn the_links_paragraph_has_its_stated_limits() {
    // A link with no text has the URL as its text, so the target is still there.
    assert_eq!(
        export_of("| a |\n|---|\n| [][r] |\n\n[r]: https://x.test/page\n"),
        "| a |\n|---|\n| [][r] |\n\n<https://x.test/page>\n"
    );
    // A cell past the header's width is not parsed, so its definition is unused and dropped.
    assert_eq!(
        export_of("| a |\n|---|\n| b | [x][r] |\n\n[r]: https://x.test/page\n"),
        "| a |\n|---|\n| b | [x][r] |\n"
    );
}

/// What the contract's guarantee 3 names as lost on import, and the literal text beside it.
#[test]
fn import_drops_what_the_contract_names() {
    for (case, expected) in [
        // A footnote definition that is not one token and a title stays literal text.
        (
            "a[^1]\n\n[^1]: a longer note\n",
            "a\\[^1\\]\n\n\\[^1\\]: a longer note\n",
        ),
        // A definition that no link uses.
        ("a\n\n[r]: https://x.test/page\n", "a\n"),
        ("a\n\n[^1]: note\n", "a\n"),
        // The separator comment between two lists.
        (
            "- a\n\n<!-- end list -->\n\n- b\n",
            "- a\n\n<!-- end list -->\n\n- b\n",
        ),
        ("a\n\n<!-- end list -->\n\nb\n", "a\n\nb\n"),
        // An image's title, and a link inside its alt text.
        (
            "![p](https://x.test/p.png \"t\")\n",
            "\\!\\[p\\](https://x.test/p.png)\n",
        ),
        (
            "![a [b](https://x.test/b)](https://x.test/p.png)\n",
            "\\!\\[a b\\](https://x.test/p.png)\n",
        ),
        // A link with no text.
        ("a [](https://x.test/page)\n", "a\n"),
    ] {
        assert_eq!(export_of(case), expected, "{case}");
    }
}

/// A table with no definition exports as it did before SF2-001, whatever links its cells show.
#[test]
fn a_table_with_no_definition_exports_unchanged() {
    for case in [
        "| a |\n|---|\n| 1 |\n",
        "| a |\n|---|\n| [x](https://x.test/in) |\n",
        "| a |\n|---|\n| ![p](https://x.test/p.png) |\n",
        "| a |\n|---|\n| <https://x.test/auto> |\n",
        "| a |\n|---|\n| [x][r] [y] |\n",
    ] {
        assert_eq!(export_of(case), case);
        assert_eq!(revise_genesis(case), case);
    }
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
