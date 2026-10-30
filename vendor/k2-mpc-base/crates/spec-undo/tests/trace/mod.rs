//! The trace generator and runner (spec §8: Generator, Run, Volume).
//!
//! A trace is a seed. Its steps are drawn one at a time from the seed's RNG and the
//! oracle's state (so edits land on region edges, deletes cross others' regions, reverts
//! name real changes), then replayed through (a) `spec_undo::model` and (b) the kit's
//! sessions on Yrs. After **every** step the runner compares every root's text, the
//! outcome, the reported conflict regions as an exact set (U11), and every pid's stack
//! depths. The first difference stops the trace and is reported, never adjusted.
//!
//! Sequential runs only: the concurrency class and the positions property test are later
//! (`docs/CONFORMANCE.md`).

pub mod kit;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use k2_mpc_base::session::{
    ConflictKind, ConflictRegion, SessionError, UndoMode, UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::VersionId;
use k2_mpc_base::text::diff::Hunk;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use spec_undo::model::{
    Bounds, ChangeId, Edit, Kind, Mode, Model, Notice, Outcome, Report, Request,
};

use kit::{Kit, KitOut, Knobs};

/// The case classes this runner covers (spec §8 Volume, sequential), plus the
/// rules every class leans on. Native-only and concurrency classes are later
/// (`docs/CONFORMANCE.md`).
pub const CLASSES: &[&str] = &[
    "append_at_end",
    "insert_inside_another_range",
    "multi_hunk_partial",
    "stays_on_top",
    "redo_after_partial",
    "revert_after_restart",
    "undo_across_purge",
    "multi_connection_pid",
    "empty_base_double_write",
    "last_line_delete_after_append",
    "whole_doc_delete_then_append",
    "touching_regions_one_step",
    "revert_beyond_compaction",
    "revert_outside_window",
    "taint",
    "revert_over_erased",
    "undo_for_person",
    "steps_n",
    "undo_drop",
    "history_lost_after_restart",
    "multi_root_batch",
    "repeated_text_identity",
];

/// Low entropy on purpose (repeated text is where a wrong position hides), with multi-byte
/// characters and newlines.
const ALPHA: &[char] = &['a', 'a', 'b', '\n', 'é', '日', ' '];
/// The repeated-text class (spec §8 Positions, generated on purpose): `{a, b, \n}`, runs
/// like `aaaa` and identical lines, where only element identity (U10) says which
/// characters an undo changes.
const REPEATED: &[char] = &['a', 'a', 'a', 'b', '\n'];
/// Secrets use letters nothing else types, so a purge finds only them.
const SECRET_ALPHA: &[u8] = b"QWERTYUPSDFGHJKLZXCVNM";

/// A first difference between the oracle and the kit.
#[derive(Debug)]
pub struct Mismatch {
    pub seed: u64,
    pub step: usize,
    pub what: String,
    /// Which finding it looks like (a label for the report; it decides nothing).
    pub label: &'static str,
    /// The trace up to and including the step.
    pub log: Vec<String>,
}

/// What one trace saw.
#[derive(Debug, Default)]
pub struct TraceResult {
    pub steps: usize,
    pub classes: BTreeMap<&'static str, u64>,
    pub mismatch: Option<Mismatch>,
    /// The first step whose applied hunks differ (same text): a diagnostic.
    pub hunk_diff: Option<String>,
    /// U22a's cost: revert regions, and those conflicting where undo would apply; overall
    /// and in reverts with touching regions.
    pub revert_regions: u64,
    pub revert_extra: u64,
    pub touching_revert_regions: u64,
    pub touching_revert_extra: u64,
    /// Reverts checked against U22a's property.
    pub reverts_checked: u64,
}

#[derive(Debug, Clone)]
pub enum Step {
    Edit {
        p: usize,
        s: usize,
        edits: Vec<(usize, Vec<Edit>)>,
    },
    Replace {
        p: usize,
        s: usize,
        root: usize,
        new: String,
    },
    Read {
        p: usize,
        s: usize,
    },
    WriteBase {
        p: usize,
        s: usize,
        root: usize,
        read: usize,
        ours: String,
    },
    Name {
        p: usize,
        s: usize,
    },
    Restore {
        p: usize,
        s: usize,
        v: usize,
    },
    Undo {
        p: usize,
        s: usize,
        steps: usize,
        mode: Mode,
        for_person: Option<String>,
        redo: bool,
    },
    Revert {
        p: usize,
        s: usize,
        change: ChangeId,
        partial: bool,
    },
    Purge {
        p: usize,
        s: usize,
        secret: String,
    },
    Leave {
        p: usize,
        s: usize,
    },
    Restart,
    Tick,
}

/// A directed scenario's next move, resolved against the state when it runs.
#[derive(Debug, Clone)]
enum Plan {
    DeleteLastLine {
        p: usize,
        root: usize,
    },
    DeleteAll {
        p: usize,
        root: usize,
    },
    Append {
        p: usize,
        root: usize,
        text: String,
    },
    Undo {
        p: usize,
        mode: Mode,
    },
    TwoSpots {
        p: usize,
        root: usize,
    },
    DeleteGap {
        p: usize,
        of: usize,
        root: usize,
    },
    InsertAtSharedEdge {
        p: usize,
        of: usize,
        root: usize,
    },
    Read {
        p: usize,
    },
    WriteAppend {
        p: usize,
        root: usize,
        text: String,
    },
    TypeSecret {
        p: usize,
        root: usize,
        split: bool,
    },
    Insert {
        p: usize,
        s: usize,
        root: usize,
        at: usize,
        text: String,
    },
    Purge {
        p: usize,
    },
    Revert {
        p: usize,
        oldest: bool,
    },
    Restart,
    Tick,
}

struct PidSpec {
    name: String,
    /// Per connection slot: the person it acts for (an assistant's connections).
    obo: Vec<Option<String>>,
}

struct ReadBase {
    p: usize,
    version: u64,
    texts: Vec<String>,
}

pub struct Runner {
    seed: u64,
    rng: SmallRng,
    model: Model,
    kit: Kit,
    pids: Vec<PidSpec>,
    present: Vec<Vec<bool>>,
    reads: Vec<ReadBase>,
    versions: Vec<(VersionId, Vec<String>)>,
    named: usize,
    /// Secrets typed and not yet purged.
    secrets: Vec<String>,
    queue: Vec<Plan>,
    first_change: ChangeId,
    restart_head: ChangeId,
    compacted_head: ChangeId,
    /// pid → slot of its latest batch.
    last_slot: HashMap<usize, usize>,
    log: Vec<String>,
    verbose: bool,
    /// A repeated-text trace ([`REPEATED`]).
    repeated: bool,
    /// The mismatching step was a revert whose kit report is the oracle's plus taints.
    revert_taints_more: bool,
    res: TraceResult,
}

/// The knobs every trace runs with: small, so the bounds, the window and compaction are
/// reached in a short trace.
pub const KNOBS: Knobs = Knobs {
    undo_depth: 8,
    keep_changes: 24,
    compact_entries: 16,
};

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Run trace `seed` to its end or its first mismatch.
pub fn run(seed: u64) -> TraceResult {
    run_with(seed, false)
}

/// [`run`], logging every root's text after each step (`verbose`).
pub fn run_with(seed: u64, verbose: bool) -> TraceResult {
    let mut rng = SmallRng::seed_from_u64(mix(seed));
    let n_pids = rng.gen_range(2..=5);
    let steps = rng.gen_range(20..=200);
    let mut pids = Vec::new();
    for i in 0..n_pids {
        let slots = rng.gen_range(1..=3);
        let assistant = i == 0 && rng.gen_bool(0.6);
        let obo = (0..slots)
            .map(|k| assistant.then(|| ["person-x", "person-y"][k % 2].to_owned()))
            .collect();
        pids.push(PidSpec {
            name: format!("p{i}"),
            obo,
        });
    }
    let repeated = rng.gen_bool(0.3);
    let alpha = if repeated { REPEATED } else { ALPHA };
    let seeds = if repeated {
        [repeated_text(&mut rng), repeated_text(&mut rng)]
    } else {
        [
            rand_text(&mut rng, alpha, 0, 30),
            rand_text(&mut rng, alpha, 0, 12),
        ]
    };
    let kit = Kit::open([&seeds[0], &seeds[1]], &KNOBS);
    let head = kit.version();
    let bounds = Bounds {
        depth: KNOBS.undo_depth,
        keep_changes: KNOBS.keep_changes,
        ..Bounds::default()
    };
    let model = Model::new(kit.texts(), head, bounds);
    assert_eq!(model.texts, seeds, "the seed texts");
    let present = pids.iter().map(|p| vec![false; p.obo.len()]).collect();
    let mut r = Runner {
        seed,
        rng,
        model,
        kit,
        pids,
        present,
        reads: Vec::new(),
        versions: Vec::new(),
        named: 0,
        secrets: Vec::new(),
        queue: Vec::new(),
        first_change: head + 1,
        restart_head: 0,
        compacted_head: 0,
        last_slot: HashMap::new(),
        log: Vec::new(),
        verbose,
        repeated,
        revert_taints_more: false,
        res: TraceResult::default(),
    };
    for i in 0..steps {
        r.res.steps = i + 1;
        if let Err(what) = r.step(i) {
            let label = if r.revert_taints_more {
                LABEL_REVERT_TAINT
            } else if r.res.hunk_diff.is_some() {
                LABEL_HUNKS
            } else {
                "unclassified"
            };
            r.res.mismatch = Some(Mismatch {
                seed,
                step: i,
                what,
                label,
                log: std::mem::take(&mut r.log),
            });
            break;
        }
    }
    r.res
}

/// Run a scripted trace: `pids` participants (one connection each, acting for nobody)
/// over `seeds`. Returns the first mismatch's description, if any.
pub fn script(seeds: [&str; 2], pids: usize, steps: Vec<Step>) -> Option<String> {
    let kit = Kit::open(seeds, &KNOBS);
    let head = kit.version();
    let bounds = Bounds {
        depth: KNOBS.undo_depth,
        keep_changes: KNOBS.keep_changes,
        ..Bounds::default()
    };
    let model = Model::new(kit.texts(), head, bounds);
    let pids: Vec<PidSpec> = (0..pids)
        .map(|i| PidSpec {
            name: format!("p{i}"),
            obo: vec![None],
        })
        .collect();
    let present = pids.iter().map(|_| vec![false]).collect();
    let mut r = Runner {
        seed: 0,
        rng: SmallRng::seed_from_u64(0),
        model,
        kit,
        pids,
        present,
        reads: Vec::new(),
        versions: Vec::new(),
        named: 0,
        secrets: Vec::new(),
        queue: Vec::new(),
        first_change: head + 1,
        restart_head: 0,
        compacted_head: 0,
        last_slot: HashMap::new(),
        log: Vec::new(),
        verbose: true,
        repeated: false,
        revert_taints_more: false,
        res: TraceResult::default(),
    };
    for (i, st) in steps.into_iter().enumerate() {
        r.kit.advance();
        if let Err(what) = r.exec(i, st) {
            return Some(format!("{}\n{what}", r.log.join("\n")));
        }
    }
    None
}

/// An edit of root `root` by `p` (connection 0).
pub fn edit(p: usize, root: usize, hunks: &[(usize, usize, &str)]) -> Step {
    let es = hunks
        .iter()
        .map(|&(start, end, insert)| Edit {
            start,
            end,
            insert: insert.into(),
        })
        .collect();
    Step::Edit {
        p,
        s: 0,
        edits: vec![(root, es)],
    }
}

/// `undo` (or `redo`) by `p`, one step, all or nothing.
pub fn undo(p: usize, redo: bool) -> Step {
    Step::Undo {
        p,
        s: 0,
        steps: 1,
        mode: Mode::All,
        for_person: None,
        redo,
    }
}

/// `revert <change>` by `p`, all or nothing.
pub fn revert(p: usize, change: ChangeId) -> Step {
    Step::Revert {
        p,
        s: 0,
        change,
        partial: false,
    }
}

/// Identical lines and runs: `aaaa\naaaa\nab\naaaa\n`.
fn repeated_text(rng: &mut SmallRng) -> String {
    let lines = ["a\n", "aa\n", "aaaa\n", "ab\n", "b\n"];
    let line = lines[rng.gen_range(0..lines.len())];
    (0..rng.gen_range(1..6))
        .map(|_| {
            if rng.gen_bool(0.7) {
                line
            } else {
                lines[rng.gen_range(0..lines.len())]
            }
        })
        .collect()
}

fn rand_text(rng: &mut SmallRng, alpha: &[char], min: usize, max: usize) -> String {
    let n = rng.gen_range(min..=max);
    let mut s: String = (0..n)
        .map(|_| alpha[rng.gen_range(0..alpha.len())])
        .collect();
    if !s.is_empty() && rng.gen_bool(0.7) && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Char boundaries of `t`, end included.
fn bounds_of(t: &str) -> Vec<usize> {
    t.char_indices().map(|(i, _)| i).chain([t.len()]).collect()
}

fn to_hunks(es: &[Edit]) -> Vec<Hunk> {
    es.iter().cloned().map(Hunk::from).collect()
}

fn kind(k: ConflictKind) -> Kind {
    match k {
        ConflictKind::Overlap => Kind::Overlap,
        ConflictKind::Taint => Kind::Taint,
        ConflictKind::Erased => Kind::Erased,
        other => panic!("an unknown conflict kind {other:?}"),
    }
}

fn reports(cs: &[ConflictRegion]) -> Vec<Report> {
    let mut v: Vec<Report> = cs
        .iter()
        .map(|c| Report {
            root: kit::ROOTS.iter().position(|r| *r == c.root).expect("root"),
            start: c.range.start,
            end: c.range.end,
            lines: c.lines,
            kind: kind(c.kind),
        })
        .collect();
    v.sort();
    v
}

/// The kit's reply in the oracle's terms (`Err`: a refusal the oracle has no counterpart
/// for).
fn normalise(out: KitOut) -> Result<Outcome, String> {
    Ok(match out {
        KitOut::Edit(Ok(changed)) => Outcome::Changed(changed.then_some(0)),
        KitOut::Write(Ok(WriteOutcome::Applied(r)), before) => {
            Outcome::Changed((r.version != before).then_some(0))
        }
        KitOut::Write(Ok(WriteOutcome::Conflicts(_)), _) => Outcome::WriteConflict,
        KitOut::Write(Ok(WriteOutcome::BaseExpired), _) => return Err("base expired".into()),
        KitOut::Undo(Ok(u)) => match u {
            UndoOutcome::Applied {
                steps,
                skipped,
                blocked,
                ..
            } => Outcome::Applied {
                steps,
                skipped: reports(&skipped),
                blocked: blocked.as_deref().map(reports),
            },
            UndoOutcome::Conflicts { step, conflicts } => Outcome::Conflicts {
                step,
                regions: reports(&conflicts),
            },
            UndoOutcome::Blocked { conflicts } => Outcome::Blocked {
                regions: reports(&conflicts),
            },
            UndoOutcome::Dropped { .. } => Outcome::Dropped,
            UndoOutcome::Nothing => Outcome::Nothing(Notice::Plain),
            UndoOutcome::HistoryLost => Outcome::Nothing(Notice::HistoryLost),
            UndoOutcome::ClearedByPurge => Outcome::Nothing(Notice::ClearedByPurge),
            other => return Err(format!("an unknown outcome {other:?}")),
        },
        KitOut::Undo(Err(SessionError::RevertTooOld { .. })) => Outcome::TooOld,
        KitOut::Edit(Err(e)) | KitOut::Write(Err(e), _) | KitOut::Undo(Err(e)) => {
            return Err(format!("the kit refused: {e}"))
        }
    })
}

/// A mismatch after a step whose applied hunks differed (same text) earlier in the trace.
pub const LABEL_HUNKS: &str = "F1: an undo, redo or revert applied other hunks than the merge's";
/// A revert reporting the oracle's regions plus more taints.
pub const LABEL_REVERT_TAINT: &str = "F2: revert taints more than U22a";

/// Every region an outcome reports.
fn regions_of(o: &Outcome) -> Vec<Report> {
    match o {
        Outcome::Applied {
            skipped, blocked, ..
        } => skipped
            .iter()
            .chain(blocked.iter().flatten())
            .cloned()
            .collect(),
        Outcome::Conflicts { regions, .. } | Outcome::Blocked { regions } => regions.clone(),
        _ => Vec::new(),
    }
}

/// `kit`'s regions are `model`'s plus at least one more, every extra one a taint, and
/// none of `model`'s is missing (ranges and kinds as a multiset; a kind may turn into a
/// taint).
fn taints_more(model: &Outcome, kit: &Outcome) -> bool {
    let mut k = regions_of(kit);
    for m in regions_of(model) {
        let at = k.iter().position(|x| *x == m).or_else(|| {
            k.iter().position(|x| {
                (x.root, x.start, x.end) == (m.root, m.start, m.end) && x.kind == Kind::Taint
            })
        });
        match at {
            Some(i) => {
                k.remove(i);
            }
            None => return false,
        }
    }
    !k.is_empty() && k.iter().all(|x| x.kind == Kind::Taint)
}

/// Outcomes compared at the spec's grain: change ids are the runner's (checked through the
/// version); everything else exactly, notices included (U18, U19).
fn same(model: &Outcome, kit: &Outcome) -> bool {
    match (model, kit) {
        (Outcome::Changed(a), Outcome::Changed(b)) => a.is_some() == b.is_some(),
        (a, b) => a == b,
    }
}

impl Runner {
    fn hit(&mut self, class: &'static str) {
        debug_assert!(CLASSES.contains(&class), "{class}");
        *self.res.classes.entry(class).or_default() += 1;
    }

    fn pick_pid(&mut self) -> usize {
        self.rng.gen_range(0..self.pids.len())
    }

    /// A connection slot of `p`, present or (re)joined now.
    fn slot(&mut self, p: usize) -> usize {
        let present: Vec<usize> = (0..self.present[p].len())
            .filter(|&s| self.present[p][s])
            .collect();
        if !present.is_empty() && self.rng.gen_bool(0.85) {
            return present[self.rng.gen_range(0..present.len())];
        }
        self.rng.gen_range(0..self.present[p].len())
    }

    fn obo(&self, p: usize, s: usize) -> Option<&str> {
        self.pids[p].obo[s].as_deref()
    }

    fn pos(&mut self, t: &str) -> usize {
        let b = bounds_of(t);
        b[self.rng.gen_range(0..b.len())]
    }

    fn word(&mut self) -> String {
        if self.repeated {
            // runs, and copies of the doc's own lines
            let lines: Vec<String> = self.model.texts[0]
                .split_inclusive('\n')
                .map(str::to_owned)
                .collect();
            return match self.rng.gen_range(0..4) {
                0 if !lines.is_empty() => lines[self.rng.gen_range(0..lines.len())].clone(),
                1 => "a\n".into(),
                2 => "a".repeat(self.rng.gen_range(1..=4)),
                _ => (0..self.rng.gen_range(1..=3))
                    .map(|_| REPEATED[self.rng.gen_range(0..REPEATED.len())])
                    .collect(),
            };
        }
        let n = self.rng.gen_range(1..=4);
        (0..n)
            .map(|_| ALPHA[self.rng.gen_range(0..ALPHA.len())])
            .collect()
    }

    /// Up to `n` non-overlapping random hunks on `t` (sometimes touching, so the
    /// canonical form joins them).
    fn random_hunks(&mut self, t: &str, n: usize) -> Vec<Edit> {
        let b = bounds_of(t);
        let mut out: Vec<Edit> = Vec::new();
        let mut cuts: Vec<usize> = (0..n).map(|_| self.rng.gen_range(0..b.len())).collect();
        cuts.sort_unstable();
        cuts.dedup();
        for ci in cuts {
            let start_i = match out.last() {
                Some(last) => {
                    let li = b.iter().position(|&x| x == last.end).expect("boundary");
                    if ci < li {
                        continue;
                    }
                    ci
                }
                None => ci,
            };
            let del = self.rng.gen_range(0..=3).min(b.len() - 1 - start_i);
            let mut insert = if self.rng.gen_bool(0.6) || del == 0 {
                self.word()
            } else {
                String::new()
            };
            let (start, end) = (b[start_i], b[start_i + del]);
            if let Some(last) = out.last() {
                // two insertions at one point are refused by the kit's validation
                if start == last.end && start == end && last.start == last.end {
                    continue;
                }
            }
            if start == end && insert.is_empty() {
                insert = self.word();
            }
            out.push(Edit { start, end, insert });
        }
        out
    }

    /// Regions of other pids' batches (and records) on `root`: (start, end).
    fn foreign_regions(&self, p: usize, root: usize) -> Vec<(usize, usize)> {
        let me = &self.pids[p].name;
        let mut v: Vec<(usize, usize)> = self
            .model
            .stacks
            .iter()
            .filter(|(pid, _)| *pid != me)
            .flat_map(|(_, st)| st.undo.iter().chain(&st.redo))
            .flat_map(|b| &b.regions)
            .filter(|r| r.root == root)
            .map(|r| (r.start, r.end))
            .collect();
        if v.is_empty() {
            v = self
                .model
                .records
                .values()
                .flat_map(|rec| &rec.regions)
                .filter(|r| r.root == root)
                .map(|r| (r.start, r.end))
                .collect();
        }
        v
    }

    fn draw(&mut self) -> Step {
        if !self.queue.is_empty() {
            let plan = self.queue.remove(0);
            if let Some(s) = self.resolve(plan) {
                return s;
            }
        }
        if self.rng.gen_bool(0.05) {
            self.scenario();
            if !self.queue.is_empty() {
                let plan = self.queue.remove(0);
                if let Some(s) = self.resolve(plan) {
                    return s;
                }
            }
        }
        let p = self.pick_pid();
        let s = self.slot(p);
        let root = usize::from(self.rng.gen_bool(0.3));
        let t = self.model.texts[root].clone();
        match self.rng.gen_range(0..100) {
            0..=17 => {
                let n = self.rng.gen_range(1..=3);
                let mut edits = vec![(root, self.random_hunks(&t, n))];
                if self.rng.gen_bool(0.2) {
                    let o = 1 - root;
                    let t2 = self.model.texts[o].clone();
                    edits.push((o, self.random_hunks(&t2, 1)));
                }
                Step::Edit { p, s, edits }
            }
            18..=23 => {
                let text = format!("{}\n", self.word().replace('\n', ""));
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: t.len(),
                            end: t.len(),
                            insert: text,
                        }],
                    )],
                }
            }
            24..=31 => {
                // an insertion exactly at (or next to) an edge of someone else's region
                let regions = self.foreign_regions(p, root);
                if regions.is_empty() {
                    return self.draw();
                }
                let (a, b) = regions[self.rng.gen_range(0..regions.len())];
                let edge = if self.rng.gen_bool(0.5) { a } else { b };
                let at = match self.rng.gen_range(0..4) {
                    0 => edge.saturating_sub(1),
                    1 => edge + 1,
                    _ => edge,
                }
                .min(t.len());
                let at = *bounds_of(&t).iter().find(|&&x| x >= at).expect("end");
                let insert = self.word();
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: at,
                            end: at,
                            insert,
                        }],
                    )],
                }
            }
            32..=36 => {
                // a deletion across an edge of someone else's region
                let regions = self.foreign_regions(p, root);
                if regions.is_empty() || t.is_empty() {
                    return self.draw();
                }
                let (a, b) = regions[self.rng.gen_range(0..regions.len())];
                let edge = if self.rng.gen_bool(0.5) { a } else { b };
                let bs = bounds_of(&t);
                let i = bs
                    .iter()
                    .position(|&x| x >= edge.min(t.len()))
                    .expect("end");
                let lo = bs[i.saturating_sub(self.rng.gen_range(1..=2))];
                let hi = bs[(i + self.rng.gen_range(1..=2)).min(bs.len() - 1)];
                if lo == hi {
                    return self.draw();
                }
                let insert = if self.rng.gen_bool(0.3) {
                    self.word()
                } else {
                    String::new()
                };
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: lo,
                            end: hi,
                            insert,
                        }],
                    )],
                }
            }
            37..=41 => {
                // sed -i over distant lines (the pipeline makes it multi-hunk)
                let mut lines: Vec<String> = t.split_inclusive('\n').map(str::to_owned).collect();
                if lines.is_empty() {
                    return self.draw();
                }
                for _ in 0..self.rng.gen_range(1..=2) {
                    let i = self.rng.gen_range(0..lines.len());
                    let w = self.word();
                    lines[i] = match self.rng.gen_range(0..3) {
                        0 => String::new(),
                        1 => format!("{w}{}", lines[i]),
                        _ => lines[i].replacen('a', &w, 1),
                    };
                }
                Step::Replace {
                    p,
                    s,
                    root,
                    new: lines.concat(),
                }
            }
            42..=45 => Step::Read { p, s },
            46..=50 => {
                // the kit caches the last 64 versions read (the runner reads every step)
                let head = self.model.head;
                let mine: Vec<usize> = (0..self.reads.len())
                    .filter(|&i| self.reads[i].p == p && self.reads[i].version + 48 > head)
                    .collect();
                let Some(&read) = mine.last() else {
                    return Step::Read { p, s };
                };
                let base = self.reads[read].texts[root].clone();
                let ours = if self.rng.gen_bool(0.5) {
                    format!("{base}{}\n", self.word().replace('\n', ""))
                } else {
                    let hs = self.random_hunks(&base, 1);
                    spec_undo::model::hunk::apply(&base, &hs)
                };
                Step::WriteBase {
                    p,
                    s,
                    root,
                    read,
                    ours,
                }
            }
            51..=63 => {
                let redo = false;
                self.undo_step(p, s, redo)
            }
            64..=70 => self.undo_step(p, s, true),
            71..=76 => self.revert_step(p, s, false),
            77..=78 => Step::Name { p, s },
            79..=80 if !self.versions.is_empty() => {
                let v = self.rng.gen_range(0..self.versions.len());
                Step::Restore { p, s, v }
            }
            81..=82 => {
                self.queue.push(Plan::TypeSecret {
                    p,
                    root,
                    split: self.rng.gen_bool(0.4),
                });
                self.draw()
            }
            83 if !self.secrets.is_empty() => {
                let secret = self.secrets[0].clone();
                Step::Purge { p, s, secret }
            }
            84..=86 => {
                let present: Vec<usize> = (0..self.present[p].len())
                    .filter(|&k| self.present[p][k])
                    .collect();
                match present.first() {
                    Some(&k) => Step::Leave { p, s: k },
                    None => self.draw(),
                }
            }
            87 => Step::Restart,
            88..=91 => Step::Tick,
            _ => {
                let n = self.rng.gen_range(1..=2);
                Step::Edit {
                    p,
                    s,
                    edits: vec![(root, self.random_hunks(&t, n))],
                }
            }
        }
    }

    fn undo_step(&mut self, p: usize, s: usize, redo: bool) -> Step {
        let steps = if self.rng.gen_bool(0.2) {
            self.rng.gen_range(2..=3)
        } else {
            1
        };
        let mode = match self.rng.gen_range(0..20) {
            0 if !redo => Mode::Drop,
            1..=7 => Mode::Partial,
            _ => Mode::All,
        };
        let obo = self.obo(p, s).map(str::to_owned);
        let for_person = match obo {
            Some(o) if !redo && self.rng.gen_bool(0.3) => Some(o),
            _ => None,
        };
        Step::Undo {
            p,
            s,
            steps,
            mode,
            for_person,
            redo,
        }
    }

    fn revert_step(&mut self, p: usize, s: usize, oldest: bool) -> Step {
        let head = self.model.head;
        if head < self.first_change {
            return Step::Read { p, s };
        }
        let held: Vec<ChangeId> = self.model.records.keys().copied().collect();
        let change = if oldest {
            self.first_change
        } else if !held.is_empty() && self.rng.gen_bool(0.7) {
            held[self.rng.gen_range(0..held.len())]
        } else {
            self.rng.gen_range(self.first_change..=head)
        };
        let partial = self.rng.gen_bool(0.3);
        Step::Revert {
            p,
            s,
            change,
            partial,
        }
    }

    /// Queue one directed scenario (the spec's named cases).
    fn scenario(&mut self) {
        let n = self.pids.len();
        let a = self.pick_pid();
        let b = (a + 1 + self.rng.gen_range(0..n - 1)) % n;
        let root = usize::from(self.rng.gen_bool(0.3));
        let mode = if self.rng.gen_bool(0.5) {
            Mode::Partial
        } else {
            Mode::All
        };
        let line = format!("{}\n", self.word().replace('\n', ""));
        let q = match self.rng.gen_range(0..8) {
            // U12: undo of a last-line delete after a foreign append
            0 => vec![
                Plan::DeleteLastLine { p: a, root },
                Plan::Append {
                    p: b,
                    root,
                    text: line,
                },
                Plan::Undo { p: a, mode },
            ],
            // U10: whole-doc delete, then an append
            1 => vec![
                Plan::DeleteAll { p: a, root },
                Plan::Append {
                    p: b,
                    root,
                    text: line,
                },
                Plan::Undo { p: a, mode },
            ],
            // U8/U10: two regions of one step made to touch, then an insert at the shared edge
            2 => vec![
                Plan::TwoSpots { p: a, root },
                Plan::DeleteGap { p: b, of: a, root },
                Plan::InsertAtSharedEdge { p: b, of: a, root },
                Plan::Undo { p: a, mode },
            ],
            // two writes to an empty doc
            3 => vec![
                Plan::DeleteAll { p: a, root },
                Plan::Read { p: a },
                Plan::Read { p: b },
                Plan::WriteAppend {
                    p: a,
                    root,
                    text: line,
                },
                Plan::WriteAppend {
                    p: b,
                    root,
                    text: "z\n".into(),
                },
            ],
            // a secret, a purge, then undo
            4 => vec![
                Plan::TypeSecret {
                    p: a,
                    root,
                    split: self.rng.gen_bool(0.5),
                },
                Plan::Purge { p: b },
                Plan::Undo { p: a, mode },
                Plan::Revert {
                    p: b,
                    oldest: false,
                },
            ],
            // revert after a restart
            5 => vec![
                Plan::Restart,
                Plan::Undo { p: a, mode },
                Plan::Revert {
                    p: a,
                    oldest: false,
                },
            ],
            // the window moves, then revert the oldest change
            6 => vec![Plan::Tick, Plan::Revert { p: a, oldest: true }],
            _ => vec![
                Plan::Append {
                    p: a,
                    root,
                    text: line.clone(),
                },
                Plan::Append {
                    p: b,
                    root,
                    text: line,
                },
                Plan::Undo { p: a, mode },
            ],
        };
        self.queue.extend(q);
    }

    fn resolve(&mut self, plan: Plan) -> Option<Step> {
        let at_end = |t: &str, insert: String| {
            vec![Edit {
                start: t.len(),
                end: t.len(),
                insert,
            }]
        };
        Some(match plan {
            Plan::DeleteLastLine { p, root } => {
                let t = &self.model.texts[root];
                let body = t.strip_suffix('\n').unwrap_or(t);
                let start = body.rfind('\n')? + 1;
                let end = t.len();
                if start >= end {
                    return None;
                }
                let s = self.slot(p);
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start,
                            end,
                            insert: String::new(),
                        }],
                    )],
                }
            }
            Plan::DeleteAll { p, root } => {
                let s = self.slot(p);
                Step::Replace {
                    p,
                    s,
                    root,
                    new: String::new(),
                }
            }
            Plan::Append { p, root, text } => {
                let s = self.slot(p);
                let t = self.model.texts[root].clone();
                Step::Edit {
                    p,
                    s,
                    edits: vec![(root, at_end(&t, text))],
                }
            }
            Plan::Undo { p, mode } => {
                let s = self.slot(p);
                Step::Undo {
                    p,
                    s,
                    steps: 1,
                    mode,
                    for_person: None,
                    redo: false,
                }
            }
            Plan::TwoSpots { p, root } => {
                let t = self.model.texts[root].clone();
                let b = bounds_of(&t);
                if b.len() < 6 {
                    return None;
                }
                let i = self.rng.gen_range(0..b.len() - 5);
                let j = i + 2 + self.rng.gen_range(1..=2);
                let (w1, w2) = (self.word(), self.word());
                let s = self.slot(p);
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![
                            Edit {
                                start: b[i],
                                end: b[i + 1],
                                insert: w1,
                            },
                            Edit {
                                start: b[j],
                                end: b[j + 1],
                                insert: w2,
                            },
                        ],
                    )],
                }
            }
            Plan::DeleteGap { p, of, root } => {
                let name = self.pids[of].name.clone();
                let rs = &self.model.stacks.get(&name)?.undo.last()?.regions;
                let w = rs
                    .windows(2)
                    .find(|w| w[0].root == root && w[1].root == root && w[0].end < w[1].start)?;
                let (start, end) = (w[0].end, w[1].start);
                let s = self.slot(p);
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start,
                            end,
                            insert: String::new(),
                        }],
                    )],
                }
            }
            Plan::InsertAtSharedEdge { p, of, root } => {
                let name = self.pids[of].name.clone();
                let rs = &self.model.stacks.get(&name)?.undo.last()?.regions;
                let w = rs
                    .windows(2)
                    .find(|w| w[0].root == root && w[1].root == root && w[0].end == w[1].start)?;
                let at = w[0].end;
                let insert = self.word();
                let s = self.slot(p);
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: at,
                            end: at,
                            insert,
                        }],
                    )],
                }
            }
            Plan::Read { p } => {
                let s = self.slot(p);
                Step::Read { p, s }
            }
            Plan::WriteAppend { p, root, text } => {
                let read = (0..self.reads.len())
                    .rev()
                    .find(|&i| self.reads[i].p == p)?;
                let base = &self.reads[read].texts[root];
                let ours = format!("{base}{text}");
                let s = self.slot(p);
                Step::WriteBase {
                    p,
                    s,
                    root,
                    read,
                    ours,
                }
            }
            Plan::TypeSecret { p, root, split } => {
                let secret: String = (0..32)
                    .map(|_| SECRET_ALPHA[self.rng.gen_range(0..SECRET_ALPHA.len())] as char)
                    .collect();
                let t = self.model.texts[root].clone();
                let at = self.pos(&t);
                let s = self.slot(p);
                self.secrets.push(secret.clone());
                let typed = if split {
                    let cut = self.rng.gen_range(1..32);
                    // the rest is typed right after it, as a second batch
                    let rest = secret[cut..].to_owned();
                    self.queue.insert(
                        0,
                        Plan::Insert {
                            p,
                            s,
                            root,
                            at: at + cut,
                            text: rest,
                        },
                    );
                    secret[..cut].to_owned()
                } else {
                    secret
                };
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: at,
                            end: at,
                            insert: typed,
                        }],
                    )],
                }
            }
            Plan::Insert {
                p,
                s,
                root,
                at,
                text,
            } => {
                if at > self.model.texts[root].len() || !self.model.texts[root].is_char_boundary(at)
                {
                    return None;
                }
                Step::Edit {
                    p,
                    s,
                    edits: vec![(
                        root,
                        vec![Edit {
                            start: at,
                            end: at,
                            insert: text,
                        }],
                    )],
                }
            }
            Plan::Purge { p } => {
                let secret = self.secrets.first()?.clone();
                let s = self.slot(p);
                Step::Purge { p, s, secret }
            }
            Plan::Revert { p, oldest } => {
                let s = self.slot(p);
                self.revert_step(p, s, oldest)
            }
            Plan::Restart => Step::Restart,
            Plan::Tick => Step::Tick,
        })
    }
}

/// A change's hunks as its record holds them (as made, per root).
fn record_hunks(m: &Model, id: ChangeId) -> Option<Vec<(usize, Vec<Hunk>)>> {
    let rec = m.records.get(&id)?;
    Some(rec.hunks.iter().map(|(r, es)| (*r, to_hunks(es))).collect())
}

impl Runner {
    fn conn(&mut self, p: usize, s: usize) -> k2_mpc_base::session::ConnId {
        self.present[p][s] = true;
        let name = self.pids[p].name.clone();
        let obo = self.pids[p].obo[s].clone();
        self.kit.conn(&name, s, obo.as_deref())
    }

    /// One step: draw it, run it on both sides, compare.
    fn step(&mut self, i: usize) -> Result<(), String> {
        self.kit.advance();
        let step = self.draw();
        self.exec(i, step)
    }

    /// Run `step` on both sides and compare.
    fn exec(&mut self, i: usize, step: Step) -> Result<(), String> {
        self.log.push(format!("{i}: {step:?}"));
        let head_before = self.model.head;
        let log_before = self.kit.log_entries();
        let mut hits = spec_undo::model::Hits::default();
        let (mo, ko): (Outcome, Option<KitOut>) = match step.clone() {
            Step::Edit { p, s, edits } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let mo = self.model.edit(&name, obo.as_deref(), edits.clone());
                let hs: Vec<(usize, Vec<Hunk>)> =
                    edits.iter().map(|(r, es)| (*r, to_hunks(es))).collect();
                (mo, Some(self.kit.edit(c, &hs)))
            }
            Step::Replace { p, s, root, new } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let mo = self.model.replace(&name, obo.as_deref(), root, &new);
                (mo, Some(self.kit.replace(c, root, &new)))
            }
            Step::Read { p, s } => {
                let c = self.conn(p, s);
                let version = self.kit.read(c);
                self.reads.push(ReadBase {
                    p,
                    version,
                    texts: self.model.texts.clone(),
                });
                (Outcome::Changed(None), None)
            }
            Step::WriteBase {
                p,
                s,
                root,
                read,
                ours,
            } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let base = self.reads[read].texts[root].clone();
                let (mo, h) = self
                    .model
                    .write_base(&name, obo.as_deref(), root, &base, &ours);
                hits = h;
                (
                    mo,
                    Some(
                        self.kit
                            .write_base(c, root, self.reads[read].version, &ours),
                    ),
                )
            }
            Step::Name { p, s } => {
                let c = self.conn(p, s);
                self.named += 1;
                let v = self.kit.name_version(c, &format!("v{}", self.named));
                self.versions.push((v, self.model.texts.clone()));
                (Outcome::Changed(None), None)
            }
            Step::Restore { p, s, v } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let (vid, texts) = self.versions[v].clone();
                let mo = self.model.restore(&name, obo.as_deref(), &texts);
                (mo, Some(self.kit.restore(c, vid)))
            }
            Step::Undo {
                p,
                s,
                steps,
                mode,
                for_person,
                redo,
            } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let req = Request {
                    steps,
                    mode,
                    for_person: for_person.clone(),
                };
                let (mo, h) = if redo {
                    self.model.redo(&name, obo.as_deref(), &req)
                } else {
                    self.model.undo(&name, obo.as_deref(), &req)
                };
                hits = h;
                let km = match mode {
                    Mode::All => UndoMode::All,
                    Mode::Partial => UndoMode::Partial,
                    Mode::Drop => UndoMode::Drop,
                };
                (
                    mo,
                    Some(self.kit.undo(c, steps, km, for_person.as_deref(), redo)),
                )
            }
            Step::Revert {
                p,
                s,
                change,
                partial,
            } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let (mo, h) = self.model.revert(&name, obo.as_deref(), change, partial);
                hits = h;
                (mo, Some(self.kit.revert(c, change, partial)))
            }
            Step::Purge { p, s, secret } => {
                let c = self.conn(p, s);
                let (name, obo) = (self.pids[p].name.clone(), self.pids[p].obo[s].clone());
                let mo = self.model.purge_text(&name, obo.as_deref(), &secret);
                self.secrets.retain(|x| *x != secret);
                self.reads.clear();
                self.versions.clear();
                (mo, Some(self.kit.purge(c, &secret)))
            }
            Step::Leave { p, s } => {
                let name = self.pids[p].name.clone();
                self.kit.leave(&name, s);
                self.present[p][s] = false;
                (Outcome::Changed(None), None)
            }
            Step::Restart => {
                self.model.restart();
                self.kit.restart();
                for v in &mut self.present {
                    v.iter_mut().for_each(|x| *x = false);
                }
                self.reads.clear();
                self.restart_head = self.model.head;
                (Outcome::Changed(None), None)
            }
            Step::Tick => {
                self.model.tick();
                self.kit.tick();
                (Outcome::Changed(None), None)
            }
        };
        // ---- compare ----
        if let Some(ko) = ko {
            let ko = normalise(ko).map_err(|e| format!("{e}; the oracle: {mo:?}"))?;
            if !same(&mo, &ko) {
                self.revert_taints_more =
                    matches!(step, Step::Revert { .. }) && taints_more(&mo, &ko);
                return Err(format!("outcome:\n  oracle {mo:?}\n  kit    {ko:?}"));
            }
        }
        if matches!(step, Step::Revert { .. }) && mo != Outcome::TooOld {
            if !hits.revert_superset_ok {
                return Err(format!(
                    "U22a's property: revert's conflicts must contain undo's on the same \
                     state, with the same bounds and text ({mo:?})"
                ));
            }
            self.res.reverts_checked += 1;
            self.res.revert_regions += hits.revert_regions as u64;
            self.res.revert_extra += hits.revert_extra_conflicts as u64;
            if hits.touching_regions {
                self.res.touching_revert_regions += hits.revert_regions as u64;
                self.res.touching_revert_extra += hits.revert_extra_conflicts as u64;
            }
        }
        let kt = self.kit.texts();
        if kt != self.model.texts {
            return Err(format!(
                "texts:\n  oracle {:?}\n  kit    {kt:?}",
                self.model.texts
            ));
        }
        for p in &self.pids {
            let (m, k) = (self.model.depths(&p.name), self.kit.depths(&p.name));
            if m != k {
                return Err(format!(
                    "{} stack depths (undo, redo): oracle {m:?}, kit {k:?}",
                    p.name
                ));
            }
        }
        let kv = self.kit.version();
        if kv != self.model.head {
            return Err(format!(
                "change count: oracle head {}, kit version {kv}",
                self.model.head
            ));
        }
        // ---- diagnostics: the applied hunks ----
        let applied = self.kit.applied();
        for id in head_before + 1..=self.model.head {
            let mut kit_h: Vec<(usize, Vec<Hunk>)> = applied
                .iter()
                .filter(|(v, _, _)| *v == id)
                .map(|(_, r, h)| (*r, h.clone()))
                .collect();
            kit_h.sort_by_key(|x| x.0);
            let Some(model_h) = record_hunks(&self.model, id) else {
                continue;
            };
            if model_h != kit_h {
                let d = format!("applied hunks of {id} differ: oracle {model_h:?}, kit {kit_h:?}");
                self.log.push(format!("    {d}"));
                if self.res.hunk_diff.is_none() {
                    self.res.hunk_diff = Some(format!("seed {} step {i} {step:?}: {d}", self.seed));
                }
            }
        }
        if self.verbose {
            self.log.push(format!("    texts {:?}", self.model.texts));
        }
        if !matches!(step, Step::Restart) && self.kit.log_entries() < log_before {
            self.compacted_head = self.model.head;
        }
        // ---- classes ----
        let applied_ok = matches!(mo, Outcome::Applied { .. });
        let flags: [(bool, &'static str); 18] = [
            (
                hits.eof_append_undo || hits.eof_append_write,
                "append_at_end",
            ),
            (hits.insert_inside, "insert_inside_another_range"),
            (hits.multi_hunk_partial, "multi_hunk_partial"),
            (hits.stays_on_top, "stays_on_top"),
            (hits.redo_after_partial && applied_ok, "redo_after_partial"),
            (hits.cleared_by_purge, "undo_across_purge"),
            (hits.empty_base_double_write, "empty_base_double_write"),
            (hits.last_line_after_append, "last_line_delete_after_append"),
            (
                hits.whole_doc_delete_then_append,
                "whole_doc_delete_then_append",
            ),
            (hits.touching_regions, "touching_regions_one_step"),
            (hits.too_old, "revert_outside_window"),
            (hits.taint, "taint"),
            (
                hits.erased && matches!(step, Step::Revert { .. }),
                "revert_over_erased",
            ),
            (hits.undo_for, "undo_for_person"),
            (hits.steps_n, "steps_n"),
            (hits.dropped, "undo_drop"),
            (hits.history_lost, "history_lost_after_restart"),
            (
                hits.identity_case && self.repeated,
                "repeated_text_identity",
            ),
        ];
        for (on, c) in flags {
            if on {
                self.hit(c);
            }
        }
        match &step {
            Step::Revert { change, .. } if mo != Outcome::TooOld => {
                if *change <= self.restart_head {
                    self.hit("revert_after_restart");
                }
                if *change <= self.compacted_head {
                    self.hit("revert_beyond_compaction");
                }
            }
            Step::Undo { p, s, .. } if applied_ok => {
                if self.last_slot.get(p).is_some_and(|l| l != s) {
                    self.hit("multi_connection_pid");
                }
                let roots: BTreeSet<(u64, usize)> =
                    applied.iter().map(|(v, r, _)| (*v, *r)).collect();
                let versions: BTreeSet<u64> = roots.iter().map(|x| x.0).collect();
                if roots.len() > versions.len() {
                    self.hit("multi_root_batch");
                }
            }
            Step::Edit { p, s, .. } | Step::Replace { p, s, .. } | Step::WriteBase { p, s, .. }
                if self.model.head > head_before =>
            {
                self.last_slot.insert(*p, *s);
            }
            _ => {}
        }
        Ok(())
    }
}
