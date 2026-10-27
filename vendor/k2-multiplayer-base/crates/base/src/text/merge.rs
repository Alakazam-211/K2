//! Three-way merge for writes made against an older version.
//!
//! A writer read version `base`, produced `ours`, and meanwhile the live doc moved to
//! `theirs`. [`merge3`] applies only what the writer changed (base -> ours) on top of the
//! live text, and refuses with the overlapping regions when the two sides touched the
//! same text. There is no last-writer-wins path. `undo` uses the same function with
//! base = after the edit, ours = before it, theirs = live.
//!
//! Granularity is the character (via [`diff::hunks`]): a writer changing one word and
//! someone typing elsewhere in the same line both survive.
//!
//! Overlap rules, on half-open byte ranges of `base`:
//! - two non-empty ranges that intersect conflict;
//! - two insertions at the same point conflict, unless they insert the same text;
//! - an insertion strictly inside the other side's range conflicts;
//! - touching at a boundary does not conflict: the insertion lands on that side of it;
//! - an identical change on both sides is taken once.

use std::ops::Range;

use super::diff::{self, Hunk};

/// Writer changes rebased onto the live text, ready to apply as edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clean {
    /// Hunks in `theirs` (live) coordinates. Applying them to `theirs` gives `text`.
    pub hunks: Vec<Hunk>,
    pub text: String,
}

/// One region both sides changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Byte range in `base`.
    pub base: Range<usize>,
    /// 1-based line range in `base` (inclusive start, inclusive end).
    pub lines: (usize, usize),
    pub base_text: String,
    pub ours: String,
    pub theirs: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    Clean(Clean),
    Conflicts(Vec<Conflict>),
}

pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge {
    let a = diff::hunks(base, ours);
    let b = diff::hunks(base, theirs);

    // Pass 1: find conflicts and drop changes made identically on both sides.
    let mut keep = vec![true; a.len()];
    let mut clash: Vec<(usize, usize)> = Vec::new(); // (index in a, index in b)
    let mut j0 = 0;
    for (i, x) in a.iter().enumerate() {
        while j0 < b.len() && b[j0].end < x.start {
            j0 += 1;
        }
        let mut j = j0;
        while j < b.len() && b[j].start <= x.end {
            let y = &b[j];
            if x == y {
                keep[i] = false;
            } else if overlaps(x, y) {
                clash.push((i, j));
            }
            j += 1;
        }
    }
    if !clash.is_empty() {
        return Merge::Conflicts(conflicts(base, &a, &b, &clash));
    }

    // Pass 2: rebase our hunks into theirs' coordinates.
    let mut out = Vec::with_capacity(a.len());
    let mut shift: isize = 0;
    let mut j = 0;
    for (i, x) in a.iter().enumerate() {
        // Every theirs-hunk ending at or before our start shifts us. A theirs-insertion at
        // our start therefore lands before our change; one at our end lands after it.
        while j < b.len() && b[j].end <= x.start {
            shift += b[j].insert.len() as isize - (b[j].end - b[j].start) as isize;
            j += 1;
        }
        if !keep[i] {
            continue;
        }
        let start = (x.start as isize + shift) as usize;
        out.push(Hunk {
            start,
            end: start + (x.end - x.start),
            insert: x.insert.clone(),
        });
    }
    // Theirs may have deleted what separated two of our hunks; join hunks that now touch.
    let out = diff::coalesce(out);
    debug_assert!(
        diff::validate(theirs, &out).is_ok(),
        "rebased hunks must be valid on theirs"
    );
    let text = diff::apply(theirs, &out);
    Merge::Clean(Clean { hunks: out, text })
}

fn overlaps(x: &Hunk, y: &Hunk) -> bool {
    match (x.is_insertion(), y.is_insertion()) {
        (true, true) => x.start == y.start, // same point, different text (identical handled by caller)
        (true, false) => y.start < x.start && x.start < y.end,
        (false, true) => x.start < y.start && y.start < x.end,
        (false, false) => x.start < y.end && y.start < x.end,
    }
}

/// Group clashing hunks into regions (a chain of overlaps is one region) and render each side.
fn conflicts(base: &str, a: &[Hunk], b: &[Hunk], clash: &[(usize, usize)]) -> Vec<Conflict> {
    let mut regions: Vec<(Range<usize>, Vec<usize>, Vec<usize>)> = Vec::new();
    for &(i, j) in clash {
        let r = a[i].start.min(b[j].start)..a[i].end.max(b[j].end);
        match regions.last_mut() {
            Some((rr, ai, bj)) if r.start <= rr.end => {
                rr.end = rr.end.max(r.end);
                rr.start = rr.start.min(r.start);
                if !ai.contains(&i) {
                    ai.push(i)
                }
                if !bj.contains(&j) {
                    bj.push(j)
                }
            }
            _ => regions.push((r, vec![i], vec![j])),
        }
    }
    regions
        .into_iter()
        .map(|(r, ai, bj)| {
            let side = |hs: &[Hunk], idx: &[usize]| -> String {
                let local: Vec<Hunk> = idx
                    .iter()
                    .map(|&k| Hunk {
                        start: hs[k].start - r.start,
                        end: hs[k].end - r.start,
                        insert: hs[k].insert.clone(),
                    })
                    .collect();
                diff::apply(&base[r.clone()], &local)
            };
            let start_line = base[..r.start].matches('\n').count() + 1;
            let inner = &base[r.clone()];
            let spans = inner
                .matches('\n')
                .count()
                .saturating_sub(inner.ends_with('\n') as usize);
            Conflict {
                lines: (start_line, start_line + spans),
                base_text: base[r.clone()].to_owned(),
                ours: side(a, &ai),
                theirs: side(b, &bj),
                base: r,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn clean(base: &str, ours: &str, theirs: &str) -> String {
        match merge3(base, ours, theirs) {
            Merge::Clean(c) => {
                assert_eq!(diff::apply(theirs, &c.hunks), c.text);
                c.text
            }
            Merge::Conflicts(c) => panic!("unexpected conflict: {c:#?}"),
        }
    }

    #[test]
    fn agent_word_change_survives_concurrent_typing_in_same_line() {
        let base = "The quick brown fox.\n";
        let ours = "The quick red fox.\n"; // agent
        let theirs = "The quick brown fox jumps.\n"; // a person typed at the end
        assert_eq!(clean(base, ours, theirs), "The quick red fox jumps.\n");
    }

    #[test]
    fn whole_file_write_back_applies_only_what_changed() {
        let base = "# A\n\none\ntwo\nthree\n";
        let ours = "# A\n\none\ntwo\nthree\nfour (agent)\n";
        let theirs = "# A (renamed by Sam)\n\none\ntwo\nthree\n";
        assert_eq!(
            clean(base, ours, theirs),
            "# A (renamed by Sam)\n\none\ntwo\nthree\nfour (agent)\n"
        );
    }

    #[test]
    fn overlapping_edit_conflicts_with_region_and_lines() {
        let base = "a\nkeep\nshared line\nz\n";
        let ours = "a\nkeep\nshared LINE by agent\nz\n";
        let theirs = "a\nkeep\nshared row, said Sam\nz\n"; // both rewrote "line"
        let Merge::Conflicts(c) = merge3(base, ours, theirs) else {
            panic!("expected conflict")
        };
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].lines, (3, 3));
        assert!(
            c[0].ours.contains("LINE") && c[0].theirs.contains("Sam"),
            "{c:#?}"
        );
    }

    #[test]
    fn typing_right_after_a_changed_word_is_not_a_conflict() {
        let base = "shared line\n";
        assert_eq!(
            clean(
                base,
                "shared LINE by agent\n",
                "shared line edited by Sam\n"
            ),
            "shared LINE by agent edited by Sam\n"
        );
    }

    #[test]
    fn same_point_insertions_conflict_unless_identical() {
        assert!(matches!(merge3("ab", "aXb", "aYb"), Merge::Conflicts(_)));
        assert_eq!(clean("ab", "aXb", "aXb"), "aXb");
    }

    #[test]
    fn boundary_touch_does_not_conflict() {
        // ours replaces "bc", theirs inserts right before it and right after it
        assert_eq!(clean("abcd", "aXd", "a1bc2d"), "a1X2d");
    }

    #[test]
    fn undo_shape() {
        // undo = merge3(after_my_edit, before_my_edit, live)
        let before = "x\ny\n";
        let after = "x\nAGENT\ny\n";
        let live = "Sam\nx\nAGENT\ny\n";
        assert_eq!(clean(after, before, live), "Sam\nx\ny\n");
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::string::string_regex("([ab日🎉\n]|ab\n){0,24}").unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3000))]
        #[test]
        fn identities(base in text(), other in text()) {
            // nothing changed on our side -> theirs
            prop_assert_eq!(clean(&base, &base, &other), other.clone());
            // nothing changed on their side -> ours
            prop_assert_eq!(clean(&base, &other, &base), other.clone());
            // both made the same change -> that change once
            prop_assert_eq!(clean(&base, &other, &other), other);
        }

        #[test]
        fn disjoint_edits_both_land(left in text(), mid in text(), right in text(), l2 in text(), r2 in text()) {
            // ours edits only the left part, theirs only the right; a fixed separator keeps them apart
            let base = format!("{left}|{mid}|{right}");
            let ours = format!("{l2}|{mid}|{right}");
            let theirs = format!("{left}|{mid}|{r2}");
            prop_assert_eq!(clean(&base, &ours, &theirs), format!("{l2}|{mid}|{r2}"));
        }

        #[test]
        fn clean_merges_are_symmetric(base in text(), o in text(), t in text()) {
            if let (Merge::Clean(x), Merge::Clean(y)) = (merge3(&base, &o, &t), merge3(&base, &t, &o)) {
                prop_assert_eq!(x.text, y.text);
            }
        }
    }
}
