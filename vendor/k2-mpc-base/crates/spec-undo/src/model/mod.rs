//! `spec_undo::model`: the oracle of record for the undo spec (`docs/SPEC-UNDO.md`, v1.5),
//! over plain strings and hunks. No engine, no store, no clock.
//!
//! **What it calls from the kit:** only the diff pipeline, D1–D2: `diff::hunks` (inside
//! [`merge::merge3_window`]) and `align_insertions(diff::hunks(..))` for edits given as a
//! new text (`replace`, `restore`). Everything else is written from the spec text: the
//! merge's pair rules (O1–O5) and its placement of ours on theirs, the canonical form (D2's
//! "coalesced when touching"), the caret carry (U8), taint (U9), the window flag `at_end`
//! (U10), the reports (U11), the modes (U13–U17), revert (U22), purge (U19) and the bounds
//! (U6).
//!
//! **State** ([`Model`]) is plain data, `Clone + Eq + Hash`, and every operation is a
//! deterministic function of it and its arguments, so an enumerator can explore it. Each
//! operation returns an [`Outcome`] and the [`Hits`] it saw (the trace runner's case
//! classes; not part of the state).
//!
//! **Readings** (where the spec text leaves a choice; each is flagged in the trace report):
//! - **R1** (U8/U9): a bound strictly inside a range a later change deletes or replaces (the
//!   region is then tainted) moves to where that range was. A replacement *starting* at a
//!   region's end is "entirely after" it and an insertion inside a replacement is not "an
//!   insertion exactly at" an edge: hunks are taken as the canonical stream reports them
//!   (an insertion next to a deletion is one replacement hunk).
//! - **R2** (U8, U10): within one step every region merges against the step's starting
//!   text (the windows are disjoint and applied back to front), and a step's own changes
//!   at a shared edge belong to the region that made them; "carrying the regions not yet
//!   applied" never moves them (nothing is applied before them).
//! - **R3** (U10): a clean region's applied change is the **merge's own hunks** ("a clean
//!   merge gives the inverse hunks (offset by s')"): ours' changes placed in theirs'
//!   coordinates, not a fresh diff of the window. They are the step's hunk stream, which
//!   carries every other region (U8) and is the step's revert record (U22).
//! - **R4** (U17): a redo region is the undo region as the undo left it: bounds of its
//!   merged text, base = the merged text, ours = the window's live text before the undo;
//!   one per **applied** region (a clean region that changed nothing included). The whole-doc
//!   flag for its `at_end` is whether the root was non-empty after the undo step.
//! - **R5** (U6): the bound counts both stacks together (an undo or redo moves a batch, so
//!   the count only grows by a new batch, after U7 has emptied redo); the oldest undo batch
//!   goes first. The byte bound counts region text (before + after).
//! - **R6** (U18, U19; settled in v1.5): "cleared by purge" is said once, on the holder's
//!   next undo or redo that finds nothing, and is withdrawn when the holder records a new
//!   batch (then plain "nothing to undo"); "history lost" is said on an undo that finds an
//!   empty stack, by a pid that made a change before the last restart.
//! - **R7** (U13): `--steps N` with fewer than N batches undoes what there is.
//! - **U22a** (v1.5), exactly: a record's region edges are carried each on its own (no
//!   shared-edge clamp while carrying), each with its own taint; at revert time a region
//!   whose carried start is at or before the previous region's carried end (same root)
//!   inherits that end's taint, then the shared-edge clamp applies. [`Record::regions`]
//!   also keeps undo's carry of the same change, so every revert checks U22a's property:
//!   revert's conflicts ⊇ undo's on the same state, and the regions both apply give the
//!   same text ([`Hits::revert_superset_ok`]). A carry persisted when the window moves
//!   keeps one taint flag per region; after a restart it taints both edges.
//! - **R8** (U22): a revert is planned as one step of undo (U13/U14 with N = 1); a fully
//!   conflicting `revert --partial` is reported like a blocked step and applies nothing;
//!   an erased region conflicts before a tainted one (`erased` wins).
//! - **R9** (U19, U22): a purge by text redacts a record region whose before or after
//!   text holds any 8-byte window of the text; `--and-remove` deletes every live copy, left
//!   to right without overlaps, as one batch by the owner, before the stacks are cleared.
//! - **R10** (U22 window): a record is outside the window when it is not pending on-behalf
//!   and its change is older than the last `keep_changes` changes; the window moves when
//!   [`Model::tick`] says so (time is the caller's).

pub mod hunk;
pub mod merge;

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::text::{diff, lines};

pub use hunk::Edit;
use hunk::Side;
use merge::Merged;

/// A participant id (U2: stacks are per pid, not per connection).
pub type Pid = String;
/// A change's stable id (U1): its log position.
pub type ChangeId = u64;

/// Why a region conflicts (U11, U22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Overlap,
    Taint,
    Erased,
}

/// One conflicting region, as reported (U11): its root, live range `[s', e')`, live lines
/// (first, last; 1-based) and kind. One per region, never a sub-range.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Report {
    pub root: usize,
    pub start: usize,
    pub end: usize,
    pub lines: (usize, usize),
    pub kind: Kind,
}

/// One region (U8): a canonical hunk of a change, carried.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Region {
    pub root: usize,
    /// Carried bounds in the current text.
    pub start: usize,
    pub end: usize,
    /// What undoing it puts back (`R_before`; "ours").
    pub before: String,
    /// What the change left (`R_after`; the merge's base).
    pub after: String,
    /// U9.
    pub tainted: bool,
    /// The whole root was non-empty right after the change (the merge's base; U10).
    pub base_doc_nonempty: bool,
    /// Redacted by a purge (revert records only; U22).
    pub erased: bool,
}

/// One batch on a stack (U1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Batch {
    /// The person an assistant made it for (`undo --for`, U2).
    pub on_behalf_of: Option<Pid>,
    /// Ordered by root, then start.
    pub regions: Vec<Region>,
    /// A redo entry made by a step that skipped regions (a case class, U17).
    pub after_partial: bool,
}

/// One pid's stacks (U2), oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Stacks {
    pub undo: Vec<Batch>,
    pub redo: Vec<Batch>,
    /// A purge cleared them; said once, on the next undo or redo that finds nothing, unless a
    /// new batch came first (U19).
    pub cleared: bool,
}

/// A change's revert record (U22): its regions, carried, and who made it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Record {
    pub author: Pid,
    pub on_behalf_of: Option<Pid>,
    /// The change's hunks as it was made (per root, against the text before it).
    pub hunks: Vec<(usize, Vec<Edit>)>,
    /// Its regions as undo carries them (U8, U9: shared-edge clamp after every change,
    /// one taint per region): what undo of the same change would see.
    pub regions: Vec<Region>,
    /// The same regions' edges as revert carries them (U22a): each edge on its own.
    pub edges: Vec<Edges>,
    /// U22a's persisted carry: when the window moved past older changes while this record
    /// was kept (pending on-behalf), its carry was persisted with **one taint flag per
    /// region**; a restart reloads it, tainting both edges of a region tainted then.
    pub persisted: Option<Vec<bool>>,
}

/// A record region's edges as revert carries them (U22a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Edges {
    pub start: usize,
    pub end: usize,
    /// A later hunk deleted across the start (U9).
    pub start_taint: bool,
    /// A later hunk deleted across the end (U9).
    pub end_taint: bool,
}

/// The bounds (U6) and the revert window (U22), as knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounds {
    /// Batches per pid (U6: 100).
    pub depth: usize,
    /// Retained region text per pid, bytes (U6: 4 MiB).
    pub max_bytes: usize,
    /// The window's change count (U22: 10,000).
    pub keep_changes: u64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            depth: 100,
            max_bytes: 4 << 20,
            keep_changes: 10_000,
        }
    }
}

/// How an undo, redo or revert treats conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// U13.
    All,
    /// U14.
    Partial,
    /// U14's escape: remove the top step unapplied.
    Drop,
}

/// An undo or redo request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Request {
    pub steps: usize,
    pub mode: Mode,
    /// `--for <person>` (U2).
    pub for_person: Option<Pid>,
}

impl Request {
    pub fn new(steps: usize, mode: Mode) -> Self {
        Request {
            steps,
            mode,
            for_person: None,
        }
    }
}

/// What a "nothing" reply says (U18, U19; R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Notice {
    Plain,
    HistoryLost,
    ClearedByPurge,
}

/// The outcome of one operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// An edit, replace, restore, `write --base` or purge removal: the change it made, if
    /// any.
    Changed(Option<ChangeId>),
    /// `write --base` overlapped (O1, O2, O5); nothing applied.
    WriteConflict,
    /// Undo, redo or revert applied `steps` steps (U13–U17, U22). `skipped`: regions
    /// `--partial` left; `blocked`: the next step's regions, all conflicting, which stays on
    /// top. Sorted.
    Applied {
        steps: usize,
        skipped: Vec<Report>,
        blocked: Option<Vec<Report>>,
    },
    /// All or nothing: step `step` (1-based) conflicts in these regions; nothing applied.
    Conflicts { step: usize, regions: Vec<Report> },
    /// `--partial`: the top step's every region conflicts; nothing applied, it stays on top
    /// (for a revert: nothing applied).
    Blocked { regions: Vec<Report> },
    /// `--drop` removed the top step.
    Dropped,
    /// Nothing to undo or redo.
    Nothing(Notice),
    /// The change is outside the revert window (U22, exit 8).
    TooOld,
}

/// The case classes an operation hit (§8 Volume; the trace runner counts them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hits {
    /// O5 landed in an undo, redo or revert region.
    pub eof_append_undo: bool,
    /// O5 landed in `write --base`.
    pub eof_append_write: bool,
    /// O2: an insertion strictly inside the other side's range.
    pub insert_inside: bool,
    /// O1 in an undo, redo or revert region.
    pub overlap: bool,
    /// A taint conflict (U9).
    pub taint: bool,
    /// An erased conflict (U22).
    pub erased: bool,
    /// `--partial` applied a step with several regions and skipped some (U14).
    pub multi_hunk_partial: bool,
    /// `--partial` left an all-conflicting step on top (U14).
    pub stays_on_top: bool,
    /// A redo of a step whose undo skipped regions (U17).
    pub redo_after_partial: bool,
    /// An undo or redo with `--steps N`, N ≥ 2, applied ≥ 2 steps (U13).
    pub steps_n: bool,
    /// `undo --for` applied (U2).
    pub undo_for: bool,
    /// `--drop` (U14).
    pub dropped: bool,
    /// Nothing, with "cleared by purge" (U19).
    pub cleared_by_purge: bool,
    /// Nothing, with "history lost" (U18).
    pub history_lost: bool,
    /// Two `write --base` insertions at one point of an empty doc conflicted (O5).
    pub empty_base_double_write: bool,
    /// O5 landed for a region whose own base is empty (U12's worked case).
    pub last_line_after_append: bool,
    /// Same-point insertions at the doc's end conflicted because the whole doc was empty
    /// in the merge's base (U10).
    pub whole_doc_delete_then_append: bool,
    /// A planned step had two regions touching on one root (U8, U10).
    pub touching_regions: bool,
    /// A revert's change was outside the window.
    pub too_old: bool,
    /// A clean region whose merge hunks differ from a fresh diff of its window: repeated
    /// text, where only element identity (U10) says which characters change.
    pub identity_case: bool,
    /// A revert: U22a's property held (conflicts ⊇ undo's on the same state, same bounds,
    /// same text where both apply). `false` on anything but a revert.
    pub revert_superset_ok: bool,
    /// A revert: its regions, and how many conflict where undo would apply (U22a's cost).
    pub revert_regions: usize,
    pub revert_extra_conflicts: usize,
}

/// The oracle's state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Model {
    /// Every root's text.
    pub texts: Vec<String>,
    /// The newest change's id.
    pub head: ChangeId,
    pub stacks: BTreeMap<Pid, Stacks>,
    /// Revert records in the window (U22).
    pub records: BTreeMap<ChangeId, Record>,
    /// Pids that made any change.
    pub authors: BTreeSet<Pid>,
    /// Pids that made a change before the last restart (U18).
    pub authors_before_restart: BTreeSet<Pid>,
    pub bounds: Bounds,
}

/// A merged clean region: (index in the batch, merged window text, its edits in window
/// coordinates).
type Clean = (usize, String, Vec<Edit>);

impl Model {
    /// A doc whose roots hold `texts`, its newest change being `head` (an import is no
    /// participant's batch, U4).
    pub fn new(texts: Vec<String>, head: ChangeId, bounds: Bounds) -> Self {
        Model {
            texts,
            head,
            stacks: BTreeMap::new(),
            records: BTreeMap::new(),
            authors: BTreeSet::new(),
            authors_before_restart: BTreeSet::new(),
            bounds,
        }
    }

    /// `(undo, redo)` depths of `pid`.
    pub fn depths(&self, pid: &str) -> (usize, usize) {
        self.stacks
            .get(pid)
            .map_or((0, 0), |s| (s.undo.len(), s.redo.len()))
    }

    // ---------- changes ----------

    /// `edit`: explicit hunks per root (sorted, non-overlapping, valid), one batch (U1).
    pub fn edit(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        edits: Vec<(usize, Vec<Edit>)>,
    ) -> Outcome {
        let edits = edits
            .into_iter()
            .map(|(r, es)| (r, hunk::canonical(es)))
            .collect();
        self.new_batch(pid, obo, edits)
    }

    /// `replace_text`: the root becomes `new`, through the kit's pipeline (D1).
    pub fn replace(&mut self, pid: &str, obo: Option<&str>, root: usize, new: &str) -> Outcome {
        let edits = pipeline(&self.texts[root], new);
        self.new_batch(pid, obo, vec![(root, edits)])
    }

    /// `restore`: every root becomes `targets`' text, as one batch of the caller (U5, D1).
    pub fn restore(&mut self, pid: &str, obo: Option<&str>, targets: &[String]) -> Outcome {
        let edits = targets
            .iter()
            .enumerate()
            .map(|(r, t)| (r, pipeline(&self.texts[r], t)))
            .collect();
        self.new_batch(pid, obo, edits)
    }

    /// `write --base`: what the caller changed from `base` to `ours`, merged onto root
    /// `root` (§4).
    pub fn write_base(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        root: usize,
        base: &str,
        ours: &str,
    ) -> (Outcome, Hits) {
        let mut hits = Hits::default();
        let (m, rules) = merge::merge3(base, ours, &self.texts[root]);
        hits.eof_append_write = rules.o5_landed;
        hits.insert_inside = rules.o2;
        match m {
            Merged::Clean { edits, .. } => (self.new_batch(pid, obo, vec![(root, edits)]), hits),
            Merged::Conflict => {
                hits.empty_base_double_write = base.is_empty() && rules.same_point_at_window_end;
                (Outcome::WriteConflict, hits)
            }
        }
    }

    /// A participant's batch: applied, recorded, on their undo stack; it empties their redo
    /// stack (U7). A change that changes nothing is no batch.
    fn new_batch(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        edits: Vec<(usize, Vec<Edit>)>,
    ) -> Outcome {
        match self.commit(pid, obo, edits) {
            Some((id, regions)) => {
                let st = self.stacks.entry(pid.to_owned()).or_default();
                st.redo.clear();
                st.cleared = false;
                st.undo.push(Batch {
                    on_behalf_of: obo.map(str::to_owned),
                    regions,
                    after_partial: false,
                });
                self.bound(pid);
                Outcome::Changed(Some(id))
            }
            None => Outcome::Changed(None),
        }
    }

    /// U6 (R5): at most `depth` batches and `max_bytes` of region text per pid, both stacks
    /// together; the oldest go first.
    fn bound(&mut self, pid: &str) {
        let b = self.bounds;
        let Some(st) = self.stacks.get_mut(pid) else {
            return;
        };
        let bytes = |st: &Stacks| -> usize {
            st.undo
                .iter()
                .chain(&st.redo)
                .flat_map(|b| &b.regions)
                .map(|r| r.before.len() + r.after.len())
                .sum()
        };
        while st.undo.len() + st.redo.len() > b.depth || bytes(st) > b.max_bytes {
            if !st.undo.is_empty() {
                st.undo.remove(0);
            } else if !st.redo.is_empty() {
                st.redo.remove(0);
            } else {
                break;
            }
        }
    }

    /// Apply one change (canonical edits per root) by `author`: carry every region held
    /// (stacks and records) through it, record it (U22). `None` when it changes nothing.
    /// Returns its id and its regions (U8: one per canonical hunk).
    fn commit(
        &mut self,
        author: &str,
        obo: Option<&str>,
        mut edits: Vec<(usize, Vec<Edit>)>,
    ) -> Option<(ChangeId, Vec<Region>)> {
        edits.retain(|(_, es)| !es.is_empty());
        edits.sort_by_key(|(r, _)| *r);
        if edits.is_empty() {
            return None;
        }
        let mut regions = Vec::new();
        let hunks = edits.clone();
        for (root, es) in edits {
            let before = std::mem::take(&mut self.texts[root]);
            let after = hunk::apply(&before, &es);
            let mut delta: isize = 0;
            for e in &es {
                let start = (e.start as isize + delta) as usize;
                delta += e.growth();
                regions.push(Region {
                    root,
                    start,
                    end: start + e.insert.len(),
                    before: before[e.start..e.end].to_owned(),
                    after: e.insert.clone(),
                    tainted: false,
                    base_doc_nonempty: !after.is_empty(),
                    erased: false,
                });
            }
            self.carry_all(root, &es);
            self.texts[root] = after;
        }
        self.head += 1;
        self.authors.insert(author.to_owned());
        self.records.insert(
            self.head,
            Record {
                author: author.to_owned(),
                on_behalf_of: obo.map(str::to_owned),
                hunks,
                edges: regions
                    .iter()
                    .map(|r| Edges {
                        start: r.start,
                        end: r.end,
                        start_taint: false,
                        end_taint: false,
                    })
                    .collect(),
                regions: regions.clone(),
                persisted: None,
            },
        );
        Some((self.head, regions))
    }

    /// Carry every held region through a change to `root` (U8, U9).
    fn carry_all(&mut self, root: usize, es: &[Edit]) {
        for st in self.stacks.values_mut() {
            for b in st.undo.iter_mut().chain(st.redo.iter_mut()) {
                carry_regions(&mut b.regions, root, es);
            }
        }
        for rec in self.records.values_mut() {
            carry_regions(&mut rec.regions, root, es);
            for (r, ed) in rec.regions.iter().zip(rec.edges.iter_mut()) {
                if r.root != root {
                    continue;
                }
                ed.start_taint |= hunk::crosses(ed.start, es);
                ed.end_taint |= hunk::crosses(ed.end, es);
                ed.start = hunk::carry(ed.start, es, Side::Start);
                ed.end = hunk::carry(ed.end, es, Side::End);
            }
        }
    }

    // ---------- undo, redo, revert ----------

    /// `undo` (U13, U14; `--for`, U2).
    pub fn undo(&mut self, pid: &str, obo: Option<&str>, req: &Request) -> (Outcome, Hits) {
        self.undo_redo(pid, obo, req, false)
    }

    /// `redo` (U17).
    pub fn redo(&mut self, pid: &str, obo: Option<&str>, req: &Request) -> (Outcome, Hits) {
        self.undo_redo(pid, obo, req, true)
    }

    fn undo_redo(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        req: &Request,
        redo: bool,
    ) -> (Outcome, Hits) {
        let mut hits = Hits::default();
        let st = self.stacks.entry(pid.to_owned()).or_default();
        let cleared = st.cleared;
        let src = if redo { &st.redo } else { &st.undo };
        let want = if req.mode == Mode::Drop {
            1
        } else {
            req.steps.max(1)
        };
        let picked: Vec<usize> = (0..src.len())
            .rev()
            .filter(|&k| match &req.for_person {
                Some(p) => src[k].on_behalf_of.as_ref() == Some(p),
                None => true,
            })
            .take(want)
            .collect();
        if picked.is_empty() {
            let notice = if cleared {
                hits.cleared_by_purge = true;
                self.stacks.get_mut(pid).expect("entry made above").cleared = false;
                Notice::ClearedByPurge
            } else if !redo && src.is_empty() && self.authors_before_restart.contains(pid) {
                hits.history_lost = true;
                Notice::HistoryLost
            } else {
                Notice::Plain
            };
            return (Outcome::Nothing(notice), hits);
        }
        if req.mode == Mode::Drop {
            let st = self.stacks.get_mut(pid).expect("entry made above");
            if redo {
                st.redo.remove(picked[0]);
            } else {
                st.undo.remove(picked[0]);
            }
            hits.dropped = true;
            return (Outcome::Dropped, hits);
        }
        // plan on a scratch copy (U13): every step applied to it, carrying the rest
        let mut work = self.clone();
        let mut done = 0;
        let mut skipped = Vec::new();
        let mut blocked = None;
        for (k, &idx) in picked.iter().enumerate() {
            let batch = {
                let st = &work.stacks[pid];
                if redo {
                    st.redo[idx].clone()
                } else {
                    st.undo[idx].clone()
                }
            };
            if redo && batch.after_partial {
                hits.redo_after_partial = true;
            }
            if touching(&batch.regions) {
                hits.touching_regions = true;
            }
            let (clean, conflicts) = work.merge_regions(&batch.regions, &mut hits);
            if !conflicts.is_empty() {
                match req.mode {
                    Mode::All => {
                        return (
                            Outcome::Conflicts {
                                step: k + 1,
                                regions: sorted(conflicts),
                            },
                            hits,
                        );
                    }
                    _ if clean.is_empty() => {
                        hits.stays_on_top = true;
                        blocked = Some(sorted(conflicts));
                        break;
                    }
                    _ => {
                        if batch.regions.len() > 1 {
                            hits.multi_hunk_partial = true;
                        }
                    }
                }
            }
            let partial = !conflicts.is_empty();
            skipped.extend(conflicts);
            let inverse = work.apply_step(pid, obo, &batch.regions, &clean);
            let st = work.stacks.get_mut(pid).expect("entry made above");
            // a new batch on the pid's stacks withdraws the purge notice (U19)
            st.cleared = false;
            let (from, to) = if redo {
                (&mut st.redo, &mut st.undo)
            } else {
                (&mut st.undo, &mut st.redo)
            };
            // picked is newest first: removing idx moves none of the indices still to come
            from.remove(idx);
            to.push(Batch {
                on_behalf_of: batch.on_behalf_of.clone(),
                regions: inverse,
                after_partial: partial,
            });
            work.bound(pid);
            done += 1;
        }
        if done == 0 {
            let regions = blocked.unwrap_or_default();
            return (Outcome::Blocked { regions }, hits);
        }
        *self = work;
        if done >= 2 {
            hits.steps_n = true;
        }
        if req.for_person.is_some() && !redo {
            hits.undo_for = true;
        }
        (
            Outcome::Applied {
                steps: done,
                skipped: sorted(skipped),
                blocked,
            },
            hits,
        )
    }

    /// `revert <change>` (U22): the record's regions merged as one undo step (R8); a new
    /// batch of the reverter.
    pub fn revert(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        change: ChangeId,
        partial: bool,
    ) -> (Outcome, Hits) {
        let mut hits = Hits::default();
        let Some(rec) = self.records.get(&change) else {
            hits.too_old = true;
            return (Outcome::TooOld, hits);
        };
        let regions = revert_regions(rec);
        let undo_regions = rec.regions.clone();
        if touching(&regions) {
            hits.touching_regions = true;
        }
        let (clean, conflicts) = self.merge_regions(&regions, &mut hits);
        // U22a's property, against undo of the same change on the same state
        let mut probe = Hits::default();
        let (u_clean, u_conflicts) = self.merge_regions(&undo_regions, &mut probe);
        let bounds = |r: &Region| (r.root, r.start, r.end);
        let same_bounds = regions.len() == undo_regions.len()
            && regions
                .iter()
                .zip(&undo_regions)
                .all(|(a, b)| bounds(a) == bounds(b));
        let u_conflicting: BTreeSet<usize> = (0..undo_regions.len())
            .filter(|i| !u_clean.iter().any(|c| c.0 == *i))
            .collect();
        let r_conflicting: BTreeSet<usize> = (0..regions.len())
            .filter(|i| !clean.iter().any(|c| c.0 == *i))
            .collect();
        let same_text = clean.iter().all(|(i, text, _)| {
            u_clean
                .iter()
                .find(|c| c.0 == *i)
                .map_or(true, |c| c.1 == *text)
        });
        hits.revert_superset_ok =
            same_bounds && r_conflicting.is_superset(&u_conflicting) && same_text;
        hits.revert_regions = regions.len();
        hits.revert_extra_conflicts = r_conflicting.difference(&u_conflicting).count();
        debug_assert_eq!(u_conflicts.len(), u_conflicting.len());
        if !conflicts.is_empty() {
            if !partial {
                let regions = sorted(conflicts);
                return (Outcome::Conflicts { step: 1, regions }, hits);
            }
            if clean.is_empty() {
                let regions = sorted(conflicts);
                return (Outcome::Blocked { regions }, hits);
            }
        }
        let edits = step_edits(&self.texts, &regions, &clean);
        self.new_batch(pid, obo, edits);
        (
            Outcome::Applied {
                steps: 1,
                skipped: sorted(conflicts),
                blocked: None,
            },
            hits,
        )
    }

    /// Merge each region against the live text (U10): the clean ones, and the reports of
    /// the conflicting ones (U11).
    fn merge_regions(&self, regions: &[Region], hits: &mut Hits) -> (Vec<Clean>, Vec<Report>) {
        let mut clean = Vec::new();
        let mut conflicts = Vec::new();
        for (i, r) in regions.iter().enumerate() {
            let live = &self.texts[r.root];
            let report = |kind| report(live, r, kind);
            if r.erased {
                hits.erased = true;
                conflicts.push(report(Kind::Erased));
                continue;
            }
            if r.tainted {
                hits.taint = true;
                conflicts.push(report(Kind::Taint));
                continue;
            }
            let theirs = &live[r.start..r.end];
            // U10: the live doc's end, and the whole doc non-empty in the base
            let at_end = r.end == live.len() && r.base_doc_nonempty;
            let (m, rules) = merge::merge3_window(&r.after, &r.before, theirs, at_end);
            hits.insert_inside |= rules.o2;
            hits.overlap |= rules.o1;
            if rules.o5_landed {
                hits.eof_append_undo = true;
                if r.after.is_empty() {
                    hits.last_line_after_append = true;
                }
            }
            if rules.same_point_at_window_end && r.end == live.len() && !r.base_doc_nonempty {
                hits.whole_doc_delete_then_append = true;
            }
            match m {
                Merged::Clean { text, edits } => {
                    // the N4 case: a re-diff of the window would change other elements
                    let rediff: Vec<Edit> = diff::hunks(theirs, &text)
                        .into_iter()
                        .map(Edit::from)
                        .collect();
                    if rediff != edits {
                        hits.identity_case = true;
                    }
                    clean.push((i, text, edits))
                }
                Merged::Conflict => conflicts.push(report(Kind::Overlap)),
            }
        }
        (clean, conflicts)
    }

    /// Apply one undo or redo step's clean regions as one change by `pid`; returns the
    /// regions for the opposite stack (R4), in the coordinates after the step.
    fn apply_step(
        &mut self,
        pid: &str,
        obo: Option<&str>,
        regions: &[Region],
        clean: &[Clean],
    ) -> Vec<Region> {
        let edits = step_edits(&self.texts, regions, clean);
        let mut inverse = Vec::with_capacity(clean.len());
        let mut shift: BTreeMap<usize, isize> = BTreeMap::new();
        for (i, merged, _) in clean {
            let r = &regions[*i];
            let s = shift.entry(r.root).or_insert(0);
            let start = (r.start as isize + *s) as usize;
            *s += merged.len() as isize - (r.end - r.start) as isize;
            inverse.push(Region {
                root: r.root,
                start,
                end: start + merged.len(),
                before: self.texts[r.root][r.start..r.end].to_owned(),
                after: merged.clone(),
                tainted: false,
                base_doc_nonempty: true, // set below
                erased: false,
            });
        }
        self.commit(pid, obo, edits);
        for g in &mut inverse {
            g.base_doc_nonempty = !self.texts[g.root].is_empty();
        }
        inverse
    }

    // ---------- purge, restart, window ----------

    /// Purge by text with `--and-remove` (U19, R9): every live copy deleted as one batch by
    /// `owner`; then every stack of the doc cleared (holders get the notice once) and every
    /// record region holding the text redacted.
    pub fn purge_text(&mut self, owner: &str, obo: Option<&str>, secret: &str) -> Outcome {
        let edits: Vec<(usize, Vec<Edit>)> = self
            .texts
            .iter()
            .enumerate()
            .map(|(r, t)| {
                let es = t
                    .match_indices(secret)
                    .map(|(i, m)| Edit {
                        start: i,
                        end: i + m.len(),
                        insert: String::new(),
                    })
                    .collect();
                (r, es)
            })
            .collect();
        let out = self.new_batch(owner, obo, edits);
        for st in self.stacks.values_mut() {
            if !st.undo.is_empty() || !st.redo.is_empty() {
                st.cleared = true;
            }
            st.undo.clear();
            st.redo.clear();
        }
        let windows: Vec<&[u8]> = secret.as_bytes().windows(8).collect();
        let holds = |t: &str| {
            windows
                .iter()
                .any(|w| t.as_bytes().windows(8).any(|x| x == *w))
        };
        for rec in self.records.values_mut() {
            for r in &mut rec.regions {
                if holds(&r.before) || holds(&r.after) {
                    r.erased = true;
                }
            }
        }
        out
    }

    /// A host restart (U6, U18): every stack is lost; records stay (U22).
    pub fn restart(&mut self) {
        self.stacks.clear();
        self.authors_before_restart = self.authors.clone();
        // U22a: persisted carries come back with one taint flag per region
        for rec in self.records.values_mut() {
            if let Some(flags) = &rec.persisted {
                for (ed, t) in rec.edges.iter_mut().zip(flags) {
                    ed.start_taint |= *t;
                    ed.end_taint |= *t;
                }
            }
        }
    }

    /// The revert window moves (R10): records older than the last `keep_changes` changes go,
    /// except pending on-behalf ones (no approval is ever settled in a trace, so every
    /// on-behalf change is pending).
    pub fn tick(&mut self) {
        let cut = self.head.saturating_sub(self.bounds.keep_changes);
        let dropped: Vec<ChangeId> = self
            .records
            .iter()
            .filter(|(id, rec)| **id <= cut && rec.on_behalf_of.is_none())
            .map(|(id, _)| *id)
            .collect();
        let Some(&last) = dropped.last() else {
            return;
        };
        for id in dropped {
            self.records.remove(&id);
        }
        // U22a: the kept (pending) records older than the move have their carry persisted
        for (_, rec) in self.records.range_mut(..last) {
            rec.persisted = Some(
                rec.edges
                    .iter()
                    .map(|e| e.start_taint || e.end_taint)
                    .collect(),
            );
        }
    }
}

/// D1: the edit pipeline for a new text.
fn pipeline(old: &str, new: &str) -> Vec<Edit> {
    lines::align_insertions(old, diff::hunks(old, new))
        .into_iter()
        .map(Edit::from)
        .collect()
}

/// Carry one batch's (or record's) regions through a change to `root` (U8, U9), then the
/// shared-edge rule (U8): two regions of it that touch keep an insertion at the shared
/// edge in the left one only.
fn carry_regions(regions: &mut [Region], root: usize, es: &[Edit]) {
    let mut any = false;
    for r in regions.iter_mut().filter(|r| r.root == root) {
        if hunk::crosses(r.start, es) || hunk::crosses(r.end, es) {
            r.tainted = true;
        }
        r.start = hunk::carry(r.start, es, Side::Start);
        r.end = hunk::carry(r.end, es, Side::End);
        any = true;
    }
    if any {
        for k in 1..regions.len() {
            let (l, rest) = regions.split_at_mut(k);
            let (prev, next) = (&l[k - 1], &mut rest[0]);
            if prev.root == next.root && next.start < prev.end {
                next.start = prev.end;
                next.end = next.end.max(next.start);
            }
        }
    }
}

/// A record's regions as revert sees them (U22a): its own carried edges and taints; a
/// region whose start is at or before the previous region's end (same root) takes that
/// end's taint; then the shared-edge clamp.
fn revert_regions(rec: &Record) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::with_capacity(rec.regions.len());
    let mut prev: Option<(usize, usize, bool)> = None;
    for (g, ed) in rec.regions.iter().zip(&rec.edges) {
        let inherits = prev.is_some_and(|(root, end, t)| root == g.root && ed.start <= end && t);
        prev = Some((g.root, ed.end, ed.end_taint));
        out.push(Region {
            start: ed.start,
            end: ed.end.max(ed.start),
            tainted: ed.start_taint || ed.end_taint || inherits,
            ..g.clone()
        });
    }
    for k in 1..out.len() {
        let (l, rest) = out.split_at_mut(k);
        let (prev, next) = (&l[k - 1], &mut rest[0]);
        if prev.root == next.root && next.start < prev.end {
            next.start = prev.end;
            next.end = next.end.max(next.start);
        }
    }
    out
}

/// Two regions of one batch touch on a root.
fn touching(regions: &[Region]) -> bool {
    regions
        .windows(2)
        .any(|w| w[0].root == w[1].root && w[0].end == w[1].start)
}

/// A step's change: each clean region's merged edits offset by its start, per root,
/// canonical (readings R2, R3).
fn step_edits(texts: &[String], regions: &[Region], clean: &[Clean]) -> Vec<(usize, Vec<Edit>)> {
    let mut per: BTreeMap<usize, Vec<Edit>> = BTreeMap::new();
    for (i, _, edits) in clean {
        let r = &regions[*i];
        per.entry(r.root)
            .or_default()
            .extend(edits.iter().map(|e| Edit {
                start: e.start + r.start,
                end: e.end + r.start,
                insert: e.insert.clone(),
            }));
    }
    per.into_iter()
        .map(|(root, es)| {
            let es = hunk::canonical(es);
            debug_assert!(es.iter().all(|e| e.end <= texts[root].len()));
            (root, es)
        })
        .collect()
}

/// U11's report for region `r` of `live`.
fn report(live: &str, r: &Region, kind: Kind) -> Report {
    let first = hunk::line_at(live, r.start);
    let last = if r.end > r.start {
        hunk::line_at(live, r.end - 1)
    } else {
        first
    };
    Report {
        root: r.root,
        start: r.start,
        end: r.end,
        lines: (first, last),
        kind,
    }
}

fn sorted(mut v: Vec<Report>) -> Vec<Report> {
    v.sort();
    v
}

#[cfg(test)]
mod tests;
