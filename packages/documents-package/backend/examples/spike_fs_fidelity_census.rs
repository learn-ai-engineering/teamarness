//! SPIKE (filesystem-docs prototype) — throwaway, never merge as-is.
//!
//! Fidelity census: how many real planning documents change when they pass through the
//! Documents body (file -> Document genesis -> Canonical Markdown), and which Markdown
//! constructs cause the change. Also checks the on-disk round trip: the export is a fixed
//! point and merging it back produces no ops.
//!
//! Run: cargo run -p teamarness-documents-package --example spike_fs_fidelity_census -- <dir>...

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};
use loro::LoroDoc;
use teamarness_documents_package::markdown::{canonical, export, merge, write_tree};

const TEXT_BUDGET: usize = 192 * 1024;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Constructs present in the original that the body schema cannot carry.
fn constructs(markdown: &str) -> Vec<&'static str> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.strikethrough = true;
    options.extension.front_matter_delimiter = Some("---".into());
    let root = parse_document(&arena, markdown, &options);
    let mut found = Vec::new();
    fn visit<'a>(node: &'a AstNode<'a>, found: &mut Vec<&'static str>) {
        let tag = match &node.data.borrow().value {
            NodeValue::FrontMatter(_) => Some("front-matter"),
            NodeValue::Table(_) => Some("table"),
            NodeValue::TaskItem(_) => Some("task-list"),
            NodeValue::Image(_) => Some("image"),
            NodeValue::HtmlBlock(_) | NodeValue::HtmlInline(_) => Some("html"),
            NodeValue::FootnoteDefinition(_) | NodeValue::FootnoteReference(_) => {
                Some("footnote")
            }
            _ => None,
        };
        if let Some(tag) = tag {
            if !found.contains(&tag) {
                found.push(tag);
            }
        }
        for child in node.children() {
            visit(child, found);
        }
    }
    visit(root, &mut found);
    found
}

/// Lines of `a` not matched in order in `b` (cheap LCS-free approximation: multiset diff).
fn changed_lines(a: &str, b: &str) -> usize {
    let mut counts: BTreeMap<&str, isize> = BTreeMap::new();
    for line in a.lines() {
        *counts.entry(line).or_default() += 1;
    }
    for line in b.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    counts.values().filter(|v| **v > 0).map(|v| *v as usize).sum()
}

fn main() {
    let mut files = Vec::new();
    for dir in std::env::args().skip(1) {
        walk(Path::new(&dir), &mut files);
    }
    files.sort();
    let mut unchanged = 0;
    let mut changed_plain = 0; // changed, but no unsupported construct (style normalisation only)
    let mut by_construct: BTreeMap<&str, (usize, usize)> = BTreeMap::new(); // (files with, files changed with)
    let mut over_budget = 0;
    let mut not_fixed_point = 0;
    let mut nonzero_noop = 0;
    let mut errors = 0;
    let mut total_lines = 0;
    let mut total_changed_lines = 0;
    let mut slowest = (0u128, PathBuf::new());
    let mut plain_examples = Vec::new();
    for path in &files {
        let original = fs::read_to_string(path).unwrap();
        let found = constructs(&original);
        for c in &found {
            by_construct.entry(c).or_default().0 += 1;
        }
        if original.len() > TEXT_BUDGET {
            over_budget += 1;
        }
        let started = Instant::now();
        let doc = LoroDoc::new();
        write_tree(&doc, &[]).unwrap();
        let merged = match merge(&doc, &doc.oplog_frontiers(), &original, 0xF5F5) {
            Ok(m) => m,
            Err(error) => {
                errors += 1;
                println!("ERROR {}: {error}", path.display());
                continue;
            }
        };
        doc.import(&merged.update).unwrap();
        let exported = export(&doc).unwrap();
        // Round trip on disk: export is a fixed point, and merging it back is a no-op.
        if canonical(&exported).unwrap() != exported {
            not_fixed_point += 1;
        }
        let noop = merge(&doc, &doc.oplog_frontiers(), &exported, 0xF6F6).unwrap();
        if noop.ops != 0 {
            nonzero_noop += 1;
        }
        let elapsed = started.elapsed().as_micros();
        if elapsed > slowest.0 {
            slowest = (elapsed, path.clone());
        }
        total_lines += original.lines().count();
        if exported == original {
            unchanged += 1;
            continue;
        }
        total_changed_lines += changed_lines(&original, &exported);
        if found.is_empty() {
            changed_plain += 1;
            if plain_examples.len() < 4 {
                plain_examples.push(path.clone());
            }
        }
        for c in &found {
            by_construct.entry(c).or_default().1 += 1;
        }
    }
    println!("files: {}", files.len());
    println!("byte-identical after round trip: {unchanged}");
    println!("changed with no unsupported construct (style only): {changed_plain}");
    println!("construct: files containing / files changed");
    for (c, (with, changed)) in &by_construct {
        println!("  {c}: {with} / {changed}");
    }
    println!("lines: {total_lines}, lines changed (approx): {total_changed_lines}");
    println!("over 192 KiB text budget: {over_budget}");
    println!("export not a fixed point: {not_fixed_point}");
    println!("merge of unchanged export produced ops: {nonzero_noop}");
    println!("merge errors: {errors}");
    println!("slowest genesis+export+noop: {} ms {}", slowest.0 / 1000, slowest.1.display());
    for p in plain_examples {
        println!("style-only example: {}", p.display());
    }
}
