//! The model check's bounded world (spec §8 "Model check"): `spec_undo::model` driven by a
//! finite action space, with element authors tracked beside it and the spec's rules checked
//! on every transition. The exhaustive enumerator (`undo_enum`) and the ITF replay of the
//! Quint model (`spec/quint/undo.qnt`) both run on it, so the two are one abstraction.
//!
//! **Abstraction** (the same in `undo.qnt`, definition by definition):
//! - One root; text over the alphabet `{a, b}` (no `\n`, so every report's lines are
//!   `(1, 1)`); each character carries its **author** (0: the seed text, an import that is
//!   no participant's batch; pid `k` is `p{k}` and author `k`). Element identity beyond the
//!   author is not kept: what `hist.undo_own_only` checks is who owns what an undo deletes.
//! - Pids `p1..pN`, every one acting for nobody, all with every right.
//! - **Events** per pid (budget `events`): an edit or a purge. Edits are explicit canonical
//!   hunks (no diff): insert one character, delete one, replace one (with the other
//!   letter), delete two, and (with `multi`) two single-character deletes or replacements
//!   at least one character apart (two regions). A purge removes every live copy of a
//!   letter, as the purger's batch, then clears the stacks (U19; R9's 8-byte windows never
//!   match at these sizes, so no record region is ever erased).
//! - **Undos** (budget `undos`, shared): `undo` all or nothing or `--partial`, `--steps` 1
//!   or 2; `undo --drop`; `redo` all or nothing or `--partial`. Every request spends one,
//!   whatever it finds.
//! - **Reverts** (budget `reverts`, shared): `revert <change>` of any recorded change by any
//!   pid, all or nothing or `--partial`.
//! - **No** restart, window move, `write --base`, `replace_text`, `restore`, `undo --for`
//!   or on-behalf edits here: the trace test covers them (§8).
//!
//! **Reductions** (each machine-checked by a test at small bounds):
//! - **The cut:** no state whose text is longer than `maxlen` (an action that would make
//!   one is not taken). So every string the merge diffs is at most `maxlen` long, which is
//!   what lets `undo.qnt` pin `diff::hunks` (`spec/quint/undo_diff.qnt`, equal on every pair of
//!   texts up to 4 long: `maxlen` ≤ 4 for any bound compared with Quint).
//! - **Records after the last revert:** once the revert budget is spent, no action reads
//!   a revert record again, so they are dropped (and `head` with them).
//! - **Fields no rule reads** are not state: a record's original hunks (diagnostics),
//!   a batch's `after_partial` and `on_behalf_of`, the `authors` sets (no restart).
//! - **Pid symmetry** (enumerator only, `--sym`): pids are interchangeable (same budgets,
//!   same rights), so states equal up to renaming pids are one.
//!
//! **Transition checks** (each a rule id; the Quint model checks the same, as invariants
//! over its `viol` set): `own_only` (`hist.undo_own_only`: every character an undo or redo
//! by `p` deletes is `p`'s; every one a revert of change `c` deletes is `c`'s author's),
//! `U9` (a tainted region is never applied), `U13` (a refused all-or-nothing request or a
//! blocked one changes nothing), `U15` (all or nothing's conflicts = `--partial`'s reports,
//! first conflicting step), `U17` (a step pushes one region per region it applied),
//! `U19` (a purge leaves every stack empty), `U22a` (revert's superset property) and `O5`
//! (O5 lands exactly at the doc's end with a non-empty whole-doc base: never elsewhere, and
//! never refused there).

use std::collections::{BTreeMap, BTreeSet};

use crate::model::hunk::Edit;
use crate::model::{
    merge, ChangeId, Hits, Kind, Mode, Model, Outcome, Region, Report, Request, StepNote,
};

/// A bound: what the enumeration (or a Quint instance) explores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// Pids `p1..pN`.
    pub pids: usize,
    /// Events (edits or purges) per pid.
    pub events: u8,
    /// Undo-family requests, shared.
    pub undos: u8,
    /// Reverts, shared.
    pub reverts: u8,
    /// The longest text explored (the cut).
    pub maxlen: usize,
    /// The seed text (author 0).
    pub init: String,
    /// Two-region edits.
    pub multi: bool,
    /// Purges among the events.
    pub purge: bool,
    /// Drop records once the revert budget is spent (off only to check the reduction).
    pub reduce: bool,
    /// Also check U12's equivalence (a diagnostic; not in the Quint model).
    pub u12: bool,
}

impl Bound {
    /// The TLC-sized bound: 2 pids × 2 events + 1 undo, no revert.
    pub fn small() -> Self {
        Bound {
            pids: 2,
            events: 2,
            undos: 1,
            reverts: 0,
            maxlen: 4,
            init: "ab".into(),
            multi: true,
            purge: true,
            reduce: true,
            u12: false,
        }
    }

    /// The G1 bound: 3 pids × 3 events, 2 undos, 1 revert (spec §8 Model check), from
    /// `abb` (three letters, so a two-region edit exists from the start), `maxlen` 4.
    pub fn g1() -> Self {
        Bound {
            pids: 3,
            events: 3,
            undos: 2,
            reverts: 1,
            init: "abb".into(),
            ..Self::small()
        }
    }

    /// One line for logs and checkpoints.
    pub fn describe(&self) -> String {
        format!(
            "pids={} events={} undos={} reverts={} maxlen={} init={:?} multi={} purge={} reduce={} u12={}",
            self.pids,
            self.events,
            self.undos,
            self.reverts,
            self.maxlen,
            self.init,
            self.multi,
            self.purge,
            self.reduce,
            self.u12
        )
    }
}

/// Pid `k` (0-based) is `p{k+1}`, author `k+1`.
pub fn pid(k: usize) -> String {
    format!("p{}", k + 1)
}

fn author_of(pid: &str) -> u8 {
    pid[1..].parse().expect("a pid p<k>")
}

/// One action of the bounded world.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Act {
    /// An edit: canonical hunks on the root.
    Edit { p: usize, hunks: Vec<Edit> },
    /// A purge of every live copy of `secret` (one letter).
    Purge { p: usize, secret: char },
    /// `undo` (`mode` All, Partial or Drop; `steps` 1 or 2, 1 for Drop).
    Undo { p: usize, mode: Mode, steps: usize },
    /// `redo` (All or Partial), one step.
    Redo { p: usize, mode: Mode },
    /// `revert <change>`.
    Revert {
        p: usize,
        change: ChangeId,
        partial: bool,
    },
}

impl Act {
    pub fn pid(&self) -> usize {
        match self {
            Act::Edit { p, .. }
            | Act::Purge { p, .. }
            | Act::Undo { p, .. }
            | Act::Redo { p, .. }
            | Act::Revert { p, .. } => *p,
        }
    }
}

/// The text with authors, the stacks and the records (U13's "nothing changed").
type Core = (Vec<(char, u8)>, Vec<VStacks>, Vec<(ChangeId, VRecord)>);

/// A state of the bounded world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The oracle (one root).
    pub m: Model,
    /// Each character's author (0: the seed).
    pub authors: Vec<u8>,
    /// Events spent per pid.
    pub events: Vec<u8>,
    pub undos: u8,
    pub reverts: u8,
    /// Rules violated so far (empty on the oracle of record).
    pub viol: BTreeSet<&'static str>,
}

/// A region as state ([`View`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VRegion {
    pub start: usize,
    pub end: usize,
    pub before: String,
    pub after: String,
    pub tainted: bool,
    pub base_doc_nonempty: bool,
    pub erased: bool,
}

/// One pid's stacks as state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VStacks {
    pub undo: Vec<Vec<VRegion>>,
    pub redo: Vec<Vec<VRegion>>,
    pub cleared: bool,
}

/// A revert record as state: author, regions (undo's carry) and edges (revert's carry:
/// start, end, start tainted, end tainted).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VRecord {
    pub author: u8,
    pub regions: Vec<VRegion>,
    pub edges: Vec<(usize, usize, bool, bool)>,
}

/// A state as the model check sees it: exactly the Quint model's state (`undo.qnt`'s
/// `st`), so distinct views are distinct Quint states and vice versa.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct View {
    /// (letter, author) per character.
    pub doc: Vec<(char, u8)>,
    /// Per pid, `p1` first.
    pub stacks: Vec<VStacks>,
    pub records: Vec<(ChangeId, VRecord)>,
    pub head: ChangeId,
    pub events: Vec<u8>,
    pub undos: u8,
    pub reverts: u8,
    pub viol: Vec<String>,
}

fn vregion(r: &Region) -> VRegion {
    VRegion {
        start: r.start,
        end: r.end,
        before: r.before.clone(),
        after: r.after.clone(),
        tainted: r.tainted,
        base_doc_nonempty: r.base_doc_nonempty,
        erased: r.erased,
    }
}

/// The edits an event may make on `t` (see the module docs).
pub fn edits(t: &str, b: &Bound) -> Vec<Vec<Edit>> {
    let n = t.len();
    let c = t.as_bytes();
    let e = |s: usize, t: usize, i: &str| Edit {
        start: s,
        end: t,
        insert: i.into(),
    };
    let other = |i: usize| if c[i] == b'a' { "b" } else { "a" };
    let mut out = Vec::new();
    if n < b.maxlen {
        for i in 0..=n {
            for l in ["a", "b"] {
                out.push(vec![e(i, i, l)]);
            }
        }
    }
    for i in 0..n {
        out.push(vec![e(i, i + 1, "")]);
        out.push(vec![e(i, i + 1, other(i))]);
    }
    for i in 0..n.saturating_sub(1) {
        out.push(vec![e(i, i + 2, "")]);
    }
    if b.multi {
        let ops = |i: usize| [e(i, i + 1, ""), e(i, i + 1, other(i))];
        for i in 0..n {
            for j in i + 2..n {
                for x in ops(i) {
                    for y in ops(j) {
                        out.push(vec![x.clone(), y]);
                    }
                }
            }
        }
    }
    out
}

/// A U11 report, compared without its lines (always `(1, 1)` here).
fn rkey(r: &Report) -> (usize, usize, Kind) {
    (r.start, r.end, r.kind)
}

impl State {
    pub fn init(b: &Bound) -> Self {
        assert!(b.init.bytes().all(|x| x == b'a' || x == b'b'));
        assert!(b.init.len() <= b.maxlen && (1..=9).contains(&b.pids));
        State {
            m: Model::new(vec![b.init.clone()], 1, Default::default()),
            authors: vec![0; b.init.len()],
            events: vec![0; b.pids],
            undos: 0,
            reverts: 0,
            viol: BTreeSet::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.m.texts[0]
    }

    /// Every action enabled here, in a fixed order (the cut is applied by [`State::step`]).
    pub fn actions(&self, b: &Bound) -> Vec<Act> {
        let mut out = Vec::new();
        let es = edits(self.text(), b);
        for p in 0..b.pids {
            if self.events[p] < b.events {
                for hunks in &es {
                    out.push(Act::Edit {
                        p,
                        hunks: hunks.clone(),
                    });
                }
                if b.purge {
                    for secret in ['a', 'b'] {
                        out.push(Act::Purge { p, secret });
                    }
                }
            }
            if self.undos < b.undos {
                for (mode, steps) in [
                    (Mode::All, 1),
                    (Mode::Partial, 1),
                    (Mode::All, 2),
                    (Mode::Partial, 2),
                    (Mode::Drop, 1),
                ] {
                    out.push(Act::Undo { p, mode, steps });
                }
                for mode in [Mode::All, Mode::Partial] {
                    out.push(Act::Redo { p, mode });
                }
            }
            if self.reverts < b.reverts {
                for &change in self.m.records.keys() {
                    for partial in [false, true] {
                        out.push(Act::Revert { p, change, partial });
                    }
                }
            }
        }
        out
    }

    /// Take `a`: the next state and the oracle's outcome, or `None` when the cut forbids it
    /// (the text would be longer than `maxlen`).
    pub fn step(&self, b: &Bound, a: &Act) -> Option<(State, Outcome)> {
        let mut s = self.clone();
        let p = pid(a.pid());
        let (out, hits) = match a {
            Act::Edit { hunks, .. } => {
                s.events[a.pid()] += 1;
                (
                    s.m.edit(&p, None, vec![(0, hunks.clone())]),
                    Hits::default(),
                )
            }
            Act::Purge { secret, .. } => {
                s.events[a.pid()] += 1;
                let o = s.m.purge_text(&p, None, &secret.to_string());
                (o, Hits::default())
            }
            Act::Undo { mode, steps, .. } => {
                s.undos += 1;
                s.m.undo(&p, None, &Request::new(*steps, *mode))
            }
            Act::Redo { mode, .. } => {
                s.undos += 1;
                s.m.redo(&p, None, &Request::new(1, *mode))
            }
            Act::Revert {
                change, partial, ..
            } => {
                s.reverts += 1;
                s.m.revert(&p, None, *change, *partial)
            }
        };
        if s.text().len() > b.maxlen {
            return None;
        }
        // the authors: every new change's hunks, in order
        let mut deleted: Vec<u8> = Vec::new();
        for id in self.m.head + 1..=s.m.head {
            let rec = &s.m.records[&id];
            let who = author_of(&rec.author);
            for (root, es) in &rec.hunks {
                debug_assert_eq!(*root, 0);
                let mut delta: isize = 0;
                for e in es {
                    let at = (e.start as isize + delta) as usize;
                    let gone = at..at + (e.end - e.start);
                    deleted.extend(s.authors.drain(gone));
                    let new = std::iter::repeat(who).take(e.insert.len());
                    s.authors.splice(at..at, new);
                    delta += e.growth();
                }
            }
        }
        assert_eq!(s.authors.len(), s.text().len(), "authors track the text");
        s.check(b, self, a, &out, &hits, &deleted);
        if b.reduce && s.reverts == b.reverts {
            s.m.records.clear();
            s.m.head = 0;
        }
        Some((s, out))
    }

    /// The transition checks (module docs), against the pre-state `pre`.
    fn check(
        &mut self,
        b: &Bound,
        pre: &State,
        a: &Act,
        out: &Outcome,
        hits: &Hits,
        deleted: &[u8],
    ) {
        let me = a.pid() as u8 + 1;
        // own_only (hist.undo_own_only, per element)
        let owner = match a {
            Act::Undo { .. } | Act::Redo { .. } => Some(me),
            Act::Revert { change, .. } => pre.m.records.get(change).map(|r| author_of(&r.author)),
            _ => None,
        };
        if let Some(o) = owner {
            if deleted.iter().any(|x| *x != o) {
                self.viol.insert("own_only");
            }
        }
        // U9: a tainted region is never applied
        if hits.notes.iter().any(|n| n.tainted && !n.erased && n.clean) {
            self.viol.insert("U9");
        }
        // O5: exactly at the doc's end with a non-empty whole-doc base
        for n in &hits.notes {
            let at_doc_end = n.end == n.live.len() && n.base_doc_nonempty;
            if (n.o5_landed && !at_doc_end) || (n.same_point_end && at_doc_end) {
                self.viol.insert("O5");
            }
        }
        // U13: a refused or blocked request changes nothing
        if matches!(out, Outcome::Conflicts { .. } | Outcome::Blocked { .. })
            && self.view_core() != pre.view_core()
        {
            self.viol.insert("U13");
        }
        // U17: one pushed region per applied region
        if hits
            .steps
            .iter()
            .any(|s| !s.applied.is_empty() && s.pushed != s.applied.len())
        {
            self.viol.insert("U17");
        }
        // U15: all or nothing's conflicts = --partial's reports (the first conflicting step)
        let other = |mode: Mode| -> Hits {
            let mut m = pre.m.clone();
            let p = pid(a.pid());
            match a {
                Act::Undo { steps, .. } => m.undo(&p, None, &Request::new(*steps, mode)).1,
                Act::Redo { .. } => m.redo(&p, None, &Request::new(1, mode)).1,
                _ => unreachable!(),
            }
        };
        let pair = match a {
            Act::Undo {
                mode: Mode::All, ..
            }
            | Act::Redo {
                mode: Mode::All, ..
            } => Some((hits.steps.clone(), other(Mode::Partial).steps)),
            Act::Undo {
                mode: Mode::Partial,
                ..
            }
            | Act::Redo {
                mode: Mode::Partial,
                ..
            } => Some((other(Mode::All).steps, hits.steps.clone())),
            _ => None,
        };
        if let Some((all, part)) = pair {
            if !u15(&all, &part) {
                self.viol.insert("U15");
            }
        }
        // U19: a purge leaves every stack empty
        if matches!(a, Act::Purge { .. })
            && self
                .m
                .stacks
                .values()
                .any(|s| !s.undo.is_empty() || !s.redo.is_empty())
        {
            self.viol.insert("U19");
        }
        // U22a: revert's conflicts ⊇ undo's, same bounds, same text
        if matches!(a, Act::Revert { .. }) && *out != Outcome::TooOld && !hits.revert_superset_ok {
            self.viol.insert("U22a");
        }
        // U12 (diagnostic): outside taint, the window merge equals whole-text merge3
        if b.u12 {
            for n in hits.notes.iter().filter(|n| !n.tainted && !n.erased) {
                let (l, r) = (&n.live[..n.start], &n.live[n.end..]);
                let base = format!("{l}{}{r}", n.after);
                let ours = format!("{l}{}{r}", n.before);
                let (w, _) = merge::merge3(&base, &ours, &n.live);
                let theirs = &n.live[n.start..n.end];
                let (m, _) = merge::merge3_window(&n.after, &n.before, theirs, n.at_end);
                let whole = match w {
                    merge::Merged::Clean { text, .. } => Some(text),
                    merge::Merged::Conflict => None,
                };
                let window = match m {
                    merge::Merged::Clean { text, .. } => Some(format!("{l}{text}{r}")),
                    merge::Merged::Conflict => None,
                };
                if whole != window {
                    self.viol.insert("U12");
                }
            }
        }
    }

    /// The text, authors, stacks and records (U13's "nothing changed").
    fn view_core(&self) -> Core {
        let v = self.view(&identity(self.events.len()));
        (v.doc, v.stacks, v.records)
    }

    /// This state as the Quint model's state, pids renamed by `perm` (`perm[k]` is pid
    /// `k`'s new index).
    pub fn view(&self, perm: &[usize]) -> View {
        let n = self.events.len();
        let map = |a: u8| {
            if a == 0 {
                0
            } else {
                perm[a as usize - 1] as u8 + 1
            }
        };
        let mut stacks = vec![VStacks::default(); n];
        for (name, st) in &self.m.stacks {
            let k = author_of(name) as usize - 1;
            let batches = |v: &[crate::model::Batch]| -> Vec<Vec<VRegion>> {
                v.iter()
                    .map(|b| b.regions.iter().map(vregion).collect())
                    .collect()
            };
            stacks[perm[k]] = VStacks {
                undo: batches(&st.undo),
                redo: batches(&st.redo),
                cleared: st.cleared,
            };
        }
        let mut events = vec![0; n];
        for k in 0..n {
            events[perm[k]] = self.events[k];
        }
        View {
            doc: self
                .text()
                .chars()
                .zip(self.authors.iter().map(|a| map(*a)))
                .collect(),
            stacks,
            records: self
                .m
                .records
                .iter()
                .map(|(id, r)| {
                    (
                        *id,
                        VRecord {
                            author: map(author_of(&r.author)),
                            regions: r.regions.iter().map(vregion).collect(),
                            edges: r
                                .edges
                                .iter()
                                .map(|e| (e.start, e.end, e.start_taint, e.end_taint))
                                .collect(),
                        },
                    )
                })
                .collect(),
            head: self.m.head,
            events,
            undos: self.undos,
            reverts: self.reverts,
            viol: self.viol.iter().map(|v| v.to_string()).collect(),
        }
    }

    /// The canonical view: the least over every renaming of pids (`sym`), or the view.
    pub fn canonical(&self, sym: bool) -> View {
        let n = self.events.len();
        if !sym {
            return self.view(&identity(n));
        }
        perms(n)
            .iter()
            .map(|p| self.view(p))
            .min()
            .expect("one permutation at least")
    }
}

/// U15 on the step notes of one request planned both ways: the first step all or nothing
/// refuses reports, as a set, what `--partial` reports for that step; with no refusal,
/// `--partial` reports nothing.
fn u15(all: &[StepNote], part: &[StepNote]) -> bool {
    let set =
        |s: &StepNote| -> BTreeSet<(usize, usize, Kind)> { s.reports.iter().map(rkey).collect() };
    match all.iter().position(|s| !s.reports.is_empty()) {
        Some(k) => part.get(k).is_some_and(|p| set(p) == set(&all[k])),
        None => part.iter().all(|s| s.reports.is_empty()),
    }
}

pub fn identity(n: usize) -> Vec<usize> {
    (0..n).collect()
}

impl View {
    /// One line of canonical JSON (`tools/undo/statediff.py` writes TLC's states the same
    /// way, to compare the two state sets exactly).
    pub fn json(&self) -> String {
        let q = |s: &str| format!("\"{s}\"");
        let b = |x: bool| if x { "true" } else { "false" };
        let region = |r: &VRegion| {
            format!(
                "[{},{},{},{},{},{},{}]",
                r.start,
                r.end,
                q(&r.before),
                q(&r.after),
                b(r.tainted),
                b(r.base_doc_nonempty),
                b(r.erased)
            )
        };
        let batch =
            |v: &[VRegion]| format!("[{}]", v.iter().map(region).collect::<Vec<_>>().join(","));
        let batches = |v: &[Vec<VRegion>]| {
            format!(
                "[{}]",
                v.iter().map(|x| batch(x)).collect::<Vec<_>>().join(",")
            )
        };
        let doc: Vec<String> = self
            .doc
            .iter()
            .map(|(c, a)| format!("[\"{c}\",{a}]"))
            .collect();
        let stacks: Vec<String> = self
            .stacks
            .iter()
            .map(|s| {
                format!(
                    "[{},{},{}]",
                    batches(&s.undo),
                    batches(&s.redo),
                    b(s.cleared)
                )
            })
            .collect();
        let records: Vec<String> = self
            .records
            .iter()
            .map(|(id, r)| {
                let edges: Vec<String> = r
                    .edges
                    .iter()
                    .map(|e| format!("[{},{},{},{}]", e.0, e.1, b(e.2), b(e.3)))
                    .collect();
                format!(
                    "[{id},{},{},[{}]]",
                    r.author,
                    batch(&r.regions),
                    edges.join(",")
                )
            })
            .collect();
        let ev: Vec<String> = self.events.iter().map(u8::to_string).collect();
        let viol: Vec<String> = self.viol.iter().map(|v| q(v)).collect();
        format!(
            "[[{}],[{}],[{}],{},[{}],{},{},[{}]]",
            doc.join(","),
            stacks.join(","),
            records.join(","),
            self.head,
            ev.join(","),
            self.undos,
            self.reverts,
            viol.join(",")
        )
    }
}

/// Every permutation of `0..n`.
pub fn perms(n: usize) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new()];
    for k in 0..n {
        let mut next = Vec::new();
        for p in &out {
            for at in 0..=k {
                let mut q = p.clone();
                q.insert(at, k);
                next.push(q);
            }
        }
        out = next;
    }
    out
}

/// Counts from a breadth-first enumeration ([`bfs`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Distinct (canonical) states per depth (actions taken), the initial state at 0.
    pub per_depth: Vec<u64>,
    /// Transitions taken (cut ones excluded).
    pub transitions: u64,
    /// Actions refused by the cut.
    pub cut: u64,
    /// Rule → the least depth at which it was first violated, and the action sequence.
    pub violations: BTreeMap<&'static str, (usize, Vec<Act>)>,
}

impl Counts {
    pub fn distinct(&self) -> u64 {
        self.per_depth.iter().sum()
    }
}

/// Every rule the transition checks can record.
pub const RULES: &[&str] = &[
    "own_only", "U9", "U13", "U15", "U17", "U19", "U22a", "O5", "U12",
];

/// The rules a run stops at by default: all but `own_only` and `U12`, which the oracle of
/// record violates (findings F3 and F4, `docs/MODEL-CHECK.md`), so they are observed
/// (with their least depth and a witness) and the states past them are explored.
pub const STOP: &[&str] = &["U9", "U13", "U15", "U17", "U19", "U22a", "O5"];

/// Breadth-first, with an exact visited set of canonical views: TLC's "distinct states"
/// (with `sym` off). A state that violates a rule in `stop` is counted and not expanded
/// (TLC stops at its first violation); every rule's least depth is reported. With
/// `first`, the run ends with the level where a rule in `stop` is first violated.
pub fn bfs(
    b: &Bound,
    sym: bool,
    stop: &[&str],
    first: bool,
    mut visit: impl FnMut(&State, &View),
) -> Counts {
    let mut counts = Counts::default();
    let s0 = State::init(b);
    let v0 = s0.canonical(sym);
    visit(&s0, &v0);
    let mut seen: std::collections::HashSet<View> = std::collections::HashSet::new();
    seen.insert(v0);
    let mut frontier: Vec<(State, Vec<Act>)> = vec![(s0, Vec::new())];
    counts.per_depth.push(1);
    while !frontier.is_empty() {
        let depth = counts.per_depth.len();
        let mut found = 0u64;
        let mut next = Vec::new();
        for (s, path) in &frontier {
            for a in s.actions(b) {
                let Some((t, _)) = s.step(b, &a) else {
                    counts.cut += 1;
                    continue;
                };
                counts.transitions += 1;
                let v = t.canonical(sym);
                if !seen.insert(v.clone()) {
                    continue;
                }
                found += 1;
                visit(&t, &v);
                let mut p = path.clone();
                p.push(a);
                for rule in t.viol.difference(&s.viol) {
                    counts
                        .violations
                        .entry(rule)
                        .or_insert_with(|| (depth, p.clone()));
                }
                if !t.viol.iter().any(|r| stop.contains(r)) {
                    next.push((t, p));
                }
            }
        }
        if found > 0 {
            counts.per_depth.push(found);
        }
        frontier = next;
        if first && counts.violations.keys().any(|r| stop.contains(r)) {
            break;
        }
    }
    counts
}

#[cfg(test)]
mod tests;
