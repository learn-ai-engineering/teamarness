//! Markdown export and import through comrak (CommonMark plus GFM strikethrough), and the
//! canonicalisation layer that makes the export byte-stable.
//!
//! Import also recognises GFM tables, YAML front matter and HTML blocks as Preserved Blocks. The
//! writer never needs those extensions: a Preserved Block is written as raw text, so the
//! extensions cannot change how anything else is escaped.

use std::borrow::Cow;
use std::collections::HashMap;

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{
    AstNode, ListDelimType, ListType, NodeCode, NodeCodeBlock, NodeHeading, NodeLink, NodeList,
    NodeValue,
};
use comrak::options::ListStyleType;
use comrak::{Arena, Options, format_commonmark, parse_document};
use loro::LoroDoc;

use super::schema::{
    Block, Inline, Link, MAX_NESTING, Marks, PreservedKind, SchemaViolation, check_blocks,
    normalize_inlines, read_tree,
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

/// The deepest syntax tree import walks. The walk is recursive, and a stack overflow aborts the
/// process, so every import that can see stored or outside text goes through this bound.
const MAX_IMPORT_DEPTH: usize = MAX_NESTING + INLINE_NESTING + 1;

/// Imports Markdown from outside the backend: its nesting is checked before any recursive
/// walk, and the blocks must be writable under the schema.
pub(super) fn parse_revision(markdown: &str) -> Result<Vec<Block>, SchemaViolation> {
    let blocks = import(markdown, Recognise::Checked, MAX_IMPORT_DEPTH, true)?;
    check_blocks(&blocks)?;
    Ok(blocks)
}

/// Imports Markdown the backend wrote from stored text, which can read as any nesting. A
/// [`SchemaViolation::TooDeep`] means the text does not import as what it was written from.
pub(super) fn reimport(markdown: &str) -> Result<Vec<Block>, SchemaViolation> {
    import(markdown, Recognise::Checked, MAX_IMPORT_DEPTH, false)
}

/// The options that write Markdown, and that import it where nothing is recognised.
fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.render.list_style = ListStyleType::Dash;
    options.render.prefer_fenced = true;
    options.render.width = 0;
    options
}

/// The options that import Markdown with Preserved Blocks recognised.
fn import_options(front_matter: bool) -> Options<'static> {
    let mut options = options();
    options.extension.table = true;
    if front_matter {
        options.extension.front_matter_delimiter = Some(FRONT_MATTER_DELIMITER.to_owned());
    }
    options
}

/// Comrak separates adjacent lists of the same kind with this comment.
const LIST_SEPARATOR: &str = "<!-- end list -->";

const FRONT_MATTER_DELIMITER: &str = "---";

// ---------------------------------------------------------------- import

/// How import treats the constructs a Preserved Block can hold.
#[derive(Clone, Copy, PartialEq)]
enum Recognise {
    /// A construct becomes a Preserved Block only if its source is sound; otherwise it is
    /// imported as literal text.
    Checked,
    /// Every construct becomes a Preserved Block, as the soundness check itself needs.
    Unchecked,
    /// Nothing is recognised. The parser has no extensions for them, and HTML is literal text.
    Never,
}

/// Imports Markdown as blocks with no bound on its nesting, for the property suites on Markdown
/// they generate. The backend uses [`parse_revision`] or [`reimport`].
pub fn parse(markdown: &str) -> Vec<Block> {
    import(markdown, Recognise::Checked, usize::MAX, true).expect("the nesting is not bounded here")
}

fn import(
    markdown: &str,
    recognise: Recognise,
    max_depth: usize,
    budget: bool,
) -> Result<Vec<Block>, SchemaViolation> {
    if recognise == Recognise::Never {
        let text = Cow::Borrowed(markdown);
        let arena = Arena::new();
        let root = parse_document(&arena, &text, &options());
        check_depth(root, max_depth)?;
        return Ok(blocks_of(root, &Source::new(&text, recognise)));
    }
    let text = if budget {
        with_budget(markdown, front_matter_end(markdown))
    } else {
        Cow::Borrowed(markdown)
    };
    let source = Source::new(&text, recognise);
    let arena = Arena::new();
    let root = parse_document(&arena, &text, &import_options(true));
    check_depth(root, max_depth)?;
    let opening_is_literal = recognise == Recognise::Checked
        && root.first_child().is_some_and(|first| {
            matches!(&first.data.borrow().value, NodeValue::FrontMatter(literal)
                if !is_sound(PreservedKind::FrontMatter, &front_matter_source(literal)))
        });
    if !opening_is_literal {
        return Ok(blocks_of(root, &source));
    }
    // What opened the document is not front matter after all, such as two horizontal rules.
    let text = if budget {
        with_budget(markdown, 0)
    } else {
        Cow::Borrowed(markdown)
    };
    let source = Source::new(&text, recognise);
    let arena = Arena::new();
    let root = parse_document(&arena, &text, &import_options(false));
    check_depth(root, max_depth)?;
    Ok(blocks_of(root, &source))
}

/// The most comrak's reference budget can be.
const REFERENCE_BUDGET: usize = 100_000;

/// Where front matter ends, if the text opens with it.
fn front_matter_end(markdown: &str) -> usize {
    let bom = if markdown.starts_with('\u{feff}') {
        3
    } else {
        0
    };
    if !markdown.contains("]:") || !markdown[bom..].starts_with(FRONT_MATTER_DELIMITER) {
        return 0;
    }
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &import_options(true));
    root.first_child()
        .map_or(0, |first| match &first.data.borrow().value {
            NodeValue::FrontMatter(literal) => bom + literal.len(),
            _ => 0,
        })
}

/// Comrak stops resolving references once the URLs and titles they repeat exceed the size of
/// the text it parsed, up to [`REFERENCE_BUDGET`]. One line of spaces where a block cannot be
/// open gives every text the full budget and changes no parse. The line goes after a byte order
/// mark, which only counts on the first line, and after front matter, which must open the text.
/// Parsing and source lines both read the returned text.
fn with_budget(markdown: &str, after: usize) -> Cow<'_, str> {
    let body = markdown.len() - after;
    if !markdown.contains("]:") || body == 0 || body >= REFERENCE_BUDGET {
        return Cow::Borrowed(markdown);
    }
    let at = if after == 0 && markdown.starts_with('\u{feff}') {
        3
    } else {
        after
    };
    let mut padded = String::with_capacity(markdown.len() + REFERENCE_BUDGET);
    padded.push_str(&markdown[..at]);
    padded.extend(std::iter::repeat_n(' ', REFERENCE_BUDGET - body));
    padded.push('\n');
    padded.push_str(&markdown[at..]);
    Cow::Owned(padded)
}

/// The number of each ordered list item's marker, as written. Export numbers the items from
/// their list's start, so the numbers are syntax and not text. The caller has already imported
/// `markdown` within the nesting bound; this walk does not recurse.
pub(super) fn item_numbers(markdown: &str) -> Vec<&str> {
    let lines = source_lines(markdown);
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &import_options(true));
    let mut numbers = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        let NodeValue::Item(item) = &data.value else {
            continue;
        };
        if item.list_type != ListType::Ordered {
            continue;
        }
        let position = data.sourcepos.start;
        let marker = position
            .line
            .checked_sub(1)
            .and_then(|line| lines.get(line))
            .and_then(|line| line.get(position.column.checked_sub(1)?..));
        if let Some(marker) = marker {
            let digits = marker.len()
                - marker
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .len();
            if digits > 0 {
                numbers.push(&marker[..digits]);
            }
        }
    }
    numbers
}

/// Refuses a syntax tree deeper than `max_depth`, without recursion.
fn check_depth<'a>(root: &'a AstNode<'a>, max_depth: usize) -> Result<(), SchemaViolation> {
    let mut depth = 0usize;
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(_) => {
                depth += 1;
                // The document node itself is depth 1 here.
                if depth > max_depth {
                    return Err(SchemaViolation::TooDeep);
                }
            }
            NodeEdge::End(_) => depth -= 1,
        }
    }
    Ok(())
}

/// The input being imported, by line, for the source of Preserved Blocks.
struct Source<'s> {
    lines: Vec<&'s str>,
    recognise: Recognise,
}

impl<'s> Source<'s> {
    fn new(markdown: &'s str, recognise: Recognise) -> Self {
        Self {
            lines: source_lines(markdown),
            recognise,
        }
    }
}

/// Lines as comrak counts them: `\n`, `\r\n` and `\r` each end one.
fn source_lines(markdown: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = markdown;
    while let Some(end) = rest.find(['\n', '\r']) {
        lines.push(&rest[..end]);
        let ending = if rest[end..].starts_with("\r\n") {
            2
        } else {
            1
        };
        rest = &rest[end + ending..];
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines
}

fn blocks_of<'a>(node: &'a AstNode<'a>, source: &Source) -> Vec<Block> {
    node.children()
        .flat_map(|child| block_of(child, source))
        .collect()
}

fn block_of<'a>(node: &'a AstNode<'a>, source: &Source) -> Vec<Block> {
    let value = node.data.borrow().value.clone();
    vec![match value {
        NodeValue::Paragraph => Block::Paragraph(inlines_of(node)),
        NodeValue::Heading(heading) => Block::Heading {
            level: heading.level,
            content: inlines_of(node)
                .into_iter()
                .filter(|inline| !matches!(inline, Inline::Break))
                .collect(),
        },
        NodeValue::BlockQuote => Block::Blockquote(non_empty(blocks_of(node, source))),
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
                .map(|item| list_item(blocks_of(item, source)))
                .collect();
            match list.list_type {
                ListType::Bullet => Block::Bullet { items },
                ListType::Ordered => Block::Ordered {
                    order: list.start as u32,
                    items,
                },
            }
        }
        NodeValue::HtmlBlock(html) if html.literal.trim() == LIST_SEPARATOR => return Vec::new(),
        NodeValue::HtmlBlock(html) => {
            let kept = (source.recognise != Recognise::Never)
                .then(|| source_of(node, source))
                .flatten()
                .filter(|text| source.accepts(PreservedKind::Html, text));
            match kept {
                Some(text) => Block::Preserved {
                    kind: PreservedKind::Html,
                    source: text,
                },
                // Kept as literal text so nothing an author wrote is lost.
                None => literal_paragraph(&html.literal),
            }
        }
        NodeValue::Table(_) => {
            let kept =
                source_of(node, source).filter(|text| source.accepts(PreservedKind::Table, text));
            let lines = kept.clone().unwrap_or_else(|| loose_lines(node, source));
            let mut blocks = match kept {
                Some(text) => vec![Block::Preserved {
                    kind: PreservedKind::Table,
                    source: text,
                }],
                // What the same lines are with no table extension.
                None => import(&lines, Recognise::Never, MAX_IMPORT_DEPTH, false)
                    .unwrap_or_else(|_| vec![literal_paragraph(&lines)]),
            };
            blocks.extend(links_paragraph(node, &lines, source));
            return blocks;
        }
        NodeValue::FrontMatter(literal) => Block::Preserved {
            kind: PreservedKind::FrontMatter,
            source: front_matter_source(&literal),
        },
        _ => return Vec::new(),
    }]
}

impl Source<'_> {
    fn accepts(&self, kind: PreservedKind, text: &str) -> bool {
        self.recognise == Recognise::Unchecked || is_sound(kind, text)
    }
}

/// The paragraph that holds what a table's links and images resolved through a reference
/// definition. A definition is consumed and the table keeps its lines, so nothing else would
/// hold the target. What the table's lines give alone, with no definition, needs no paragraph.
///
/// Without a definition the export never resolves anything here, so the paragraph is added once.
/// A revision that brings the definition back after it finds the paragraph already there.
fn links_paragraph<'a>(node: &'a AstNode<'a>, lines: &str, source: &Source) -> Option<Block> {
    if source.recognise != Recognise::Checked {
        return None;
    }
    let targets = link_targets(node);
    if targets.is_empty() {
        return None;
    }
    let arena = Arena::new();
    let alone = parse_document(&arena, lines, &import_options(false));
    let mut own: HashMap<Vec<Inline>, usize> = HashMap::new();
    // Too deep to walk means nothing is known about the lines alone, and every target is kept.
    if check_depth(alone, MAX_IMPORT_DEPTH).is_ok() {
        for target in link_targets(alone) {
            *own.entry(target).or_default() += 1;
        }
    }
    let mut content = Vec::new();
    for target in targets {
        match own.get_mut(&target) {
            Some(count) if *count > 0 => *count -= 1,
            _ => {
                if !content.is_empty() {
                    content.push(Inline::Text(" ".to_owned(), Marks::default()));
                }
                content.extend(target);
            }
        }
    }
    if content.is_empty() {
        return None;
    }
    let links = Block::Paragraph(normalize_inlines(content));
    let already_there = node.next_sibling().is_some_and(|next| {
        matches!(next.data.borrow().value, NodeValue::Paragraph)
            && canonicalize(&[Block::Paragraph(inlines_of(next))])
                == canonicalize(std::slice::from_ref(&links))
    });
    (!already_there).then_some(links)
}

/// Each Link and Image under `node` as inline content. A link with no text has its URL as text,
/// so that it still shows.
fn link_targets<'a>(node: &'a AstNode<'a>) -> Vec<Vec<Inline>> {
    let mut targets = Vec::new();
    for target in node.descendants() {
        let link = match &target.data.borrow().value {
            NodeValue::Link(link) => Some(link_mark(link)),
            NodeValue::Image(_) => None,
            _ => continue,
        };
        let mut content = Vec::new();
        collect_inline(target, &Marks::default(), &mut content);
        if let (true, Some(link)) = (content.is_empty(), link) {
            let text = link.href.clone();
            let marks = Marks {
                link: Some(link),
                ..Marks::default()
            };
            content.push(Inline::Text(text, marks));
        }
        targets.push(normalize_inlines(content));
    }
    targets
}

/// The source of a front matter node: comrak's literal runs through the blank line after it.
fn front_matter_source(literal: &str) -> String {
    source_lines(literal.trim_end()).join("\n")
}

/// The exact source lines of a block node with every container prefix removed, or `None` when
/// a prefix is not one this can remove exactly (a tab across a column boundary).
fn source_of<'a>(node: &'a AstNode<'a>, source: &Source) -> Option<String> {
    let position = node.data.borrow().sourcepos;
    let first = position.start.line.checked_sub(1)?;
    let lines = source.lines.get(first..position.end.line)?;
    let (opening, rest) = lines.split_first()?;
    let mut containers: Vec<&AstNode> = node.ancestors().skip(1).collect();
    containers.reverse();
    let mut out = vec![opening.get(position.start.column.checked_sub(1)?..)?];
    for line in rest {
        out.push(strip_containers(line, &containers)?);
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    Some(out.join("\n"))
}

/// Removes the container prefix from one continuation line, column by column.
fn strip_containers<'l>(line: &'l str, containers: &[&AstNode]) -> Option<&'l str> {
    let mut rest = line;
    for container in containers {
        match &container.data.borrow().value {
            NodeValue::BlockQuote => {
                let indent = rest.len() - rest.trim_start_matches(' ').len();
                let after = rest[indent..].strip_prefix('>').filter(|_| indent <= 3)?;
                rest = after.strip_prefix(' ').unwrap_or(after);
            }
            NodeValue::Item(item) => {
                let width = item.marker_offset + item.padding;
                let indent = rest.len() - rest.trim_start_matches(' ').len();
                if indent >= width {
                    rest = &rest[width..];
                } else if rest.trim().is_empty() {
                    rest = "";
                } else {
                    return None;
                }
            }
            _ => {}
        }
    }
    Some(rest)
}

/// A node's lines with their edge whitespace gone, for when they stay literal text.
fn loose_lines<'a>(node: &'a AstNode<'a>, source: &Source) -> String {
    let position = node.data.borrow().sourcepos;
    let first = position.start.line.saturating_sub(1);
    let mut containers: Vec<&AstNode> = node.ancestors().skip(1).collect();
    containers.reverse();
    let mut out = String::new();
    for (index, line) in source
        .lines
        .iter()
        .enumerate()
        .take(position.end.line)
        .skip(first)
    {
        let text = if index == first {
            line.get(position.start.column.saturating_sub(1)..)
                .unwrap_or(line)
        } else {
            strip_containers(line, &containers).unwrap_or(line)
        };
        out.push_str(text.trim());
        out.push('\n');
    }
    out
}

/// The literal-text form of what cannot be a Preserved Block: one paragraph, its line breaks
/// soft as in any paragraph. Nothing an author wrote is lost.
fn literal_paragraph(text: &str) -> Block {
    let lines: Vec<&str> = source_lines(text)
        .into_iter()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let text = lines.join(" ");
    if text.is_empty() {
        return Block::Paragraph(Vec::new());
    }
    Block::Paragraph(vec![Inline::Text(text, Marks::default())])
}

/// Whether `source` can be stored as a Preserved Block of `kind`: it re-imports as exactly that
/// block wherever the block can be moved, and whatever follows it stays out of it.
fn is_sound(kind: PreservedKind, source: &str) -> bool {
    if source.contains('\r') || source.trim().is_empty() {
        return false;
    }
    let block = Block::Preserved {
        kind,
        source: source.to_owned(),
    };
    let text = |words: &str| Block::Paragraph(vec![Inline::Text(words.into(), Marks::default())]);
    let after = text("after");
    let bullet = |item: Vec<Block>| Block::Bullet {
        items: vec![item, vec![after.clone()]],
    };
    let contexts = match kind {
        PreservedKind::FrontMatter => {
            let lines: Vec<&str> = source.lines().collect();
            // A blank line inside either delimiter is how horizontal rules are written.
            if lines.get(1).is_none_or(|line| line.trim().is_empty())
                || lines
                    .get(lines.len().wrapping_sub(2))
                    .is_none_or(|line| line.trim().is_empty())
            {
                return false;
            }
            vec![vec![block, after.clone()]]
        }
        PreservedKind::Table | PreservedKind::Html => vec![
            vec![
                Block::Bullet {
                    items: vec![vec![text("before")]],
                },
                block.clone(),
                after.clone(),
            ],
            vec![Block::Blockquote(vec![block.clone(), after.clone()])],
            vec![bullet(vec![Block::Paragraph(Vec::new()), block.clone()])],
            vec![bullet(vec![text("before"), block, after.clone()])],
        ],
    };
    contexts.iter().all(|blocks| {
        import(
            &serialize_raw(blocks),
            Recognise::Unchecked,
            MAX_IMPORT_DEPTH,
            false,
        )
        .as_ref()
            == Ok(blocks)
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
        collect_inline(child, marks, out);
    }
}

fn collect_inline<'a>(node: &'a AstNode<'a>, marks: &Marks, out: &mut Vec<Inline>) {
    let value = node.data.borrow().value.clone();
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
            node,
            &Marks {
                em: true,
                ..marks.clone()
            },
            out,
        ),
        NodeValue::Strong => collect_inlines(
            node,
            &Marks {
                strong: true,
                ..marks.clone()
            },
            out,
        ),
        NodeValue::Strikethrough => collect_inlines(
            node,
            &Marks {
                strike: true,
                ..marks.clone()
            },
            out,
        ),
        NodeValue::Link(link) => collect_inlines(
            node,
            &Marks {
                link: Some(link_mark(&link)),
                ..marks.clone()
            },
            out,
        ),
        // Not in the baseline: kept as literal text.
        NodeValue::HtmlInline(html) => out.push(Inline::Text(html, marks.clone())),
        NodeValue::Image(image) => {
            let mut alt = Vec::new();
            collect_inlines(node, &Marks::default(), &mut alt);
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
        _ => collect_inlines(node, marks, out),
    }
}

fn link_mark(link: &NodeLink) -> Link {
    Link {
        href: link.url.clone(),
        title: (!link.title.is_empty()).then(|| link.title.clone()),
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
        // Raw text in a paragraph: comrak adds the container prefixes and the block spacing as
        // for a paragraph, and never escapes it.
        Block::Preserved { source, .. } => {
            let paragraph = node(arena, NodeValue::Paragraph);
            paragraph.append(node(arena, NodeValue::Raw(source.clone())));
            paragraph
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
                } else if reads_as_delimiter_row(content, index) {
                    parent.append(node(arena, NodeValue::Raw("\\".to_owned())));
                    node(arena, NodeValue::Text(Cow::Owned(text.clone())))
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

/// Whether the text at `index` is a whole line after a hard break that would make the line
/// above it a table header. Comrak already escapes a line that starts with `-`.
fn reads_as_delimiter_row(content: &[Inline], index: usize) -> bool {
    let Some(Inline::Text(text, marks)) = content.get(index) else {
        return false;
    };
    index > 0
        && content[index - 1] == Inline::Break
        && *marks == Marks::default()
        && matches!(content.get(index + 1), None | Some(Inline::Break))
        && text.starts_with(['|', ':'])
        && text.contains('-')
        && text
            .chars()
            .all(|c| matches!(c, '|' | ':' | '-' | ' ' | '\t'))
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
        // A concurrent edit can leave a source that no longer reads as what it was.
        Block::Preserved { kind, source } if !is_sound(*kind, source) => {
            canonical_block(&literal_paragraph(source))
        }
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
        [Block::Preserved { .. }, _] => false,
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
    let Ok(parsed) = reimport(&serialize_raw(std::slice::from_ref(&block))) else {
        return false;
    };
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
