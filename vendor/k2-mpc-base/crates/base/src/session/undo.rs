//! Undo, redo and revert over text, to the undo spec v1.4 (`.k2/prds/spec-undo-v1.md`).
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
//! Undoing a region is `merge3_window(base = after, ours = before, theirs = live[s'..e'],
//! at_end)` where `at_end` holds only when the region ends at the root's end **and** the
//! root was non-empty after the change being undone (the merge's base, U10, v1.4 m11), so the
//! end-of-text rule O5 fires exactly where a whole-text `merge3` would fire it (U12).
//!
//! Within one step, regions are applied **back to front**; the windows are disjoint, so each
//! region merges against the step's starting text and lower regions keep their bounds
//! (U10, m-d). A plan carries the regions of the later steps through each planned step.
//!
//! Modes (U13–U15): all-or-nothing reports **every** conflicting region of the first
//! conflicting step and applies nothing; `--partial` applies every clean region, reports the
//! rest (the same set), and stops at a step whose every region conflicts (it stays on top).

use std::ops::Range;

use super::position::{self, transform, Bias};
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
    pub tainted: bool,
    /// The root was empty right after the change (the whole doc's merge base): the
    /// end-of-text rule O5 then never applies (U10, v1.4 m11).
    pub base_doc_empty: bool,
    /// The region overlaps redacted content (revert records after a purge).
    pub erased: bool,
}

impl Region {
    /// One region per canonical hunk of a change: `hunks` (sorted, valid against
    /// `before_text`) applied to `before_text`. Touching hunks are coalesced first. Bounds
    /// are in the coordinates of the text after the change.
    pub fn of_change(root: &str, before_text: &str, hunks: &[Hunk]) -> Vec<Region> {
        let hunks = diff::coalesce(hunks.to_vec());
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
                Region {
                    root: root.to_owned(),
                    start,
                    end: start + h.insert.len(),
                    before: before_text[h.start..h.end].to_owned(),
                    after: h.insert,
                    tainted: false,
                    base_doc_empty: empty_after,
                    erased: false,
                }
            })
            .collect()
    }

    /// Carry the region through a later change to its root (U8, U9).
    pub fn transform(&mut self, hunks: &[Hunk]) {
        let crosses = |p: usize| hunks.iter().any(|h| h.start < p && p < h.end);
        if crosses(self.start) || crosses(self.end) {
            self.tainted = true;
        }
        let s = transform(self.start, hunks, Bias::Before);
        let e = transform(self.end, hunks, Bias::After);
        self.start = s;
        self.end = e.max(s);
    }

    pub fn bytes(&self) -> usize {
        self.before.len() + self.after.len() + self.root.len() + 64
    }

    /// The region's text after undoing it, merged with `live` (`Ok(merged window text)`),
    /// or the conflict (U10, U11).
    fn revert(&self, live: &str) -> Result<String, Box<ConflictRegion>> {
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
        let at_end = self.end == live.len() && !self.base_doc_empty;
        match merge::merge3_window(&self.after, &self.before, theirs, at_end) {
            Merge::Clean(c) => Ok(c.text),
            Merge::Conflicts(_) => Err(self.conflict(live, ConflictKind::Overlap).into()),
        }
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
/// order (by root, then start); the clamp is idempotent and commutes with carrying.
pub(crate) fn clamp_shared_edges(regions: &mut [Region]) {
    for k in 1..regions.len() {
        let (l, r) = regions.split_at_mut(k);
        let (prev, next) = (&l[k - 1], &mut r[0]);
        if prev.root == next.root && next.start < prev.end {
            next.start = prev.end;
            next.end = next.end.max(next.start);
        }
    }
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
pub(crate) fn plan(texts: &[(&'static str, String)], mut batches: Vec<Batch>, mode: Mode) -> Plan {
    let mut scratch: Vec<(&str, String)> = texts
        .iter()
        .filter(|(r, _)| {
            batches
                .iter()
                .any(|b| b.regions.iter().any(|g| g.root == *r))
        })
        .map(|(r, t)| (*r, t.clone()))
        .collect();
    let mut steps = Vec::with_capacity(batches.len());
    for k in 0..batches.len() {
        let regions = std::mem::take(&mut batches[k].regions);
        // every region merges against the step's starting text (windows are disjoint)
        let mut results: Vec<(usize, Result<String, Box<ConflictRegion>>)> = Vec::new();
        for (idx, r) in regions.iter().enumerate() {
            let Some((_, live)) = scratch.iter().find(|(n, _)| *n == r.root) else {
                continue; // a root the schema no longer has: nothing to undo there
            };
            results.push((idx, r.revert(live)));
        }
        let conflicts: Vec<ConflictRegion> = results
            .iter()
            .filter_map(|(_, r)| r.as_ref().err().map(|c| (**c).clone()))
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
            inverse: Vec::new(),
            skipped: conflicts,
        };
        for (root, live) in scratch.iter_mut() {
            // applied regions of this root, in order (regions are ordered by start)
            let applied: Vec<(&Region, &String)> = results
                .iter()
                .filter_map(|(i, r)| r.as_ref().ok().map(|m| (&regions[*i], m)))
                .filter(|(g, _)| g.root == *root)
                .collect();
            if applied.is_empty() {
                continue;
            }
            let mut hunks = Vec::new();
            let mut shift: isize = 0;
            let mut inverse = Vec::with_capacity(applied.len());
            for (g, merged) in &applied {
                let theirs = &live[g.start..g.end];
                for h in diff::hunks(theirs, merged) {
                    hunks.push(Hunk {
                        start: h.start + g.start,
                        end: h.end + g.start,
                        insert: h.insert,
                    });
                }
                let start = (g.start as isize + shift) as usize;
                inverse.push(Region {
                    root: (*root).to_owned(),
                    start,
                    end: start + merged.len(),
                    before: theirs.to_owned(),
                    after: (*merged).clone(),
                    tainted: false,
                    base_doc_empty: false, // set below, once the step's text is known
                    erased: false,
                });
                shift += merged.len() as isize - (g.end - g.start) as isize;
            }
            let hunks = diff::coalesce(hunks);
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
            *live = after;
            if !hunks.is_empty() {
                step.edits.push(Edit::Text {
                    root: (*root).to_owned(),
                    hunks,
                });
            }
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

    fn batch(before: &str, after: &str) -> Batch {
        let hs = diff::hunks(before, after);
        Batch {
            seq: 1,
            on_behalf_of: None,
            regions: Region::of_change(R, before, &hs),
        }
    }

    fn carry(b: &mut Batch, old: &str, new: &str) {
        b.transform(R, &diff::hunks(old, new));
    }

    fn texts(t: &str) -> Vec<(&'static str, String)> {
        vec![(R, t.to_owned())]
    }

    fn undone(live: &str, b: Batch, mode: Mode) -> (String, Vec<ConflictRegion>) {
        match plan(&texts(live), vec![b], mode) {
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
        let all = match plan(&texts(live), vec![b.clone()], Mode::All) {
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
        let Plan::Steps { steps, .. } = plan(&texts(live), vec![b], Mode::Partial) else {
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
        let Plan::Steps { steps, .. } = plan(&texts(&undone), vec![redo], Mode::All) else {
            panic!()
        };
        let Edit::Text { hunks, .. } = &steps[0].edits[0];
        assert_eq!(diff::apply(&undone, hunks), live);
    }
}
