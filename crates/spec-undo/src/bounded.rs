//! The model check's bounded world (spec §8 "Model check"): `spec_undo::model` driven by a
//! finite action space, with element authors tracked beside it and the spec's rules checked
//! on every transition. The exhaustive enumerator (`undo_enum`) and the ITF replay of the
//! Quint model (`spec/quint/undo.qnt`) both run on it, so the two are one abstraction.
//!
//! **Abstraction** (the same in `undo.qnt`, definition by definition):
//! - One root; text over the alphabet `{a, b}` (no `\n`, so every report's lines are
//!   `(1, 1)`); each character carries its **author** (0: the seed text, an import that is
//!   no participant's batch; pid `k` is `p{k}` and author `k`) and its **provenance root**
//!   (U23, v1.8). Element identity beyond these is not kept.
//! - **Roots as labels** (v1.8): the rules only compare roots for equality, and a fresh
//!   root equals no other, so a state holds them as labels `0, 1, ...` numbered by first
//!   appearance in a fixed walk of the state ([`View::relabel`]: the doc left to right; each
//!   pid's undo then redo batches, oldest first, each region's `before` roots then `after`
//!   roots; the records by id, the same per region). States equal up to renaming roots are
//!   one state; `undo.qnt` renumbers its state the same way after every step, and gives a
//!   fresh root the least label not in use.
//! - A region holds its bounds, `before`, `after`, ours, the roots of `before` and `after`,
//!   and its flags; theirs is read off the live window by root when it merges (U8, v1.8), so
//!   the v1.7 carried theirs (the oracle's cross-check) is not state, nor is a record
//!   edge's.
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
//! - **Restarts** (budget `restarts`, shared; v1.7): a host restart. Every stack is lost
//!   (U6); records stay (U22), each edge's taint included (U22a v1.6); a pid that made a
//!   change before it gets "history lost" on an empty undo (U18). With a restart budget the
//!   state also holds who made a change and who did before the last restart (the notice
//!   reads them); without one those fields stay empty, as nothing reads them.
//! - **No** window move, `write --base`, `replace_text`, `restore`, `undo --for` or
//!   on-behalf edits here: the trace test covers them (§8).
//!
//! **Reductions** (each machine-checked by a test at small bounds):
//! - **The cut:** no state whose text is longer than `maxlen` (an action that would make
//!   one is not taken). So every string the merge diffs is at most `maxlen` long, which is
//!   what lets `undo.qnt` pin `diff::hunks` (`spec/quint/undo_diff.qnt`, equal on every pair of
//!   texts up to 4 long: `maxlen` ≤ 4 for any bound compared with Quint).
//! - **Records after the last revert:** once the revert budget is spent, no action reads
//!   a revert record again, so they are dropped (and `head` with them).
//! - **Fields no rule reads** are not state: a record's original hunks (diagnostics),
//!   a batch's `after_partial` and `on_behalf_of`, the `authors` sets when the bound has no
//!   restart.
//! - **Pid symmetry** (enumerator only, `--sym`): pids are interchangeable (same budgets,
//!   same rights), so states equal up to renaming pids are one.
//!
//! **Transition checks** (each a rule id; the Quint model checks the same, as invariants
//! over its `viol` set): `own_only` (`hist.undo_own_only`, enforced since v1.7; by root
//! since v1.8, U10a: every character an undo, redo or revert deletes has a root the change
//! it reverses inserted, in the regions the step applied since v1.9), `I1` (v1.8: at most
//! one live element per root; after every step of an undo or redo since v1.9), `RT`
//! (restart transparency, U22a v1.6: a restart changes no revert's outcome or text, for
//! every record, all or nothing and `--partial`),
//! `U9` (a tainted region is never applied), `U13` (a refused all-or-nothing request or a
//! blocked one changes nothing), `U15` (all or nothing's conflicts = `--partial`'s reports,
//! first conflicting step), `U17` (a step pushes one region per region it applied),
//! `U19` (a purge leaves every stack empty), `U22a` (revert's superset property) and `O5`
//! (O5 lands exactly at the doc's end with a non-empty whole-doc base: never elsewhere, and
//! never refused there).

use std::collections::{BTreeMap, BTreeSet};

use crate::model::hunk::{Edit, Prov};
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
    /// Host restarts, shared (v1.7).
    pub restarts: u8,
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
            restarts: 0,
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
            "pids={} events={} undos={} reverts={} restarts={} maxlen={} init={:?} multi={} purge={} reduce={} u12={}",
            self.pids,
            self.events,
            self.undos,
            self.reverts,
            self.restarts,
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
    /// A host restart (no pid's; `p` is 0).
    Restart,
}

impl Act {
    pub fn pid(&self) -> usize {
        match self {
            Act::Edit { p, .. }
            | Act::Purge { p, .. }
            | Act::Undo { p, .. }
            | Act::Redo { p, .. }
            | Act::Revert { p, .. } => *p,
            Act::Restart => 0,
        }
    }
}

/// The text with authors and roots, the stacks and the records (U13's "nothing changed").
type Core = (
    Vec<(char, u8, usize)>,
    Vec<VStacks>,
    Vec<(ChangeId, VRecord)>,
);

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
    pub restarts: u8,
    /// The bound has restarts: the authors sets are state.
    pub track_authors: bool,
    /// The records were dropped (the revert budget is spent): the view's head is 0.
    pub reduced: bool,
    /// Rules violated so far (empty on the oracle of record).
    pub viol: BTreeSet<&'static str>,
    /// The case classes the transition into this state hit (E-HIST-13; not state).
    pub hit: Vec<&'static str>,
}

/// A region as state ([`View`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VRegion {
    pub start: usize,
    pub end: usize,
    pub before: String,
    pub after: String,
    /// Ours (U10 v1.7), as `(start, end, insert)`.
    pub ours: Vec<(usize, usize, String)>,
    /// The root labels of `before`'s and `after`'s characters (U23, v1.8).
    pub before_roots: Vec<usize>,
    pub after_roots: Vec<usize>,
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

/// One revert-carried edge pair (U22a): start, end, start tainted, end tainted.
pub type VEdges = (usize, usize, bool, bool);

/// A revert record as state: author, regions (undo's carry) and edges (revert's carry).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VRecord {
    pub author: u8,
    pub regions: Vec<VRegion>,
    pub edges: Vec<VEdges>,
}

/// A state as the model check sees it: exactly the Quint model's state (`undo.qnt`'s
/// `st`), so distinct views are distinct Quint states and vice versa.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct View {
    /// (letter, author, root label) per character.
    pub doc: Vec<(char, u8, usize)>,
    /// Per pid, `p1` first.
    pub stacks: Vec<VStacks>,
    pub records: Vec<(ChangeId, VRecord)>,
    pub head: ChangeId,
    pub events: Vec<u8>,
    pub undos: u8,
    pub reverts: u8,
    pub restarts: u8,
    /// Per pid: made a change; made one before the last restart (U18). Empty without a
    /// restart budget.
    pub authors: Vec<bool>,
    pub lost: Vec<bool>,
    pub viol: Vec<String>,
}

fn vedits(es: &[Edit]) -> Vec<(usize, usize, String)> {
    es.iter()
        .map(|e| (e.start, e.end, e.insert.clone()))
        .collect()
}

/// Roots as provisional labels (any injective numbering; [`View::relabel`] makes them
/// canonical).
#[derive(Default)]
struct Labels(std::collections::HashMap<Prov, usize>);

impl Labels {
    fn of(&mut self, p: &Prov) -> usize {
        let n = self.0.len();
        *self.0.entry(*p).or_insert(n)
    }
    fn all(&mut self, ps: &[Prov]) -> Vec<usize> {
        ps.iter().map(|p| self.of(p)).collect()
    }
}

fn vregion(r: &Region, l: &mut Labels) -> VRegion {
    VRegion {
        start: r.start,
        end: r.end,
        before: r.before.clone(),
        after: r.after.clone(),
        ours: vedits(&r.ours),
        before_roots: l.all(&r.before_roots),
        after_roots: l.all(&r.after_roots),
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
            restarts: 0,
            track_authors: b.restarts > 0,
            reduced: false,
            viol: BTreeSet::new(),
            hit: Vec::new(),
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
        if self.restarts < b.restarts {
            out.push(Act::Restart);
        }
        out
    }

    /// Take `a`: the next state and the oracle's outcome, or `None` when the cut forbids it
    /// (the text would be longer than `maxlen`).
    /// [`State::step`] with a panic in the model caught: `Err(message)`.
    pub fn try_step(&self, b: &Bound, a: &Act) -> Result<Option<(State, Outcome)>, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.step(b, a)))
            .map_err(|p| panic_message(p.as_ref()))
    }

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
            Act::Restart => {
                s.restarts += 1;
                s.m.restart();
                (Outcome::Changed(None), Hits::default())
            }
        };
        if s.text().len() > b.maxlen {
            return None;
        }
        // the authors: every new change's hunks, in order
        for id in self.m.head + 1..=s.m.head {
            let rec = &s.m.records[&id];
            let who = author_of(&rec.author);
            for (root, es) in &rec.hunks {
                debug_assert_eq!(*root, 0);
                let mut delta: isize = 0;
                for e in es {
                    let at = (e.start as isize + delta) as usize;
                    s.authors.drain(at..at + (e.end - e.start));
                    let new = std::iter::repeat(who).take(e.insert.len());
                    s.authors.splice(at..at, new);
                    delta += e.growth();
                }
            }
        }
        assert_eq!(s.authors.len(), s.text().len(), "authors track the text");
        debug_assert!(
            s.m.elems[0]
                .iter()
                .zip(&s.authors)
                .all(|(x, a)| s.m.who[x.owner as usize]
                    == if *a == 0 {
                        String::new()
                    } else {
                        pid(*a as usize - 1)
                    }),
            "the oracle's owners are the authors"
        );
        s.check(b, self, a, &out, &hits);
        if b.reduce && s.reverts == b.reverts {
            // the oracle's head goes on (it names fresh roots); the view's is 0
            s.m.records.clear();
            s.reduced = true;
        }
        Some((s, out))
    }

    /// The transition checks (module docs), against the pre-state `pre`.
    fn check(&mut self, b: &Bound, pre: &State, a: &Act, out: &Outcome, hits: &Hits) {
        let mut hit: Vec<&'static str> = Vec::new();
        // own_only (hist.undo_own_only, enforced since v1.7; by root since v1.8, U10a: the
        // oracle's Hits::foreign_deleted, from the roots it deleted; over the regions each
        // step applied since v1.9)
        if matches!(a, Act::Undo { .. } | Act::Redo { .. } | Act::Revert { .. })
            && hits.foreign_deleted
        {
            self.viol.insert("own_only");
        }
        // I1 (v1.8): at most one live element per root, in every state, and after every step
        // of an undo or redo (v1.9: each step is its own change)
        if !self.m.i1() || hits.i1_violated {
            self.viol.insert("I1");
        }
        // RT (U22a v1.6, restart transparency): a restart changes no revert's outcome or text
        if matches!(a, Act::Restart) {
            for (&c, rec) in &pre.m.records {
                for partial in [false, true] {
                    let run = |m: &Model| {
                        let mut m = m.clone();
                        let (o, _) = m.revert(&rec.author, None, c, partial);
                        (o, m.texts)
                    };
                    if run(&pre.m) != run(&self.m) {
                        self.viol.insert("RT");
                    }
                }
            }
            let held = pre
                .m
                .stacks
                .values()
                .any(|s| !s.undo.is_empty() || !s.redo.is_empty());
            if held || !pre.m.records.is_empty() {
                hit.push("restart");
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
        // the case classes (E-HIST-13) this transition hit
        let applied = matches!(out, Outcome::Applied { .. });
        for (on, class) in [
            (hits.eof_append_undo, "eof_append"),
            (hits.edge_insert, "edge_insert"),
            (hits.taint, "taint"),
            (
                hits.multi_hunk_partial
                    || hits.stays_on_top
                    || matches!(out, Outcome::Applied { skipped, .. } if !skipped.is_empty()),
                "partial",
            ),
            (hits.redo_after_partial && applied, "redo_after_partial"),
            (hits.rediff_differs, "exact_vs_rediff"),
            (hits.revert_extra_conflicts > 0, "revert_extra_conflict"),
            (hits.touching_regions, "touching_regions"),
            (hits.concurrent_restore, "concurrent_restore"),
            // its three parts (v0.9.2; the trace test's v1.8 split): (a) O4 took a
            // restoration once, by root; (b) O4a fired (the root is already live: a
            // conflict); (c) a window character was the base's by root only
            (hits.o4_restore, "concurrent_restore_o4"),
            (hits.o4a, "concurrent_restore_o4a"),
            (hits.base_by_root, "concurrent_restore_base_by_root"),
        ] {
            if on {
                hit.push(class);
            }
        }
        // U4: an applied undo, redo or revert next to the import's text, which no stack holds
        if applied
            && hits.notes.iter().any(|n| {
                n.clean
                    && ((n.start > 0 && pre.authors.get(n.start - 1) == Some(&0))
                        || pre.authors.get(n.end) == Some(&0))
            })
        {
            hit.push("import_kept");
        }
        if matches!(a, Act::Purge { .. })
            && pre
                .m
                .stacks
                .values()
                .any(|s| !s.undo.is_empty() || !s.redo.is_empty())
        {
            hit.push("purge_drops");
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
            if all.iter().any(|s| !s.reports.is_empty()) {
                hit.push("identical_reports");
            }
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
        // U12 (v1.7, a diagnostic): outside taint, the window merge of the exact sets equals
        // the whole-text merge of the same sets (offset into the doc); the old re-diff form
        // is only informative (F4)
        if b.u12 {
            for n in hits.notes.iter().filter(|n| !n.tainted && !n.erased) {
                let (l, r) = (&n.live[..n.start], &n.live[n.end..]);
                let base = format!("{l}{}{r}", n.after);
                let shift = |es: &[Edit]| -> Vec<Edit> {
                    es.iter()
                        .map(|e| Edit {
                            start: e.start + l.len(),
                            end: e.end + l.len(),
                            insert: e.insert.clone(),
                        })
                        .collect()
                };
                let (op, tp) = (&n.ours_roots, &n.theirs_roots);
                let same = merge::Same::Root;
                let (w, _) = merge::merge_edits(
                    &base,
                    &shift(&n.ours),
                    op,
                    &shift(&n.theirs),
                    tp,
                    n.at_end,
                    same,
                );
                let (m, _) =
                    merge::merge_edits(&n.after, &n.ours, op, &n.theirs, tp, n.at_end, same);
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
        self.hit = hit;
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
        let mut l = Labels::default();
        let doc = self
            .text()
            .chars()
            .zip(self.authors.iter().map(|a| map(*a)))
            .zip(&self.m.elems[0])
            .map(|((c, a), x)| (c, a, l.of(&x.prov)))
            .collect();
        let mut stacks = vec![VStacks::default(); n];
        for (name, st) in &self.m.stacks {
            let k = author_of(name) as usize - 1;
            let mut batches = |v: &[crate::model::Batch]| -> Vec<Vec<VRegion>> {
                v.iter()
                    .map(|b| b.regions.iter().map(|r| vregion(r, &mut l)).collect())
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
        let (mut authors, mut lost) = (Vec::new(), Vec::new());
        if self.track_authors {
            authors = vec![false; n];
            lost = vec![false; n];
            for k in 0..n {
                authors[perm[k]] = self.m.authors.contains(&pid(k));
                lost[perm[k]] = self.m.authors_before_restart.contains(&pid(k));
            }
        }
        let records = self
            .m
            .records
            .iter()
            .map(|(id, r)| {
                (
                    *id,
                    VRecord {
                        author: map(author_of(&r.author)),
                        regions: r.regions.iter().map(|g| vregion(g, &mut l)).collect(),
                        edges: r
                            .edges
                            .iter()
                            .map(|e| (e.start, e.end, e.start_taint, e.end_taint))
                            .collect(),
                    },
                )
            })
            .collect();
        let mut v = View {
            doc,
            stacks,
            records,
            head: if self.reduced { 0 } else { self.m.head },
            events,
            undos: self.undos,
            reverts: self.reverts,
            restarts: self.restarts,
            authors,
            lost,
            viol: self.viol.iter().map(|v| v.to_string()).collect(),
        };
        v.relabel();
        v
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
    /// Every root label, in the walk that numbers them (v1.8): the doc left to right; each
    /// pid's undo then redo batches, oldest first, each region's `before` roots then `after`
    /// roots; the records by id, the same per region.
    fn walk(&mut self, f: &mut impl FnMut(&mut usize)) {
        for (_, _, r) in &mut self.doc {
            f(r);
        }
        let region = |g: &mut VRegion, f: &mut dyn FnMut(&mut usize)| {
            for r in g.before_roots.iter_mut().chain(g.after_roots.iter_mut()) {
                f(r);
            }
        };
        for st in &mut self.stacks {
            for g in st.undo.iter_mut().chain(st.redo.iter_mut()).flatten() {
                region(g, f);
            }
        }
        for (_, rec) in &mut self.records {
            for g in &mut rec.regions {
                region(g, f);
            }
        }
    }

    /// Number the roots by first appearance in [`View::walk`]: states equal up to renaming
    /// roots get one view (`undo.qnt`'s `relabel`).
    pub fn relabel(&mut self) {
        let mut m: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        self.walk(&mut |r| {
            let n = m.len();
            *r = *m.entry(*r).or_insert(n);
        });
    }

    /// One line of canonical JSON (`tools/undo/statediff.py` writes TLC's states the same
    /// way, to compare the two state sets exactly).
    pub fn json(&self) -> String {
        let q = |s: &str| format!("\"{s}\"");
        let b = |x: bool| if x { "true" } else { "false" };
        let edits = |es: &[(usize, usize, String)]| {
            let v: Vec<String> = es
                .iter()
                .map(|(s, e, i)| format!("[{s},{e},{}]", q(i)))
                .collect();
            format!("[{}]", v.join(","))
        };
        let ints = |v: &[usize]| {
            format!(
                "[{}]",
                v.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
            )
        };
        let region = |r: &VRegion| {
            format!(
                "[{},{},{},{},{},{},{},{},{},{}]",
                r.start,
                r.end,
                q(&r.before),
                q(&r.after),
                edits(&r.ours),
                ints(&r.before_roots),
                ints(&r.after_roots),
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
            .map(|(c, a, r)| format!("[\"{c}\",{a},{r}]"))
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
        let flags = |v: &[bool]| v.iter().map(|x| b(*x)).collect::<Vec<_>>().join(",");
        format!(
            "[[{}],[{}],[{}],{},[{}],{},{},{},[{}],[{}],[{}]]",
            doc.join(","),
            stacks.join(","),
            records.join(","),
            self.head,
            ev.join(","),
            self.undos,
            self.reverts,
            self.restarts,
            flags(&self.authors),
            flags(&self.lost),
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
    /// The model panicked ([`PANIC`] is then among the violations): its message. The search
    /// stopped there.
    pub panic: Option<String>,
}

impl Counts {
    pub fn distinct(&self) -> u64 {
        self.per_depth.iter().sum()
    }
}

/// Every rule the transition checks can record.
pub const RULES: &[&str] = &[
    "own_only", "I1", "U9", "U13", "U15", "U17", "U19", "U22a", "RT", "O5", "U12",
];

/// The rules a run stops at by default: every rule but the diagnostic `U12` (`--u12` only).
/// `own_only` is enforced since v1.7 (F3 fixed: exact hunk sets), by root since v1.8; `I1`
/// since v1.8.
/// The model panicked while stepping (one of the oracle's own assertions, e.g. `window.rs`'s
/// shared-edge check): reported like a violated rule, `panic@<depth>` with its message, and
/// a stop rule in every mode (the run ends).
pub const PANIC: &str = "panic";

/// A caught panic's message.
pub fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

pub const STOP: &[&str] = &[
    "own_only", "I1", "U9", "U13", "U15", "U17", "U19", "U22a", "RT", "O5",
];

/// The case classes the walks count (E-HIST-13, as far as the bounded world reaches; the
/// trace test covers leave and rejoin, multi-byte text, `write --base` and the window).
pub const CLASSES: &[&str] = &[
    "eof_append",
    "edge_insert",
    "taint",
    "partial",
    "redo_after_partial",
    "identical_reports",
    "import_kept",
    "purge_drops",
    "restart",
    "exact_vs_rediff",
    "touching_regions",
    "revert_extra_conflict",
    "concurrent_restore",
    // its parts (a transition can hit more than one): O4 took a restoration once (by
    // root); O4a fired; the base was counted by root only
    "concurrent_restore_o4",
    "concurrent_restore_o4a",
    "concurrent_restore_base_by_root",
];

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
                let t = match s.try_step(b, &a) {
                    Ok(Some((t, _))) => t,
                    Ok(None) => {
                        counts.cut += 1;
                        continue;
                    }
                    Err(msg) => {
                        let mut p = path.clone();
                        p.push(a);
                        counts.violations.insert(PANIC, (depth, p));
                        counts.panic = Some(msg);
                        return counts;
                    }
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
