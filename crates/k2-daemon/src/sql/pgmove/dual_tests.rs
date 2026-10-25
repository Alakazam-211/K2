//! The live dual-run machine over fakes (prd-app-pg-choice-dualrun-v1 §12
//! unit list): `--check --live` refusals change nothing, setup and its
//! undo, the watch loop, every cut-over step and its undo, boot recovery,
//! orphans and `--prepare-live`. No Postgres, no helper.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Value};

use super::super::dual::*;
use super::*;
use k2_root_proto::pg_dual::DualOp;

// ── fakes ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct DualState {
    log: Vec<String>,
    /// Status answers, served in order; the last one repeats.
    statuses: Vec<Value>,
    served: usize,
    fail: Vec<&'static str>,
    passwords: Vec<String>,
    forced: Vec<u32>,
    /// The fake cluster's connection modes (linked to [`FakeOps`]): the
    /// owner's refresh is refused unless the target is open, and a
    /// teardown logs a closed source.
    conn: Option<Rc<RefCell<HashMap<(Major, String), String>>>>,
}

struct FakeDual(RefCell<DualState>);

impl FakeDual {
    fn new() -> Self {
        FakeDual(RefCell::new(DualState { forced: vec![16500], ..Default::default() }))
    }
    fn log(&self) -> Vec<String> {
        self.0.borrow().log.clone()
    }
    fn did(&self, what: &str) -> bool {
        self.log().iter().any(|l| l.starts_with(what))
    }
    fn gate(&self, op: &'static str) -> Result<(), String> {
        if self.0.borrow().fail.contains(&op) {
            Err(format!("{op} failed (injected)"))
        } else {
            Ok(())
        }
    }
    fn push(&self, s: String) {
        self.0.borrow_mut().log.push(s);
    }
    fn link(&self, ops: &FakeOps) {
        self.0.borrow_mut().conn = Some(ops.0.borrow().conn.clone());
    }
    fn mode(&self, m: Major, db: &str) -> Option<String> {
        self.0.borrow().conn.as_ref().and_then(|c| c.borrow().get(&(m, db.to_string())).cloned())
    }
    fn statuses(&self, v: Vec<Value>) {
        let mut s = self.0.borrow_mut();
        s.statuses = v;
        s.served = 0;
    }
}

impl DualOps for FakeDual {
    fn ready(&self) -> Result<(), String> {
        self.gate("ready")
    }
    fn logical_conf(&self, m: Major, wal_keep_mb: Option<u32>, if_logical: bool) -> Result<Value, String> {
        self.push(format!("logical_conf {m} {wal_keep_mb:?} if_logical={if_logical}"));
        self.gate("logical_conf")?;
        let wal = if if_logical { "logical" } else { "replica" };
        Ok(json!({"walLevel": wal, "changed": true}))
    }
    fn dump_schema(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String> {
        self.push(format!("dump_schema {from} {to} {db} {tag}"));
        self.gate("dump_schema")?;
        Ok(1024)
    }
    fn restore_keep_force_off(&self, to: Major, db: &str, _tag: &str, role: &str, _pw: &str) -> Result<Vec<u32>, String> {
        self.push(format!("restore_keep_force_off {to} {db} as {role}"));
        self.gate("restore")?;
        Ok(self.0.borrow().forced.clone())
    }
    fn force_rls(&self, m: Major, db: &str, oids: &[u32]) -> Result<(), String> {
        self.push(format!("force_rls {m} {db} {oids:?}"));
        self.gate("force_rls")
    }
    fn dual(&self, op: DualOp, from: Major, to: Major, db: &str, id: &str, password: Option<&str>) -> Result<Value, String> {
        if op != DualOp::Status {
            let closed = if op == DualOp::Teardown && self.mode(from, db).as_deref() == Some("off") { " (source closed)" } else { "" };
            self.push(format!("dual {} {from} {to} {db} {id}{closed}", op.token()));
        }
        if let Some(pw) = password {
            self.0.borrow_mut().passwords.push(pw.to_string());
        }
        match op {
            DualOp::Status => {
                self.gate("status")?;
                let mut s = self.0.borrow_mut();
                let i = s.served.min(s.statuses.len().saturating_sub(1));
                s.served += 1;
                s.statuses.get(i).cloned().ok_or_else(|| "no status scripted".to_string())
            }
            DualOp::SetupSource => self.gate("setup").map(|_| json!({})),
            DualOp::Subscribe => self.gate("subscribe").map(|_| json!({})),
            DualOp::Detach => self.gate("detach").map(|_| json!({})),
            DualOp::Teardown => self.gate("teardown").map(|_| json!({"roleDropped": true})),
        }
    }
    fn restart_cluster(&self, m: Major) -> Result<(), String> {
        self.push(format!("restart {m}"));
        self.gate("restart")
    }
    fn refresh_matviews(&self, m: Major, port: u16, db: &str, role: &str, _pw: &str, timeout_secs: u64) -> Result<(), String> {
        self.push(format!("refresh {m} {port} {db} as {role} within {timeout_secs}s"));
        if let Some(mode) = self.mode(m, db) {
            return Err(format!("FATAL: {db} is not accepting connections from {role} ({mode})"));
        }
        self.gate("refresh")
    }
}

#[derive(Default)]
struct StoreState {
    runs: Vec<DualRun>,
    moves: Vec<(String, MoveState)>,
    /// The move worker is held elsewhere (a switch, an abort).
    busy: bool,
    holds: u32,
}

struct FakeStore(RefCell<StoreState>);

impl FakeStore {
    fn new() -> Self {
        FakeStore(RefCell::new(StoreState::default()))
    }
    fn only(&self) -> DualRun {
        let s = self.0.borrow();
        assert_eq!(s.runs.len(), 1, "{:?}", s.runs.iter().map(|r| (&r.id, r.state)).collect::<Vec<_>>());
        s.runs[0].clone()
    }
}

impl RunStore for FakeStore {
    fn open_run(&self, db: &str) -> Option<DualRun> {
        self.0.borrow().runs.iter().rev().find(|r| r.db == db && r.state.open()).cloned()
    }
    fn open_runs(&self) -> Vec<DualRun> {
        self.0.borrow().runs.iter().filter(|r| r.state.open()).cloned().collect()
    }
    fn last_run(&self, db: &str) -> Option<DualRun> {
        self.0.borrow().runs.iter().rev().find(|r| r.db == db).cloned()
    }
    fn run_by_object(&self, object_id: &str) -> Option<DualRun> {
        self.0.borrow().runs.iter().rev().find(|r| r.objects.object_id == object_id).cloned()
    }
    fn insert_run(&self, r: &DualRun) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.runs.iter().any(|x| x.database_id == r.database_id && x.state.open()) {
            return Err("UNIQUE constraint failed: idx_sql_dual_runs_open".into());
        }
        s.runs.push(r.clone());
        Ok(())
    }
    fn save_run(&self, r: &DualRun) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        let slot = s.runs.iter_mut().find(|x| x.id == r.id).ok_or("no such run")?;
        *slot = r.clone();
        Ok(())
    }
    fn new_password(&self) -> Result<String, String> {
        Ok("A".repeat(43))
    }
    fn database_id(&self, _db: &str) -> Result<String, String> {
        Ok("9f1c2b3a-4d5e-4f60-8a71-b2c3d4e5f607".into())
    }
    fn move_state(&self, move_id: &str) -> Option<MoveState> {
        self.0.borrow().moves.iter().find(|(m, _)| m == move_id).map(|(_, s)| *s)
    }
    fn hold_worker(&self, db: &str) -> Result<Box<dyn std::any::Any>, String> {
        let mut s = self.0.borrow_mut();
        if s.busy {
            return Err(format!("busy: a move of {db} is running"));
        }
        s.holds += 1;
        Ok(Box::new(()))
    }
}

const LIVE_FACTS: &str = r#"{"noPk":[],"largeObjects":0,"unlogged":[],"foreign":0,"matviews":1,"partitioned":0,"generated":0}"#;

fn live_ops() -> FakeOps {
    let ops = FakeOps::new();
    {
        let mut s = ops.0.borrow_mut();
        s.answers.push(("SHOW wal_level".into(), "logical".into()));
        s.answers.push(("'noPk'".into(), LIVE_FACTS.into()));
        s.answers.push(("json_build_array(schemaname, sequencename".into(), r#"[["public","jobs_id_seq",42,1]]"#.into()));
        s.answers.push(("pg_catalog.pg_largeobject_metadata".into(), "0".into()));
        s.installed.push(17);
    }
    ops
}

fn lo() -> LiveOpts {
    LiveOpts { base: opts(), replica_identity_full: false, dual_ok: true }
}

fn store_env() -> (FakeStore, FakeEnv) {
    (FakeStore::new(), FakeEnv::new())
}

fn set_facts(ops: &FakeOps, facts: Value) {
    let mut s = ops.0.borrow_mut();
    s.answers.retain(|(n, _)| n != "'noPk'");
    s.answers.push(("'noPk'".into(), facts.to_string()));
}

/// The objects' id comes from the database's K2 id (PA23): 12 hex.
#[test]
fn object_ids_and_the_wal_cap() {
    assert_eq!(object_id("9f1c2b3a-4d5e-4f60-8a71-b2c3d4e5f607"), "9f1c2b3a4d5e");
    let short = object_id("d1");
    assert!(k2_root_proto::pg_dual::dual_id_ok(&short), "{short}");
    assert_eq!(short, object_id("d1"), "stable");
    // min(8 GB, 25% of free), never under the helper's 64 MB floor.
    assert_eq!(wal_keep_mb(100 << 30), 8192);
    assert_eq!(wal_keep_mb(4 << 30), 1024);
    assert_eq!(wal_keep_mb(1 << 20), 64);
}

// ── --check --live (PA19, PA20) ─────────────────────────────────────────

fn refused_ids(r: &check::Report) -> Vec<String> {
    r.lines.iter().filter(|l| l.level == check::Level::Refuse).map(|l| l.id.clone()).collect()
}

#[test]
fn check_live_passes_a_good_database_and_warns() {
    let ops = live_ops();
    let (store, env) = store_env();
    let rec = env.record("ws_jobs").unwrap();
    let (r, no_pk) = check_live(&ops, &env, &store, &rec, &lo()).unwrap();
    assert!(!r.refused(), "{:?}", r.lines);
    assert!(no_pk.is_empty());
    assert!(r.lines.iter().any(|l| l.id == "matviews" && l.level == check::Level::Warn));
    assert!(r.lines.iter().any(|l| l.id == "copy" && l.hint.as_deref().unwrap_or_default().contains("temp tables")));
    assert!(!r.lines.iter().any(|l| l.text.starts_with("Apps stopped and restarted")), "the copy path's line goes");
    assert!(ops.log().iter().all(|l| !MUTATING.iter().any(|m| l.starts_with(m))), "read-only: {:?}", ops.log());
}

#[test]
fn every_live_refusal_names_its_objects_and_changes_nothing() {
    let cases: Vec<(&str, Box<dyn Fn(&FakeOps, &mut LiveOpts, &FakeStore)>)> = vec![
        ("helper", Box::new(|_, o, _| o.dual_ok = false)),
        (SOURCE_NOT_LOGICAL, Box::new(|ops, _, _| {
            let mut s = ops.0.borrow_mut();
            s.answers.retain(|(n, _)| n != "SHOW wal_level");
            s.answers.insert(0, ("SHOW wal_level".into(), "replica".into()));
        })),
        ("no_primary_key", Box::new(|ops, _, _| set_facts(ops, json!({"noPk": ["public.log", "public.events"], "largeObjects": 0, "unlogged": [], "foreign": 0})))),
        ("large_objects", Box::new(|ops, _, _| set_facts(ops, json!({"noPk": [], "largeObjects": 3, "unlogged": [], "foreign": 0})))),
        ("unlogged", Box::new(|ops, _, _| set_facts(ops, json!({"noPk": [], "largeObjects": 0, "unlogged": ["public.cache"], "foreign": 0})))),
        ("foreign_tables", Box::new(|ops, _, _| set_facts(ops, json!({"noPk": [], "largeObjects": 0, "unlogged": [], "foreign": 1})))),
        ("disk", Box::new(|ops, _, _| ops.0.borrow_mut().free = 1 << 30)),
        (DUAL_RUNNING, Box::new(|_, _, store| {
            store.0.borrow_mut().runs.push(DualRun {
                id: "dr_x".into(), database_id: "9f1c2b3a-4d5e-4f60-8a71-b2c3d4e5f607".into(), db: "ws_jobs".into(), from: 16, to: 17,
                state: RunState::Streaming, objects: Objects::default(), replica_identity_full: vec![], lag: Lag::default(), move_id: None,
                actor: "owner".into(), started_at: NOW, streaming_at: None, finished_at: None, last_error: None,
            })
        })),
    ];
    for (id, setup) in cases {
        let ops = live_ops();
        let (store, env) = store_env();
        let mut o = lo();
        setup(&ops, &mut o, &store);
        let rec = env.record("ws_jobs").unwrap();
        let (r, _) = check_live(&ops, &env, &store, &rec, &o).unwrap();
        assert!(refused_ids(&r).iter().any(|x| x == id), "{id}: {:?}", r.lines);
        let dops = FakeDual::new();
        let before = store.0.borrow().runs.len();
        let e = start(&ops, &dops, &env, &store, "ws_jobs", &o).unwrap_err();
        assert!(e.starts_with("refused:") && e.contains(id), "{id}: {e}");
        assert!(dops.log().is_empty(), "{id}: nothing ran on the helper: {:?}", dops.log());
        assert!(mutated(&ops).is_empty(), "{id}: {:?}", mutated(&ops));
        assert_eq!(store.0.borrow().runs.len(), before, "{id}: no run row");
    }
    // The names appear in the no-PK refusal; the owner's opt-in turns it into a warning.
    let ops = live_ops();
    set_facts(&ops, json!({"noPk": ["public.log"], "largeObjects": 0, "unlogged": [], "foreign": 0}));
    let (store, env) = store_env();
    let rec = env.record("ws_jobs").unwrap();
    let (r, _) = check_live(&ops, &env, &store, &rec, &lo()).unwrap();
    assert!(r.lines.iter().any(|l| l.id == "no_primary_key" && l.text.contains("public.log")));
    let (r, no_pk) = check_live(&ops, &env, &store, &rec, &LiveOpts { replica_identity_full: true, ..lo() }).unwrap();
    assert!(!r.refused(), "{:?}", r.lines);
    assert_eq!(no_pk, vec!["public.log".to_string()]);
}

// ── setup and its undo (PA21–PA28, PA34) ────────────────────────────────

#[test]
fn start_sets_up_in_order_and_leaves_the_run_syncing() {
    let ops = live_ops();
    let dops = FakeDual::new();
    let (store, env) = store_env();
    let run = start(&ops, &dops, &env, &store, "ws_jobs", &lo()).unwrap();
    assert_eq!(run.state, RunState::Syncing);
    assert_eq!(run.objects.object_id, "9f1c2b3a4d5e");
    assert_eq!(run.objects.forced_oids, vec![16500]);
    assert!(run.objects.target_created && run.objects.source_setup && run.objects.subscribed && !run.objects.detached);
    assert_eq!(run.objects.roles_created, vec!["ws_jobs_agent".to_string(), "ws_jobs_bind".into(), "ws_jobs_migrator".into()]);
    let d = dops.log();
    let order = ["logical_conf 16 Some(", "dump_schema 16 17 ws_jobs", "restore_keep_force_off 17 ws_jobs as ws_jobs_migrator", "dual setup-source 16 17 ws_jobs 9f1c2b3a4d5e", "dual subscribe 16 17 ws_jobs 9f1c2b3a4d5e"];
    let mut last = 0;
    for w in order {
        let i = d.iter().position(|l| l.starts_with(w)).unwrap_or_else(|| panic!("{w}: {d:?}"));
        assert!(i >= last, "{w} out of order: {d:?}");
        last = i;
    }
    assert!(d[0].ends_with("if_logical=true"), "a live start never changes wal_level (L25): {d:?}");
    let o = ops.log();
    assert!(o.iter().any(|l| l == "ensure_cluster 17 5433"), "{o:?}");
    assert!(o.iter().position(|l| l.contains("CREATE DATABASE")).unwrap() < o.iter().position(|l| l == "allow_conn 17 ws_jobs quiesce").unwrap());
    assert!(ops.did("remove_dump ws_jobs"), "the schema dump is removed");
    assert!(!ops.did("allow_conn 16"), "the source is never paused at setup");
    // One password, made by the daemon, on stdin twice; never stored.
    let pws = dops.0.borrow().passwords.clone();
    assert_eq!(pws.len(), 2);
    assert_eq!(pws[0], pws[1]);
    let row = serde_json::to_string(&store.only()).unwrap();
    assert!(!row.contains(&pws[0]), "the run row never holds the password");
    assert!(env.0.borrow().audits.iter().any(|(a, l)| a == "db-upgrade-live-start" && l["to"] == 17));
    assert!(env.0.borrow().inbox.iter().any(|(t, _)| t.contains("live move started")));
    // A second start: refused (one open run per database).
    let e = start(&ops, &dops, &env, &store, "ws_jobs", &lo()).unwrap_err();
    assert!(e.contains(DUAL_RUNNING), "{e}");
}

#[test]
fn a_setup_failure_drops_everything_and_records_aborted() {
    for step in ["logical_conf", "dump_schema", "restore", "setup", "subscribe"] {
        let ops = live_ops();
        let dops = FakeDual::new();
        dops.0.borrow_mut().fail.push(step);
        let (store, env) = store_env();
        let e = start(&ops, &dops, &env, &store, "ws_jobs", &lo()).unwrap_err();
        assert!(e.starts_with("setup_failed"), "{step}: {e}");
        let run = store.only();
        assert_eq!(run.state, RunState::Aborted, "{step}: {:?}", run.last_error);
        let created = matches!(step, "restore" | "setup" | "subscribe");
        assert_eq!(ops.did("drop_db 17 ws_jobs"), created, "{step}: drop only what was created: {:?}", ops.log());
        let made_source = matches!(step, "setup" | "subscribe");
        assert_eq!(dops.did("dual teardown"), made_source, "{step}: {:?}", dops.log());
        assert!(!run.objects.target_created, "{step}");
        // 0.46.4 drill: the roles it made on 17 go (they blocked a later run).
        let dropped = ops.0.borrow().full_sql.iter().any(|(m, d, s)| *m == 17 && d == "postgres" && s.contains("DROP ROLE IF EXISTS \"ws_jobs_migrator\""));
        assert_eq!(dropped, step != "logical_conf", "{step}");
        assert!(run.objects.roles_created.is_empty(), "{step}");
        assert!(!ops.did("allow_conn 16"), "{step}: the source was never paused");
        assert!(env.0.borrow().audits.iter().any(|(a, _)| a == "db-upgrade-live-abort"), "{step}");
    }
}

/// Q10: the owner's opt-in sets REPLICA IDENTITY FULL on exactly the
/// listed tables before the publication, and an abort puts it back.
#[test]
fn replica_identity_full_is_set_then_put_back_on_abort() {
    let ops = live_ops();
    set_facts(&ops, json!({"noPk": ["public.log"], "largeObjects": 0, "unlogged": [], "foreign": 0}));
    let dops = FakeDual::new();
    let (store, env) = store_env();
    let run = start(&ops, &dops, &env, &store, "ws_jobs", &LiveOpts { replica_identity_full: true, ..lo() }).unwrap();
    assert_eq!(run.replica_identity_full, vec!["public.log".to_string()]);
    let sql = ops.0.borrow().full_sql.clone();
    assert!(sql.iter().any(|(m, _, s)| *m == 16 && s == "ALTER TABLE \"public\".\"log\" REPLICA IDENTITY FULL;\n"));
    let r = abort_run(&ops, &dops, &env, &store, run, "aborted by owner", false);
    assert_eq!(r.state, RunState::Aborted);
    assert!(ops.0.borrow().full_sql.iter().any(|(m, _, s)| *m == 16 && s.contains("REPLICA IDENTITY DEFAULT")));
}

// ── the watch loop (PA29, PA30) ─────────────────────────────────────────

fn streaming_status(lag: u64, lsn: &str) -> Value {
    json!({
        "source": {"walLevel": "logical", "currentLsn": lsn, "insertLsn": lsn, "walsenders": 1, "publication": true,
                   "slot": {"active": true, "confirmedFlushLsn": lsn, "walStatus": "reserved", "lagBytes": lag}},
        "target": {"subscription": true, "enabled": true, "tablesTotal": 3, "tablesReady": 3, "applyErrors": 0, "syncErrors": 0}
    })
}

fn started() -> (FakeOps, FakeDual, FakeStore, FakeEnv, DualRun) {
    let ops = live_ops();
    let dops = FakeDual::new();
    dops.link(&ops);
    let (store, env) = store_env();
    let run = start(&ops, &dops, &env, &store, "ws_jobs", &lo()).unwrap();
    assert_eq!(dops.mode(17, "ws_jobs").as_deref(), Some("quiesce"), "the target stays quiesced while it streams");
    env.0.borrow_mut().inbox.clear();
    (ops, dops, store, env, run)
}

#[test]
fn the_watch_loop_moves_a_run_to_streaming_and_tells_once() {
    let (ops, dops, store, env, run) = started();
    let mut syncing = streaming_status(1 << 20, "0/16B3748");
    syncing["target"]["tablesReady"] = json!(1);
    dops.statuses(vec![syncing]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Syncing);
    assert_eq!((run.lag.tables_ready, run.lag.tables_total), (1, 3));
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Streaming);
    assert!(run.streaming_at.is_some());
    let run = tick(&ops, &dops, &env, &store, run);
    let ready: Vec<_> = env.0.borrow().inbox.iter().filter(|(t, _)| t.contains("ready to switch")).cloned().collect();
    assert_eq!(ready.len(), 1, "told once");
    assert_eq!(store.only().state, RunState::Streaming);
    assert!(worker::busy().is_none(), "a streaming run never holds the move worker (L5)");
    assert_eq!(run.json()["tables"]["ready"], 3);
}

#[test]
fn a_disabled_subscription_stalls_a_lost_slot_fails_and_day_seven_aborts() {
    // Disabled by an apply error: stalled, told once, nothing dropped.
    let (ops, dops, store, env, run) = started();
    let mut st = streaming_status(0, "0/1");
    st["target"]["enabled"] = json!(false);
    st["target"]["applyErrors"] = json!(2);
    dops.statuses(vec![st]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Stalled);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(env.0.borrow().inbox.iter().filter(|(t, b)| t.contains("stalled") && b.contains("journalctl")).count(), 1);
    assert!(!dops.did("dual teardown") && !ops.did("drop_db"), "a stall never drops");
    drop(run);
    // A lost slot: failed and cleaned; the source never touched.
    let (ops, dops, store, env, run) = started();
    let mut st = streaming_status(0, "0/1");
    st["source"]["slot"]["walStatus"] = json!("lost");
    dops.statuses(vec![st]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Failed);
    assert!(dops.did("dual teardown") && ops.did("drop_db 17 ws_jobs"));
    assert!(!ops.did("allow_conn 16"), "the source is untouched");
    // Day 3 warns once; day 7 aborts.
    let (ops, dops, store, env, run) = started();
    dops.statuses(vec![streaming_status(0, "0/1")]);
    env.0.borrow_mut().now = NOW + 3 * 86_400 + 5;
    let run = tick(&ops, &dops, &env, &store, run);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(env.0.borrow().inbox.iter().filter(|(t, _)| t.contains("day 3 of 7")).count(), 1);
    env.0.borrow_mut().now = NOW + 7 * 86_400;
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Aborted);
    assert!(run.last_error.as_deref().unwrap_or_default().contains("7-day"));
    assert!(dops.did("dual teardown") && ops.did("drop_db 17 ws_jobs"));
}

/// The watch loop never overwrites an abort (or a switch) that moved the
/// run while it was looking: a stale read loses.
#[test]
fn a_tick_with_a_stale_read_never_overwrites_an_abort() {
    let (ops, dops, store, env, stale) = started();
    let mut aborted = stale.clone();
    aborted.state = RunState::Aborted;
    store.save_run(&aborted).unwrap();
    dops.statuses(vec![streaming_status(0, "0/1")]);
    let r = tick(&ops, &dops, &env, &store, stale.clone());
    assert_eq!(r.state, RunState::Aborted);
    assert_eq!(store.only().state, RunState::Aborted, "the abort stands");
    // And when the abort lands between the read and the write.
    let mut s2 = stale;
    s2.state = RunState::Syncing;
    store.save_run(&s2).unwrap();
    let mut newer = s2.clone();
    newer.state = RunState::Streaming;
    assert!(!store.save_run_if(&newer, RunState::Stalled).unwrap(), "expected Stalled, the row is Syncing");
    assert!(store.save_run_if(&newer, RunState::Syncing).unwrap());
    newer.state = RunState::Syncing;
    store.save_run(&newer).unwrap();
    assert_eq!(store.only().state, RunState::Syncing, "nothing written");
}

#[test]
fn lag_is_told_once_per_episode() {
    let (ops, dops, store, env, run) = started();
    dops.statuses(vec![streaming_status(300 << 20, "0/1")]);
    let run = tick(&ops, &dops, &env, &store, run);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(env.0.borrow().inbox.iter().filter(|(t, _)| t.contains("behind")).count(), 1);
    dops.statuses(vec![streaming_status(0, "0/2")]);
    let run = tick(&ops, &dops, &env, &store, run);
    dops.statuses(vec![streaming_status(300 << 20, "0/3")]);
    let _ = tick(&ops, &dops, &env, &store, run);
    assert_eq!(env.0.borrow().inbox.iter().filter(|(t, _)| t.contains("behind")).count(), 2, "a new episode is told again");
}

// ── the switch (PA31) ───────────────────────────────────────────────────

fn streaming() -> (FakeOps, FakeDual, FakeStore, FakeEnv) {
    let (ops, dops, store, env, run) = started();
    // The target cluster exists now (ensure_cluster added it).
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Streaming);
    env.0.borrow_mut().inbox.clear();
    (ops, dops, store, env)
}

fn so() -> SwitchOpts {
    SwitchOpts { actor: "owner".into(), now: false, refresh_timeout_secs: None }
}

#[test]
fn the_switch_runs_every_step_in_order() {
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    // 0.46.4 drill: the source's ACL still grants the run's role CONNECT at
    // the verify (it goes at teardown); it must not count as a difference.
    ops.0.borrow_mut().prints.get_mut(&16).unwrap()["database"] =
        json!(["ws_jobs_migrator", ["=Tc/ws_jobs_migrator", "k2_repl_9f1c2b3a4d5e=c/postgres", "ws_jobs_agent=c/ws_jobs_migrator"]]);
    ops.0.borrow_mut().prints.get_mut(&17).unwrap()["database"] = json!(["ws_jobs_migrator", ["=Tc/ws_jobs_migrator", "ws_jobs_agent=c/ws_jobs_migrator"]]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!(p.state, MoveState::Succeeded, "{:?}", p.last_error);
    assert_eq!(p.method, "dual");
    assert_eq!(run.state, RunState::Switched);
    assert_eq!(run.move_id.as_deref(), Some(p.id.as_str()));
    let o = ops.log();
    let pos = |w: &str| o.iter().position(|l| l.starts_with(w)).unwrap_or_else(|| panic!("{w}: {o:?}"));
    assert!(pos("allow_conn 16 ws_jobs quiesce-repl") < pos("allow_conn 17 ws_jobs on"));
    assert!(pos("allow_conn 17 ws_jobs on") < pos("allow_conn 16 ws_jobs off"));
    let d = dops.log();
    let dpos = |w: &str| d.iter().position(|l| l.starts_with(w)).unwrap_or_else(|| panic!("{w}: {d:?}"));
    assert!(dpos("dual detach") < dpos("force_rls 17 ws_jobs [16500]"));
    assert!(dpos("force_rls") < dpos("refresh 17 5433 ws_jobs as ws_jobs_migrator within 600s"), "B4: bounded: {d:?}");
    assert!(dpos("refresh") < dpos("dual teardown"), "the role goes at the end (step 10)");
    // B1: the target opens to K2's roles right after the detach (the
    // fakes refuse the owner's refresh and the logins on a quiesced copy,
    // so a success proves it), and only once.
    assert!(pos("allow_conn 17 ws_jobs on") > pos("allow_conn 16 ws_jobs quiesce-repl"));
    assert!(o.iter().any(|l| l.starts_with("login 17 5433")), "{o:?}");
    // A-M1: the teardown runs while the source is still reachable, then it closes.
    assert!(d.iter().any(|l| l == "dual teardown 16 17 ws_jobs 9f1c2b3a4d5e"), "{d:?}");
    assert!(!d.iter().any(|l| l.ends_with("(source closed)")), "{d:?}");
    assert_eq!(dops.mode(16, "ws_jobs").as_deref(), Some("off"), "the kept source ends closed");
    assert!(!run.objects.source_setup, "the role is gone");
    assert!(!p.warnings.iter().any(|w| w.contains("replication role")), "{:?}", p.warnings);
    // B3: the large objects looked at under the write block, before the detach.
    assert!(ops.0.borrow().full_sql.iter().any(|(m, _, s)| *m == 16 && s.contains("pg_catalog.pg_largeobject_metadata")));
    // Exact sequences on the target (L21).
    assert!(ops.0.borrow().full_sql.iter().any(|(m, _, s)| *m == 17 && s.contains("setval('\"public\".\"jobs_id_seq\"'::regclass, 42, true)")));
    let e = env.0.borrow();
    assert_eq!(e.sleeps.first(), Some(&60), "the 60 s notice");
    assert_eq!(e.majors, vec![17]);
    assert_eq!(e.repoints, vec![(5432, 5433)]);
    // PA10: every App on the database re-hosted on the new DSN, the running one included.
    assert_eq!(e.started, Vec::<String>::new());
    assert_eq!(e.rehosted, vec![(vec!["jobs".to_string()], p.id.clone())]);
    assert_eq!(e.moved.last().unwrap().as_ref().unwrap().source_state, SourceState::Kept);
    assert!(e.audits.iter().any(|(a, l)| a == "db-upgrade-live-switch" && l["pausedSecs"].is_number()));
    assert!(e.inbox.iter().any(|(t, b)| t.contains("moved 16 → 17") && b.contains("live by owner") && b.contains("writes paused")));
    assert!(e.rows.iter().all(|r| r.method == "dual"));
}

/// PA31 step 3: a drain that doesn't catch up undoes the cut-over; the
/// target stays and the run streams on.
#[test]
fn a_drain_timeout_undoes_the_cutover_and_the_run_streams_on() {
    let (ops, dops, store, env) = streaming();
    let mut behind = streaming_status(4096, "0/2000");
    behind["source"]["slot"]["confirmedFlushLsn"] = json!("0/1000");
    dops.statuses(vec![behind]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.last_error.as_deref().unwrap_or_default().starts_with("drain"), "{:?}", p.last_error);
    assert_eq!(run.state, RunState::Streaming);
    assert!(!dops.did("dual detach") && !dops.did("dual teardown"));
    assert!(!ops.did("drop_db"), "the target is kept: {:?}", ops.log());
    assert!(ops.did("allow_conn 16 ws_jobs on"), "the source re-opened");
    assert!(env.0.borrow().majors.is_empty());
    // A walsender that drops mid-block: the same.
    let (ops, dops, store, env) = streaming();
    let mut gone = streaming_status(0, "0/2000");
    gone["source"]["walsenders"] = json!(0);
    dops.statuses(vec![streaming_status(0, "0/2000"), streaming_status(0, "0/2000"), gone]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Streaming));
    assert!(p.last_error.as_deref().unwrap_or_default().contains("replication connection dropped"), "{:?}", p.last_error);
}

/// PA31 steps 4–7: after the detach an error drops the target, re-opens
/// the source, ends the run aborted and leaves nothing of it.
#[test]
fn a_failure_after_the_detach_is_the_copy_paths_undo() {
    for what in ["force_rls", "refresh", "verify", "sequences"] {
        let (ops, dops, store, env) = streaming();
        dops.statuses(vec![streaming_status(0, "0/16B3748")]);
        match what {
            "verify" => {
                ops.0.borrow_mut().counts.insert(17, json!({"public.jobs": 2}));
            }
            "sequences" => ops.0.borrow_mut().answers.insert(0, ("pg_catalog.setval(!".into(), "boom".into())),
            other => dops.0.borrow_mut().fail.push(other),
        }
        let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
        assert_eq!(p.state, MoveState::Aborted, "{what}: {:?}", p.last_error);
        assert_eq!(run.state, RunState::Aborted, "{what}");
        assert!(ops.did("drop_db 17 ws_jobs"), "{what}: target dropped: {:?}", ops.log());
        assert!(ops.did("allow_conn 16 ws_jobs on"), "{what}: source re-opened");
        assert!(dops.did("dual teardown"), "{what}: the rest of the run goes");
        assert!(env.0.borrow().majors.is_empty(), "{what}: the record never moved");
        if what == "verify" {
            assert!(!p.diff.is_empty(), "the diff is attached");
        }
    }
}

#[test]
fn the_switch_refuses_before_anything_changes() {
    // Not streaming yet.
    let (ops, dops, store, env, _run) = started();
    let e = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap_err();
    assert!(e.starts_with("not_streaming"), "{e}");
    // Too far behind; an App mid re-host; no run at all.
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![streaming_status(32 << 20, "0/1")]);
    let e = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap_err();
    assert!(e.starts_with("lag_too_high"), "{e}");
    dops.statuses(vec![streaming_status(0, "0/1")]);
    env.0.borrow_mut().rehost_running = Some("jobs".into());
    let e = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap_err();
    assert!(e.starts_with("rehost_running"), "{e}");
    assert!(!ops.did("allow_conn 16") && !dops.did("dual detach"));
    assert_eq!(store.only().state, RunState::Streaming, "nothing changed");
    let e = switch(&ops, &dops, &env, &store, "ws_other", &so()).unwrap_err();
    assert!(e.starts_with("no_live_move"), "{e}");
}

// ── restarts (PA36), orphans (PA35), --prepare-live (PA21) ──────────────

fn interrupted(step: Step, detached: bool, switched: bool) -> (FakeOps, FakeDual, FakeStore, FakeEnv, MoveProgress) {
    let (ops, dops, store, env) = streaming();
    let mut run = store.only();
    run.state = RunState::Switching;
    run.objects.detached = detached;
    let mut p = super::new_progress("move", "ws_jobs", 16, 17, "owner", NOW);
    p.method = "dual".into();
    p.step = step;
    p.from_port = Some(5432);
    p.to_port = Some(5433);
    p.quiesced = true;
    p.switched = switched;
    run.move_id = Some(p.id.clone());
    store.save_run(&run).unwrap();
    if switched {
        env.0.borrow_mut().rec.as_mut().unwrap().major = 17;
    }
    (ops, dops, store, env, p)
}

#[test]
fn a_restart_mid_switch_undoes_before_and_finishes_after() {
    // At the drain (not detached, still subscribed): undone, streams on.
    let (ops, dops, store, env, p) = interrupted(Step::Copy, false, false);
    dops.statuses(vec![streaming_status(0, "0/1")]);
    let done = recover_switch(&ops, &dops, &env, &store, p);
    assert_eq!(done.state, MoveState::Aborted);
    assert_eq!(store.only().state, RunState::Streaming);
    assert!(ops.did("allow_conn 16 ws_jobs on") && !ops.did("drop_db"));
    // After the detach, before the record moved: target dropped, aborted.
    let (ops, dops, store, env, p) = interrupted(Step::Verify, true, false);
    let done = recover_switch(&ops, &dops, &env, &store, p);
    assert_eq!(done.state, MoveState::Aborted);
    assert_eq!(store.only().state, RunState::Aborted);
    assert!(ops.did("drop_db 17 ws_jobs") && ops.did("allow_conn 16 ws_jobs on"));
    // At step 9 (the record on 17): finished forward, never undone.
    let (ops, dops, store, env, p) = interrupted(Step::Resume, true, true);
    let done = recover_switch(&ops, &dops, &env, &store, p);
    assert_eq!(done.state, MoveState::Succeeded, "{:?}", done.last_error);
    assert_eq!(store.only().state, RunState::Switched);
    assert!(!ops.did("drop_db"), "the live copy is never dropped");
    assert!(ops.did("allow_conn 17 ws_jobs on") && ops.did("allow_conn 16 ws_jobs off"));
}

#[test]
fn boot_drops_a_run_left_setting_up_and_settles_switching_rows() {
    let ops = live_ops();
    let dops = FakeDual::new();
    let (store, env) = store_env();
    let mk = |db: &str, dbid: &str, state: RunState, mv: Option<&str>| DualRun {
        id: format!("dr_{db}"), database_id: dbid.into(), db: db.into(), from: 16, to: 17, state,
        objects: Objects { object_id: object_id(dbid), source_setup: true, target_created: true, ..Default::default() },
        replica_identity_full: vec![], lag: Lag::default(), move_id: mv.map(str::to_string), actor: "owner".into(),
        started_at: NOW, streaming_at: None, finished_at: None, last_error: None,
    };
    store.insert_run(&mk("ws_jobs", "d-a", RunState::SettingUp, None)).unwrap();
    store.insert_run(&mk("ws_b", "d-b", RunState::Switching, Some("mv_b"))).unwrap();
    store.insert_run(&mk("ws_c", "d-c", RunState::Switching, Some("mv_c"))).unwrap();
    store.insert_run(&mk("ws_d", "d-d", RunState::Streaming, None)).unwrap();
    store.0.borrow_mut().moves.push(("mv_b".into(), MoveState::Succeeded));
    store.0.borrow_mut().moves.push(("mv_c".into(), MoveState::Aborted));
    let out = boot_runs(&ops, &dops, &env, &store);
    let st = |db: &str| store.last_run(db).unwrap().state;
    assert_eq!(st("ws_jobs"), RunState::Aborted, "{out:?}");
    assert!(dops.did("dual teardown 16 17 ws_jobs"));
    assert_eq!(st("ws_b"), RunState::Switched);
    assert_eq!(st("ws_c"), RunState::Streaming);
    assert!(ops.did("allow_conn 16 ws_c on"));
    assert_eq!(st("ws_d"), RunState::Streaming, "a streaming run keeps streaming");
}

#[test]
fn orphans_of_finished_runs_are_torn_down_and_unknown_ones_reported() {
    let ops = live_ops();
    let dops = FakeDual::new();
    let (store, _env) = store_env();
    let mut done = DualRun {
        id: "dr_old".into(), database_id: "d-old".into(), db: "ws_jobs".into(), from: 16, to: 17, state: RunState::Aborted,
        objects: Objects { object_id: "aaaaaaaaaaaa".into(), ..Default::default() }, replica_identity_full: vec![], lag: Lag::default(),
        move_id: None, actor: "owner".into(), started_at: NOW, streaming_at: None, finished_at: Some(NOW), last_error: None,
    };
    store.0.borrow_mut().runs.push(done.clone());
    done.id = "dr_open".into();
    done.objects.object_id = "bbbbbbbbbbbb".into();
    done.state = RunState::Streaming;
    store.0.borrow_mut().runs.push(done);
    ops.0.borrow_mut().answers.insert(
        0,
        (
            "'slots', (SELECT".into(),
            json!({"slots": [
                {"name": "k2_dual_aaaaaaaaaaaa", "db": "ws_jobs", "active": false},
                {"name": "k2_dual_bbbbbbbbbbbb", "db": "ws_jobs", "active": true},
                {"name": "k2_dual_cccccccccccc", "db": "ws_x", "active": false}
            ], "roles": ["k2_repl_aaaaaaaaaaaa"]})
            .to_string(),
        ),
    );
    let v = sweep_orphans(&ops, &dops, &store, true);
    assert_eq!(v["cleaned"], json!(["16/aaaaaaaaaaaa"]), "{v}");
    assert!(dops.did("dual teardown 16 17 ws_jobs aaaaaaaaaaaa"));
    assert!(!dops.did("dual teardown 16 17 ws_jobs bbbbbbbbbbbb"), "an open run's objects are never orphans");
    assert!(v["failures"].to_string().contains("cccccccccccc"), "an inactive slot K2 can't tie to a run fails the doctor: {v}");
    // The doctor's read-only look cleans nothing.
    let dops2 = FakeDual::new();
    let v = sweep_orphans(&ops, &dops2, &store, false);
    assert!(dops2.log().is_empty());
    assert_eq!(v["orphans"].as_array().unwrap().len(), 2, "{v}");
}

#[test]
fn prepare_live_restarts_once_and_only_when_needed() {
    let (store, env) = store_env();
    drop(store);
    let ops = live_ops();
    let told = RefCell::new(Vec::new());
    let notice = |m: Major, t: &str| told.borrow_mut().push((m, t.to_string()));
    // Not logical yet: the drop-in, a 60 s notice, one restart, then logical.
    let dops = FakeDual::new();
    let v = prepare_live(&ops, &dops, &env, 16, false, &notice).unwrap();
    assert_eq!(v["restarted"], true);
    assert_eq!(dops.log(), vec!["logical_conf 16 Some(8192) if_logical=false".to_string(), "restart 16".to_string()]);
    assert_eq!(told.borrow().len(), 1);
    assert!(told.borrow()[0].1.contains("5-15 s"));
    assert_eq!(env.0.borrow().sleeps, vec![60]);
    // Still not logical after the restart: an error, never a quiet success.
    let ops2 = FakeOps::new();
    ops2.0.borrow_mut().answers.push(("SHOW wal_level".into(), "replica".into()));
    let e = prepare_live(&ops2, &FakeDual::new(), &env, 16, true, &notice).unwrap_err();
    assert!(e.contains("still runs with wal_level = replica"), "{e}");
}

// ── VM drill fixes (0.46.4) ─────────────────────────────────────────────

/// A walsender ended in the write block: Postgres disables the
/// subscription; the undo enables it again and the run streams on. If it
/// stays disabled, the run ends failed and everything goes.
#[test]
fn an_undone_switch_enables_a_disabled_subscription_or_fails_the_run() {
    let mut gone = streaming_status(0, "0/2000");
    gone["source"]["walsenders"] = json!(0);
    let mut disabled = streaming_status(0, "0/2000");
    disabled["target"]["enabled"] = json!(false);
    let ok = streaming_status(0, "0/2000");
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![ok.clone(), ok.clone(), gone.clone(), disabled.clone(), ok.clone()]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Streaming), "{:?}", run.last_error);
    assert!(ops.0.borrow().full_sql.iter().any(|(m, d, s)| *m == 17 && d == "ws_jobs" && s == "ALTER SUBSCRIPTION \"k2_dual_9f1c2b3a4d5e\" ENABLE;\n"), "enabled on the target");
    assert!(!ops.did("drop_db") && !dops.did("dual teardown"));
    // Still disabled after ENABLE: failed, cleaned, the owner told.
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![ok.clone(), ok.clone(), gone, disabled]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Failed), "{:?}", run.last_error);
    assert!(run.last_error.as_deref().unwrap_or_default().contains("couldn't resume"), "{:?}", run.last_error);
    assert!(dops.did("dual teardown") && ops.did("drop_db 17 ws_jobs") && ops.did("allow_conn 16 ws_jobs on"));
    assert!(env.0.borrow().inbox.iter().any(|(t, _)| t.contains("live move failed")), "{:?}", env.0.borrow().inbox);
    assert_eq!(store.only().state, RunState::Failed);
}

// ── security review fixes (B1–B8) ───────────────────────────────────────

/// B2: the drain waits for the WAL INSERT position read after the write
/// block (a `synchronous_commit = off` commit sits past the write
/// position): a slot that only reached the write position never switches.
#[test]
fn the_drain_waits_for_the_insert_position() {
    let at = |confirmed: &str| {
        let mut s = streaming_status(0, "0/1000");
        s["source"]["insertLsn"] = json!("0/2000");
        s["source"]["slot"]["confirmedFlushLsn"] = json!(confirmed);
        s
    };
    // Ready to switch (lag 0), the drain target, then two looks short of
    // the insert position, then there.
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![at("0/1000"), at("0/1000"), at("0/1000"), at("0/1800"), at("0/2000")]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!(p.state, MoveState::Succeeded, "{:?}", p.last_error);
    assert_eq!(run.state, RunState::Switched);
    assert_eq!(env.0.borrow().sleeps.iter().filter(|s| **s == 1).count(), 2, "waited past the write position: {:?}", env.0.borrow().sleeps);
    // Stuck at the write position: never switched, undone, streams on.
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![at("0/1000")]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Streaming), "{:?}", p.last_error);
    assert!(p.last_error.as_deref().unwrap_or_default().starts_with("drain"), "{:?}", p.last_error);
    assert!(!dops.did("dual detach") && env.0.borrow().majors.is_empty());
    // No insert position at all: refused loudly, never the write position.
    let (ops, dops, store, env) = streaming();
    let mut old = streaming_status(0, "0/1000");
    old["source"].as_object_mut().unwrap().remove("insertLsn");
    dops.statuses(vec![old]);
    let (p, _) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.last_error.as_deref().unwrap_or_default().contains("insert position"), "{:?}", p.last_error);
}

/// B3: a large object made during the run (lo_create isn't DDL) undoes the
/// cut-over before the detach; the run streams on and says why.
#[test]
fn a_large_object_made_during_the_run_undoes_the_switch() {
    let (ops, dops, store, env) = streaming();
    ops.0.borrow_mut().answers.insert(0, ("pg_catalog.pg_largeobject_metadata".into(), "2".into()));
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Streaming), "{:?}", p.last_error);
    assert!(p.last_error.as_deref().unwrap_or_default().starts_with("large_objects: ws_jobs gained 2"), "{:?}", p.last_error);
    assert!(!dops.did("dual detach") && !ops.did("drop_db"), "nothing dropped: {:?}", dops.log());
    assert!(ops.did("allow_conn 16 ws_jobs on"), "the source re-opened");
    assert!(env.0.borrow().majors.is_empty());
    // An unreadable answer is an error too, never a pass.
    let (ops, dops, store, env) = streaming();
    ops.0.borrow_mut().answers.insert(0, ("pg_catalog.pg_largeobject_metadata".into(), "".into()));
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    let (p, _) = switch(&ops, &dops, &env, &store, "ws_jobs", &so()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(!dops.did("dual detach"));
}

/// B4: the refresh's bound comes from the switch (clamped), and a refresh
/// that fails (its timeout) undoes the cut-over (the copy path's undo).
#[test]
fn the_refresh_is_bounded_and_its_timeout_undoes() {
    assert_eq!(refresh_timeout_secs(None), REFRESH_TIMEOUT_SECS, "K2_DB_LIVE_REFRESH_TIMEOUT_SECS must be unset in tests");
    assert_eq!((refresh_timeout_secs(Some(5)), refresh_timeout_secs(Some(1_000_000)), refresh_timeout_secs(Some(45))), (30, 86_400, 45));
    let (ops, dops, store, env) = streaming();
    dops.statuses(vec![streaming_status(0, "0/16B3748")]);
    dops.0.borrow_mut().fail.push("refresh");
    let o = SwitchOpts { refresh_timeout_secs: Some(45), ..so() };
    let (p, run) = switch(&ops, &dops, &env, &store, "ws_jobs", &o).unwrap();
    assert!(dops.did("refresh 17 5433 ws_jobs as ws_jobs_migrator within 45s"), "{:?}", dops.log());
    assert_eq!((p.state, run.state), (MoveState::Aborted, RunState::Aborted), "{:?}", p.last_error);
    assert!(ops.did("drop_db 17 ws_jobs") && ops.did("allow_conn 16 ws_jobs on"));
}

/// B5: a restart mid-cut-over, before the detach: a target that says its
/// subscription is gone is dropped (the old order forgot it); a status that
/// can't be read drops nothing and the run streams on.
#[test]
fn a_restart_before_the_detach_drops_only_what_it_knows_is_gone() {
    let (ops, dops, store, env, p) = interrupted(Step::Copy, false, false);
    let mut gone = streaming_status(0, "0/1");
    gone["target"]["subscription"] = json!(false);
    dops.statuses(vec![gone]);
    let done = recover_switch(&ops, &dops, &env, &store, p);
    assert_eq!(done.state, MoveState::Aborted);
    assert_eq!(store.only().state, RunState::Aborted);
    assert!(ops.did("drop_db 17 ws_jobs"), "the target copy is dropped: {:?}", ops.log());
    assert!(dops.did("dual teardown") && ops.did("allow_conn 16 ws_jobs on"));
    // Unknown: nothing dropped, the source re-opened, the run streams on.
    let (ops, dops, store, env, p) = interrupted(Step::Copy, false, false);
    dops.0.borrow_mut().fail.push("status");
    let done = recover_switch(&ops, &dops, &env, &store, p);
    assert_eq!(done.state, MoveState::Aborted);
    let r = store.only();
    assert_eq!(r.state, RunState::Streaming);
    assert!(r.last_error.as_deref().unwrap_or_default().contains("can't be read"), "{:?}", r.last_error);
    assert!(!ops.did("drop_db") && !dops.did("dual teardown"), "{:?} {:?}", ops.log(), dops.log());
    assert!(ops.did("allow_conn 16 ws_jobs on"));
}

/// B6: the watch loop ends a run (a lost slot, day 7) only holding the
/// move worker; busy, it changes nothing and tries again next tick.
#[test]
fn the_watch_loop_ends_a_run_only_holding_the_worker() {
    let (ops, dops, store, env, run) = started();
    let mut st = streaming_status(0, "0/1");
    st["source"]["slot"]["walStatus"] = json!("lost");
    dops.statuses(vec![st]);
    store.0.borrow_mut().busy = true;
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Syncing, "busy: untouched");
    assert!(run.lag.errors.iter().any(|e| e.contains("move worker")), "{:?}", run.lag.errors);
    assert!(!dops.did("dual teardown") && !ops.did("drop_db"));
    assert_eq!(store.only().state, RunState::Syncing);
    store.0.borrow_mut().busy = false;
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(store.0.borrow().holds, 1);
    assert!(dops.did("dual teardown") && ops.did("drop_db 17 ws_jobs"));
}

/// B7: an old or unreachable helper: `--abort` parks the run as
/// `needs_helper` (open, so K2 keeps refusing `db_dual_running`; the
/// target kept, its subscription blocks the drop), told once; the watch
/// loop finishes it once the helper speaks `pgdual` again.
#[test]
fn an_abort_without_the_helper_waits_for_it() {
    let (ops, dops, store, env, run) = started();
    dops.0.borrow_mut().fail.push("ready");
    let r = abort_run(&ops, &dops, &env, &store, run, "aborted by owner", false);
    assert_eq!(r.state, RunState::NeedsHelper);
    assert!(r.state.open());
    assert_eq!(r.objects.end_as, Some(RunState::Aborted));
    assert!(r.objects.source_setup && r.objects.target_created, "{:?}", r.objects);
    assert!(store.open_run("ws_jobs").is_some(), "still open: db_dual_running holds");
    assert!(!dops.did("dual teardown") && !ops.did("drop_db"), "nothing half-done: {:?}", ops.log());
    assert!(r.last_error.as_deref().unwrap_or_default().contains("waiting for the root helper"), "{:?}", r.last_error);
    // The helper is still old: the loop waits, tells nothing new.
    let r = tick(&ops, &dops, &env, &store, r);
    let r = tick(&ops, &dops, &env, &store, r);
    assert_eq!(r.state, RunState::NeedsHelper);
    assert_eq!(env.0.borrow().inbox.iter().filter(|(t, _)| t.contains("needs the root helper")).count(), 1, "told once");
    // Updated: the teardown runs, the run ends as it was going to.
    dops.0.borrow_mut().fail.clear();
    let r = tick(&ops, &dops, &env, &store, r);
    assert_eq!(r.state, RunState::Aborted, "{:?}", r.last_error);
    assert_eq!(r.last_error.as_deref(), Some("aborted by owner"));
    assert!(dops.did("dual teardown") && ops.did("drop_db 17 ws_jobs"));
    assert!(store.open_run("ws_jobs").is_none(), "db_dual_running clears only now");
    // A lost slot whose teardown fails (helper unreachable): parked as
    // needs_helper, ends failed.
    let (ops, dops, store, env, run) = started();
    let mut st = streaming_status(0, "0/1");
    st["source"]["slot"]["walStatus"] = json!("lost");
    dops.statuses(vec![st]);
    dops.0.borrow_mut().fail.push("teardown");
    let r = tick(&ops, &dops, &env, &store, run);
    assert_eq!((r.state, r.objects.end_as), (RunState::NeedsHelper, Some(RunState::Failed)));
    dops.0.borrow_mut().fail.clear();
    let r = tick(&ops, &dops, &env, &store, r);
    assert_eq!(r.state, RunState::Failed);
}

/// VM drill round 3: right after a daemon restart the target can't be
/// read yet; the run keeps its last table counts (never "0/0") and stays
/// streaming.
#[test]
fn an_unreadable_target_keeps_the_last_sample() {
    let (ops, dops, store, env) = streaming();
    let run = store.only();
    assert_eq!((run.lag.tables_ready, run.lag.tables_total), (3, 3));
    let mut st = streaming_status(0, "0/16B3748");
    st["target"] = json!({"error": "cluster_down"});
    dops.statuses(vec![st]);
    let run = tick(&ops, &dops, &env, &store, run);
    assert_eq!(run.state, RunState::Streaming);
    assert_eq!((run.lag.tables_ready, run.lag.tables_total, run.lag.enabled), (3, 3, Some(true)));
    assert!(run.lag.errors.iter().any(|e| e.contains("target: cluster_down")), "{:?}", run.lag.errors);
    assert_eq!(run.json()["tables"], json!({"ready": 3, "total": 3}));
}
