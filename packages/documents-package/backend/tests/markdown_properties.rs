//! Property suites for the Markdown core: Canonical Markdown fidelity (P1–P3), the tree
//! store, and base-versioned merge (P4–P7). `CASES` sets the scale (default 3 000; merge
//! properties run a third of it).

use loro::{Container, ContainerTrait, ExportMode, Frontiers, LoroDoc, ValueOrContainer};
use proptest::prelude::*;
use teamarness_documents_package::markdown::testing::{canonicalize, parse};
use teamarness_documents_package::markdown::{
    Block, Inline, Link, MAX_NESTING, Marks, MergeError, Merged, SchemaViolation, export, merge,
    read_tree, write_tree,
};

mod common;
use common::*;

/// Canonical Markdown of a body, through the public writer and export.
fn to_markdown(body: &[Block]) -> String {
    let doc = LoroDoc::new();
    write_tree(&doc, body).unwrap();
    export(&doc).unwrap()
}

/// Merges `markdown` against `base` as `peer` and imports the update into `doc`.
fn merge_into(doc: &LoroDoc, base: &Frontiers, markdown: &str, peer: u64) -> Merged {
    let merged = merge(doc, base, markdown, peer).unwrap();
    doc.import(&merged.update).unwrap();
    merged
}

fn latest(doc: &LoroDoc) -> Frontiers {
    doc.oplog_frontiers()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), max_global_rejects: 200_000, ..ProptestConfig::default() })]

    /// P1: a body whose formatting sits on word boundaries survives export → import exactly.
    #[test]
    fn expressible_bodies_round_trip_exactly(body in blocks(spaced_text().boxed(), true)) {
        prop_assume!(canonicalize(&body) == body);
        let md = to_markdown(&body);
        prop_assert_eq!(parse(&md), body, "markdown was:\n{}", md);
    }

    /// P1b: ANY body imports back as its canonical form (formatting Markdown
    /// cannot express is dropped; nothing else changes).
    #[test]
    fn any_body_round_trips_to_canonical(body in blocks(word_text().boxed(), false)) {
        let md = to_markdown(&body);
        prop_assert_eq!(parse(&md), canonicalize(&body), "markdown was:\n{}", md);
    }

    /// P2: for ANY body (hostile text, any marks), the exported Markdown is a
    /// fixed point: import → export reproduces it byte for byte.
    #[test]
    fn export_is_a_fixed_point(body in blocks(word_text().boxed(), false)) {
        let md = to_markdown(&body);
        let again = to_markdown(&parse(&md));
        prop_assert_eq!(&again, &md, "body: {:?}", body);
    }

    /// P3: for ANY body, no character of content is lost by export → import
    /// (whitespace at line edges may be normalised).
    #[test]
    fn export_loses_no_content(body in blocks(word_text().boxed(), false)) {
        let md = to_markdown(&body);
        let squash = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
        prop_assert_eq!(squash(plain_text(&parse(&md))), squash(plain_text(&body)), "markdown was:\n{}", md);
    }

    /// The Loro tree stores the body exactly.
    #[test]
    fn loro_tree_round_trips(body in blocks(word_text().boxed(), false)) {
        let doc = LoroDoc::new();
        write_tree(&doc, &body).unwrap();
        prop_assert_eq!(read_tree(&doc).unwrap(), body.clone());
        // And so does a replica that only saw the snapshot.
        let replica = LoroDoc::from_snapshot(&doc.export(ExportMode::Snapshot).unwrap()).unwrap();
        prop_assert_eq!(read_tree(&replica).unwrap(), body);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases() / 3, max_global_rejects: 200_000, ..ProptestConfig::default() })]

    /// P4: merging back exactly what was exported changes nothing — even for
    /// bodies Markdown cannot represent exactly (empty paragraphs, odd marks).
    #[test]
    fn unchanged_revision_is_a_no_op(body in blocks(word_text().boxed(), false)) {
        let doc = doc_with(&body);
        let before = read_tree(&doc).unwrap();
        let md = export(&doc).unwrap();
        let merged = merge_into(&doc, &latest(&doc), &md, AGENT);
        prop_assert_eq!(merged.ops, 0, "markdown:\n{}", md);
        prop_assert_eq!(read_tree(&doc).unwrap(), before);
    }

    /// P5: after merging any revision, the body exports exactly that revision's canonical Markdown.
    #[test]
    fn any_revision_lands_exactly(
        from in blocks(word_text().boxed(), false),
        to in blocks(word_text().boxed(), false),
    ) {
        let doc = doc_with(&from);
        let revision = to_markdown(&to);
        merge_into(&doc, &latest(&doc), &revision, AGENT);
        prop_assert_eq!(export(&doc).unwrap(), revision);
    }

    /// P6: editing one top-level block touches only that block; every other
    /// block keeps its Loro identity (so concurrent edits inside them survive).
    #[test]
    fn one_block_edit_keeps_other_blocks(
        body in blocks(word_text().boxed(), false),
        pick in any::<prop::sample::Index>(),
        replacement in blocks(word_text().boxed(), false),
    ) {
        let canonical = canonicalize(&body);
        prop_assume!(canonical.len() >= 2);
        let doc = doc_with(&canonical);
        let ids = block_ids(&doc);
        let index = pick.index(canonical.len());
        let mut revised = canonical.clone();
        revised[index] = canonicalize(&replacement[..1]).into_iter().next().unwrap_or(para("x"));
        prop_assume!(revised[index] != canonical[index]);
        let revision = to_markdown(&revised);
        // Skip revisions where Markdown itself merges blocks (e.g. adjacent lists).
        prop_assume!(parse(&revision).len() == revised.len());
        merge_into(&doc, &latest(&doc), &revision, AGENT);
        // All but the edited block keep their container identity. With
        // duplicate blocks the surviving copy may be either one (an LCS tie).
        let after = block_ids(&doc);
        prop_assert_eq!(after.len(), ids.len());
        let kept = ids.iter().filter(|id| after.contains(id)).count();
        prop_assert!(kept >= ids.len() - 1, "only {} of {} blocks kept identity", kept, ids.len());
        prop_assert_eq!(export(&doc).unwrap(), revision);
    }

    /// P7: a person types in one paragraph while an agent revises another
    /// (or the same) paragraph from an older export; both edits survive.
    #[test]
    fn concurrent_human_and_agent_edits_both_survive(
        words in prop::collection::vec("[a-z]{1,8}( [a-z]{1,8}){0,5}", 2..8),
        human_block in any::<prop::sample::Index>(),
        agent_block in any::<prop::sample::Index>(),
    ) {
        let body: Vec<Block> = words.iter().map(|w| para(w)).collect();
        let server = doc_with(&body);
        let human = peer_copy(&server, HUMAN);
        let base = latest(&server);
        let exported = export(&server).unwrap();

        let h = human_block.index(body.len());
        human_types(&human, h, false, "HUMAN ");

        let a = agent_block.index(body.len());
        let mut revised = parse(&exported);
        if let Block::Paragraph(content) = &mut revised[a] {
            content.push(Inline::Text(" AGENT".into(), Marks::default()));
        }
        server.import(&human.export(ExportMode::all_updates()).unwrap()).unwrap();
        merge_into(&server, &base, &to_markdown(&revised), AGENT);
        sync(&server, &human);

        let text = plain_text(&read_tree(&server).unwrap());
        prop_assert_eq!(text.matches("HUMAN").count(), 1, "{}", text);
        prop_assert_eq!(text.matches("AGENT").count(), 1, "{}", text);
        prop_assert_eq!(read_tree(&server).unwrap(), read_tree(&human).unwrap());
    }
}

// ---------------------------------------------------------------- regressions

/// Found at 30 000 cases: comrak writes this as a link reference definition,
/// which would silently delete the paragraph.
#[test]
fn text_that_reads_as_a_reference_definition_is_not_lost() {
    let body = vec![Block::Paragraph(vec![
        Inline::Text(
            "]:".into(),
            Marks {
                code: true,
                link: Some(Link {
                    href: "https://a.test/".into(),
                    title: None,
                }),
                ..Marks::default()
            },
        ),
        Inline::Text("a".into(), Marks::default()),
    ])];
    let md = to_markdown(&body);
    assert_eq!(plain_text(&parse(&md)), plain_text(&body));
    assert_eq!(to_markdown(&parse(&md)), md);
    // The same body inside a quoted list, as the generator first found it.
    let nested = vec![Block::Blockquote(vec![Block::Ordered {
        order: 1,
        items: vec![body.clone()],
    }])];
    let md = to_markdown(&nested);
    assert_eq!(plain_text(&parse(&md)), plain_text(&nested));
    assert_eq!(to_markdown(&parse(&md)), md);
}

/// Found at 20 000 cases: with duplicate blocks, either copy may survive an edit
/// to one of them; both alignments change exactly one block.
#[test]
fn duplicate_blocks_tie_keeps_all_but_one_identity() {
    let rule_quote = Block::Blockquote(vec![Block::Rule]);
    let canonical = vec![para("a"), para("a"), rule_quote.clone()];
    let doc = doc_with(&canonical);
    let ids = block_ids(&doc);
    let revision = to_markdown(&[para("a"), rule_quote.clone(), rule_quote]);
    let merged = merge_into(&doc, &latest(&doc), &revision, AGENT);
    assert!(merged.ops > 0);
    let after = block_ids(&doc);
    assert_eq!(after.len(), 3);
    assert_eq!(ids.iter().filter(|id| after.contains(id)).count(), 2);
    assert_eq!(export(&doc).unwrap(), revision);
}

// ---------------------------------------------------------------- merge behaviour

#[test]
fn formatting_only_revision_marks_without_rewriting_text() {
    let server = doc_with(&[para("Ship it on Friday.")]);
    let run_id = |doc: &LoroDoc| match top_children(doc, 0).get(0) {
        Some(ValueOrContainer::Container(Container::Text(run))) => run.id(),
        _ => unreachable!(),
    };
    let before = run_id(&server);
    let merged = merge_into(&server, &latest(&server), "Ship it on **Friday**.\n", AGENT);
    assert_eq!(run_id(&server), before);
    // A Loro mark is two ops (its start and end anchors); no text ops.
    assert_eq!(merged.ops, 2);
}

fn top_children(doc: &LoroDoc, index: usize) -> loro::LoroList {
    match block_map(doc, index).get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => panic!("no children"),
    }
}

#[test]
fn blank_paragraphs_a_person_left_are_kept() {
    let server = doc_with(&[para("one"), Block::Paragraph(vec![]), para("two")]);
    assert_eq!(export(&server).unwrap(), "one\n\ntwo\n");
    merge_into(&server, &latest(&server), "one\n\ntwo changed\n", AGENT);
    assert_eq!(
        read_tree(&server).unwrap(),
        vec![para("one"), Block::Paragraph(vec![]), para("two changed")]
    );
}

#[test]
fn a_blank_body_is_replaced_by_the_first_revision() {
    // A new Document's genesis is one empty paragraph; the first revision edits it in place.
    let server = doc_with(&[]);
    let genesis = block_ids(&server);
    merge_into(&server, &latest(&server), "first\n\nsecond\n", AGENT);
    assert_eq!(
        read_tree(&server).unwrap(),
        vec![para("first"), para("second")]
    );
    assert_eq!(block_ids(&server)[0], genesis[0]);
}

#[test]
fn an_empty_revision_leaves_one_empty_paragraph() {
    let server = doc_with(&[para("one"), para("two")]);
    merge_into(&server, &latest(&server), "", AGENT);
    assert_eq!(read_tree(&server).unwrap(), vec![Block::Paragraph(vec![])]);
    assert_eq!(export(&server).unwrap(), "");
}

#[test]
fn merge_does_not_change_the_document() {
    let server = doc_with(&[para("one")]);
    let before = latest(&server);
    let merged = merge(&server, &before, "two\n", AGENT).unwrap();
    assert!(merged.ops > 0);
    assert_eq!(latest(&server), before);
    assert_eq!(export(&server).unwrap(), "one\n");
}

#[test]
fn an_unknown_base_is_refused() {
    let server = doc_with(&[para("one")]);
    let other = doc_with(&[para("elsewhere")]);
    other.set_peer_id(77).unwrap();
    merge_into(&other, &latest(&other), "moved on\n", 78);
    assert!(matches!(
        merge(&server, &latest(&other), "x\n", AGENT),
        Err(MergeError::BaseUnavailable)
    ));
}

#[test]
fn a_peer_with_operations_is_refused() {
    let server = doc_with(&[para("one")]);
    merge_into(&server, &latest(&server), "two\n", AGENT);
    assert!(matches!(
        merge(&server, &latest(&server), "three\n", AGENT),
        Err(MergeError::PeerInUse)
    ));
    assert!(matches!(
        merge(&server, &latest(&server), "three\n", 1),
        Err(MergeError::PeerInUse)
    ));
}

#[test]
fn a_document_without_a_body_is_refused() {
    let empty = LoroDoc::new();
    assert!(matches!(
        merge(&empty, &latest(&empty), "x\n", AGENT),
        Err(MergeError::Schema(SchemaViolation::MissingBody))
    ));
    assert_eq!(export(&empty), Err(SchemaViolation::MissingBody));
}

#[test]
fn revision_nesting_is_bounded() {
    let server = doc_with(&[para("x")]);
    // Blockquotes `depth - 1` deep around a paragraph is `depth` tree nodes.
    let quoted = |depth: usize| format!("{}deep\n", "> ".repeat(depth - 1));
    merge_into(&server, &latest(&server), &quoted(MAX_NESTING), AGENT);
    assert!(export(&server).unwrap().ends_with("deep\n"));
    assert!(matches!(
        merge(
            &server,
            &latest(&server),
            &quoted(MAX_NESTING + 1),
            AGENT + 1
        ),
        Err(MergeError::Schema(SchemaViolation::TooDeep))
    ));
    // Far deeper hostile input is refused before any recursive walk.
    for hostile in ["> ".repeat(100_000), "1. ".repeat(50_000)] {
        assert!(matches!(
            merge(&server, &latest(&server), &hostile, AGENT + 2),
            Err(MergeError::Schema(SchemaViolation::TooDeep))
        ));
    }
    // Inline syntax that does not nest is merged as ordinary text.
    for flat in ["*a ".repeat(30_000), "[".repeat(100_000)] {
        assert!(merge(&server, &latest(&server), &flat, AGENT + 2).is_ok());
    }
}
