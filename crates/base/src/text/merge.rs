//! Three-way merge for writes made against an older version.
//!
//! A writer read version `base`, produced `ours`, and meanwhile the live doc moved to
//! `theirs`. [`merge3`] applies only what the writer changed (base -> ours) on top of the
//! live text, and refuses with the overlapping regions when the two sides touched the
//! same lines. There is no last-writer-wins path. Undo, redo and revert merge exact hunk
//! sets with the same pair rules ([`merge3_hunks`]).
//!
//! **Whole lines (v0.9.7.1).** `write --base` merges by line, as an outside save does
//! (diff3 by line): the texts are [`closed`] (a final "\n" added when one has text and lacks
//! it) and each side is [`write_base_sides`], a diff from the closed base at whole lines
//! ([`diff::whole_line_hunks`]): every hunk starts at a line start and covers and writes
//! whole lines. So two changes to one line, or to overlapping lines, always overlap: a
//! conflict, and the write is refused. Nothing is ever merged inside a line, so a clean
//! merge never holds a line no one wrote (v0.1.0 to v0.9.7 diffed the sides to the
//! character: lines sharing their first characters could pair one line off, and two lines
//! could merge "cleanly" into one garbled line). The final "\n" is decided on its own, as
//! the outside save's merge does ([`end_is_open`]). The clean result's hunks are the
//! verb edits' diff from the live text to the merged text ([`lines::edit_hunks`]), so a
//! write names the characters it changed, as any edit given as a new text does.
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
//! "Identical" is hunk equality, which is sound because [`diff::whole_line_hunks`] is a pure
//! function (counted work bounds, no clock): the same change from `base` gives the same hunks
//! on both sides.

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
    /// The region's 1-based line range in ours' text (the writer's; v0.9.7.1), as `lines`
    /// (a region ours deleted gives one line: where it was).
    pub ours_lines: (usize, usize),
    /// The region's 1-based line range in theirs' text (the live text; v0.9.7.1).
    pub theirs_lines: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    Clean(Clean),
    Conflicts(Vec<Conflict>),
}

/// Merge whole texts: the end of `base` is the end of the text, so O5 applies there when
/// `base` is non-empty (two writes to an empty doc conflict). `write --base`'s merge.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge {
    merge3_window(base, ours, theirs, !base.is_empty())
}

/// [`merge3`] with O5's end of the text given: `at_end` says whether the window ends where
/// the whole text ends **and** the whole text was non-empty before the merge (the caller
/// folds that in; the window's own base may be empty): only then do same-point insertions
/// at the window's end both land (O5); elsewhere they conflict as anywhere else.
///
/// **By whole lines (v0.9.7.1):** the sides are [`write_base_sides`] on the [`closed`]
/// texts; the pair rules O1–O5 decide on them; the final "\n" of a clean merge follows the
/// side that wrote its last line ([`end_is_open`]). A conflict's ranges are on `base`, its
/// line numbers in `base`, ours and theirs ([`Conflict`]).
pub fn merge3_window(base: &str, ours: &str, theirs: &str, at_end: bool) -> Merge {
    let (cb, ct) = (closed(base), closed(theirs));
    let (a, b) = write_base_sides(base, ours, theirs);
    match merge_sets(&cb, &a, &b, &ct, at_end, |i, j| a[i] == b[j]) {
        Merge::Conflicts(cs) => Merge::Conflicts(
            cs.into_iter()
                .map(|c| on_open_texts(c, base, ours, theirs))
                .collect(),
        ),
        Merge::Clean(c) => {
            let open = end_is_open(base, ours, theirs, &a, &b, &c.hunks, &c.text);
            let mut text = c.text;
            if open {
                text.pop();
            }
            Merge::Clean(Clean {
                hunks: lines::edit_hunks(theirs, &text),
                text,
            })
        }
    }
}

/// `write --base`'s two sides (undo spec O0, v0.9.7.1): [`closed`] `base` to closed `ours`
/// and to closed `theirs`, each [`diff::whole_line_hunks`] (hunks on the closed base that
/// start at line starts and cover and write whole lines). The same sides as an outside
/// save's ([`outside_sides`]): two changes touching one line always overlap.
pub fn write_base_sides(base: &str, ours: &str, theirs: &str) -> (Vec<Hunk>, Vec<Hunk>) {
    outside_sides(base, ours, theirs)
}

/// A conflict found on the [`closed`] texts, on the texts themselves: a range that reached
/// the closing "\n" of an open base ends at the base's end, and a side's text there loses
/// the "\n" its own text doesn't have. Line numbers don't change (closing adds no line).
fn on_open_texts(mut c: Conflict, base: &str, ours: &str, theirs: &str) -> Conflict {
    if c.base.end > base.len() {
        c.base.end = base.len();
        c.base.start = c.base.start.min(base.len());
        c.base_text = base[c.base.clone()].to_owned();
        for (side, text) in [(&mut c.ours, ours), (&mut c.theirs, theirs)] {
            if is_open(text) && side.ends_with('\n') {
                side.pop();
            }
        }
    }
    c
}

/// Merge an outside save onto the live text (disk sync, v0.9.5): `base` is the text the file
/// held at the last sync, `ours` the file's new text, `theirs` the live text. Never refuses.
///
/// **By line (v0.9.7).** The three texts are merged as whole lines: each is first
/// [`closed`] (a final "\n" added when it has text and lacks one), and the sides are
/// [`outside_sides`], each a diff from the closed base at whole lines
/// ([`diff::whole_line_hunks`]): every hunk starts at a line start and covers and writes
/// whole lines, so two sides' changes never merge inside one line; wherever they touch the
/// same line they overlap.
/// - When [`merge3_hunks`] on those sides is clean, its result (the pair rules O1–O5 of
///   `write --base`: the save is applied as a `write --base` of the file's text made on the
///   synced text).
/// - Where the two sides overlap, both are kept, as the CRDT merge of an outside edit kept
///   them before v0.9.5, line by line: every line either side deleted is gone and every line
///   either side wrote lands (at one point: pure insertions before replacements, the live
///   side first within each). A change made identically on both sides is taken once (O4).
///
/// **The end of the text** is merged on its own ([`end_is_open`]): whether the result ends
/// without a final "\n" follows the side that wrote its last line (as that side's text
/// ends); when that line is a base line both kept, or both wrote it, the side that changed
/// the base's ending decides (ours when it did, else theirs). So a line one side deleted
/// never comes back because the other only added or removed a final "\n" after it, and a
/// line written after an open last line is never glued to it. This agrees with `write
/// --base`'s end-of-text rule (O5a, v0.9.8) wherever both apply (a clean merge on a base
/// ending with "\n").
///
/// So at most one person's changes are on any line of the result, and no two lines of the
/// texts are joined into one. The hunks are on `theirs`, so they apply to the live doc as
/// they are.
pub fn merge3_keep_both(base: &str, ours: &str, theirs: &str) -> Clean {
    let (cb, ct) = (closed(base), closed(theirs));
    let (a, b) = outside_sides(base, ours, theirs);
    // keep_both on the closed texts' sides: merge3_hunks' clean result when they don't
    // overlap (a property holds it); every group ends with "\n", so no two lines meet
    let hunks = keep_both(&cb, &a, &b);
    let merged = diff::apply(&ct, &hunks);
    let open = end_is_open(base, ours, theirs, &a, &b, &hunks, &merged);
    let hunks = onto_theirs(theirs, hunks, &merged, open);
    debug_assert!(
        diff::validate(theirs, &hunks).is_ok(),
        "keep-both hunks must be valid on theirs"
    );
    let text = diff::apply(theirs, &hunks);
    debug_assert_eq!(
        text,
        if open {
            &merged[..merged.len() - 1]
        } else {
            &merged[..]
        }
    );
    Clean { hunks, text }
}

/// `t` with a final "\n" when it has text and doesn't end with one (the outside save's
/// merge works on closed texts, v0.9.7).
pub fn closed(t: &str) -> std::borrow::Cow<'_, str> {
    if t.is_empty() || t.ends_with('\n') {
        std::borrow::Cow::Borrowed(t)
    } else {
        std::borrow::Cow::Owned(format!("{t}\n"))
    }
}

/// Whether `t` ends inside a line (has text, no final "\n").
fn is_open(t: &str) -> bool {
    !t.is_empty() && !t.ends_with('\n')
}

/// Whether the outside save's merge ends without a final "\n" (v0.9.7). `a` and `b` are
/// [`outside_sides`], `hunks` keep_both's on the closed `theirs`, `merged` their result
/// (closed). The last line's author decides: ours when the merge's last "\n" is in ours'
/// written text, theirs when it is in text theirs wrote; when it is a base line, or a change
/// both made (O4), the side that changed the base's ending (ours when it did, else theirs).
pub fn end_is_open(
    base: &str,
    ours: &str,
    theirs: &str,
    a: &[Hunk],
    b: &[Hunk],
    hunks: &[Hunk],
    merged: &str,
) -> bool {
    // an empty last line can't be left open (that would delete it)
    if merged.is_empty() || merged == "\n" || merged.ends_with("\n\n") {
        return false;
    }
    let both = || {
        if is_open(ours) != is_open(base) {
            is_open(ours)
        } else {
            is_open(theirs)
        }
    };
    // where character `target` of `hs` applied comes from: Err(offset in the text they
    // apply to) when copied from there, Ok(index) when hunk `index` writes it
    let origin = |hs: &[Hunk], target: usize| -> Result<usize, usize> {
        let (mut out, mut at) = (0, 0);
        for (k, h) in hs.iter().enumerate() {
            let copy = h.start - at;
            if target < out + copy {
                return Err(at + (target - out));
            }
            out += copy;
            if target < out + h.insert.len() {
                return Ok(k);
            }
            out += h.insert.len();
            at = h.end;
        }
        Err(at + (target - out))
    };
    match origin(hunks, merged.len() - 1) {
        Ok(_) => is_open(ours),
        // copied from theirs' text: written by theirs, by both, or a base line
        Err(q) => match origin(b, q) {
            Ok(k) if a.contains(&b[k]) => both(),
            Ok(_) => is_open(theirs),
            Err(_) => both(),
        },
    }
}

/// `hunks` on the closed `theirs` (keep_both's, giving `merged`) as hunks on `theirs`
/// itself, giving `merged`, or `merged` without its final "\n" when `open`.
fn onto_theirs(theirs: &str, mut hunks: Vec<Hunk>, merged: &str, open: bool) -> Vec<Hunk> {
    let n = theirs.len();
    if is_open(theirs) {
        // the closed text's last "\n" (at n) isn't in theirs: a hunk covering it ends at n
        // instead, and an insertion after it starts with it
        for h in &mut hunks {
            if h.start == n + 1 {
                h.start = n;
                h.end = n;
                h.insert.insert(0, '\n');
            } else if h.end == n + 1 {
                h.end = n;
            }
        }
        hunks.retain(|h| h.start != h.end || !h.insert.is_empty());
    }
    let want = if open {
        &merged[..merged.len() - 1]
    } else {
        merged
    };
    let got = diff::apply(theirs, &hunks);
    if got.len() < want.len() {
        // the final "\n", closed by the merge, not in theirs
        debug_assert_eq!(format!("{got}\n"), want);
        match hunks.last_mut() {
            Some(h) if h.end == n => h.insert.push('\n'),
            _ => hunks.push(Hunk {
                start: n,
                end: n,
                insert: "\n".to_owned(),
            }),
        }
    } else if got.len() > want.len() {
        debug_assert_eq!(got, format!("{want}\n"));
        // drop the final "\n": from the last hunk's text when it ends the result, else
        // theirs' own "\n" there (joined to the deletions after it)
        match hunks.last_mut() {
            Some(h) if h.end == n && !h.insert.is_empty() => {
                h.insert.pop();
                if h.start == h.end && h.insert.is_empty() {
                    hunks.pop();
                }
            }
            _ => {
                // the last character of theirs not deleted by a hunk
                let mut q = n;
                for h in hunks.iter().rev() {
                    if h.end < q {
                        break;
                    }
                    q = q.min(h.start);
                }
                q -= 1;
                hunks.push(Hunk {
                    start: q,
                    end: q + 1,
                    insert: String::new(),
                });
                hunks.sort_by_key(|h| (h.start, h.end));
                hunks = diff::coalesce(hunks);
            }
        }
    }
    hunks
}

/// The two sides [`merge3_keep_both`] merges: [`closed`] `base` to closed `ours` and to
/// closed `theirs` (hunks on the closed base), each [`diff::whole_line_hunks`] (v0.9.7): a
/// diff with its pure line insertions slid to line starts and its pure line deletions to
/// whole lines, then every hunk grown to whole lines ([`lines::to_whole_lines`]).
pub fn outside_sides(base: &str, ours: &str, theirs: &str) -> (Vec<Hunk>, Vec<Hunk>) {
    let cb = closed(base);
    (
        diff::whole_line_hunks(&cb, &closed(ours)),
        diff::whole_line_hunks(&cb, &closed(theirs)),
    )
}

/// The sides [`merge3_keep_both`] merges ([`outside_sides`], on the closed base: whole lines
/// already, so nothing more is widened where they overlap, v0.9.7), and `merge3_hunks`'s
/// clean result on them (on the closed texts) when there is one.
pub fn outside_merge_sides(
    base: &str,
    ours: &str,
    theirs: &str,
) -> (Vec<Hunk>, Vec<Hunk>, Option<Clean>) {
    let (a, b) = outside_sides(base, ours, theirs);
    let cb = closed(base);
    let clean = match merge3_hunks(&cb, &a, &b, !cb.is_empty()) {
        Merge::Clean(c) => Some(c),
        Merge::Conflicts(_) => None,
    };
    (a, b, clean)
}

/// The keep-both merge: `a` (ours) and `b` (theirs) hunks on one base (sorted, never
/// overlapping; whole-line sides may touch at a line start); returns
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
                if lines::is_whole_lines(base, x)
                    && lines::is_whole_lines(base, y)
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
                if lines::is_whole_lines(base, x)
                    && lines::is_whole_lines(base, y)
                    && !insert.ends_with('\n')
                {
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
            let span = |start: usize, inner: &str| {
                let spans = inner
                    .matches('\n')
                    .count()
                    .saturating_sub(inner.ends_with('\n') as usize);
                (start, start + spans)
            };
            let before = base[..r.start].matches('\n').count();
            // the region's first line in a side's text: the base's lines before it, plus
            // what that side's hunks before it (not the region's) added or removed
            let side_start = |hs: &[Hunk], idx: &[usize]| -> usize {
                let delta: isize = hs
                    .iter()
                    .enumerate()
                    .filter(|(k, h)| !idx.contains(k) && h.end <= r.start && h.start <= r.start)
                    .map(|(_, h)| {
                        h.insert.matches('\n').count() as isize
                            - base[h.start..h.end].matches('\n').count() as isize
                    })
                    .sum();
                (before as isize + delta) as usize + 1
            };
            let (ours, theirs) = (side(a, &ai), side(b, &bj));
            Conflict {
                lines: span(before + 1, &base[r.clone()]),
                ours_lines: span(side_start(a, &ai), &ours),
                theirs_lines: span(side_start(b, &bj), &theirs),
                base_text: base[r.clone()].to_owned(),
                ours,
                theirs,
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

    /// v0.9.7.1: nothing is merged inside a line. An agent's word change and a person's
    /// typing in the same line conflict (before, both landed); on different lines both land.
    #[test]
    fn agent_word_change_and_typing_in_the_same_line_conflict() {
        let base = "The quick brown fox.\n";
        let ours = "The quick red fox.\n"; // agent
        let theirs = "The quick brown fox jumps.\n"; // a person typed at the end
        let Merge::Conflicts(c) = merge3(base, ours, theirs) else {
            panic!("one line changed by both sides conflicts")
        };
        assert_eq!(
            (c[0].lines, c[0].ours_lines, c[0].theirs_lines),
            ((1, 1), (1, 1), (1, 1))
        );
        let base = "The quick brown fox.\nIt ran.\n";
        assert_eq!(
            clean(
                base,
                "The quick red fox.\nIt ran.\n",
                "The quick brown fox.\nIt jumped.\n"
            ),
            "The quick red fox.\nIt jumped.\n"
        );
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

    /// v0.9.7.1: different lines changed by each side merge, adjacent ones too, and at an
    /// open end; one line changed by both conflicts.
    #[test]
    fn adjacent_lines_changed_by_each_side_merge() {
        assert_eq!(clean("a\nb\n", "A\nb\n", "a\nB\n"), "A\nB\n");
        assert_eq!(clean("a\nb\n", "a\nB\n", "A\nb\n"), "A\nB\n");
        assert_eq!(clean("a\nb", "A\nb", "a\nB"), "A\nB");
        assert_eq!(
            clean("a\nb\nc\n", "a\nX\nY\nc\n", "A\nb\nC\n"),
            "A\nX\nY\nC\n"
        );
        // a deletion next to the other side's rewrite
        assert_eq!(clean("a\nb\nc\n", "a\nc\n", "a\nb\nC\n"), "a\nC\n");
        assert!(matches!(
            merge3("a\nb\n", "A\nb\n", "a2\nb\n"),
            Merge::Conflicts(_)
        ));
    }

    /// v0.9.7.1: typing into a line the writer changed is a conflict (before, both landed).
    #[test]
    fn typing_into_a_line_the_writer_changed_conflicts() {
        let base = "shared line\n";
        assert!(matches!(
            merge3(
                base,
                "shared LINE by agent\n",
                "shared line edited by Sam\n"
            ),
            Merge::Conflicts(_)
        ));
    }

    #[test]
    fn same_point_insertions_conflict_unless_identical() {
        let base = "a\nb\n";
        assert!(matches!(
            merge3(base, "a\nX\nb\n", "a\nY\nb\n"),
            Merge::Conflicts(_)
        ));
        assert!(matches!(
            merge3(base, "X\na\nb\n", "Y\na\nb\n"),
            Merge::Conflicts(_)
        ));
        assert_eq!(clean(base, "a\nX\nb\n", "a\nX\nb\n"), "a\nX\nb\n");
        // at the end of the text the two land instead (O5)
        assert_eq!(clean(base, "a\nb\nX\n", "a\nb\nY\n"), "a\nb\nY\nX\n");
        // ... but not at the end of a window that isn't the end of the text
        assert!(matches!(
            merge3_window(base, "a\nb\nX\n", "a\nb\nY\n", false),
            Merge::Conflicts(_)
        ));
        // inside one line, two insertions are two changes of that line: a conflict (v0.9.7.1)
        assert!(matches!(merge3("ab", "abX", "abY"), Merge::Conflicts(_)));
    }

    /// The keep-both rule character by character (the model of [`merge3_keep_both`]'s
    /// overlap): at each base offset, theirs' insertions, then ours', then the base character
    /// unless either side deletes it; two sides' groups of whole lines that meet with no base
    /// character between get a "\n" between them whenever the first lacks one. Equal hunks
    /// are one change (O4).
    fn keep_both_model(base: &str, ours: &str, theirs: &str) -> String {
        let (a, b, _) = outside_merge_sides(base, ours, theirs);
        let cb = closed(base);
        let both: Vec<Hunk> = b.iter().filter(|y| a.contains(y)).cloned().collect();
        let a: Vec<Hunk> = a.into_iter().filter(|x| !b.contains(x)).collect();
        let deletes = |h: &Hunk, p: usize| h.start <= p && p < h.end;
        let mut out = String::new();
        // who wrote the last character: 0 theirs, 1 ours, 2 both (O4) or the base
        let mut author = 2;
        let mut at: Vec<(usize, Option<char>)> =
            cb.char_indices().map(|(i, c)| (i, Some(c))).collect();
        at.push((cb.len(), None));
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
                out.push_str(&h.insert);
                author = if both.contains(h) { 2 } else { side };
            }
            if let Some(c) = c {
                if !b.iter().chain(&a).any(|h| deletes(h, p)) {
                    out.push(c);
                    author = 2;
                }
            }
        }
        // the end: open as the last line's author left its text; a base line or a change
        // both made: as the side that changed the base's ending (ours first)
        let open = |t: &str| !t.is_empty() && !t.ends_with('\n');
        let end_open = match author {
            0 => open(theirs),
            1 => open(ours),
            _ if open(ours) != open(base) => open(ours),
            _ => open(theirs),
        };
        // an empty last line can't be left open (that would delete it)
        if end_open && out.ends_with('\n') && out != "\n" && !out.ends_with("\n\n") {
            out.pop();
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

    /// The file's last line is never glued to the live side's typing (v0.9.6, 2c12c64, when
    /// the sides were widened to whole lines; Option B's whole-line sides keep it so): every
    /// line written is its own line.
    #[test]
    fn a_joined_group_is_widened_back_to_whole_lines() {
        let (base, file, live) = ("\n\n", "x\ny\nz\n", "E\n\nbab");
        let c = merge3_keep_both(base, file, live);
        // v0.9.5: "E\nx\ny\nzbab". The live side's "E" (typed on the first blank line) and
        // "bab" (on the second) and the file's x, y and z are each a line of their own; both
        // blank lines are gone, as the file deleted them. Each blank line both sides changed
        // keeps both versions, the live one first (Option B), and the file wrote the last line
        // with a final newline
        assert_eq!(c.text, "E\nx\nbab\ny\nz\n");
        assert!(!c.text.contains("zbab"));
        assert_eq!(diff::apply(live, &c.hunks), c.text);
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
            // both sides change one line (v0.9.7, by line; v0.9.5 merged these character by
            // character, 5cb28e51, e48dc26f): both versions of the line are kept, the live
            // one first, each a line of its own, with a final break in the base or not
            ("b", "xb", "c", "c\nxb"),
            ("b", "a\nxb", "c", "c\na\nxb"),
            ("b\n", "xb\n", "c\n", "c\nxb\n"),
            ("\n", "b\n", "aaa", "aaa\nb\n"),
            ("foo\n", "Xfoo\n", "foo bar", "foo bar\nXfoo\n"),
            ("foo\n", "foo bar", "Xfoo\n", "Xfoo\nfoo bar"),
        ] {
            let c = merge3_keep_both(base, file, live);
            assert_eq!(c.text, want, "{base:?} {file:?} {live:?}");
            assert_eq!(diff::apply(live, &c.hunks), c.text);
            assert_eq!(c.text, keep_both_model(base, file, live));
        }
        // two changes inside one line: by line, both versions of it (v0.9.7)
        assert_eq!(
            merge3_keep_both("abcde\n", "aBcde\n", "abcdeQ\n").text,
            "abcdeQ\naBcde\n"
        );
    }

    /// Every line of `got` is a whole line of one of the texts (a last line without its
    /// "\n" counts as the same line): no line holds two sides' changes, and no two lines are
    /// joined.
    fn lines_known(base: &str, ours: &str, theirs: &str, got: &str) -> Result<(), String> {
        let known: std::collections::HashSet<&str> = base
            .lines()
            .chain(ours.lines())
            .chain(theirs.lines())
            .collect();
        match got.lines().find(|l| !known.contains(l)) {
            None => Ok(()),
            Some(l) => Err(format!(
                "{l:?} is no line of B {base:?} O {ours:?} T {theirs:?}: got {got:?}"
            )),
        }
    }

    /// The fuzz oracle's join (v0.9.7, Option B): the file inserts a line before "[" and two
    /// blank lines at the end; the live side deletes the blank first line. By line: the
    /// file's "\x15@" is its own line, never joined to a neighbour.
    #[test]
    fn an_outside_save_never_joins_two_lines() {
        let (base, ours, theirs) = ("\n[\n", "\x15@\n[\n\n\n", "[\n");
        let (a, b) = outside_sides(base, ours, theirs);
        assert!(a.iter().chain(&b).all(|h| lines::is_whole_lines(base, h)));
        let c = merge3_keep_both(base, ours, theirs);
        assert_eq!(diff::apply(theirs, &c.hunks), c.text);
        lines_known(base, ours, theirs, &c.text).unwrap();
        assert_eq!(c.text, "\x15@\n[\n\n\n");
        assert_eq!(c.text, keep_both_model(base, ours, theirs));
    }

    /// The fuzz oracle's splice (v0.9.7, Option B): the file deletes "X" and adds a space to
    /// "e"; the live side rewrites "L" (after "X") as a blank line and deletes "e". The two
    /// sides' changes meet in line "e": by line they overlap, so the file's "e " and the live
    /// side's blank line are both kept, each a line of its own, never spliced.
    #[test]
    fn an_outside_save_never_splices_one_line_from_two_sides() {
        let (base, ours, theirs) = ("X\nL\ne\nL\n", "L\ne \nL\n", "X\n\nL\n");
        let (a, b) = outside_sides(base, ours, theirs);
        assert!(a.iter().chain(&b).all(|h| lines::is_whole_lines(base, h)));
        let c = merge3_keep_both(base, ours, theirs);
        assert_eq!(diff::apply(theirs, &c.hunks), c.text);
        lines_known(base, ours, theirs, &c.text).unwrap();
        let got: Vec<&str> = c.text.lines().collect();
        assert!(got.contains(&"e "), "{got:?}");
        assert!(got.contains(&""), "{got:?}");
        assert_eq!(c.text, keep_both_model(base, ours, theirs));
    }

    /// The keep-both join class (a fuzz crash on main, with or without the shifted-pairing
    /// fix): the file rewrites the text into four lines, the live side types "\u{1}" and
    /// "lo" at two line starts. v0.9.5 glued the file's "aaaaaaaaaaaaa" onto the live "lo"
    /// ("loaaaaaaaaaaaaa"); by line (v0.9.7) every line is a line of one side.
    #[test]
    fn an_outside_save_never_glues_a_line_to_the_other_sides_typing() {
        let base = "line 3\néline 3\nline 3\nline 3\naaline 3\n\n\n\n";
        let ours = "ab\naéline 3\nline 3\naaaaaaaaaaaaa\n";
        let theirs = "\u{1}line 3\nloéline 3\nline 3\nline 3\naaline 3\n\n\n\n";
        let c = merge3_keep_both(base, ours, theirs);
        assert_eq!(diff::apply(theirs, &c.hunks), c.text);
        lines_known(base, ours, theirs, &c.text).unwrap();
        assert!(c.text.lines().any(|l| l == "aaaaaaaaaaaaa"), "{:?}", c.text);
        assert!(!c.text.contains("loaaaaaaaaaaaaa"), "{:?}", c.text);
        assert_eq!(c.text, keep_both_model(base, ours, theirs));
        // by base line: the live and the file's rewrites of line 1, then of line 2; the
        // "line 3" both kept; the file's rewrite of lines 4-8
        assert_eq!(
            c.text,
            "\u{1}line 3\nab\nloéline 3\naéline 3\nline 3\naaaaaaaaaaaaa\n"
        );
    }

    /// The end of the text is merged on its own (v0.9.7): the texts are merged closed, and
    /// the result ends without a final "\n" only when the side that wrote its last line left
    /// its text so (a base line: the side that changed the ending). A line one side deleted
    /// never comes back because the other side only closed it or wrote lines after it, and
    /// a final "\n" one side added survives. Agrees with write --base's O5a (cases A-D).
    #[test]
    fn the_end_of_the_text_is_merged_on_its_own() {
        for (base, file, live, want) in [
            // the open-end class (a deletion brought back before): the probe, and the keep_both
            // corpus shapes
            ("a\nb", "a\nb\nc", "a\n", "a\nc"),
            ("[", "", "[\n", ""),
            (
                "aa",
                "a",
                "aa\naaaaaaaaaaaaaaaaaaaaaaa",
                "a\naaaaaaaaaaaaaaaaaaaaaaa",
            ),
            ("[", "\n[\n", "", "\n"),
            ("éa", "a", "éa\naéa", "a\naéa"),
            // a final "\n" one side added survives; one side removed stays removed
            ("a\nb", "a\nb\n", "A\nb", "A\nb\n"),
            ("a\nb\n", "a\nb", "A\nb\n", "A\nb"),
            ("a", "a\n", "a", "a\n"),
            // O5a's cases (write --base, v0.9.8): A, B, C, D, and an open last line of one's own
            ("b\n", "b\nZ\n", "a", "a\nZ\n"),
            ("x\n", "x\nB\n", "x\nA", "x\nA\nB\n"),
            ("x\n", "x", "x\nT\n", "x\nT\n"),
            ("x\n", "x\nB", "x\nA", "x\nA\nB"),
            ("x\n", "x\nB", "x\nA\n", "x\nA\nB"),
        ] {
            let c = merge3_keep_both(base, file, live);
            assert_eq!(c.text, want, "{base:?} {file:?} {live:?}");
            assert_eq!(diff::apply(live, &c.hunks), c.text);
            assert_eq!(c.text, keep_both_model(base, file, live));
            lines_known(base, file, live, &c.text).unwrap();
        }
    }

    /// A line one side kept is never read as part of its change (v0.9.7): the live side
    /// keeps the blank line and writes lines around it; the file deletes it. Before, the
    /// side diff took the kept line into the live side's change (its suffix trimmed at a line
    /// start of the base only), and keep-both brought the deleted line back.
    #[test]
    fn a_kept_line_is_never_part_of_a_change() {
        for (base, file, live, want) in [
            ("\n", "", "ab\n\naaaaaa", "ab\naaaaaa"),
            ("?\n\n", "", "\naaaaaaaaa", "aaaaaaaaa"),
            ("\n", "ab\n\nabaaaa", "", "ab\nabaaaa"),
            // the CHANGELOG's example: the live side deletes "A", the file never touches it
            // (v0.9.5 and v0.9.6 gave "Z\nX\nA")
            ("aX\nA\nb\n", "X\nA", "Z", "Z\nX"),
        ] {
            let c = merge3_keep_both(base, file, live);
            assert_eq!(c.text, want, "{base:?} {file:?} {live:?}");
            assert_eq!(diff::apply(live, &c.hunks), c.text);
            assert_eq!(c.text, keep_both_model(base, file, live));
        }
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
        // ours replaces line "bc", theirs inserts lines right before it and right after it
        assert_eq!(
            clean("a\nbc\nd\n", "a\nX\nd\n", "a\n1\nbc\n2\nd\n"),
            "a\n1\nX\n2\nd\n"
        );
        // inside a line the same edits are one line changed twice: a conflict (v0.9.7.1)
        assert!(matches!(
            merge3("abcd", "aXd", "a1bc2d"),
            Merge::Conflicts(_)
        ));
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
            // ours edits only the left lines, theirs only the right ones; a fixed separator
            // line keeps them apart (v0.9.7.1: whole lines)
            let base = format!("{left}\n|{mid}|\n{right}");
            let ours = format!("{l2}\n|{mid}|\n{right}");
            let theirs = format!("{left}\n|{mid}|\n{r2}");
            prop_assert_eq!(clean(&base, &ours, &theirs), format!("{l2}\n|{mid}|\n{r2}"));
        }

        /// Symmetric up to O5: when both sides append different text at the end, swapping
        /// the sides swaps the two appends; everything before them is the same.
        #[test]
        fn clean_merges_are_symmetric_up_to_eof_order(base in text(), o in text(), t in text()) {
            if let (Merge::Clean(x), Merge::Clean(y)) = (merge3(&base, &o, &t), merge3(&base, &t, &o)) {
                // on the closed texts (v0.9.7.1: the sides are whole lines of those)
                let cb = closed(&base);
                let eof = |new: &str| {
                    diff::whole_line_hunks(&cb, &closed(new))
                        .pop()
                        .filter(|h| h.is_insertion() && h.start == cb.len())
                        .map(|h| h.insert)
                };
                let (xt, yt) = (closed(&x.text).into_owned(), closed(&y.text).into_owned());
                match (eof(&o), eof(&t)) {
                    (Some(eo), Some(et)) if eo != et => {
                        let tail = eo.len() + et.len();
                        prop_assert!(xt.ends_with(&format!("{et}{eo}")), "{:?}", x.text);
                        prop_assert!(yt.ends_with(&format!("{eo}{et}")), "{:?}", y.text);
                        prop_assert_eq!(&xt[..xt.len() - tail], &yt[..yt.len() - tail]);
                    }
                    _ => prop_assert_eq!(x.text, y.text),
                }
            }
        }

        /// An outside save's hunks trimmed to the characters that change (the session's
        /// ingest, v0.9.7) give `merge3_keep_both`'s text byte for byte, and each trimmed hunk
        /// lies within its untrimmed one.
        #[test]
        fn trimmed_outside_hunks_keep_the_merged_text(base in text(), file in text(), live in text()) {
            let c = merge3_keep_both(&base, &file, &live);
            let trimmed = diff::trim_hunks(&live, &c.hunks);
            prop_assert!(diff::validate(&live, &trimmed).is_ok());
            prop_assert_eq!(diff::apply(&live, &trimmed), c.text.clone());
            for h in &c.hunks {
                if let Some(t) = diff::trim_hunk(&live, h) {
                    prop_assert!(h.start <= t.start && t.end <= h.end, "{:?} outside {:?}", t, h);
                    prop_assert!(h.insert.contains(t.insert.as_str()));
                }
            }
        }

        /// The keep-both merge (an outside save, v0.9.5) is the character model, always valid
        /// on theirs, and on hunk sets merge3 finds clean it is merge3's own result.
        #[test]
        fn keep_both_is_the_model_and_merge3_when_clean(base in text(), o in text(), t in text()) {
            // the diffs of the texts as they are, and the outside sides (on the closed texts)
            let (cb, co, ct) = (closed(&base), closed(&o), closed(&t));
            for (bb, tt, (a, b)) in [
                (&base[..], &t[..], (diff::hunks(&base, &o), diff::hunks(&base, &t))),
                (&cb[..], &ct[..], outside_sides(&base, &o, &t)),
            ] {
                let kb = keep_both(bb, &a, &b);
                prop_assert!(diff::validate(tt, &kb).is_ok());
                if let Merge::Clean(c) = merge3_hunks(bb, &a, &b, !bb.is_empty()) {
                    let text = diff::apply(tt, &kb);
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
            for (side, new) in [(&a, &co), (&b, &ct)] {
                prop_assert!(diff::validate(&cb, side).is_ok(), "{:?}", side);
                prop_assert_eq!(&diff::apply(&cb, side), &**new);
            }
            // the merge is the model; when clean, merge3_hunks' on the closed texts but for
            // its final "\n"
            let m = merge3_keep_both(&base, &o, &t);
            prop_assert_eq!(diff::apply(&t, &m.hunks), m.text.clone());
            prop_assert_eq!(&m.text, &keep_both_model(&base, &o, &t));
            if let Some(c) = &clean {
                prop_assert_eq!(&*closed(&m.text), &c.text);
            } else {
                // what overlaps is whole lines on both sides, so no two sides' lines are
                // spliced into one
                for x in &a {
                    for y in &b {
                        if x != y && overlaps(x, y) {
                            prop_assert!(
                                lines::is_whole_lines(&cb, x) && lines::is_whole_lines(&cb, y),
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

        /// No line of an outside save's merge holds two sides' changes, and no two lines are
        /// joined (v0.9.7): on any texts, every line of the merge is a line of the base, the
        /// file or the live text; the sides are whole lines.
        #[test]
        fn an_outside_merge_neither_splices_nor_joins(base in text(), o in text(), t in text()) {
            let (a, b) = outside_sides(&base, &o, &t);
            let cb = closed(&base);
            for (side, new) in [(&a, closed(&o)), (&b, closed(&t))] {
                prop_assert!(side.iter().all(|h| lines::is_whole_lines(&cb, h)), "{:?}", side);
                prop_assert_eq!(diff::apply(&cb, side), new.into_owned());
            }
            let m = merge3_keep_both(&base, &o, &t);
            if let Err(e) = lines_known(&base, &o, &t, &m.text) {
                return Err(TestCaseError::fail(e));
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

    // ---------- write --base's sides, by line then by character ----------

    /// The false conflict the line-aligned sides remove: three lines sharing their first
    /// character; ours deletes the last, theirs the middle one and appends two.
    /// `diff::hunks`' character trim read theirs' deletion one line off (its common prefix ran
    /// into the next line's shared "p"), so the character-trimmed sides overlapped ours'; the
    /// line-aligned sides don't.
    #[test]
    fn lines_sharing_a_start_never_pair_one_line_off() {
        let base = "p05:2216\np22:2150\np10:2205\n";
        let ours = "p05:2216\np22:2150\n";
        let theirs = "p05:2216\np10:2205\np23:2229\np25:2184\n";
        assert_eq!(clean(base, ours, theirs), "p05:2216\np23:2229\np25:2184\n");
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let (a, b) = write_base_sides(base, ours, theirs);
        assert_eq!(a, vec![h(18, 27, "")]);
        assert_eq!(b, vec![h(9, 18, ""), h(27, 27, "p23:2229\np25:2184\n")]);
        // the control: the character-trimmed sides conflict on the same texts
        let (a0, b0) = (diff::hunks(base, ours), diff::hunks(base, theirs));
        assert!(matches!(
            merge_sets(base, &a0, &b0, theirs, true, |i, j| a0[i] == b0[j]),
            Merge::Conflicts(_)
        ));
        // the sides swapped: the same text
        assert_eq!(clean(base, theirs, ours), "p05:2216\np23:2229\np25:2184\n");
        // still conflict-refusing: both deleting the same line is O4, one deleting a line the
        // other rewrote is a conflict
        assert_eq!(clean(base, ours, ours), ours);
        assert!(matches!(
            merge3(base, ours, "p05:2216\np22:2150\np10:9999\n"),
            Merge::Conflicts(_)
        ));
    }

    /// The splice the line-aligned sides remove (soak_32 seed 1, call 7836, trimmed): ours
    /// inserts a line, theirs rewrites the lines around it into one. The character-trimmed
    /// theirs side kept single characters of the old lines ("p" "2" "0" ":90") and ours'
    /// insertion landed between two of them, cleanly, splicing two lines into "p213:102" and
    /// "p0:90". The line-aligned sides overlap there: a conflict, nothing spliced.
    #[test]
    fn an_insertion_never_lands_inside_another_sides_rewritten_line() {
        let base = "p26:95\np05:90\n";
        let ours = "p26:95\np13:102\np05:90\n";
        let theirs = "p20:90\n";
        // the control: the character-trimmed sides merge "cleanly" into a splice
        let (a0, b0) = (diff::hunks(base, ours), diff::hunks(base, theirs));
        let Merge::Clean(c) = merge_sets(base, &a0, &b0, theirs, true, |i, j| a0[i] == b0[j])
        else {
            panic!("the control merged clean before")
        };
        assert_eq!(c.text, "p213:102\np0:90\n");
        let Merge::Conflicts(c) = merge3(base, ours, theirs) else {
            panic!("ours' line lands inside theirs' rewritten lines: a conflict")
        };
        assert_eq!(c[0].lines, (1, 2));
        assert!(matches!(merge3(base, theirs, ours), Merge::Conflicts(_)));
    }

    /// v0.9.7.1: nothing is merged inside a line. Two writers changing different words of
    /// one line conflict (before, both landed); changes to different lines merge, on lines
    /// sharing their start too.
    #[test]
    fn different_words_of_one_line_conflict_and_different_lines_merge() {
        let base = "alpha beta gamma\n";
        let Merge::Conflicts(c) = merge3(base, "alpha BETA gamma\n", "alpha beta GAMMA\n") else {
            panic!("two words of one line conflict")
        };
        assert_eq!(c[0].lines, (1, 1));
        let base = "p05 2216 a\np05 2150 b\n";
        assert_eq!(
            clean(base, "p05 2216 A\np05 2150 b\n", "p05 2216 a\np05 9999 b\n"),
            "p05 2216 A\np05 9999 b\n"
        );
        assert!(matches!(
            merge3(base, "p05 2216 A\np05 2150 b\n", "p05 7777 a\np05 2150 b\n"),
            Merge::Conflicts(_)
        ));
    }

    /// Distinct lines of the form `pNN:NNNN`, sharing their first characters (many share
    /// "p0" or "p1"), shuffled.
    fn shared_start_lines() -> impl Strategy<Value = Vec<String>> {
        prop::collection::btree_set(0u32..40, 2..9)
            .prop_map(|s| {
                s.into_iter()
                    .map(|n| format!("p{n:02}:{:04}\n", (n * 7919 + 2150) % 10_000))
                    .collect::<Vec<_>>()
            })
            .prop_shuffle()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(cases(3000)))]

        /// Lines sharing their first characters: ours and theirs delete different lines of a
        /// base of distinct `pNN:NNNN` lines, and theirs may append new ones; the merge is
        /// always clean, and is the base without both sides' lines, theirs' new lines last.
        /// (The character-trimmed sides read some of these one line off and conflicted.)
        #[test]
        fn deleting_different_lines_that_share_a_start_merges(
            base in shared_start_lines(),
            del in prop::collection::vec(0u8..3, 9),
            add in prop::collection::btree_set(50u32..90, 0..3),
        ) {
            let add: String = add
                .into_iter()
                .map(|n| format!("p{n:02}:{:04}\n", (n * 104_729) % 10_000))
                .collect();
            // per line: 0 kept, 1 ours deletes it, 2 theirs deletes it
            let keep = |side: u8| -> String {
                base.iter()
                    .zip(&del)
                    .filter(|(_, d)| **d != side)
                    .map(|(l, _)| l.as_str())
                    .collect()
            };
            let b: String = base.concat();
            let ours = keep(1);
            let theirs = format!("{}{add}", keep(2));
            let want: String = base
                .iter()
                .zip(&del)
                .filter(|(_, d)| **d == 0)
                .map(|(l, _)| l.as_str())
                .collect::<String>()
                + &add;
            match merge3(&b, &ours, &theirs) {
                Merge::Clean(c) => prop_assert_eq!(c.text, want, "B {:?} O {:?} T {:?}", b, ours, theirs),
                Merge::Conflicts(c) => prop_assert!(false, "a false conflict: B {:?} O {:?} T {:?}: {:?}", b, ours, theirs, c),
            }
        }
    }
}
