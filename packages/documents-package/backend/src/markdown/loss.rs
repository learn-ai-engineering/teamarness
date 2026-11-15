//! The loss check: whether importing outside Markdown would lose any of its text.
//!
//! Import drops some text, and the contract's list of drops is what testing has found, not a
//! proof (see `context/02-system/09-documents/markdown-contract.md`). The check does not depend
//! on that list. It compares the words of the input with the words of its Canonical Markdown.

use std::collections::HashMap;

use super::export::{canonical, item_numbers};
use super::schema::SchemaViolation;

/// The most tokens [`lost_tokens`] reports.
pub const MAX_LOST_TOKENS: usize = 8;

/// The tokens of `markdown` that its Canonical Markdown does not keep, at most
/// [`MAX_LOST_TOKENS`], each once, in input order. Empty means import loses no text.
///
/// A token is a maximal run of Unicode alphanumeric characters. Each token must appear in
/// [`canonical`]`(markdown)` at least as often as in `markdown`, less one occurrence for each
/// time it is syntax that import consumes:
///
/// - a token inside a reference label: a definition `[label]:`, a full reference `][label]` or a
///   collapsed reference `[label][]`. A resolved reference link loses its label and keeps its
///   text and target;
/// - the number of an ordered list item's marker. Export numbers items from their list's start,
///   so `1.` `1.` `1.` becomes `1.` `2.` `3.`.
///
/// The check is conservative. An entity such as `&copy;` imports as `©`, so it loses the token
/// `copy`. The error is the one [`canonical`] gives.
pub fn lost_tokens(markdown: &str) -> Result<Vec<String>, SchemaViolation> {
    let exported = canonical(markdown)?;
    let mut kept = counts(tokens(&exported));
    let mut syntax = counts(
        label_spans(markdown)
            .into_iter()
            .flat_map(tokens)
            .chain(item_numbers(markdown)),
    );
    let mut lost: Vec<String> = Vec::new();
    for token in tokens(markdown) {
        if take(&mut kept, token) || take(&mut syntax, token) {
            continue;
        }
        if !lost.iter().any(|seen| seen == token) {
            lost.push(token.to_owned());
            if lost.len() == MAX_LOST_TOKENS {
                break;
            }
        }
    }
    Ok(lost)
}

/// Each maximal run of Unicode alphanumeric characters, in order.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
}

fn counts<'t>(tokens: impl Iterator<Item = &'t str>) -> HashMap<&'t str, usize> {
    let mut counts = HashMap::new();
    for token in tokens {
        *counts.entry(token).or_default() += 1;
    }
    counts
}

/// Uses one occurrence of `token`, if one is left.
fn take(counts: &mut HashMap<&str, usize>, token: &str) -> bool {
    match counts.get_mut(token) {
        Some(count) if *count > 0 => {
            *count -= 1;
            true
        }
        _ => false,
    }
}

/// The text inside each bracket pair that reads as a reference label: `[label]:`, `][label]` or
/// `[label][]`. A bracket pair holds no unescaped bracket. Where the label sits, such as in code,
/// is not checked, so the exemption can be wider than the references import resolves.
fn label_spans(markdown: &str) -> Vec<&str> {
    let bytes = markdown.as_bytes();
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'[' => open = Some(index),
            b']' => {
                if let Some(start) = open.take() {
                    let after = &bytes[index + 1..];
                    let full = start > 0 && bytes[start - 1] == b']';
                    let definition = after.first() == Some(&b':');
                    let collapsed = after.starts_with(b"[]");
                    if index > start + 1 && (full || definition || collapsed) {
                        spans.push(&markdown[start + 1..index]);
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lost(markdown: &str) -> Vec<String> {
        lost_tokens(markdown).unwrap()
    }

    #[test]
    fn tokens_are_runs_of_alphanumerics() {
        let found: Vec<&str> = tokens("a-b c_d é1 **x**,y\n日本").collect();
        assert_eq!(found, ["a", "b", "c", "d", "é1", "x", "y", "日本"]);
    }

    #[test]
    fn labels_are_found_in_each_reference_form() {
        assert_eq!(
            label_spans("[x][r] [s][] [t]: u [plain] \\[e\\]: f"),
            ["r", "s", "t"]
        );
    }

    #[test]
    fn known_drops_are_reported() {
        // An unused definition, and an unused footnote.
        assert_eq!(
            lost("a\n\n[r]: https://x.test/page\n"),
            ["https", "x", "test", "page"]
        );
        assert_eq!(lost("a\n\n[^1]: note\n"), ["note"]);
        // An image's title.
        assert_eq!(lost("![p](https://x.test/p.png \"title\")\n"), ["title"]);
        // A destination with a space removes the target.
        assert_eq!(lost("a [x](<b c>) d\n"), ["b", "c"]);
        // The separator comment between two lists other than lists.
        assert_eq!(lost("a\n\n<!-- end list -->\n\nb\n"), ["end", "list"]);
    }

    #[test]
    fn what_import_keeps_reports_nothing() {
        for case in [
            "Plain *text* with `code` and **bold**.\n",
            // Escaping only.
            "a \\* b \\_c\\_ 1\\. d\n",
            "a * b _c 1. d [e] f\n",
            // Resolved references of each form, and a shortcut reference.
            "[x][r] [r][] [r]\n\n[r]: https://x.test/page \"Page\"\n",
            // A table keeps its lines and gets a links paragraph.
            "| a |\n|---|\n| [x][r] |\n\n[r]: https://x.test/page\n",
            // Syntax forms that import as their canonical equivalents.
            "Title\n=====\n\n* a\n+ b\n\n1) c\n\n__d__ _e_ ~f~\n\n~~~ rust\ng\n~~~\n",
            "---\ntitle: X\n---\n\nBody <b>inline</b>\n\n<div>\nhtml\n</div>\n",
        ] {
            assert!(lost(case).is_empty(), "{case}: {:?}", lost(case));
        }
    }

    #[test]
    fn renumbered_list_items_lose_no_text() {
        for case in [
            "1. a\n1. b\n1. c\n",
            "3. a\n7. b\n",
            "- x\n\n  01) a\n  09) b\n",
            "> 2. a\n> 2. b\n",
        ] {
            assert!(lost(case).is_empty(), "{case}: {:?}", lost(case));
        }
        // A number in an item's text is text.
        assert_eq!(
            lost("1. a\n\n[r]: https://x.test/7\n"),
            ["https", "x", "test", "7"]
        );
    }

    #[test]
    fn an_entity_is_reported_because_the_check_is_conservative() {
        assert_eq!(lost("a &copy; b\n"), ["copy"]);
    }

    #[test]
    fn a_label_is_exempt_only_as_often_as_it_is_a_label() {
        // `r` is a label twice, and the export keeps it once as text.
        assert!(lost("[r][r]\n\n[r]: https://x.test/page\n").is_empty());
        // The definition is unused, so its label is exempt but its target is lost.
        // `r` is a label once, so its use in the lost target is still reported.
        assert_eq!(
            lost("r\n\n[r]: https://x.test/r\n"),
            ["https", "x", "test", "r"]
        );
    }

    #[test]
    fn at_most_eight_tokens_are_reported_each_once() {
        let markdown = "a\n\n[r]: https://x.test/one/two/three/four/five/six/seven?page=page\n";
        assert_eq!(
            lost(markdown),
            ["https", "x", "test", "one", "two", "three", "four", "five"]
        );
        assert_eq!(
            lost("a\n\n[r]: https://x.test/x/x/x\n"),
            ["https", "x", "test"]
        );
    }

    #[test]
    fn a_schema_violation_is_the_error() {
        let deep = format!("{}a\n", "> ".repeat(200));
        assert_eq!(lost_tokens(&deep), Err(SchemaViolation::TooDeep));
    }
}
