//! Per-participant undo, kept small.
//!
//! Undo is a three-way merge (`merge3(after, before, live)`), so it only reverts what the
//! participant changed and refuses when someone else has since edited the same text. Keeping
//! whole before/after texts per batch would cost two copies of the doc per step, so a batch
//! keeps only its **region**: the span its hunks covered, as text before and after the
//! change. The region's bounds are carried through every later change (anyone's) like a
//! caret; at undo time the region's live text is "theirs".
//!
//! Inside the region the merge is exactly `merge3`. Changes outside it are, by definition,
//! not overlapping, so they would merge cleanly anyway. Insertions exactly at a region's
//! edges are pulled inside it, so `merge3`'s "two insertions at one point conflict" rule
//! still applies there. The one case a region can't represent is a later deletion that
//! crosses one of its edges: the batch is then **tainted** and its undo reports a conflict
//! (conservative: a whole-text merge might have succeeded).

use std::collections::VecDeque;

use super::position::{self, transform, Bias};
use crate::doc::Edit;
use crate::store::Seq;
use crate::text::diff::{self, Hunk};
use crate::text::merge::{self, Conflict, Merge};

#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub root: String,
    /// Bounds in the current text (carried through every change).
    pub start: usize,
    pub end: usize,
    /// The region's text before and after the participant's change.
    pub before: String,
    pub after: String,
    pub tainted: bool,
}

impl Region {
    /// The region of a change: `hunks` (non-empty, valid) applied to `before_text`.
    pub fn of_change(root: &str, before_text: &str, hunks: &[Hunk]) -> Self {
        let (first, last) = (&hunks[0], &hunks[hunks.len() - 1]);
        let (s, e) = (first.start, last.end);
        let local: Vec<Hunk> = hunks
            .iter()
            .map(|h| Hunk {
                start: h.start - s,
                end: h.end - s,
                insert: h.insert.clone(),
            })
            .collect();
        let before = before_text[s..e].to_owned();
        let after = diff::apply(&before, &local);
        Region {
            root: root.to_owned(),
            start: s,
            end: s + after.len(),
            before,
            after,
            tainted: false,
        }
    }

    /// Carry the region through a later change to its root.
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

    /// Hunks (in `live` coordinates) that revert this region's change, or the conflicts.
    fn revert(&self, live: &str) -> Result<Vec<Hunk>, Vec<Conflict>> {
        let theirs = match live.get(self.start..self.end) {
            Some(t) if !self.tainted => t,
            _ => return Err(vec![self.whole_conflict(live)]),
        };
        // The region is a window of the doc: O5 (appends at the end both land) only applies
        // when it ends where the doc does, so undo matches a whole-text merge3 (U12).
        let at_end = self.end == live.len();
        match merge::merge3_window(&self.after, &self.before, theirs, at_end) {
            Merge::Clean(c) => Ok(c
                .hunks
                .into_iter()
                .map(|h| Hunk {
                    start: h.start + self.start,
                    end: h.end + self.start,
                    insert: h.insert,
                })
                .collect()),
            Merge::Conflicts(cs) => {
                // Report in live coordinates (approximate: offsets within the region are
                // the region's own, before the later edits).
                let l0 = position::line_of(live, self.start) - 1;
                Err(cs
                    .into_iter()
                    .map(|c| Conflict {
                        base: c.base.start + self.start..c.base.end + self.start,
                        lines: (c.lines.0 + l0, c.lines.1 + l0),
                        ..c
                    })
                    .collect())
            }
        }
    }

    fn whole_conflict(&self, live: &str) -> Conflict {
        let s = self.start.min(live.len());
        let e = self.end.clamp(s, live.len());
        let theirs = live.get(s..e).unwrap_or("").to_owned();
        let first = position::line_of(live, s);
        Conflict {
            base: s..e,
            lines: (first, first + theirs.matches('\n').count()),
            base_text: self.after.clone(),
            ours: self.before.clone(),
            theirs,
        }
    }
}

/// One participant edit call (or one undo/redo step), as it can be reverted.
#[derive(Debug, Clone)]
pub(crate) struct Batch {
    /// Log position of the change (orders batches across participants for eviction).
    pub seq: Seq,
    /// At most one region per root.
    pub regions: Vec<Region>,
}

impl Batch {
    pub fn bytes(&self) -> usize {
        self.regions.iter().map(Region::bytes).sum()
    }

    pub fn transform(&mut self, root: &str, hunks: &[Hunk]) {
        for r in self.regions.iter_mut().filter(|r| r.root == root) {
            r.transform(hunks);
        }
    }
}

/// Plan reverting the newest `n` batches of `stack`, newest first, against `texts`.
/// Returns one edit list per step (an empty list when a step has nothing left to revert),
/// or the first conflicting step (1-based) with its conflicts. Changes nothing.
pub(crate) fn plan(
    texts: &[(&'static str, String)],
    stack: &VecDeque<Batch>,
    n: usize,
) -> Result<Vec<Vec<Edit>>, (usize, Vec<Conflict>)> {
    let mut pending: Vec<Batch> = stack.iter().rev().take(n).cloned().collect();
    let mut scratch: Vec<(&str, String)> = texts
        .iter()
        .filter(|(r, _)| {
            pending
                .iter()
                .any(|b| b.regions.iter().any(|g| g.root == *r))
        })
        .map(|(r, t)| (*r, t.clone()))
        .collect();
    let mut out = Vec::with_capacity(pending.len());
    for k in 0..pending.len() {
        let mut edits = Vec::new();
        for r in &pending[k].regions {
            let live = &scratch
                .iter()
                .find(|(n, _)| *n == r.root)
                .expect("scratch has every region's root")
                .1;
            match r.revert(live) {
                Ok(hs) if hs.is_empty() => {}
                Ok(hs) => edits.push(Edit::Text {
                    root: r.root.clone(),
                    hunks: hs,
                }),
                Err(c) => return Err((k + 1, c)),
            }
        }
        for e in &edits {
            let Edit::Text { root, hunks } = e;
            let t = &mut scratch
                .iter_mut()
                .find(|(n, _)| *n == root)
                .expect("root present")
                .1;
            *t = diff::apply(t, hunks);
            for later in &mut pending[k + 1..] {
                later.transform(root, hunks);
            }
        }
        out.push(edits);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(before: &str, after: &str) -> (Region, Vec<Hunk>) {
        let hs = diff::hunks(before, after);
        (Region::of_change("r", before, &hs), hs)
    }

    #[test]
    fn region_holds_only_the_changed_span() {
        let (r, _) = region("aaa bbb ccc\nddd\n", "aaa BBB ccc\nddd\n");
        assert_eq!((r.start, r.end), (4, 7));
        assert_eq!((r.before.as_str(), r.after.as_str()), ("bbb", "BBB"));
        assert_eq!(
            r.revert("aaa BBB ccc\nddd\n").unwrap(),
            diff::hunks("BBB", "bbb")
                .into_iter()
                .map(|h| Hunk {
                    start: h.start + 4,
                    end: h.end + 4,
                    ..h
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn later_edits_elsewhere_merge_and_overlaps_conflict() {
        let before = "one two three\n";
        let after = "one TWO three\n";
        let (mut r, _) = region(before, after);
        // someone prepends text and appends at the end: the region moves, undo still applies
        let live = "zero one TWO three!\n";
        r.transform(&diff::hunks(after, live));
        let hs = r.revert(live).unwrap();
        assert_eq!(diff::apply(live, &hs), "zero one two three!\n");
        // someone edits inside the region: conflict
        let mut r2 = region(before, after).0;
        let live2 = "one TwO three\n";
        r2.transform(&diff::hunks(after, live2));
        assert!(r2.revert(live2).is_err());
    }

    #[test]
    fn a_deletion_across_an_edge_taints() {
        let (mut r, _) = region("abc def ghi", "abc DEF ghi");
        let live = "abc DEghi";
        r.transform(&[Hunk {
            start: 6,
            end: 8,
            insert: String::new(),
        }]);
        assert!(r.tainted);
        let c = r.revert(live).unwrap_err();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].ours, "def");
    }
}
