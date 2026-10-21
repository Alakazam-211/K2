//! K2's side of a move (S5 on PG-A's S1): [`K2Env`] over migration 0201's
//! records and the resolver (`sql::cluster`), the vault re-point, Apps,
//! the guest-door pause, audit and inbox; the `/cli/db/upgrade` route
//! (gated by PG-A's `versions_access::decide`); boot recovery.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::request::{self, Act, Scope, UpgradeRequest};
use super::{
    check_only, forget_old, recover, rollback, run_move, worker, ClusterRole, DbRecord, ForgetOpts, Major, MoveEnv,
    MoveOpts, MoveProgress, MovedInfo, MoveState, RollbackOpts, SourceState, Users,
};
use crate::cli_response::CliResponse;
use crate::sql::cluster;
use crate::sql::versions_access::{self, VersionsAct, Who};

// ── the guest-door pause (§6.2 step 3) ──────────────────────────────────

static MAINTENANCE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn maint() -> std::sync::MutexGuard<'static, Option<HashSet<String>>> {
    MAINTENANCE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The guest doors (query, rows) of a workspace whose database is moving
/// answer `503 db_maintenance` (retry after the pause).
pub fn maintenance_response(project_id: &str) -> Option<CliResponse> {
    let on = maint().as_ref().is_some_and(|s| s.contains(project_id));
    on.then(|| CliResponse {
        status: "503 Service Unavailable",
        content_type: "application/json",
        body: json!({
            "error": "db_maintenance",
            "hint": "this workspace's database is moving to a newer Postgres; retry in a minute",
            "retryAfter": 60,
        })
        .to_string(),
    })
}

fn set_maintenance(project_id: &str, on: bool) {
    let mut g = maint();
    let s = g.get_or_insert_with(HashSet::new);
    if on {
        s.insert(project_id.to_string());
    } else {
        s.remove(project_id);
    }
}

// ── the env ─────────────────────────────────────────────────────────────

pub struct K2Env {
    pub actor: String,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn ops() -> &'static dyn crate::sql::sysops::SystemOps {
    crate::sql::routes::current_ops()
}

struct Row {
    id: String,
    project_id: String,
}

fn row(db: &str) -> Result<Row, String> {
    let (id, project_id, _) = crate::sql::ops::move_roles(db).map_err(|e| e.hint().to_string())?;
    Ok(Row { id, project_id })
}

fn project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get::<_, String>(0))
        .ok()
}

fn last_move_tag(conn: &rusqlite::Connection, database_id: &str) -> Option<String> {
    let pj: Option<String> = conn
        .query_row(
            "SELECT progress_json FROM sql_moves WHERE database_id = ?1 AND kind = 'move' AND state = 'succeeded' \
             ORDER BY started_at DESC LIMIT 1",
            rusqlite::params![database_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    pj.and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v["tag"].as_str().map(str::to_string))
}

fn vault_has(project_id: &str, name: &str) -> bool {
    use k2_vault::SecretStore as _;
    let Ok(v) = crate::vault::get() else { return false };
    v.meta(
        &k2_vault::Selector::user(k2_vault::Scope::workspace(project_id), name),
        &k2_vault::Actor::System("db".into()),
    )
    .is_ok()
}

/// Re-mint each vaulted `DATABASE_URL`/`_TEST` that names `db` from the
/// record (which already points at the cluster to use), under the same
/// `k2v://` reference (V35). Owner workspace and every grantee.
fn remint(db: &str) -> Result<Vec<String>, (Vec<String>, String)> {
    let (database_id, owner, _) = crate::sql::ops::move_roles(db).map_err(|e| (vec![], e.hint().to_string()))?;
    let mut projects = vec![owner];
    let grantees: Vec<String> = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare("SELECT project_id FROM sql_grants WHERE database_id = ?1") {
            if let Ok(rows) = st.query_map(rusqlite::params![database_id], |r| r.get::<_, String>(0)) {
                out.extend(rows.flatten());
            }
        }
        out
    };
    projects.extend(grantees);
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let mut done = Vec::new();
    for pid in projects {
        for (test, name) in [(false, "DATABASE_URL"), (true, "DATABASE_URL_TEST")] {
            if !vault_has(&pid, name) {
                continue;
            }
            let v = match crate::sql::ops::dsn_for_project_opts(ops(), &secrets, &pid, 1, test, false) {
                Ok(v) => v,
                // Not this workspace's test database, etc.: nothing to do.
                Err(_) => continue,
            };
            let names_db = v["dsn"].as_str().is_some_and(|d| d.ends_with(&format!("/{db}")) || d.contains(&format!("/{db}?")));
            if !names_db {
                continue;
            }
            let r = crate::sql::routes::dsn_ref_for_move(&pid, &v, test);
            if !r.status.starts_with("200") {
                return Err((done, format!("vault {name} for {pid}: {}", r.body)));
            }
            done.push(format!("vault {name} ({pid})"));
        }
    }
    Ok(done)
}

impl MoveEnv for K2Env {
    fn now(&self) -> i64 {
        now_secs()
    }
    fn sleep_secs(&self, s: u64) {
        std::thread::sleep(std::time::Duration::from_secs(s));
    }
    fn record(&self, db: &str) -> Result<DbRecord, String> {
        let (database_id, project_id, k2_roles) = crate::sql::ops::move_roles(db).map_err(|e| e.hint().to_string())?;
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let (major, moved_from, moved_at, until, state): (Option<i64>, Option<i64>, Option<i64>, Option<i64>, String) = conn
            .query_row(
                "SELECT cluster_major, moved_from_major, moved_at, rollback_until, source_state FROM sql_databases WHERE id = ?1",
                rusqlite::params![database_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(|e| format!("database record: {e}"))?;
        let major = match major {
            Some(m) if m > 0 => m as u32,
            _ => cluster::primary_in(&conn).major,
        };
        Ok(DbRecord {
            name: db.to_string(),
            project_id,
            major,
            k2_roles,
            moved_from: moved_from.map(|m| m as u32),
            moved_at,
            rollback_until: until,
            source_state: match state.as_str() {
                "kept" => SourceState::Kept,
                "forgotten" => SourceState::Forgotten,
                _ => SourceState::None,
            },
            move_tag: last_move_tag(&conn, &database_id),
        })
    }
    fn cluster_role(&self, m: Major) -> ClusterRole {
        match cluster::for_major(m).map(|c| c.role) {
            Some(r) if r == "adopted" => ClusterRole::Adopted,
            Some(r) if r == "foreign" => ClusterRole::Foreign,
            _ => ClusterRole::K2,
        }
    }
    fn logins(&self, db: &str) -> Result<Vec<(String, String)>, String> {
        crate::sql::ops::move_logins(db).map_err(|e| e.hint().to_string())
    }
    fn users(&self, db: &str) -> Users {
        let Ok(r) = row(db) else { return Users::default() };
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let apps: Vec<String> = k2_core::deploy::store::owned_app_ids(&conn, &r.project_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| crate::apps::supervisor::live_instance(a).is_some())
            .collect();
        Users { guest_doors: apps.len() as u32, apps, literal_dsns: vec![] }
    }
    fn notice(&self, db: &str, text: &str) {
        if let Ok(r) = row(db) {
            if let Some(p) = project_path(&r.project_id) {
                let _ = k2_core::inbox::compose(std::path::Path::new(&p), "Database maintenance", text, Some("high"), Some("db"), Some("db"));
            }
        }
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::SyncSettings,
            json!({ "dbMaintenance": { "db": db, "text": text } }),
        );
    }
    fn clear_notice(&self, db: &str) {
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "dbMaintenance": { "db": db, "done": true } }));
    }
    fn pause_doors(&self, db: &str, paused: bool) {
        if let Ok(r) = row(db) {
            set_maintenance(&r.project_id, paused);
        }
    }
    fn stop_apps(&self, apps: &[String]) -> Result<Vec<String>, String> {
        let mut stopped = Vec::new();
        for id in apps {
            let app = {
                let k2db = k2_core::db::shared();
                let conn = k2db.lock();
                k2_core::deploy::store::get_app(&conn, id).ok().flatten()
            };
            let Some(app) = app else { continue };
            if crate::apps::supervisor::live_instance(id).is_none() {
                continue;
            }
            crate::apps::supervisor::stop_app(&app, &self.actor).map_err(|e| format!("{id}: {e:?}"))?;
            stopped.push(id.clone());
        }
        Ok(stopped)
    }
    fn start_apps(&self, apps: &[String]) -> Vec<String> {
        let mut failed = Vec::new();
        for id in apps {
            let app = {
                let k2db = k2_core::db::shared();
                let conn = k2db.lock();
                k2_core::deploy::store::get_app(&conn, id).ok().flatten()
            };
            let Some(app) = app else {
                failed.push(id.clone());
                continue;
            };
            let Some(seed) = app.live_seed.clone() else {
                failed.push(id.clone());
                continue;
            };
            crate::apps::supervisor::reset_backoff(id);
            if crate::apps::supervisor::begin(&app, &seed, k2_core::deploy::store::KIND_START, &self.actor, Some("k2 db upgrade")).is_err() {
                failed.push(id.clone());
            }
        }
        failed
    }
    fn rerun_k2_setup(&self, _m: Major, db: &str) -> Result<(), String> {
        crate::sql::ops::move_rerun_k2_setup(ops(), db).map_err(|e| e.hint().to_string())
    }
    fn verify_k2(&self, _m: Major, db: &str) -> Result<(), String> {
        // The agent's own login through the resolver (the door guests use).
        let logins = self.logins(db)?;
        let (role, pw) = logins.first().ok_or("no agent login")?;
        let c = cluster::for_db(db)?;
        super::helper_ops::role_login(c.major, c.port, db, role, pw)
    }
    fn repoint(&self, db: &str, _from_port: u16, _to_port: u16) -> Result<Vec<String>, (Vec<String>, String)> {
        remint(db)
    }
    fn unpoint(&self, db: &str, _done: &[String], _from_port: u16, _to_port: u16) -> Result<(), String> {
        // The record already points back: re-mint from it.
        remint(db).map(|_| ()).map_err(|(_, e)| e)
    }
    fn set_major(&self, db: &str, m: Major) -> Result<(), String> {
        let r = row(db)?;
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        cluster::set_db_major_in(&conn, &r.id, m)
    }
    fn set_moved(&self, db: &str, info: Option<&MovedInfo>) -> Result<(), String> {
        let r = row(db)?;
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let state = match info.map(|i| i.source_state) {
            Some(SourceState::Kept) => "kept",
            Some(SourceState::Forgotten) => "forgotten",
            _ => "none",
        };
        conn.execute(
            "UPDATE sql_databases SET moved_from_major = ?2, moved_at = ?3, rollback_until = ?4, source_state = ?5 WHERE id = ?1",
            rusqlite::params![r.id, info.map(|i| i.from as i64), info.map(|i| i.at), info.map(|i| i.until), state],
        )
        .map(|_| ())
        .map_err(|e| format!("record the move: {e}"))
    }
    fn record_cluster(&self, _m: Major, _port: u16) {
        if let Err(e) = cluster::reconcile(ops()) {
            eprintln!("[db] upgrade: clusters: {e}");
        }
    }
    fn progress(&self, p: &MoveProgress) {
        let Ok(r) = row(&p.db) else { return };
        let state = serde_json::to_value(p.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let pj = serde_json::to_string(p).unwrap_or_default();
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let _ = conn.execute(
            "INSERT INTO sql_moves (id, database_id, db, from_major, to_major, method, kind, state, progress_json, actor, started_at, finished_at, last_error) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'copy', 'move', ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(id) DO UPDATE SET state = ?6, progress_json = ?7, finished_at = ?10, last_error = ?11",
            rusqlite::params![p.id, r.id, p.db, p.from as i64, p.to as i64, state, pj, p.actor, p.started_at, p.finished_at, p.last_error],
        );
    }
    fn audit(&self, act: &str, line: Value) {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(act, &self.actor, line.to_string(), "-", "-", "api"));
    }
    fn inbox(&self, title: &str, body: &str) {
        let db = title.split_whitespace().nth(1).unwrap_or_default().to_string();
        if let Some(p) = row(&db).ok().and_then(|r| project_path(&r.project_id)) {
            let _ = k2_core::inbox::compose(std::path::Path::new(&p), title, body, Some("normal"), Some("db"), Some("db"));
        }
    }
}

// ── the route ───────────────────────────────────────────────────────────

fn err(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    let s = match status {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    };
    CliResponse { status: s, content_type: "application/json", body: json!({"error": code, "hint": hint.into()}).to_string() }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// The `versions_access` acts a request needs (every one must pass).
pub fn acts_for(r: &UpgradeRequest) -> Vec<VersionsAct> {
    let mut v = vec![match r.act {
        Act::Check => VersionsAct::Check,
        Act::Move => VersionsAct::Move,
        Act::Rollback => VersionsAct::Rollback,
        Act::ForgetOld => VersionsAct::ForgetOld,
        Act::AcknowledgeFailed => VersionsAct::AcknowledgeFailed,
    }];
    if r.now {
        v.push(VersionsAct::Now);
    }
    if r.allow_adopted {
        v.push(VersionsAct::AllowAdopted);
    }
    if r.scope == Some(Scope::All) && r.act != Act::Check {
        v.push(VersionsAct::All);
    }
    v
}

/// The K2 databases a scope names: (db, owner project).
fn databases_for(scope: &Scope, to: Major) -> Result<Vec<(String, String)>, CliResponse> {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    let q = |sql: &str, p: &str| -> Vec<(String, String)> {
        conn.prepare(sql)
            .ok()
            .map(|mut st| {
                st.query_map(rusqlite::params![p], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map(|rows| rows.flatten().collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    };
    let v = match scope {
        Scope::Db(n) => q("SELECT name, project_id FROM sql_databases WHERE name = ?1 AND status = 'active'", n),
        Scope::App(id) => {
            let app = k2_core::deploy::store::get_app(&conn, id).ok().flatten().ok_or_else(|| err(404, "not_found", format!("no App {id}")))?;
            q("SELECT name, project_id FROM sql_databases WHERE project_id = ?1 AND status = 'active'", &app.owner_workspace)
        }
        Scope::All => q(
            "SELECT name, project_id FROM sql_databases WHERE status = 'active' AND coalesce(cluster_major, 0) < ?1 ORDER BY name",
            &to.to_string(),
        ),
    };
    if v.is_empty() {
        return Err(err(404, "not_found", "no K2 database in that scope"));
    }
    Ok(v)
}

fn helper_ok() -> bool {
    crate::sql::sysops::pgmajor_gate().is_ok()
}

/// `GET|POST /cli/db/upgrade`.
pub fn handle(who: Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !crate::sql::sql_supported() {
        return err(503, "unsupported", "Postgres runs on Linux deployments only");
    }
    if !is_post {
        return status(params.get("db").map(String::as_str));
    }
    let req = match request::parse(body) {
        Ok(r) => r,
        Err(e) => return err(400, "usage", e),
    };
    let label = who.label();
    if req.act == Act::AcknowledgeFailed {
        if let Err(r) = versions_access::decide(&who, VersionsAct::AcknowledgeFailed, None, false, false) {
            return err(r.status, r.code, r.hint);
        }
        return ok(json!({"acknowledged": worker::acknowledge()}));
    }
    let to = req.to.unwrap_or(k2_root_proto::pg_major::NEW_INSTALL);
    let dbs = match databases_for(req.scope.as_ref().expect("parse requires a scope"), to) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let agent_switch = match &who {
        Who::Agent { project_id, .. } => versions_access::switch_on(project_id),
        _ => false,
    };
    for (_, project) in &dbs {
        for act in acts_for(&req) {
            if let Err(r) = versions_access::decide(&who, act, Some(project), false, agent_switch) {
                return err(r.status, r.code, r.hint);
            }
        }
    }
    let opts = MoveOpts { to: Some(to), actor: label.clone(), now: req.now, allow_adopted: req.allow_adopted, helper_ok: helper_ok() };
    let helper = crate::root_helper::RealDoor;
    match req.act {
        Act::Check => {
            let ops = super::helper_ops::HelperOps { door: &helper };
            let env = K2Env { actor: label };
            let mut reports = Vec::new();
            for (db, _) in &dbs {
                match check_only(&ops, &env, db, &opts) {
                    Ok(r) => reports.push(serde_json::to_value(&r).unwrap_or(Value::Null)),
                    Err(e) => reports.push(json!({"db": db, "error": e})),
                }
            }
            ok(json!({"check": reports}))
        }
        Act::Move => {
            let first = dbs[0].0.clone();
            let busy = match worker::begin(&first) {
                Ok(b) => b,
                Err(e) => return err(409, if e.starts_with("busy") { "upgrade_in_progress" } else { "failed_latched" }, e),
            };
            let names: Vec<String> = dbs.iter().map(|d| d.0.clone()).collect();
            let started = names.clone();
            let spawned = std::thread::Builder::new().name("db-upgrade".into()).spawn(move || {
                let _busy = busy;
                let door = crate::root_helper::RealDoor;
                let ops = super::helper_ops::HelperOps { door: &door };
                let env = K2Env { actor: opts.actor.clone() };
                // --all: one database at a time, stop at the first failure.
                for db in names {
                    match run_move(&ops, &env, &db, &opts) {
                        Ok(p) => {
                            worker::settle(&p);
                            if p.state != MoveState::Succeeded {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("[db] upgrade {db}: {e}");
                            break;
                        }
                    }
                }
            });
            if let Err(e) = spawned {
                return err(500, "spawn_failed", e.to_string());
            }
            ok(json!({"started": started, "to": to, "poll": "GET /cli/db/upgrade?db=<name>"}))
        }
        Act::Rollback => {
            let db = dbs[0].0.clone();
            let _busy = match worker::begin(&db) {
                Ok(b) => b,
                Err(e) => return err(409, "upgrade_in_progress", e),
            };
            let ops = super::helper_ops::HelperOps { door: &helper };
            let env = K2Env { actor: label.clone() };
            match rollback(&ops, &env, &db, &RollbackOpts { actor: label, yes: req.yes, now: req.now }) {
                Ok(v) => ok(v),
                Err(e) => err(409, e.split(':').next().unwrap_or("refused"), e.clone()),
            }
        }
        Act::ForgetOld => {
            let db = dbs[0].0.clone();
            let busy = worker::busy().is_some();
            let ops = super::helper_ops::HelperOps { door: &helper };
            let env = K2Env { actor: label.clone() };
            let owner = matches!(who, Who::Owner);
            match forget_old(&ops, &env, &db, &ForgetOpts { actor: label, owner, force_now: req.force_now, move_busy: busy }) {
                Ok(v) => ok(v),
                Err(e) => err(409, e.split(':').next().unwrap_or("refused"), e.clone()),
            }
        }
        Act::AcknowledgeFailed => unreachable!("handled above"),
    }
}

/// `GET /cli/db/upgrade[?db=]`: the worker, the latch, recent moves.
pub fn status(db: Option<&str>) -> CliResponse {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    let mut moves = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT progress_json FROM sql_moves WHERE (?1 IS NULL OR db = ?1) ORDER BY started_at DESC LIMIT 20",
    ) {
        if let Ok(rows) = st.query_map(rusqlite::params![db], |r| r.get::<_, Option<String>>(0)) {
            for pj in rows.flatten().flatten() {
                if let Ok(v) = serde_json::from_str::<Value>(&pj) {
                    moves.push(v);
                }
            }
        }
    }
    ok(json!({"busy": worker::busy(), "failed": worker::failed(), "moves": moves}))
}

/// Once per boot: a move the last daemon died in is undone before its
/// switch, finished after it (§13 last row).
pub fn recover_at_boot() {
    let open: Vec<MoveProgress> = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let mut v = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT progress_json FROM sql_moves WHERE state IN ('queued','checking','copying','verifying','switching','interrupted')",
        ) {
            if let Ok(rows) = st.query_map([], |r| r.get::<_, Option<String>>(0)) {
                for pj in rows.flatten().flatten() {
                    if let Ok(p) = serde_json::from_str::<MoveProgress>(&pj) {
                        v.push(p);
                    }
                }
            }
        }
        v
    };
    for p in open {
        let Ok(busy) = worker::begin(&p.db) else { continue };
        let door = crate::root_helper::RealDoor;
        let ops = super::helper_ops::HelperOps { door: &door };
        let env = K2Env { actor: format!("recovery ({})", p.actor) };
        let db = p.db.clone();
        let done = recover(&ops, &env, p);
        worker::settle(&done);
        drop(busy);
        eprintln!("[db] upgrade {db}: recovered after a restart → {:?}", done.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_map_onto_the_versions_gate() {
        let p = |s: &str| request::parse(s.as_bytes()).unwrap();
        assert_eq!(acts_for(&p(r#"{"db":"ws_a","check":true}"#)), vec![VersionsAct::Check]);
        assert_eq!(acts_for(&p(r#"{"db":"ws_a"}"#)), vec![VersionsAct::Move]);
        assert_eq!(acts_for(&p(r#"{"db":"ws_a","now":true}"#)), vec![VersionsAct::Move, VersionsAct::Now]);
        assert_eq!(acts_for(&p(r#"{"db":"ws_a","allowAdopted":true}"#)), vec![VersionsAct::Move, VersionsAct::AllowAdopted]);
        assert_eq!(acts_for(&p(r#"{"all":true}"#)), vec![VersionsAct::Move, VersionsAct::All]);
        assert_eq!(acts_for(&p(r#"{"all":true,"check":true}"#)), vec![VersionsAct::Check]);
        assert_eq!(acts_for(&p(r#"{"db":"ws_a","rollback":true,"yes":true}"#)), vec![VersionsAct::Rollback]);
        assert_eq!(acts_for(&p(r#"{"db":"ws_a","forgetOld":true}"#)), vec![VersionsAct::ForgetOld]);
        // An agent with the switch on: own database ok, owner-only acts refused.
        let agent = Who::Agent { project_id: "w".into(), session_id: "s".into(), local: true, sandboxed: false };
        for a in acts_for(&p(r#"{"db":"ws_a"}"#)) {
            assert!(versions_access::decide(&agent, a, Some("w"), false, true).is_ok());
        }
        let refused = |body: &str| {
            acts_for(&p(body)).into_iter().any(|a| versions_access::decide(&agent, a, Some("w"), false, true).is_err())
        };
        assert!(refused(r#"{"db":"ws_a","now":true}"#));
        assert!(refused(r#"{"db":"ws_a","forgetOld":true}"#));
        assert!(refused(r#"{"all":true}"#));
        assert!(versions_access::decide(&agent, VersionsAct::Move, Some("other"), false, true).is_err());
    }

    #[test]
    fn the_guest_doors_pause_per_workspace() {
        set_maintenance("w1", true);
        let r = maintenance_response("w1").expect("paused");
        assert!(r.status.starts_with("503"));
        assert!(r.body.contains("db_maintenance") && r.body.contains("retryAfter"));
        assert!(maintenance_response("w2").is_none());
        set_maintenance("w1", false);
        assert!(maintenance_response("w1").is_none());
    }
}
