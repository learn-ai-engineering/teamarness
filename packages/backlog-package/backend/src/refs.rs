//! Ticket references (ADR 0048): the shortest prefix of a Ticket ID, at least [`MIN_REF`]
//! characters, that no other Ticket known to this Installation shares. A reference is derived
//! from the projection; it is never stored or carried in a record.

/// The shortest reference.
pub(crate) const MIN_REF: usize = 4;
/// The longest reference: a whole Ticket ID.
pub(crate) const MAX_REF: usize = 64;

fn common_prefix(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(a, b)| a == b)
        .count()
}

/// The reference of `id`, given its neighbours in ID order among every known Ticket. In
/// sorted order, the longest prefix `id` shares with any other ID is shared with a neighbour.
pub(crate) fn reference(previous: Option<&str>, id: &str, next: Option<&str>) -> String {
    let shared = previous
        .map_or(0, |other| common_prefix(other, id))
        .max(next.map_or(0, |other| common_prefix(id, other)));
    let length = (shared + 1).clamp(MIN_REF, id.len().min(MAX_REF));
    id[..length].to_owned()
}

/// Reads a reference a Person or Agent typed: 4 to 64 hexadecimal characters with an optional
/// leading `#`, in any case. Returns the lowercase prefix, or `None`.
pub(crate) fn parse(input: &str) -> Option<String> {
    let digits = input.strip_prefix('#').unwrap_or(input);
    ((MIN_REF..=MAX_REF).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| digits.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(prefix: &str) -> String {
        format!("{prefix}{}", "0".repeat(64 - prefix.len()))
    }

    // Verifies: TEAM.SYS.BACKLOG-R11 — four characters when unique, one more than the longest
    // shared prefix otherwise.
    #[test]
    fn reference_is_the_shortest_unique_prefix_of_at_least_four() {
        let lone = id("abcdef");
        assert_eq!(reference(None, &lone, None), "abcd");
        let (a, b, c) = (id("a3f27"), id("a3f2c"), id("a3f9"));
        assert_eq!(reference(None, &a, Some(&b)), "a3f27");
        assert_eq!(reference(Some(&a), &b, Some(&c)), "a3f2c");
        assert_eq!(reference(Some(&b), &c, None), "a3f9");
        let near = id("12");
        let far = id("13");
        assert_eq!(reference(None, &near, Some(&far)), "1200");
        let same_but_last = format!("{}1", "f".repeat(63));
        let other = format!("{}2", "f".repeat(63));
        assert_eq!(reference(None, &same_but_last, Some(&other)).len(), 64);
    }

    // Verifies: TEAM.SYS.BACKLOG-R11 — `#` is optional, case is ignored, length is 4 to 64.
    #[test]
    fn parse_accepts_hash_and_any_case_within_bounds() {
        assert_eq!(parse("#A3F2").as_deref(), Some("a3f2"));
        assert_eq!(parse("a3f2c").as_deref(), Some("a3f2c"));
        assert_eq!(parse(&"b".repeat(64)).as_deref(), Some(&*"b".repeat(64)));
        for refused in [
            "",
            "#",
            "a3f",
            "#a3f",
            "a3fg",
            "##a3f2",
            " a3f2",
            &"b".repeat(65),
        ] {
            assert_eq!(parse(refused), None, "{refused}");
        }
    }
}
