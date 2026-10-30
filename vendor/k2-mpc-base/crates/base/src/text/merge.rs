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
//! Overlap rules (undo spec O1–O5), on half-open byte ranges of `base`:
//! - O1: two non-empty ranges that intersect conflict;
//! - O2: an insertion strictly inside the other side's range conflicts;
//! - O3: touching at a boundary does not conflict: the insertion lands on that side of it;
//! - O4: an identical change on both sides is taken once;
//! - O5: two different insertions at the same point conflict, except at the end of a
//!   **non-empty** text, where both land, theirs first, then ours (two agents appending to
//!   a log). Two writes to an empty doc conflict (K-BR-11, spec v1.3 R3).
//!
//! O5 makes the merge asymmetric in one place: with both sides appending, swapping ours
//! and theirs swaps the order of the two appends. Everything else is symmetric.
//!
//! "Identical" is hunk equality, which is sound because [`diff::hunks`] is a pure function
//! (counted work bounds, no clock) and canonical (touching hunks are coalesced): the same
//! change from `base` gives the same hunks on both sides.

use std::ops::Range;

use super::diff::{self, Hunk};
use super::lines;

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

/// Merge whole texts: the end of `base` is the end of the text, so O5 applies there when
/// `base` is non-empty (two writes to an empty doc conflict).
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge {
    merge3_window(base, ours, theirs, !base.is_empty())
}

/// Merge a window of a longer text (undo merges one region of the doc). `at_end` says
/// whether the window ends where the whole text ends **and** the whole text was non-empty
/// before the merge (the caller folds that in; the window's own base may be empty): only
/// then do same-point insertions at the window's end both land (O5); elsewhere they
/// conflict as anywhere else.
pub fn merge3_window(base: &str, ours: &str, theirs: &str, at_end: bool) -> Merge {
    let a = diff::hunks(base, ours);
    let b = diff::hunks(base, theirs);
    merge_sets(base, &a, &b, theirs, at_end, |i, j| a[i] == b[j])
}

/// Merge an outside save onto the live text (disk sync, v0.9.5): `base` is the text the file
/// held at the last sync, `ours` the file's new text, `theirs` the live text. The two sides
/// are [`outside_sides`]: each a diff from `base` with its pure line insertions slid to line
/// starts, as an outside edit's always were, and its pure line deletions to whole lines
/// ([`super::lines`]), so a line added on one side and an end-of-line edit or a deleted
/// neighbour on the other don't meet inside each other. Never refuses:
/// - when [`merge3_hunks`] on those sides is clean, its result (the pair rules O1–O5 of
///   `write --base`: the save is applied as a `write --base` of the file's text made on the
///   synced text), except as below (outside saves only; `write --base` is unchanged);
/// - where the two sides overlap, both are kept, as the CRDT merge of an outside edit kept
///   them before v0.9.5, and **at line granularity**: each hunk in an overlap is widened to
///   whole lines ([`outside_merge_sides`]), then every line either side deleted is gone and
///   every line either side wrote lands (at one point: pure insertions before
///   replacements, the live side first within each). Two sides' lines are never spliced
///   into one: where a side's group doesn't end with "\n" (the end of a text with no final
///   newline) and the other side's follows at that point, one "\n" separates them, always
///   (a group starting with "\n" keeps its own blank line). A change made identically on
///   both sides is taken once (O4).
/// - In an outside save, a line break separates text one side left without a final line
///   break (when the original text ended with one) from a change of the other side that is
///   inserted at the end of the text ([`end_line_break`]). Everything else on the last line
///   (a change that starts earlier and runs to the end, or a base with no final break)
///   merges character by character, as merge3_hunks does.
///
/// The hunks are on `theirs`, so they apply to the live doc as they are.
pub fn merge3_keep_both(base: &str, ours: &str, theirs: &str) -> Clean {
    let (a, b, _) = outside_merge_sides(base, ours, theirs);
    // keep_both on the final sides: merge3_hunks' clean result when they don't overlap (a
    // property holds it), and two sides' lines kept apart where groups of whole lines meet
    let hunks = end_line_break(base, ours, theirs, &a, &b, keep_both(base, &a, &b));
    debug_assert!(
        diff::validate(theirs, &hunks).is_ok(),
        "keep-both hunks must be valid on theirs"
    );
    let text = diff::apply(theirs, &hunks);
    Clean { hunks, text }
}

/// The two sides [`merge3_keep_both`] merges: `base` to `ours` and `base` to `theirs`, each
/// a canonical diff with its pure line insertions slid to line starts and its pure line
/// deletions to whole lines (same texts; [`lines::align_deletions`]).
pub fn outside_sides(base: &str, ours: &str, theirs: &str) -> (Vec<Hunk>, Vec<Hunk>) {
    let side = |new: &str| {
        lines::align_deletions(
            base,
            lines::align_insertions(base, diff::line_hunks(base, new)),
        )
    };
    (side(ours), side(theirs))
}

/// The sides [`merge3_keep_both`] ends with, and `merge3_hunks`'s clean result on them when
/// there is one. From [`outside_sides`]: while [`merge3_hunks`] finds overlaps, every hunk
/// of either side that meets an overlapping region is widened to whole lines (same texts;
/// hunks that come to meet are joined), until it is clean or nothing more widens. A clean
/// first merge is returned as it is (character granularity).
pub fn outside_merge_sides(
    base: &str,
    ours: &str,
    theirs: &str,
) -> (Vec<Hunk>, Vec<Hunk>, Option<Clean>) {
    let (mut a, mut b) = outside_sides(base, ours, theirs);
    loop {
        let regions = match merge3_hunks(base, &a, &b, !base.is_empty()) {
            Merge::Clean(c) => return (a, b, Some(c)),
            Merge::Conflicts(c) => c.into_iter().map(|c| c.base).collect::<Vec<_>>(),
        };
        let meets = |h: &Hunk| regions.iter().any(|r| h.start <= r.end && r.start <= h.end);
        let (a2, b2) = (widen(base, &a, &meets), widen(base, &b, &meets));
        if a2 == a && b2 == b {
            return (a, b, None);
        }
        (a, b) = (a2, b2);
    }
}

/// The end-of-text rule of an outside save (v0.9.5): when the change that lands last at
/// the end of the text (one side's insertion at `base`'s end) follows text that
/// doesn't end with "\n" in the merge, though that side alone left it ending with "\n" (or
/// empty), the other side's change opened it (an insert without a final "\n", or a delete
/// that ran to the end): one "\n" goes before the landing text. `hunks` are keep_both's, on
/// `theirs`; the result is too.
fn end_line_break(
    base: &str,
    ours: &str,
    theirs: &str,
    a: &[Hunk],
    b: &[Hunk],
    mut hunks: Vec<Hunk>,
) -> Vec<Hunk> {
    // only where the base ended with a line break: a side then removed it, or didn't put it
    // back; with none, the last line merges character by character (v0.9.5)
    if !base.ends_with('\n') {
        return hunks;
    }
    let len = base.len();
    let text = diff::apply(theirs, &hunks);
    let open = |p: &str| !p.is_empty() && !p.ends_with('\n');
    let closed = |p: &str| p.is_empty() || p.ends_with('\n');
    let ours_end = a
        .iter()
        .rev()
        .find(|x| !b.contains(x))
        .filter(|x| x.start == len && !x.insert.is_empty());
    let theirs_end = b.last().filter(|y| y.start == len && !y.insert.is_empty());
    // ours lands last: keep_both put its text at the very end of theirs
    if let Some(x) = ours_end {
        if let Some(h) = hunks.last_mut() {
            if h.end == theirs.len() && h.insert.ends_with(&x.insert) {
                let prefix = &text[..text.len() - x.insert.len()];
                let alone = &ours[..ours.len() - x.insert.len()];
                if open(prefix) && closed(alone) {
                    let k = h.insert.len() - x.insert.len();
                    h.insert.insert(k, '\n');
                }
                return hunks;
            }
        }
    }
    // theirs lands last: its text ends theirs, and nothing of ours follows it
    if let Some(y) = theirs_end {
        let q = theirs.len() - y.insert.len();
        if text.ends_with(&y.insert) && hunks.last().map_or(true, |h| h.end <= q) {
            let prefix = &text[..text.len() - y.insert.len()];
            if open(prefix) && closed(&theirs[..q]) {
                match hunks.last_mut() {
                    Some(h) if h.end == q => h.insert.push('\n'),
                    _ => hunks.push(Hunk {
                        start: q,
                        end: q,
                        insert: "\n".to_owned(),
                    }),
                }
            }
        }
    }
    hunks
}

/// Whether `h` changes whole lines of `base`: it starts at a line start, ends at one (or at
/// the end of the text), and what it writes is whole lines (or nothing).
fn whole_lines(base: &str, h: &Hunk) -> bool {
    let at_line = |p: usize| p == 0 || p == base.len() || base.as_bytes()[p - 1] == b'\n';
    (h.start == 0 || base.as_bytes()[h.start - 1] == b'\n')
        && at_line(h.end)
        && (h.insert.is_empty() || h.insert.ends_with('\n') || h.end == base.len())
}

/// `hunks` (canonical, on `base`) with each one `widen` picks, and isn't whole lines already,
/// grown to whole lines; hunks that come to overlap or touch are joined. Same text.
fn widen(base: &str, hunks: &[Hunk], pick: &dyn Fn(&Hunk) -> bool) -> Vec<Hunk> {
    // each hunk's range after widening
    let ranges: Vec<(usize, usize)> = hunks
        .iter()
        .map(|h| {
            if !pick(h) || whole_lines(base, h) {
                return (h.start, h.end);
            }
            let s = base[..h.start].rfind('\n').map_or(0, |k| k + 1);
            let mut e = h.end;
            let mut tail = format!("{}{}", &base[s..h.start], h.insert);
            loop {
                let boundary = e == base.len() || (e > 0 && base.as_bytes()[e - 1] == b'\n');
                if boundary && (tail.is_empty() || tail.ends_with('\n') || e == base.len()) {
                    break;
                }
                let next = base[e..].find('\n').map_or(base.len(), |k| e + k + 1);
                tail.push_str(&base[e..next]);
                e = next;
            }
            (s, e)
        })
        .collect();
    // join hunks whose ranges overlap or touch; each group's text from its own hunks
    let mut out: Vec<Hunk> = Vec::new();
    let mut k = 0;
    while k < hunks.len() {
        let (s, mut e) = ranges[k];
        let first = k;
        k += 1;
        while k < hunks.len() && ranges[k].0 <= e {
            e = e.max(ranges[k].1);
            k += 1;
        }
        let local: Vec<Hunk> = hunks[first..k]
            .iter()
            .map(|h| Hunk {
                start: h.start - s,
                end: h.end - s,
                insert: h.insert.clone(),
            })
            .collect();
        let insert = diff::apply(&base[s..e], &local);
        if insert != base[s..e] {
            out.push(Hunk {
                start: s,
                end: e,
                insert,
            });
        }
    }
    out
}

/// The keep-both merge: `a` (ours) and `b` (theirs) canonical hunks on one base; returns
/// hunks on `theirs` (the base with `b` applied) that make the union: every base character
/// either side deletes is deleted, every insertion of either side kept, each where its hunk
/// starts. At one point, as [`merge_sets`] places them: pure insertions before replacements
/// (O3: an insertion at the start of the other side's range lands before it), and theirs
/// before ours within each (O5 at the end of a text). Equal hunks are taken once (O4). On
/// hunk sets that don't overlap this is [`merge_sets`]'s clean result.
fn keep_both(base: &str, a: &[Hunk], b: &[Hunk]) -> Vec<Hunk> {
    // pre[k]: the length change of b[..k]
    let mut pre = Vec::with_capacity(b.len() + 1);
    pre.push(0isize);
    for y in b {
        let last = *pre.last().expect("non-empty");
        pre.push(last + y.insert.len() as isize - (y.end - y.start) as isize);
    }
    // where base offset `p` is in theirs: after every theirs hunk ending at or before it
    // (a pure insertion at `p` too); when `p` is inside theirs' range, or both replace from
    // `p` (`replaces`), after theirs' text for that range
    let at = |p: usize, replaces: bool| -> usize {
        let k = b.partition_point(|y| y.end <= p);
        let mut shift = pre[k];
        if let Some(y) = b.get(k) {
            if y.start < p || (replaces && y.start == p) {
                shift += y.insert.len() as isize - (p - y.start) as isize;
            }
        }
        (p as isize + shift) as usize
    };
    let mut out = Vec::new();
    // where the last ours group was put: a later one at the same point follows ours' own
    // lines, not theirs' text
    let mut last_put: Option<usize> = None;
    for x in a {
        if b.iter().any(|y| y == x) {
            continue; // O4: theirs holds it already
        }
        if !x.insert.is_empty() {
            let p = at(x.start, !x.is_insertion());
            let mut insert = x.insert.clone();
            // theirs' text just before it at the same point (as `at` placed it): two kept
            // groups of whole lines meet there; when theirs' doesn't end with "\n" (the end of
            // a text with no final newline), a "\n" keeps the two sides' lines apart, always
            // (when this group starts with "\n", that one is its own blank line)
            // (v0.9.5)
            let k = b.partition_point(|y| y.end <= x.start);
            let before = b
                .get(k)
                .filter(|y| y.start < x.start || (!x.is_insertion() && y.start == x.start))
                .or_else(|| k.checked_sub(1).map(|j| &b[j]).filter(|y| y.end == x.start));
            if let Some(y) = before.filter(|_| last_put != Some(p)) {
                if whole_lines(base, x)
                    && whole_lines(base, y)
                    && !y.insert.is_empty()
                    && !y.insert.ends_with('\n')
                {
                    insert.insert(0, '\n');
                }
            }
            // theirs' text just after it (every base character between deleted by x): the
            // same, the other way round
            // (the first one that writes: one that only deletes leaves nothing between)
            let after = b[k..]
                .iter()
                .find(|y| {
                    !y.insert.is_empty()
                        && (y.start > x.start
                            || (x.is_insertion() && y.start == x.start && !y.is_insertion()))
                })
                .filter(|y| y.start <= x.end);
            if let Some(y) = after {
                if whole_lines(base, x) && whole_lines(base, y) && !insert.ends_with('\n') {
                    insert.push('\n');
                }
            }
            last_put = Some(p);
            out.push(Hunk {
                start: p,
                end: p,
                insert,
            });
        }
        // the base characters x deletes that theirs still holds, split at theirs' insertions
        let mut u = x.start;
        let first = b.partition_point(|y| y.end < x.start);
        for y in b[first..].iter().take_while(|y| y.start < x.end) {
            let inside = y.end > x.start || (y.is_insertion() && y.start > x.start);
            if !inside {
                continue;
            }
            if y.start > u {
                let s = at(u, false);
                out.push(Hunk {
                    start: s,
                    end: s + (y.start - u),
                    insert: String::new(),
                });
            }
            u = u.max(y.end);
        }
        if u < x.end {
            let s = at(u, false);
            out.push(Hunk {
                start: s,
                end: s + (x.end - u),
                insert: String::new(),
            });
        }
    }
    diff::coalesce(out)
}

/// Merge two **exact hunk sets** on `base` (undo spec v1.7, U10): `ours` and `theirs` are
/// canonical hunks on `base` (sorted, non-overlapping, never empty, joined when touching),
/// not found by diffing. Undo, redo and revert merge this way: ours is the inverse of the
/// change's own hunks, theirs the later hunks that touched the region, both at element
/// identity. A diff can't say which of two identical elements changed; these sets do, so
/// the clean result's hunks name exactly the elements ours changes. `at_end` as in
/// [`merge3_window`]. The pair rules (O1–O5) and the placement are the same.
pub fn merge3_hunks(base: &str, ours: &[Hunk], theirs: &[Hunk], at_end: bool) -> Merge {
    merge3_hunks_by(base, ours, theirs, at_end, |i, j| ours[i] == theirs[j])
}

/// [`merge3_hunks`] with O4's identity given by the caller: `same(i, j)` says whether ours'
/// hunk `i` and theirs' hunk `j` are **the same change** (taken once). `merge3_hunks` is
/// this with hunk equality. Undo, redo and revert (undo spec v1.8, O4) judge identity on
/// **(position, root)**: the same range of the base and inserted elements with the same
/// provenance roots in the same order, so a fresh `X` someone typed where an undo would
/// put back `X` is not that `X`. `same` is asked only of hunks with the same range or a
/// clash between them (it must imply equal ranges and equal inserted text).
pub fn merge3_hunks_by(
    base: &str,
    ours: &[Hunk],
    theirs: &[Hunk],
    at_end: bool,
    same: impl Fn(usize, usize) -> bool,
) -> Merge {
    debug_assert!(diff::validate(base, ours).is_ok() && diff::validate(base, theirs).is_ok());
    let t = diff::apply(base, theirs);
    merge_sets(base, ours, theirs, &t, at_end, same)
}

/// The merge proper: `a` (ours) and `b` (theirs) are canonical hunks on `base`, and
/// `theirs` is `b` applied to `base`; `same(i, j)` is O4's identity of `a[i]` and `b[j]`.
fn merge_sets(
    base: &str,
    a: &[Hunk],
    b: &[Hunk],
    theirs: &str,
    at_end: bool,
    same: impl Fn(usize, usize) -> bool,
) -> Merge {
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
            let eof_appends =
                at_end && x.is_insertion() && y.is_insertion() && x.start == base.len();
            if x.start == y.start && x.end == y.end && same(i, j) {
                debug_assert_eq!(x, y, "O4's identity implies equal hunks");
                keep[i] = false;
            } else if !eof_appends && overlaps(x, y) {
                clash.push((i, j));
            }
            j += 1;
        }
    }
    if !clash.is_empty() {
        return Merge::Conflicts(conflicts(base, a, b, &clash));
    }

    // Pass 2: rebase our hunks into theirs' coordinates.
    let mut out = Vec::with_capacity(a.len());
    let mut shift: isize = 0;
    let mut j = 0;
    for (i, x) in a.iter().enumerate() {
        // Every theirs-hunk ending at or before our start shifts us. A theirs-insertion at
        // our start therefore lands before our change; one at our end lands after it. This
        // also puts theirs first for two appends at the end of the text (O5).
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
        // same point, different text (identical, and appends at the end, handled by caller)
        (true, true) => x.start == y.start,
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
        assert!(matches!(merge3("ab", "Xab", "Yab"), Merge::Conflicts(_)));
        assert_eq!(clean("ab", "aXb", "aXb"), "aXb");
        // at the end of the text the two land instead (O5)
        assert_eq!(clean("ab", "abX", "abY"), "abYX");
        // ... but not at the end of a window that isn't the end of the text
        assert!(matches!(
            merge3_window("ab", "abX", "abY", false),
            Merge::Conflicts(_)
        ));
    }

    /// The keep-both rule character by character (the model of [`merge3_keep_both`]'s
    /// overlap): at each base offset, theirs' insertions, then ours', then the base character
    /// unless either side deletes it; two sides' groups of whole lines that meet with no base
    /// character between get a "\n" between them whenever the first lacks one. Equal hunks
    /// are one change (O4).
    fn keep_both_model(base: &str, ours: &str, theirs: &str) -> String {
        let (a, b, _) = outside_merge_sides(base, ours, theirs);
        let a: Vec<Hunk> = a.into_iter().filter(|x| !b.contains(x)).collect();
        let deletes = |h: &Hunk, p: usize| h.start <= p && p < h.end;
        let mut out = String::new();
        // the last group (its side), with no base character after it yet: two sides' groups
        // of whole lines meeting there are kept apart by a "\n" when the first lacks one
        let mut last: Option<(usize, &Hunk)> = None;
        let mut at: Vec<(usize, Option<char>)> =
            base.char_indices().map(|(i, c)| (i, Some(c))).collect();
        at.push((base.len(), None));
        for (p, c) in at {
            // pure insertions, then replacements; theirs first within each
            let mut here: Vec<(bool, usize, &Hunk)> = b
                .iter()
                .map(|h| (0, h))
                .chain(a.iter().map(|h| (1, h)))
                .filter(|(_, h)| h.start == p)
                .map(|(side, h)| (!h.is_insertion(), side, h))
                .collect();
            here.sort_by_key(|(r, side, _)| (*r, *side));
            for (_, side, h) in here {
                if h.insert.is_empty() {
                    continue;
                }
                if let Some((ps, prev)) = last {
                    if ps != side
                        && whole_lines(base, prev)
                        && whole_lines(base, h)
                        && !prev.insert.ends_with('\n')
                    {
                        out.push('\n');
                    }
                }
                out.push_str(&h.insert);
                last = Some((side, h));
            }
            if let Some(c) = c {
                if !b.iter().chain(&a).any(|h| deletes(h, p)) {
                    out.push(c);
                    last = None;
                }
            }
        }
        // the end rule (the base ended with a line break): the last group is inserted at the
        // end, after text its side alone left closed but the merge left open
        if let Some((side, h)) = last.filter(|(_, h)| base.ends_with('\n') && h.start == base.len())
        {
            let alone = if side == 1 { ours } else { theirs };
            let k = out.len() - h.insert.len();
            let closed = |p: &str| p.is_empty() || p.ends_with('\n');
            if !closed(&out[..k]) && closed(&alone[..alone.len() - h.insert.len()]) {
                out.insert(k, '\n');
            }
        }
        out
    }

    /// An outside save that overlaps the live text's unwritten change keeps both, as the
    /// CRDT merge did before v0.9.5 (the live text first); a clean one is `write --base`'s.
    #[test]
    fn an_overlapping_outside_save_keeps_both_live_first() {
        let base = "line 3\n";
        let c = merge3_keep_both(base, "ZZZ\n", "AAA\n");
        assert_eq!(c.text, "AAA\nZZZ\n", "whole lines, the live one first");
        assert_eq!(c.text, keep_both_model(base, "ZZZ\n", "AAA\n"));
        assert_eq!(diff::apply("AAA\n", &c.hunks), c.text);
        // the live side deleted the line; the file changed it: the file's text comes back
        // (what either side deleted is gone)
        let c = merge3_keep_both("a\nline 3\nb\n", "a\nLINE 3\nb\n", "a\nb\n");
        assert_eq!(
            c.text,
            keep_both_model("a\nline 3\nb\n", "a\nLINE 3\nb\n", "a\nb\n")
        );
        assert!(c.text.contains("LINE"), "{:?}", c.text);
        // overlapping in one line: both lines kept whole, the live one first
        assert_eq!(
            merge3_keep_both("abcd\n", "aXd\n", "abYcd\n").text,
            "abYcd\naXd\n"
        );
        // the same change on both sides, beside an overlap: taken once
        let c = merge3_keep_both("x\nab\n", "y\nP\n", "y\nQ\n");
        assert_eq!(c.text, "y\nQ\nP\n");
        assert_eq!(c.text, keep_both_model("x\nab\n", "y\nP\n", "y\nQ\n"));
        // clean: merge3's result
        assert_eq!(
            merge3_keep_both("a\nb\n", "a\nB\n", "A\nb\n").text,
            clean("a\nb\n", "a\nB\n", "A\nb\n")
        );
    }

    /// Two sides' groups meeting at one point at the end of a text with no final newline keep
    /// their own lines (v0.9.5): the live group gets a "\n" before the file's.
    #[test]
    fn groups_at_the_end_of_a_text_without_a_final_newline_keep_their_lines() {
        for (base, file, live, want) in [
            ("a\nb", "a\nB1", "a\nB2", "a\nB2\nB1"),
            ("x", "y", "z", "z\ny"),
            ("a\nb\n", "a\nB1", "a\nB2", "a\nB2\nB1"),
            ("a\nb", "a\nB1\n", "a\nB2", "a\nB2\nB1\n"),
            // two appends (O5, a clean merge3), and a final newline already there
            ("a\n", "a\nY", "a\nX", "a\nX\nY"),
            ("a\nb", "a\nB1\n", "a\nB2\n", "a\nB2\nB1\n"),
            // a group starting with "\n" keeps its blank line (the fuzz worker's cases)
            ("", "\n\n", "aaaaa", "aaaaa\n\n\n"),
            ("", "\n\u{b}\u{1d}", " \u{4}", " \u{4}\n\n\u{b}\u{1d}"),
            // the clean path (outside saves only): an open last line, then the file's line
            ("b\n", "b\nZ\n", "a", "a\nZ\n"),
            // the end rule: one side's change opens the end (a delete to the end), the other's
            // lands there (fdc799da)
            ("\n", "x\ny", "", "x\ny"),
            ("a\n\n", "a\nx\ny", "a\n", "a\nx\ny"),
            // one side removes the final newline, the other appends a line: separate lines
            ("a\n", "a", "a\ng", "a\ng"),
            ("a\n", "a\ng", "a", "a\ng"),
            // an append to an open last line by one side alone stays on that line
            ("a", "ay", "a", "ay"),
            // no final break in the base: the last line merges character by character
            // (5cb28e51)
            ("b", "xb", "c", "xc"),
            ("b", "a\nxb", "c", "a\nxc"),
            ("b\n", "xb\n", "c\n", "xc\n"),
            // only an insertion at the end gets the break; a change that starts earlier and
            // runs to the end merges character by character (e48dc26f), with the line empty
            // or not, either side
            ("\n", "b\n", "aaa", "baaa"),
            ("foo\n", "Xfoo\n", "foo bar", "Xfoo bar"),
            ("foo\n", "foo bar", "Xfoo\n", "Xfoo bar"),
        ] {
            let c = merge3_keep_both(base, file, live);
            assert_eq!(c.text, want, "{base:?} {file:?} {live:?}");
            assert_eq!(diff::apply(live, &c.hunks), c.text);
            assert_eq!(c.text, keep_both_model(base, file, live));
        }
        // inside a line, a clean merge stays character-level
        assert_eq!(
            merge3_keep_both("abcde\n", "aBcde\n", "abcdeQ\n").text,
            "aBcdeQ\n"
        );
    }

    /// O4 by a caller's identity (undo spec v1.8: position and root): equal text that isn't
    /// the same change is not taken once; O1–O5 decide instead. `merge3_hunks` is the
    /// variant with hunk equality.
    #[test]
    fn identity_predicate_decides_o4() {
        let h = |s: usize, e: usize, t: &str| Hunk {
            start: s,
            end: e,
            insert: t.into(),
        };
        let ours = [h(1, 1, "X")];
        let theirs = [h(1, 1, "X")];
        // the same change: taken once
        let Merge::Clean(c) = merge3_hunks_by("ab", &ours, &theirs, false, |_, _| true) else {
            panic!()
        };
        assert_eq!((c.text.as_str(), c.hunks.len()), ("aXb", 0));
        assert_eq!(
            merge3_hunks("ab", &ours, &theirs, false),
            Merge::Clean(c.clone())
        );
        // the same text, another change: two insertions at one point conflict (O5) ...
        assert!(matches!(
            merge3_hunks_by("ab", &ours, &theirs, false, |_, _| false),
            Merge::Conflicts(_)
        ));
        // ... except at the end of a non-empty doc, where both land, theirs first
        let (o, t) = ([h(2, 2, "X")], [h(2, 2, "X")]);
        let Merge::Clean(c) = merge3_hunks_by("ab", &o, &t, true, |_, _| false) else {
            panic!()
        };
        assert_eq!(c.text, "abXX");
        assert_eq!(c.hunks, vec![h(3, 3, "X")]);
        // identical deletions are one change whatever the predicate says of insertions
        let d = [h(0, 1, "")];
        let Merge::Clean(c) = merge3_hunks_by("ab", &d, &d, false, |i, j| d[i] == d[j]) else {
            panic!()
        };
        assert_eq!(c.text, "b");
        // a replacement of the same range with the same text, but not the same change
        let r = [h(0, 1, "Z")];
        assert!(matches!(
            merge3_hunks_by("ab", &r, &r, false, |_, _| false),
            Merge::Conflicts(_)
        ));
    }

    #[test]
    fn eof_appends_both_land() {
        // two agents append to a log from the same base: theirs (already live) first
        let base = "# Log\n- one\n";
        assert_eq!(
            clean(base, "# Log\n- one\n- ours\n", "# Log\n- one\n- theirs\n"),
            "# Log\n- one\n- theirs\n- ours\n"
        );
        // an append repeating the last line is still an append (the diff trims the prefix first)
        assert_eq!(
            clean(base, "# Log\n- one\n- one\n", "# Log\n- one\n- two\n"),
            "# Log\n- one\n- two\n- one\n"
        );
        // an identical append taken once (O4 before O5)
        assert_eq!(
            clean(base, "# Log\n- one\nx\n", "# Log\n- one\nx\n"),
            "# Log\n- one\nx\n"
        );
        // appends land with other, disjoint edits on both sides
        assert_eq!(
            clean(base, "# LOG\n- one\n- ours\n", "# Log\n- ONE\n- theirs\n"),
            "# LOG\n- ONE\n- theirs\n- ours\n"
        );
        // a replace ending at the end still conflicts with a different one (O1)
        assert!(matches!(
            merge3(base, "# Log\n- uno\n", "# Log\n- eins\n"),
            Merge::Conflicts(_)
        ));
    }

    #[test]
    fn empty_base_double_write_conflicts() {
        // K-BR-11 / CT merge.empty_base_same_point_conflicts: O5 needs a non-empty doc
        assert!(matches!(merge3("", "b\n", "a\n"), Merge::Conflicts(_)));
        // the same text written twice is still taken once (O4)
        assert_eq!(clean("", "a\n", "a\n"), "a\n");
        // a window may have an empty base and still be at the end of a non-empty doc
        assert!(matches!(
            merge3_window("", "x\n", "y\n", true),
            Merge::Clean(_)
        ));
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

    fn assert_identities(base: &str, other: &str) {
        assert_eq!(clean(base, base, other), other, "merge3(base, base, other)");
        assert_eq!(clean(base, other, base), other, "merge3(base, other, base)");
        assert_eq!(
            clean(base, other, other),
            other,
            "merge3(base, other, other)"
        );
    }

    #[test]
    fn identities_regression_emoji_lines() {
        assert_identities(
            "日日ab\nab\n日ab\n日ab\n日🎉ab\nbab\n日ab\nab\nab\naab\nab\n日ab\n",
            "🎉🎉ab\nab\n\n🎉日日ab\n🎉ab\nab\n🎉\n日ab\naab\n🎉日日",
        );
    }

    #[test]
    fn identities_regression_ab_lines() {
        assert_identities(
            "ab\nab\nba\nab\nab\nab\nab\n\nab\na🎉ab\nab\nab\nbab\n日\n日ab\n",
            "日ab\n日\n🎉ab\nab\n日🎉aab\n\nab\n🎉b\na",
        );
    }

    /// `PROPTEST_CASES` overrides the default count (`with_cases` alone would ignore it).
    fn cases(default: u32) -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::string::string_regex("([ab日🎉\n]|ab\n){0,24}").unwrap()
    }

    /// A base of up to 6 lines, each one character, all different.
    fn base_lines() -> impl Strategy<Value = Vec<String>> {
        Just(["a", "b", "c", "x", "y", "z"].map(String::from).to_vec())
            .prop_shuffle()
            .prop_flat_map(|v| (Just(v), 0..=6usize))
            .prop_map(|(v, n)| v[..n].to_vec())
    }

    /// Whole-line edits of a base, writing new lines from `alpha`: per base line, keep,
    /// drop, replace (0, 1, 2), and lines inserted before it.
    #[derive(Debug, Clone)]
    struct LineEdits(Vec<(u8, Vec<String>, String)>, Vec<String>);

    fn line_edits(alpha: &'static str) -> impl Strategy<Value = LineEdits> {
        let tok = move || proptest::string::string_regex(&format!("[{alpha}]{{1,3}}")).unwrap();
        (
            prop::collection::vec((0u8..3, prop::collection::vec(tok(), 0..2), tok()), 6),
            prop::collection::vec(tok(), 0..2),
        )
            .prop_map(|(per, tail)| LineEdits(per, tail))
    }

    fn apply_line_edits(base: &[String], e: &LineEdits) -> Vec<String> {
        let mut out = Vec::new();
        for (k, line) in base.iter().enumerate() {
            let (op, before, with) = &e.0[k];
            out.extend(before.iter().cloned());
            match op {
                0 => out.push(line.clone()),
                1 => {}
                _ => out.push(with.clone()),
            }
        }
        out.extend(e.1.iter().cloned());
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(cases(3000)))]
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

        /// Symmetric up to O5: when both sides append different text at the end, swapping
        /// the sides swaps the two appends; everything before them is the same.
        #[test]
        fn clean_merges_are_symmetric_up_to_eof_order(base in text(), o in text(), t in text()) {
            if let (Merge::Clean(x), Merge::Clean(y)) = (merge3(&base, &o, &t), merge3(&base, &t, &o)) {
                let eof = |new: &str| {
                    diff::hunks(&base, new)
                        .pop()
                        .filter(|h| h.is_insertion() && h.start == base.len())
                        .map(|h| h.insert)
                };
                match (eof(&o), eof(&t)) {
                    (Some(eo), Some(et)) if eo != et => {
                        let tail = eo.len() + et.len();
                        prop_assert!(x.text.ends_with(&format!("{et}{eo}")), "{:?}", x.text);
                        prop_assert!(y.text.ends_with(&format!("{eo}{et}")), "{:?}", y.text);
                        prop_assert_eq!(&x.text[..x.text.len() - tail], &y.text[..y.text.len() - tail]);
                    }
                    _ => prop_assert_eq!(x.text, y.text),
                }
            }
        }

        /// The keep-both merge (an outside save, v0.9.5) is the character model, always valid
        /// on theirs, and on hunk sets merge3 finds clean it is merge3's own result.
        #[test]
        fn keep_both_is_the_model_and_merge3_when_clean(base in text(), o in text(), t in text()) {
            for (a, b) in [
                (diff::hunks(&base, &o), diff::hunks(&base, &t)),
                outside_sides(&base, &o, &t),
            ] {
                let kb = keep_both(&base, &a, &b);
                prop_assert!(diff::validate(&t, &kb).is_ok());
                if let Merge::Clean(c) = merge3_hunks(&base, &a, &b, !base.is_empty()) {
                    let text = diff::apply(&t, &kb);
                    if kb != c.hunks {
                        // only the "\n" keeping two appends at the end of a text with no
                        // final newline apart
                        prop_assert_eq!(text.replace('\n', ""), c.text.replace('\n', ""));
                        prop_assert_eq!(text.matches('\n').count(), c.text.matches('\n').count() + 1);
                    } else {
                        prop_assert_eq!(text, c.text);
                    }
                }
            }
            let (a, b, clean) = outside_merge_sides(&base, &o, &t);
            for (side, new) in [(&a, &o), (&b, &t)] {
                prop_assert!(diff::validate(&base, side).is_ok(), "{:?}", side);
                prop_assert_eq!(&diff::apply(&base, side), new);
            }
            // the merge (keep_both, then the end rule) is the model
            let m = merge3_keep_both(&base, &o, &t);
            prop_assert_eq!(diff::apply(&t, &m.hunks), m.text.clone());
            prop_assert_eq!(&m.text, &keep_both_model(&base, &o, &t));
            if clean.is_none() {
                // what still overlaps is whole lines on both sides, so no two sides' lines
                // are spliced into one
                for x in &a {
                    for y in &b {
                        let eof = !base.is_empty()
                            && x.is_insertion()
                            && y.is_insertion()
                            && x.start == base.len();
                        if x != y && !eof && overlaps(x, y) {
                            prop_assert!(
                                whole_lines(&base, x) && whole_lines(&base, y),
                                "{:?} and {:?} overlap, not whole lines", x, y
                            );
                        }
                    }
                }
            }
        }

        /// No line of an outside save's merge is spliced from two sides (v0.9.5): with whole-line
        /// edits, every line of the result is a line of the base, the file or the live text.
        /// Texts with and without a final newline (a side may add or remove one). The base's
        /// lines are single characters, all
        /// different, and each side writes from its own alphabet, so a character diff can't
        /// build a side's line out of another line's characters (that is the D1 gap's
        /// family, not a splice by the merge).
        #[test]
        fn no_line_of_the_merge_is_spliced_from_two_sides(
            base in base_lines(),
            fo in line_edits("def"),
            lo in line_edits("ghi"),
            nl in any::<(bool, bool, bool)>(),
        ) {
            let text = |ls: &[String], nl: bool| {
                let mut t = ls.join("\n");
                if nl && !ls.is_empty() {
                    t.push('\n');
                }
                t
            };
            let file_lines = apply_line_edits(&base, &fo);
            let live_lines = apply_line_edits(&base, &lo);
            let (b, f, l) = (text(&base, nl.0), text(&file_lines, nl.1), text(&live_lines, nl.2));
            let got = merge3_keep_both(&b, &f, &l).text;
            let known: std::collections::HashSet<&str> =
                b.lines().chain(f.lines()).chain(l.lines()).collect();
            // an empty line isn't two sides' fragments joined (a final newline added on one
            // side and a line appended on the other give one)
            for line in got.lines().filter(|l| !l.is_empty()) {
                prop_assert!(known.contains(line), "{:?} spliced: B {:?} F {:?} L {:?} -> {:?}", line, b, f, l, got);
            }
        }

        /// O5 only adds clean merges: whenever the merge without it (a window not at the
        /// end) is clean, the whole-text merge is clean with the same text.
        #[test]
        fn eof_rule_only_adds_clean_merges(base in text(), o in text(), t in text()) {
            if let Merge::Clean(w) = merge3_window(&base, &o, &t, false) {
                prop_assert_eq!(clean(&base, &o, &t), w.text);
            }
        }
    }
}
