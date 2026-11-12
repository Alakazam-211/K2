//! A live grid, generically (the view schema's grid slot, `cells` presence, cells claims, what a
//! verb wrote, a snapshot page's derived values, a field's class by prefix), on a workbook-shaped
//! doc (`tests/sheet_pack`) and the shared vector `client/test/vectors/grid_view.tsv`, which the
//! client's rows read too. Public API only.
//!
//! - Row numbers and column letters at revs 20, 28 and 29 equal the vector's, from the gap rule
//!   alone; every corner and range names the A1 the vector gives; refused corners are refused.
//! - An agent's cells after its claim and after each verb (cell and range sets, an insert, a
//!   delete, a sheet rename and add) equal the vector's; the claim's end gives the range's rows
//!   by number.
//! - Corners are dropped, never moved; a cells claim holds off other agents' writes and deletes
//!   inside it, never a person's; its start event carries the actor.
//! - A snapshot page of the cells carries the formula text of its own cells.
//! - A cell write's unit delta is classed `cell` by the schema's prefix rule.
//!
//! Planted faults caught by name (`k2_mpc_base::differential::GridMutant`): each `plant_*` row
//! runs its check under the fault and requires it to fail.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_grid_mutant, GridMutant};
use k2_mpc_base::elem::{Anchor, ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::grid::{a1, col_letters, Axis, CellRange, Cells, Corner, GridNumbers};
use k2_mpc_base::session::{
    AuditEvent, ClaimPolicy, Config, ConnId, EditOptions, Host, Participant, ParticipantKind,
    Rights, RosterEvent, Session, SessionError, SessionManager, Stay, WatchFrame, WatchOptions,
    WatchPart, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{FieldClass, ViewSchema};
use k2_mpc_base::{Edit, Key, Value};

#[path = "sheet_pack/mod.rs"]
mod sheet_pack;
use sheet_pack::{k, vector, Sheet, CELLS, ROWS, SEP, VIEW};

// ---------- the fixture ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(10, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    mgr: SessionManager,
    host: Arc<TestHost>,
}

impl Fx {
    /// Time passes and the rosters flush.
    fn tick(&self) {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-grid-view-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c);
    Fx { dir, mgr, host }
}

fn open_with(f: &Fx, view: ViewSchema) -> Arc<Session> {
    let key = DocKey {
        space: "ws".into(),
        path: "budget.sheet".into(),
    };
    f.mgr
        .open(
            Arc::new(Sheet { view }),
            &key,
            Some(vector().file.as_bytes()),
        )
        .unwrap()
}

fn open(f: &Fx) -> Arc<Session> {
    open_with(f, VIEW)
}

fn join(s: &Session, pid: &str, name: &str, kind: ParticipantKind) -> ConnId {
    let p = Participant::new(pid, name, "#6d5bd0", kind, "cli");
    s.join(p, Stay::Held).unwrap().conn
}

fn version(s: &Session) -> u64 {
    s.status().unwrap().version
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Apply rev `rev`'s step: a verb by `agent` (`grid step <rev>`, under `--base`), or an outside
/// save's ops as a raw edit by `person`.
fn step(s: &Session, agent: ConnId, person: ConnId, rev: u64) {
    let st = vector().steps.iter().find(|x| x.rev == rev).unwrap();
    let v = version(s);
    match &st.verb {
        Some(_) => {
            let r = rev.to_string();
            let a = argv(&["grid", "step", &r, "--base", &v.to_string()]);
            let out = s.run_tool(agent, ROWS, &a, b"").unwrap();
            assert_eq!(out.exit, 0, "rev {rev}: {}", out.stderr_summary);
            assert!(out.changed.is_some(), "rev {rev} changed nothing");
        }
        None => {
            s.edit_ops(person, st.edits.clone(), &EditOptions::new().if_version(v))
                .unwrap();
        }
    }
}

/// Replay every rev up to and including `upto`.
fn replay(s: &Session, agent: ConnId, person: ConnId, upto: u64) {
    for st in &vector().steps {
        if st.rev <= upto {
            step(s, agent, person, st.rev);
        }
    }
}

fn numbers(s: &Session, container: &str) -> GridNumbers {
    s.grid_numbers_at(container)
        .unwrap()
        .unwrap_or_else(|| panic!("no live container {container}"))
}

fn addr(root: &str, key: &Key) -> String {
    sheet_pack::address_of_key(root, key.as_str())
}

fn a1_of(g: &GridNumbers, c: &Corner) -> String {
    let (r, col) = g
        .cell_of(c)
        .unwrap_or_else(|| panic!("{c:?} names no cell"));
    a1(r, col)
}

fn range_a1(g: &GridNumbers, r: &CellRange) -> String {
    format!("{}:{}", a1_of(g, &r.start), a1_of(g, &r.end))
}

/// The roster's cells of `pid`, as seen by `conn`.
fn cells_of(s: &Session, conn: ConnId, pid: &str) -> Option<Cells> {
    s.roster(conn)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .and_then(|e| e.cells)
}

// ---------- numbers and letters ----------

/// The kit's numbers and letters at revs 20, 28 and 29 equal the vector's, and its blank rows.
fn check_numbers() -> Result<(), String> {
    let f = fx("numbers");
    let s = open(&f);
    let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let v = vector();
    let mut at = 20;
    for rev in [20u64, 28, 29] {
        for st in v.steps.iter().filter(|x| x.rev > at && x.rev <= rev) {
            step(&s, bot, ada, st.rev);
        }
        at = rev;
        for (r, container, axis, want) in v.numbers.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, container);
            let root = match axis {
                Axis::Rows => ROWS,
                Axis::Cols => sheet_pack::COLS,
            };
            let got: Vec<(String, String)> = g
                .keyed(*axis)
                .iter()
                .map(|(k, n)| {
                    let label = match axis {
                        Axis::Rows => n.to_string(),
                        Axis::Cols => col_letters(*n),
                    };
                    (addr(root, k), label)
                })
                .collect();
            if got != *want {
                return Err(format!(
                    "rev {r} {container} {}: got {got:?}, want {want:?}",
                    axis.as_str()
                ));
            }
        }
        for (r, container, want) in v.blank_rows.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, container);
            let keyed: Vec<u64> = g.keyed(Axis::Rows).iter().map(|(_, n)| *n).collect();
            let last = keyed.last().copied().unwrap_or(0);
            let got: Vec<u64> = (1..=last).filter(|n| !keyed.contains(n)).collect();
            if got != *want {
                return Err(format!(
                    "rev {r} {container} blank rows: got {got:?}, want {want:?}"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn numbers_and_letters_follow_the_gap_rule() {
    check_numbers().unwrap();
}

#[test]
fn plant_gap_ignored_is_caught() {
    let r = with_grid_mutant(GridMutant::GapIgnored, check_numbers);
    assert!(r.is_err(), "numbering that ignores the gap passed the rows");
}

#[test]
fn letters() {
    for (n, l) in [
        (1, "A"),
        (26, "Z"),
        (27, "AA"),
        (702, "ZZ"),
        (703, "AAA"),
        (16384, "XFD"),
    ] {
        assert_eq!(col_letters(n), l);
    }
}

// ---------- corners and A1 ----------

#[test]
fn corners_name_the_vectors_cells() {
    let f = fx("corners");
    let s = open(&f);
    let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let v = vector();
    let mut checked = 0;
    let mut at = 20;
    for rev in [20u64, 22, 23, 27] {
        for st in v.steps.iter().filter(|x| x.rev > at && x.rev <= rev) {
            step(&s, bot, ada, st.rev);
        }
        at = rev;
        for (_, id, c, active, ranges) in v.cells_a1.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, &addr(sheet_pack::SHEETS, c.container.as_ref().unwrap()));
            assert_eq!(a1_of(&g, c.active.as_ref().unwrap()), *active, "{id}");
            let got: Vec<String> = c.ranges.iter().map(|r| range_a1(&g, r)).collect();
            let got = if got.is_empty() {
                "-".to_owned()
            } else {
                got.join(",")
            };
            assert_eq!(got, *ranges, "{id}");
            // and the kit takes it as a person's cells
            let set = s.set_cells(ada, Some(c.clone())).unwrap().unwrap();
            assert_eq!(set, *c, "{id}: the cells as stored");
            checked += 1;
        }
        for (_, id, container, r, want) in v.range_a1.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, &addr(sheet_pack::SHEETS, container));
            assert_eq!(range_a1(&g, r), *want, "{id} at rev {rev}");
            checked += 1;
        }
        for (_, id, container, c, want) in v.corner_a1.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, &addr(sheet_pack::SHEETS, container));
            assert_eq!(a1_of(&g, c), *want, "{id} at rev {rev}");
            checked += 1;
        }
        for (_, container, c, why) in v.corner_refused.iter().filter(|x| x.0 == rev) {
            let g = numbers(&s, &addr(sheet_pack::SHEETS, container));
            assert_eq!(g.cell_of(c), None, "refused: {why}");
            let cells = Cells::new(container.clone(), Some(c.clone()), Vec::new());
            let e = s.set_cells(ada, Some(cells)).unwrap_err();
            assert!(
                matches!(e, SessionError::BadPosition(_)),
                "{why}: {e:?} (MPC-E114)"
            );
            checked += 1;
        }
    }
    assert!(checked >= 15, "the vector's corner rows: {checked}");
}

#[test]
fn cells_refusals_and_focus() {
    let f = fx("refusals");
    let s = open(&f);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let one = || {
        CellRange::new(sheet_pack::corner("@sheet:1/row:r3,0,@sheet:1/col:c3,0"), {
            sheet_pack::corner("@sheet:1/row:r4,0,@sheet:1/col:c4,0")
        })
    };
    // 17 ranges: MPC-E310
    let many = Cells::new(k("@sheet:1"), None, (0..17).map(|_| one()).collect());
    assert!(matches!(
        s.set_cells(ada, Some(many)).unwrap_err(),
        SessionError::SelectionTooLarge { len: 17, max: 16 }
    ));
    // a caret outside every range
    let out = Cells::new(
        k("@sheet:1"),
        Some(sheet_pack::corner("@sheet:1/row:r16,0,@sheet:1/col:c5,0")),
        vec![one()],
    );
    assert!(matches!(
        s.set_cells(ada, Some(out)).unwrap_err(),
        SessionError::BadPosition(_)
    ));
    // by address: the same value as by key; it sets the focus
    let at = Cells::at(
        "@sheet:2",
        Some(Corner::at(
            Some("@sheet:2/row:r1"),
            0,
            Some("@sheet:2/col:c1"),
            0,
        )),
        vec![CellRange::new(
            Corner::at(Some("@sheet:2/row:r1"), 0, Some("@sheet:2/col:c1"), 0),
            Corner::at(Some("@sheet:2/row:r4"), 0, Some("@sheet:2/col:c2"), 0),
        )],
    );
    let stored = s.set_cells_at(ada, Some(&at)).unwrap().unwrap();
    assert_eq!(stored.address, "@sheet:2");
    assert_eq!(
        stored.ranges[0].end.row_address.as_deref(),
        Some("@sheet:2/row:r4")
    );
    let me = |s: &Session| {
        s.roster(ada)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == "person:ada")
            .unwrap()
    };
    assert_eq!(
        me(&s).focus.map(|f| f.address),
        Some("@sheet:2".to_owned()),
        "cells set the focus"
    );
    // a focus on another container drops them
    s.set_focus_at(ada, Some("@sheet:1")).unwrap();
    assert_eq!(me(&s).cells, None, "a focus move drops the cells");
    // a watcher is refused (MPC-E305) is the kit's rule for every presence value: not repeated
}

// ---------- an agent's cells after its claim and verbs ----------

/// The agent's claim, then every rev: the agent's cells after each equal the vector's.
fn check_agent_cells() -> Result<(), String> {
    let f = fx("agent");
    let s = open(&f);
    let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let v = vector();
    let (container, range, note, hold) = v.claim.clone().unwrap();
    let info = s
        .claim_cells(bot, &container, range, Some(60_000), Some(note), hold)
        .unwrap();
    let got = cells_of(&s, ada, "agent:atlas");
    if got != v.claim_cells {
        return Err(format!(
            "after the claim: {got:?}, want {:?}",
            v.claim_cells
        ));
    }
    let roster = s.subscribe_roster(ada).unwrap();
    for st in &v.steps {
        step(&s, bot, ada, st.rev);
        f.tick();
        while roster.try_recv().is_ok() {}
        if let Some(want) = &st.agent_cells {
            let got = cells_of(&s, ada, "agent:atlas");
            if got != *want {
                return Err(format!("after rev {}: {got:?}, want {want:?}", st.rev));
            }
        }
        if let Some((first, last)) = st.claim_lines {
            s.release_claim(bot, Some(info.id)).unwrap();
            f.tick();
            let ended = std::iter::from_fn(|| roster.try_recv().ok()).find_map(|e| match e {
                RosterEvent::ClaimEnded { lines, .. } => Some(lines),
                _ => None,
            });
            let Some(l) = ended else {
                return Err("no claim_ended".into());
            };
            if (l.root.as_str(), l.first, l.last) != (ROWS, first, last) {
                return Err(format!("claim_ended lines {l:?}, want {first}..{last}"));
            }
        }
    }
    Ok(())
}

#[test]
fn an_agents_cells_follow_its_claim_and_verbs() {
    check_agent_cells().unwrap();
}

#[test]
fn a_persons_cells_are_never_moved_by_a_verb() {
    let f = fx("person");
    let s = open(&f);
    let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let caret = Cells::new(
        k("@sheet:1"),
        Some(sheet_pack::corner("@sheet:1/row:r16,0,@sheet:1/col:c5,0")),
        Vec::new(),
    );
    s.set_cells(ada, Some(caret.clone())).unwrap();
    replay(&s, bot, ada, 25);
    assert_eq!(cells_of(&s, ada, "person:ada"), Some(caret));
}

// ---------- dropped, never moved ----------

/// A range grows with an insert inside it (no change to its corners); a range whose corner's
/// row is deleted goes, its caret staying; a range whose form an insert breaks goes.
fn check_corners_drop() -> Result<(), String> {
    let f = fx("drop");
    let s = open(&f);
    let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let sam = join(&s, "person:sam", "Sam", ParticipantKind::Person);
    let c = sheet_pack::corner;
    // grows: C4:D15, then the rev 23 insert inside it
    let grows = Cells::new(
        k("@sheet:1"),
        None,
        vec![CellRange::new(
            c("@sheet:1/row:r3,0,@sheet:1/col:c3,0"),
            c("@sheet:1/row:r14,0,@sheet:1/col:c4,0"),
        )],
    );
    s.set_cells(ada, Some(grows.clone())).unwrap();
    // broken form: B16:C16 (blank row 16), then a row inserted at 16
    let blank = Cells::new(
        k("@sheet:1"),
        None,
        vec![CellRange::new(
            c("@sheet:1/row:r14,1,@sheet:1/col:c2,0"),
            c("@sheet:1/row:r14,1,@sheet:1/col:c3,0"),
        )],
    );
    s.set_cells(sam, Some(blank)).unwrap();
    replay(&s, bot, ada, 23);
    if cells_of(&s, ada, "person:ada") != Some(grows) {
        return Err("an insert inside a range changed it".into());
    }
    let v = version(&s);
    s.edit_ops(
        bot,
        vec![
            Edit::List {
                root: ROWS.into(),
                ops: vec![ListOp::Insert {
                    key: k("@sheet:1/row:r20"),
                    at: Anchor::After(k("@sheet:1/row:r14")),
                    value: Value::str("row"),
                }],
            },
            Edit::Map {
                root: CELLS.into(),
                ops: vec![
                    MapOp::Set {
                        key: format!("s1-r20{SEP}@sheet"),
                        value: Value::str("1"),
                    },
                    MapOp::Delete {
                        key: format!("s1-r15{SEP}@gap"),
                    },
                ],
            },
        ],
        &EditOptions::new().base(v),
    )
    .map_err(|e| format!("the insert: {e}"))?;
    if let Some(got) = cells_of(&s, ada, "person:sam") {
        return Err(format!("a range whose form broke was kept: {got:?}"));
    }
    // a corner's row deleted: the range goes, the caret stays
    let keep = Cells::new(
        k("@sheet:1"),
        Some(c("@sheet:1/row:r9,0,@sheet:1/col:c3,0")),
        vec![CellRange::new(
            c("@sheet:1/row:r9,0,@sheet:1/col:c3,0"),
            c("@sheet:1/row:r13,0,@sheet:1/col:c4,0"),
        )],
    );
    s.set_cells(sam, Some(keep)).unwrap();
    let v = version(&s);
    s.edit_ops(
        bot,
        vec![Edit::List {
            root: ROWS.into(),
            ops: vec![ListOp::Delete {
                key: k("@sheet:1/row:r13"),
            }],
        }],
        &EditOptions::new().base(v),
    )
    .map_err(|e| format!("the delete: {e}"))?;
    let got = cells_of(&s, ada, "person:sam").ok_or("the caret went too")?;
    if !got.ranges.is_empty() || got.active != Some(c("@sheet:1/row:r9,0,@sheet:1/col:c3,0")) {
        return Err(format!("after the delete: {got:?}"));
    }
    Ok(())
}

#[test]
fn corners_are_dropped_never_moved() {
    check_corners_drop().unwrap();
}

#[test]
fn plant_corners_move_not_drop_is_caught() {
    let r = with_grid_mutant(GridMutant::CornersMoveNotDrop, check_corners_drop);
    assert!(
        r.is_err(),
        "corners moved instead of dropped passed the rows"
    );
}

// ---------- cells claims ----------

#[test]
fn a_cells_claim_holds_off_other_agents_not_people() {
    let f = fx("claims");
    let s = open(&f);
    let atlas = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let brook = join(&s, "agent:brook", "Brook", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let (container, range, note, _) = vector().claim.clone().unwrap();
    s.claim_cells(
        atlas,
        &container,
        range.clone(),
        Some(60_000),
        Some(note),
        true,
    )
    .unwrap();
    // another agent's claim meeting it is refused
    assert!(matches!(
        s.claim_cells(brook, &container, range, None, None, false)
            .unwrap_err(),
        SessionError::ElemsClaimed(_)
    ));
    let respect = |s: &Session| {
        EditOptions::new()
            .policy(ClaimPolicy::Respect)
            .base(version(s))
    };
    let cell = |row: &str, col: &str, v: &str| {
        vec![Edit::Map {
            root: CELLS.into(),
            ops: vec![MapOp::Set {
                key: format!("{}{SEP}{col}", k(row)),
                value: Value::str(v),
            }],
        }]
    };
    // D7 (r5, c4) is inside: refused; E7 (c5) outside: applied
    let e = s
        .edit_ops(brook, cell("@sheet:1/row:r5", "c4", "9900"), &respect(&s))
        .unwrap_err();
    assert!(matches!(e, SessionError::ElemsClaimed(_)), "{e:?}");
    s.edit_ops(brook, cell("@sheet:1/row:r5", "c5", "1"), &respect(&s))
        .unwrap();
    // a delete of row 9 (r8), inside: refused
    let del = vec![Edit::List {
        root: ROWS.into(),
        ops: vec![ListOp::Delete {
            key: k("@sheet:1/row:r8"),
        }],
    }];
    assert!(matches!(
        s.edit_ops(brook, del, &respect(&s)).unwrap_err(),
        SessionError::ElemsClaimed(_)
    ));
    // a row field (hidden) is not a cell: applied
    s.edit_ops(brook, cell("@sheet:1/row:r8", "@hidden", "1"), &respect(&s))
        .unwrap();
    // a person is never refused
    s.edit_ops(
        ada,
        cell("@sheet:1/row:r5", "c4", "1"),
        &EditOptions::new().policy(ClaimPolicy::Respect),
    )
    .unwrap();
    // its owner's write inside doesn't end a held claim
    s.edit_ops(atlas, cell("@sheet:1/row:r6", "c4", "2"), &respect(&s))
        .unwrap();
    let held = s
        .roster(ada)
        .unwrap()
        .into_iter()
        .find(|e| e.participant.pid == "agent:atlas")
        .unwrap()
        .claims;
    assert_eq!(held.len(), 1, "the held claim stays");
}

/// The claim's start event carries the actor: its name, kind and colour.
fn check_claim_actor() -> Result<(), String> {
    let f = fx("actor");
    let s = open(&f);
    let atlas = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let roster = s.subscribe_roster(ada).unwrap();
    let (container, range, note, hold) = vector().claim.clone().unwrap();
    s.claim_cells(atlas, &container, range, None, Some(note), hold)
        .unwrap();
    f.tick();
    let got = std::iter::from_fn(|| roster.try_recv().ok()).find_map(|e| match e {
        RosterEvent::ClaimStartedCells { actor, address, .. } => Some((actor, address)),
        _ => None,
    });
    let Some((actor, address)) = got else {
        return Err("no claim_started_cells".into());
    };
    if address != "@sheet:1" {
        return Err(format!("container {address}"));
    }
    if (actor.name.as_str(), actor.kind, actor.colour.as_str())
        != ("Atlas", ParticipantKind::Agent, "#6d5bd0")
    {
        return Err(format!("actor {actor:?}"));
    }
    Ok(())
}

#[test]
fn a_cells_claims_start_carries_the_actor() {
    check_claim_actor().unwrap();
}

#[test]
fn plant_claim_cells_actor_missing_is_caught() {
    let r = with_grid_mutant(GridMutant::ClaimCellsActorMissing, check_claim_actor);
    assert!(r.is_err(), "a claim event with no actor passed the rows");
}

// ---------- the snapshot's pages and the view deltas ----------

fn listed() -> WatchOptions {
    WatchOptions::new(&[KEYED_WATCH_FEATURE])
}

/// Every formula cell's text is on the page that holds its entry, and only there.
#[test]
fn a_cells_page_carries_its_formula_text() {
    let f = fx("pages");
    let s = open(&f);
    let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let frames = s.watch_snapshot(&mut w).unwrap();
    let mut found: Vec<(String, String, String)> = Vec::new();
    let mut pages = 0;
    for fr in &frames {
        let WatchFrame::Page(p) = fr else { continue };
        if p.root != CELLS {
            assert!(p.derived.is_empty(), "derived on a {} page", p.root);
            continue;
        }
        pages += 1;
        let WatchPart::Entries(es) = &p.part else {
            panic!("a cells page holds entries");
        };
        for (name, unit, field, text) in &p.derived {
            assert_eq!(*name, "a1");
            assert!(
                es.iter()
                    .any(|(a, f, _)| a == unit && f.as_deref() == Some(field.as_str())),
                "{unit} {field}: its text is on a page without its entry"
            );
            found.push((unit.clone(), field.clone(), text.clone()));
        }
        // the frame's JSON carries it as derived.a1.<unit>.<field>
        if !p.derived.is_empty() {
            let j = fr.to_json();
            assert!(j.contains("\"derived\":{\"a1\":{"), "{j}");
        }
    }
    assert!(pages >= 1);
    let mut want: Vec<(String, String, String)> = vector()
        .a1
        .iter()
        .map(|(u, f, t)| (addr(ROWS, u), f.clone(), t.clone()))
        .collect();
    want.sort();
    found.sort();
    assert_eq!(found, want, "every formula cell's text, once");
    assert!(want.len() >= 10, "the vector's formula cells");
}

/// A cell write is classed `cell` by the schema's prefix rule (and `props` without one).
#[test]
fn a_cell_write_is_classed_cell_by_prefix() {
    assert_eq!(VIEW.class_of_field("c4"), Some(FieldClass::Cell));
    assert_eq!(VIEW.class_of_field("@hidden"), Some(FieldClass::Visibility));
    for (view, want) in [
        (VIEW, "cell"),
        (
            ViewSchema {
                field_class_prefix: &[],
                ..VIEW
            },
            "props",
        ),
    ] {
        let f = fx("class");
        let s = open_with(&f, view);
        let bot = join(&s, "agent:atlas", "Atlas", ParticipantKind::Agent);
        let ada = join(&s, "person:ada", "Ada", ParticipantKind::Person);
        let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
        let head = s.watch_snapshot(&mut w).unwrap()[0].to_json();
        assert!(
            head.contains("\"grid\":{\"rows\":\"rows\"")
                && head.contains("\"cell_derived\":[\"a1\"]"),
            "the snapshot's schema carries its grid slot: {head}"
        );
        assert_eq!(
            head.contains("\"field_class_prefix\":[[\"c\",\"cell\"]]"),
            want == "cell",
            "the prefix rule is in the schema when declared"
        );
        step(&s, bot, ada, 22);
        let fs = w.try_frames();
        let ch = fs
            .iter()
            .find(|x| matches!(x, WatchFrame::Change(_)))
            .expect("a change frame");
        let Json::Obj(m) = ch.to_value() else {
            panic!("an object")
        };
        let units = m.iter().find(|(k, _)| k == "units").unwrap().1.to_json();
        assert!(
            units.contains(&format!("\"ops\":[\"{want}\"]")),
            "{want}: {units}"
        );
    }
}
