//! The three-way merge of the spec (§4, O1–O5), written from `docs/SPEC-UNDO.md`.
//!
//! The only kit code it calls is the deterministic diff (D1–D2): both sides' changes are
//! `diff::hunks(base, side)`. Classifying pairs of hunks (O1–O5) and placing ours into
//! theirs' coordinates are this module's own.

use k2_mpc_base::text::diff;

use super::hunk::{self, Edit};

/// What a merge found ([`merge3_window`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// Ours' changes placed on theirs: `edits` in theirs' coordinates (canonical), and the
    /// text they give.
    Clean { text: String, edits: Vec<Edit> },
    /// Some pair of changes overlaps (O1, O2, or O5 outside the doc's end).
    Conflict,
}

/// Which rule decided a merge, for the trace runner's case classes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rules {
    /// O1: two non-empty ranges intersect.
    pub o1: bool,
    /// O2: an insertion strictly inside the other side's non-empty range.
    pub o2: bool,
    /// O4: an identical change on both sides, taken once.
    pub o4: bool,
    /// O5 fired: two different insertions at the doc's end both landed, theirs first.
    pub o5_landed: bool,
    /// Two different insertions at the window's end conflicted because `at_end` was false
    /// (the window isn't the doc's end, or the whole doc was empty in the base).
    pub same_point_at_window_end: bool,
}

/// `merge3_window(base, ours, theirs, at_end)` (§4, U10): apply what ours changed from
/// `base` on top of `theirs`. `at_end` is the caller's: the window ends at the live doc's end
/// **and** the whole doc was non-empty in the merge's base (U10). Only then do two
/// different insertions at the window's end both land (O5).
pub fn merge3_window(base: &str, ours: &str, theirs: &str, at_end: bool) -> (Merged, Rules) {
    let a: Vec<Edit> = diff::hunks(base, ours)
        .into_iter()
        .map(Edit::from)
        .collect();
    let b: Vec<Edit> = diff::hunks(base, theirs)
        .into_iter()
        .map(Edit::from)
        .collect();
    let mut rules = Rules::default();
    let mut conflict = false;
    let mut taken_once = vec![false; a.len()];
    for (i, x) in a.iter().enumerate() {
        for y in &b {
            if x == y {
                // O4: theirs already has it
                taken_once[i] = true;
                rules.o4 = true;
                continue;
            }
            let (xi, yi) = (x.is_insertion(), y.is_insertion());
            if !xi && !yi {
                // O1
                if x.start < y.end && y.start < x.end {
                    rules.o1 = true;
                    conflict = true;
                }
            } else if xi && !yi {
                // O2 (O3: at y's start or end it lands before or after it)
                if y.start < x.start && x.start < y.end {
                    rules.o2 = true;
                    conflict = true;
                }
            } else if !xi && yi {
                if x.start < y.start && y.start < x.end {
                    rules.o2 = true;
                    conflict = true;
                }
            } else if x.start == y.start {
                // two different insertions at one point: O5 at the doc's end, else conflict
                if at_end && x.start == base.len() {
                    rules.o5_landed = true;
                } else {
                    if x.start == base.len() {
                        rules.same_point_at_window_end = true;
                    }
                    conflict = true;
                }
            }
        }
    }
    if conflict {
        rules.o5_landed = false;
        return (Merged::Conflict, rules);
    }
    // Place ours in theirs' coordinates. Every theirs change that ends at or before our
    // start lands before ours: theirs' insertion at our start (O3), theirs' range ending at
    // our insertion point (O3), and theirs' append before ours at the doc's end (O5).
    let mut placed = Vec::with_capacity(a.len());
    for (i, x) in a.iter().enumerate() {
        if taken_once[i] {
            continue;
        }
        let shift: isize = b
            .iter()
            .filter(|y| y.end <= x.start)
            .map(Edit::growth)
            .sum();
        placed.push(Edit {
            start: (x.start as isize + shift) as usize,
            end: (x.end as isize + shift) as usize,
            insert: x.insert.clone(),
        });
    }
    // theirs may have deleted what separated two of ours: canonical again
    let edits = hunk::canonical(placed);
    let text = hunk::apply(theirs, &edits);
    (Merged::Clean { text, edits }, rules)
}

/// Whole-text `merge3` (`write --base`): the window is the whole doc, so O5 applies at its
/// end when the base is non-empty (two writes to an empty doc conflict, O5).
pub fn merge3(base: &str, ours: &str, theirs: &str) -> (Merged, Rules) {
    merge3_window(base, ours, theirs, !base.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(m: (Merged, Rules)) -> Option<String> {
        match m.0 {
            Merged::Clean { text, .. } => Some(text),
            Merged::Conflict => None,
        }
    }

    #[test]
    fn rules() {
        // O3: touching does not conflict
        assert_eq!(text(merge3("abc", "Xabc", "abcY")), Some("XabcY".into()));
        // O1
        assert_eq!(text(merge3("abcd", "aXd", "abYd")), None);
        // O4
        assert_eq!(text(merge3("ab", "aXb", "aXb")), Some("aXb".into()));
        // O5: both appends land, theirs first; not on an empty base
        assert_eq!(
            text(merge3("a\n", "a\nx\n", "a\ny\n")),
            Some("a\ny\nx\n".into())
        );
        assert_eq!(text(merge3("", "x", "y")), None);
        // not at a window's end that isn't the doc's
        assert_eq!(text(merge3_window("a", "ax", "ay", false)), None);
        // U12's worked case: the window's own base may be empty
        assert_eq!(
            text(merge3_window("", "x\n", "y\n", true)),
            Some("y\nx\n".into())
        );
    }
}
