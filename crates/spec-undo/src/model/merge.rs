//! The three-way merge of the spec (§4, O1–O5), written from `docs/SPEC-UNDO.md`.
//!
//! Undo, redo and revert merge **exact edit sets** ([`merge_edits`], v1.7): no diff. Only
//! `write --base` finds its sides with the kit's deterministic diff (D1–D2, O0), **by whole
//! lines** (v0.9.7.1): the texts closed (a final "\n" added when one has text and lacks
//! it), each side `diff::whole_line_hunks(closed base, closed side)` ([`sides`]), so two
//! changes touching one line always overlap; the final "\n" of a clean merge follows the
//! side that wrote its last line ([`ends_open`], this module's own); the clean result's
//! edits are the verb edits' diff from the live text (`lines::edit_hunks`). (The mutant that
//! re-diffs undo's windows keeps `diff::hunks`.) Classifying pairs of hunks (O1–O5) and
//! placing ours into theirs' coordinates are this module's own.
//!
//! **O4 (v1.8)** is judged on (position, root) for undo, redo and revert ([`Same::Root`]):
//! each side's edits come with the provenance roots of what they insert. `write --base` has
//! no history and keeps hunk equality ([`Same::Hunk`]). O4a (a root put back at most once)
//! reads the live text, so it is the caller's (`Model::merge_regions`).

use k2_mpc_base::text::{diff, lines};

use super::hunk::{self, Edit, Prov};

/// What O4 calls an identical change on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Same {
    /// Equal hunks (`write --base`: both sides are diffs, no history).
    Hunk,
    /// The same range on the base and the same roots inserted, in the same order (v1.8).
    Root,
    /// Mutant (x) `provenance-ignored-in-O4`: element identity. A restoration is a new
    /// element, so an ours edit that inserts is never identical to a theirs edit; pure
    /// deletions of the same range still are.
    Identity,
}

/// What a merge found ([`merge3_window`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// Ours' changes placed on theirs: `edits` in theirs' coordinates (canonical), the
    /// provenance roots of what each inserts, and the text they give.
    Clean {
        text: String,
        edits: Vec<Edit>,
        provs: Vec<Vec<Prov>>,
    },
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
    /// O4 took once an edit that inserts: two restorations of one root met (v1.8).
    pub o4_insert: bool,
    /// O5 fired: two different insertions at the doc's end both landed, theirs first.
    pub o5_landed: bool,
    /// Two different insertions at the window's end conflicted because `at_end` was false
    /// (the window isn't the doc's end, or the whole doc was empty in the base).
    pub same_point_at_window_end: bool,
}

/// `merge3_window(base, ours, theirs, at_end)` (§4, U10): apply what ours changed from
/// `base` on top of `theirs`. `at_end` is the caller's: the window ends at the live doc's end
/// **and** the whole doc was non-empty in the merge's base (U10). Only then do two
/// different insertions at the window's end both land (O5). `write --base`'s merge: by whole
/// lines on the closed texts (v0.9.7.1).
pub fn merge3_window(base: &str, ours: &str, theirs: &str, at_end: bool) -> (Merged, Rules) {
    let (cb, ct) = (closed(base), closed(theirs));
    let (a, b) = (sides(base, ours), sides(base, theirs));
    let (ap, bp) = (vec![Vec::new(); a.len()], vec![Vec::new(); b.len()]);
    match merge_edits(&cb, &a, &ap, &b, &bp, at_end, Same::Hunk) {
        (Merged::Clean { text, edits, .. }, rules) => {
            let mut text = text;
            if ends_open(base, ours, theirs, &a, &b, &edits, &text) {
                text.pop();
            }
            debug_assert_eq!(hunk::apply(&ct, &edits), closed(&text));
            let edits: Vec<Edit> = lines::edit_hunks(theirs, &text)
                .into_iter()
                .map(Edit::from)
                .collect();
            let provs = vec![Vec::new(); edits.len()];
            (Merged::Clean { text, edits, provs }, rules)
        }
        conflict => conflict,
    }
}

/// `t` with a final "\n" when it has text and lacks one.
fn closed(t: &str) -> String {
    if t.is_empty() || t.ends_with('\n') {
        t.to_owned()
    } else {
        format!("{t}\n")
    }
}

fn is_open(t: &str) -> bool {
    !t.is_empty() && !t.ends_with('\n')
}

/// One of `write --base`'s sides (O0, v0.9.7.1): the closed `base` -> the closed `side`, by
/// whole lines (`diff::whole_line_hunks`): every edit starts at a line start and covers and
/// writes whole lines.
pub fn sides(base: &str, side: &str) -> Vec<Edit> {
    diff::whole_line_hunks(&closed(base), &closed(side))
        .into_iter()
        .map(Edit::from)
        .collect()
}

/// Whether a clean whole-line merge ends without its final "\n": its last line's author
/// decides. `placed` are ours' edits on the closed theirs, giving `merged`; `a` and `b` the
/// sides on the closed base. The last "\n" written by ours: as ours' text ends; written by
/// theirs: as theirs' does; a base line, or a change both made (O4): the side that changed
/// the base's ending (ours when it did, else theirs). An empty last line is never left open.
fn ends_open(
    base: &str,
    ours: &str,
    theirs: &str,
    a: &[Edit],
    b: &[Edit],
    placed: &[Edit],
    merged: &str,
) -> bool {
    if merged.is_empty() || merged == "\n" || merged.ends_with("\n\n") {
        return false;
    }
    let both = if is_open(ours) != is_open(base) {
        is_open(ours)
    } else {
        is_open(theirs)
    };
    // the edit that wrote character `p` of the text `es` give, or where it was copied from
    let source = |es: &[Edit], p: usize| -> Result<usize, usize> {
        let (mut out, mut at) = (0usize, 0usize);
        for (k, e) in es.iter().enumerate() {
            if p < out + (e.start - at) {
                return Err(at + (p - out));
            }
            out += e.start - at;
            if p < out + e.insert.len() {
                return Ok(k);
            }
            out += e.insert.len();
            at = e.end;
        }
        Err(at + (p - out))
    };
    match source(placed, merged.len() - 1) {
        Ok(_) => is_open(ours),
        Err(q) => match source(b, q) {
            Ok(k) if a.contains(&b[k]) => both,
            Ok(_) => is_open(theirs),
            Err(_) => both,
        },
    }
}

/// The merge of two **exact edit sets** on `base` (v1.7, U10): `a` (ours) and `b` (theirs)
/// canonical edits on `base`, as undo, redo and revert have them (no diff), each with the
/// provenance roots of what it inserts (`ap`, `bp`; v1.8). The pair rules and the placement
/// are the same as [`merge3_window`]'s; O4 is judged per `same`.
pub fn merge_edits(
    base: &str,
    a: &[Edit],
    ap: &[Vec<Prov>],
    b: &[Edit],
    bp: &[Vec<Prov>],
    at_end: bool,
    same: Same,
) -> (Merged, Rules) {
    debug_assert!(ap.len() == a.len() && bp.len() == b.len());
    let theirs = hunk::apply(base, b);
    let theirs = theirs.as_str();
    let mut rules = Rules::default();
    let mut conflict = false;
    let mut taken_once = vec![false; a.len()];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            let identical = match same {
                Same::Hunk => x == y,
                Same::Root => x.start == y.start && x.end == y.end && ap[i] == bp[j],
                Same::Identity => {
                    x.start == y.start
                        && x.end == y.end
                        && x.insert.is_empty()
                        && y.insert.is_empty()
                }
            };
            if identical {
                // O4: theirs already has it
                taken_once[i] = true;
                rules.o4 = true;
                rules.o4_insert |= !x.insert.is_empty();
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
        placed.push((
            Edit {
                start: (x.start as isize + shift) as usize,
                end: (x.end as isize + shift) as usize,
                insert: x.insert.clone(),
            },
            ap[i].clone(),
        ));
    }
    // theirs may have deleted what separated two of ours: canonical again
    let (edits, provs): (Vec<Edit>, Vec<Vec<Prov>>) =
        hunk::canonical_provs(placed).into_iter().unzip();
    let text = hunk::apply(theirs, &edits);
    (Merged::Clean { text, edits, provs }, rules)
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
        // O3: touching does not conflict (whole lines, v0.9.7.1)
        assert_eq!(
            text(merge3("a\nb\n", "X\na\nb\n", "a\nb\nY\n")),
            Some("X\na\nb\nY\n".into())
        );
        assert_eq!(
            text(merge3("a\nb\n", "A\nb\n", "a\nB\n")),
            Some("A\nB\n".into())
        );
        // O1
        assert_eq!(text(merge3("a\nb\nc\n", "a\nX\n", "a\nb\nY\n")), None);
        // one line changed by both: a conflict, even in different words (v0.9.7.1)
        assert_eq!(text(merge3("abc", "Xabc", "abcY")), None);
        // O4
        assert_eq!(text(merge3("ab", "aXb", "aXb")), Some("aXb".into()));
        // O5: both appends land, theirs first; not on an empty base
        assert_eq!(
            text(merge3("a\n", "a\nx\n", "a\ny\n")),
            Some("a\ny\nx\n".into())
        );
        assert_eq!(text(merge3("", "x", "y")), None);
        // not at a window's end that isn't the doc's
        assert_eq!(text(merge3_window("a\n", "a\nx\n", "a\ny\n", false)), None);
        // U12's worked case: the window's own base may be empty
        assert_eq!(
            text(merge3_window("", "x\n", "y\n", true)),
            Some("y\nx\n".into())
        );
        // the final "\n" follows the side that wrote the last line
        assert_eq!(text(merge3("a\nb", "A\nb", "a\nB")), Some("A\nB".into()));
        assert_eq!(
            text(merge3("a\nb\n", "a\nb\n", "a\nB")),
            Some("a\nB".into())
        );
    }

    /// `write --base`'s sides by whole lines (v0.9.7.1): lines sharing their start never pair
    /// one line off (the false conflict), an insertion never lands inside the other side's
    /// rewritten lines (the splice), and two changes in one line conflict.
    #[test]
    fn sides_by_whole_lines() {
        assert_eq!(
            text(merge3(
                "p05:2216\np22:2150\np10:2205\n",
                "p05:2216\np22:2150\n",
                "p05:2216\np10:2205\np23:2229\np25:2184\n",
            )),
            Some("p05:2216\np23:2229\np25:2184\n".into())
        );
        assert_eq!(
            text(merge3(
                "p26:95\np05:90\n",
                "p26:95\np13:102\np05:90\n",
                "p20:90\n"
            )),
            None
        );
        assert_eq!(
            text(merge3(
                "alpha beta gamma\n",
                "alpha BETA gamma\n",
                "alpha beta GAMMA\n"
            )),
            None
        );
        assert_eq!(
            text(merge3(
                "p01:2\n",
                "p01:3 w0 w0\np00:3 w0 w3\n",
                "p00:0 w1 w0\np00:0\np02:3\n"
            )),
            None
        );
    }
}
