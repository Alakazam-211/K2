//! K2's side of a live dual-run (prd-app-pg-choice-dualrun-v1 D2): the run
//! rows (`sql_dual_runs`, migration 0222) as [`RunStore`], the root
//! helper's protocol-10 verbs as [`DualOps`], and the watch loop (PA29,
//! daemon-first: it runs headless, every 30 s while a run is open).

use serde_json::{json, Value};

use k2_root_proto::pg_dual::{DualOp, REFRESH_SQL};
use k2_root_proto::pg_major::PgMajor;

use super::dual::{self, DualOps, DualRun, Lag, Objects, RunState, RunStore};
use super::{Major, MoveProgress, MoveState};
use crate::root_helper::{call_with, Door};

// ── the run rows ────────────────────────────────────────────────────────

pub struct K2Store;

const COLS: &str = "id, database_id, db, from_major, to_major, state, objects_json, replica_identity_full_json, lag_json, move_id, \
                    actor, started_at, streaming_at, finished_at, last_error";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DualRun> {
    let state: String = r.get(5)?;
    let objects: Option<String> = r.get(6)?;
    let ri: Option<String> = r.get(7)?;
    let lag: Option<String> = r.get(8)?;
    Ok(DualRun {
        id: r.get(0)?,
        database_id: r.get(1)?,
        db: r.get(2)?,
        from: r.get::<_, i64>(3)? as Major,
        to: r.get::<_, i64>(4)? as Major,
        // The CHECK keeps it one of these; an unreadable one reads failed.
        state: RunState::parse(&state).unwrap_or(RunState::Failed),
        objects: objects.and_then(|s| serde_json::from_str::<Objects>(&s).ok()).unwrap_or_default(),
        replica_identity_full: ri.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        lag: lag.and_then(|s| serde_json::from_str::<Lag>(&s).ok()).unwrap_or_default(),
        move_id: r.get(9)?,
        actor: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        started_at: r.get(11)?,
        streaming_at: r.get(12)?,
        finished_at: r.get(13)?,
        last_error: r.get(14)?,
    })
}

fn query(sql: &str, p: &[&dyn rusqlite::ToSql]) -> Vec<DualRun> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut st) = conn.prepare(sql) else { return Vec::new() };
    st.query_map(p, from_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

const OPEN: &str = "state NOT IN ('switched', 'aborted', 'failed')";

/// PA27: a live run of this database is open (K2's own verbs refuse
/// `db_dual_running`; the boot seal and rotation skip it).
pub fn running(db: &str) -> bool {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    conn.query_row(&format!("SELECT count(*) FROM sql_dual_runs WHERE db = ?1 AND {OPEN}"), rusqlite::params![db], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .unwrap_or(false)
}

impl RunStore for K2Store {
    fn open_run(&self, db: &str) -> Option<DualRun> {
        query(&format!("SELECT {COLS} FROM sql_dual_runs WHERE db = ?1 AND {OPEN} ORDER BY started_at DESC LIMIT 1"), &[&db]).into_iter().next()
    }
    fn open_runs(&self) -> Vec<DualRun> {
        query(&format!("SELECT {COLS} FROM sql_dual_runs WHERE {OPEN} ORDER BY started_at"), &[])
    }
    fn last_run(&self, db: &str) -> Option<DualRun> {
        query(&format!("SELECT {COLS} FROM sql_dual_runs WHERE db = ?1 ORDER BY started_at DESC LIMIT 1"), &[&db]).into_iter().next()
    }
    fn run_by_object(&self, object_id: &str) -> Option<DualRun> {
        query(
            &format!("SELECT {COLS} FROM sql_dual_runs WHERE json_extract(objects_json, '$.objectId') = ?1 ORDER BY started_at DESC LIMIT 1"),
            &[&object_id],
        )
        .into_iter()
        .next()
    }
    fn insert_run(&self, r: &DualRun) -> Result<(), String> {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        conn.execute(
            "INSERT INTO sql_dual_runs (id, database_id, db, from_major, to_major, state, objects_json, replica_identity_full_json, lag_json, \
             actor, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                r.id,
                r.database_id,
                r.db,
                r.from as i64,
                r.to as i64,
                r.state.as_str(),
                serde_json::to_string(&r.objects).map_err(|e| e.to_string())?,
                serde_json::to_string(&r.replica_identity_full).map_err(|e| e.to_string())?,
                serde_json::to_string(&r.lag).map_err(|e| e.to_string())?,
                r.actor,
                r.started_at
            ],
        )
        .map(|_| ())
        .map_err(|e| format!("a live move of {} is already open ({e})", r.db))
    }
    fn save_run(&self, r: &DualRun) -> Result<(), String> {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let n = conn
            .execute(
                "UPDATE sql_dual_runs SET state = ?2, objects_json = ?3, replica_identity_full_json = ?4, lag_json = ?5, move_id = ?6, \
                 streaming_at = ?7, finished_at = ?8, last_error = ?9 WHERE id = ?1",
                rusqlite::params![
                    r.id,
                    r.state.as_str(),
                    serde_json::to_string(&r.objects).map_err(|e| e.to_string())?,
                    serde_json::to_string(&r.replica_identity_full).map_err(|e| e.to_string())?,
                    serde_json::to_string(&r.lag).map_err(|e| e.to_string())?,
                    r.move_id,
                    r.streaming_at,
                    r.finished_at,
                    r.last_error
                ],
            )
            .map_err(|e| format!("sql_dual_runs: {e}"))?;
        if n == 1 {
            Ok(())
        } else {
            Err(format!("sql_dual_runs: no row {}", r.id))
        }
    }
    fn save_run_if(&self, r: &DualRun, expect: RunState) -> Result<bool, String> {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let n = conn
            .execute(
                "UPDATE sql_dual_runs SET state = ?2, objects_json = ?3, replica_identity_full_json = ?4, lag_json = ?5, move_id = ?6, \
                 streaming_at = ?7, finished_at = ?8, last_error = ?9 WHERE id = ?1 AND state = ?10",
                rusqlite::params![
                    r.id,
                    r.state.as_str(),
                    serde_json::to_string(&r.objects).map_err(|e| e.to_string())?,
                    serde_json::to_string(&r.replica_identity_full).map_err(|e| e.to_string())?,
                    serde_json::to_string(&r.lag).map_err(|e| e.to_string())?,
                    r.move_id,
                    r.streaming_at,
                    r.finished_at,
                    r.last_error,
                    expect.as_str()
                ],
            )
            .map_err(|e| format!("sql_dual_runs: {e}"))?;
        Ok(n == 1)
    }
    fn new_password(&self) -> Result<String, String> {
        use base64::Engine as _;
        let mut b = [0u8; 32];
        // A failing system RNG stops the setup; never a weak password.
        getrandom::getrandom(&mut b).map_err(|e| format!("random: {e}"))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))
    }
    fn database_id(&self, db: &str) -> Result<String, String> {
        crate::sql::ops::move_roles(db).map(|(id, _, _)| id).map_err(|e| e.hint().to_string())
    }
    fn hold_worker(&self, db: &str) -> Result<Box<dyn std::any::Any>, String> {
        super::worker::begin(db).map(|b| Box::new(b) as Box<dyn std::any::Any>)
    }
    fn move_state(&self, move_id: &str) -> Option<MoveState> {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let pj: Option<String> = conn
            .query_row("SELECT progress_json FROM sql_moves WHERE id = ?1", rusqlite::params![move_id], |r| r.get(0))
            .ok()
            .flatten();
        pj.and_then(|s| serde_json::from_str::<MoveProgress>(&s).ok()).map(|p| p.state)
    }
}

// ── the helper's verbs ──────────────────────────────────────────────────

pub struct HelperDualOps<'a> {
    pub door: &'a dyn Door,
}

fn tok(m: Major) -> Result<String, String> {
    PgMajor::new(m).map(PgMajor::token).ok_or_else(|| format!("unknown Postgres major {m}"))
}

impl HelperDualOps<'_> {
    fn json(&self, argv: Vec<String>, stdin: Option<&[u8]>) -> Result<Value, String> {
        let out = call_with(self.door, &argv, stdin)?;
        serde_json::from_slice(&out).map_err(|_| "root helper answer is not JSON".to_string())
    }
}

impl DualOps for HelperDualOps<'_> {
    fn ready(&self) -> Result<(), String> {
        crate::sql::sysops::pgdual_gate()
    }
    fn logical_conf(&self, m: Major, wal_keep_mb: Option<u32>, if_logical: bool) -> Result<Value, String> {
        let mut argv = vec!["pg".into(), "logical-conf".into(), tok(m)?];
        match wal_keep_mb {
            None => argv.push("--dry-run".into()),
            Some(mb) => {
                argv.push("--wal-keep".into());
                argv.push(mb.to_string());
                if if_logical {
                    argv.push("--if-logical".into());
                }
            }
        }
        self.json(argv, None)
    }
    fn dump_schema(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String> {
        let v = self.json(vec!["pg".into(), "move-dump".into(), tok(from)?, tok(to)?, db.into(), tag.into(), "--schema-only".into()], None)?;
        v["bytes"].as_u64().ok_or_else(|| "move-dump answered no size".into())
    }
    fn restore_keep_force_off(&self, to: Major, db: &str, tag: &str, role: &str, password: &str) -> Result<Vec<u32>, String> {
        let body = serde_json::to_vec(&json!({ "role": role, "password": password })).map_err(|e| e.to_string())?;
        let v = self.json(vec!["pg".into(), "move-restore".into(), tok(to)?, db.into(), tag.into(), "--keep-force-off".into()], Some(&body))?;
        let oids = v["forcedOids"].as_array().ok_or("move-restore answered no forcedOids (a helper older than protocol 10?)")?;
        oids.iter().map(|o| o.as_u64().map(|n| n as u32).ok_or_else(|| "forcedOids: not a number".to_string())).collect()
    }
    fn force_rls(&self, m: Major, db: &str, oids: &[u32]) -> Result<(), String> {
        let body = serde_json::to_vec(&json!({ "oids": oids })).map_err(|e| e.to_string())?;
        self.json(vec!["pg".into(), "force-rls".into(), tok(m)?, db.into(), "on".into()], Some(&body)).map(|_| ())
    }
    fn dual(&self, op: DualOp, from: Major, to: Major, db: &str, id: &str, password: Option<&str>) -> Result<Value, String> {
        let argv = vec!["pg".into(), "dual".into(), op.token().into(), tok(from)?, tok(to)?, db.into(), id.into()];
        match password {
            Some(pw) => {
                let body = serde_json::to_vec(&json!({ "password": pw })).map_err(|e| e.to_string())?;
                self.json(argv, Some(&body))
            }
            None => self.json(argv, None),
        }
    }
    fn restart_cluster(&self, m: Major) -> Result<(), String> {
        call_with(self.door, &["pg".into(), "systemctl".into(), "restart".into(), format!("postgresql@{}-main", tok(m)?)], None).map(|_| ())
    }
    fn refresh_matviews(&self, m: Major, port: u16, db: &str, role: &str, password: &str, timeout_secs: u64) -> Result<(), String> {
        let psql = PgMajor::new(m).ok_or("unknown major")?.bin("psql");
        let pass = super::helper_ops::PassFile::new(port, db, role, password)?;
        let mut child = std::process::Command::new(&psql)
            .args(["-X", "-h", "127.0.0.1", "-p", &port.to_string(), "-U", role, "-d", db, "-v", "ON_ERROR_STOP=1", "-tA", "-w"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("PGPASSFILE", &pass.path)
            .env("PGCONNECT_TIMEOUT", "10")
            // B4: bounded (the whole refresh is one DO statement).
            .env("PGOPTIONS", format!("-c statement_timeout={}", timeout_secs.saturating_mul(1000)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("{psql}: {e}"))?;
        {
            use std::io::Write as _;
            let mut si = child.stdin.take().ok_or("psql: no stdin")?;
            si.write_all(REFRESH_SQL.as_bytes()).map_err(|e| format!("psql stdin: {e}"))?;
        }
        // And a wall clock behind the server's own timeout (a hung
        // connection never holds the write block).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs + 30);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("stopped after {timeout_secs} s (K2_DB_LIVE_REFRESH_TIMEOUT_SECS)"));
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                Err(e) => return Err(format!("psql: {e}")),
            }
        }
        let out = child.wait_with_output().map_err(|e| format!("psql: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().find(|l| l.contains("ERROR:")).unwrap_or("psql failed").chars().take(300).collect())
        }
    }
}

// ── the watch loop ──────────────────────────────────────────────────────

/// Boot (after the move recovery): the runs a restart left mid-way
/// (PA36), the orphans (PA35), then the watch loop forever.
pub fn spawn_watch() {
    if !crate::sql::sql_supported() {
        return;
    }
    let _ = std::thread::Builder::new().name("db-live-watch".into()).spawn(|| {
        // Postgres first: nothing to look at while the sidecar is down.
        loop {
            if crate::sql::supervisor::current_status().as_deref() == Some("running") && !super::worker::recovering() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        let door = crate::root_helper::RealDoor;
        let ops = super::helper_ops::HelperOps { door: &door };
        let dops = HelperDualOps { door: &door };
        let env = super::k2::K2Env { actor: "system (live watch)".into() };
        // Security review B7: the boot pass waits for a helper that speaks
        // `pgdual` (rechecked every tick: it may be updated later), never
        // skipped for good.
        let mut booted = false;
        let boot_pass = |booted: &mut bool| {
            if *booted || crate::sql::sysops::pgdual_gate().is_err() {
                return;
            }
            *booted = true;
            for line in dual::boot_runs(&ops, &dops, &env, &K2Store) {
                eprintln!("[db] live: after the restart: {line}");
            }
            let o = dual::sweep_orphans(&ops, &dops, &K2Store, true);
            if o["orphans"].as_array().is_some_and(|a| !a.is_empty()) {
                eprintln!("[db] live: orphans: {o}");
            }
        };
        boot_pass(&mut booted);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(dual::WATCH_EVERY_SECS));
            if crate::sql::supervisor::current_status().as_deref() != Some("running") {
                continue;
            }
            boot_pass(&mut booted);
            for run in K2Store.open_runs() {
                let db = run.db.clone();
                let before = run.state;
                let after = dual::tick(&ops, &dops, &env, &K2Store, run);
                if after.state != before {
                    eprintln!("[db] live {db}: {} → {}", before.as_str(), after.state.as_str());
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, state: RunState) -> DualRun {
        DualRun {
            id: id.into(),
            database_id: "d-k2store".into(),
            db: "ws_k2store".into(),
            from: 16,
            to: 17,
            state,
            objects: Objects { object_id: "0123456789ab".into(), forced_oids: vec![16500], ..Default::default() },
            replica_identity_full: vec!["public.log".into()],
            lag: Lag::default(),
            move_id: None,
            actor: "owner".into(),
            started_at: 1,
            streaming_at: None,
            finished_at: None,
            last_error: None,
        }
    }

    /// The rows over migration 0222: one open run per database, the
    /// conditional save, `running()` for K2's own verbs, the object lookup.
    #[test]
    fn the_run_rows_round_trip_and_hold_one_open_run() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let s = K2Store;
        assert!(!running("ws_k2store"));
        s.insert_run(&run("dr_a", RunState::SettingUp)).unwrap();
        assert!(running("ws_k2store"));
        assert!(s.insert_run(&run("dr_b", RunState::SettingUp)).is_err(), "one open run per database");
        let mut r = s.open_run("ws_k2store").expect("open");
        assert_eq!((r.objects.forced_oids.clone(), r.replica_identity_full.clone()), (vec![16500], vec!["public.log".to_string()]));
        r.state = RunState::Streaming;
        assert!(!s.save_run_if(&r, RunState::Syncing).unwrap(), "the row is setting_up, not syncing");
        assert!(s.save_run_if(&r, RunState::SettingUp).unwrap());
        assert_eq!(s.open_run("ws_k2store").unwrap().state, RunState::Streaming);
        assert_eq!(s.run_by_object("0123456789ab").unwrap().id, "dr_a");
        r.state = RunState::Aborted;
        s.save_run(&r).unwrap();
        assert!(!running("ws_k2store"));
        assert!(s.open_run("ws_k2store").is_none());
        assert_eq!(s.last_run("ws_k2store").unwrap().state, RunState::Aborted);
        s.insert_run(&run("dr_b", RunState::SettingUp)).unwrap();
        // B7: a run waiting for the helper is open (the CHECK takes it;
        // db_dual_running holds) and keeps its end in objects_json.
        let mut b = s.open_run("ws_k2store").expect("dr_b");
        b.state = RunState::NeedsHelper;
        b.objects.end_as = Some(RunState::Failed);
        b.objects.end_why = Some("the slot lost WAL".into());
        s.save_run(&b).unwrap();
        assert!(running("ws_k2store"));
        let back = s.open_run("ws_k2store").expect("still open");
        assert_eq!((back.state, back.objects.end_as, back.objects.end_why.as_deref()), (RunState::NeedsHelper, Some(RunState::Failed), Some("the slot lost WAL")));
        let pw = s.new_password().unwrap();
        assert_eq!(pw.len(), 43);
        k2_root_proto::pg_dual::DualSecret { password: pw }.validate().unwrap();
    }
}
