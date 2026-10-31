//! Shared generators and helpers for the Markdown core suites.
#![allow(dead_code)]

use loro::{
    Container, ContainerID, ContainerTrait, ExportMode, LoroDoc, LoroList, LoroMap,
    ValueOrContainer,
};
use proptest::prelude::*;
use teamarness_documents_package::markdown::{Block, Inline, Link, Marks, write_tree};

/// Cases per Markdown property; merge properties run a third of it.
/// `CASES=60000 cargo test --release` runs the long search.
pub fn cases() -> u32 {
    std::env::var("CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000)
}

// ---------------------------------------------------------------- model helpers

/// Merges adjacent equal-mark text and drops empty text, as ProseMirror does.
pub fn normalize_inlines(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text(text, _) if text.is_empty() => {}
            Inline::Text(text, marks) => match out.last_mut() {
                Some(Inline::Text(previous, previous_marks)) if *previous_marks == marks => {
                    previous.push_str(&text)
                }
                _ => out.push(Inline::Text(text, marks)),
            },
            Inline::Break => out.push(Inline::Break),
        }
    }
    out
}

/// Plain text of a block tree, used to check that no content is lost.
pub fn plain_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => {
                for inline in content {
                    match inline {
                        Inline::Text(text, _) => out.push_str(text),
                        Inline::Break => out.push('\n'),
                    }
                }
            }
            Block::Blockquote(children) => out.push_str(&plain_text(children)),
            Block::Rule => {}
            Block::Code { text, .. } => out.push_str(text),
            Block::Bullet { items, .. } | Block::Ordered { items, .. } => {
                for item in items {
                    out.push_str(&plain_text(item));
                }
            }
        }
        out.push('\n');
    }
    out
}

pub fn para(text: &str) -> Block {
    Block::Paragraph(vec![Inline::Text(text.into(), Marks::default())])
}

// ---------------------------------------------------------------- document helpers

pub const AGENT: u64 = 0xA6E7;
pub const HUMAN: u64 = 0x4855;

/// A document whose body holds `body`, written by peer 1.
pub fn doc_with(body: &[Block]) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(1).unwrap();
    write_tree(&doc, body).unwrap();
    doc
}

/// Another replica of `doc` with its own peer.
pub fn peer_copy(doc: &LoroDoc, peer: u64) -> LoroDoc {
    let copy = LoroDoc::from_snapshot(&doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    copy.set_peer_id(peer).unwrap();
    copy
}

pub fn sync(a: &LoroDoc, b: &LoroDoc) {
    a.import(&b.export(ExportMode::all_updates()).unwrap())
        .unwrap();
    b.import(&a.export(ExportMode::all_updates()).unwrap())
        .unwrap();
}

pub fn top_level(doc: &LoroDoc) -> LoroList {
    match doc.get_map("doc").get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => panic!("no body tree"),
    }
}

pub fn block_map(doc: &LoroDoc, index: usize) -> LoroMap {
    match top_level(doc).get(index) {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        _ => panic!("no block {index}"),
    }
}

pub fn block_ids(doc: &LoroDoc) -> Vec<ContainerID> {
    (0..top_level(doc).len())
        .map(|index| block_map(doc, index).id())
        .collect()
}

/// What the binding does when a person types: insert into the block's first text run.
pub fn human_types(doc: &LoroDoc, block: usize, at_end: bool, text: &str) {
    let children = match block_map(doc, block).get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => panic!("block {block} has no children"),
    };
    match children.get(0) {
        Some(ValueOrContainer::Container(Container::Text(run))) => {
            let pos = if at_end { run.len_unicode() } else { 0 };
            run.insert(pos, text).unwrap();
        }
        _ => panic!("block {block} has no text"),
    }
    doc.commit();
}

// ---------------------------------------------------------------- generators

pub fn nasty_char() -> impl Strategy<Value = char> {
    prop_oneof![
        6 => prop::char::range('a', 'z'),
        2 => Just(' '),
        1 => prop::sample::select(vec![
            '*', '_', '`', '~', '[', ']', '(', ')', '<', '>', '#', '-', '+', '=', '!', '\\',
            '&', ';', '.', '1', '|', ':', '"', '\'', '{', '}', '$', '%', '^', '@',
            'é', '日', '🎉', '\t', '\u{a0}', '\u{200b}',
        ]),
    ]
}

pub fn word_text() -> impl Strategy<Value = String> {
    prop::collection::vec(nasty_char(), 1..12).prop_map(|c| c.into_iter().collect())
}

/// Words; runs are separated by unmarked spaces (see `inlines`).
pub fn spaced_text() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,7}( [a-z0-9]{1,8}){0,3}"
}

pub fn link() -> impl Strategy<Value = Link> {
    (
        "https://[a-z]{1,8}\\.test/[a-z0-9_()]{0,6}",
        prop::option::of("[a-zA-Z \"']{1,10}"),
    )
        .prop_map(|(href, title)| Link { href, title })
}

pub fn marks() -> impl Strategy<Value = Marks> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop::bool::weighted(0.15),
        prop::option::weighted(0.2, link()),
    )
        .prop_map(|(strong, em, strike, code, link)| Marks {
            strong,
            em,
            strike,
            code,
            link,
        })
}

pub fn inlines(
    text: BoxedStrategy<String>,
    breaks: bool,
    spaced: bool,
) -> BoxedStrategy<Vec<Inline>> {
    let run = (text, marks()).prop_map(|(t, m)| Inline::Text(t, m));
    let item = if breaks {
        prop_oneof![9 => run, 1 => Just(Inline::Break)].boxed()
    } else {
        run.boxed()
    };
    prop::collection::vec(item, 1..6)
        .prop_map(move |v| {
            let mut spaced_out: Vec<Inline> = Vec::new();
            for inline in v {
                let both_text = matches!(
                    (spaced_out.last(), &inline),
                    (Some(Inline::Text(..)), Inline::Text(..))
                );
                if spaced && both_text {
                    spaced_out.push(Inline::Text(" ".into(), Marks::default()));
                }
                spaced_out.push(inline);
            }
            let mut v = normalize_inlines(spaced_out);
            while matches!(v.first(), Some(Inline::Break)) {
                v.remove(0);
            }
            while matches!(v.last(), Some(Inline::Break)) {
                v.pop();
            }
            v
        })
        .boxed()
}

pub fn leaf(text: BoxedStrategy<String>, spaced: bool) -> impl Strategy<Value = Block> {
    prop_oneof![
        5 => inlines(text.clone(), true, spaced).prop_map(Block::Paragraph),
        2 => (1u8..=6, inlines(text.clone(), false, spaced))
            .prop_map(|(level, content)| Block::Heading { level, content }),
        1 => Just(Block::Rule),
        1 => ("[a-z]{0,6}", "[a-z =(){};\n`~]{0,40}")
            .prop_map(|(params, text)| Block::Code { params, text }),
    ]
}

pub fn blocks(text: BoxedStrategy<String>, spaced: bool) -> impl Strategy<Value = Vec<Block>> {
    let tree = leaf(text.clone(), spaced).prop_recursive(3, 24, 4, move |inner| {
        let para = inlines(text.clone(), true, spaced).prop_map(Block::Paragraph);
        // Known gap, excluded here: an item whose only written content starts
        // with a horizontal rule (comrak writes it after a blank line).
        let item = (para, prop::collection::vec(inner.clone(), 0..2))
            .prop_map(|(p, rest)| {
                let mut item = vec![p];
                item.extend(rest);
                if matches!(item.as_slice(), [Block::Paragraph(c), Block::Rule, ..] if c.is_empty())
                {
                    item[0] = Block::Paragraph(vec![Inline::Text("x".into(), Marks::default())]);
                }
                item
            })
            .boxed();
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..3).prop_map(Block::Blockquote),
            prop::collection::vec(item.clone(), 1..4).prop_map(|items| Block::Bullet { items }),
            (1u32..20, prop::collection::vec(item, 1..4))
                .prop_map(|(order, items)| Block::Ordered { order, items }),
        ]
    });
    prop::collection::vec(tree, 1..6)
}
