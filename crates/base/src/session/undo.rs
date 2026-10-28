//! Undo, redo and revert over text, to the undo spec v1.9 (`docs/SPEC-UNDO.md`).
//!
//! Undo is a three-way merge per **region** (U8): a region is one **canonical hunk** of a
//! change (sorted, non-overlapping, coalesced when touching), kept as its text before and
//! after the change. A `sed -i` touching lines 3 and 300 is two regions. Each region's
//! bounds are carried through every later change to its root, by anyone, like a caret: an
//! insertion exactly at its start or end is pulled inside it (start [`Bias::Before`], end
//! [`Bias::After`]). Two regions of one change can come to touch (someone deleted what
//! separated them); an insertion at their shared edge then goes to the **left** region
//! only (m-d). A later deletion across a region's edge **taints** it (U9): a tainted region
//! always conflicts.
//!
//! Undoing a region merges two **exact hunk sets** on its base (the text the change left,
//! `after`), never diffs of strings (U10, v1.7): **ours**, the inverse of the change's own
//! hunk (as made), and **theirs**, every later hunk that touched the region's carried
//! window, composed in base coordinates ([`super::foreign`]). `at_end` holds only when the
//! region ends at the root's end **and** the root was non-empty after the change being
//! undone (the merge's base, U10, v1.4 m11), so the end-of-text rule O5 fires exactly where
//! a whole-text merge would fire it (U12). A clean region applies **the merge's own hunks**
//! (offset by `s'`). In repeated text a diff can't tell another participant's identical
//! element from the region's own; exact sets can, so undo, redo and revert never remove an
//! element they didn't write (finding F3, `hist.undo_own_only`).
//!
//! **By root** (v1.8, U8, U23): every element has a provenance root ([`super::prov`]), and a
//! region keeps the roots of its base's elements (`after`), of `before`'s, and of what each
//! ours hunk inserts. Theirs is read off the live window by root: scanning it left to right,
//! an element whose root is base element `i`'s counts as base element `i` when `i` is past
//! the last base element counted; every other element is foreign. So text someone removed
//! and put back (their undo, redo or revert) is the base's again, and an undo removes only
//! elements whose root the change it reverses inserted (U10a). O4 takes a hunk once when
//! both sides make it with the same range **and the same roots** (a fresh `X` typed where
//! the undo would put `X` back is not that `X`); O4a refuses (conflict, `Overlap`) to put
//! back a root that is live, so at most one live element has any root (I1). The v1.7
//! composition at element identity ([`super::foreign`]) stays as a debug cross-check (every
//! base element it says survives counts by root), and is the merge for a region whose
//! roots are unknown (a revert record written before store format 5): that region behaves
//! exactly as in v1.7, and what it puts back are fresh elements.
//!
//! Within one step, regions are applied **back to front**; the windows are disjoint, so each
//! region merges against the step's starting text and lower regions keep their bounds
//! (U10, m-d). A plan carries the regions of the later steps through each planned step, and
//! applies each planned step to a scratch copy of the texts and of the provenance map, so a
//! later step reads its windows' roots, and O4a the live roots, as they will be.
//!
//! Modes (U13–U15): all-or-nothing reports **every** conflicting region of the first
//! conflicting step and applies nothing; `--partial` applies every clean region, reports the
//! rest (the same set), and stops at a step whose every region conflicts (it stays on top).

use std::collections::HashMap;
use std::ops::Range;

use super::foreign::{self, Shape};
use super::position::{self, transform, Bias};
use super::prov::{coalesce_with, ElemId, IdRun, Ids, Inserted, Prov};
use crate::doc::Edit;
use crate::store::Seq;
use crate::text::diff::{self, Hunk};
use crate::text::merge::{self, Merge};

/// Why a region of an undo, redo or revert conflicts (U11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConflictKind {
    /// Someone else changed text overlapping the region since (merge rules O1, O2, O5).
    Overlap,
    /// A later deletion crossed one of the region's edges (U9).
    Taint,
    /// The region overlaps content a purge redacted (revert only; v1.3 m-g).
    Erased,
}

/// One conflicting region of an undo, redo or revert, in **live** coordinates (U11): one
/// entry per region (= per canonical hunk of the change), never a sub-range of one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConflictRegion {
    pub root: String,
    /// The region's live byte range `[s', e')`.
    pub range: Range<usize>,
    /// 1-based live lines (first, last), inclusive.
    pub lines: (usize, usize),
    pub kind: ConflictKind,
    /// The region's text as the change left it (the merge's base).
    pub base: String,
    /// What the undo wanted to put back.
    pub ours: String,
    /// The live text of the region now.
    pub theirs: String,
}

/// One region of a change: what to turn back (`after` into `before`) and where it is now.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub root: String,
    /// Bounds in the current text (carried through every change).
    pub start: usize,
    pub end: usize,
    /// The text to restore.
    pub before: String,
    /// The text the change left (the merge's base).
    pub after: String,
    /// Ours: hunks on `after` that give `before` back, exactly as the change made them
    /// inverted (a change's region: one hunk over all of `after`; an undo's or redo's
    /// region: the inverse of the hunks it applied).
    pub ours: Vec<Hunk>,
    /// Theirs: every later hunk that touched the window, composed, in `after`'s
    /// coordinates (empty once tainted: a tainted region conflicts whatever they are).
    pub theirs: Vec<Shape>,
    pub tainted: bool,
    /// The root was empty right after the change (the whole doc's merge base): the
    /// end-of-text rule O5 then never applies (U10, v1.4 m11).
    pub base_doc_empty: bool,
    /// The region overlaps redacted content (revert records after a purge).
    pub erased: bool,
    /// The provenance roots of its texts (v1.8); `None`: unknown (a revert record written
    /// before store format 5), and the region merges as in v1.7.
    pub roots: Option<RegionRoots>,
}

/// A region's provenance roots (undo spec v1.8, U23), each aligned with its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegionRoots {
    /// The roots of the base's elements (`after`: what the change left).
    pub after: Ids,
    /// The roots of `before`'s elements (what the change removed).
    pub before: Ids,
    /// Per ours hunk, the roots of the elements it puts back (aligned with its insert).
    pub ours: Vec<Ids>,
}

impl RegionRoots {
    fn mem(&self) -> usize {
        self.after.mem() + self.before.mem() + self.ours.iter().map(Ids::mem).sum::<usize>()
    }
}

/// A clean region: the merge, and per clean hunk the roots of what it puts back (`None`:
/// the region's roots are unknown, and its insertions are fresh elements).
#[derive(Debug, Clone)]
pub(crate) struct Merged {
    pub clean: merge::Clean,
    pub roots: Option<Vec<Ids>>,
}

/// Do two root runs share a root?
fn meets(a: &IdRun, b: &IdRun) -> bool {
    a.start.client == b.start.client
        && a.start.clock < b.start.clock + b.units
        && b.start.clock < a.start.clock + a.units
}

/// Theirs read **by root** (U8, v1.8): the live `window`'s elements (roots `wroots`, one per
/// character) matched to the base `after` (roots `base`, one per character). Scanning the
/// window left to right, an element whose root is base element `i`'s counts as base element
/// `i` when `i` is greater than the last base index counted; every other element is
/// foreign. Returns theirs as canonical hunks on `after` (D2's form), the roots each one
/// inserts, and which base elements counted.
fn theirs_by_root(
    after: &str,
    base: &[ElemId],
    window: &str,
    wroots: &[ElemId],
) -> (Vec<Hunk>, Vec<Ids>, Vec<bool>) {
    let bytes: Vec<usize> = after.char_indices().map(|(i, _)| i).collect();
    debug_assert_eq!(bytes.len(), base.len());
    let at_byte = |i: usize| bytes.get(i).copied().unwrap_or(after.len());
    // base roots are distinct: the base's elements were all live at once (I1)
    let index: HashMap<ElemId, usize> = base.iter().enumerate().map(|(i, r)| (*r, i)).collect();
    debug_assert_eq!(index.len(), base.len(), "I1: a base holds a root twice");
    let mut counted = vec![false; base.len()];
    let (mut hunks, mut roots) = (Vec::new(), Vec::new());
    let (mut next, mut last): (usize, Option<usize>) = (0, None);
    let (mut text, mut ids) = (String::new(), Vec::new());
    let mut flush = |upto: usize, text: &mut String, ids: &mut Vec<ElemId>, next: usize| {
        if upto > next || !text.is_empty() {
            let insert = std::mem::take(text);
            roots.push(Ids::from_chars(&insert, ids));
            ids.clear();
            hunks.push(Hunk {
                start: at_byte(next),
                end: at_byte(upto),
                insert,
            });
        }
    };
    for (ch, r) in window.chars().zip(wroots) {
        match index.get(r) {
            Some(&i) if last.map_or(true, |l| i > l) => {
                debug_assert_eq!(
                    after[at_byte(i)..].chars().next(),
                    Some(ch),
                    "an element counted by root holds its root's character"
                );
                flush(i, &mut text, &mut ids, next);
                counted[i] = true;
                (next, last) = (i + 1, Some(i));
            }
            _ => {
                text.push(ch);
                ids.push(*r);
            }
        }
    }
    flush(base.len(), &mut text, &mut ids, next);
    (hunks, roots, counted)
}

impl Region {
    /// One region per canonical hunk of a change: `hunks` (sorted, valid against
    /// `before_text`) applied to `before_text`. Touching hunks are coalesced first. Bounds
    /// are in the coordinates of the text after the change. `roots`: per hunk (already
    /// canonical), the roots of what it deleted and of what it inserted (v1.8).
    pub fn of_change(
        root: &str,
        before_text: &str,
        hunks: &[Hunk],
        roots: Option<Vec<(Ids, Ids)>>,
    ) -> Vec<Region> {
        let hunks = diff::coalesce(hunks.to_vec());
        debug_assert!(roots.as_ref().map_or(true, |r| r.len() == hunks.len()));
        let mut roots = roots.map(Vec::into_iter);
        let grow: isize = hunks
            .iter()
            .map(|h| h.insert.len() as isize - (h.end - h.start) as isize)
            .sum();
        let empty_after = (before_text.len() as isize + grow) == 0;
        let mut delta: isize = 0;
        hunks
            .into_iter()
            .map(|h| {
                let start = (h.start as isize + delta) as usize;
                delta += h.insert.len() as isize - (h.end - h.start) as isize;
                let before = before_text[h.start..h.end].to_owned();
                let roots = roots.as_mut().and_then(Iterator::next).map(|(b, a)| {
                    debug_assert_eq!((b.bytes(), a.bytes()), (before.len(), h.insert.len()));
                    RegionRoots {
                        after: a,
                        ours: vec![b.clone()],
                        before: b,
                    }
                });
                Region {
                    root: root.to_owned(),
                    start,
                    end: start + h.insert.len(),
                    ours: vec![Hunk {
                        start: 0,
                        end: h.insert.len(),
                        insert: before.clone(),
                    }],
                    theirs: Vec::new(),
                    before,
                    after: h.insert,
                    tainted: false,
                    base_doc_empty: empty_after,
                    erased: false,
                    roots,
                }
            })
            .collect()
    }

    /// Carry the region through a later change to its root (U8, U9): its bounds, its taint,
    /// and the hunks inside its window composed into theirs.
    pub fn transform(&mut self, hunks: &[Hunk]) {
        let c = foreign::carry_window(
            self.start,
            self.end,
            &foreign::shapes(hunks),
            &mut self.theirs,
            self.after.len(),
            !self.tainted,
        );
        if c.start || c.end {
            self.tainted = true;
        }
        let s = transform(self.start, hunks, Bias::Before);
        let e = transform(self.end, hunks, Bias::After);
        self.start = s;
        self.end = e.max(s);
        debug_assert!(
            self.tainted
                || self.end - self.start == foreign::window_len(self.after.len(), &self.theirs),
            "a region's theirs must rebuild its window"
        );
    }

    /// Retained bytes (U6): its texts, ours' text, theirs' hunks and its roots.
    pub fn bytes(&self) -> usize {
        self.before.len()
            + self.after.len()
            + self.ours.iter().map(|h| h.insert.len() + 24).sum::<usize>()
            + self.theirs.len() * 24
            + self.roots.as_ref().map_or(0, RegionRoots::mem)
            + self.root.len()
            + 64
    }

    /// The region undone, merged with `live`: the clean merge (its text, **its own hunks**
    /// in window coordinates, and the roots each puts back), or the conflict (U10, U11).
    /// Both sides are exact hunk sets: ours, and theirs read from the live window **by
    /// root** (v1.8; `wroots`: the window's roots) or, for a region whose roots are unknown
    /// (or with no `wroots`), at element identity as composed (v1.7). The merge's hunks are
    /// what the step applies: they say which elements change (element identity, U10). O4 is
    /// judged on (position, root); O4a: a kept ours hunk putting back a root that `live_root`
    /// says is live conflicts.
    fn revert(
        &self,
        live: &str,
        wroots: Option<&Ids>,
        live_root: &dyn Fn(&IdRun) -> bool,
    ) -> Result<Merged, Box<ConflictRegion>> {
        let theirs = match live.get(self.start..self.end) {
            Some(t) => t,
            None => return Err(self.conflict(live, ConflictKind::Taint).into()),
        };
        if self.erased {
            return Err(self.conflict(live, ConflictKind::Erased).into());
        }
        if self.tainted {
            return Err(self.conflict(live, ConflictKind::Taint).into());
        }
        let composed = foreign::materialize(&self.theirs, &self.after, theirs);
        let at_end = self.end == live.len() && !self.base_doc_empty;
        let (Some(roots), Some(wroots)) = (&self.roots, wroots) else {
            // roots unknown: v1.7, theirs at element identity as composed
            let Some(th) = composed else {
                // never on a correct stream: the carried theirs must rebuild the window
                debug_assert!(
                    false,
                    "theirs doesn't rebuild the window: {self:?} {theirs:?}"
                );
                return Err(self.conflict(live, ConflictKind::Overlap).into());
            };
            return match merge::merge3_hunks(&self.after, &self.ours, &th, at_end) {
                Merge::Clean(clean) => Ok(Merged { clean, roots: None }),
                Merge::Conflicts(_) => Err(self.conflict(live, ConflictKind::Overlap).into()),
            };
        };
        debug_assert_eq!(wroots.bytes(), theirs.len(), "the window's roots fit it");
        let base = roots.after.chars(&self.after);
        let (th, th_roots, counted) =
            theirs_by_root(&self.after, &base, theirs, &wroots.chars(theirs));
        // cross-check (debug): every base element the composition at element identity says
        // survives counts by root (a surviving element is its own base element, same root)
        #[cfg(debug_assertions)]
        match &composed {
            Some(hs) => {
                let mut at = 0;
                for (k, (b, _)) in self.after.char_indices().enumerate() {
                    while at < hs.len() && hs[at].end <= b {
                        at += 1;
                    }
                    let gone = hs.get(at).is_some_and(|h| h.start <= b && b < h.end);
                    debug_assert!(
                        gone || counted[k],
                        "base element {k} survives (element identity) but doesn't count by \
                         root: {self:?} {theirs:?}"
                    );
                }
            }
            None => debug_assert!(false, "theirs doesn't rebuild the window: {self:?}"),
        }
        let _ = (&composed, &counted);
        // O4 on (position, root): the same range (the merge checks it) and the same roots
        let same = |i: usize, j: usize| roots.ours[i] == th_roots[j];
        let clean = match merge::merge3_hunks_by(&self.after, &self.ours, &th, at_end, same) {
            Merge::Clean(c) => c,
            Merge::Conflicts(_) => return Err(self.conflict(live, ConflictKind::Overlap).into()),
        };
        // the ours hunks the merge kept (not taken once), in order
        let kept: Vec<usize> = (0..self.ours.len())
            .filter(|&i| {
                !th.iter().enumerate().any(|(j, y)| {
                    y.start == self.ours[i].start && y.end == self.ours[i].end && same(i, j)
                })
            })
            .collect();
        // O4a: a root is put back at most once
        if kept
            .iter()
            .flat_map(|&i| roots.ours[i].runs())
            .any(live_root)
        {
            return Err(self.conflict(live, ConflictKind::Overlap).into());
        }
        // the clean hunks are the kept ones rebased and joined where they touch: their
        // roots, concatenated in order, cut at each clean hunk's insertion
        let mut all = Ids::default();
        for &i in &kept {
            all.extend(&roots.ours[i]);
        }
        let joined: String = kept.iter().map(|&i| self.ours[i].insert.as_str()).collect();
        let mut out = Vec::with_capacity(clean.hunks.len());
        let mut at = 0;
        for h in &clean.hunks {
            out.push(all.slice(&joined, at, at + h.insert.len()));
            at += h.insert.len();
        }
        debug_assert_eq!(
            at,
            joined.len(),
            "the clean hunks insert exactly the kept ones'"
        );
        Ok(Merged {
            clean,
            roots: Some(out),
        })
    }

    fn conflict(&self, live: &str, kind: ConflictKind) -> ConflictRegion {
        let s = floor_char(live, self.start.min(live.len()));
        let e = floor_char(live, self.end.clamp(s, live.len()));
        let theirs = live[s..e].to_owned();
        let first = position::line_of(live, s);
        let spans = theirs
            .matches('\n')
            .count()
            .saturating_sub(usize::from(theirs.ends_with('\n')));
        ConflictRegion {
            root: self.root.clone(),
            range: s..e,
            lines: (first, first + spans),
            kind,
            base: self.after.clone(),
            ours: self.before.clone(),
            theirs,
        }
    }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The shared-edge rule (m-d): two regions of one change that came to touch keep a foreign
/// insertion at their shared edge in the left region only. `regions` are one change's, in
/// order (by root, then start); the clamp is idempotent and commutes with carrying. The
/// right region's window loses those elements from its front, so its theirs does too.
pub(crate) fn clamp_shared_edges(regions: &mut [Region]) {
    for k in 1..regions.len() {
        let (l, r) = regions.split_at_mut(k);
        let (prev, next) = (&l[k - 1], &mut r[0]);
        if prev.root == next.root && next.start < prev.end {
            let n = prev.end - next.start;
            // (an erased region conflicts whatever theirs is, and its text may be blanked)
            if !next.tainted
                && !next.erased
                && !foreign::trim_front(&mut next.theirs, n.min(next.end - next.start))
            {
                // never on a correct stream: what both windows hold is foreign
                debug_assert!(false, "a shared edge's elements aren't foreign: {next:?}");
                next.tainted = true;
                next.theirs.clear();
            }
            next.start = prev.end;
            next.end = next.end.max(next.start);
        }
    }
}

/// A record region carried from its change through every later change on its root (U22a):
/// each edge on its own with its own taint, and theirs (the later hunks inside its window,
/// composed). A pure function of the record and the later changes' shapes, so the same
/// after a host restart (restart transparency, v1.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Carried {
    pub root: String,
    pub start: usize,
    pub end: usize,
    pub start_taint: bool,
    pub end_taint: bool,
    pub theirs: Vec<Shape>,
}

impl Carried {
    /// As the revert index reports a record's edges.
    pub fn edges(&self) -> super::carry::Edges {
        (
            self.root.clone(),
            self.start,
            self.end.max(self.start),
            self.start_taint,
            self.end_taint,
        )
    }
}

/// Carry a change's regions (from its own shape, `own`) through `later` (the shapes of every
/// change after it, in log order; the store keeps them for the doc's life): the edges
/// exactly as the revert index carries them, and theirs. O(L) in the changes replayed;
/// revert time only. Shapes, not the record's text: a purge blanks a redacted record's text.
pub(crate) fn replay_record(
    own: &[crate::store::RootShape],
    later: &[(Seq, Vec<crate::store::RootShape>)],
) -> Vec<Carried> {
    let mut out: Vec<(Carried, usize)> = Vec::new();
    for rs in own {
        for (s, e) in super::carry::regions_of_shape(&rs.hunks) {
            out.push((
                Carried {
                    root: rs.root.clone(),
                    start: s,
                    end: e,
                    start_taint: false,
                    end_taint: false,
                    theirs: Vec::new(),
                },
                e - s,
            ));
        }
    }
    for (_, shape) in later {
        for rs in shape {
            let hs: Vec<Shape> = rs
                .hunks
                .iter()
                .map(|&(s, e, n)| (s as usize, e as usize, n as usize))
                .collect();
            for (c, base_len) in out.iter_mut().filter(|(c, _)| c.root == rs.root) {
                let keep = !(c.start_taint || c.end_taint);
                let x = foreign::carry_window(c.start, c.end, &hs, &mut c.theirs, *base_len, keep);
                c.start_taint |= x.start;
                c.end_taint |= x.end;
                c.start = foreign::transform_shape(c.start, &hs, Bias::Before);
                c.end = foreign::transform_shape(c.end, &hs, Bias::After);
            }
        }
    }
    out.into_iter().map(|(c, _)| c).collect()
}

/// A revert's regions (U22): the record's hunks at their carried bounds with theirs
/// ([`replay_record`], in record order), with the shared-edge rule (m-d) applied, and at
/// least as conservative as undo on the same change and live text.
///
/// Undo clamps after **every** change, so once two regions of a change touch, the right
/// region's start *is* the left region's end, and a later deletion across that point taints
/// both. The carry tree moves each position on its own and the clamp runs only here, so the
/// right start it carries can sit behind the left end (a foreign insertion at the shared
/// edge pulls the left end forward and leaves the Before-biased start where it was), and
/// a deletion across the left end then misses it. Positions agree after the clamp (ends are
/// never moved by it, and carrying commutes with it); only that taint differs. So here: a
/// region whose carried start is at or before the previous region's end (they touch) takes
/// that end's taint. That is a superset of undo's taint (it also counts a deletion across
/// the left end from before the regions came to touch), never a subset: revert's
/// conflicts ⊇ undo's for the same change and live text.
pub(crate) fn revert_regions(
    rec: &crate::store::RevertRecord,
    carried: Vec<Carried>,
) -> Vec<Region> {
    let hunks = rec
        .roots
        .iter()
        .flat_map(|r| r.hunks.iter().map(move |h| (r, h)));
    let mut regions: Vec<Region> = Vec::with_capacity(carried.len());
    let mut prev: Option<(String, usize, bool)> = None; // root, end, end tainted
    for ((root, h), c) in hunks.zip(carried) {
        debug_assert_eq!(root.root, c.root);
        let (s, e, ts, te) = (c.start, c.end.max(c.start), c.start_taint, c.end_taint);
        let touches_tainted = prev
            .as_ref()
            .is_some_and(|(r, pe, pt)| *r == root.root && s <= *pe && *pt);
        prev = Some((root.root.clone(), e, te));
        let tainted = ts || te || touches_tainted;
        // the record's roots (v1.8, U22): base = the elements the change inserted, ours puts
        // back the elements it deleted; a redacted hunk's text is blank (it conflicts as
        // erased), so its roots aren't aligned with anything and aren't kept
        let roots = h.roots.as_ref().filter(|_| !h.redacted).map(|x| {
            let run = |r: &(u32, u32, u64, u32)| IdRun {
                bytes: r.0,
                units: r.1,
                start: ElemId::new(r.2, r.3),
            };
            let deleted = Ids::from_runs(x.deleted.iter().map(run));
            RegionRoots {
                after: Ids::from_runs(x.inserted.iter().map(run)),
                ours: vec![deleted.clone()],
                before: deleted,
            }
        });
        debug_assert!(roots
            .as_ref()
            .map_or(true, |r| r.after.bytes() == h.insert.len()
                && r.before.bytes() == h.deleted.len()));
        regions.push(Region {
            root: root.root.clone(),
            start: s,
            end: e,
            ours: vec![Hunk {
                start: 0,
                end: h.insert.len(),
                insert: h.deleted.clone(),
            }],
            theirs: if tainted { Vec::new() } else { c.theirs },
            before: h.deleted.clone(),
            after: h.insert.clone(),
            tainted,
            base_doc_empty: root.empty_after,
            erased: h.redacted,
            roots,
        });
    }
    clamp_shared_edges(&mut regions);
    regions
}

/// One change by one participant, as it can be undone (U1): its regions, in order.
#[derive(Debug, Clone)]
pub(crate) struct Batch {
    /// The change's log position (its stable change id). For an undo step that changed
    /// nothing, the version it was planned at.
    pub seq: Seq,
    /// The person an assistant made the change for (`undo --for`, U2).
    pub on_behalf_of: Option<String>,
    /// Ordered by root, then start.
    pub regions: Vec<Region>,
}

impl Batch {
    pub fn bytes(&self) -> usize {
        self.regions.iter().map(Region::bytes).sum::<usize>() + 64
    }

    pub fn transform(&mut self, root: &str, hunks: &[Hunk]) {
        let mut any = false;
        for r in self.regions.iter_mut().filter(|r| r.root == root) {
            r.transform(hunks);
            any = true;
        }
        if any {
            clamp_shared_edges(&mut self.regions);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// All or nothing (U13).
    All,
    /// Apply every clean region; stop at a step whose every region conflicts (U14).
    Partial,
}

/// One planned step.
#[derive(Debug)]
pub(crate) struct StepPlan {
    /// The edits, one per touched root, in the coordinates of the text before the step
    /// (canonical hunks). Empty when every applied region was already as wanted.
    pub edits: Vec<Edit>,
    /// Per edit, per hunk, the roots of what it puts back (v1.8, U23); `None` for an edit of
    /// regions whose roots are unknown (its insertions are fresh elements).
    pub restores: Vec<Option<Vec<Ids>>>,
    /// The regions of the step for the opposite stack, in the coordinates after the step:
    /// one per **applied** region, turning the step back (U17: redo reapplies exactly these).
    pub inverse: Vec<Region>,
    /// Regions skipped as conflicts (`--partial` only).
    pub skipped: Vec<ConflictRegion>,
}

#[derive(Debug)]
pub(crate) enum Plan {
    /// Steps to apply, newest first. `blocked`: with `--partial`, the next step whose every
    /// region conflicts (it stays where it is; planning stopped there). `steps` is empty
    /// when the very first step was blocked.
    Steps {
        steps: Vec<StepPlan>,
        blocked: Option<Vec<ConflictRegion>>,
    },
    /// All or nothing: step `step` (1-based) conflicts; every conflicting region of it.
    Conflict {
        step: usize,
        conflicts: Vec<ConflictRegion>,
    },
}

/// Plan undoing `batches` (newest first) against `texts` in `mode`. Changes nothing.
/// `prov` is the live provenance map (it names the windows' roots, and the live roots for
/// O4a); `next` is the undoer's next element ID (its client and clock): the plan applies
/// each step to scratch copies of the texts and of `prov` as the step will apply, so a
/// later step sees what an earlier one put back.
pub(crate) fn plan(
    texts: &[(&'static str, String)],
    prov: &Prov,
    next: ElemId,
    mut batches: Vec<Batch>,
    mode: Mode,
) -> Plan {
    let mut scratch: Vec<(&str, String)> = texts
        .iter()
        .filter(|(r, _)| {
            batches
                .iter()
                .any(|b| b.regions.iter().any(|g| g.root == *r))
        })
        .map(|(r, t)| (*r, t.clone()))
        .collect();
    let mut sprov = prov.clone();
    let mut clock = next.clock;
    let mut steps = Vec::with_capacity(batches.len());
    for k in 0..batches.len() {
        let regions = std::mem::take(&mut batches[k].regions);
        // every region merges against the step's starting text (windows are disjoint); O4a
        // also counts the roots the step's earlier regions put back
        let mut planned: Vec<IdRun> = Vec::new();
        type Res = Result<Merged, Box<ConflictRegion>>;
        let mut results: Vec<(usize, Res, Option<Ids>)> = Vec::new();
        for (idx, r) in regions.iter().enumerate() {
            let Some((_, live)) = scratch.iter().find(|(n, _)| *n == r.root) else {
                continue; // a root the schema no longer has: nothing to undo there
            };
            // the window's live roots (also for an unknown region: its redo region's roots)
            let wroots =
                (sprov.has(&r.root) && r.start <= r.end && live.get(r.start..r.end).is_some())
                    .then(|| sprov.roots(&r.root, live, r.start, r.end));
            let live_root = |x: &IdRun| sprov.any_live(x) || planned.iter().any(|p| meets(p, x));
            let res = r.revert(
                live,
                wroots.as_ref().filter(|_| r.roots.is_some()),
                &live_root,
            );
            if let Ok(Merged {
                roots: Some(rs), ..
            }) = &res
            {
                for ids in rs {
                    planned.extend(ids.runs().iter().copied());
                }
            }
            results.push((idx, res, wroots));
        }
        let conflicts: Vec<ConflictRegion> = results
            .iter()
            .filter_map(|(_, r, _)| r.as_ref().err().map(|c| (**c).clone()))
            .collect();
        if !conflicts.is_empty() {
            match mode {
                Mode::All => {
                    return Plan::Conflict {
                        step: k + 1,
                        conflicts,
                    }
                }
                Mode::Partial if conflicts.len() == results.len() => {
                    return Plan::Steps {
                        steps,
                        blocked: Some(conflicts),
                    };
                }
                Mode::Partial => {}
            }
        }
        let mut step = StepPlan {
            edits: Vec::new(),
            restores: Vec::new(),
            inverse: Vec::new(),
            skipped: conflicts,
        };
        for (root, live) in scratch.iter_mut() {
            // applied regions of this root, in order (regions are ordered by start)
            let applied: Vec<(&Region, &Merged, Option<&Ids>)> = results
                .iter()
                .filter_map(|(i, r, w)| r.as_ref().ok().map(|m| (&regions[*i], m, w.as_ref())))
                .filter(|(g, _, _)| g.root == *root)
                .collect();
            if applied.is_empty() {
                continue;
            }
            let known = applied.iter().all(|(_, m, _)| m.roots.is_some());
            debug_assert!(
                known || applied.iter().all(|(_, m, _)| m.roots.is_none()),
                "one step's regions on a root are all of known roots or all unknown"
            );
            let mut hunks = Vec::new();
            let mut hroots: Vec<Ids> = Vec::new();
            let mut shift: isize = 0;
            let mut inverse = Vec::with_capacity(applied.len());
            for (g, m, wr) in &applied {
                let clean = &m.clean;
                let theirs = &live[g.start..g.end];
                let merged = &clean.text;
                // the merge's own hunks, never a re-diff of the window (U10)
                for (j, h) in clean.hunks.iter().enumerate() {
                    hunks.push(Hunk {
                        start: h.start + g.start,
                        end: h.end + g.start,
                        insert: h.insert.clone(),
                    });
                    hroots.push(match &m.roots {
                        Some(rs) if known => rs[j].clone(),
                        _ => Ids::default(),
                    });
                }
                let start = (g.start as isize + shift) as usize;
                // the redo region (U17): ours is the inverse of exactly what was applied;
                // its base's roots are the merged window's, before's the window's (v1.8)
                let roots = match (wr, &m.roots) {
                    (Some(wr), Some(rs)) if known => Some(RegionRoots {
                        after: wr.apply(theirs, &clean.hunks, rs),
                        before: (*wr).clone(),
                        ours: clean
                            .hunks
                            .iter()
                            .map(|h| wr.slice(theirs, h.start, h.end))
                            .collect(),
                    }),
                    _ => None,
                };
                inverse.push(Region {
                    root: (*root).to_owned(),
                    start,
                    end: start + merged.len(),
                    ours: foreign::inverse(theirs, &clean.hunks),
                    theirs: Vec::new(),
                    before: theirs.to_owned(),
                    after: merged.clone(),
                    tainted: false,
                    base_doc_empty: false, // set below, once the step's text is known
                    erased: false,
                    roots,
                });
                shift += merged.len() as isize - (g.end - g.start) as isize;
            }
            let (hunks, hroots) = coalesce_with(hunks, hroots);
            let after = diff::apply(live, &hunks);
            for g in &mut inverse {
                g.base_doc_empty = after.is_empty();
            }
            step.inverse.extend(inverse);
            for later in &mut batches[k + 1..] {
                later.transform(root, &hunks);
            }
            // the redo regions of the steps already planned move with this one too
            for prev in &mut steps {
                let p: &mut StepPlan = prev;
                for g in p.inverse.iter_mut().filter(|g| g.root == *root) {
                    g.transform(&hunks);
                }
                clamp_shared_edges(&mut p.inverse);
            }
            if !hunks.is_empty() {
                let edit = Edit::Text {
                    root: (*root).to_owned(),
                    hunks,
                };
                // the scratch map applies the step as the engine will: the undoer's next
                // clocks, in the adapter's order (`insert_clocks`)
                let Edit::Text { hunks, .. } = &edit;
                let firsts =
                    crate::engine::insert_clocks(clock, std::slice::from_ref(&edit)).remove(0);
                let ins: Vec<Inserted> = hunks
                    .iter()
                    .zip(&firsts)
                    .zip(&hroots)
                    .map(|((h, c), r)| Inserted {
                        id: ElemId::new(next.client, *c),
                        roots: (known && !h.insert.is_empty()).then(|| r.clone()),
                    })
                    .collect();
                clock += hunks
                    .iter()
                    .map(|h| h.insert.encode_utf16().count() as u32)
                    .sum::<u32>();
                if sprov.has(root) {
                    sprov.apply(root, live, hunks, &ins);
                }
                step.edits.push(edit);
                step.restores.push(known.then_some(hroots));
            }
            *live = after;
        }
        steps.push(step);
    }
    Plan::Steps {
        steps,
        blocked: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: &str = "r";

    /// `plan` with no provenance map and regions of unknown roots: v1.7's merge, which these
    /// tests pin (by root is the session's, `tests/history.rs`).
    fn plan_v17(texts: &[(&'static str, String)], batches: Vec<Batch>, mode: Mode) -> Plan {
        plan(texts, &Prov::default(), ElemId::new(9, 0), batches, mode)
    }

    fn batch(before: &str, after: &str) -> Batch {
        let hs = diff::hunks(before, after);
        Batch {
            seq: 1,
            on_behalf_of: None,
            regions: Region::of_change(R, before, &hs, None),
        }
    }

    fn carry(b: &mut Batch, old: &str, new: &str) {
        b.transform(R, &diff::hunks(old, new));
    }

    fn texts(t: &str) -> Vec<(&'static str, String)> {
        vec![(R, t.to_owned())]
    }

    fn undone(live: &str, b: Batch, mode: Mode) -> (String, Vec<ConflictRegion>) {
        match plan_v17(&texts(live), vec![b], mode) {
            Plan::Steps { steps, blocked } => {
                assert!(blocked.is_none(), "blocked: {blocked:?}");
                let s = &steps[0];
                let out = match s.edits.first() {
                    Some(Edit::Text { hunks, .. }) => diff::apply(live, hunks),
                    None => live.to_owned(),
                };
                (out, s.skipped.clone())
            }
            Plan::Conflict { conflicts, .. } => (live.to_owned(), conflicts),
        }
    }

    #[test]
    fn a_region_per_canonical_hunk() {
        let b = batch("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n");
        assert_eq!(b.regions.len(), 2);
        assert_eq!((b.regions[0].start, b.regions[0].end), (2, 3));
        assert_eq!((b.regions[1].start, b.regions[1].end), (8, 9));
        assert_eq!(b.regions[1].before, "e");
    }

    #[test]
    fn partial_skips_exactly_what_all_or_nothing_reports() {
        let before = "one\ntwo\nthree\nfour\n";
        let after = "ONE\ntwo\nthree\nFOUR\n";
        let live = "ONE\ntwo\nthree\nFOxUR\n"; // someone edits inside the second region
        let mut b = batch(before, after);
        carry(&mut b, after, live);
        let all = match plan_v17(&texts(live), vec![b.clone()], Mode::All) {
            Plan::Conflict { step: 1, conflicts } => conflicts,
            p => panic!("{p:?}"),
        };
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].range, 14..19);
        assert_eq!(all[0].lines, (4, 4));
        let (out, skipped) = undone(live, b, Mode::Partial);
        assert_eq!(skipped, all);
        assert_eq!(out, "one\ntwo\nthree\nFOxUR\n");
        // an insertion touching a region's edge is O3: no conflict
        let mut b = batch(before, after);
        carry(&mut b, after, "ONE\ntwo\nthree\nFOUR!\n");
        let (out, skipped) = undone("ONE\ntwo\nthree\nFOUR!\n", b, Mode::All);
        assert!(skipped.is_empty());
        assert_eq!(out, "one\ntwo\nthree\nfour!\n");
    }

    #[test]
    fn eof_rule_uses_the_whole_doc_base() {
        // B deletes the last line, someone appends: undo lands, theirs first (U12)
        let mut b = batch("a\nx\n", "a\n");
        carry(&mut b, "a\n", "a\ny\n");
        assert_eq!(undone("a\ny\n", b, Mode::All).0, "a\ny\nx\n");
        // B deletes the whole doc, someone appends: the base is "", so it conflicts (m11)
        let mut b = batch("x\n", "");
        carry(&mut b, "", "y\n");
        let (out, c) = undone("y\n", b, Mode::All);
        assert_eq!(out, "y\n");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, ConflictKind::Overlap);
    }

    #[test]
    fn shared_edge_insert_goes_to_the_left_region_only() {
        let mut b = batch("a\nX\nb\nY\nc\n", "a\n1\nb\n2\nc\n");
        assert_eq!(b.regions.len(), 2);
        // someone deletes what separated them: the regions touch at 3
        carry(&mut b, "a\n1\nb\n2\nc\n", "a\n12\nc\n");
        assert_eq!((b.regions[0].end, b.regions[1].start), (3, 3));
        assert!(!b.regions[0].tainted && !b.regions[1].tainted);
        // someone inserts "Z" at the shared edge: the left region takes it
        carry(&mut b, "a\n12\nc\n", "a\n1Z2\nc\n");
        assert_eq!((b.regions[0].start, b.regions[0].end), (2, 4));
        assert_eq!((b.regions[1].start, b.regions[1].end), (4, 5));
    }

    #[test]
    fn a_deletion_across_an_edge_taints() {
        let mut b = batch("abc def ghi", "abc DEF ghi");
        carry(&mut b, "abc DEF ghi", "abc DEghi");
        assert!(b.regions[0].tainted);
        let (_, c) = undone("abc DEghi", b, Mode::All);
        assert_eq!(c[0].kind, ConflictKind::Taint);
        assert_eq!(c[0].ours, "def");
    }

    #[test]
    fn inverse_regions_redo_exactly_what_was_undone() {
        let before = "one\ntwo\nthree\n";
        let after = "ONE\ntwo\nTHREE\n";
        let live = "ONE\ntwo\nTHxREE\n";
        let mut b = batch(before, after);
        carry(&mut b, after, live);
        let Plan::Steps { steps, .. } = plan_v17(&texts(live), vec![b], Mode::Partial) else {
            panic!()
        };
        let Edit::Text { hunks, .. } = &steps[0].edits[0];
        let undone = diff::apply(live, hunks);
        assert_eq!(undone, "one\ntwo\nTHxREE\n");
        assert_eq!(steps[0].inverse.len(), 1); // the skipped region isn't redone
        let redo = Batch {
            seq: 2,
            on_behalf_of: None,
            regions: steps[0].inverse.clone(),
        };
        let Plan::Steps { steps, .. } = plan_v17(&texts(&undone), vec![redo], Mode::All) else {
            panic!()
        };
        let Edit::Text { hunks, .. } = &steps[0].edits[0];
        assert_eq!(diff::apply(&undone, hunks), live);
    }

    // ----- revert is at least as conservative as undo (C7b) -----

    use crate::session::carry::RevertIndex;
    use crate::store::{RecordHunk, RecordRoot, RevertRecord, RootShape};
    use proptest::prelude::*;

    /// The revert side of a change: its record, the revert index it is staged in, and the
    /// shapes of every later change (what the store keeps).
    struct Rev {
        rec: RevertRecord,
        ix: RevertIndex,
        log: Vec<(Seq, Vec<RootShape>)>,
    }

    /// The change `before -> hunks`, as an undo batch, a revert record, and staged in a
    /// fresh revert index as change 1 (as a session does).
    fn change(before: &str, hunks: &[Hunk]) -> (Batch, Rev) {
        let hunks = diff::coalesce(hunks.to_vec());
        let regions = Region::of_change(R, before, &hunks, None);
        let after = diff::apply(before, &hunks);
        let rec = RevertRecord {
            author: "a".into(),
            on_behalf_of: None,
            at_ms: 0,
            roots: vec![RecordRoot {
                root: R.into(),
                empty_after: after.is_empty(),
                hunks: hunks
                    .iter()
                    .map(|h| RecordHunk {
                        start: h.start as u64,
                        end: h.end as u64,
                        insert: h.insert.clone(),
                        deleted: before[h.start..h.end].to_owned(),
                        redacted: false,
                        roots: None,
                    })
                    .collect(),
            }],
        };
        let mut ix = RevertIndex::empty_for_tests();
        ix.stage(
            &regions
                .iter()
                .map(|g| (g.root.clone(), g.start, g.end))
                .collect::<Vec<_>>(),
        );
        ix.bind(&[1]);
        let b = Batch {
            seq: 1,
            on_behalf_of: None,
            regions,
        };
        (
            b,
            Rev {
                rec,
                ix,
                log: Vec::new(),
            },
        )
    }

    /// Carry both through a later change to `live` (canonical hunks).
    fn later(b: &mut Batch, rv: &mut Rev, live: &mut String, hunks: &[Hunk]) {
        b.transform(R, hunks);
        rv.ix.carry(R, hunks);
        let seq = rv.log.len() as Seq + 2;
        rv.log.push((
            seq,
            vec![RootShape {
                root: R.into(),
                hunks: hunks
                    .iter()
                    .map(|h| (h.start as u64, h.end as u64, h.insert.len() as u64))
                    .collect(),
            }],
        ));
        *live = diff::apply(live, hunks);
    }

    /// Conflicting regions of undoing `b` / reverting `rec` on `live` (all or nothing), as
    /// `(range, kind)`, or the edits when clean.
    fn outcome(live: &str, b: Batch) -> Result<Vec<Edit>, Vec<(Range<usize>, ConflictKind)>> {
        match plan_v17(&texts(live), vec![b], Mode::All) {
            Plan::Steps { mut steps, .. } => Ok(steps.pop().map(|s| s.edits).unwrap_or_default()),
            Plan::Conflict { conflicts, .. } => {
                Err(conflicts.into_iter().map(|c| (c.range, c.kind)).collect())
            }
        }
    }

    /// The revert of change 1, as a session plans it: replayed, and the edges checked
    /// against the index's.
    fn reverted(rv: &mut Rev) -> Batch {
        let carried = replay_record(&rv.rec.shape(), &rv.log);
        assert_eq!(
            carried.iter().map(Carried::edges).collect::<Vec<_>>(),
            rv.ix.edges(1).expect("record 1"),
            "replayed edges = the index's"
        );
        Batch {
            seq: 1,
            on_behalf_of: None,
            regions: revert_regions(&rv.rec, carried),
        }
    }

    /// C7a's edge case: two regions of a change come to touch, someone inserts at the
    /// shared edge, then a deletion crosses the right region's (corrected) start without
    /// touching the rest of it. Undo taints both regions; revert must too.
    #[test]
    fn revert_taints_the_right_region_at_a_crossed_shared_edge() {
        // the right hunk is wider than the change (an explicit `edit`): "2abQ" -> "2abc"
        let before = "x--2abQ\n";
        let h = |s: usize, e: usize, t: &str| Hunk {
            start: s,
            end: e,
            insert: t.into(),
        };
        let (mut b, mut rv) = change(before, &[h(0, 1, "L"), h(3, 7, "2abc")]);
        let mut live = "L--2abc\n".to_owned();
        later(&mut b, &mut rv, &mut live, &[h(1, 3, "")]); // "L2abc": they touch
        later(&mut b, &mut rv, &mut live, &[h(1, 1, "Z")]); // "LZ2abc": at the shared edge
        later(&mut b, &mut rv, &mut live, &[h(1, 3, "")]); // "Labc": across it
        assert_eq!(live, "Labc\n");
        let undo = outcome(&live, b.clone()).unwrap_err();
        assert_eq!(
            undo,
            vec![(0..1, ConflictKind::Taint), (1..4, ConflictKind::Taint)]
        );
        let r = outcome(&live, reverted(&mut rv)).unwrap_err();
        assert_eq!(r, undo);
        // --partial: nothing is clean, so nothing applies (C7a applied the right region)
        let p = plan_v17(&texts(&live), vec![reverted(&mut rv)], Mode::Partial);
        assert!(
            matches!(&p, Plan::Steps { steps, blocked: Some(c) } if steps.is_empty() && c.len() == 2),
            "{p:?}"
        );
    }

    /// One drawn later change: `(how, which, position, deleted, inserted)`. `how` 0: at a
    /// random position; 1: at an edge of one of the change's regions (± 1); 2: delete
    /// exactly what separates two regions (so they come to touch). The edges come from the
    /// undo batch's bounds, so histories reach touching regions, insertions at shared edges
    /// and deletions across them often.
    type Draw = (u8, u8, u16, u8, String);

    fn draws() -> impl Strategy<Value = Vec<Draw>> {
        prop::collection::vec(
            (0u8..3, any::<u8>(), any::<u16>(), 0u8..4, "[xyz\n]{0,2}"),
            1..3,
        )
    }

    /// Canonical hunks for `text` from draws, anchored on `regions` (none: all random).
    fn canonical(text: &str, regions: &[Region], raw: &[Draw]) -> Vec<Hunk> {
        let len = text.len();
        let n = regions.len().max(1);
        let mut pts: Vec<(usize, usize, String)> = Vec::new();
        for (how, which, pos, del, ins) in raw {
            let g = regions.get(*which as usize % n);
            let (s, e, ins) = match (how, g) {
                (1, Some(g)) => {
                    let edge = if pos % 2 == 0 { g.start } else { g.end };
                    let s = match pos % 3 {
                        0 => edge,
                        1 => edge.saturating_sub(1),
                        _ => edge + 1,
                    }
                    .min(len);
                    (s, (s + *del as usize).min(len), ins.clone())
                }
                (2, Some(g)) => match regions.get(*which as usize % n + 1) {
                    Some(next) if next.start > g.end => (g.end, next.start, String::new()),
                    _ => continue,
                },
                _ => {
                    let s = *pos as usize % (len + 1);
                    (s, (s + *del as usize).min(len), ins.clone())
                }
            };
            pts.push((s, e, ins));
        }
        pts.sort_by_key(|x| (x.0, x.1));
        let mut out: Vec<Hunk> = Vec::new();
        for (s, e, ins) in pts {
            if out.last().is_some_and(|l| s <= l.end) || (s == e && ins.is_empty()) {
                continue;
            }
            out.push(Hunk {
                start: s,
                end: e,
                insert: ins,
            });
        }
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]
        /// For random histories, reverting a change conflicts on every region undoing it
        /// conflicts on (same change, same live text), and when neither conflicts they
        /// make the same edit.
        #[test]
        fn revert_conflicts_are_a_superset_of_undos(
            before in "[ab\n]{0,10}",
            first in prop::collection::vec((any::<u16>(), 0u8..4, "[ab\n]{0,3}"), 1..4),
            rest in prop::collection::vec(draws(), 0..10),
        ) {
            let first: Vec<Draw> = first.into_iter().map(|(p, d, i)| (0, 0, p, d, i)).collect();
            let hunks = canonical(&before, &[], &first);
            prop_assume!(!hunks.is_empty());
            let (mut b, mut rv) = change(&before, &hunks);
            let mut live = diff::apply(&before, &diff::coalesce(hunks));
            for raw in &rest {
                let hs = canonical(&live, &b.regions, raw);
                later(&mut b, &mut rv, &mut live, &hs);
            }
            let undo = outcome(&live, b);
            let rv = outcome(&live, reverted(&mut rv));
            match (&undo, &rv) {
                (Ok(u), Ok(r)) => prop_assert_eq!(u, r),
                (Ok(_), Err(_)) => {}
                (Err(u), Ok(_)) => prop_assert!(false, "undo conflicts {:?}, revert is clean", u),
                (Err(u), Err(r)) => {
                    for (range, kind) in u {
                        prop_assert!(
                            r.iter().any(|(rr, rk)| rr == range && (rk == kind || *rk == ConflictKind::Taint)),
                            "undo conflict {:?} {:?} missing from revert's {:?}", range, kind, r
                        );
                    }
                }
            }
        }
    }
}
