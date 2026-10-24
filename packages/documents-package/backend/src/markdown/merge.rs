//! Merging a Markdown revision into the body as a minimal change.
//!
//! The revision is applied to a fork of the document at the version the Markdown was exported
//! from (the base), so concurrent edits made since then merge through Loro instead of being
//! overwritten. Blocks whose Markdown did not change are never touched, even when Markdown
//! cannot represent them exactly; changed blocks of the same kind are edited in place (text by
//! character diff, marks by range), so concurrent edits inside them survive.

use std::fmt;

use loro::{
    Container, ExportMode, Frontiers, LoroDoc, LoroError, LoroList, LoroMap, LoroText, LoroValue,
    PeerID, UpdateOptions, ValueOrContainer,
};

use super::export::{parse, parse_revision, serialize};
use super::schema::{
    ATTRIBUTES, Block, CHILDREN, Inline, Marks, SchemaViolation, Segment, block_attributes,
    body_list, child_list, configure, insert_block, insert_inlines, insert_item, list_entries,
    mark_values, read_block, read_item, read_runs, read_tree, segments,
};

/// The longest-common-subsequence table is bounded at this many cells (16 MiB of `u32`).
/// Beyond it, the blocks between the unchanged prefix and suffix are paired by position.
const LCS_CELLS: usize = 4 * 1024 * 1024;

/// A merged revision.
#[derive(Debug)]
pub struct Merged {
    /// One Loro update holding every op the revision produced. It is empty of ops, but still
    /// valid to import, when `ops` is 0.
    pub update: Vec<u8>,
    /// How many Loro operations the revision produced; 0 means the revision changed nothing.
    pub ops: usize,
}

/// Why a revision could not be merged. Nothing was changed.
#[derive(Debug)]
pub enum MergeError {
    /// The base frontiers are not in the document's history.
    BaseUnavailable,
    /// The peer already has operations in the document, so its op ids would collide.
    PeerInUse,
    /// The body at the base, or the revision, is not a body under the schema.
    Schema(SchemaViolation),
    Loro(LoroError),
}

impl From<SchemaViolation> for MergeError {
    fn from(violation: SchemaViolation) -> Self {
        Self::Schema(violation)
    }
}

impl From<LoroError> for MergeError {
    fn from(error: LoroError) -> Self {
        Self::Loro(error)
    }
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaseUnavailable => write!(f, "the base version is not in the document history"),
            Self::PeerInUse => write!(f, "the merge peer already has operations"),
            Self::Schema(violation) => violation.fmt(f),
            Self::Loro(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for MergeError {}

/// Merges `markdown`, a revision of the Markdown exported at `base`, as the fresh peer `peer`.
///
/// `doc` is not changed: the caller appends the returned update and then imports it. `peer`
/// must have no operations in `doc`; a fork at an older base cannot see a reused peer's later
/// operations and would mint colliding op ids.
pub fn merge(
    doc: &LoroDoc,
    base: &Frontiers,
    markdown: &str,
    peer: PeerID,
) -> Result<Merged, MergeError> {
    let mut target = parse_revision(markdown)?;
    // Schema `doc` is `block+`: an empty revision leaves one empty paragraph.
    if target.is_empty() {
        target.push(Block::Paragraph(Vec::new()));
    }
    let known = doc.oplog_vv();
    if base.iter().any(|id| !known.includes_id(id)) {
        return Err(MergeError::BaseUnavailable);
    }
    if known.get(&peer).is_some_and(|end| *end > 0) {
        return Err(MergeError::PeerInUse);
    }
    let work = doc.fork_at(base).map_err(|_| MergeError::BaseUnavailable)?;
    work.set_peer_id(peer)?;
    configure(&work);
    // The whole body at the base must be valid before any of it is reconciled.
    read_tree(&work)?;
    let before = work.oplog_vv();
    reconcile_blocks(&body_list(&work)?, &target, 1)?;
    work.commit();
    let after = work.oplog_vv();
    let ops = after
        .iter()
        .map(|(peer, end)| (*end - before.get(peer).copied().unwrap_or(0)) as usize)
        .sum();
    let update = work
        .export(ExportMode::updates(&before))
        .map_err(LoroError::from)?;
    Ok(Merged { update, ops })
}

/// What Markdown makes of a block on its own. `None` means it vanishes (an empty paragraph).
fn normalize(block: &Block) -> Option<Block> {
    let mut parsed = parse(&serialize(std::slice::from_ref(block)));
    match parsed.len() {
        0 => None,
        1 => parsed.pop(),
        _ => Some(block.clone()),
    }
}

fn same_kind(a: &Block, b: &Block) -> bool {
    a.node_name() == b.node_name()
}

/// Matched index pairs in order: the unchanged prefix and suffix, and a longest common
/// subsequence of what lies between them while its table fits [`LCS_CELLS`].
fn matches<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let middle_a = &a[prefix..a.len() - suffix];
    let middle_b = &b[prefix..b.len() - suffix];
    let mut out: Vec<(usize, usize)> = (0..prefix).map(|i| (i, i)).collect();
    if middle_a.len().saturating_mul(middle_b.len()) <= LCS_CELLS {
        out.extend(
            lcs(middle_a, middle_b)
                .into_iter()
                .map(|(i, j)| (i + prefix, j + prefix)),
        );
    }
    out.extend((0..suffix).map(|k| (a.len() - suffix + k, b.len() - suffix + k)));
    out
}

/// Longest common subsequence: matched index pairs in order.
fn lcs<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let width = m + 1;
    let mut table = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

enum Step<'t> {
    Keep,
    Update(&'t Block),
    UpdateItem(&'t [Block]),
    Delete,
    Insert(&'t Block),
    InsertItem(&'t [Block]),
}

/// Aligns current entries with targets: matched equality keys, then leftovers of the same
/// kind paired by position inside each gap.
fn align<K: PartialEq>(
    current_keys: &[K],
    target_keys: &[K],
    pairable: impl Fn(usize, usize) -> bool,
    protected: impl Fn(usize) -> bool,
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (mi, mj) in matches(current_keys, target_keys)
        .into_iter()
        .chain(std::iter::once((current_keys.len(), target_keys.len())))
    {
        let mut gap_target = (j..mj).peekable();
        for c in i..mi {
            if protected(c) {
                out.push((Some(c), None));
                continue;
            }
            match gap_target.peek() {
                Some(&t) if pairable(c, t) => {
                    out.push((Some(c), Some(t)));
                    gap_target.next();
                }
                _ => out.push((Some(c), None)),
            }
        }
        for t in gap_target {
            out.push((None, Some(t)));
        }
        if mi < current_keys.len() {
            out.push((Some(mi), Some(mj)));
        }
        i = mi + 1;
        j = mj + 1;
    }
    out
}

/// Reconciles a block list whose blocks sit at `depth`.
fn reconcile_blocks(list: &LoroList, target: &[Block], depth: usize) -> Result<(), MergeError> {
    let maps = child_maps(list);
    let current: Vec<Block> = maps
        .iter()
        .map(|map| read_block(map, "doc", depth))
        .collect::<Result<_, _>>()?;
    let current_keys: Vec<Option<Block>> = current.iter().map(normalize).collect();
    let target_keys: Vec<Option<Block>> = target.iter().map(normalize).collect();
    // An empty paragraph the revision does not mention stays, because Markdown cannot show
    // it; but a list that shows nothing at all (a new Document's genesis paragraph) is
    // replaced by a revision that shows something.
    let keep_blank =
        !(current_keys.iter().all(Option::is_none) && target_keys.iter().any(Option::is_some));
    let alignment = align(
        &current_keys,
        &target_keys,
        |c, t| same_kind(&current[c], &target[t]),
        |c| keep_blank && current_keys[c].is_none(),
    );
    let steps: Vec<Step> = alignment
        .into_iter()
        .map(|pair| match pair {
            (Some(c), Some(t)) if current[c] == target[t] => Step::Keep,
            (Some(c), Some(t)) if current_keys[c] == target_keys[t] => Step::Keep,
            (Some(_), Some(t)) => Step::Update(&target[t]),
            (Some(c), None) if keep_blank && current_keys[c].is_none() => Step::Keep,
            (Some(_), None) => Step::Delete,
            (None, Some(t)) => Step::Insert(&target[t]),
            (None, None) => unreachable!("an alignment pair holds at least one side"),
        })
        .collect();
    apply_steps(list, &steps, depth)
}

/// Reconciles the items of a list whose items sit at `depth`.
fn reconcile_items(list: &LoroList, target: &[Vec<Block>], depth: usize) -> Result<(), MergeError> {
    let maps = child_maps(list);
    let current: Vec<Vec<Block>> = maps
        .iter()
        .map(|map| read_item(map, depth))
        .collect::<Result<_, _>>()?;
    let key = |item: &Vec<Block>| -> Vec<Block> { item.iter().filter_map(normalize).collect() };
    let current_keys: Vec<Vec<Block>> = current.iter().map(key).collect();
    let target_keys: Vec<Vec<Block>> = target.iter().map(key).collect();
    let alignment = align(&current_keys, &target_keys, |_, _| true, |_| false);
    let steps: Vec<Step> = alignment
        .into_iter()
        .map(|pair| match pair {
            (Some(c), Some(t)) if current_keys[c] == target_keys[t] => Step::Keep,
            (Some(_), Some(t)) => Step::UpdateItem(&target[t]),
            (Some(_), None) => Step::Delete,
            (None, Some(t)) => Step::InsertItem(&target[t]),
            (None, None) => unreachable!("an alignment pair holds at least one side"),
        })
        .collect();
    apply_steps(list, &steps, depth)
}

fn apply_steps(list: &LoroList, steps: &[Step], depth: usize) -> Result<(), MergeError> {
    let mut cursor = 0;
    for step in steps {
        match step {
            Step::Keep => cursor += 1,
            Step::Update(block) => {
                update_block(&map_at(list, cursor)?, block, depth)?;
                cursor += 1;
            }
            Step::UpdateItem(item) => {
                let map = map_at(list, cursor)?;
                reconcile_blocks(&children(&map)?, item, depth + 1)?;
                cursor += 1;
            }
            Step::Delete => list.delete(cursor, 1)?,
            Step::Insert(block) => {
                insert_block(list, cursor, block)?;
                cursor += 1;
            }
            Step::InsertItem(item) => {
                insert_item(list, cursor, item)?;
                cursor += 1;
            }
        }
    }
    Ok(())
}

fn child_maps(list: &LoroList) -> Vec<LoroMap> {
    list_entries(list)
        .into_iter()
        .filter_map(|entry| match entry {
            ValueOrContainer::Container(Container::Map(map)) => Some(map),
            _ => None,
        })
        .collect()
}

fn map_at(list: &LoroList, index: usize) -> Result<LoroMap, MergeError> {
    match list.get(index) {
        Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
        _ => Err(MergeError::Schema(SchemaViolation::MisplacedContent {
            parent: "doc",
        })),
    }
}

/// A node's `children` list, created when the node has none.
fn children(map: &LoroMap) -> Result<LoroList, MergeError> {
    match child_list(map, "node")? {
        Some(list) => Ok(list),
        None => Ok(map.insert_container(CHILDREN, LoroList::new())?),
    }
}

/// A node's `attributes` map, created when the node has none.
fn attributes(map: &LoroMap) -> Result<LoroMap, MergeError> {
    match map.get(ATTRIBUTES) {
        Some(ValueOrContainer::Container(Container::Map(attributes))) => Ok(attributes),
        None => Ok(map.insert_container(ATTRIBUTES, LoroMap::new())?),
        Some(_) => Err(MergeError::Schema(SchemaViolation::InvalidContainer {
            node: "node",
        })),
    }
}

/// Edits a block of the same kind as `target`, sitting at `depth`, into `target`.
fn update_block(map: &LoroMap, target: &Block, depth: usize) -> Result<(), MergeError> {
    let attributes = attributes(map)?;
    for (key, value) in block_attributes(target) {
        let same =
            matches!(attributes.get(key), Some(ValueOrContainer::Value(ref v)) if *v == value);
        if !same {
            attributes.insert(key, value)?;
        }
    }
    let list = children(map)?;
    match target {
        Block::Paragraph(content) | Block::Heading { content, .. } => {
            reconcile_inlines(&list, content)
        }
        Block::Code { text, .. } => {
            match list_entries(&list).into_iter().next() {
                Some(ValueOrContainer::Container(Container::Text(run))) => {
                    update_text(&run, &[(text.clone(), Marks::default())])?
                }
                _ => {
                    list.clear()?;
                    if !text.is_empty() {
                        let run = list.insert_container(0, LoroText::new())?;
                        run.insert(0, text)?;
                    }
                }
            }
            Ok(())
        }
        Block::Blockquote(blocks) => reconcile_blocks(&list, blocks, depth + 1),
        Block::Rule => Ok(()),
        Block::Bullet { items, .. } | Block::Ordered { items, .. } => {
            reconcile_items(&list, items, depth + 1)
        }
    }
}

fn reconcile_inlines(list: &LoroList, target: &[Inline]) -> Result<(), MergeError> {
    let entries = list_entries(list);
    let wanted = segments(target);
    let shape_matches = entries.len() == wanted.len()
        && entries.iter().zip(&wanted).all(|(entry, want)| {
            matches!(
                (entry, want),
                (
                    ValueOrContainer::Container(Container::Text(_)),
                    Segment::Text(_)
                ) | (
                    ValueOrContainer::Container(Container::Map(_)),
                    Segment::Break
                )
            )
        });
    if shape_matches {
        for (entry, want) in entries.iter().zip(&wanted) {
            if let (ValueOrContainer::Container(Container::Text(run)), Segment::Text(runs)) =
                (entry, want)
            {
                update_text(run, runs)?;
            }
        }
    } else {
        // A change in hard-break structure rebuilds the paragraph's inline content.
        list.clear()?;
        insert_inlines(list, 0, target)?;
    }
    Ok(())
}

/// Character diff for the text, then range marks for any formatting difference.
fn update_text(run: &LoroText, wanted: &[(String, Marks)]) -> Result<(), MergeError> {
    let target: String = wanted.iter().map(|(text, _)| text.as_str()).collect();
    if run.to_string() != target {
        run.update(
            &target,
            UpdateOptions {
                timeout_ms: None,
                use_refined_diff: true,
            },
        )
        .expect("a text diff without a timeout cannot time out");
    }
    let mut want_marks: Vec<Marks> = Vec::new();
    for (text, marks) in wanted {
        want_marks.extend(std::iter::repeat_n(marks.clone(), text.chars().count()));
    }
    let mut have_marks: Vec<Marks> = Vec::new();
    for inline in read_runs(run)? {
        if let Inline::Text(text, marks) = inline {
            have_marks.extend(std::iter::repeat_n(marks, text.chars().count()));
        }
    }
    debug_assert_eq!(want_marks.len(), have_marks.len());
    for key in ["strong", "em", "strikethrough", "code", "link"] {
        let value = |marks: &Marks| -> Option<LoroValue> {
            mark_values(marks)
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
        };
        let mut index = 0;
        while index < want_marks.len() {
            let want = value(&want_marks[index]);
            if want == value(&have_marks[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < want_marks.len()
                && value(&want_marks[index]) == want
                && value(&have_marks[index]) != want
            {
                index += 1;
            }
            match &want {
                Some(v) => run.mark(start..index, key, v.clone())?,
                None => run.unmark(start..index, key)?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_trims_prefix_and_suffix() {
        assert_eq!(
            matches(&[1, 2, 3, 4], &[1, 9, 3, 4]),
            vec![(0, 0), (2, 2), (3, 3)]
        );
        assert_eq!(matches(&[1, 2], &[1, 2]), vec![(0, 0), (1, 1)]);
        assert_eq!(matches::<u8>(&[], &[1]), vec![]);
    }

    #[test]
    fn matches_without_a_table_beyond_the_cell_bound() {
        // 2049 × 2049 middle cells exceed the bound; only prefix and suffix match.
        let a: Vec<u32> = (0..2051).collect();
        let mut b = a.clone();
        b[1..2050].reverse();
        b[1] = u32::MAX;
        b[2049] = u32::MAX - 1;
        assert_eq!(matches(&a, &b), vec![(0, 0), (2050, 2050)]);
    }
}
