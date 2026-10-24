//! Markdown export and import through comrak (CommonMark plus GFM strikethrough), and the
//! canonicalisation layer that makes the export byte-stable.

use std::borrow::Cow;

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{
    AstNode, ListDelimType, ListType, NodeCode, NodeCodeBlock, NodeHeading, NodeLink, NodeList,
    NodeValue,
};
use comrak::options::ListStyleType;
use comrak::{Arena, Options, format_commonmark, parse_document};
use loro::LoroDoc;

use super::schema::{
    Block, Inline, Link, MAX_NESTING, Marks, SchemaViolation, check_blocks, normalize_inlines,
    read_tree,
};

/// Exports the body as Canonical Markdown.
///
/// The export always imports back to the body it describes, minus what Markdown cannot carry
/// (see the module documentation), and exporting that import gives the same bytes.
pub fn export(doc: &LoroDoc) -> Result<String, SchemaViolation> {
    Ok(serialize(&read_tree(doc)?))
}

/// The Canonical Markdown of a revision from outside the backend: what [`export`] gives once
/// `markdown` is the body. Nesting and the schema are checked as for a merge.
pub fn canonical(markdown: &str) -> Result<String, SchemaViolation> {
    Ok(serialize(&parse_revision(markdown)?))
}

/// Inline formatting may nest this much deeper than the block limit in comrak's syntax tree.
const INLINE_NESTING: usize = 16;

/// Imports Markdown from outside the backend: its nesting is checked before any recursive
/// walk, and the blocks must be writable under the schema.
pub(super) fn parse_revision(markdown: &str) -> Result<Vec<Block>, SchemaViolation> {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options());
    let mut depth = 0usize;
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(_) => {
                depth += 1;
                // The document node itself is depth 1 here.
                if depth > MAX_NESTING + INLINE_NESTING + 1 {
                    return Err(SchemaViolation::TooDeep);
                }
            }
            NodeEdge::End(_) => depth -= 1,
        }
    }
    let blocks = blocks_of(root);
    check_blocks(&blocks)?;
    Ok(blocks)
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.render.list_style = ListStyleType::Dash;
    options.render.prefer_fenced = true;
    options.render.width = 0;
    options
}

/// Comrak separates adjacent lists of the same kind with this comment.
const LIST_SEPARATOR: &str = "<!-- end list -->";

// ---------------------------------------------------------------- import

/// Imports Markdown as blocks. Callers bound its nesting first (see [`parse_revision`]).
pub fn parse(markdown: &str) -> Vec<Block> {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &options());
    blocks_of(root)
}

fn blocks_of<'a>(node: &'a AstNode<'a>) -> Vec<Block> {
    node.children().filter_map(block_of).collect()
}

fn block_of<'a>(node: &'a AstNode<'a>) -> Option<Block> {
    let value = node.data.borrow().value.clone();
    Some(match value {
        NodeValue::Paragraph => Block::Paragraph(inlines_of(node)),
        NodeValue::Heading(heading) => Block::Heading {
            level: heading.level,
            content: inlines_of(node)
                .into_iter()
                .filter(|inline| !matches!(inline, Inline::Break))
                .collect(),
        },
        NodeValue::BlockQuote => Block::Blockquote(non_empty(blocks_of(node))),
        NodeValue::ThematicBreak => Block::Rule,
        NodeValue::CodeBlock(code) => {
            let mut text = code.literal.clone();
            if text.ends_with('\n') {
                text.pop();
            }
            Block::Code {
                params: code.info.clone(),
                text,
            }
        }
        NodeValue::List(list) => {
            let items = node
                .children()
                .map(|item| list_item(blocks_of(item)))
                .collect();
            match list.list_type {
                ListType::Bullet => Block::Bullet { items },
                ListType::Ordered => Block::Ordered {
                    order: list.start as u32,
                    items,
                },
            }
        }
        NodeValue::HtmlBlock(html) if html.literal.trim() == LIST_SEPARATOR => return None,
        // Not in the baseline: kept as literal text so nothing an author wrote is lost.
        NodeValue::HtmlBlock(html) => Block::Paragraph(vec![Inline::Text(
            html.literal.trim_end().to_owned(),
            Marks::default(),
        )]),
        _ => return None,
    })
}

/// Schema: `list_item` is `paragraph block*`.
fn list_item(mut blocks: Vec<Block>) -> Vec<Block> {
    if !matches!(blocks.first(), Some(Block::Paragraph(_))) {
        blocks.insert(0, Block::Paragraph(Vec::new()));
    }
    blocks
}

/// Schema: `blockquote` is `block+`.
fn non_empty(mut blocks: Vec<Block>) -> Vec<Block> {
    if blocks.is_empty() {
        blocks.push(Block::Paragraph(Vec::new()));
    }
    blocks
}

fn inlines_of<'a>(node: &'a AstNode<'a>) -> Vec<Inline> {
    let mut out = Vec::new();
    collect_inlines(node, &Marks::default(), &mut out);
    let mut out = normalize_inlines(out);
    while matches!(out.last(), Some(Inline::Break)) {
        out.pop();
    }
    out
}

fn collect_inlines<'a>(node: &'a AstNode<'a>, marks: &Marks, out: &mut Vec<Inline>) {
    for child in node.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Text(text) => out.push(Inline::Text(text.into_owned(), marks.clone())),
            NodeValue::SoftBreak => out.push(Inline::Text(" ".to_owned(), marks.clone())),
            NodeValue::LineBreak => out.push(Inline::Break),
            NodeValue::Code(code) => out.push(Inline::Text(
                code.literal,
                Marks {
                    code: true,
                    ..marks.clone()
                },
            )),
            NodeValue::Emph => collect_inlines(
                child,
                &Marks {
                    em: true,
                    ..marks.clone()
                },
                out,
            ),
            NodeValue::Strong => collect_inlines(
                child,
                &Marks {
                    strong: true,
                    ..marks.clone()
                },
                out,
            ),
            NodeValue::Strikethrough => collect_inlines(
                child,
                &Marks {
                    strike: true,
                    ..marks.clone()
                },
                out,
            ),
            NodeValue::Link(link) => collect_inlines(
                child,
                &Marks {
                    link: Some(Link {
                        href: link.url.clone(),
                        title: (!link.title.is_empty()).then(|| link.title.clone()),
                    }),
                    ..marks.clone()
                },
                out,
            ),
            // Not in the baseline: kept as literal text.
            NodeValue::HtmlInline(html) => out.push(Inline::Text(html, marks.clone())),
            NodeValue::Image(image) => {
                let mut alt = Vec::new();
                collect_inlines(child, &Marks::default(), &mut alt);
                let alt: String = alt
                    .iter()
                    .filter_map(|inline| match inline {
                        Inline::Text(text, _) => Some(text.as_str()),
                        Inline::Break => None,
                    })
                    .collect();
                out.push(Inline::Text(
                    format!("![{alt}]({})", image.url),
                    marks.clone(),
                ));
            }
            _ => collect_inlines(child, marks, out),
        }
    }
}

// ---------------------------------------------------------------- export

/// Writes canonical Markdown: formatting that CommonMark cannot express
/// around punctuation or whitespace is dropped first, so the output always
/// imports back to the body it was produced from.
pub(super) fn serialize(blocks: &[Block]) -> String {
    serialize_raw(&canonicalize(blocks))
}

fn serialize_raw(blocks: &[Block]) -> String {
    let arena = Arena::new();
    let root = node(&arena, NodeValue::Document);
    for block in blocks {
        root.append(block_node(&arena, block));
    }
    let mut out = String::new();
    format_commonmark(root, &options(), &mut out).expect("formatting into a String");
    out
}

fn node<'a>(arena: &'a Arena<'a>, value: NodeValue) -> &'a AstNode<'a> {
    arena.alloc(AstNode::from(value))
}

fn block_node<'a>(arena: &'a Arena<'a>, block: &Block) -> &'a AstNode<'a> {
    match block {
        Block::Paragraph(content) => {
            let paragraph = node(arena, NodeValue::Paragraph);
            append_inlines(arena, paragraph, content);
            paragraph
        }
        Block::Heading { level, content } => {
            let heading = node(
                arena,
                NodeValue::Heading(NodeHeading {
                    level: *level,
                    setext: false,
                    closed: false,
                }),
            );
            append_inlines(arena, heading, content);
            heading
        }
        Block::Blockquote(children) => {
            let quote = node(arena, NodeValue::BlockQuote);
            for child in children {
                quote.append(block_node(arena, child));
            }
            quote
        }
        Block::Rule => node(arena, NodeValue::ThematicBreak),
        Block::Code { params, text } => node(
            arena,
            NodeValue::CodeBlock(Box::new(NodeCodeBlock {
                fenced: true,
                fence_char: b'`',
                fence_length: 3,
                info: params.clone(),
                literal: format!("{text}\n"),
                closed: true,
                ..Default::default()
            })),
        ),
        Block::Bullet { items } => list_node(arena, ListType::Bullet, 1, items),
        Block::Ordered { order, items } => {
            list_node(arena, ListType::Ordered, *order as usize, items)
        }
    }
}

fn list_node<'a>(
    arena: &'a Arena<'a>,
    list_type: ListType,
    start: usize,
    items: &[Vec<Block>],
) -> &'a AstNode<'a> {
    let list = NodeList {
        list_type,
        start,
        tight: can_be_tight(items),
        delimiter: ListDelimType::Period,
        bullet_char: b'-',
        ..Default::default()
    };
    let list_node = node(arena, NodeValue::List(list));
    for item in items {
        let item_node = node(arena, NodeValue::Item(list));
        for child in written(item) {
            item_node.append(block_node(arena, child));
        }
        list_node.append(item_node);
    }
    list_node
}

/// Marks nest outermost-first in this order; code is innermost because a
/// code span cannot contain other markup.
fn mark_stack(marks: &Marks) -> Vec<MarkKind> {
    let mut stack = Vec::new();
    if let Some(link) = &marks.link {
        stack.push(MarkKind::Link(link.clone()));
    }
    if marks.strong {
        stack.push(MarkKind::Strong);
    }
    if marks.em {
        stack.push(MarkKind::Em);
    }
    if marks.strike {
        stack.push(MarkKind::Strike);
    }
    stack
}

#[derive(Clone, PartialEq)]
enum MarkKind {
    Link(Link),
    Strong,
    Em,
    Strike,
}

fn append_inlines<'a>(arena: &'a Arena<'a>, parent: &'a AstNode<'a>, content: &[Inline]) {
    // Open containers, outermost first: (kind, node).
    let mut open: Vec<(MarkKind, &'a AstNode<'a>)> = Vec::new();
    let stacks: Vec<Option<Vec<MarkKind>>> = content
        .iter()
        .map(|inline| match inline {
            Inline::Text(_, marks) => Some(mark_stack(marks)),
            Inline::Break => None,
        })
        .collect();
    for (index, inline) in content.iter().enumerate() {
        let (wanted, leaf) = match inline {
            Inline::Text(text, marks) => (
                mark_stack(marks),
                if marks.code {
                    node(
                        arena,
                        NodeValue::Code(NodeCode {
                            num_backticks: 1,
                            literal: text.clone(),
                        }),
                    )
                } else {
                    node(arena, NodeValue::Text(Cow::Owned(text.clone())))
                },
            ),
            // A break stays inside the marks shared by the text on both sides.
            Inline::Break => {
                let before = stacks[..index].iter().rev().find_map(Clone::clone);
                let after = stacks[index + 1..].iter().find_map(Clone::clone);
                let shared = match (before, after) {
                    (Some(before), Some(after)) => before
                        .into_iter()
                        .zip(after)
                        .take_while(|(a, b)| a == b)
                        .map(|(a, _)| a)
                        .collect(),
                    _ => Vec::new(),
                };
                (shared, node(arena, NodeValue::LineBreak))
            }
        };
        let keep = open
            .iter()
            .zip(&wanted)
            .take_while(|((kind, _), want)| kind == *want)
            .count();
        open.truncate(keep);
        for kind in &wanted[keep..] {
            let container = node(
                arena,
                match kind {
                    MarkKind::Link(link) => NodeValue::Link(Box::new(NodeLink {
                        url: link.href.clone(),
                        title: link.title.clone().unwrap_or_default(),
                    })),
                    MarkKind::Strong => NodeValue::Strong,
                    MarkKind::Em => NodeValue::Emph,
                    MarkKind::Strike => NodeValue::Strikethrough,
                },
            );
            open.last().map_or(parent, |(_, n)| *n).append(container);
            open.push((kind.clone(), container));
        }
        open.last().map_or(parent, |(_, n)| *n).append(leaf);
    }
}

// ---------------------------------------------------------------- canonical form

/// The body as Markdown can carry it. Identity for anything Markdown expresses.
pub fn canonicalize(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .map(canonical_block)
        .filter(|block| !is_empty_paragraph(block))
        .collect()
}

fn is_empty_paragraph(block: &Block) -> bool {
    matches!(block, Block::Paragraph(content) if content.is_empty())
}

/// Schema `list_item`: `paragraph block*` — the leading paragraph stays even when empty.
fn canonical_item(item: &[Block]) -> Vec<Block> {
    list_item(canonicalize(item))
}

fn canonical_block(block: &Block) -> Block {
    match block {
        Block::Paragraph(content) => Block::Paragraph(canonical_inlines(content, false)),
        Block::Heading { level, content } => Block::Heading {
            level: *level,
            content: canonical_inlines(content, true),
        },
        Block::Blockquote(children) => Block::Blockquote(non_empty(canonicalize(children))),
        Block::Bullet { items } => Block::Bullet {
            items: items.iter().map(|item| canonical_item(item)).collect(),
        },
        Block::Ordered { order, items } => Block::Ordered {
            order: *order,
            items: items.iter().map(|item| canonical_item(item)).collect(),
        },
        other => other.clone(),
    }
}

/// The blocks an item shows in Markdown: the schema's empty leading paragraph is not written.
fn written(item: &[Block]) -> &[Block] {
    match item {
        [Block::Paragraph(content), rest @ ..] if content.is_empty() && !rest.is_empty() => rest,
        _ => item,
    }
}

/// In a tight list blocks run straight into each other, so any pair where
/// the next block would be absorbed by the previous one forces a loose list.
fn can_be_tight(items: &[Vec<Block>]) -> bool {
    items.iter().all(|item| tight_safe(written(item)))
}

/// Comrak writes every block under a tight item without blank lines, down
/// through blockquotes until the next list.
fn tight_safe(blocks: &[Block]) -> bool {
    blocks.windows(2).all(|pair| match pair {
        [Block::Heading { .. } | Block::Code { .. } | Block::Rule, _] => true,
        [Block::Bullet { .. }, Block::Bullet { .. }]
        | [Block::Ordered { .. }, Block::Ordered { .. }] => false,
        [_, next] => interrupts_paragraph(next),
        _ => true,
    }) && blocks.iter().all(|block| match block {
        Block::Blockquote(children) => tight_safe(children),
        _ => true,
    })
}

/// Conservative: only blocks that interrupt a paragraph and render on their
/// own lines without blank lines inside them.
fn interrupts_paragraph(block: &Block) -> bool {
    match block {
        Block::Ordered { order, items } => {
            *order == 1 && can_be_tight(items) && !starts_empty(items)
        }
        Block::Bullet { items } => can_be_tight(items) && !starts_empty(items),
        Block::Code { .. } | Block::Heading { .. } => true,
        _ => false,
    }
}

/// An empty first item cannot interrupt a paragraph.
fn starts_empty(items: &[Vec<Block>]) -> bool {
    matches!(items.first().and_then(|item| item.first()), Some(Block::Paragraph(content)) if content.is_empty())
}

#[derive(Clone, Copy, PartialEq)]
enum Delimited {
    Strong,
    Em,
    Strike,
}

const DELIMITED: [Delimited; 3] = [Delimited::Strong, Delimited::Em, Delimited::Strike];

fn has(marks: &Marks, kind: Delimited) -> bool {
    match kind {
        Delimited::Strong => marks.strong,
        Delimited::Em => marks.em,
        Delimited::Strike => marks.strike,
    }
}

fn unset(marks: &mut Marks, kind: Delimited) {
    match kind {
        Delimited::Strong => marks.strong = false,
        Delimited::Em => marks.em = false,
        Delimited::Strike => marks.strike = false,
    }
}

/// One character (or a break) with its marks.
type Item = (Option<char>, Marks);

fn flatten(content: &[Inline]) -> Vec<Item> {
    let mut items = Vec::new();
    for inline in content {
        match inline {
            Inline::Text(text, marks) => {
                items.extend(text.chars().map(|c| (Some(c), marks.clone())))
            }
            Inline::Break => items.push((None, Marks::default())),
        }
    }
    items
}

fn unflatten(items: &[Item]) -> Vec<Inline> {
    normalize_inlines(
        items
            .iter()
            .map(|(c, marks)| match c {
                Some(c) => Inline::Text(c.to_string(), marks.clone()),
                None => Inline::Break,
            })
            .collect(),
    )
}

/// Maximal character ranges carrying `kind`; breaks inside a span do not end it.
fn spans(items: &[Item], kind: Delimited) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut end = 0;
    for (index, (c, marks)) in items.iter().enumerate() {
        if c.is_none() {
            continue;
        }
        if has(marks, kind) {
            start.get_or_insert(index);
            end = index + 1;
        } else if let Some(s) = start.take() {
            out.push((s, end));
        }
    }
    if let Some(s) = start {
        out.push((s, end));
    }
    out
}

fn drop_span(items: &mut [Item], kind: Delimited, (start, end): (usize, usize)) {
    for item in &mut items[start..end] {
        unset(&mut item.1, kind);
    }
}

/// Moves whitespace at the edges of a delimited span outside it, as editors do.
fn trim_edges(items: &mut [Item]) {
    let blank = |item: &Item| item.0.is_none_or(char::is_whitespace);
    for kind in DELIMITED {
        for (mut start, mut end) in spans(items, kind) {
            while start < end && blank(&items[start]) {
                unset(&mut items[start].1, kind);
                start += 1;
            }
            while end > start && blank(&items[end - 1]) {
                unset(&mut items[end - 1].1, kind);
                end -= 1;
            }
        }
    }
}

/// Whitespace next to a change in delimited formatting loses that
/// formatting, so delimiters can sit against words (invisible on screen).
fn trim_boundaries(items: &mut [Item]) {
    let delimited = |marks: &Marks| DELIMITED.map(|kind| has(marks, kind));
    let snapshot: Vec<Item> = items.to_vec();
    for (index, (c, marks)) in items.iter_mut().enumerate() {
        if !c.is_some_and(char::is_whitespace) {
            continue;
        }
        let here = delimited(marks);
        let differs = |other: Option<&Item>| {
            other.is_some_and(|(oc, om)| oc.is_some() && delimited(om) != here)
        };
        if differs(index.checked_sub(1).and_then(|i| snapshot.get(i)))
            || differs(snapshot.get(index + 1))
        {
            for kind in DELIMITED {
                unset(marks, kind);
            }
        }
    }
}

fn round_trips(content: &[Inline], heading: bool) -> bool {
    let block = if heading {
        Block::Heading {
            level: 1,
            content: content.to_vec(),
        }
    } else {
        Block::Paragraph(content.to_vec())
    };
    let parsed = parse(&serialize_raw(std::slice::from_ref(&block)));
    match (parsed.as_slice(), content.is_empty()) {
        ([], true) => true,
        ([only], _) => *only == block,
        _ => false,
    }
}

/// Markdown drops spaces at the start of a line and at the end of a block.
fn strip_line_edges(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for item in items {
        let at_line_start = matches!(out.last(), None | Some((None, _)));
        if at_line_start && item.0 == Some(' ') {
            continue;
        }
        out.push(item);
    }
    while matches!(out.last(), Some((Some(' ') | None, _))) {
        out.pop();
    }
    while matches!(out.first(), Some((None, _))) {
        out.remove(0);
    }
    out
}

fn canonical_inlines(content: &[Inline], heading: bool) -> Vec<Inline> {
    if round_trips(content, heading) {
        return content.to_vec();
    }
    let stripped = strip_line_edges(flatten(content));
    let without_code: Vec<Item> = stripped
        .iter()
        .map(|(c, m)| {
            (
                *c,
                Marks {
                    code: false,
                    ..m.clone()
                },
            )
        })
        .collect();
    let without_links: Vec<Item> = without_code
        .iter()
        .map(|(c, m)| {
            (
                *c,
                Marks {
                    link: None,
                    ..m.clone()
                },
            )
        })
        .collect();
    // Content beats formatting: fall back to dropping code, then links, when
    // comrak's writer cannot carry them (e.g. text that reads as a link
    // reference definition).
    for candidate in [stripped, without_code, without_links] {
        let bare: Vec<Item> = candidate
            .iter()
            .map(|(c, m)| {
                let mut m = m.clone();
                for kind in DELIMITED {
                    unset(&mut m, kind);
                }
                (*c, m)
            })
            .collect();
        if round_trips(&unflatten(&bare), heading) {
            return repair_delimiters(candidate, heading);
        }
    }
    content.to_vec()
}

fn repair_delimiters(mut items: Vec<Item>, heading: bool) -> Vec<Inline> {
    trim_edges(&mut items);
    if !round_trips(&unflatten(&items), heading) {
        let mut trimmed = items.clone();
        trim_boundaries(&mut trimmed);
        if round_trips(&unflatten(&trimmed), heading) {
            return unflatten(&trimmed);
        }
    }
    loop {
        let current = unflatten(&items);
        if round_trips(&current, heading) {
            return current;
        }
        let candidates: Vec<(Delimited, (usize, usize))> = DELIMITED
            .iter()
            .flat_map(|&kind| {
                spans(&items, kind)
                    .into_iter()
                    .map(move |span| (kind, span))
            })
            .collect();
        let mut by_size = candidates.clone();
        by_size.sort_by_key(|(_, (start, end))| end - start);
        let single_fix = by_size.iter().find(|(kind, span)| {
            let mut trial = items.clone();
            drop_span(&mut trial, *kind, *span);
            round_trips(&unflatten(&trial), heading)
        });
        let (kind, span) = *single_fix.unwrap_or_else(|| {
            candidates
                .iter()
                .min_by_key(|(_, (start, _))| *start)
                .expect("bare content round-trips, so some span remains")
        });
        drop_span(&mut items, kind, span);
    }
}
