//! The Markdown corpus: canonical documents are byte-stable, and agent-style Markdown
//! normalises once to its recorded canonical form and then stays fixed.
//!
//! `MARKDOWN_CORPUS_BLESS=1` rewrites the `*.expected.md` files after a reviewed change.

use std::fs;
use std::path::{Path, PathBuf};

use loro::LoroDoc;
use teamarness_documents_package::markdown::testing::parse;
use teamarness_documents_package::markdown::{
    MAX_LOST_TOKENS, canonical, export, lost_tokens, merge, write_tree,
};

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

/// A URL of exactly 100 bytes.
fn url_100() -> String {
    let url = format!("https://x.test/{}", "a".repeat(85));
    assert_eq!(url.len(), 100);
    url
}

/// `body` after a filler paragraph that makes it exactly `len` bytes.
fn sized(body: &str, len: usize) -> String {
    let filler = len - body.len() - 2;
    let text = format!("{}\n\n{body}", "w".repeat(filler));
    assert_eq!(text.len(), len);
    text
}

/// How many times `markdown` links to `url` inline.
fn links_to(markdown: &str, url: &str) -> usize {
    markdown.matches(&format!("]({url})")).count()
}

/// SM4-005. comrak's reference budget is 100 000 bytes for every text, however short. The
/// 100 000 bytes are a cap: a lookup past it resolves to nothing and stays literal text.
#[test]
fn the_reference_budget_is_capped_at_100_000_bytes() {
    let url = url_100();
    let uses = "[x][r] ".repeat(1001);
    let markdown = format!("{}\n\n[r]: {url}\n", uses.trim_end());
    for out in [export_of(&markdown), canonical(&markdown).unwrap()] {
        assert_eq!(links_to(&out, &url), 1000);
        assert_eq!(out.matches("\\[x\\]\\[r\\]").count(), 1);
    }
}

/// SM4-005. A link destination with a space has no canonical form, so its paragraph loses
/// every link target and its inline code.
#[test]
fn a_destination_with_a_space_loses_its_paragraphs_targets() {
    assert_eq!(export_of("a [x](<b c>) d\n"), "a x d\n");
    assert_eq!(
        export_of("[y](https://x.test/y) `c` [x](<b c>) d\n"),
        "y c x d\n"
    );
}

/// SM4-005. A definition with a space puts only the link's text in a table's links paragraph.
#[test]
fn a_definition_with_a_space_leaves_only_text_after_a_table() {
    assert_eq!(
        export_of("| a |\n|---|\n| [x][r] |\n\n[r]: <b c>\n"),
        "| a |\n|---|\n| [x][r] |\n\nx\n"
    );
}

/// CR3-001. Reusing a definition no longer runs out of budget in a short text.
#[test]
fn a_definition_can_be_used_often_in_a_short_text() {
    let url = url_100();
    let markdown = sized(&format!("[x][r] [x][r] [x][r]\n\n[r]: {url}\n"), 199);
    for out in [
        export_of(&markdown),
        canonical(&markdown).unwrap(),
        revise_genesis(&markdown),
    ] {
        assert_eq!(links_to(&out, &url), 3, "{out}");
    }
}

/// CR3-001. Front matter does not reduce the budget of the body after it.
#[test]
fn front_matter_does_not_reduce_the_reference_budget() {
    let url = url_100();
    let front = format!("---\n{}---\n\n", "key: a front matter value\n".repeat(80));
    assert!(front.len() > 2000);
    let uses = "[x][r] ".repeat(12);
    let body = sized(&format!("{}\n\n[r]: {url}\n", uses.trim_end()), 450);
    let markdown = format!("{front}{body}");
    for out in [
        export_of(&markdown),
        canonical(&markdown).unwrap(),
        revise_genesis(&markdown),
    ] {
        assert!(out.starts_with(&front), "{out}");
        assert_eq!(links_to(&out, &url), 12, "{out}");
    }
    // With a byte order mark before it, too.
    let out = canonical(&format!("\u{feff}{markdown}")).unwrap();
    assert_eq!(links_to(&out, &url), 12, "{out}");
}

/// CR3-001. Revising an export with its definitions back does not grow the Document.
#[test]
fn revising_a_table_with_its_definition_back_does_not_grow_it() {
    let url = url_100();
    let definition = format!("[r]: {url}\n");
    let markdown = format!("| a |\n|---|\n{}\n{definition}", "| [x][r] |\n".repeat(8));
    let doc = revised_document(&markdown);
    let exported = export(&doc).unwrap();
    assert_eq!(links_to(&exported, &url), 8, "{exported}");
    for (round, peer) in [0x10, 0x11, 0x12].into_iter().enumerate() {
        let ops = revise(&doc, &format!("{exported}\n{definition}"), peer);
        assert_eq!(ops, 0, "round {round}");
        assert_eq!(export(&doc).unwrap(), exported, "round {round}");
    }
}

/// The budget line goes where no block can be open, so a fence that is never closed gets no
/// blank lines added.
#[test]
fn an_unclosed_fence_with_a_definition_gains_no_lines() {
    for (case, expected) in [
        ("```\n[r]: x\n", "```\n[r]: x\n```\n"),
        ("```\n[x][r]\n[r]: x\n", "```\n[x][r]\n[r]: x\n```\n"),
        ("a\n\n```\n[r]: x\n", "a\n\n```\n[r]: x\n```\n"),
        (
            "---\nk: v\n---\n\n```\n[r]: x\n",
            "---\nk: v\n---\n\n```\n[r]: x\n```\n",
        ),
    ] {
        assert_eq!(export_of(case), expected, "{case}");
        assert_eq!(canonical(case).unwrap(), expected, "{case}");
    }
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

/// The corpus files whose import drops text, with the tokens the loss check reports. Every other
/// corpus file must report none.
const KNOWN_LOSSES: [(&str, &[&str]); 2] = [
    // Entities import as the characters they name, so their names are lost (conservative).
    ("15-entities", &["amp", "copy", "35"]),
    // The definition is unused: the only reference to it is inside inline code.
    ("40-table-inline-link", &["https", "x", "test", "unused"]),
];

/// Verifies: TEAM.SYS.DOCS-R14 (No lost text). The loss check passes every corpus file that keeps its text.
#[test]
fn the_loss_check_passes_the_corpus() {
    let mut failures = Vec::new();
    for kind in ["canonical", "agent-style"] {
        for path in corpus(kind) {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let case = fs::read_to_string(&path).unwrap();
            let lost = lost_tokens(&case).unwrap();
            let expected: Vec<String> = KNOWN_LOSSES
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, tokens)| tokens.iter().map(|t| (*t).to_owned()).collect())
                .unwrap_or_default();
            if lost != expected {
                failures.push(format!("{kind}/{name}: {lost:?}, expected {expected:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Verifies: TEAM.SYS.DOCS-R14 (No lost text). Each drop the contract names is reported, including a reference past the budget.
#[test]
fn the_loss_check_reports_each_known_drop() {
    let url = url_100();
    let uses = "[x][r] ".repeat(1000);
    let past_budget = format!("{uses}[y][late]\n\n[late]: https://x.test/target\n[r]: {url}\n");
    for (case, expected) in [
        (
            "a\n\n[r]: https://x.test/unused\n",
            &["https", "x", "test", "unused"][..],
        ),
        ("a\n\n[^1]: note\n", &["note"]),
        ("![p](https://x.test/p.png \"A title\")\n", &["A", "title"]),
        ("a [x](<b c>) d\n", &["b", "c"]),
        // The literal `[y][late]` keeps the label, and the target's other tokens are in the
        // resolved links, so only `target` is missing.
        (&past_budget, &["target"]),
    ] {
        assert_eq!(lost_tokens(case).unwrap(), expected, "{case}");
    }
}

/// Verifies: TEAM.SYS.DOCS-R14 (No lost text). Normalisation that only escapes or rewrites syntax keeps every token.
#[test]
fn the_loss_check_passes_escaping_and_resolved_references() {
    for case in [
        "1. a * b\n\n- [ ] task <span> & a_b_c\n",
        "Text \\# not a heading, `code` and \\`ticks\\`.\n",
        "[x][r], [y][], [r] and [Y]\n\n[r]: https://x.test/r \"Title\"\n[y]: https://x.test/y\n",
        "| a |\n|---|\n| [x][r] |\n\n[r]: https://x.test/page\n",
    ] {
        assert_eq!(lost_tokens(case).unwrap(), Vec::<String>::new(), "{case}");
    }
    let many = format!(
        "a\n\n[r]: https://x.test/{}\n",
        "one/two/three/four/five/six"
    );
    assert_eq!(lost_tokens(&many).unwrap().len(), MAX_LOST_TOKENS);
}
