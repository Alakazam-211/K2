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

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::text::{diff, lines};

use super::hunk::{self, Edit, Prov};
use super::WriteRegion;

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
    /// The end of the text (amendment O5 end-of-text; `write --base`, clean, on a base ending
    /// with "\n"): one side's line landed after the other side left the text open. Read off
    /// the whole-line sides and the texts: the merge itself has no separate rule.
    pub end_break: Option<EndBreak>,
    /// A clean `write --base` on a base with two adjacent lines that start with the same
    /// character, where a character diff reads a side one line off (a hunk starts inside a
    /// line and crosses a line break).
    pub shared_start: bool,
    /// ... and v0.9.7's character-trimmed sides conflict there (a false conflict).
    /// Clean writes only: [`classes`] sets it after its `if !clean { return; }`.
    pub shared_start_was_conflict: bool,
    /// A `write --base` refused where each side turned one base line into a different single
    /// line (different words of one line too: whole lines, v0.9.7.1).
    pub words_same_line: bool,
    /// A `write --base` where one side rewrote one whole base line into two or more lines
    /// and the other changed that line too.
    pub multiline_rewrite: bool,
}

/// The amendment's cases of the end of the text (amendment O5 end-of-text r2), for the trace
/// class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndBreak {
    /// Ours' line lands last; theirs opened the end without inserting there.
    A,
    /// Ours' line lands last after theirs' own open line; ours' text ends with a line break.
    B,
    /// Theirs' line lands last; ours opened the end (it removed the final break).
    C,
    /// Ours' line lands last after theirs' own open line; ours' text stays open.
    D,
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
    let (m, rules, _) = merge_pairs(base, a, ap, b, bp, at_end, same);
    (m, rules)
}

/// [`merge_edits`], with the pairs `(i, j)` of `a[i]` and `b[j]` that conflict (O1, O2, or
/// two different insertions at one point that O5 doesn't land), in the order found.
fn merge_pairs(
    base: &str,
    a: &[Edit],
    ap: &[Vec<Prov>],
    b: &[Edit],
    bp: &[Vec<Prov>],
    at_end: bool,
    same: Same,
) -> (Merged, Rules, Vec<(usize, usize)>) {
    debug_assert!(ap.len() == a.len() && bp.len() == b.len());
    let theirs = hunk::apply(base, b);
    let theirs = theirs.as_str();
    let mut rules = Rules::default();
    let mut conflict = false;
    let mut clash: Vec<(usize, usize)> = Vec::new();
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
                    clash.push((i, j));
                }
            } else if xi && !yi {
                // O2 (O3: at y's start or end it lands before or after it)
                if y.start < x.start && x.start < y.end {
                    rules.o2 = true;
                    conflict = true;
                    clash.push((i, j));
                }
            } else if !xi && yi {
                if x.start < y.start && y.start < x.end {
                    rules.o2 = true;
                    conflict = true;
                    clash.push((i, j));
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
                    clash.push((i, j));
                }
            }
        }
    }
    if conflict {
        rules.o5_landed = false;
        return (Merged::Conflict, rules, clash);
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
    (Merged::Clean { text, edits, provs }, rules, clash)
}

/// Whole-text `merge3` (`write --base`): the window is the whole doc, so O5 applies at its
/// end when the base is non-empty (two writes to an empty doc conflict, O5).
pub fn merge3(base: &str, ours: &str, theirs: &str) -> (Merged, Rules) {
    let (m, mut rules) = merge3_window(base, ours, theirs, !base.is_empty());
    classes(
        base,
        ours,
        theirs,
        matches!(m, Merged::Clean { .. }),
        &mut rules,
    );
    (m, rules)
}

/// The conflict regions of a refused `write --base` (K-BR-2: "conflict, nothing applied,
/// with regions and line numbers"), the oracle's own, read off [`merge3`]'s own sides and
/// pairs: every pair of hunks that conflicts (O1, O2, O5) joins the two hunks, a chain of
/// such pairs is one region, and regions whose ranges on the base overlap or touch join into
/// one (O6, v0.11.1: the kit's grouping, stated). Per region: its
/// range on the base (on the closed base, then within the base's own text: a range that
/// reached the closing "\n" of an open base ends at the base's end), and its 1-based lines
/// (first, last) in the base, in ours and in theirs, on the closed texts (closing adds no
/// line). A side's lines are where that side's text of the region stands in its own text:
/// after what its other hunks before the region did (an insertion at the region's start
/// lands before it, O3), and with every hunk of that side inside the region applied, the
/// region's own and any other (v0.11.1, O6: an insertion where two clashes touch clashes with
/// nothing yet lands inside; one at the region's end lands after it); a region the side
/// deleted gives one line, where it was. Sorted.
/// Empty when the merge is clean.
pub fn write_base_regions(root: usize, base: &str, ours: &str, theirs: &str) -> Vec<WriteRegion> {
    let (cb, co, ct) = (closed(base), closed(ours), closed(theirs));
    let (a, b) = (sides(base, ours), sides(base, theirs));
    let (ap, bp) = (vec![Vec::new(); a.len()], vec![Vec::new(); b.len()]);
    let (_, _, pairs) = merge_pairs(&cb, &a, &ap, &b, &bp, !base.is_empty(), Same::Hunk);
    // the chains: a's hunks are 0..a.len(), b's follow
    let mut up: Vec<usize> = (0..a.len() + b.len()).collect();
    fn top(up: &mut [usize], mut x: usize) -> usize {
        while up[x] != x {
            up[x] = up[up[x]];
            x = up[x];
        }
        x
    }
    for &(i, j) in &pairs {
        let (x, y) = (top(&mut up, i), top(&mut up, a.len() + j));
        up[x] = y;
    }
    let mut chains: BTreeMap<usize, (BTreeSet<usize>, BTreeSet<usize>)> = BTreeMap::new();
    for &(i, j) in &pairs {
        let t = top(&mut up, i);
        let c = chains.entry(t).or_default();
        c.0.insert(i);
        c.1.insert(j);
    }
    let lines_of = |t: &str, at: usize, len: usize| {
        let first = hunk::line_at(t, at);
        let last = if len > 0 {
            hunk::line_at(t, at + len - 1)
        } else {
            first
        };
        (first, last)
    };
    // a side's lines of the region [s, e) of the closed base: `es` that side's hunks, `own`
    // the region's, `t` that side's closed text. The region's text in `t` is the base range
    // with every hunk of that side inside it applied (O6, v0.11.1): its own and any other
    // there (an insertion where two clashes touch clashes with nothing, yet lands inside); an
    // insertion at `s` lands before the region, one at `e` after it (O3)
    let side = |t: &str, es: &[Edit], own: &BTreeSet<usize>, s: usize, e: usize| {
        let before: isize = es
            .iter()
            .enumerate()
            .filter(|(k, x)| !own.contains(k) && x.end <= s)
            .map(|(_, x)| x.growth())
            .sum();
        let inside = |k: usize, x: &Edit| {
            assert!(
                own.contains(&k) || !((x.start < s && s < x.end) || (x.start < e && e < x.end)),
                "write_base_regions: hunk {x:?} crosses the region [{s}, {e}) without clashing"
            );
            own.contains(&k)
                || (s <= x.start
                    && x.end <= e
                    && !(x.is_insertion() && (x.start == s || x.start == e)))
        };
        let grown: isize = es
            .iter()
            .enumerate()
            .filter(|&(k, x)| inside(k, x))
            .map(|(_, x)| x.growth())
            .sum();
        let at = (s as isize + before) as usize;
        let len = ((e - s) as isize + grown) as usize;
        debug_assert!(at + len <= t.len());
        lines_of(t, at, len)
    };
    // each chain's span on the closed base; then chains whose spans overlap or touch join
    // (O6): one region
    let span = |ai: &BTreeSet<usize>, bj: &BTreeSet<usize>| {
        let s = ai
            .iter()
            .map(|&i| a[i].start)
            .chain(bj.iter().map(|&j| b[j].start))
            .min()
            .expect("a chain has hunks");
        let e = ai
            .iter()
            .map(|&i| a[i].end)
            .chain(bj.iter().map(|&j| b[j].end))
            .max()
            .expect("a chain has hunks");
        (s, e)
    };
    let mut spans: Vec<(usize, usize, BTreeSet<usize>, BTreeSet<usize>)> = chains
        .into_values()
        .map(|(ai, bj)| {
            let (s, e) = span(&ai, &bj);
            (s, e, ai, bj)
        })
        .collect();
    spans.sort_by_key(|x| (x.0, x.1));
    let mut joined: Vec<(usize, usize, BTreeSet<usize>, BTreeSet<usize>)> = Vec::new();
    for (s, e, ai, bj) in spans {
        match joined.last_mut() {
            Some(last) if s <= last.1 => {
                last.1 = last.1.max(e);
                last.2.extend(ai);
                last.3.extend(bj);
            }
            _ => joined.push((s, e, ai, bj)),
        }
    }
    let mut out: Vec<WriteRegion> = joined
        .iter()
        .map(|(s, e, ai, bj)| {
            let (s, e) = (*s, *e);
            WriteRegion {
                root,
                start: s.min(base.len()),
                end: e.min(base.len()),
                lines: lines_of(&cb, s, e - s),
                ours_lines: side(&co, &a, ai, s, e),
                theirs_lines: side(&ct, &b, bj, s, e),
            }
        })
        .collect();
    out.sort();
    out
}

/// The trace classes of `write --base` (v0.9.8), read off the sides and the texts.
fn classes(base: &str, ours: &str, theirs: &str, clean: bool, rules: &mut Rules) {
    let (a, b) = (sides(base, ours), sides(base, theirs));
    let cb = closed(base);
    let is_open = |t: &str| !t.is_empty() && !t.ends_with('\n');
    let line_start = |p: usize| p == 0 || cb.as_bytes()[p - 1] == b'\n';
    // one whole base line
    let one_line = |e: &Edit| {
        e.start < e.end
            && line_start(e.start)
            && line_start(e.end)
            && !cb[e.start..e.end - 1].contains('\n')
    };
    let writes = |e: &Edit| e.insert.split_inclusive('\n').count();
    let meet = |e: &Edit, f: &Edit| e.start < f.end && f.start < e.end;
    rules.words_same_line = !clean
        && a.iter().any(|x| {
            one_line(x)
                && writes(x) == 1
                && b.iter()
                    .any(|y| x != y && (x.start, x.end) == (y.start, y.end) && writes(y) == 1)
        });
    rules.multiline_rewrite = a
        .iter()
        .any(|x| one_line(x) && writes(x) >= 2 && b.iter().any(|y| x != y && meet(x, y)))
        || b.iter()
            .any(|y| one_line(y) && writes(y) >= 2 && a.iter().any(|x| x != y && meet(x, y)));
    if !clean {
        return;
    }
    if base.ends_with('\n') {
        let landing = |e: &Edit| e.start == cb.len() && !e.insert.is_empty();
        let ours_lands = a.last().is_some_and(|x| landing(x) && !b.contains(x));
        let theirs_lands = b.last().is_some_and(landing);
        rules.end_break = match (ours_lands, theirs_lands) {
            (true, false) if is_open(theirs) => Some(EndBreak::A),
            (true, true) if is_open(theirs) && !is_open(ours) => Some(EndBreak::B),
            (true, true) if is_open(theirs) => Some(EndBreak::D),
            (false, true) if is_open(ours) => Some(EndBreak::C),
            _ => None,
        };
    }
    // two adjacent lines with the same first character, a character diff one line off
    let shared = cb
        .split_inclusive('\n')
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0].chars().next() == w[1].chars().next());
    let chars =
        |t: &str| -> Vec<Edit> { diff::hunks(&cb, t).into_iter().map(Edit::from).collect() };
    let off = |h: &[Edit]| {
        h.iter().any(|e| {
            !line_start(e.start) && (cb[e.start..e.end].contains('\n') || e.insert.contains('\n'))
        })
    };
    let (a0, b0) = (chars(&closed(ours)), chars(&closed(theirs)));
    if shared && (off(&a0) || off(&b0)) {
        rules.shared_start = true;
        let (ap, bp) = (vec![Vec::new(); a0.len()], vec![Vec::new(); b0.len()]);
        // a clean write only (the conflicting ones returned above)
        rules.shared_start_was_conflict = matches!(
            merge_edits(&cb, &a0, &ap, &b0, &bp, !cb.is_empty(), Same::Hunk).0,
            Merged::Conflict
        );
    }
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

    /// Trace seed 1001280 (job f77bab22), step 53: the base read, the writer's text, the live
    /// text.
    const SEED_1001280: [&str; 3] = [
        " aaa日 b日a日aé\na日 a日é\nb日aa\na\nabba\n bab日b日\nalps p10\n",
        " aaa日 b日a日aé\na日 a日é\nb日aa\na\nabba\n bab日\naalps p10\n",
        " a日日 b日a日aé\na日\nb日 a日bé\nb日aa\na\naabba\né日bab\na a日\nalph日 日éa a\nalps\nal alpha\n日b\nb日éq\nao\na\n",
    ];

    fn seed_region(ours_lines: (usize, usize), theirs_lines: (usize, usize)) -> WriteRegion {
        WriteRegion {
            root: 0,
            start: 41,
            end: 67,
            lines: (5, 7),
            ours_lines,
            theirs_lines,
        }
    }

    /// A side's lines of a region are where the region's text stands in that side's whole
    /// text, every one of its hunks inside the region applied (O6, v0.11.1): a hunk inside the
    /// region that clashes with nothing still moves the region's last line. Seed 1001280:
    /// theirs inserts two lines at 58, where ours' two hunks meet (no clash: an insertion at
    /// a hunk's edge), inside the region [41, 67) the clashes (46..58 with 41..58, 58..67 with
    /// 58..67) join into; the region is live lines 6 to 15, not 6 to 11 (the region's own
    /// hunks' growth only).
    #[test]
    fn write_base_region_lines_count_a_non_clashing_hunk_inside() {
        let [base, ours, live] = SEED_1001280;
        assert_eq!(
            write_base_regions(0, base, ours, live),
            vec![seed_region((5, 7), (6, 15))]
        );
    }

    /// [`write_base_region_lines_count_a_non_clashing_hunk_inside`] swapped: the hunk that
    /// clashes with nothing is ours', and ours' lines count it.
    #[test]
    fn write_base_region_lines_count_a_non_clashing_hunk_of_ours_inside() {
        let [base, ours, live] = SEED_1001280;
        assert_eq!(
            write_base_regions(0, base, live, ours),
            vec![seed_region((6, 15), (5, 7))]
        );
    }

    /// O6 (v0.11.1): conflicting hunks whose ranges overlap or touch are one region, its
    /// range and lines the union; regions with a base line between them stay apart.
    #[test]
    fn write_base_regions_join_touching() {
        let region = |start, end, lines, ours_lines, theirs_lines| WriteRegion {
            root: 0,
            start,
            end,
            lines,
            ours_lines,
            theirs_lines,
        };
        // both sides rewrite lines 2 and 3, each line its own hunk (the whole-line diff pairs
        // line to line): two chains, lines 2 and 3, that touch: one region
        assert_eq!(
            write_base_regions(0, "a\nb\nc\nd\ne\n", "a\nQ\nR\nd\ne\n", "a\nY\nZ\nd\ne\n"),
            vec![region(2, 6, (2, 3), (2, 3), (2, 3))]
        );
        // ours rewrites lines 1-2 and 4-5, theirs 2-3 and 5: line by line, the clashes are
        // lines 2 and 5, with lines 3 and 4 (one side each) between: two regions
        assert_eq!(
            write_base_regions(0, "a\nb\nc\nd\ne\n", "P\nQ\nc\nR\nS\n", "a\nY\nZ\nd\nW\n"),
            vec![
                region(2, 4, (2, 2), (2, 2), (2, 2)),
                region(8, 10, (5, 5), (5, 5), (5, 5))
            ]
        );
        // lines 1 and 5 on both sides: two regions
        assert_eq!(
            write_base_regions(0, "a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n", "X\nb\nc\nd\nZ\n"),
            vec![
                region(0, 2, (1, 1), (1, 1), (1, 1)),
                region(8, 10, (5, 5), (5, 5), (5, 5))
            ]
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
