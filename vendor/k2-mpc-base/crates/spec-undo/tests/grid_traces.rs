//! The grid trace test (spec v1.11 §8, structured kinds): `spec_undo::grid` against the kit's
//! sessions on Yrs, after every step of seeded traces over a table. Each trace runs one case
//! class: a scripted start that sets the class's situation up, then a random tail of grid
//! verbs, undos, redos, reverts, outside saves and restarts. After every step it compares
//! the table by key (live order, deleted keys, cells, names, meta), the version, the outcome
//! (kind and the conflicting regions as an exact set), every participant's stack depths, and
//! the holders of every facet the step wrote (the kit's `blame_elem`: the resolved change and
//! its author, `on_behalf_of` and `restored_by`); at the end of the trace, every facet's holder
//! (`Facet::Insert` holders are skipped: `blame_elem` has no insert facet; see
//! `holders_differ`). Every comparison is counted, and each count and coverage key must reach
//! its floor in each class ([`FLOORS`]); the mismatches are reported before the floors.
//!
//! `GRID_TRACES` sets the traces per class (by default 256 / the classes + 1, as CI runs them:
//! 10 per class today; the node runs ≥ 10^4 per class), `GRID_CLASSES=a,b` a node shard's
//! classes (the floors then hold per class for those). Every class prints a run record
//! (`grid_traces_<class>`). `grid_mutants_each_fail` runs the model
//! as each mutant of §8 (xv)–(xxvi) and records CAUGHT or SURVIVED per mutant
//! (`grid_mutant_<name>`); over every class each must be caught (a shard only records).
//! `write_with_no_identity` (KC7, Yrs only) plants writes with no identity through the kit's
//! conformance hook `set_outside_without_identity_for_tests`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::{Doc, Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Facet, Key, KeyedRoot, KeyedState, MapOp};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, Session, SessionManager, Stay, UndoMode, UndoOutcome,
    UndoRequest, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{self, Field, Record, Shape, SortKey, Table};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use spec_undo::grid::{self as model, Grid, GridMutant, Outcome};

const LITE: Schema = Schema {
    name: "csv-lite",
    roots: table::ROOTS,
};
const LISTS: [&str; 2] = [table::ROWS, table::COLS];
const MAPS: [&str; 4] = [table::CELLS, table::NAMES, table::QUOTING, table::META];
const PIDS: [&str; 3] = ["p1", "p2", "p0"];
const VALUES: [&str; 5] = ["a", "b", "c", "x", ""];

/// One record per line, fields split by `,`: values never hold `,` or a line break here.
struct Lite;

fn records(s: &str) -> Vec<Record> {
    s.lines()
        .map(|l| l.split(',').map(Field::new).collect())
        .collect()
}

fn render(t: &Table) -> String {
    let mut out = String::new();
    for rec in t.render() {
        let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
        out.push_str(&f.join(","));
        out.push('\n');
    }
    out
}

impl Pack for Lite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let recs = records(std::str::from_utf8(b).map_err(|_| Declined {
            code: "not_utf8",
            message: "not UTF-8".into(),
        })?);
        let (_, edits) =
            Table::import(&recs, false, Shape::detect(&recs)).map_err(|e| Declined {
                code: "malformed",
                message: e.to_string(),
            })?;
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        Ok(render(&t).into_bytes())
    }
}

struct H {
    now: AtomicU64,
}
impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(who.pid == "p0")
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// The kit side: a real session on Yrs over a real store and file in a temporary directory.
struct Kit {
    dir: PathBuf,
    host: Arc<H>,
    mgr: Option<SessionManager>,
    s: Option<Arc<Session>>,
    conns: HashMap<&'static str, ConnId>,
}

impl Kit {
    fn new(seed: u64, body: &str) -> Kit {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-gridtrace-{}-{seed}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files/t.csv"), body).unwrap();
        let mut k = Kit {
            dir,
            host: Arc::new(H {
                now: AtomicU64::new(1),
            }),
            mgr: None,
            s: None,
            conns: HashMap::new(),
        };
        k.open();
        k
    }

    fn open(&mut self) {
        let store = Arc::new(DocStore::open(self.dir.join("docs.redb")).unwrap());
        store.unsynced_for_tests();
        let mut c = Config::default();
        c.edit_burst = 1_000_000;
        c.edit_rate_per_s = 1_000_000;
        c.write_quiet_ms = u64::MAX / 4;
        c.write_max_delay_ms = u64::MAX / 4;
        c.idle_version_ms = u64::MAX / 4;
        let mgr = SessionManager::new(store, self.host.clone(), c);
        let key = DocKey {
            space: "ws".into(),
            path: "t.csv".into(),
        };
        let files = self.dir.join("files");
        let s = mgr
            .open_file(Arc::new(Lite), &key, &files.join("t.csv"), &files)
            .unwrap();
        self.conns.clear();
        for p in PIDS {
            let c = s
                .join(
                    Participant::new(p, p, "#111111", ParticipantKind::Person, "cli"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            self.conns.insert(p, c);
        }
        let c = s
            .join(
                Participant::new("a0", "a0", "#111111", ParticipantKind::Agent, "cli")
                    .on_behalf_of("p1"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        self.conns.insert("a0", c);
        self.s = Some(s);
        self.mgr = Some(mgr);
    }

    fn s(&self) -> &Session {
        self.s.as_ref().unwrap()
    }

    fn restart(&mut self) {
        self.s = None;
        if let Some(m) = self.mgr.take() {
            m.release().unwrap();
        }
        self.open();
    }
}

impl Drop for Kit {
    fn drop(&mut self) {
        self.s = None;
        if let Some(m) = self.mgr.take() {
            let _ = m.release();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The model's state as a keyed state (for the table builders and the comparison).
fn keyed(g: &Grid) -> KeyedState {
    let mut roots = Vec::new();
    for (name, _) in table::ROOTS {
        if let Some(l) = g.lists.get(*name) {
            let live = l
                .live()
                .into_iter()
                .map(|k| {
                    let v = l.vals[&k].clone();
                    (k, v)
                })
                .collect();
            let hidden = l
                .dead
                .iter()
                .map(|k| (k.clone(), l.vals[k].clone()))
                .collect();
            roots.push((
                (*name).to_owned(),
                KeyedRoot::List {
                    live,
                    hidden,
                    places: l.places.clone(),
                },
            ));
        } else {
            roots.push(((*name).to_owned(), KeyedRoot::Map(g.maps[*name].clone())));
        }
    }
    KeyedState { roots }
}

/// What the comparison reads of a keyed state: live orders, deleted keys with values, maps.
fn shape(k: &KeyedState) -> BTreeMap<String, String> {
    k.roots
        .iter()
        .map(|(r, x)| {
            let s = match x {
                KeyedRoot::List { live, hidden, .. } => format!("{live:?} / {hidden:?}"),
                KeyedRoot::Map(m) => format!("{m:?}"),
                KeyedRoot::Text(t) => t.clone(),
                _ => String::new(),
            };
            (r.clone(), s)
        })
        .collect()
}

fn outcome_of(r: UndoOutcome) -> Outcome {
    let regions = |cs: Vec<k2_mpc_base::session::ConflictRegion>| {
        let mut v: Vec<(String, String, String)> = cs
            .into_iter()
            .map(|c| (c.root, c.key.unwrap_or_default(), c.why.unwrap_or_default()))
            .collect();
        v.sort();
        v
    };
    match r {
        UndoOutcome::Applied {
            steps,
            skipped,
            blocked,
            ..
        } => Outcome::Applied {
            steps,
            skipped: regions(skipped),
            blocked: blocked.is_some(),
        },
        UndoOutcome::Conflicts { step, conflicts } => Outcome::Conflicts {
            step,
            conflicts: regions(conflicts),
        },
        UndoOutcome::Blocked { conflicts } => Outcome::Blocked {
            conflicts: regions(conflicts),
        },
        UndoOutcome::Dropped { .. } => Outcome::Dropped,
        UndoOutcome::Nothing => Outcome::Nothing,
        UndoOutcome::HistoryLost => Outcome::HistoryLost,
        other => panic!("an undo outcome the grid trace doesn't expect: {other:?}"),
    }
}

/// One step of a trace.
#[derive(Debug, Clone)]
enum Step {
    /// A grid verb by a participant: `(pid, op)`.
    Op(&'static str, Verb),
    /// A verb by the agent `a0` on behalf of `p1`.
    OpFor(Verb),
    /// `undo --for p1` (or redo) by `a0`.
    UndoFor(usize, bool, bool),
    /// `write --base` by a participant: it reads, these live verbs land, then it writes the text
    /// its read made with these verbs; with `--partial` or not.
    WriteBase(&'static str, Vec<(&'static str, Verb)>, Vec<Verb>, bool),
    Undo(&'static str, usize, UndoMode),
    Redo(&'static str, usize, UndoMode),
    /// Revert the `n`th-newest change `pid` made.
    Revert(&'static str, usize, bool),
    DropDeleted,
    /// An outside save applying these verbs to the file.
    Outside(Vec<Verb>),
    /// The same, its writes recorded with no identity (a write merged in from elsewhere
    /// whose writer the engine doesn't report: KC7, a pinned Yrs deviation, spec S2).
    OutsideNoId(Vec<Verb>),
    Restart,
}

#[derive(Debug, Clone)]
enum Verb {
    Set(usize, usize, usize),
    Clear(usize, usize),
    /// A map delete of a present cell (an absent cell, as a ragged row's).
    Unset(usize, usize),
    InsRow(usize),
    MoveRow(usize, usize),
    DelRow(usize),
    ResRow(usize),
    InsCol(usize),
    DelCol(usize),
    MoveCol(usize, usize),
    ResCol(usize),
    /// `cells set`: a 2x2 block from a row and column.
    BlockSet(usize, usize, usize),
    /// Two rows moved together, after a row.
    BlockMove(usize, usize),
    Rename(usize, usize),
    Sort(usize, bool),
    Header(bool),
}

/// A verb's edits against `t` (`None`: not applicable now).
fn verb_edits(t: &mut Table, v: &Verb) -> Option<Vec<Edit>> {
    let rows = t.live_rows();
    let cols = t.live_cols();
    let r = |i: usize| rows.get(i % rows.len().max(1)).cloned();
    let c = |i: usize| cols.get(i % cols.len().max(1)).cloned();
    let e = match v {
        Verb::Set(i, j, x) => t.set_cell(&r(*i)?, &c(*j)?, VALUES[x % VALUES.len()]),
        Verb::Clear(i, j) => t.clear_cell(&r(*i)?, &c(*j)?),
        Verb::Unset(i, j) => {
            let (rk, ck) = (r(*i)?, c(*j)?);
            t.cell(&rk, &ck)?;
            let e = vec![Edit::Map {
                root: table::CELLS.into(),
                ops: vec![MapOp::Delete {
                    key: table::cell_key(&rk, &ck),
                }],
            }];
            t.apply(&e).ok()?;
            Ok(e)
        }
        Verb::InsRow(i) => {
            let at = r(*i).map_or(Anchor::End, Anchor::After);
            let vals: Vec<String> = cols.iter().map(|_| "n".to_owned()).collect();
            t.insert_row(at, &vals).map(|x| x.1)
        }
        Verb::MoveRow(i, j) => {
            let (a, b) = (r(*i)?, r(*j)?);
            if a == b {
                t.move_row(&a, Anchor::Start)
            } else {
                t.move_row(&a, Anchor::After(b))
            }
        }
        Verb::DelRow(i) => t.delete_row(&r(*i)?),
        Verb::ResRow(i) => {
            let d: Vec<Key> = t.rows.deleted().cloned().collect();
            let k = d.get(*i % d.len().max(1))?.clone();
            t.restore_row(&k)
        }
        Verb::InsCol(i) => t
            .insert_col(c(*i).map_or(Anchor::End, Anchor::After), None)
            .map(|x| x.1),
        Verb::DelCol(i) => {
            if cols.len() < 2 {
                return None;
            }
            t.delete_col(&c(*i)?)
        }
        Verb::MoveCol(i, j) => {
            let (a, b) = (c(*i)?, c(*j)?);
            if a == b {
                t.move_col(&a, Anchor::Start)
            } else {
                t.move_col(&a, Anchor::After(b))
            }
        }
        Verb::ResCol(i) => {
            let d: Vec<Key> = t.cols.deleted().cloned().collect();
            let k = d.get(*i % d.len().max(1))?.clone();
            t.restore_col(&k)
        }
        Verb::BlockSet(i, j, x) => {
            let v = |n: usize| VALUES[(x + n) % VALUES.len()].to_owned();
            t.set_block(&r(*i)?, &c(*j)?, &[vec![v(0), v(1)], vec![v(2), v(3)]])
        }
        Verb::BlockMove(i, j) => {
            let (a, b, to) = (r(*i)?, r(*i + 1)?, r(*j)?);
            if a == b || to == a || to == b {
                return None;
            }
            t.move_rows(&[a, b], Anchor::After(to))
        }
        Verb::Rename(j, x) => {
            if !t.header {
                return None;
            }
            t.rename_col(&c(*j)?, VALUES[x % VALUES.len()])
        }
        Verb::Sort(j, desc) => {
            let mut k = SortKey::by(c(*j)?);
            k.desc = *desc;
            t.sort(&[k])
        }
        Verb::Header(on) => t.set_header(*on),
    };
    e.ok().filter(|e| !e.is_empty())
}

/// A verb's coverage key.
fn verb_key(v: &Verb) -> &'static str {
    match v {
        Verb::Set(..) => "cell set",
        Verb::Clear(..) | Verb::Unset(..) => "cell clear",
        Verb::InsRow(..) => "row insert",
        Verb::MoveRow(..) => "row move",
        Verb::DelRow(..) => "row delete",
        Verb::ResRow(..) => "row restore",
        Verb::InsCol(..) => "col insert",
        Verb::DelCol(..) => "col delete",
        Verb::MoveCol(..) => "col move",
        Verb::ResCol(..) => "col restore",
        Verb::BlockSet(..) => "cells set",
        Verb::BlockMove(..) => "block move",
        Verb::Rename(..) => "col rename",
        Verb::Sort(..) => "sort",
        Verb::Header(..) => "header",
    }
}

fn random_verb(rng: &mut SmallRng) -> Verb {
    let i = rng.gen_range(0..8);
    let j = rng.gen_range(0..8);
    match rng.gen_range(0..26) {
        22 => Verb::MoveCol(i, j),
        23 => Verb::ResCol(i),
        24 => Verb::BlockSet(i, j, rng.gen_range(0..VALUES.len())),
        25 => Verb::BlockMove(i, j),
        0..=5 => Verb::Set(i, j, rng.gen_range(0..VALUES.len())),
        6 => Verb::Clear(i, j),
        7 => Verb::Unset(i, j),
        8..=9 => Verb::InsRow(i),
        10..=12 => Verb::MoveRow(i, j),
        13..=14 => Verb::DelRow(i),
        15 => Verb::ResRow(i),
        16 => Verb::InsCol(i),
        17 => Verb::DelCol(i),
        18 => Verb::Rename(j, i),
        19..=20 => Verb::Sort(j, rng.gen_bool(0.5)),
        _ => Verb::Header(rng.gen_bool(0.5)),
    }
}

fn random_step(rng: &mut SmallRng) -> Step {
    let p = PIDS[rng.gen_range(0..PIDS.len())];
    let mode = match rng.gen_range(0..4) {
        0 | 1 => UndoMode::All,
        2 => UndoMode::Partial,
        _ => UndoMode::Drop,
    };
    let n = if rng.gen_bool(0.8) { 1 } else { 2 };
    match rng.gen_range(0..46) {
        40..=41 => Step::OpFor(random_verb(rng)),
        42 => Step::UndoFor(n, rng.gen_bool(0.3), rng.gen_bool(0.3)),
        // half the time a live edit on a row the text changes: the row rule's conflict
        43..=45 if rng.gen_bool(0.5) => {
            let q = PIDS[rng.gen_range(0..PIDS.len())];
            let (i, j) = (rng.gen_range(0..8), rng.gen_range(0..8));
            Step::WriteBase(
                p,
                vec![(q, Verb::Set(i, j, rng.gen_range(0..4)))],
                vec![Verb::Set(i, j + 1, rng.gen_range(0..4))],
                rng.gen_bool(0.3),
            )
        }
        43..=45 => Step::WriteBase(
            p,
            (0..rng.gen_range(0..2))
                .map(|_| (PIDS[rng.gen_range(0..PIDS.len())], random_verb(rng)))
                .collect(),
            (0..rng.gen_range(1..4)).map(|_| random_verb(rng)).collect(),
            rng.gen_bool(0.3),
        ),
        0..=21 => Step::Op(p, random_verb(rng)),
        22..=28 => Step::Undo(p, n, mode),
        29..=32 => Step::Redo(p, n, mode),
        33..=35 => Step::Revert(p, rng.gen_range(0..3), rng.gen_bool(0.3)),
        36 => Step::DropDeleted,
        37..=38 => Step::Outside(vec![random_verb(rng)]),
        _ => Step::Restart,
    }
}

/// The case classes (spec §8, v1.11) and each one's scripted start.
pub const CLASSES: &[&str] = &[
    "identical_value_write",
    "sort_vs_foreign_move",
    "delete_foreign_cell_in_hidden_row",
    "insert_then_foreign_cell",
    "move_back_past_deleted_neighbour",
    "redo_after_partial_sort_undo",
    "revert_after_restart",
    "own_lifo_chain",
    "own_later_write_off_stack",
    "own_cell_clear_then_undo",
    "already_by_value",
    "drop_between_batch_and_undo",
    "header_switch_partial",
    "undo_across_outside_save",
    "write_with_no_identity",
    "move_back_then_restore_left_neighbour",
    "revert_vs_undo",
    "random",
    "row_rewritten_like_another",
    "text_row_both_changed",
    "undo_after_text_pair",
    "revert_after_text_pair",
    "undo_for_a_person",
    "column_and_block_verbs",
    "revert_over_a_drop",
    "partial_write_over_a_column_change",
];

fn script(class: &str, rng: &mut SmallRng) -> Vec<Step> {
    use Step::{DropDeleted, Op, Outside, OutsideNoId, Redo, Restart, Revert, Undo};
    use Verb::*;
    let i = rng.gen_range(0..4);
    let j = rng.gen_range(0..3);
    let x = rng.gen_range(0..4);
    let partial = UndoMode::Partial;
    let all = UndoMode::All;
    match class {
        "identical_value_write" => vec![
            Op("p1", Set(i, j, x)),
            Op("p2", Set(i, j, (x + 1) % 4)),
            Op("p2", Set(i, j, x)),
            Undo("p1", 1, all),
        ],
        "sort_vs_foreign_move" => vec![
            Op("p1", Sort(j, false)),
            Op("p2", MoveRow(i, (i + 2) % 5)),
            Undo("p1", 1, all),
            Undo("p1", 1, partial),
        ],
        "delete_foreign_cell_in_hidden_row" => vec![
            Op("p1", DelRow(i)),
            Undo("p2", 1, all),
            Op("p2", Set(i, j, x)),
            Undo("p1", 1, all),
        ],
        "insert_then_foreign_cell" => vec![
            Op("p1", InsRow(i)),
            Op("p2", Set(i + 1, j, x)),
            Undo("p1", 1, all),
            Undo("p1", 1, partial),
        ],
        "move_back_past_deleted_neighbour" => vec![
            Op("p1", MoveRow(i, (i + 2) % 4)),
            Op("p2", DelRow((i + 1) % 4)),
            Undo("p1", 1, all),
            Op("p2", ResRow(0)),
        ],
        "redo_after_partial_sort_undo" => vec![
            Op("p1", Sort(j, true)),
            Op("p2", MoveRow(i, 0)),
            Undo("p1", 1, partial),
            Redo("p1", 1, all),
            Redo("p1", 1, partial),
        ],
        "revert_after_restart" => vec![
            Op("p1", Set(i, j, x)),
            Op("p1", MoveRow(i, j)),
            Restart,
            Revert("p1", 0, false),
            Revert("p1", 1, true),
        ],
        "own_lifo_chain" => vec![
            Op("p1", Set(i, j, x)),
            Op("p1", Set(i, j, (x + 1) % 4)),
            Op("p1", MoveRow(i, j)),
            Op("p1", DelRow(j)),
            Undo("p1", 2, all),
            Undo("p1", 2, all),
            Redo("p1", 4, all),
            Undo("p1", 1, all),
        ],
        "own_later_write_off_stack" => vec![
            Op("p1", Set(i, j, x)),
            Op("p1", Set(i, j, (x + 1) % 4)),
            Undo("p1", 1, UndoMode::Drop),
            Undo("p1", 1, all),
        ],
        "own_cell_clear_then_undo" => vec![
            Op("p1", Clear(i, j)),
            Undo("p1", 1, all),
            Op("p1", Unset(i, j)),
            Undo("p1", 1, all),
            Redo("p1", 1, all),
        ],
        "already_by_value" => vec![
            Op("p1", Set(i, j, x)),
            Op("p2", Set(i, j, 4)),
            Op("p2", Set(i, j, 0)),
            Op("p1", Set(i, j, 0)),
            Op("p2", Set(i, j, x)),
            Undo("p1", 1, all),
            Redo("p1", 1, all),
        ],
        "drop_between_batch_and_undo" => vec![
            Op("p1", DelRow(i)),
            Op("p1", Set(i, j, x)),
            DropDeleted,
            Undo("p1", 2, all),
            Undo("p1", 2, partial),
        ],
        "header_switch_partial" => vec![
            Op("p1", Header(true)),
            Op("p2", Rename(j, x)),
            Undo("p1", 1, partial),
            Undo("p1", 1, UndoMode::Drop),
        ],
        "undo_across_outside_save" => vec![
            Op("p1", Set(i, j, x)),
            Outside(vec![MoveRow(i, j), Set((i + 1) % 4, j, 2)]),
            Op("p1", MoveRow(j, i)),
            Undo("p1", 2, all),
            Undo("p1", 1, partial),
        ],
        // KC7 (Yrs only): a holder with no identity is someone else's, never own
        // the outside writes "" (VALUES[4]): never a body value nor one p1 wrote, so the
        // undo isn't `already`
        "write_with_no_identity" => match rng.gen_range(0..3) {
            // a cell's value
            0 => vec![
                Op("p1", Set(i, j, x)),
                Op("p1", Set(i, (j + 1) % 3, x)),
                OutsideNoId(vec![Set(i, j, 4)]),
                Undo("p1", 2, all),
                Undo("p1", 2, partial),
                Redo("p1", 1, all),
            ],
            // a row's position: p1 moves row i to the start, the outside moves it on
            1 => vec![
                Op("p1", MoveRow(i, i)),
                OutsideNoId(vec![MoveRow(0, i % 2 + 1)]),
                Undo("p1", 1, all),
                Undo("p1", 1, partial),
            ],
            // a cell in p1's new row
            _ => vec![
                Op("p1", InsRow(i)),
                OutsideNoId(vec![Set(i + 1, j, 4)]),
                Undo("p1", 1, all),
                Undo("p1", 1, partial),
            ],
        },
        // S4 (v1.11 addendum): p2 deletes row 1; p1 moves the next row to the start and
        // undoes it (back after the deleted row, its place-order neighbour); p2's delete is
        // undone: every row is back at its index before the delete
        "move_back_then_restore_left_neighbour" => vec![
            Op("p2", DelRow(1)),
            Op("p1", MoveRow(1, 1)),
            Undo("p1", 1, all),
            Undo("p2", 1, all),
        ],
        // v0.10.0's row pairing: a live edit on B, then a save that rewrites A, often like B,
        // and deletes B (paired by likeness, in an unequal gap), or a swap around a row that
        // stays (two moves); undo, revert and later edits on what came of them
        "row_rewritten_like_another" => vec![
            Op("p1", Set((i + 1) % 4, j, x)),
            Outside(vec![
                Set(i, 0, (x + 1) % 4),
                Set(i, 1, (x + 2) % 4),
                DelRow((i + 1) % 4),
            ]),
            Op("p2", Set(i, j, (x + 3) % 4)),
            Undo("p1", 1, partial),
            Outside(vec![MoveRow(0, 2), MoveRow(1, 0)]),
            Op("p1", Set(i, j, x)),
            Undo("p2", 1, all),
            Revert("p1", 0, true),
        ],
        // ruling (a): every trace has a `--partial` write --base with a column layout change on
        // one side (a live column delete, a text column delete or move) beside a cell edit, so
        // the model's `layout_held` is checked against the kit at every head
        "partial_write_over_a_column_change" => {
            let c = rng.gen_range(0..3);
            let (live, text) = match rng.gen_range(0..4) {
                0 => (vec![("p2", DelCol(c))], vec![Set(i, (c + 1) % 3, x)]),
                1 => (
                    vec![("p2", DelCol(c))],
                    vec![Set(i, c, (x + 1) % 4), MoveCol((c + 1) % 3, 0)],
                ),
                2 => (
                    vec![("p2", Set(i, c, x))],
                    vec![DelCol(c), Set((i + 1) % 4, (c + 1) % 3, (x + 2) % 4)],
                ),
                _ => (
                    vec![("p2", DelCol(c))],
                    vec![MoveCol(c, 2), Set(i, (c + 2) % 3, x)],
                ),
            };
            vec![
                Step::WriteBase("p1", live, text, true),
                Op("p2", Set((i + 2) % 4, j, (x + 3) % 4)),
                Undo("p1", 1, if rng.gen_bool(0.5) { all } else { partial }),
                Undo("p2", 1, all),
            ]
        }
        // write --base: the text and a live edit change one row, or different rows
        "text_row_both_changed" => vec![
            Step::WriteBase(
                "p1",
                vec![("p2", Set(i, j, x))],
                vec![Set(i, (j + 1) % 3, (x + 1) % 4)],
                rng.gen_bool(0.5),
            ),
            Step::WriteBase(
                "p2",
                vec![("p1", Set(i, j, x))],
                vec![Set((i + 1) % 4, j, (x + 2) % 4)],
                false,
            ),
            Undo("p1", 1, all),
        ],
        // S13: a text re-pairs a row, someone edits it, the text's writer undoes
        "undo_after_text_pair" => vec![
            Step::WriteBase(
                "p1",
                Vec::new(),
                vec![
                    Set(i, 0, (x + 1) % 4),
                    Set(i, 1, (x + 2) % 4),
                    DelRow((i + 1) % 4),
                ],
                false,
            ),
            Op("p2", Set(i, j, (x + 3) % 4)),
            Undo("p1", 1, if rng.gen_bool(0.5) { all } else { partial }),
            Redo("p1", 1, all),
        ],
        // S13: the same, and a revert rewrites the row whole
        "revert_after_text_pair" => vec![
            Step::WriteBase(
                "p1",
                Vec::new(),
                vec![
                    Set(i, 0, (x + 1) % 4),
                    Set(i, 1, (x + 2) % 4),
                    DelRow((i + 1) % 4),
                ],
                false,
            ),
            Op("p2", Set(i, j, (x + 3) % 4)),
            Revert("p1", 0, false),
            Undo("p1", 1, partial),
        ],
        "undo_for_a_person" => vec![
            Step::OpFor(Set(i, j, x)),
            Step::OpFor(MoveRow(i, j)),
            Step::UndoFor(1, false, false),
            Step::UndoFor(1, true, false),
            Op("p2", Set(i, j, (x + 1) % 4)),
            Step::UndoFor(2, false, true),
        ],
        "column_and_block_verbs" => vec![
            Op("p1", MoveCol(j, j + 1)),
            Op("p2", DelCol(j)),
            Op("p1", ResCol(0)),
            Op("p1", BlockSet(i, j, x)),
            Op("p2", BlockMove(i, j + 2)),
            Undo("p1", 1, partial),
            Undo("p2", 1, all),
            Outside(vec![BlockSet(i, 0, x)]),
        ],
        // S11: a drop of a row and a column holding writes, then reverts of the drop and of the
        // writes, partial and not
        "revert_over_a_drop" => vec![
            Op("p2", Set(i, j, x)),
            Op("p2", Set((i + 1) % 4, (j + 1) % 3, (x + 1) % 4)),
            Op("p0", DelRow(i)),
            Op("p0", DelCol((j + 1) % 3)),
            DropDeleted,
            Revert("p2", 0, true),
            Revert("p2", 1, false),
            Undo("p2", 1, partial),
        ],
        "revert_vs_undo" => vec![
            Op("p1", Set(i, j, x)),
            Op("p1", MoveRow(i, j)),
            Op("p2", Set(i, j, (x + 2) % 4)),
            Revert("p1", 1, false),
            Undo("p1", 1, all),
            Revert("p1", 0, true),
        ],
        _ => Vec::new(),
    }
}

/// A trace's result.
pub struct TraceResult {
    pub class: &'static str,
    pub steps: usize,
    pub mismatch: Option<String>,
    /// What the trace reached and compared, by key ([`FLOORS`]).
    pub hits: BTreeMap<&'static str, u64>,
}

/// A floor on one key's count in one class's share of a run of N traces per class.
#[derive(Debug, Clone, Copy)]
enum Floor {
    /// At least `N / f` (a hit per `f` traces; integer division, so below `N = f` it asks
    /// nothing of a class).
    Every(u64),
    /// At least `N × k` (`k` a trace).
    Each(u64),
}

impl Floor {
    fn of(self, n: u64) -> u64 {
        match self {
            Floor::Every(f) => n / f,
            Floor::Each(k) => n * k,
        }
    }
}

/// The classes a floor applies to.
#[derive(Debug, Clone, Copy)]
enum In {
    /// Every class: every class's random tail can produce the key at the floor's rate.
    All,
    /// These classes: their scripts produce the key; elsewhere only the random tail does, too
    /// rarely for a floor per class (some class reached it 0 times in 256 traces), and the key
    /// counts only toward the run's "at least once".
    Only(&'static [&'static str]),
}

/// What the traces must reach and compare, per class (spec §8: a run of N traces per class must
/// hit each key at least N / floor in each class, and at least once): each row is a key (a
/// coverage key, or a comparison count: `C_*`), its [`Floor`] and the classes it applies to
/// ([`In`]). A class a row applies to that falls below the row's floor for the traces it ran
/// fails the test; so does a key of this table that the whole run (every class: CI, the node's
/// full run) never reached. A key may have two rows: a weak one in every class and a strong one
/// in the classes that script it. Each floor is the strongest (the smallest `f`, the largest
/// `k`) that leaves a 2× margin under the lowest class it applies to at N = 10 (CI), 64 and
/// 256 (measured on node jobs 83c76426, ef65cc44 for `on_behalf_of` and 7f8275fd for
/// `restored_by`); at N = 10 the rows `Every(f)` with `f > 10` ask nothing of a class.
const FLOORS: &[(&str, Floor, In)] = &[
    // the comparisons: every class runs them on every trace
    (C_STATES, Floor::Each(2), In::All),
    (C_VERSIONS, Floor::Each(4), In::All),
    (C_STACKS, Floor::Each(8), In::All),
    (C_OUTCOMES, Floor::Each(1), In::All),
    (C_HOLDERS, Floor::Each(16), In::All),
    (AUTHORS_COMPARED, Floor::Each(16), In::All),
    (C_OBO, Floor::Each(16), In::All),
    (C_OBO_AGENT, Floor::Every(16), In::All),
    (
        C_OBO_AGENT,
        Floor::Each(1),
        In::Only(&["undo_for_a_person"]),
    ),
    (C_RESTORED, Floor::Each(16), In::All),
    (C_RESTORED_SOME, Floor::Every(8), In::All),
    // its script's undos put b1's write back (S2)
    (
        C_RESTORED_SOME,
        Floor::Each(4),
        In::Only(&["own_lifo_chain"]),
    ),
    (C_CONFLICTS, Floor::Every(8), In::All),
    (C_CONFLICTS, Floor::Every(2), In::Only(WRITE_BASE_CLASSES)),
    (C_SYNCS, Floor::Every(16), In::All),
    (C_SYNCS, Floor::Every(2), In::Only(OUTSIDE_CLASSES)),
    // the generator's steps
    ("cell set", Floor::Every(4), In::All),
    ("cell clear", Floor::Every(16), In::All),
    ("cells set", Floor::Every(128), In::All),
    (
        "cells set",
        Floor::Every(4),
        In::Only(&["column_and_block_verbs"]),
    ),
    ("row insert", Floor::Every(16), In::All),
    ("row move", Floor::Every(16), In::All),
    ("block move", Floor::Every(128), In::All),
    (
        "block move",
        Floor::Every(8),
        In::Only(&["column_and_block_verbs"]),
    ),
    ("row delete", Floor::Every(16), In::All),
    ("row restore", Floor::Every(128), In::All),
    (
        "row restore",
        Floor::Every(2),
        In::Only(&["move_back_past_deleted_neighbour"]),
    ),
    ("col insert", Floor::Every(32), In::All),
    ("col move", Floor::Every(128), In::All),
    (
        "col move",
        Floor::Every(4),
        In::Only(&["column_and_block_verbs"]),
    ),
    ("col delete", Floor::Every(32), In::All),
    // no row in every class: the tail alone reaches it too rarely
    (
        "col restore",
        Floor::Every(2),
        In::Only(&["column_and_block_verbs"]),
    ),
    // no row in every class: the tail alone reaches it too rarely
    (
        "col rename",
        Floor::Every(4),
        In::Only(&["header_switch_partial"]),
    ),
    ("header", Floor::Every(128), In::All),
    (
        "header",
        Floor::Every(2),
        In::Only(&["header_switch_partial"]),
    ),
    ("sort", Floor::Every(16), In::All),
    (
        "sort",
        Floor::Every(2),
        In::Only(&["sort_vs_foreign_move", "redo_after_partial_sort_undo"]),
    ),
    ("outside save", Floor::Every(16), In::All),
    ("outside save", Floor::Every(2), In::Only(OUTSIDE_CLASSES)),
    ("drop-deleted", Floor::Every(32), In::All),
    (
        "drop-deleted",
        Floor::Every(2),
        In::Only(&["drop_between_batch_and_undo", "revert_over_a_drop"]),
    ),
    ("restart", Floor::Every(32), In::All),
    (
        "restart",
        Floor::Every(2),
        In::Only(&["revert_after_restart"]),
    ),
    ("undo", Floor::Every(4), In::All),
    ("redo", Floor::Every(8), In::All),
    (
        "redo",
        Floor::Every(2),
        In::Only(&[
            "redo_after_partial_sort_undo",
            "own_lifo_chain",
            "already_by_value",
            "own_cell_clear_then_undo",
        ]),
    ),
    ("revert", Floor::Every(32), In::All),
    (
        "revert",
        Floor::Every(2),
        In::Only(&[
            "revert_after_restart",
            "revert_vs_undo",
            "revert_over_a_drop",
            "revert_after_text_pair",
        ]),
    ),
    ("undo --partial", Floor::Every(8), In::All),
    ("undo --steps", Floor::Every(16), In::All),
    (
        "undo --steps",
        Floor::Every(2),
        In::Only(&[
            "own_lifo_chain",
            "drop_between_batch_and_undo",
            "undo_across_outside_save",
        ]),
    ),
    ("undo --drop", Floor::Every(16), In::All),
    (
        "undo --drop",
        Floor::Every(2),
        In::Only(&["own_later_write_off_stack"]),
    ),
    ("undo --for", Floor::Every(32), In::All),
    (
        "undo --for",
        Floor::Every(1),
        In::Only(&["undo_for_a_person"]),
    ),
    ("write --base clean", Floor::Every(16), In::All),
    (
        "write --base clean",
        Floor::Every(2),
        In::Only(&["undo_after_text_pair", "revert_after_text_pair"]),
    ),
    ("write --base conflict", Floor::Every(32), In::All),
    (
        "write --base conflict",
        Floor::Every(4),
        In::Only(&["text_row_both_changed"]),
    ),
    ("write --base --partial", Floor::Every(32), In::All),
    (
        "write --base --partial",
        Floor::Every(2),
        In::Only(&["partial_write_over_a_column_change"]),
    ),
    // no row in every class: the tail alone reaches it too rarely
    (
        "write --base --partial held by a column",
        Floor::Every(8),
        In::Only(&["partial_write_over_a_column_change"]),
    ),
    // S13: an undo or redo held a row (every class at 256 traces; strongly where scripted)
    ("S13 held", Floor::Every(256), In::All),
    (
        "S13 held",
        Floor::Every(16),
        In::Only(&["undo_after_text_pair"]),
    ),
    // S13: a revert rewrote a row whole; no row in every class: the tail alone reaches it too
    // rarely
    (
        "S13 whole",
        Floor::Every(8),
        In::Only(&["revert_after_text_pair"]),
    ),
];

/// The classes whose scripts run a write --base.
const WRITE_BASE_CLASSES: &[&str] = &[
    "text_row_both_changed",
    "undo_after_text_pair",
    "revert_after_text_pair",
    "partial_write_over_a_column_change",
];

/// The classes whose scripts run an outside save.
const OUTSIDE_CLASSES: &[&str] = &[
    "undo_across_outside_save",
    "row_rewritten_like_another",
    "column_and_block_verbs",
    "write_with_no_identity",
];

/// Every [`FLOORS`] row that a class falls below, one message each (a shard checks the classes
/// it ran); with `whole_run`, also each key the run never reached. Prints each row's lowest
/// class.
fn floors_unmet(hits: &BTreeMap<&'static str, ClassRun>, whole_run: bool) -> Vec<String> {
    let mut unmet = Vec::new();
    for (key, _, within) in FLOORS {
        if let In::Only(cs) = within {
            for c in cs.iter() {
                assert!(CLASSES.contains(c), "FLOORS: {key}: no class {c}");
            }
        }
    }
    for (i, (key, floor, within)) in FLOORS.iter().enumerate() {
        let mut lowest: Option<(u64, u64, &str)> = None;
        for (c, h) in hits {
            if let In::Only(cs) = within {
                if !cs.contains(c) {
                    continue;
                }
            }
            let got = h.cov.get(key).copied().unwrap_or(0);
            let want = floor.of(h.traces);
            if lowest.map_or(true, |(g, _, _)| got < g) {
                lowest = Some((got, want, *c));
            }
            if got < want {
                unmet.push(format!(
                    "floor: {key} {floor:?} {within:?}: class {c} reached {got} in {} traces, \
                     below {want}",
                    h.traces
                ));
            }
        }
        if let Some((got, want, c)) = lowest {
            println!(
                "[grid traces] floor {key} {floor:?} {within:?}: lowest {got} ({c}), floor {want}"
            );
        }
        // the run as a whole: once per key (a key may have two rows)
        if whole_run
            && FLOORS.iter().position(|r| r.0 == *key) == Some(i)
            && hits
                .values()
                .all(|h| h.cov.get(key).copied().unwrap_or(0) == 0)
        {
            unmet.push(format!("floor: {key}: no trace of the run reached it"));
        }
    }
    unmet
}

/// The spec's generator line (`SPEC-UNDO.md` §8, "**Generator**"), phrase by phrase, and the
/// coverage key each means. A phrase with no key, or a key with no hit, fails the test.
const SPEC_GENERATOR: &[(&str, &str)] = &[
    ("cell set", "cell set"),
    ("clear", "cell clear"),
    ("block set", "cells set"),
    ("row insert", "row insert"),
    ("row move", "row move"),
    ("row delete", "row delete"),
    ("row restore", "row restore"),
    ("column insert", "col insert"),
    ("column move", "col move"),
    ("column delete", "col delete"),
    ("column restore", "col restore"),
    ("rename", "col rename"),
    ("header on/off", "header"),
    ("sort", "sort"),
    ("a block move", "block move"),
    ("write --base on the grid", "write --base clean"),
    ("write --base --partial", "write --base --partial"),
    ("an outside save (reconcile)", "outside save"),
    ("drop-deleted", "drop-deleted"),
    ("restart", "restart"),
    ("undo", "undo"),
    ("--steps N", "undo --steps"),
    ("--partial", "undo --partial"),
    ("--drop", "undo --drop"),
    ("redo", "redo"),
    ("revert", "revert"),
    ("undo --for", "undo --for"),
];

fn the_body(rng: &mut SmallRng) -> String {
    let rows = rng.gen_range(3..6);
    let cols = rng.gen_range(2..4);
    let mut s = String::new();
    for r in 0..rows {
        let f: Vec<String> = (0..cols)
            .map(|c| VALUES[(r * 3 + c + rng.gen_range(0..2)) % 4].to_owned())
            .collect();
        s.push_str(&f.join(","));
        s.push('\n');
    }
    s
}

/// Run one trace of `class` from `seed`. `MPC_MERGE_MUTANT` plants a kit merge fault
/// (`k2_mpc_base::differential::MergeMutant`: `whole-row-restores-dropped` is (xxxv),
/// `rewritten-row-before-dropped` is (xxxvi)) on the trace's thread, so the model can be shown
/// to disagree with the mutated kit. Unset, the kit runs as is.
pub fn run(class: &'static str, seed: u64, mutant: GridMutant) -> TraceResult {
    use k2_mpc_base::differential::{with_merge_mutant, MergeMutant};
    match std::env::var("MPC_MERGE_MUTANT").as_deref() {
        Ok("whole-row-restores-dropped") => {
            with_merge_mutant(MergeMutant::WholeRowRestoresDropped, || {
                run_trace(class, seed, mutant)
            })
        }
        Ok("rewritten-row-before-dropped") => {
            with_merge_mutant(MergeMutant::RewrittenRowBeforeDropped, || {
                run_trace(class, seed, mutant)
            })
        }
        Ok(other) => panic!("MPC_MERGE_MUTANT={other}: not a merge mutant the traces plant"),
        Err(_) => run_trace(class, seed, mutant),
    }
}

fn run_trace(class: &'static str, seed: u64, mutant: GridMutant) -> TraceResult {
    let mut rng = SmallRng::seed_from_u64(
        seed ^ (CLASSES.iter().position(|c| *c == class).unwrap() as u64) << 40,
    );
    let body = the_body(&mut rng);
    let mut kit = Kit::new(seed, &body);
    let mut g = Grid::new(&LISTS, &MAPS);
    g.mutant = mutant;
    // the agent joins on p1's behalf (`Kit::open`)
    g.acts_for.insert("a0".to_owned(), "p1".to_owned());
    // the import, as the kit's session imports
    let content = Lite.decode(body.as_bytes()).unwrap().roots;
    let mut imp = Vec::new();
    for (root, n) in &content {
        match n {
            Neutral::List(items) if !items.is_empty() => {
                let mut at = Anchor::Start;
                let mut ops = Vec::new();
                for (k, v) in items {
                    ops.push(k2_mpc_base::elem::ListOp::Insert {
                        key: k.clone(),
                        at: at.clone(),
                        value: v.clone(),
                    });
                    at = Anchor::After(k.clone());
                }
                imp.push(Edit::List {
                    root: root.clone(),
                    ops,
                });
            }
            Neutral::Map(m) if !m.is_empty() => imp.push(Edit::Map {
                root: root.clone(),
                ops: m
                    .iter()
                    .map(|(k, v)| MapOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect(),
            }),
            _ => {}
        }
    }
    g.apply("kit:import", &imp, &HashMap::new());
    let mut steps = script(class, &mut rng);
    let tail = rng.gen_range(4..14);
    for _ in 0..tail {
        steps.push(random_step(&mut rng));
    }
    let mut own: HashMap<&'static str, Vec<u64>> = HashMap::new();
    // the kit's recent writes, as its echo detection keeps them (`Disk::own`): a file holding
    // one is its own write coming back, not an outside edit; an outside save that comes in
    // clears them, as the kit does (v0.9.5's echo rule, on grid ingests since the C1 merge
    // with main: after a person's save, an earlier write of the kit's is their edit)
    let mut owns: Vec<String> = Vec::new();
    let mut done = 0;
    let mut cov: BTreeMap<&'static str, u64> = BTreeMap::new();
    let fail = |done: usize, step: &Step, what: String| TraceResult {
        class,
        steps: done,
        mismatch: Some(format!(
            "{class} seed {seed} step {done} ({step:?}): {what}"
        )),
        hits: BTreeMap::new(),
    };
    for step in &steps {
        let before_holders = g.holders.clone();
        let kit_out: Option<Outcome>;
        let model_out: Option<Outcome>;
        match step {
            Step::Op(p, v) => {
                let mut t = Table::from_keyed(&keyed(&g)).unwrap();
                let Some(edits) = verb_edits(&mut t, v) else {
                    continue;
                };
                let r = kit
                    .s()
                    .edit_ops(kit.conns[p], edits.clone(), &EditOptions::new());
                let r = match r {
                    Ok(r) => r,
                    Err(e) => return fail(done, step, format!("the kit refused: {e}")),
                };
                if let Some(s) = g.apply(p, &edits, &HashMap::new()) {
                    g.push_edit(p, s, None);
                    own.entry(p).or_default().push(s);
                }
                *cov.entry(C_VERSIONS).or_default() += 1;
                if r.version != g.seq {
                    return fail(
                        done,
                        step,
                        format!("versions: kit {} model {}", r.version, g.seq),
                    );
                }
                *cov.entry(verb_key(v)).or_default() += 1;
                kit_out = None;
                model_out = None;
            }
            Step::OpFor(v) => {
                let mut t = Table::from_keyed(&keyed(&g)).unwrap();
                let Some(edits) = verb_edits(&mut t, v) else {
                    continue;
                };
                // an agent names a version
                let o = EditOptions::new().if_version(g.seq);
                let r = kit.s().edit_ops(kit.conns["a0"], edits.clone(), &o);
                if let Err(e) = r {
                    return fail(done, step, format!("the kit refused: {e}"));
                }
                if let Some(s) = g.apply("a0", &edits, &HashMap::new()) {
                    g.push_edit("a0", s, Some("p1"));
                }
                *cov.entry(verb_key(v)).or_default() += 1;
                kit_out = None;
                model_out = None;
            }
            Step::UndoFor(n, redo, partial) => {
                let mode = if *partial {
                    UndoMode::Partial
                } else {
                    UndoMode::All
                };
                let req = UndoRequest::steps(*n).mode(mode).for_person("p1");
                let r = if *redo {
                    kit.s().redo_with(kit.conns["a0"], &req)
                } else {
                    kit.s().undo_with(kit.conns["a0"], &req)
                };
                kit_out = Some(outcome_of(r.unwrap()));
                model_out = Some(g.undo_for("a0", "p1", *n, *partial, *redo, true));
                *cov.entry("undo --for").or_default() += 1;
            }
            Step::WriteBase(p, live, text, partial) => {
                // Option A (the evidence note): the merge rule itself comes from the kit's
                // `merge3_text`; this checks everything downstream of it against the model. The
                // rule's independent checks are the row-pairing property and K2 Engine's model.
                let conn = kit.conns[p];
                let read = kit.s().read(conn).unwrap().version;
                let t0 = Table::from_keyed(&keyed(&g)).unwrap();
                for (q, v) in live {
                    let mut t = Table::from_keyed(&keyed(&g)).unwrap();
                    let Some(edits) = verb_edits(&mut t, v) else {
                        continue;
                    };
                    if let Err(e) =
                        kit.s()
                            .edit_ops(kit.conns[q], edits.clone(), &EditOptions::new())
                    {
                        return fail(done, step, format!("the kit refused: {e}"));
                    }
                    if let Some(s) = g.apply(q, &edits, &HashMap::new()) {
                        g.push_edit(q, s, None);
                        own.entry(q).or_default().push(s);
                    }
                }
                let theirs = Table::from_keyed(&keyed(&g)).unwrap();
                let mut t = t0.clone();
                for v in text {
                    verb_edits(&mut t, v);
                }
                let file = render(&t);
                let rec = table::reconcile(&t0, &records(&file)).unwrap();
                let mut ours = t0.clone();
                ours.apply(&rec.edits).unwrap();
                let exact = rec.exact_rows.iter().cloned().collect();
                let m = table::merge3_text(&t0, &ours, &theirs, &exact).unwrap();
                let held: Vec<&Key> = m
                    .conflicts
                    .iter()
                    .filter_map(|c| match &c.at {
                        table::Region::Row(r) | table::Region::Cell { row: r, .. } => Some(r),
                        _ => None,
                    })
                    .collect();
                let marks: Vec<Key> = if rec.cols_exact {
                    t0.live_rows()
                        .into_iter()
                        .filter(|k| {
                            ours.rows.is_live(k)
                                && !rec.exact_rows.contains(k)
                                && !held.contains(&k)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let opts = if *partial {
                    k2_mpc_base::session::GridWriteOptions::default().partial()
                } else {
                    k2_mpc_base::session::GridWriteOptions::default()
                };
                let r = kit
                    .s()
                    .write_base_grid(conn, read, file.as_bytes(), &opts)
                    .unwrap();
                let says = |c: &[k2_mpc_base::session::GridConflict]| {
                    let mut v: Vec<String> = c.iter().map(|x| x.conflict.to_string()).collect();
                    v.sort();
                    v
                };
                let mut want: Vec<String> = m.conflicts.iter().map(|c| c.to_string()).collect();
                want.sort();
                let got = match &r.outcome {
                    k2_mpc_base::session::GridWrite::Conflicts(c) => says(c),
                    k2_mpc_base::session::GridWrite::Applied(_) => says(&r.skipped),
                    o => return fail(done, step, format!("write --base: {o:?}")),
                };
                *cov.entry(C_CONFLICTS).or_default() += 1;
                if got != want {
                    return fail(done, step, format!("conflicts: kit {got:?} merge {want:?}"));
                }
                // ruling (a): `--partial` applies nothing of a text when a column layout change on
                // either side conflicts with it (any conflict at a column, or a `deleted_col` at
                // a cell or a name): the session's `layout_held` (grid.rs, write_base_grid)
                let layout_held = m.conflicts.iter().any(|c| {
                    matches!(c.at, table::Region::Col(_))
                        || c.rule == table::ConflictRule::DeletedCol
                });
                if want.is_empty() || (*partial && !layout_held) {
                    if let Some(s) = g.apply(p, &m.edits, &HashMap::new()) {
                        g.push_edit(p, s, None);
                        g.mark_text(s, &marks, !rec.cols_exact);
                        own.entry(p).or_default().push(s);
                    }
                }
                *cov.entry(if want.is_empty() {
                    "write --base clean"
                } else {
                    "write --base conflict"
                })
                .or_default() += 1;
                if *partial {
                    *cov.entry("write --base --partial").or_default() += 1;
                    if layout_held && !want.is_empty() {
                        *cov.entry("write --base --partial held by a column")
                            .or_default() += 1;
                    }
                }
                // the change ids later steps revert are the model's: if the kit's version moved
                // otherwise, every later id would name another change (the seq skew): stop here
                let kv = kit.s().read(conn).unwrap().version;
                *cov.entry(C_VERSIONS).or_default() += 1;
                if kv != g.seq {
                    return fail(done, step, format!("version: kit {kv} model {}", g.seq));
                }
                kit_out = None;
                model_out = None;
            }
            Step::Undo(p, n, mode) | Step::Redo(p, n, mode) => {
                let redo = matches!(step, Step::Redo(..));
                *cov.entry(if redo { "redo" } else { "undo" }).or_default() += 1;
                if *n > 1 {
                    *cov.entry("undo --steps").or_default() += 1;
                }
                match mode {
                    UndoMode::Partial => *cov.entry("undo --partial").or_default() += 1,
                    UndoMode::Drop => *cov.entry("undo --drop").or_default() += 1,
                    _ => {}
                }
                let req = UndoRequest::steps(*n).mode(*mode);
                let r = if redo {
                    kit.s().redo_with(kit.conns[p], &req)
                } else {
                    kit.s().undo_with(kit.conns[p], &req)
                };
                kit_out = Some(outcome_of(r.unwrap()));
                model_out = Some(g.undo(
                    p,
                    *n,
                    *mode == UndoMode::Partial,
                    *mode == UndoMode::Drop,
                    redo,
                    true,
                ));
            }
            Step::Revert(p, back, partial) => {
                let mine = own.get(p).cloned().unwrap_or_default();
                let Some(&c) = mine.iter().rev().nth(*back) else {
                    continue;
                };
                kit_out = Some(outcome_of(
                    kit.s().revert(kit.conns[p], c, *partial).unwrap(),
                ));
                *cov.entry("revert").or_default() += 1;
                let before = g.seq;
                model_out = Some(g.revert(p, c, *partial, true));
                if g.seq > before {
                    own.entry(p).or_default().push(g.seq);
                }
            }
            Step::DropDeleted => {
                let mut t = Table::from_keyed(&keyed(&g)).unwrap();
                let edits = t.drop_deleted().unwrap();
                kit.s().drop_deleted(kit.conns["p0"]).unwrap();
                g.apply("p0", &edits, &HashMap::new());
                *cov.entry("drop-deleted").or_default() += 1;
                kit_out = None;
                model_out = None;
            }
            Step::Outside(vs) | Step::OutsideNoId(vs) => {
                let no_id = matches!(step, Step::OutsideNoId(_));
                let fl = kit.s().flush(kit.conns["p1"]).unwrap();
                let t0 = Table::from_keyed(&keyed(&g)).unwrap();
                if fl.written {
                    // the kit's own recent writes: a file holding one is an echo, not an
                    // outside edit (it keeps the last 8)
                    owns.push(render(&t0));
                    if owns.len() > 8 {
                        owns.remove(0);
                    }
                }
                let mut t = t0.clone();
                for v in vs {
                    verb_edits(&mut t, v);
                }
                let file = render(&t);
                // the same bytes are no outside edit (the file didn't change)
                let echo = owns.contains(&file);
                let (edits, marks, remap) = if file == render(&t0) || echo {
                    (Vec::new(), Vec::new(), false)
                } else {
                    // S13: the rows the text kept on their keys without leaving them exact, or
                    // the columns remapped (a doc-level mark instead)
                    let r = table::reconcile(&t0, &records(&file)).unwrap();
                    let marks: Vec<Key> = if r.cols_exact {
                        t0.live_rows()
                            .into_iter()
                            .filter(|k| r.rows.contains(k) && !r.exact_rows.contains(k))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    (r.edits, marks, !r.cols_exact)
                };
                // a save that reaches the kit's ingest clears its memory of its own writes
                if file != render(&t0) && !echo {
                    owns.clear();
                }
                std::fs::write(kit.dir.join("files/t.csv"), &file).unwrap();
                kit.s()
                    .set_outside_without_identity_for_tests(no_id)
                    .unwrap();
                let dc = kit.s().file_changed().unwrap();
                kit.s()
                    .set_outside_without_identity_for_tests(false)
                    .unwrap();
                // K-AUD-1's sync check after every outside step (the harness flushed first, so
                // the doc was clean): the replica the next one is made on is the doc, a file
                // left as it was included (the change-counter marker re-syncs a replica that
                // lagged a change that only deleted; v0.9.5's rule)
                let drift = kit.s().audit_disk_sync().unwrap();
                *cov.entry(C_SYNCS).or_default() += 1;
                if !drift.is_empty() {
                    return fail(
                        done,
                        step,
                        format!("K-AUD-1 after the outside save: {drift:?}"),
                    );
                }
                if std::env::var("GRID_VERBOSE").is_ok() {
                    println!("outside: {:?} -> {file:?}: {dc:?}", render(&t0));
                }
                *cov.entry("outside save").or_default() += 1;
                if let Some(seq) = g.apply("kit:external", &edits, &HashMap::new()) {
                    g.mark_text(seq, &marks, remap);
                    if no_id {
                        g.anon.insert(seq);
                    }
                }
                kit_out = None;
                model_out = None;
            }
            Step::Restart => {
                *cov.entry("restart").or_default() += 1;
                // the kit's own writes are remembered in memory only
                owns.clear();
                kit.restart();
                g.restart();
                kit_out = None;
                model_out = None;
            }
        }
        done += 1;
        if std::env::var("GRID_VERBOSE").is_ok() {
            let kk = kit_keyed(&kit);
            let rows = |k: &KeyedState| match &k.roots[0].1 {
                KeyedRoot::List { places, hidden, .. } => {
                    format!("{places:?} dead {:?}", hidden.keys().collect::<Vec<_>>())
                }
                _ => String::new(),
            };
            println!(
                "step {done} {step:?}\n  kit   {}\n  model {}",
                rows(&kk),
                rows(&keyed(&g))
            );
        }
        if kit_out.is_some() || model_out.is_some() {
            *cov.entry(C_OUTCOMES).or_default() += 1;
        }
        if kit_out != model_out {
            return fail(
                done,
                step,
                format!("outcome: kit {kit_out:?} model {model_out:?}"),
            );
        }
        let ks = kit.s().read(kit.conns["p1"]).unwrap();
        *cov.entry(C_VERSIONS).or_default() += 1;
        if ks.version != g.seq {
            return fail(
                done,
                step,
                format!("version: kit {} model {}", ks.version, g.seq),
            );
        }
        let kitk = kit_keyed(&kit);
        *cov.entry(C_STATES).or_default() += 1;
        if shape(&kitk) != shape(&keyed(&g)) {
            return fail(
                done,
                step,
                format!(
                    "state:\n kit   {:?}\n model {:?}",
                    shape(&kitk),
                    shape(&keyed(&g))
                ),
            );
        }
        for p in PIDS.into_iter().chain(["a0"]) {
            let kd = kit.s().undo_depths(p).unwrap();
            *cov.entry(C_STACKS).or_default() += 1;
            if kd != g.depths(p) {
                return fail(
                    done,
                    step,
                    format!("{p}'s stacks: kit {kd:?} model {:?}", g.depths(p)),
                );
            }
        }
        // the holders this step wrote
        let changed: Vec<(String, String, model::Facet)> = g
            .holders
            .iter()
            .filter(|(k, h)| before_holders.get(*k) != Some(*h))
            .map(|(k, _)| k.clone())
            .collect();
        if let Some(m) = holders_differ(&kit, &g, changed, &mut cov) {
            return fail(done, step, m);
        }
    }
    let all: Vec<(String, String, model::Facet)> = g.holders.keys().cloned().collect();
    if let Some(m) = holders_differ(&kit, &g, all, &mut cov) {
        return TraceResult {
            class,
            steps: done,
            mismatch: Some(format!("{class} seed {seed} at the end: {m}")),
            hits: BTreeMap::new(),
        };
    }
    *cov.entry("S13 held").or_default() += g.s13_held;
    *cov.entry("S13 whole").or_default() += g.s13_whole;
    TraceResult {
        class,
        steps: done,
        mismatch: None,
        hits: cov,
    }
}

/// The kit's state by key, deleted elements included (the `conformance` feature's hook).
fn kit_keyed(kit: &Kit) -> KeyedState {
    kit.s().keyed_state_for_tests().unwrap()
}

// The comparison counts, kept beside the coverage keys in a trace's `hits` (a trace that
// failed reports none): how often each comparison of kit and model ran.
/// The table by key after a step (`shape`).
const C_STATES: &str = "states compared";
/// The version: after every step, and inside an edit and a write --base.
const C_VERSIONS: &str = "versions compared";
/// One participant's stack depths after a step (four participants a step).
const C_STACKS: &str = "stacks compared";
/// An undo, redo, revert or undo --for outcome (a step with one on either side).
const C_OUTCOMES: &str = "outcomes compared";
/// A facet holder's change (S2's resolved change against `blame_elem`'s), after a step that
/// wrote it and at the end of the trace (`Facet::Insert` skipped).
const C_HOLDERS: &str = "holders compared";
/// A facet holder's author (the model's change author against `blame_elem`'s), after its change
/// agreed.
const AUTHORS_COMPARED: &str = "holder authors compared";
/// A facet holder's `on_behalf_of` (the model's [`Grid::on_behalf_of`] of the resolved
/// change against `blame_elem`'s), after its author agreed.
const C_OBO: &str = "holder on_behalf_of compared";
/// Of those, the holders whose change an agent made for a person (`on_behalf_of` set).
const C_OBO_AGENT: &str = "holder on_behalf_of compared (an agent's)";
/// A facet holder's `restored_by` (the author of the change that put the write back, when an
/// undo, redo or revert did: the model's holder `seq` when it `restores` another, against
/// `blame_elem`'s), after its `on_behalf_of` agreed.
const C_RESTORED: &str = "holder restored_by compared";
/// Of those, the holders whose write an undo, redo or revert put back (`restored_by` set).
const C_RESTORED_SOME: &str = "holder restored_by compared (a restored write)";
/// A write --base's conflicts as an exact set (the kit's against `merge3_text`'s).
const C_CONFLICTS: &str = "write --base conflicts compared";
/// K-AUD-1's disk sync check after an outside save.
const C_SYNCS: &str = "disk syncs audited";

/// Compare each facet's holder: the change it resolves to (S2), that change's author, whom it
/// was made for (`on_behalf_of`) and who put the write back (`restored_by`), each exactly. The
/// two sides name authors alike: the model records the pid that made the change (`p0`..`p2`,
/// `a0`) or the kit's reserved pids (`kit:import`, `kit:external`), as the kit's change log
/// does, so no mapping. An agent's change is the agent's (`a0`), made on p1's behalf (the
/// model's `acts_for`, as `Kit::open` joins it), an undo --for step too. `restored_by` is the
/// author of the holder's own change when that change restores another's write (an undo, redo
/// or revert), else none; `restored_in` (that change) is not compared. `Facet::Insert` holders
/// are skipped: the kit's `blame_elem` has no insert facet, and the traces don't compare who
/// inserted a row or column. k2-mpc-base's `tests/blame.rs`,
/// `grid::blame_structure_inserted_and_moved_by`, asserts it on one fixed case only
/// (`blame_structure`'s inserting author per row: the import's and an agent's).
fn holders_differ(
    kit: &Kit,
    g: &Grid,
    facets: Vec<(String, String, model::Facet)>,
    cov: &mut BTreeMap<&'static str, u64>,
) -> Option<String> {
    let c = kit.conns["p1"];
    for (root, key, f) in facets {
        let facet = match f {
            model::Facet::Value => Facet::Value,
            model::Facet::Position => Facet::Position,
            model::Facet::Existence => Facet::Existence,
            model::Facet::Insert => continue,
        };
        let h = g.holders.get(&(root.clone(), key.clone(), f))?;
        let b = kit.s().blame_elem(c, &root, &key, facet).ok().flatten();
        let got_obo = b.as_ref().map(|b| b.on_behalf_of.clone());
        let got_restored = b.as_ref().map(|b| b.restored_by.clone());
        // S2: a holder that restores another's write was put back by its own change's author
        let want_restored = h.restores.and_then(|_| g.authors.get(&h.seq).cloned());
        let want = (h.resolved(), g.authors.get(&h.resolved()).cloned());
        let want_obo = g.on_behalf_of(h.resolved()).map(str::to_owned);
        let got = b.map(|b| (b.change, Some(b.author)));
        *cov.entry(C_HOLDERS).or_default() += 1;
        if got.as_ref().map(|x| x.0) != Some(want.0) {
            return Some(format!(
                "holder of {root}/{key}/{f:?}: kit {got:?} model {want:?}"
            ));
        }
        *cov.entry(AUTHORS_COMPARED).or_default() += 1;
        if got.as_ref().map(|x| &x.1) != Some(&want.1) {
            return Some(format!(
                "holder author of {root}/{key}/{f:?}: kit {got:?} model {want:?}"
            ));
        }
        *cov.entry(C_OBO).or_default() += 1;
        if want_obo.is_some() {
            *cov.entry(C_OBO_AGENT).or_default() += 1;
        }
        if got_obo.as_ref() != Some(&want_obo) {
            return Some(format!(
                "holder on_behalf_of of {root}/{key}/{f:?}: kit {got_obo:?} model {want_obo:?} \
                 (change {}, author {:?})",
                want.0, want.1
            ));
        }
        *cov.entry(C_RESTORED).or_default() += 1;
        if want_restored.is_some() {
            *cov.entry(C_RESTORED_SOME).or_default() += 1;
        }
        if got_restored.as_ref() != Some(&want_restored) {
            return Some(format!(
                "holder restored_by of {root}/{key}/{f:?}: kit {got_restored:?} model \
                 {want_restored:?} (change {}, put back by change {})",
                want.0, h.seq
            ));
        }
    }
    None
}

// ---------- the runs ----------

fn traces_per_class() -> u64 {
    std::env::var("GRID_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256 / CLASSES.len() as u64 + 1)
}

/// One class's share of a run.
#[derive(Debug, Default, Clone)]
struct ClassRun {
    traces: u64,
    steps: u64,
    mismatches: u64,
    cov: BTreeMap<&'static str, u64>,
}

fn run_classes(
    mutant: GridMutant,
    per_class: u64,
    stop_at_first: bool,
) -> (u64, Vec<String>, BTreeMap<&'static str, ClassRun>) {
    let threads = 4u64;
    // `GRID_CLASSES=a,b,...`: only these classes (a node shard)
    let only: Option<Vec<String>> = std::env::var("GRID_CLASSES")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    // a stale name would run nothing for that class, and a shard skips the coverage floors
    if let Some(o) = &only {
        for x in o {
            assert!(CLASSES.contains(&x.as_str()), "GRID_CLASSES: no class {x}");
        }
    }
    let jobs: Vec<(&'static str, u64)> = CLASSES
        .iter()
        .filter(|c| only.as_ref().map_or(true, |o| o.iter().any(|x| x == *c)))
        .flat_map(|c| (0..per_class).map(move |s| (*c, s)))
        .collect();
    let found = Mutex::new(Vec::new());
    let hits = Mutex::new(BTreeMap::new());
    let steps = AtomicU64::new(0);
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|sc| {
        for k in 0..threads {
            let jobs = &jobs;
            let (found, hits, steps, stop) = (&found, &hits, &steps, &stop);
            sc.spawn(move || {
                for (n, (c, s)) in jobs.iter().enumerate() {
                    if n as u64 % threads != k || stop.load(Ordering::Relaxed) {
                        continue;
                    }
                    let r = run(c, *s, mutant);
                    steps.fetch_add(r.steps as u64, Ordering::Relaxed);
                    {
                        let mut h = hits.lock().unwrap();
                        let e = h.entry(r.class).or_insert_with(ClassRun::default);
                        e.traces += 1;
                        e.steps += r.steps as u64;
                        e.mismatches += u64::from(r.mismatch.is_some());
                        for (k, n) in r.hits {
                            *e.cov.entry(k).or_default() += n;
                        }
                    }
                    if let Some(m) = r.mismatch {
                        found.lock().unwrap().push(m);
                        if stop_at_first {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    (
        steps.load(Ordering::Relaxed),
        found.into_inner().unwrap(),
        hits.into_inner().unwrap(),
    )
}

#[test]
fn grid_traces_model_equals_kit() {
    let per = traces_per_class();
    let t0 = std::time::Instant::now();
    let (steps, mismatches, hits) = run_classes(GridMutant::None, per, false);
    // the mismatches first: a failed trace reports no coverage, so a run with many would fail
    // the floors below and hide them
    println!(
        "[grid traces] {} traces ({per} per class), {steps} steps, {} mismatches, {:.1} s",
        hits.values().map(|h| h.traces).sum::<u64>(),
        mismatches.len(),
        t0.elapsed().as_secs_f64()
    );
    for (c, h) in &hits {
        println!("[grid traces]   {c:36} {}", h.traces);
    }
    for m in mismatches.iter().take(50) {
        println!("[grid traces] MISMATCH {m}");
    }
    let job = std::env::var("K2_COMPUTE_JOB").unwrap_or_default();
    for (c, h) in &hits {
        println!(
            "RUNREC {{\"job\":\"{job}\",\"run\":\"grid_traces_{c}\",\"tool\":\"grid_traces\",\"class\":\"{c}\",\"traces\":{},\"steps\":{},\"mismatches\":{},\"status\":\"{}\"}}",
            h.traces,
            h.steps,
            h.mismatches,
            if h.mismatches == 0 { "COMPLETE" } else { "MISMATCH" }
        );
    }
    assert!(mismatches.is_empty(), "{} mismatches", mismatches.len());
    // coverage: every step the spec lists, and S13, reached (a full run only: a shard's classes
    // are only some)
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for h in hits.values() {
        for (k, n) in &h.cov {
            *reached.entry(k).or_default() += n;
        }
    }
    println!("[grid traces] coverage {reached:?}");
    for (c, h) in &hits {
        println!("[grid traces] class {c} ({} traces): {:?}", h.traces, h.cov);
    }
    let authors = reached.get(AUTHORS_COMPARED).copied().unwrap_or(0);
    println!("[grid traces] holder authors compared: {authors}");
    // the floors per class (a shard checks its own classes), each unmet one printed
    let unmet = floors_unmet(&hits, std::env::var("GRID_CLASSES").is_err());
    for u in &unmet {
        println!("[grid traces] UNMET {u}");
    }
    assert!(unmet.is_empty(), "{} floors unmet", unmet.len());
    if std::env::var("GRID_CLASSES").is_err() {
        // the spec's generator line, phrase by phrase
        let spec = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SPEC-UNDO.md"),
        )
        .expect("docs/SPEC-UNDO.md");
        let line = spec
            .lines()
            .find(|l| l.contains("**Generator** (grid traces"))
            .expect("the generator line");
        let list = line
            .split("**Generator** (grid traces")
            .nth(1)
            .and_then(|l| l.split_once("):"))
            .and_then(|(_, l)| l.split("**Not in the generator:**").next())
            .expect("the generator's list");
        for phrase in list.trim().trim_end_matches('.').split(", ") {
            let p = phrase.trim().trim_start_matches("and ");
            let key = SPEC_GENERATOR
                .iter()
                .find(|(s, _)| *s == p)
                .unwrap_or_else(|| panic!("the spec's generator lists {p:?}, with no step here"));
            assert!(
                reached.get(key.1).copied().unwrap_or(0) > 0,
                "the spec's generator lists {p:?}, and no trace reached it"
            );
        }
    }
}

/// Each mutant of spec §8 (xv)–(xxvi) must disagree with the kit on some trace (a model
/// check of the oracle's own wrong variants), stopping at the first. Every mutant prints a
/// run record (`grid_mutant_<name>`: CAUGHT or SURVIVED, the traces run). Over every class
/// (no `GRID_CLASSES`: CI, and the node's dedicated mutant part) each must be caught, or the
/// test fails. A node shard (`GRID_CLASSES` set) records and doesn't fail: a mutant whose
/// classes are in another shard survives this one; the evidence note aggregates the shards
/// (each mutant caught by at least one) beside the all-class run. (xxv) is reached by the
/// class `write_with_no_identity` (an outside save recorded with no identity).
#[test]
fn grid_mutants_each_fail() {
    let per: u64 = std::env::var("GRID_MUTANT_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24);
    let shard = std::env::var("GRID_CLASSES").ok();
    let job = std::env::var("K2_COMPUTE_JOB").unwrap_or_default();
    let mut survived = Vec::new();
    for (name, m) in GridMutant::ALL {
        let (_, found, hits) = run_classes(m, per, true);
        let traces: u64 = hits.values().map(|h| h.traces).sum();
        match found.first() {
            Some(f) => println!("[grid mutants] {name}: caught after {traces} traces: {f}"),
            None => {
                println!("[grid mutants] {name}: SURVIVED {per} traces per class");
                survived.push(name);
            }
        }
        println!(
            "RUNREC {{\"job\":\"{job}\",\"run\":\"grid_mutant_{name}\",\"tool\":\"grid_traces\",\"mutant\":\"{name}\",\"classes\":\"{}\",\"traces_per_class\":{per},\"traces\":{traces},\"caught\":{},\"status\":\"{}\"}}",
            shard.as_deref().unwrap_or("all"),
            u8::from(!found.is_empty()),
            if found.is_empty() { "SURVIVED" } else { "CAUGHT" }
        );
    }
    match shard {
        Some(_) => println!(
            "[grid mutants] a shard: survivors {survived:?} recorded, not failed (each mutant \
             must be caught by at least one shard, and by the all-class run)"
        ),
        None => assert!(survived.is_empty(), "mutants survived: {survived:?}"),
    }
}

/// One trace, printed (`GRID_ONE=<class>:<seed>`, `GRID_ONE_MUTANT=<name>`): for debugging.
#[test]
fn grid_trace_one() {
    let Ok(one) = std::env::var("GRID_ONE") else {
        return;
    };
    let (c, s) = one.split_once(':').expect("class:seed");
    let class = CLASSES.iter().find(|x| **x == c).expect("a class");
    // `GRID_ONE_MUTANT=<name>`: the trace under that mutant
    let m = std::env::var("GRID_ONE_MUTANT").map_or(GridMutant::None, |n| {
        GridMutant::ALL
            .iter()
            .find(|(x, _)| *x == n)
            .expect("a mutant")
            .1
    });
    let r = run(class, s.parse().unwrap(), m);
    println!("{:?}", r.mismatch);
}

/// Regression (node job fe1b0fb0, indicative (bundle); source stated as 3910b73): a map
/// delete of an empty cell moves no byte and
/// only deletes, so v0.9.4's re-sync marker (the state vectors) left the replica holding the
/// cell; the next outside save, aligned on that replica, paired two blank rows the other way
/// round (right bytes, wrong keys and blame). Grid ingest now aligns on the live doc when it
/// renders to the synced bytes.
#[test]
fn regression_revert_after_restart_seed_7420() {
    let r = run("revert_after_restart", 7420, GridMutant::None);
    assert!(r.mismatch.is_none(), "{:?}", r.mismatch);
}
