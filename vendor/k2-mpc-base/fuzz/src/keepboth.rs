//! An independent oracle for a conflicting outside save (`merge-rule`): the kit keeps both
//! sides at line granularity. With base = the synced text, ours = the file's new text and
//! theirs = the live text, where the two sides' changes overlap (a `merge3` conflict, widened
//! to whole lines and to every change of either side it meets), the result
//!
//! - (i) holds every line either side wrote,
//! - (ii) holds no base line either side deleted,
//! - (iii) holds no line spliced from fragments of both sides, and no two lines joined,
//!
//! and elsewhere the changes merge as in a clean merge. (iv), that a flush then leaves the
//! file the doc and nothing pending, is the caller's.
//!
//! Since v0.9.7 an outside save's sides are whole lines ([`check_aligned`] holds that
//! independently), so two sides' changes never share a line: (iii) holds for every outside
//! merge, clean or not ([`no_splice_no_join`]), with no tolerance for a clean merge's splice
//! or join (the COSMETIC class is gone).
//!
//! Which base lines a side deleted and which it wrote come from its hunks (as the merge
//! aligns them), widened to whole lines, so each line's count in the result is exact; the
//! result is compared as a **multiset** of lines, never by position (with repeated
//! identical lines, the order of copies can't be told apart). Skipped cases are counted
//! in [`Stats`].
//!
//! Nothing here calls the kit's keep-both merge (that would check it against itself): only
//! `merge3_hunks` to find the conflicting regions, on sides aligned as the caller says.

use std::collections::HashMap;
use std::ops::Range;

use k2_mpc_base::text::diff::{self, Hunk};
use k2_mpc_base::text::merge::{self, Merge};

/// What a check could and couldn't decide.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Conflicting regions (after widening and joining).
    pub regions: usize,
    /// Line texts whose count was checked.
    pub exact: usize,
    /// Changes made identically on both sides, taken once (O4).
    pub o4: usize,
    /// Line texts skipped: a change outside the regions touches them, so their count is
    /// a clean merge's, not this rule's.
    pub skipped: usize,
    /// Some text ended without a line break: its last line was compared as if it had one.
    pub open_end: bool,
}

fn lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// A line as compared: a last line without a line break counts as the same line with one
/// (keeping both sides at the end of a text may have to put a break after one of them; a
/// splice, "y" + "x", stays a different line).
fn key(l: &str) -> &str {
    l.strip_suffix('\n').unwrap_or(l)
}

fn count<'a>(ls: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut m = HashMap::new();
    for l in ls {
        *m.entry(key(l)).or_insert(0) += 1;
    }
    m
}

/// `[start, end)` of `t` widened to whole lines. An empty range at a line start stays
/// empty: an insertion there touches no line (whole lines inserted between two), and
/// [`close`] takes in the next line when what a side writes there isn't whole lines.
fn widen(t: &str, r: Range<usize>) -> Range<usize> {
    if r.is_empty() && (r.start == 0 || t.as_bytes()[r.start - 1] == b'\n') && r.start < t.len() {
        return r;
    }
    let s = t[..r.start].rfind('\n').map_or(0, |i| i + 1);
    let e = if r.end > s && t[..r.end].ends_with('\n') {
        r.end
    } else {
        t[r.end..].find('\n').map_or(t.len(), |i| r.end + i + 1)
    };
    s..e.max(s)
}

/// Whether hunk `h` meets `[s, e)` (touching counts).
fn meets(h: &Hunk, r: &Range<usize>) -> bool {
    h.start <= r.end && h.end >= r.start
}

/// Where base offset `p` lands in the side (`hunks` applied), taking insertions at `p` in
/// when `inclusive` (the region's end) and leaving them out otherwise (its start).
fn map(hunks: &[Hunk], p: usize, inclusive: bool) -> usize {
    let mut shift: isize = 0;
    for h in hunks {
        let before = if inclusive {
            h.end <= p
        } else {
            h.end < p || (h.end == p && !h.is_insertion())
        };
        if before {
            shift += h.insert.len() as isize - (h.end - h.start) as isize;
        }
    }
    (p as isize + shift) as usize
}

/// `r` grown until stable: by every hunk of `fold` it meets (touching counts), to whole
/// base lines, and to whole lines of each side's text over it (a side whose text over it
/// doesn't end a line takes in the next base line). `fold` holds every hunk of both sides,
/// so no hunk crosses the result's ends and each side's text over it is well defined.
fn close(
    base: &str,
    sides: [(&[Hunk], &str); 2],
    fold: &[&Hunk],
    mut r: Range<usize>,
) -> Range<usize> {
    loop {
        let mut n = r.clone();
        for h in fold {
            if meets(h, &r) {
                n.start = n.start.min(h.start);
                n.end = n.end.max(h.end);
            }
        }
        let n = widen(base, n);
        if n != r {
            r = n;
            continue;
        }
        // stable under folding: the side texts over it are well defined; its end is a
        // line start (or the end), so the next base line is whole
        let mut e = r.end;
        for (hs, text) in sides {
            let t = &text[map(hs, r.start, false)..map(hs, r.end, true)];
            if !t.is_empty() && !t.ends_with('\n') && r.end < base.len() {
                e = base[r.end..]
                    .find('\n')
                    .map_or(base.len(), |i| r.end + i + 1);
            }
        }
        if e == r.end {
            return r;
        }
        r.end = e;
    }
}

/// `t` with a final "\n" when it has text and lacks one: the outside merge works on closed
/// texts (v0.9.7). The oracle's own, not the kit's `merge::closed`.
pub fn closed_text(t: &str) -> String {
    if t.is_empty() || t.ends_with('\n') {
        t.to_owned()
    } else {
        format!("{t}\n")
    }
}

fn open(t: &str) -> bool {
    !t.is_empty() && !t.ends_with('\n')
}

/// A clean outside merge, built independently (v0.9.7): `ours` and `theirs` are the sides on
/// the closed `base` (whole lines); every hunk of theirs (the live side) and every hunk of
/// ours (the file) not identical to one of theirs (O4) is applied to the closed base in base
/// order (at one point: insertions before replacements, theirs' insertion first). Then the
/// end: the result is left without its final "\n" when the side that wrote its last line
/// left its own text so (`ours_text`, `theirs_text`); when its last line is a base line, or
/// a change both made, when the side that changed the base's ending did (ours first).
/// `None` when the hunks overlap (not a clean merge).
pub fn clean_outside(
    base: &str,
    ours_text: &str,
    theirs_text: &str,
    ours: &[Hunk],
    theirs: &[Hunk],
) -> Option<String> {
    let cb = closed_text(base);
    // (hunk, side: 0 ours, 1 theirs, 2 both; rank at one point)
    let mut all: Vec<(&Hunk, u8, u8)> = theirs
        .iter()
        .map(|h| {
            let side = if ours.contains(h) { 2 } else { 1 };
            (h, side, if h.is_insertion() { 0 } else { 2 })
        })
        .collect();
    for h in ours {
        if !theirs.contains(h) {
            all.push((h, 0, if h.is_insertion() { 1 } else { 2 }));
        }
    }
    all.sort_by_key(|(h, _, rank)| (h.start, *rank));
    let mut out = String::new();
    let mut at = 0;
    // who wrote the last character so far: 0 ours, 1 theirs, 2 both or the base
    let mut author = 2;
    for (h, side, _) in all {
        if h.start < at {
            return None;
        }
        if h.start > at {
            out.push_str(&cb[at..h.start]);
            author = 2;
        }
        if !h.insert.is_empty() {
            out.push_str(&h.insert);
            author = side;
        }
        at = h.end;
    }
    if at < cb.len() {
        out.push_str(&cb[at..]);
        author = 2;
    }
    let end_open = match author {
        0 => open(ours_text),
        1 => open(theirs_text),
        _ if open(ours_text) != open(base) => open(ours_text),
        _ => open(theirs_text),
    };
    // an empty last line can't be left open (that would delete it)
    if end_open && out.ends_with('\n') && out != "\n" && !out.ends_with("\n\n") {
        out.pop();
    }
    Some(out)
}

/// The lines of an outside merge, exactly (v0.9.7), as a multiset (a last line without its
/// "\n" is the same line): `a` and `b` are the sides on the closed `base` (whole lines), so
/// the merge holds each base line neither side's hunk covers, every line ours' hunks write
/// (one identical to a hunk of theirs once, O4) and every line theirs' write. `Err` names a
/// line the result lacks (lost) or holds too often (a line a side deleted come back, or a
/// line twice). Clean merges and kept-both ones alike: nothing either side deleted comes
/// back, nothing either kept or wrote is lost.
pub fn check_lines(base: &str, a: &[Hunk], b: &[Hunk], result: &str) -> Result<(), String> {
    let cb = closed_text(base);
    let mut want: HashMap<&str, isize> = HashMap::new();
    let mut s = 0;
    for l in lines(&cb) {
        let e = s + l.len();
        if !a.iter().chain(b).any(|h| h.start < e && s < h.end) {
            *want.entry(key(l)).or_insert(0) += 1;
        }
        s = e;
    }
    for h in a.iter().filter(|h| !b.contains(h)).chain(b) {
        for l in lines(&h.insert) {
            *want.entry(key(l)).or_insert(0) += 1;
        }
    }
    let got = count(lines(result));
    let mut keys: Vec<&str> = want.keys().chain(got.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let (w, g) = (
            want.get(k).copied().unwrap_or(0),
            got.get(k).copied().unwrap_or(0) as isize,
        );
        if g < w {
            return Err(format!(
                "a line lost: {k:?} {g} times, the sides keep or write it {w} times"
            ));
        }
        if g > w {
            return Err(format!(
                "a deleted line back (or a line twice): {k:?} {g} times, the sides keep or write it {w} times"
            ));
        }
    }
    Ok(())
}

/// Over **every** longest common subsequence of `base` and `side` (line texts): the fewest
/// and the most copies of each text of `base` it keeps (matches), by plain O(nm) tables over
/// lines: one for the LCS lengths, then, per text, one that counts that text's matches along
/// optimal paths only (min and max). The oracle's own; no kit diff code. `None` above
/// [`LCS_MAX_CELLS`] table cells.
fn lcs_keeps<'t>(base: &[&'t str], side: &[&str]) -> Option<HashMap<&'t str, (usize, usize)>> {
    let (n, m) = (base.len(), side.len());
    let mut texts: Vec<&str> = base.to_vec();
    texts.sort_unstable();
    texts.dedup();
    if n.saturating_mul(m).saturating_mul(texts.len() + 1) > LCS_MAX_CELLS {
        return None;
    }
    let w = m + 1;
    let mut len = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i * w + j] = if base[i] == side[j] {
                len[(i + 1) * w + j + 1] + 1
            } else {
                len[(i + 1) * w + j].max(len[i * w + j + 1])
            };
        }
    }
    let mut out = HashMap::new();
    let (mut lo, mut hi) = (vec![0u32; (n + 1) * w], vec![0u32; (n + 1) * w]);
    for x in texts {
        for i in (0..=n).rev() {
            for j in (0..=m).rev() {
                let k = i * w + j;
                if i == n || j == m {
                    (lo[k], hi[k]) = (0, 0);
                    continue;
                }
                let (mut l, mut h) = (u32::MAX, 0u32);
                let here = len[k];
                if base[i] == side[j] && len[(i + 1) * w + j + 1] + 1 == here {
                    let add = u32::from(base[i] == x);
                    l = l.min(lo[(i + 1) * w + j + 1] + add);
                    h = h.max(hi[(i + 1) * w + j + 1] + add);
                }
                if len[(i + 1) * w + j] == here {
                    l = l.min(lo[(i + 1) * w + j]);
                    h = h.max(hi[(i + 1) * w + j]);
                }
                if len[i * w + j + 1] == here {
                    l = l.min(lo[i * w + j + 1]);
                    h = h.max(hi[i * w + j + 1]);
                }
                (lo[k], hi[k]) = (l, h);
            }
        }
        out.insert(x, (lo[0] as usize, hi[0] as usize));
    }
    Some(out)
}

/// Above this many table cells (base lines times side lines times base line texts)
/// [`check_lcs`] can't judge a merge; such merges are counted and reported
/// ([`lcs_skipped`]), never passed silently as judged.
pub const LCS_MAX_CELLS: usize = 50_000_000;

/// Merges [`check_lcs`] could not judge (too large), so far.
pub fn lcs_skipped() -> usize {
    LCS_SKIPPED.load(std::sync::atomic::Ordering::Relaxed)
}

static LCS_SKIPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The line check, independent of the kit's diff (v0.9.7): each side's line operations from
/// the oracle's own line LCS against `base` ([`lcs_keeps`]; lines compared without their
/// "\n"). Repeated lines (blank ones above all) make the alignment ambiguous, so the rule is
/// written over **all** LCS alignments, per line text `x`, with `d_b`, `d_o`, `d_t` its
/// copies in base, ours and theirs and `[kmin_s, kmax_s]` the copies of it an LCS of base
/// and side `s` keeps (fewest, most):
/// - **no deleted line back:** `result` holds `x` at most
///   `min(d_b, kmax_o, kmax_t) + (d_o - kmin_o) + (d_t - kmin_t)` times: the base copies
///   both sides can keep, plus the copies each side can be said to add. Every alignment's
///   keep-both result is within it; a copy one side deleted under every alignment while the
///   other kept it is not. A line counts as kept if any LCS keeps it.
/// - **nothing lost:** `result` holds `x` at least
///   `max(0, kmin_o + kmin_t - d_b) + max(d_o - kmax_o, d_t - kmax_t)` times: the base
///   copies both sides keep under every alignment, plus the copies one side adds under
///   every alignment (the larger side: a line both added may be one change, O4).
pub fn check_lcs(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    let split = |t| -> Vec<&str> { lines(t).into_iter().map(key).collect() };
    let (bl, ol, tl) = (split(base), split(ours), split(theirs));
    let (Some(ko), Some(kt)) = (lcs_keeps(&bl, &ol), lcs_keeps(&bl, &tl)) else {
        LCS_SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        eprintln!(
            "check_lcs: a merge too large to judge ({} base lines), {} so far",
            bl.len(),
            lcs_skipped()
        );
        return Ok(());
    };
    let (db, d_o, dt) = (count(lines(base)), count(lines(ours)), count(lines(theirs)));
    let got = count(lines(result));
    let mut keys: Vec<&str> = db
        .keys()
        .chain(d_o.keys())
        .chain(dt.keys())
        .chain(got.keys())
        .copied()
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let c = |m: &HashMap<&str, usize>, x: &str| m.get(x).copied().unwrap_or(0);
    for x in keys {
        let (b, o, t, g) = (c(&db, x), c(&d_o, x), c(&dt, x), c(&got, x));
        let (o_lo, o_hi) = ko.get(x).copied().unwrap_or((0, 0));
        let (t_lo, t_hi) = kt.get(x).copied().unwrap_or((0, 0));
        let upper = b.min(o_hi).min(t_hi) + (o - o_lo) + (t - t_lo);
        let lower = (o_lo + t_lo).saturating_sub(b) + (o - o_hi).max(t - t_hi);
        if g > upper {
            return Err(format!(
                "(lcs) a deleted line back (or a line too often): {x:?} {g} times, at most {upper}"
            ));
        }
        if g < lower {
            return Err(format!(
                "(lcs) a line lost: {x:?} {g} times, at least {lower}"
            ));
        }
    }
    Ok(())
}

/// What [`check_outside`] judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    KeptBoth(Stats),
}

/// The whole outside-merge oracle (v0.9.7) for `result`, the merge of `ours` (the file) and
/// `theirs` (the live text) on `base` (the synced text); `a` and `b` are the kit's sides
/// (`merge::outside_sides`: hunks on the closed base). In order: each side as the kit takes
/// it ([`check_aligned`] on the closed texts: whole lines, the kit's side diff); no splice,
/// no join ([`no_splice_no_join`]); the exact line multiset ([`check_lines`]); then a clean
/// merge is [`clean_outside`] exactly, a conflicting one keeps both by line ([`check_sides`]
/// on the closed texts) and never ends open unless one of the texts did.
pub fn check_outside(
    base: &str,
    ours: &str,
    theirs: &str,
    a: &[Hunk],
    b: &[Hunk],
    result: &str,
) -> Result<Outcome, String> {
    let (cb, co, ct) = (closed_text(base), closed_text(ours), closed_text(theirs));
    for (side, h, what) in [(&co, a, "ours"), (&ct, b, "theirs")] {
        check_aligned(&cb, side, h).map_err(|e| format!("the {what} side: {e}"))?;
    }
    no_splice_no_join(base, ours, theirs, result)?;
    check_lines(base, a, b, result)?;
    check_lcs(base, ours, theirs, result)?;
    match merge::merge3_hunks(&cb, a, b, !cb.is_empty()) {
        Merge::Clean(_) => {
            let want = clean_outside(base, ours, theirs, a, b)
                .ok_or("merge3_hunks says clean; the hunks overlap")?;
            if result != want {
                return Err(format!("a clean outside merge: {result:?}, want {want:?}"));
            }
            Ok(Outcome::Clean)
        }
        Merge::Conflicts(_) => {
            let st = check_sides(&cb, &co, &ct, a, b, &closed_text(result))?;
            if open(result) && !(open(base) || open(ours) || open(theirs)) {
                return Err(format!("ends open, though no text did: {result:?}"));
            }
            Ok(Outcome::KeptBoth(st))
        }
    }
}

/// (iii) on any outside merge (v0.9.7), clean or not: every line of `result` is a whole line
/// of `base`, `ours` or `theirs` (a last line without its break counts as the same line with
/// one), so no line holds two sides' changes and no two lines are joined. `Err` names the
/// line: a join when it is two lines of the texts end to end, a splice otherwise.
pub fn no_splice_no_join(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    let known: std::collections::HashSet<&str> = [base, ours, theirs]
        .iter()
        .flat_map(|t| lines(t))
        .map(key)
        .collect();
    for l in lines(result) {
        let k = key(l);
        if known.contains(k) {
            continue;
        }
        let join = k
            .char_indices()
            .map(|(i, _)| i)
            .skip(1)
            .any(|i| known.contains(&k[..i]) && known.contains(&k[i..]));
        return Err(if join {
            format!("(iii) two lines joined into one: {l:?}")
        } else {
            format!("(iii) a line spliced from both sides: {l:?}")
        });
    }
    Ok(())
}

/// [`check_sides`] with each side the outside merge's diff from `base`
/// (`diff::whole_line_hunks`, on the closed texts).
pub fn check(base: &str, ours: &str, theirs: &str, result: &str) -> Result<Stats, String> {
    let (cb, co, ct) = (closed_text(base), closed_text(ours), closed_text(theirs));
    let (a, b) = (
        diff::whole_line_hunks(&cb, &co),
        diff::whole_line_hunks(&cb, &ct),
    );
    check_sides(&cb, &co, &ct, &a, &b, &closed_text(result))
}

/// Check `result` against the rule; `Err` names the violated property. `a` and `b` are the
/// sides as hunks on `base` (`ours` and `theirs` applied), as the merge aligns them (the
/// kit's outside merge slides line insertions to line starts).
pub fn check_sides(
    base: &str,
    ours: &str,
    theirs: &str,
    a: &[Hunk],
    b: &[Hunk],
    result: &str,
) -> Result<Stats, String> {
    let mut st = Stats::default();
    debug_assert_eq!(diff::apply(base, a), ours);
    debug_assert_eq!(diff::apply(base, b), theirs);
    let conflicts = match merge::merge3_hunks(base, a, b, !base.is_empty()) {
        Merge::Conflicts(c) => c,
        Merge::Clean(_) => return Err("not a conflicting merge".into()),
    };
    let (a, b) = (a.to_vec(), b.to_vec());
    st.open_end = [base, ours, theirs, result]
        .iter()
        .any(|t| !t.is_empty() && !t.ends_with('\n'));
    // the regions: each conflict widened to lines and to every hunk of either side it meets,
    // until nothing changes; then joined where they meet
    let all: Vec<&Hunk> = a.iter().chain(&b).collect();
    let sides: [(&[Hunk], &str); 2] = [(&a, ours), (&b, theirs)];
    let mut regions: Vec<Range<usize>> = Vec::new();
    for c in &conflicts {
        regions.push(close(base, sides, &all, widen(base, c.base.clone())));
    }
    regions.sort_by_key(|r| r.start);
    let mut joined: Vec<Range<usize>> = Vec::new();
    for r in regions {
        match joined.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => joined.push(r),
        }
    }
    st.regions = joined.len();
    let inside = |h: &Hunk| {
        joined
            .iter()
            .any(|r| h.start >= r.start && h.end <= r.end && meets(h, r))
    };

    // outside the regions: the clean merge's lines; any line text they touch is skipped.
    // Each cluster of outside changes (of either side), folded like a region until no
    // hunk crosses its ends, gives the touched lines of the base and of both sides.
    let outside: Vec<&Hunk> = a.iter().chain(&b).filter(|h| !inside(h)).collect();
    let mut touched: Vec<&str> = Vec::new();
    // (folded with every hunk: a cluster that runs into a region takes it in, and its
    // lines are then only skipped, never judged wrongly)
    let mut clusters: Vec<Range<usize>> = Vec::new();
    for h in &outside {
        clusters.push(close(base, sides, &all, widen(base, h.start..h.end)));
    }
    for r in &clusters {
        touched.extend(lines(&base[r.clone()]).into_iter().map(key));
        for (hs, text) in [(&a, ours), (&b, theirs)] {
            let (s, e) = (map(hs, r.start, false), map(hs, r.end, true));
            touched.extend(lines(&text[s..e]).into_iter().map(key));
        }
    }

    // the expected count of each line text: base lines outside the regions as they are,
    // plus each region's keep-both lines, as a range when copies can't be told apart
    let mut lo: HashMap<&str, usize> = HashMap::new();
    let mut hi: HashMap<&str, usize> = HashMap::new();
    let mut at = 0;
    let mut outside_base: Vec<&str> = Vec::new();
    for r in &joined {
        outside_base.extend(lines(&base[at..r.start]));
        at = r.end;
    }
    outside_base.extend(lines(&base[at..]));
    for (l, n) in count(outside_base) {
        *lo.entry(l).or_insert(0) += n;
        *hi.entry(l).or_insert(0) += n;
    }
    for r in &joined {
        // each side over the region, positionally: its hunks widened to whole lines (a
        // group's base span, and what the side writes over it, whole lines too), groups
        // that meet joined; the base lines either side's groups cover are deleted, and
        // every group's lines are written (an identical group on both sides once: O4)
        let mut deleted = vec![false; lines(&base[r.clone()]).len()];
        let line_of = |p: usize| lines(&base[r.start..p]).len();
        let mut written: Vec<Vec<(Range<usize>, String)>> = Vec::new();
        for hs in [&a, &b] {
            // each group: its range, and its own hunks
            let mut groups: Vec<(Range<usize>, Vec<&Hunk>)> = Vec::new();
            for h in hs.iter().filter(|h| h.start >= r.start && h.end <= r.end) {
                let s = base[..h.start].rfind('\n').map_or(0, |i| i + 1);
                let mut e = h.end;
                let mut tail = format!("{}{}", &base[s..h.start], h.insert);
                loop {
                    // a line start (the text's start too) or the text's end
                    let boundary = e == 0 || e == base.len() || base.as_bytes()[e - 1] == b'\n';
                    if boundary && (tail.is_empty() || tail.ends_with('\n') || e == base.len()) {
                        break;
                    }
                    let next = base[e..].find('\n').map_or(base.len(), |k| e + k + 1);
                    tail.push_str(&base[e..next]);
                    e = next;
                }
                // groups that overlap are one; groups of different lines that only meet at
                // a line start stay apart (one point holds one insertion), as the kit's
                // whole-line sides do, so a line changed identically on both sides is one
                // change (O4)
                match groups.last_mut() {
                    Some((g, own)) if s < g.end || (g.start == g.end && s == e && s == g.start) => {
                        g.end = g.end.max(e);
                        own.push(h);
                    }
                    _ => groups.push((s..e, vec![h])),
                }
            }
            let mut side = Vec::new();
            for (g, own) in groups {
                let in_g: Vec<Hunk> = own
                    .iter()
                    .map(|h| Hunk {
                        start: h.start - g.start,
                        end: h.end - g.start,
                        insert: h.insert.clone(),
                    })
                    .collect();
                let text = diff::apply(&base[g.clone()], &in_g);
                for d in &mut deleted[line_of(g.start)..line_of(g.end)] {
                    *d = true;
                }
                side.push((g, text));
            }
            written.push(side);
        }
        let mut region: Vec<String> = lines(&base[r.clone()])
            .into_iter()
            .zip(&deleted)
            .filter(|(_, d)| !**d)
            .map(|(l, _)| l.to_owned())
            .collect();
        for (g, text) in &written[0] {
            if written[1].iter().any(|(g2, t2)| g2 == g && t2 == text) {
                st.o4 += 1;
                continue; // O4: taken once
            }
            region.extend(lines(text).into_iter().map(str::to_owned));
        }
        for (_, text) in &written[1] {
            region.extend(lines(text).into_iter().map(str::to_owned));
        }
        for l in &region {
            let k = key(l);
            // a key that outlives this loop: the same line in one of the three texts
            let k = [base, ours, theirs]
                .iter()
                .flat_map(|t| lines(t))
                .map(key)
                .find(|x| *x == k)
                .unwrap_or_else(|| panic!("a written line from none of the texts: {k:?}"));
            *lo.entry(k).or_insert(0) += 1;
            *hi.entry(k).or_insert(0) += 1;
        }
    }
    // (iii) first: every line of the result is a whole line of one of the three texts: no
    // splice, no join, inside the regions or out (whole-line sides never share a line)
    no_splice_no_join(base, ours, theirs, result)?;
    let got = count(lines(result));
    let mut keys: Vec<&str> = lo.keys().chain(got.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    for l in keys {
        if touched.contains(&l) {
            st.skipped += 1;
            continue;
        }
        let (lo, hi, n) = (
            lo.get(l).copied().unwrap_or(0),
            hi.get(l).copied().unwrap_or(0),
            got.get(l).copied().unwrap_or(0),
        );
        st.exact += 1;
        if n < lo {
            return Err(format!(
                "(i) a line either side wrote is missing: {l:?} {n} times, want at least {lo}"
            ));
        }
        if n > hi {
            return Err(format!(
                "(ii) a line either side deleted is still there: {l:?} {n} times, want at most {hi}"
            ));
        }
    }
    Ok(st)
}

/// An independent check of a side as the kit takes it (`outside_sides`) against the side
/// diff it is (`diff::whole_line_hunks`, v0.9.7): the hunks are valid and give the same text;
/// **every hunk is whole lines** (checked here, not by the kit's predicate: it starts at a
/// line start, ends at a line start or at the end of the text, and its text ends with "\n"
/// unless it runs to the end), so two sides' changes can never share a line; and each one
/// that differs from its counterpart in the diff is the same pure insertion or pure deletion
/// slid along the text (the same result over the span it moved across, so only through a
/// run of equal text) to a line start.
pub fn check_aligned(base: &str, side: &str, aligned: &[Hunk]) -> Result<(), String> {
    diff::validate(base, aligned).map_err(|e| format!("aligned hunks invalid: {e}"))?;
    if diff::apply(base, aligned) != side {
        return Err("aligned hunks give another text".into());
    }
    let line_start = |p: usize| p == 0 || base.as_bytes()[p - 1] == b'\n';
    for h in aligned {
        let end_ok = h.end == base.len() || line_start(h.end);
        let text_ok = h.insert.is_empty() || h.insert.ends_with('\n') || h.end == base.len();
        if !line_start(h.start) || !end_ok || !text_ok {
            return Err(format!("a hunk that isn't whole lines: {h:?}"));
        }
        // no line the side kept is read as part of its change: a hunk's first and last line
        // differ from what it writes there (else that line would come back where the other
        // side deleted it)
        let (old, new) = (lines(&base[h.start..h.end]), lines(&h.insert));
        if !old.is_empty()
            && !new.is_empty()
            && (old[0] == new[0] || old[old.len() - 1] == new[new.len() - 1])
        {
            return Err(format!("a hunk holds a line its side kept: {h:?}"));
        }
    }
    let raw = diff::whole_line_hunks(base, side);
    // the reference is the kit's own side diff: it must round-trip on its own, so a wrong
    // diff can't pass silently (check_sides stays the judge of the merge)
    diff::validate(base, &raw).map_err(|e| format!("whole_line_hunks invalid: {e}"))?;
    if diff::apply(base, &raw) != side {
        return Err("whole_line_hunks(base, side) applied to base doesn't give side".into());
    }
    if raw.len() != aligned.len() {
        return Err(format!(
            "aligned {} hunks, the diff {}",
            aligned.len(),
            raw.len()
        ));
    }
    for (r, s) in raw.iter().zip(aligned) {
        if r == s {
            continue;
        }
        let same_kind = (r.is_insertion() && s.is_insertion() && r.insert.len() == s.insert.len())
            || (r.insert.is_empty() && s.insert.is_empty() && r.end - r.start == s.end - s.start);
        if !same_kind {
            return Err(format!("a hunk changed kind when aligned: {r:?} -> {s:?}"));
        }
        if !line_start(s.start) {
            return Err(format!(
                "a hunk slid, but not to a line start: {r:?} -> {s:?}"
            ));
        }
        // the span it moved across reads the same either way
        let (lo, hi) = (r.start.min(s.start), r.end.max(s.end));
        let local = |h: &Hunk| format!("{}{}{}", &base[lo..h.start], h.insert, &base[h.end..hi]);
        if local(r) != local(s) {
            return Err(format!(
                "a hunk slid across text that isn't a run of it: {r:?} -> {s:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "a\nb\nc\nd\n";

    fn sides(base: &str, ours: &str, theirs: &str) -> (Vec<Hunk>, Vec<Hunk>) {
        let cb = closed_text(base);
        (
            diff::whole_line_hunks(&cb, &closed_text(ours)),
            diff::whole_line_hunks(&cb, &closed_text(theirs)),
        )
    }

    #[test]
    fn aligned_sides_are_checked() {
        let base = "a\nb\n";
        let side = "a\nx\nb\n";
        // the diff inserts "x\n" after "a\n" (a line start already)
        check_aligned(base, side, &diff::whole_line_hunks(base, side)).unwrap();
        // slid along a run of equal text: "b\n" inserted after "b\n" is "b\n" after "a\n"
        let side2 = "a\nb\nb\n";
        let at_line = [Hunk {
            start: 2,
            end: 2,
            insert: "b\n".into(),
        }];
        check_aligned(base, side2, &at_line).unwrap();
        // a slide off a line start, and a slide across other text, are refused
        let off = [Hunk {
            start: 1,
            end: 1,
            insert: "\nb".into(),
        }];
        assert!(check_aligned(base, side2, &off).is_err());
        let wrong = [Hunk {
            start: 0,
            end: 0,
            insert: "x\n".into(),
        }];
        assert!(check_aligned(base, side, &wrong).is_err());
        // a hunk inside a line (v0.9.5's sides) isn't whole lines: refused
        let in_line = [Hunk {
            start: 2,
            end: 3,
            insert: "x".into(),
        }];
        let e = check_aligned(base, "a\nx\n", &in_line).unwrap_err();
        assert!(e.contains("isn't whole lines"), "{e}");
    }

    #[test]
    fn a_side_ending_mid_line_takes_in_the_next_line() {
        // theirs rewrites the first line and joins it to the empty second line; ours
        // deletes a word of the first: by line, theirs wrote "Z\n" over both lines
        let base = "w x\n\n";
        let (ours, theirs) = (" x\n\n", "Z\n");
        check(base, ours, theirs, "Z\n x\n").unwrap();
    }

    #[test]
    fn an_open_last_line_matches_the_same_line_closed() {
        // both sides rewrite the last line, which has no break: kept both, one of them
        // needs a break after it; a splice of the two is still caught
        let base = "a\nb";
        let (ours, theirs) = ("a\nB1", "a\nB2");
        check(base, ours, theirs, "a\nB2\nB1").unwrap();
        let e = check(base, ours, theirs, "a\nB2B1").unwrap_err();
        assert!(e.starts_with("(iii) two lines joined"), "{e}");
    }

    #[test]
    fn regions_end_on_char_boundaries() {
        // the next line starts with a multi-byte character
        let base = "w x\n日\n";
        check(base, " x\n日\n", "Z日\n", "Z日\n x\n").ok();
        check(base, "w\n日\n", "w x日\n", "w x日\nw\n").ok();
    }

    #[test]
    fn a_line_inserted_before_a_rewritten_one_is_clean() {
        // theirs inserts "aab" before the last line "a", ours rewrites that line: by line
        // (v0.9.7) they meet at a line start, a clean merge, and no line is spliced
        let base = "x\na\na\n";
        let (ours, theirs) = ("x\na\na{{\n", "x\na\naab\na\n");
        let e = check(base, ours, theirs, "x\na\naab\na{{\n").unwrap_err();
        assert_eq!(e, "not a conflicting merge");
        let (a, b) = sides(base, ours, theirs);
        let got = clean_outside(base, ours, theirs, &a, &b).unwrap();
        assert_eq!(got, "x\na\naab\na{{\n");
        no_splice_no_join(base, ours, theirs, &got).unwrap();
    }

    #[test]
    fn a_side_joining_lines_is_one_change_of_those_lines() {
        // ours deletes across lines 1-2 (joining them onto line 3) and rewrites line 4;
        // theirs rewrites line 4 too: by line, ours rewrote lines 1-3 as "aef", and both
        // rewrites of line 4 are kept
        let base = "ab\ncd\nef\ngh\n";
        let (ours, theirs) = ("aef\nG1\n", "ab\ncd\nef\nG2\n");
        check(base, ours, theirs, "aef\nG2\nG1\n").unwrap();
    }

    #[test]
    fn whole_lines_inserted_at_the_start_delete_nothing() {
        // theirs inserts two whole lines at the very start; ours rewrites the empty first
        // line: by line they meet at the text's start, a clean merge (v0.9.7)
        let base = "\nZ\n";
        let (ours, theirs) = ("PPPP\nZ\n", "line 3\n\n\nZ\n");
        let e = check(base, ours, theirs, "line 3\n\nPPPP\nZ\n").unwrap_err();
        assert_eq!(e, "not a conflicting merge");
        let (a, b) = sides(base, ours, theirs);
        assert_eq!(
            clean_outside(base, ours, theirs, &a, &b).as_deref(),
            Some("line 3\n\nPPPP\nZ\n")
        );
    }

    /// [`clean_outside`] on the closed texts' sides.
    fn clean(base: &str, ours: &str, theirs: &str) -> Option<String> {
        let (a, b) = sides(base, ours, theirs);
        clean_outside(base, ours, theirs, &a, &b)
    }

    #[test]
    fn a_clean_outside_merge_ends_as_its_last_lines_author_left_it() {
        // the live side rewrites the last line without a break; the file appends a line:
        // the file's line is last, closed (O5a's case A)
        assert_eq!(clean("b\n", "b\nZ\n", "a").as_deref(), Some("a\nZ\n"));
        // both append at the end: theirs first, then ours, open as ours left it (O5a's B
        // and D)
        assert_eq!(clean("x\n", "x\nO", "x\nT").as_deref(), Some("x\nT\nO"));
        assert_eq!(clean("x\n", "x\nB\n", "x\nA").as_deref(), Some("x\nA\nB\n"));
        // ours removed the final break, theirs appended a line: theirs' line is last,
        // closed (O5a's C)
        assert_eq!(clean("x\n", "x", "x\nT\n").as_deref(), Some("x\nT\n"));
        // the last line is a base line both kept: the side that changed the ending decides
        assert_eq!(clean("x\ny\n", "x\ny", "X\ny\n").as_deref(), Some("X\ny"));
        assert_eq!(clean("x\ny", "x\ny\n", "X\ny").as_deref(), Some("X\ny\n"));
        // a deleted last line never comes back because the other side only closed it, or
        // wrote lines after it
        assert_eq!(clean("[", "", "[\n").as_deref(), Some(""));
        assert_eq!(clean("a\nb", "a\nb\nc", "a\n").as_deref(), Some("a\nc"));
        // both sides' changes to one line: not clean
        assert_eq!(clean("X", "", "\u{6}XA\u{12}"), None);
        assert_eq!(clean(")", ")12", ")34"), None);
        let base = "ab\ncd\n";
        assert_eq!(clean(base, "aX\ncd\n", "aby\ncd\n"), None);
        // changes of two lines in the middle of a text: each line from one side
        assert_eq!(
            clean(base, "aX\ncd\n", "ab\ncY\n").as_deref(),
            Some("aX\ncY\n")
        );
    }

    #[test]
    fn the_lcs_check_decides_by_every_alignment() {
        // the open-end class and the kept-blank-line class: resurrections under every
        // alignment
        assert!(check_lcs("[", "", "[\n", "[\n").is_err());
        check_lcs("[", "", "[\n", "").unwrap();
        assert!(check_lcs("\n", "", "ab\n\naaaaaa", "ab\n\naaaaaa").is_err());
        check_lcs("\n", "", "ab\n\naaaaaa", "ab\naaaaaa").unwrap();
        // a move one side made, the other deleting the line: the moved copy may be kept
        // (an alignment reads it as added)
        check_lcs("a\nb\n", "b\na\n", "b\n", "b\na\n").unwrap();
        check_lcs("a\nb\n", "b\na\n", "b\n", "b\n").unwrap();
        // a lost line and a line twice
        assert!(check_lcs("a\nb\n", "a\nB1\n", "a\nB2\n", "a\nB2\n").is_err());
        assert!(check_lcs("a\nb\n", "a\nB1\n", "a\nB2\n", "a\nB2\nB1\nB1\n").is_err());
        // outside_edit input 707526724baa (v0.9.7 at 3fc39fa): blank lines both sides
        // rewrote around; decided by the rule (ours keeps 3 to 4 of the 6 blank lines
        // under its alignments, theirs none)
        let base = "~U???\u{1f}?-?b\nb\nb\nb\nb\nx y\nab\naéab\nééééééééeééééééé日日日日日aababab日日\n\n\n\n\n\n\nńééééééééééé y\nb\nb\nb\nx y\nline 3\n";
        let ours = "~U???\u{1f}?ab\naéééline 3\nline 3\n\nab\n\n\n\nline 3\nline 3\né日ab\nab\néééééééééé y\nb\nb\nb\nx y\nline 3\n";
        let theirs = "~U???\u{1f}?-?b\nb\nb\nb\nb\nx y\nab\naéab\néééééééééééééééééééé y\nb\nb\nb\nx y\nline 3";
        let got = merge::merge3_keep_both(base, ours, theirs).text;
        check_lcs(base, ours, theirs, &got).unwrap();
    }

    #[test]
    fn a_deleted_line_back_or_a_lost_line_fails() {
        // the open-end class: ours deletes the last line, theirs only closes it
        let (base, ours, theirs) = ("[", "", "[\n");
        let (a, b) = sides(base, ours, theirs);
        let e = check_lines(base, &a, &b, "[\n").unwrap_err();
        assert!(e.starts_with("a deleted line back"), "{e}");
        check_lines(base, &a, &b, "").unwrap();
        // ours deletes "aa" (writes "a"), theirs appends a line after it
        let (base, ours, theirs) = ("aa", "a", "aa\naaaa");
        let (a, b) = sides(base, ours, theirs);
        assert!(check_lines(base, &a, &b, "aa\naaaa\na").is_err());
        check_lines(base, &a, &b, "a\naaaa").unwrap();
        // a lost line
        let (a, b) = sides(BASE, "a\nB1\nc\nd\n", "a\nB2\nc\nd\n");
        let e = check_lines(BASE, &a, &b, "a\nB2\nc\nd\n").unwrap_err();
        assert!(e.starts_with("a line lost"), "{e}");
    }

    #[test]
    fn a_join_across_a_removed_line_break_fails() {
        // a conflict at the top; below it, ours turns an empty line into "aa" (joining it
        // onto "aaa") and theirs types at the start of "aaa": v0.9.5 merged that clean, char
        // by char, into "aaTaaa"; by line (v0.9.7) both versions of the line are kept
        let base = "p\nq\n\naaa";
        let (ours, theirs) = ("P1\nq\naaaaa", "P2\nq\n\nTaaa");
        let e = check(base, ours, theirs, "P2\nP1\nq\naaTaaa").unwrap_err();
        assert!(e.starts_with("(iii) a line spliced"), "{e}");
        check(base, ours, theirs, "P2\nP1\nq\nTaaa\naaaaa").unwrap();
    }

    #[test]
    fn appends_to_one_open_line_are_never_spliced() {
        // both rewrite the one open line (a conflict, kept both): ours as "L" and "aO",
        // theirs as "" and "aT"; their appends never share a line ("aTO")
        let base = "a";
        let (ours, theirs) = ("L\naO", "\naT");
        let e = check(base, ours, theirs, "L\n\naTO").unwrap_err();
        assert!(e.starts_with("(iii)"), "{e}");
        check(base, ours, theirs, "\naT\nL\naO").unwrap();
    }

    #[test]
    fn an_append_to_an_open_last_line_is_a_change_of_that_line() {
        // a base without a final break: ours inserts a line at the start and appends to the
        // end; theirs rewrites the one line: by line (v0.9.7), both versions of it
        let base = "a";
        let (ours, theirs) = ("L\naX", "\naaaa");
        let e = check(base, ours, theirs, "L\n\naaaaX").unwrap_err();
        assert!(e.starts_with("(iii)"), "{e}");
        check(base, ours, theirs, "\naaaa\nL\naX").unwrap();
    }

    #[test]
    fn a_join_of_two_whole_lines_fails() {
        // the shape v0.9.5 left (the COSMETIC class, no longer tolerated): "z" and "bab"
        // share a line
        let (base, ours, theirs) = ("\n\n", "x\ny\nz\n", "E\n\nbab");
        let e = no_splice_no_join(base, ours, theirs, "E\nx\ny\nzbab").unwrap_err();
        assert!(e.starts_with("(iii) two lines joined"), "{e}");
        assert!(check(base, ours, theirs, "E\nx\ny\nzbab").is_err());
        check(base, ours, theirs, "E\nbab\nx\ny\nz\n").unwrap();
    }

    #[test]
    fn a_line_glued_to_the_other_sides_typing_fails() {
        // crash-58c861b8 (the keep-both join class, on main with or without the
        // shifted-pairing fix): the file's "aaaaaaaaaaaaa" glued onto the live "lo"
        let base = "line 3\néline 3\nline 3\nline 3\naaline 3\n\n\n\n";
        let ours = "ab\naéline 3\nline 3\naaaaaaaaaaaaa\n";
        let theirs = "\u{1}line 3\nloéline 3\nline 3\nline 3\naaline 3\n\n\n\n";
        let glued = "ab\naéline 3\n\u{1}line 3\nloaaaaaaaaaaaaa\n";
        assert!(no_splice_no_join(base, ours, theirs, glued).is_err());
        let got = merge::merge3_keep_both(base, ours, theirs).text;
        no_splice_no_join(base, ours, theirs, &got).unwrap();
        assert!(check(base, ours, theirs, glued).is_err());
        check(base, ours, theirs, &got).unwrap();
    }

    #[test]
    fn a_line_changed_identically_on_both_sides_is_one_change() {
        // both rewrite line 1 as "X" and line 2 differently: the kit's whole-line sides keep
        // the two lines' hunks apart, so line 1 is one change, taken once (O4), and line 2 is
        // kept both (the shape of 19 keep_both corpus inputs)
        let base = "x\ny\n";
        let (ours, theirs) = ("X\nO\n", "X\nT\n");
        let got = merge::merge3_keep_both(base, ours, theirs).text;
        assert_eq!(got, "X\nT\nO\n");
        let st = check(base, ours, theirs, &got).unwrap();
        assert_eq!(st.o4, 1);
        assert!(check(base, ours, theirs, "X\nT\nX\nO\n").is_err());
    }

    #[test]
    fn keeping_both_sides_passes() {
        // both rewrite line b: keep both, at one point (the live side first)
        let ours = "a\nB1\nc\nd\n";
        let theirs = "a\nB2\nc\nd\n";
        let st = check(BASE, ours, theirs, "a\nB2\nB1\nc\nd\n").unwrap();
        assert_eq!(st.regions, 1);
        // either order of the two sides' lines satisfies (i)-(iii)
        check(BASE, ours, theirs, "a\nB1\nB2\nc\nd\n").unwrap();
    }

    #[test]
    fn a_lost_side_fails_i() {
        let e = check(BASE, "a\nB1\nc\nd\n", "a\nB2\nc\nd\n", "a\nB2\nc\nd\n").unwrap_err();
        assert!(e.starts_with("(i)"), "{e}");
    }

    #[test]
    fn a_kept_deleted_line_fails_ii() {
        // ours deletes b and c, theirs rewrites c: b must go
        let e = check(BASE, "a\nd\n", "a\nb\nC\nd\n", "a\nb\nC\nd\n").unwrap_err();
        assert!(e.starts_with("(ii)"), "{e}");
        check(BASE, "a\nd\n", "a\nb\nC\nd\n", "a\nC\nd\n").unwrap();
    }

    #[test]
    fn a_spliced_line_fails() {
        // a char-level splice of the two rewrites of b
        let e = check(BASE, "a\nbX\nc\nd\n", "a\nbY\nc\nd\n", "a\nbYX\nc\nd\n").unwrap_err();
        assert!(e.starts_with("(iii) a line spliced"), "{e}");
    }

    #[test]
    fn two_sides_on_one_line_elsewhere_keep_both() {
        // a conflict on b, and both sides editing line d at different places: v0.9.5 merged
        // d char by char ("XdY", skipped by this check); by line (v0.9.7) both versions of d
        // are kept, and a splice fails
        let ours = "a\nB1\nc\nXd\n";
        let theirs = "a\nB2\nc\ndY\n";
        let e = check(BASE, ours, theirs, "a\nB2\nB1\nc\nXdY\n").unwrap_err();
        assert!(e.starts_with("(iii) a line spliced"), "{e}");
        let st = check(BASE, ours, theirs, "a\nB2\nB1\nc\ndY\nXd\n").unwrap();
        assert_eq!(st.regions, 2);
    }

    #[test]
    fn repeated_lines_count_as_multisets() {
        // two copies of x; both sides rewrite the second one
        let st = check("x\nx\n", "x\nq\n", "x\nr\n", "x\nr\nq\n").unwrap();
        assert!(st.exact > 0);
        let e = check("x\nx\n", "x\nq\n", "x\nr\n", "x\nx\nr\nq\n").unwrap_err();
        assert!(e.starts_with("(ii)"), "{e}");
    }

    #[test]
    fn copies_deleted_on_both_sides_are_found_by_position() {
        // two copies of x around m: ours rewrites "x m" (the first x), theirs "m x" (the
        // second): by line, both copies of x and m are gone, both rewrites land
        let base = "x\nm\nx\n";
        let (ours, theirs) = ("M1\nx\n", "x\nM2\n");
        check(base, ours, theirs, "M2\nM1\n").unwrap();
        let e = check(base, ours, theirs, "x\nM2\nM1\n").unwrap_err();
        assert!(e.starts_with("(ii)"), "{e}");
    }

    /// The kit's own merge on the shapes above, and on the open-end class: the whole oracle
    /// passes.
    #[test]
    fn the_kits_merge_passes_on_these_shapes() {
        for (base, ours, theirs) in [
            ("\n\n", "x\ny\nz\n", "E\n\nbab"),
            ("p\nq\n\naaa", "P1\nq\naaaaa", "P2\nq\n\nTaaa"),
            ("a", "L\naO", "\naT"),
            ("a", "L\naX", "\naaaa"),
            (BASE, "a\nB1\nc\nXd\n", "a\nB2\nc\ndY\n"),
            ("\n[\n", "\u{15}@\n[\n\n\n", "[\n"),
            ("X\nL\ne\nL\n", "L\ne \nL\n", "X\n\nL\n"),
            // the keep-both join class (crash-58c861b8): "lo" + "aaaaaaaaaaaaa" in v0.9.5
            (
                "line 3\néline 3\nline 3\nline 3\naaline 3\n\n\n\n",
                "ab\naéline 3\nline 3\naaaaaaaaaaaaa\n",
                "\u{1}line 3\nloéline 3\nline 3\nline 3\naaline 3\n\n\n\n",
            ),
            // the open-end class (the keep_both corpus)
            ("[", "", "[\n"),
            ("aa", "a", "aa\naaaaaaaaaaaaaaaaaaaaaaa"),
            ("aa", "a\u{1c}a*", "aa\naaaaaaaaaaaaaaaaaaaaaaa"),
            ("ab", ";\u{4}", "aline 3\nab\naaaaaaa"),
            ("\u{8}", "", "a\n\u{8}\n\t"),
            ("line 3\nab", "aab\nline 3\nab\naaaa", ""),
            ("[", "\n[\n", ""),
            ("éa", "a", "éa\naéa"),
            ("a\nb", "a\nb\nc", "a\n"),
            // the 30-minute outside_edit run's crash-f81590cf (the pre-9504c40 oracle's O4
            // grouping): the merge at its failing step
            (
                "b\nb\nb\nb\na\na\nb\nx y\nx y\nb\nb\nb\n",
                "b\nb\nb\n 日ab\nab\nab\n\nabe\u{301}aaabab e\u{301}ae\u{301}line 3\ne\u{301}line 3\na ab\nab\nab\n\naa日e\u{301}aaa\n",
                "aaab\n🎉🎉🎉🎉🎉🎉🎉🎉ab\ne\u{301}e\u{301}ab\nabababab\nab\nab\nab\nPPPPPPab\nab\nababab\nae\u{301}e\u{301}e\u{301}line 3\nline 3\n\n",
            ),
            // O5a's cases
            ("b\n", "b\nZ\n", "a"),
            ("x\n", "x\nB\n", "x\nA"),
            ("x\n", "x", "x\nT\n"),
            ("x\n", "x\nB", "x\nA"),
            ("x\n", "x\nB", "x\nA\n"),
        ] {
            let got = merge::merge3_keep_both(base, ours, theirs).text;
            let (a, b) = merge::outside_sides(base, ours, theirs);
            check_outside(base, ours, theirs, &a, &b, &got)
                .unwrap_or_else(|e| panic!("{e}: {base:?} {ours:?} {theirs:?} -> {got:?}"));
        }
    }
}
