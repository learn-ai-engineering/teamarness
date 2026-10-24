//! The shared body schema: the block model, and the reader and writer for the Loro tree that
//! `loro-prosemirror` 0.4.4 stores for it.
//!
//! The tree is a root map `"doc"`; every node is a map `{nodeName, attributes: map, children:
//! list}`, and consecutive text is one marked `LoroText` inside `children`. Text is read from
//! deltas, because the binding's `toJSON()` drops marks.

use std::collections::HashMap;
use std::fmt;

use loro::{
    Container, ExpandType, LoroDoc, LoroError, LoroList, LoroMap, LoroText, LoroValue, StyleConfig,
    StyleConfigMap, TextDelta, ValueOrContainer,
};

pub(super) const ROOT: &str = "doc";
pub(super) const NODE_NAME: &str = "nodeName";
pub(super) const ATTRIBUTES: &str = "attributes";
pub(super) const CHILDREN: &str = "children";

/// The deepest node nesting a body may hold; the root's children are at depth 1.
///
/// The reader, the writer and merge all refuse anything deeper, so no recursive walk over a
/// body can exhaust the stack.
pub const MAX_NESTING: usize = 64;
/// The largest ordered-list start CommonMark can write (nine digits).
pub const MAX_ORDER: u32 = 999_999_999;

/// One block of the body.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// `level` is 1 to 6.
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Blockquote(Vec<Block>),
    Rule,
    /// `params` is the info string; the text carries no marks.
    Code {
        params: String,
        text: String,
    },
    /// Tight or loose spacing is chosen when writing Markdown, never stored.
    Bullet {
        items: Vec<Vec<Block>>,
    },
    Ordered {
        order: u32,
        items: Vec<Vec<Block>>,
    },
}

/// Inline content of a paragraph or heading.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Inline {
    Text(String, Marks),
    /// A hard break; headings never hold one.
    Break,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Marks {
    pub strong: bool,
    pub em: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Link {
    pub href: String,
    pub title: Option<String>,
}

impl Block {
    pub(super) fn node_name(&self) -> &'static str {
        match self {
            Block::Paragraph(_) => "paragraph",
            Block::Heading { .. } => "heading",
            Block::Blockquote(_) => "blockquote",
            Block::Rule => "horizontal_rule",
            Block::Code { .. } => "code_block",
            Block::Bullet { .. } => "bullet_list",
            Block::Ordered { .. } => "ordered_list",
        }
    }
}

/// Why a tree, or a block model, is not a body under the shared schema.
///
/// A violation means some peer wrote a shape the binding never writes. Shapes that ordinary
/// concurrent edits can produce (an empty list, a list item that lost its leading paragraph,
/// an empty body) are not violations; the reader accepts them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaViolation {
    /// The root map has no `children` list.
    MissingBody,
    /// The root map's `nodeName` is not `doc`.
    InvalidRoot,
    /// A node map has no string `nodeName`.
    UnnamedNode,
    /// A node name the schema does not define.
    UnknownNode(String),
    /// A known node, a text run or a plain value where its parent's content does not allow it.
    MisplacedContent { parent: &'static str },
    /// A node attribute of the wrong type or out of range.
    InvalidAttribute {
        node: &'static str,
        attribute: &'static str,
    },
    /// An `attributes` or `children` entry that is not the container the schema expects.
    InvalidContainer { node: &'static str },
    /// A text mark the schema does not define, a malformed link, or any mark in a code block.
    InvalidMark(String),
    /// Nodes nested deeper than [`MAX_NESTING`].
    TooDeep,
}

impl fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBody => write!(f, "the document has no body tree"),
            Self::InvalidRoot => write!(f, "the body root is not a doc node"),
            Self::UnnamedNode => write!(f, "a node has no name"),
            Self::UnknownNode(name) => write!(f, "unknown node {name:?}"),
            Self::MisplacedContent { parent } => write!(f, "content not allowed in {parent}"),
            Self::InvalidAttribute { node, attribute } => {
                write!(f, "invalid {attribute} attribute on {node}")
            }
            Self::InvalidContainer { node } => write!(f, "malformed {node} node"),
            Self::InvalidMark(key) => write!(f, "invalid mark {key:?}"),
            Self::TooDeep => write!(f, "nodes nested deeper than {MAX_NESTING}"),
        }
    }
}

impl std::error::Error for SchemaViolation {}

/// Sets the mark expand configuration the binding uses: `getLoroTextStyle(schema)` gives
/// `after` to every inclusive mark and `none` to link. The configuration is per document and
/// never saved, so every document that writes marks must set it.
pub(super) fn configure(doc: &LoroDoc) {
    let mut styles = StyleConfigMap::new();
    for key in ["em", "strong", "strikethrough", "code"] {
        styles.insert(
            key.into(),
            StyleConfig {
                expand: ExpandType::After,
            },
        );
    }
    styles.insert(
        "link".into(),
        StyleConfig {
            expand: ExpandType::None,
        },
    );
    doc.config_text_style(styles);
}

// ---------------------------------------------------------------- model checks

/// Nesting depth of a block in tree nodes: a list counts its list item as a level too.
pub(super) fn block_depth(block: &Block) -> usize {
    match block {
        Block::Blockquote(children) => 1 + blocks_depth(children),
        Block::Bullet { items } | Block::Ordered { items, .. } => {
            2 + items
                .iter()
                .map(|item| blocks_depth(item))
                .max()
                .unwrap_or(0)
        }
        _ => 1,
    }
}

pub(super) fn blocks_depth(blocks: &[Block]) -> usize {
    blocks.iter().map(block_depth).max().unwrap_or(0)
}

/// Checks a block model against the schema before it is written.
pub(super) fn check_blocks(blocks: &[Block]) -> Result<(), SchemaViolation> {
    if blocks_depth(blocks) > MAX_NESTING {
        return Err(SchemaViolation::TooDeep);
    }
    check_attributes(blocks)
}

fn check_attributes(blocks: &[Block]) -> Result<(), SchemaViolation> {
    for block in blocks {
        match block {
            Block::Heading { level, content } => {
                if !(1..=6).contains(level) {
                    return Err(SchemaViolation::InvalidAttribute {
                        node: "heading",
                        attribute: "level",
                    });
                }
                if content.contains(&Inline::Break) {
                    return Err(SchemaViolation::MisplacedContent { parent: "heading" });
                }
            }
            Block::Ordered { order, items } => {
                if *order > MAX_ORDER {
                    return Err(SchemaViolation::InvalidAttribute {
                        node: "ordered_list",
                        attribute: "order",
                    });
                }
                for item in items {
                    check_attributes(item)?;
                }
            }
            Block::Bullet { items } => {
                for item in items {
                    check_attributes(item)?;
                }
            }
            Block::Blockquote(children) => check_attributes(children)?,
            Block::Paragraph(_) | Block::Rule | Block::Code { .. } => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- write

/// Writes a body tree holding `blocks` (one empty paragraph when `blocks` is empty) into a
/// document that has none yet, and commits it.
///
/// Errors are a block model the schema does not allow, or a Loro failure (for example, a
/// detached document).
pub fn write_tree(doc: &LoroDoc, blocks: &[Block]) -> Result<(), WriteError> {
    check_blocks(blocks).map_err(WriteError::Schema)?;
    configure(doc);
    let root = doc.get_map(ROOT);
    root.insert(NODE_NAME, "doc")?;
    root.insert_container(ATTRIBUTES, LoroMap::new())?;
    let children = root.insert_container(CHILDREN, LoroList::new())?;
    if blocks.is_empty() {
        insert_block(&children, 0, &Block::Paragraph(Vec::new()))?;
    }
    for (index, block) in blocks.iter().enumerate() {
        insert_block(&children, index, block)?;
    }
    doc.commit();
    Ok(())
}

/// Why [`write_tree`] wrote nothing usable.
#[derive(Debug)]
pub enum WriteError {
    Schema(SchemaViolation),
    Loro(LoroError),
}

impl From<LoroError> for WriteError {
    fn from(error: LoroError) -> Self {
        Self::Loro(error)
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(violation) => violation.fmt(f),
            Self::Loro(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for WriteError {}

pub(super) fn insert_block(
    list: &LoroList,
    pos: usize,
    block: &Block,
) -> Result<LoroMap, LoroError> {
    let map = list.insert_container(pos, LoroMap::new())?;
    map.insert(NODE_NAME, block.node_name())?;
    let attributes = map.insert_container(ATTRIBUTES, LoroMap::new())?;
    for (key, value) in block_attributes(block) {
        attributes.insert(key, value)?;
    }
    let children = map.insert_container(CHILDREN, LoroList::new())?;
    match block {
        Block::Paragraph(content) | Block::Heading { content, .. } => {
            insert_inlines(&children, 0, content)?;
        }
        Block::Blockquote(blocks) => {
            for (index, child) in blocks.iter().enumerate() {
                insert_block(&children, index, child)?;
            }
        }
        Block::Rule => {}
        Block::Code { text, .. } => {
            if !text.is_empty() {
                let run = children.insert_container(0, LoroText::new())?;
                run.insert(0, text)?;
            }
        }
        Block::Bullet { items, .. } | Block::Ordered { items, .. } => {
            for (index, item) in items.iter().enumerate() {
                insert_item(&children, index, item)?;
            }
        }
    }
    Ok(map)
}

pub(super) fn insert_item(
    list: &LoroList,
    pos: usize,
    item: &[Block],
) -> Result<LoroMap, LoroError> {
    let map = list.insert_container(pos, LoroMap::new())?;
    map.insert(NODE_NAME, "list_item")?;
    map.insert_container(ATTRIBUTES, LoroMap::new())?;
    let children = map.insert_container(CHILDREN, LoroList::new())?;
    for (index, child) in item.iter().enumerate() {
        insert_block(&children, index, child)?;
    }
    Ok(map)
}

/// Node attributes as the binding stores them: JavaScript numbers are doubles.
pub(super) fn block_attributes(block: &Block) -> Vec<(&'static str, LoroValue)> {
    match block {
        Block::Heading { level, .. } => vec![("level", LoroValue::Double(f64::from(*level)))],
        Block::Code { params, .. } => vec![("params", LoroValue::from(params.as_str()))],
        Block::Ordered { order, .. } => vec![("order", LoroValue::Double(f64::from(*order)))],
        _ => Vec::new(),
    }
}

/// Inserts inline content at `pos` of a `children` list; returns how many entries it added.
pub(super) fn insert_inlines(
    list: &LoroList,
    mut pos: usize,
    content: &[Inline],
) -> Result<usize, LoroError> {
    let start = pos;
    for segment in segments(content) {
        match segment {
            Segment::Text(runs) => {
                let text = list.insert_container(pos, LoroText::new())?;
                write_runs(&text, &runs)?;
            }
            Segment::Break => {
                let map = list.insert_container(pos, LoroMap::new())?;
                map.insert(NODE_NAME, "hard_break")?;
                map.insert_container(ATTRIBUTES, LoroMap::new())?;
                map.insert_container(CHILDREN, LoroList::new())?;
            }
        }
        pos += 1;
    }
    Ok(pos - start)
}

pub(super) enum Segment {
    Text(Vec<(String, Marks)>),
    Break,
}

/// Groups inline content the way the binding's `normalizeNodeContent` does: consecutive text
/// is one entry.
pub(super) fn segments(content: &[Inline]) -> Vec<Segment> {
    let mut out = Vec::new();
    for inline in content {
        match inline {
            Inline::Text(text, marks) => match out.last_mut() {
                Some(Segment::Text(runs)) => runs.push((text.clone(), marks.clone())),
                _ => out.push(Segment::Text(vec![(text.clone(), marks.clone())])),
            },
            Inline::Break => out.push(Segment::Break),
        }
    }
    out
}

fn write_runs(text: &LoroText, runs: &[(String, Marks)]) -> Result<(), LoroError> {
    let joined: String = runs.iter().map(|(run, _)| run.as_str()).collect();
    text.insert(0, &joined)?;
    let mut offset = 0;
    for (run, marks) in runs {
        let len = run.chars().count();
        for (key, value) in mark_values(marks) {
            text.mark(offset..offset + len, key, value)?;
        }
        offset += len;
    }
    Ok(())
}

/// Mark values as the binding stores them: `mark.attrs` objects.
pub(super) fn mark_values(marks: &Marks) -> Vec<(&'static str, LoroValue)> {
    let empty = || LoroValue::Map(HashMap::<String, LoroValue>::new().into());
    let mut out = Vec::new();
    if marks.strong {
        out.push(("strong", empty()));
    }
    if marks.em {
        out.push(("em", empty()));
    }
    if marks.strike {
        out.push(("strikethrough", empty()));
    }
    if marks.code {
        out.push(("code", empty()));
    }
    if let Some(link) = &marks.link {
        out.push(("link", link_value(link)));
    }
    out
}

fn link_value(link: &Link) -> LoroValue {
    let mut map = HashMap::new();
    map.insert("href".to_owned(), LoroValue::from(link.href.as_str()));
    map.insert(
        "title".to_owned(),
        link.title
            .as_deref()
            .map_or(LoroValue::Null, LoroValue::from),
    );
    LoroValue::Map(map.into())
}

/// Merges adjacent equal-mark text and drops empty text, as ProseMirror does.
pub(super) fn normalize_inlines(inlines: Vec<Inline>) -> Vec<Inline> {
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

// ---------------------------------------------------------------- read

/// Reads the body tree as blocks, or reports the first schema violation.
///
/// The reader is total: it never panics, and its recursion is bounded by [`MAX_NESTING`].
pub fn read_tree(doc: &LoroDoc) -> Result<Vec<Block>, SchemaViolation> {
    let root = doc.get_map(ROOT);
    match root.get(NODE_NAME) {
        None => {}
        Some(ValueOrContainer::Value(LoroValue::String(name))) if name.as_str() == "doc" => {}
        Some(_) => return Err(SchemaViolation::InvalidRoot),
    }
    let children = match root.get(CHILDREN) {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => return Err(SchemaViolation::MissingBody),
    };
    read_blocks(&children, "doc", 1)
}

/// The body's top-level `children` list.
pub(super) fn body_list(doc: &LoroDoc) -> Result<LoroList, SchemaViolation> {
    match doc.get_map(ROOT).get(CHILDREN) {
        Some(ValueOrContainer::Container(Container::List(list))) => Ok(list),
        _ => Err(SchemaViolation::MissingBody),
    }
}

pub(super) fn list_entries(list: &LoroList) -> Vec<ValueOrContainer> {
    (0..list.len())
        .filter_map(|index| list.get(index))
        .collect()
}

fn node_name(map: &LoroMap) -> Result<String, SchemaViolation> {
    match map.get(NODE_NAME) {
        Some(ValueOrContainer::Value(LoroValue::String(name))) => Ok(name.to_string()),
        _ => Err(SchemaViolation::UnnamedNode),
    }
}

fn is_named(map: &LoroMap, name: &str) -> bool {
    matches!(map.get(NODE_NAME), Some(ValueOrContainer::Value(LoroValue::String(n))) if n.as_str() == name)
}

/// A node's `children` list; a node without one has no children.
pub(super) fn child_list(
    map: &LoroMap,
    node: &'static str,
) -> Result<Option<LoroList>, SchemaViolation> {
    match map.get(CHILDREN) {
        None => Ok(None),
        Some(ValueOrContainer::Container(Container::List(list))) => Ok(Some(list)),
        Some(_) => Err(SchemaViolation::InvalidContainer { node }),
    }
}

fn attribute(
    map: &LoroMap,
    node: &'static str,
    key: &str,
) -> Result<Option<LoroValue>, SchemaViolation> {
    match map.get(ATTRIBUTES) {
        None => Ok(None),
        Some(ValueOrContainer::Container(Container::Map(attributes))) => {
            match attributes.get(key) {
                None => Ok(None),
                Some(ValueOrContainer::Value(value)) => Ok(Some(value)),
                Some(ValueOrContainer::Container(_)) => {
                    Err(SchemaViolation::InvalidContainer { node })
                }
            }
        }
        Some(_) => Err(SchemaViolation::InvalidContainer { node }),
    }
}

/// An integral number attribute in `range`, or `default` when absent.
fn integer(
    value: Option<LoroValue>,
    default: u32,
    range: std::ops::RangeInclusive<u32>,
    node: &'static str,
    attribute: &'static str,
) -> Result<u32, SchemaViolation> {
    let invalid = SchemaViolation::InvalidAttribute { node, attribute };
    let number = match value {
        None => return Ok(default),
        Some(LoroValue::Double(n)) => n,
        Some(LoroValue::I64(n)) => n as f64,
        Some(_) => return Err(invalid),
    };
    if number.fract() != 0.0
        || number < f64::from(*range.start())
        || number > f64::from(*range.end())
    {
        return Err(invalid);
    }
    Ok(number as u32)
}

fn read_blocks(
    list: &LoroList,
    parent: &'static str,
    depth: usize,
) -> Result<Vec<Block>, SchemaViolation> {
    if depth > MAX_NESTING {
        return Err(SchemaViolation::TooDeep);
    }
    list_entries(list)
        .into_iter()
        .map(|entry| match entry {
            ValueOrContainer::Container(Container::Map(map)) => read_block(&map, parent, depth),
            _ => Err(SchemaViolation::MisplacedContent { parent }),
        })
        .collect()
}

/// Reads one block node at `depth` under `parent`.
pub(super) fn read_block(
    map: &LoroMap,
    parent: &'static str,
    depth: usize,
) -> Result<Block, SchemaViolation> {
    let name = node_name(map)?;
    Ok(match name.as_str() {
        "paragraph" => Block::Paragraph(read_inlines(map, "paragraph", true)?),
        "heading" => Block::Heading {
            level: integer(
                attribute(map, "heading", "level")?,
                1,
                1..=6,
                "heading",
                "level",
            )? as u8,
            content: read_inlines(map, "heading", false)?,
        },
        "blockquote" => Block::Blockquote(match child_list(map, "blockquote")? {
            Some(list) => read_blocks(&list, "blockquote", depth + 1)?,
            None => Vec::new(),
        }),
        "horizontal_rule" => {
            if child_list(map, "horizontal_rule")?.is_some_and(|list| !list.is_empty()) {
                return Err(SchemaViolation::MisplacedContent {
                    parent: "horizontal_rule",
                });
            }
            Block::Rule
        }
        "code_block" => Block::Code {
            params: match attribute(map, "code_block", "params")? {
                None | Some(LoroValue::Null) => String::new(),
                Some(LoroValue::String(params)) => params.to_string(),
                Some(_) => {
                    return Err(SchemaViolation::InvalidAttribute {
                        node: "code_block",
                        attribute: "params",
                    });
                }
            },
            text: read_code(map)?,
        },
        "bullet_list" => Block::Bullet {
            items: read_items(map, "bullet_list", depth)?,
        },
        "ordered_list" => Block::Ordered {
            order: integer(
                attribute(map, "ordered_list", "order")?,
                1,
                0..=MAX_ORDER,
                "ordered_list",
                "order",
            )?,
            items: read_items(map, "ordered_list", depth)?,
        },
        "list_item" | "hard_break" | "doc" => {
            return Err(SchemaViolation::MisplacedContent { parent });
        }
        other => return Err(SchemaViolation::UnknownNode(other.to_owned())),
    })
}

/// Reads the items of a list node at `depth`; its items are one level deeper.
pub(super) fn read_items(
    map: &LoroMap,
    node: &'static str,
    depth: usize,
) -> Result<Vec<Vec<Block>>, SchemaViolation> {
    let Some(list) = child_list(map, node)? else {
        return Ok(Vec::new());
    };
    if depth + 1 > MAX_NESTING {
        return Err(SchemaViolation::TooDeep);
    }
    list_entries(&list)
        .into_iter()
        .map(|entry| match entry {
            ValueOrContainer::Container(Container::Map(item)) if is_named(&item, "list_item") => {
                read_item(&item, depth + 1)
            }
            ValueOrContainer::Container(Container::Map(other)) => match node_name(&other)?.as_str()
            {
                "paragraph" | "heading" | "blockquote" | "horizontal_rule" | "code_block"
                | "bullet_list" | "ordered_list" | "hard_break" | "doc" => {
                    Err(SchemaViolation::MisplacedContent { parent: node })
                }
                unknown => Err(SchemaViolation::UnknownNode(unknown.to_owned())),
            },
            _ => Err(SchemaViolation::MisplacedContent { parent: node }),
        })
        .collect()
}

/// Reads one `list_item` node at `depth`.
pub(super) fn read_item(map: &LoroMap, depth: usize) -> Result<Vec<Block>, SchemaViolation> {
    match child_list(map, "list_item")? {
        Some(list) => read_blocks(&list, "list_item", depth + 1),
        None => Ok(Vec::new()),
    }
}

fn read_code(map: &LoroMap) -> Result<String, SchemaViolation> {
    let mut text = String::new();
    let Some(list) = child_list(map, "code_block")? else {
        return Ok(text);
    };
    for entry in list_entries(&list) {
        let ValueOrContainer::Container(Container::Text(run)) = entry else {
            return Err(SchemaViolation::MisplacedContent {
                parent: "code_block",
            });
        };
        for inline in read_runs(&run)? {
            match inline {
                Inline::Text(chunk, marks) if marks == Marks::default() => text.push_str(&chunk),
                _ => return Err(SchemaViolation::InvalidMark("code_block".to_owned())),
            }
        }
    }
    Ok(text)
}

fn read_inlines(
    map: &LoroMap,
    node: &'static str,
    breaks: bool,
) -> Result<Vec<Inline>, SchemaViolation> {
    let mut out = Vec::new();
    let Some(list) = child_list(map, node)? else {
        return Ok(out);
    };
    for entry in list_entries(&list) {
        match entry {
            ValueOrContainer::Container(Container::Text(text)) => out.extend(read_runs(&text)?),
            ValueOrContainer::Container(Container::Map(map)) => match node_name(&map)?.as_str() {
                "hard_break" if breaks => out.push(Inline::Break),
                "paragraph" | "heading" | "blockquote" | "horizontal_rule" | "code_block"
                | "bullet_list" | "ordered_list" | "list_item" | "hard_break" | "doc" => {
                    return Err(SchemaViolation::MisplacedContent { parent: node });
                }
                unknown => return Err(SchemaViolation::UnknownNode(unknown.to_owned())),
            },
            _ => return Err(SchemaViolation::MisplacedContent { parent: node }),
        }
    }
    Ok(normalize_inlines(out))
}

/// The marked runs of one text container, read from its delta.
pub(super) fn read_runs(text: &LoroText) -> Result<Vec<Inline>, SchemaViolation> {
    text.to_delta()
        .into_iter()
        .filter_map(|delta| match delta {
            TextDelta::Insert { insert, attributes } => {
                Some(marks_of(attributes.as_ref()).map(|marks| Inline::Text(insert, marks)))
            }
            _ => None,
        })
        .collect()
}

fn marks_of<'a, I>(attributes: Option<&'a I>) -> Result<Marks, SchemaViolation>
where
    &'a I: IntoIterator<Item = (&'a String, &'a LoroValue)>,
{
    let mut marks = Marks::default();
    for (key, value) in attributes.into_iter().flatten() {
        // An unmarked range can surface as a null value.
        if matches!(value, LoroValue::Null) {
            continue;
        }
        match key.as_str() {
            "strong" => marks.strong = true,
            "em" => marks.em = true,
            "strikethrough" => marks.strike = true,
            "code" => marks.code = true,
            "link" => marks.link = Some(link_of(value).ok_or_else(|| invalid_mark(key))?),
            _ => return Err(invalid_mark(key)),
        }
    }
    Ok(marks)
}

fn invalid_mark(key: &str) -> SchemaViolation {
    SchemaViolation::InvalidMark(key.chars().take(64).collect())
}

fn link_of(value: &LoroValue) -> Option<Link> {
    let LoroValue::Map(map) = value else {
        return None;
    };
    let href = match map.get("href") {
        Some(LoroValue::String(href)) => href.to_string(),
        _ => return None,
    };
    let title = match map.get("title") {
        None | Some(LoroValue::Null) => None,
        Some(LoroValue::String(title)) => Some(title.to_string()),
        Some(_) => return None,
    };
    Some(Link { href, title })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![Inline::Text(text.into(), Marks::default())])
    }

    fn body(blocks: &[Block]) -> LoroDoc {
        let doc = LoroDoc::new();
        write_tree(&doc, blocks).unwrap();
        doc
    }

    fn first_block(doc: &LoroDoc) -> LoroMap {
        match body_list(doc).unwrap().get(0) {
            Some(ValueOrContainer::Container(Container::Map(map))) => map,
            _ => panic!("no first block"),
        }
    }

    fn children_of(map: &LoroMap) -> LoroList {
        child_list(map, "test").unwrap().unwrap()
    }

    fn attributes_of(map: &LoroMap) -> LoroMap {
        match map.get(ATTRIBUTES) {
            Some(ValueOrContainer::Container(Container::Map(attributes))) => attributes,
            _ => panic!("no attributes"),
        }
    }

    #[test]
    fn genesis_is_one_empty_paragraph() {
        let doc = body(&[]);
        assert_eq!(read_tree(&doc).unwrap(), vec![Block::Paragraph(Vec::new())]);
    }

    #[test]
    fn a_document_without_a_body_tree_is_a_violation() {
        assert_eq!(
            read_tree(&LoroDoc::new()),
            Err(SchemaViolation::MissingBody)
        );
    }

    #[test]
    fn shapes_concurrent_edits_can_produce_are_read() {
        let doc = body(&[para("x")]);
        body_list(&doc).unwrap().delete(0, 1).unwrap();
        assert_eq!(read_tree(&doc).unwrap(), Vec::<Block>::new());

        let doc = body(&[Block::Bullet {
            items: vec![vec![para("a")]],
        }]);
        children_of(&first_block(&doc)).delete(0, 1).unwrap();
        assert_eq!(
            read_tree(&doc).unwrap(),
            vec![Block::Bullet { items: vec![] }]
        );
    }

    #[test]
    fn unknown_nodes_and_misplaced_content_are_violations() {
        let doc = body(&[para("x")]);
        first_block(&doc).insert(NODE_NAME, "table").unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::UnknownNode("table".into()))
        );

        let doc = body(&[para("x")]);
        body_list(&doc).unwrap().insert(0, "plain").unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::MisplacedContent { parent: "doc" })
        );

        let doc = body(&[para("x")]);
        body_list(&doc)
            .unwrap()
            .insert_container(0, LoroText::new())
            .unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::MisplacedContent { parent: "doc" })
        );

        let doc = body(&[para("x")]);
        first_block(&doc).insert(NODE_NAME, "list_item").unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::MisplacedContent { parent: "doc" })
        );

        let doc = body(&[Block::Bullet {
            items: vec![vec![para("a")]],
        }]);
        insert_block(&children_of(&first_block(&doc)), 0, &para("stray")).unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::MisplacedContent {
                parent: "bullet_list"
            })
        );

        let doc = body(&[Block::Heading {
            level: 2,
            content: vec![Inline::Text("h".into(), Marks::default())],
        }]);
        insert_inlines(&children_of(&first_block(&doc)), 1, &[Inline::Break]).unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::MisplacedContent { parent: "heading" })
        );

        let doc = body(&[para("x")]);
        first_block(&doc).delete(NODE_NAME).unwrap();
        assert_eq!(read_tree(&doc), Err(SchemaViolation::UnnamedNode));

        let doc = body(&[para("x")]);
        first_block(&doc).insert(CHILDREN, "not a list").unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::InvalidContainer { node: "paragraph" })
        );
    }

    #[test]
    fn attributes_are_checked() {
        for level in [0.0, 7.0, 1.5] {
            let doc = body(&[Block::Heading {
                level: 1,
                content: Vec::new(),
            }]);
            attributes_of(&first_block(&doc))
                .insert("level", LoroValue::Double(level))
                .unwrap();
            assert_eq!(
                read_tree(&doc),
                Err(SchemaViolation::InvalidAttribute {
                    node: "heading",
                    attribute: "level"
                })
            );
        }
        let doc = body(&[Block::Ordered {
            order: 3,
            items: vec![vec![para("a")]],
        }]);
        attributes_of(&first_block(&doc))
            .insert("order", "three")
            .unwrap();
        assert!(matches!(
            read_tree(&doc),
            Err(SchemaViolation::InvalidAttribute { .. })
        ));
        assert!(matches!(
            write_tree(
                &LoroDoc::new(),
                &[Block::Heading {
                    level: 9,
                    content: Vec::new()
                }]
            ),
            Err(WriteError::Schema(SchemaViolation::InvalidAttribute { .. }))
        ));
    }

    #[test]
    fn marks_are_checked() {
        let doc = body(&[para("text")]);
        let Some(ValueOrContainer::Container(Container::Text(run))) =
            children_of(&first_block(&doc)).get(0)
        else {
            panic!("no text");
        };
        // A hostile peer configures any style it likes before marking.
        let mut styles = StyleConfigMap::new();
        styles.insert(
            "underline".into(),
            StyleConfig {
                expand: ExpandType::After,
            },
        );
        doc.config_text_style(styles);
        run.mark(0..2, "underline", true).unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::InvalidMark("underline".into()))
        );

        let doc = body(&[para("text")]);
        let Some(ValueOrContainer::Container(Container::Text(run))) =
            children_of(&first_block(&doc)).get(0)
        else {
            panic!("no text");
        };
        configure(&doc);
        run.mark(0..2, "link", "https://bare.test").unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::InvalidMark("link".into()))
        );

        let doc = body(&[Block::Code {
            params: String::new(),
            text: "fn main()".into(),
        }]);
        let Some(ValueOrContainer::Container(Container::Text(run))) =
            children_of(&first_block(&doc)).get(0)
        else {
            panic!("no text");
        };
        configure(&doc);
        run.mark(0..2, "strong", LoroValue::Map(HashMap::new().into()))
            .unwrap();
        assert_eq!(
            read_tree(&doc),
            Err(SchemaViolation::InvalidMark("code_block".into()))
        );
    }

    #[test]
    fn nesting_is_bounded() {
        let nested = |depth: usize| {
            let mut block = para("deep");
            for _ in 1..depth {
                block = Block::Blockquote(vec![block]);
            }
            block
        };
        let doc = body(&[nested(MAX_NESTING)]);
        assert!(read_tree(&doc).is_ok());
        assert!(matches!(
            write_tree(&LoroDoc::new(), &[nested(MAX_NESTING + 1)]),
            Err(WriteError::Schema(SchemaViolation::TooDeep))
        ));

        // A hostile peer can still write one level more than the writer allows.
        let doc = body(&[nested(MAX_NESTING)]);
        let mut map = first_block(&doc);
        loop {
            match children_of(&map).get(0) {
                Some(ValueOrContainer::Container(Container::Map(child)))
                    if is_named(&child, "blockquote") =>
                {
                    map = child
                }
                _ => break,
            }
        }
        insert_block(
            &children_of(&map),
            0,
            &Block::Blockquote(vec![para("deeper")]),
        )
        .unwrap();
        assert_eq!(read_tree(&doc), Err(SchemaViolation::TooDeep));
    }
}
