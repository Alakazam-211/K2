//! The move machine over fakes: every §13 row, the check refusals, the
//! rollback, forget-old and boot recovery. No Postgres, no helper.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::check::Level;
use super::*;

const NOW: i64 = 1_792_491_300; // 2026-10-20T10:15:00Z

#[derive(Default)]
struct OpsState {
    clusters: Vec<ClusterRow>,
    installed: Vec<Major>,
    listening: Vec<u16>,
    /// (major, db) databases
    dbs: HashSet<(Major, String)>,
    log: Vec<String>,
    fail: HashSet<&'static str>,
    free: u64,
    size: u64,
    extensions: Vec<&'static str>,
    ext_missing_on_17: Vec<&'static str>,
    /// major → row counts JSON
    counts: HashMap<Major, Value>,
    /// major → fingerprint
    prints: HashMap<Major, Value>,
    roles_dry: Value,
    event_triggers: u64,
}

struct FakeOps(RefCell<OpsState>);

fn row(m: Major, port: u16, online: bool) -> ClusterRow {
    ClusterRow {
        major: m,
        name: "main".into(),
        port,
        status: if online { "online" } else { "down" }.into(),
        owner: "postgres".into(),
        data_dir: format!("/var/lib/postgresql/{m}/main"),
    }
}

impl FakeOps {
    fn new() -> Self {
        let print = json!({"extensions": ["pgcrypto"], "rls": {"public.jobs": [true, true]}, "sequences": {"public.jobs_n_seq": 42}});
        let counts = json!({"public.jobs": 3});
        let mut s = OpsState {
            clusters: vec![row(16, 5432, true)],
            installed: vec![16],
            listening: vec![5432],
            free: 50 << 30,
            size: 8 << 20,
            extensions: vec!["pgcrypto", "pg_trgm"],
            roles_dry: json!({"wouldCreate": ["ws_jobs_agent"], "same": [], "mismatch": [], "refused": [], "missing": []}),
            ..Default::default()
        };
        s.dbs.insert((16, "ws_jobs".into()));
        s.counts.insert(16, counts.clone());
        s.counts.insert(17, counts);
        s.prints.insert(16, print.clone());
        s.prints.insert(17, print);
        FakeOps(RefCell::new(s))
    }
    fn log(&self) -> Vec<String> {
        self.0.borrow().log.clone()
    }
    fn did(&self, what: &str) -> bool {
        self.log().iter().any(|l| l.starts_with(what))
    }
    fn pos(&self, what: &str) -> usize {
        self.log().iter().position(|l| l.starts_with(what)).unwrap_or_else(|| panic!("{what} never ran: {:?}", self.log()))
    }
    fn gate(&self, op: &'static str) -> Result<(), String> {
        if self.0.borrow().fail.contains(op) {
            Err(format!("{op} failed (injected)"))
        } else {
            Ok(())
        }
    }
    fn push(&self, s: String) {
        self.0.borrow_mut().log.push(s);
    }
}

impl ClusterOps for FakeOps {
    fn clusters(&self) -> Result<Vec<ClusterRow>, String> {
        Ok(self.0.borrow().clusters.clone())
    }
    fn installed(&self) -> Result<Vec<Major>, String> {
        Ok(self.0.borrow().installed.clone())
    }
    fn band_listening(&self) -> Result<Vec<u16>, String> {
        Ok(self.0.borrow().listening.clone())
    }
    fn add_major(&self, m: Major) -> Result<(), String> {
        self.push(format!("add_major {m}"));
        self.gate("add_major")?;
        self.0.borrow_mut().installed.push(m);
        Ok(())
    }
    fn ensure_cluster(&self, m: Major, port: u16) -> Result<(), String> {
        self.push(format!("ensure_cluster {m} {port}"));
        self.gate("ensure_cluster")?;
        let mut s = self.0.borrow_mut();
        if !s.clusters.iter().any(|r| r.major == m) {
            s.clusters.push(row(m, port, true));
        }
        for r in s.clusters.iter_mut().filter(|r| r.major == m) {
            r.status = "online".into();
        }
        Ok(())
    }
    fn sql(&self, m: Major, db: &str, sql: &str) -> Result<String, String> {
        let s = &self.0;
        let first = sql.lines().next().unwrap_or("").chars().take(60).collect::<String>();
        self.push(format!("sql {m} {db} {first}"));
        if sql.contains("'eventTriggers'") {
            let st = s.borrow();
            return Ok(json!({"size": st.size, "extensions": st.extensions, "eventTriggers": st.event_triggers, "publications": 0,
                "subscriptions": 0, "tablespaces": 0, "largeObjects": 0, "dbSettings": 0, "sessions": 2, "connlimit": -1,
                "allowConn": true, "unloggedTables": 0}).to_string());
        }
        if sql.contains("WITH RECURSIVE base") {
            return Ok(r#"["ws_jobs_agent","ws_jobs_bind","ws_jobs_migrator"]"#.into());
        }
        if sql.contains("FROM pg_auth_members") {
            return Ok(r#"[["ws_jobs_migrator","ws_jobs_agent",false,true,true]]"#.into());
        }
        if sql.contains("rolpassword LIKE 'md5%'") {
            return Ok("[]".into());
        }
        if sql.contains("SELECT count(*) FROM pg_database WHERE datname") {
            let has = s.borrow().dbs.contains(&(m, db_in(sql)));
            return Ok(if has { "1" } else { "0" }.into());
        }
        if sql.contains("to_jsonb(d) - 'datacl'") {
            return Ok(json!({"owner": "ws_jobs_migrator", "enc": "UTF8", "datcollate": "C.UTF-8", "datctype": "C.UTF-8", "datlocprovider": "c"}).to_string());
        }
        if sql.contains("'isNull', d.datacl IS NULL") {
            return Ok(r#"{"isNull": false, "items": [["ws_jobs_agent","CONNECT",false],["ws_jobs_migrator","CONNECT",false]]}"#.into());
        }
        if sql.contains("FROM pg_db_role_setting s, unnest") {
            return Ok("[]".into());
        }
        if sql.contains("string_agg(format('SELECT %L AS t") {
            return Ok("SELECT 'public.jobs' AS t, count(*) AS n FROM \"public\".\"jobs\"".into());
        }
        if sql.contains("json_object_agg(t, n)") {
            return Ok(s.borrow().counts[&m].to_string());
        }
        if sql.contains("'policies'") {
            return Ok(s.borrow().prints[&m].to_string());
        }
        if sql.starts_with("CREATE DATABASE") {
            self.gate("create_db")?;
            s.borrow_mut().dbs.insert((m, "ws_jobs".into()));
            return Ok(String::new());
        }
        if sql.contains("RENAME TO") {
            let mut st = s.borrow_mut();
            st.dbs.remove(&(m, "ws_jobs".into()));
            let new = sql.split("RENAME TO \"").nth(1).unwrap().trim_end_matches(";\n").trim_end_matches('"').to_string();
            st.dbs.insert((m, new));
            return Ok(String::new());
        }
        if sql.starts_with("SELECT 1") {
            self.gate("healthy")?;
        }
        Ok(String::new())
    }
    fn copy_roles(&self, from: Major, to: Major, roles: &[String], dry_run: bool) -> Result<Value, String> {
        self.push(format!("copy_roles {from} {to} {} dry={dry_run}", roles.join(",")));
        if dry_run {
            return Ok(self.0.borrow().roles_dry.clone());
        }
        self.gate("copy_roles")?;
        Ok(json!({"created": ["ws_jobs_agent", "ws_jobs_bind", "ws_jobs_migrator"], "same": []}))
    }
    fn allow_conn(&self, m: Major, db: &str, mode: ConnMode) -> Result<(), String> {
        self.push(format!("allow_conn {m} {db} {}", mode.token()));
        self.gate(match mode {
            ConnMode::On => "allow_on",
            ConnMode::Quiesce => "allow_quiesce",
            ConnMode::Off => "allow_off",
        })
    }
    fn dump(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String> {
        self.push(format!("dump {from} {to} {db} {tag}"));
        self.gate("dump")?;
        Ok(4096)
    }
    fn restore(&self, to: Major, db: &str, tag: &str) -> Result<(), String> {
        self.push(format!("restore {to} {db} {tag}"));
        self.gate("restore")
    }
    fn drop_db(&self, m: Major, db: &str) -> Result<bool, String> {
        self.push(format!("drop_db {m} {db}"));
        self.gate("drop_db")?;
        Ok(self.0.borrow_mut().dbs.remove(&(m, db.to_string())))
    }
    fn remove_dump(&self, db: &str, tag: &str) -> Result<(), String> {
        self.push(format!("remove_dump {db} {tag}"));
        Ok(())
    }
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(self.0.borrow().free)
    }
    fn extension_available(&self, m: Major, ext: &str) -> Option<bool> {
        let s = self.0.borrow();
        if !s.installed.contains(&m) {
            return None;
        }
        Some(!(m == 17 && s.ext_missing_on_17.contains(&ext)))
    }
    fn login(&self, m: Major, port: u16, _db: &str, role: &str, _pw: &str) -> Result<(), String> {
        self.push(format!("login {m} {port} {role}"));
        self.gate("login")
    }
}

fn db_in(sql: &str) -> String {
    sql.split("datname = '").nth(1).and_then(|r| r.split('\'').next()).unwrap_or_default().to_string()
}

#[derive(Default)]
struct EnvState {
    rec: Option<DbRecord>,
    now: i64,
    sleeps: Vec<u64>,
    notices: Vec<String>,
    cleared: u32,
    doors: Vec<bool>,
    running_apps: Vec<String>,
    stop_fails: bool,
    started: Vec<String>,
    majors: Vec<Major>,
    moved: Vec<Option<MovedInfo>>,
    repoint_steps: Vec<&'static str>,
    repoint_fail_at: Option<usize>,
    unpoint_fails: bool,
    repoints: Vec<(u16, u16)>,
    unpoints: Vec<Vec<String>>,
    progress: Vec<(Step, MoveState)>,
    audits: Vec<(String, Value)>,
    inbox: Vec<(String, String)>,
    clusters_recorded: Vec<(Major, u16)>,
    role: HashMap<Major, ClusterRole>,
}

struct FakeEnv(RefCell<EnvState>);

fn rec16() -> DbRecord {
    DbRecord {
        name: "ws_jobs".into(),
        project_id: "p1".into(),
        major: 16,
        k2_roles: vec!["ws_jobs_migrator".into(), "ws_jobs_agent".into()],
        moved_from: None,
        moved_at: None,
        rollback_until: None,
        source_state: SourceState::None,
        move_tag: None,
    }
}

impl FakeEnv {
    fn new() -> Self {
        FakeEnv(RefCell::new(EnvState {
            rec: Some(rec16()),
            now: NOW,
            running_apps: vec!["jobs".into()],
            repoint_steps: vec!["vault DATABASE_URL", "vault DATABASE_URL_TEST"],
            ..Default::default()
        }))
    }
    fn states(&self) -> Vec<MoveState> {
        self.0.borrow().progress.iter().map(|p| p.1).collect()
    }
}

impl MoveEnv for FakeEnv {
    fn now(&self) -> i64 {
        self.0.borrow().now
    }
    fn sleep_secs(&self, s: u64) {
        self.0.borrow_mut().sleeps.push(s);
    }
    fn record(&self, _db: &str) -> Result<DbRecord, String> {
        self.0.borrow().rec.clone().ok_or_else(|| "not_found".into())
    }
    fn cluster_role(&self, m: Major) -> ClusterRole {
        self.0.borrow().role.get(&m).copied().unwrap_or(ClusterRole::K2)
    }
    fn logins(&self, _db: &str) -> Result<Vec<(String, String)>, String> {
        Ok(vec![("ws_jobs_agent".into(), "pw-a".into()), ("ws_jobs_migrator".into(), "pw-m".into())])
    }
    fn users(&self, _db: &str) -> Users {
        Users { apps: vec!["jobs".into()], literal_dsns: vec![], guest_doors: 1 }
    }
    fn notice(&self, _db: &str, text: &str) {
        self.0.borrow_mut().notices.push(text.into());
    }
    fn clear_notice(&self, _db: &str) {
        self.0.borrow_mut().cleared += 1;
    }
    fn pause_doors(&self, _db: &str, paused: bool) {
        self.0.borrow_mut().doors.push(paused);
    }
    fn stop_apps(&self, apps: &[String]) -> Result<Vec<String>, String> {
        let mut s = self.0.borrow_mut();
        if s.stop_fails {
            return Err("jobs didn't stop".into());
        }
        let was: Vec<String> = apps.iter().filter(|a| s.running_apps.contains(a)).cloned().collect();
        s.running_apps.retain(|a| !apps.contains(a));
        Ok(was)
    }
    fn start_apps(&self, apps: &[String]) -> Vec<String> {
        let mut s = self.0.borrow_mut();
        s.started.extend(apps.iter().cloned());
        s.running_apps.extend(apps.iter().cloned());
        vec![]
    }
    fn rerun_k2_setup(&self, _m: Major, _db: &str) -> Result<(), String> {
        Ok(())
    }
    fn verify_k2(&self, _m: Major, _db: &str) -> Result<(), String> {
        Ok(())
    }
    fn repoint(&self, _db: &str, from_port: u16, to_port: u16) -> Result<Vec<String>, (Vec<String>, String)> {
        let mut s = self.0.borrow_mut();
        s.repoints.push((from_port, to_port));
        let mut done = Vec::new();
        for (i, st) in s.repoint_steps.iter().enumerate() {
            if s.repoint_fail_at == Some(i) {
                return Err((done, format!("{st} failed")));
            }
            done.push(st.to_string());
        }
        Ok(done)
    }
    fn unpoint(&self, _db: &str, done: &[String], _f: u16, _t: u16) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        s.unpoints.push(done.to_vec());
        if s.unpoint_fails {
            Err("vault write failed".into())
        } else {
            Ok(())
        }
    }
    fn set_major(&self, _db: &str, m: Major) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        s.majors.push(m);
        if let Some(r) = s.rec.as_mut() {
            r.major = m;
        }
        Ok(())
    }
    fn set_moved(&self, _db: &str, info: Option<&MovedInfo>) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        s.moved.push(info.cloned());
        if let Some(r) = s.rec.as_mut() {
            r.moved_from = info.map(|i| i.from);
            r.moved_at = info.map(|i| i.at);
            r.rollback_until = info.map(|i| i.until);
            r.move_tag = info.map(|i| i.tag.clone());
            r.source_state = info.map(|i| i.source_state).unwrap_or_default();
        }
        Ok(())
    }
    fn record_cluster(&self, m: Major, port: u16) {
        self.0.borrow_mut().clusters_recorded.push((m, port));
    }
    fn progress(&self, p: &MoveProgress) {
        self.0.borrow_mut().progress.push((p.step, p.state));
    }
    fn audit(&self, act: &str, line: Value) {
        self.0.borrow_mut().audits.push((act.into(), line));
    }
    fn inbox(&self, title: &str, body: &str) {
        self.0.borrow_mut().inbox.push((title.into(), body.into()));
    }
}

fn opts() -> MoveOpts {
    MoveOpts { to: Some(17), actor: "agent:jobs".into(), now: false, allow_adopted: false, helper_ok: true }
}

const MUTATING: &[&str] = &["add_major", "ensure_cluster", "copy_roles 16 17 ws_jobs_agent,ws_jobs_bind,ws_jobs_migrator dry=false", "allow_conn", "dump", "restore", "drop_db"];

fn mutated(ops: &FakeOps) -> Vec<String> {
    ops.log().into_iter().filter(|l| MUTATING.iter().any(|m| l.starts_with(m)) || l.contains("CREATE DATABASE")).collect()
}

/// Everything before the switch was undone and the source serves again.
fn assert_undone(ops: &FakeOps, env: &FakeEnv) {
    let log = ops.log();
    assert!(log.iter().any(|l| l == "drop_db 17 ws_jobs"), "target copy dropped: {log:?}");
    assert_eq!(log.iter().filter(|l| l.as_str() == "allow_conn 16 ws_jobs on").count(), 1, "source re-opened: {log:?}");
    let e = env.0.borrow();
    assert_eq!(e.started, vec!["jobs".to_string()], "the stopped App started again");
    assert_eq!(e.doors.last(), Some(&false), "doors open again");
    assert!(e.cleared >= 1);
    assert!(!e.majors.contains(&17) || e.majors.last() == Some(&16), "record back on 16: {:?}", e.majors);
    assert!(!ops.0.borrow().dbs.contains(&(17, "ws_jobs".into())));
}

#[test]
fn tag_is_utc_compact() {
    assert_eq!(tag_for(NOW), "20261020T101500Z");
    assert_eq!(tag_for(0), "19700101T000000Z");
    assert!(k2_root_proto::pg_major::tag_ok(&tag_for(NOW)));
}

#[test]
fn a_move_runs_every_step_in_order() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Succeeded, "{:?}", p.last_error);
    assert_eq!(p.step, Step::Done);
    assert_eq!(p.to_port, Some(5433));
    assert_eq!(p.dump_bytes, Some(4096));
    // Helper order: install, cluster, roles, memberships, quiesce, counts,
    // dump, create, restore, grants, verify, analyze, close the source.
    let order = [
        "add_major 17",
        "ensure_cluster 17 5433",
        "copy_roles 16 17 ws_jobs_agent,ws_jobs_bind,ws_jobs_migrator dry=false",
        "sql 17 postgres GRANT \"ws_jobs_migrator\" TO \"ws_jobs_agent\"",
        "allow_conn 16 ws_jobs quiesce",
        "dump 16 17 ws_jobs 20261020T101500Z",
        "sql 17 postgres CREATE DATABASE \"ws_jobs\" OWNER \"ws_jobs_migrator\"",
        "restore 17 ws_jobs 20261020T101500Z",
        "sql 17 postgres REVOKE ALL ON DATABASE \"ws_jobs\" FROM PUBLIC;",
        "login 17 5433 ws_jobs_agent",
        "login 17 5433 ws_jobs_migrator",
        "sql 17 ws_jobs ANALYZE;",
        "allow_conn 16 ws_jobs off",
    ];
    let mut last = 0;
    for o in order {
        let i = ops.pos(o);
        assert!(i >= last, "{o} out of order: {:?}", ops.log());
        last = i;
    }
    assert!(!ops.did("drop_db"), "nothing dropped on success");
    let e = env.0.borrow();
    assert_eq!(e.sleeps, vec![60], "the 60 s notice");
    assert!(e.notices[0].contains("moves to Postgres 17"));
    assert_eq!(e.doors, vec![true, false]);
    assert_eq!(e.majors, vec![17]);
    assert_eq!(e.repoints, vec![(5432, 5433)]);
    assert_eq!(e.started, vec!["jobs".to_string()]);
    assert_eq!(e.clusters_recorded, vec![(17, 5433)]);
    let moved = e.moved.last().unwrap().clone().unwrap();
    assert_eq!(moved, MovedInfo { from: 16, at: NOW, until: NOW + 14 * 86_400, tag: "20261020T101500Z".into(), source_state: SourceState::Kept });
    assert!(e.audits.iter().any(|(a, l)| a == "db-upgrade-move" && l["from"] == 16 && l["to"] == 17));
    assert!(e.inbox.iter().any(|(t, b)| t == "Database ws_jobs moved 16 → 17" && b.contains("not carried back")));
    // Every step was recorded, in order.
    let steps: Vec<Step> = e.progress.iter().map(|p| p.0).collect();
    for w in steps.windows(2) {
        assert!(w[0] <= w[1], "{steps:?}");
    }
    assert_eq!(*e.progress.last().unwrap(), (Step::Done, MoveState::Succeeded));
}

#[test]
fn the_owner_may_skip_the_notice_and_an_existing_cluster_is_reused() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.clusters.push(row(17, 5433, true));
        s.installed.push(17);
    }
    let mut o = opts();
    o.now = true;
    let p = run_move(&ops, &env, "ws_jobs", &o).unwrap();
    assert_eq!(p.state, MoveState::Succeeded, "{:?}", p.last_error);
    assert!(env.0.borrow().sleeps.is_empty());
    assert!(!ops.did("add_major"));
    assert!(ops.did("ensure_cluster 17 5433"), "ensure is idempotent and starts it if down");
    assert!(ops.did("copy_roles 16 17 ws_jobs_agent,ws_jobs_bind,ws_jobs_migrator dry=true"), "check reads the dry run");
}

/// Brief: a refused move (a missing extension) changes nothing.
#[test]
fn a_missing_extension_refuses_before_anything_changes() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.installed.push(17);
        s.ext_missing_on_17 = vec!["pg_trgm"];
    }
    let r = check_only(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert!(r.refused());
    let l = r.lines.iter().find(|l| l.level == Level::Refuse).unwrap();
    assert_eq!(l.id, "extensions");
    assert!(l.text.contains("pg_trgm") && l.text.contains("no 17 build"), "{l:?}");
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.last_error.as_deref().unwrap().contains("pg_trgm"));
    assert!(mutated(&ops).is_empty(), "{:?}", mutated(&ops));
    let e = env.0.borrow();
    assert!(e.notices.is_empty() && e.doors.is_empty() && e.majors.is_empty() && e.moved.is_empty());
}

/// Brief: low disk refuses (2 × size + 1 GiB).
#[test]
fn low_disk_refuses_before_anything_changes() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.size = 2 << 30;
        s.free = 4 << 30; // needs 5 GiB
    }
    let r = check_only(&ops, &env, "ws_jobs", &opts()).unwrap();
    let l = r.lines.iter().find(|l| l.id == "disk").unwrap();
    assert_eq!(l.level, Level::Refuse);
    assert!(l.text.contains("4096 MiB free, 5120 MiB needed"), "{l:?}");
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(mutated(&ops).is_empty());
    // Exactly enough is fine.
    ops.0.borrow_mut().free = 5 << 30;
    assert!(!check_only(&ops, &env, "ws_jobs", &opts()).unwrap().refused());
}

#[test]
fn check_refusals() {
    // Old helper, downward move, adopted source, event triggers, band full,
    // a kept source, a verifier mismatch in an existing target.
    let cases: Vec<(&str, Box<dyn Fn(&FakeOps, &FakeEnv, &mut MoveOpts)>)> = vec![
        ("helper", Box::new(|_, _, o| o.helper_ok = false)),
        ("majors", Box::new(|_, _, o| o.to = Some(16))),
        ("majors", Box::new(|_, _, o| o.to = Some(18))),
        ("source", Box::new(|_, e, _| {
            e.0.borrow_mut().role.insert(16, ClusterRole::Adopted);
        })),
        ("objects", Box::new(|ops, _, _| ops.0.borrow_mut().event_triggers = 1)),
        ("target", Box::new(|ops, _, _| ops.0.borrow_mut().listening = (5432..=5449).collect())),
        ("kept", Box::new(|_, e, _| {
            let mut s = e.0.borrow_mut();
            let r = s.rec.as_mut().unwrap();
            r.source_state = SourceState::Kept;
            r.moved_from = Some(16);
        })),
        ("roles", Box::new(|ops, _, _| {
            let mut s = ops.0.borrow_mut();
            s.clusters.push(row(17, 5433, true));
            s.installed.push(17);
            s.roles_dry = json!({"wouldCreate": [], "same": [], "mismatch": ["ws_crm_agent"], "refused": [], "missing": []});
        })),
    ];
    for (id, setup) in cases {
        let (ops, env) = (FakeOps::new(), FakeEnv::new());
        let mut o = opts();
        setup(&ops, &env, &mut o);
        let r = check_only(&ops, &env, "ws_jobs", &o).unwrap();
        assert!(r.lines.iter().any(|l| l.id == id && l.level == Level::Refuse), "{id}: {:#?}", r.lines);
        let p = run_move(&ops, &env, "ws_jobs", &o).unwrap();
        assert_eq!(p.state, MoveState::Aborted, "{id}");
        assert!(mutated(&ops).is_empty(), "{id}: {:?}", mutated(&ops));
    }
    // The owner's --allow-adopted turns the adopted refusal into a warning.
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    env.0.borrow_mut().role.insert(16, ClusterRole::Adopted);
    let mut o = opts();
    o.allow_adopted = true;
    let r = check_only(&ops, &env, "ws_jobs", &o).unwrap();
    assert!(r.lines.iter().any(|l| l.id == "source" && l.level == Level::Warn));
    assert!(!r.refused());
}

#[test]
fn verify_mismatch_aborts_and_undoes_everything() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.counts.insert(17, json!({"public.jobs": 2}));
        s.prints.get_mut(&17).unwrap()["rls"] = json!({"public.jobs": [true, false]});
    }
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.last_error.as_deref().unwrap().starts_with("verify: 2 difference(s)"), "{:?}", p.last_error);
    assert!(p.diff.contains(&"rows public.jobs: 3 → 2".to_string()), "{:?}", p.diff);
    assert!(p.diff.contains(&"rls public.jobs: [true,true] → [true,false]".to_string()), "{:?}", p.diff);
    assert_undone(&ops, &env);
    let e = env.0.borrow();
    assert!(e.majors.is_empty() && e.repoints.is_empty(), "never switched");
    assert!(e.moved.is_empty());
    assert!(e.audits.iter().any(|(a, _)| a == "db-upgrade-aborted"));
}

#[test]
fn a_failed_login_on_the_target_is_a_verify_failure() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    ops.0.borrow_mut().fail.insert("login");
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.diff.iter().any(|d| d.starts_with("login ws_jobs_agent")), "{:?}", p.diff);
    assert_undone(&ops, &env);
}

#[test]
fn dump_or_restore_failure_aborts_and_undoes() {
    for (op, created) in [("dump", false), ("restore", true)] {
        let (ops, env) = (FakeOps::new(), FakeEnv::new());
        ops.0.borrow_mut().fail.insert(op);
        let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
        assert_eq!(p.state, MoveState::Aborted, "{op}");
        assert!(p.last_error.as_deref().unwrap().starts_with(op), "{op}: {:?}", p.last_error);
        assert_eq!(ops.did("drop_db 17 ws_jobs"), created, "{op}: drop only what was created");
        assert!(ops.did("allow_conn 16 ws_jobs on"), "{op}");
        assert_eq!(env.0.borrow().started, vec!["jobs".to_string()], "{op}");
        assert!(env.0.borrow().majors.is_empty(), "{op}");
    }
}

#[test]
fn an_app_that_wont_stop_aborts_before_the_lock() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    env.0.borrow_mut().stop_fails = true;
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(p.last_error.as_deref().unwrap().starts_with("stop Apps"));
    assert!(!ops.did("allow_conn 16 ws_jobs quiesce") && !ops.did("dump"));
    assert_eq!(env.0.borrow().doors.last(), Some(&false));
}

#[test]
fn a_role_copy_refusal_aborts_with_the_source_serving() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    ops.0.borrow_mut().fail.insert("copy_roles");
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Aborted);
    assert!(!ops.did("allow_conn 16 ws_jobs quiesce"));
    assert!(!ops.did("dump"));
}

#[test]
fn a_repoint_failure_is_reversed_and_rolled_back() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    env.0.borrow_mut().repoint_fail_at = Some(1);
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::RolledBack, "{:?}", p.last_error);
    let e = env.0.borrow();
    assert_eq!(e.unpoints, vec![vec!["vault DATABASE_URL".to_string()]], "only what was re-pointed is reversed");
    assert_eq!(e.majors, vec![17, 16], "record back on 16");
    assert!(e.moved.is_empty());
    drop(e);
    assert_undone(&ops, &env);
}

#[test]
fn a_failed_reversal_latches_failed_and_tells_the_owner() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = env.0.borrow_mut();
        s.repoint_fail_at = Some(1);
        s.unpoint_fails = true;
    }
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Failed);
    assert_eq!(p.repointed, vec!["vault DATABASE_URL".to_string()]);
    let e = env.0.borrow();
    assert!(e.inbox.iter().any(|(t, b)| t.contains("failed") && b.contains("--acknowledge-failed") && b.contains("vault DATABASE_URL")));
    assert!(e.audits.iter().any(|(a, _)| a == "db-upgrade-failed"));
}

#[test]
fn an_incomplete_undo_latches_failed() {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.fail.insert("restore");
        s.fail.insert("drop_db");
    }
    let p = run_move(&ops, &env, "ws_jobs", &opts()).unwrap();
    assert_eq!(p.state, MoveState::Failed);
    assert!(p.last_error.unwrap().contains("undo incomplete: drop the target copy"));
}

#[test]
fn recovery_undoes_before_the_switch_and_finishes_after_it() {
    let base = |step: Step| MoveProgress {
        id: "mv".into(),
        db: "ws_jobs".into(),
        from: 16,
        to: 17,
        tag: "20261020T101500Z".into(),
        actor: "owner".into(),
        state: MoveState::Copying,
        step,
        started_at: NOW,
        finished_at: None,
        last_error: None,
        from_port: Some(5432),
        to_port: Some(5433),
        target_created: step >= Step::Copy,
        quiesced: step >= Step::Quiesce,
        stopped_apps: vec!["jobs".into()],
        doors_paused: true,
        repointed: vec![],
        roles_created: vec![],
        dump_bytes: None,
        diff: vec![],
        warnings: vec![],
        check: vec![],
    };
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    ops.0.borrow_mut().clusters.push(row(17, 5433, true));
    ops.0.borrow_mut().dbs.insert((17, "ws_jobs".into()));
    let p = recover(&ops, &env, base(Step::Copy));
    assert_eq!(p.state, MoveState::Aborted);
    assert!(env.states().contains(&MoveState::Interrupted));
    assert_undone(&ops, &env);

    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    let p = recover(&ops, &env, base(Step::Switch));
    assert_eq!(p.state, MoveState::Succeeded, "{:?}", p.last_error);
    let e = env.0.borrow();
    assert_eq!(e.majors, vec![17]);
    assert_eq!(e.repoints, vec![(5432, 5433)]);
    assert_eq!(e.started, vec!["jobs".to_string()]);
    assert_eq!(e.moved.last().unwrap().as_ref().unwrap().source_state, SourceState::Kept);
    assert!(ops.did("allow_conn 16 ws_jobs off"));

    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    let p = recover(&ops, &env, base(Step::Resume));
    assert_eq!(p.state, MoveState::Succeeded);
    assert!(env.0.borrow().repoints.is_empty(), "already re-pointed before the restart");
}

fn moved_env() -> (FakeOps, FakeEnv) {
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    {
        let mut s = ops.0.borrow_mut();
        s.clusters.push(row(17, 5433, true));
        s.dbs.insert((17, "ws_jobs".into()));
    }
    {
        let mut s = env.0.borrow_mut();
        let r = s.rec.as_mut().unwrap();
        r.major = 17;
        r.moved_from = Some(16);
        r.moved_at = Some(NOW);
        r.rollback_until = Some(NOW + ROLLBACK_WINDOW_SECS);
        r.source_state = SourceState::Kept;
        r.move_tag = Some("20261020T101500Z".into());
    }
    (ops, env)
}

#[test]
fn rollback_goes_back_to_the_kept_source() {
    let (ops, env) = moved_env();
    let o = RollbackOpts { actor: "agent:jobs".into(), yes: false, now: false };
    assert!(rollback(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("rollback_needs_yes"));
    let o = RollbackOpts { yes: true, ..o };
    env.0.borrow_mut().now = NOW + 3600;
    let v = rollback(&ops, &env, "ws_jobs", &o).unwrap();
    let kept = format!("ws_jobs__k2rb_{}", NOW + 3600);
    assert_eq!(v["major"], 16);
    assert_eq!(v["keptAs"], kept);
    let order = ["allow_conn 17 ws_jobs quiesce", "allow_conn 16 ws_jobs on", "sql 17 postgres SELECT count(pg_terminate_backend", &format!("allow_conn 17 {kept} off")];
    let mut last = 0;
    for o in order {
        let i = ops.pos(o);
        assert!(i >= last, "{o}: {:?}", ops.log());
        last = i;
    }
    assert!(ops.0.borrow().dbs.contains(&(17, kept)), "the 17 copy is kept, renamed");
    let e = env.0.borrow();
    assert_eq!(e.majors, vec![16]);
    assert_eq!(e.repoints, vec![(5433, 5432)]);
    assert_eq!(e.moved.last(), Some(&None));
    assert_eq!(e.sleeps, vec![60]);
    assert!(e.inbox.iter().any(|(t, b)| t.contains("rolled back 17 → 16") && b.contains("not carried back")));
    assert!(!ops.did("drop_db"), "a rollback never drops");
}

#[test]
fn rollback_refusals() {
    let o = RollbackOpts { actor: "o".into(), yes: true, now: true };
    let (ops, env) = moved_env();
    env.0.borrow_mut().now = NOW + ROLLBACK_WINDOW_SECS + 1;
    assert!(rollback(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("window_closed"));
    let (ops, env) = moved_env();
    env.0.borrow_mut().rec.as_mut().unwrap().source_state = SourceState::Forgotten;
    assert!(rollback(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("source_gone"));
    let (ops, env) = (FakeOps::new(), FakeEnv::new());
    assert!(rollback(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("not_moved"));
    assert!(ops.log().is_empty());
    // A re-point failure on the way back is reversed: still on 17.
    let (ops, env) = moved_env();
    env.0.borrow_mut().repoint_fail_at = Some(0);
    let err = rollback(&ops, &env, "ws_jobs", &o).unwrap_err();
    assert!(err.contains("reversed; still on 17"), "{err}");
    assert_eq!(env.0.borrow().majors, vec![16, 17]);
    assert!(ops.did("allow_conn 17 ws_jobs on"));
    assert!(!ops.did("allow_conn 16 ws_jobs on"));
}

#[test]
fn forget_old_is_owner_only_and_never_touches_packages() {
    let (ops, env) = moved_env();
    let mut o = ForgetOpts { actor: "agent:jobs".into(), owner: false, force_now: false, move_busy: false };
    assert!(forget_old(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("owner_only"));
    o.owner = true;
    assert!(forget_old(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("too_soon"));
    o.move_busy = true;
    assert!(forget_old(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("busy"));
    o.move_busy = false;
    ops.0.borrow_mut().fail.insert("healthy");
    o.force_now = true;
    assert!(forget_old(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("not_healthy"));
    assert!(!ops.did("drop_db"));
    ops.0.borrow_mut().fail.clear();
    let v = forget_old(&ops, &env, "ws_jobs", &o).unwrap();
    assert_eq!(v["dropped"], true);
    assert_eq!(v["packagesKept"], true);
    assert!(ops.did("drop_db 16 ws_jobs"));
    assert!(ops.did("remove_dump ws_jobs 20261020T101500Z"));
    assert!(!ops.did("drop_db 17"), "never the live copy");
    assert!(!ops.log().iter().any(|l| l.contains("remove") && l.contains("postgresql")), "no package ever");
    assert_eq!(env.0.borrow().moved.last().unwrap().as_ref().unwrap().source_state, SourceState::Forgotten);
    assert!(forget_old(&ops, &env, "ws_jobs", &o).unwrap_err().starts_with("nothing_kept"));
    // 24 h later without --force-now is fine.
    let (ops, env) = moved_env();
    env.0.borrow_mut().now = NOW + FORGET_MIN_AGE_SECS;
    o.force_now = false;
    assert!(forget_old(&ops, &env, "ws_jobs", &o).is_ok());
}

/// Sew 2026-10-20: tidy stops empty K2 clusters only; packages stay (Sew's
/// own Postgres runs postgresql-16), nothing is dropped.
#[test]
fn tidy_stops_empty_k2_clusters_only_and_keeps_packages() {
    let rows = vec![row(16, 5432, true), row(17, 5433, true), row(18, 5434, true)];
    let roles = |m: Major| if m == 18 { ClusterRole::Adopted } else { ClusterRole::K2 };
    let dbs = |m: Major| if m == 17 { 3 } else { 0 };
    let kept = |_m: Major| 0;
    let t = tidy_plan(&rows, &roles, &dbs, &kept, Some(16));
    assert_eq!(t.stop, vec![16]);
    assert_eq!(t.keep, vec![(17, "3 K2 database(s)".to_string()), (18, "not K2's cluster".to_string())]);
    assert!(t.packages_kept);
    assert!(t.note.contains("never removes Postgres packages") && t.note.contains("Sew's own Postgres runs postgresql-16"));
    let kept16 = |m: Major| usize::from(m == 16);
    let t = tidy_plan(&rows, &roles, &dbs, &kept16, None);
    assert!(t.stop.is_empty(), "a kept source keeps its cluster");
}

#[test]
fn free_band_port_skips_clusters_and_listeners() {
    let rows = vec![row(16, 5432, true)];
    assert_eq!(free_band_port(&rows, &[5432, 5433]), Some(5434));
    assert_eq!(free_band_port(&rows, &(5433..=5449).collect::<Vec<_>>()), None);
}
