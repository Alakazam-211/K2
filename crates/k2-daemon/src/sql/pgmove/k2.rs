//! K2's side of a move (S5 on PG-A's S1): [`K2Env`] over migration 0201's
//! records and the resolver (`sql::cluster`), the vault re-point, Apps,
//! the guest-door pause and the per-database move lock, audit and inbox;
//! the `/cli/db/upgrade` route (gated by PG-A's `versions_access::decide`,
//! GET included); boot recovery before any new move.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::request::{self, Act, Scope, UpgradeRequest};
use super::{
    check_only, forget_old, recover, rollback, run_move, sweep_rollback_copies, worker, ClusterRole, DbRecord,
    ForgetOpts, Major, MoveEnv, MoveOpts, MoveProgress, MovedInfo, MoveState, RollbackOpts, SourceState, Users,
};
use crate::cli_response::CliResponse;
use crate::sql::cluster;
use crate::sql::versions_access::{self, VersionsAct, Who};

// ── the guest-door pause and the per-database move lock ─────────────────

#[derive(Default)]
struct Maint {
    /// Workspaces whose guest doors answer 503 (§6.2 step 3).
    doors: HashSet<String>,
    /// Databases moving: K2's own superuser paths (rotate, grant, revoke,
    /// drop, bind, migrate, the boot seal) refuse them (review #2 F8).
    dbs: HashSet<String>,
}

static MAINTENANCE: Mutex<Option<Maint>> = Mutex::new(None);

fn maint() -> std::sync::MutexGuard<'static, Option<Maint>> {
    MAINTENANCE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The guest doors (query, rows) of a workspace whose database is moving
/// answer `503 db_maintenance` (retry after the pause).
pub fn maintenance_response(project_id: &str) -> Option<CliResponse> {
    let on = maint().as_ref().is_some_and(|m| m.doors.contains(project_id));
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

/// Is this database in a move or rollback right now (F8)?
pub fn moving(db: &str) -> bool {
    maint().as_ref().is_some_and(|m| m.dbs.contains(db))
}

fn set_maintenance(project_id: &str, db: &str, on: bool) {
    let mut g = maint();
    let m = g.get_or_insert_with(Maint::default);
    if on {
        m.doors.insert(project_id.to_string());
        m.dbs.insert(db.to_string());
    } else {
        m.doors.remove(project_id);
        m.dbs.remove(db);
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

fn grantee_projects(database_id: &str) -> Vec<String> {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    let mut out = Vec::new();
    if let Ok(mut st) = conn.prepare("SELECT project_id FROM sql_grants WHERE database_id = ?1 ORDER BY project_id") {
        if let Ok(rows) = st.query_map(rusqlite::params![database_id], |r| r.get::<_, String>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
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
/// `k2v://` reference (V35). Owner workspace and every grantee. A vaulted
/// URL K2 can't re-mint is reported as `skipped: …` (a warning on the
/// move), never silently left behind (F11).
fn remint(db: &str) -> Result<Vec<String>, (Vec<String>, String)> {
    let (database_id, owner, _) = crate::sql::ops::move_roles(db).map_err(|e| (vec![], e.hint().to_string()))?;
    let mut projects = vec![owner];
    projects.extend(grantee_projects(&database_id));
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let mut done = Vec::new();
    for pid in projects {
        for (test, name) in [(false, "DATABASE_URL"), (true, "DATABASE_URL_TEST")] {
            if !vault_has(&pid, name) {
                continue;
            }
            let v = match crate::sql::ops::dsn_for_project_opts(ops(), &secrets, &pid, 1, test, false) {
                Ok(v) => v,
                Err(e) => {
                    done.push(format!("skipped: vault {name} ({pid}): {}", e.hint()));
                    continue;
                }
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
    /// No record = not K2's (fail closed, F9): the owner's --allow-adopted.
    fn cluster_role(&self, m: Major) -> ClusterRole {
        match cluster::for_major(m).map(|c| c.role) {
            Some(r) if r == "k2" => ClusterRole::K2,
            Some(r) if r == "foreign" => ClusterRole::Foreign,
            _ => ClusterRole::Adopted,
        }
    }
    fn refresh_clusters(&self) {
        if let Err(e) = cluster::reconcile(ops()) {
            eprintln!("[db] upgrade: clusters: {e}");
        }
    }
    fn shared_with(&self, db: &str) -> Vec<String> {
        row(db).map(|r| grantee_projects(&r.id)).unwrap_or_default()
    }
    fn logins(&self, db: &str) -> Result<Vec<(String, String)>, String> {
        crate::sql::ops::move_logins(db).map_err(|e| e.hint().to_string())
    }
    /// The owner workspace's and every grantee workspace's running Apps (F11).
    fn users(&self, db: &str) -> Users {
        let Ok(r) = row(db) else { return Users::default() };
        let mut projects = vec![r.project_id.clone()];
        projects.extend(grantee_projects(&r.id));
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let mut apps: Vec<String> = Vec::new();
        for p in &projects {
            apps.extend(
                k2_core::deploy::store::owned_app_ids(&conn, p)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|a| crate::apps::supervisor::live_instance(a).is_some()),
            );
        }
        apps.sort();
        apps.dedup();
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
            set_maintenance(&r.project_id, db, paused);
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
    /// The cluster K2 just created is K2's (`created`), never "adopted".
    fn record_cluster(&self, m: Major, port: u16) {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        if let Err(e) = cluster::record_in(&conn, &cluster::Cluster::layout(m, port, "created"), false) {
            eprintln!("[db] upgrade: record cluster {m}: {e}");
        }
    }
    fn progress(&self, p: &MoveProgress) -> Result<(), String> {
        let r = row(&p.db)?;
        let state = serde_json::to_value(p.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let pj = serde_json::to_string(p).map_err(|e| e.to_string())?;
        let kind = if p.kind == "rollback" { "rollback" } else { "move" };
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        conn.execute(
            "INSERT INTO sql_moves (id, database_id, db, from_major, to_major, method, kind, state, progress_json, actor, started_at, finished_at, last_error) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'copy', ?12, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(id) DO UPDATE SET state = ?6, progress_json = ?7, finished_at = ?10, last_error = ?11",
            rusqlite::params![p.id, r.id, p.db, p.from as i64, p.to as i64, state, pj, p.actor, p.started_at, p.finished_at, p.last_error, kind],
        )
        .map(|_| ())
        .map_err(|e| format!("sql_moves: {e}"))
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
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        429 => "429 Too Many Requests",
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

/// One move or rollback per database per hour for an agent (F7).
pub const AGENT_COOLDOWN_SECS: i64 = 3600;

fn recent_attempt(db: &str, now: i64) -> Option<i64> {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    conn.query_row(
        "SELECT max(started_at) FROM sql_moves WHERE db = ?1 AND started_at > ?2 AND state <> 'aborted'",
        rusqlite::params![db, now - AGENT_COOLDOWN_SECS],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// Shell tabs aren't the agent: a Member at a K2 shell would otherwise get
/// the agent's switch (review #2 F11). An unknown session is sandboxed
/// (fail closed, F10).
pub fn classify(who: &crate::compute_routes::Who, local: bool) -> Who {
    use crate::compute_routes::Who as C;
    match who {
        C::Shell { .. } => Who::Other,
        C::Agent { session_id, .. } if !session_known(session_id) => Who::Other,
        _ => Who::from_compute(who, local),
    }
}

fn session_known(session_id: &str) -> bool {
    if cfg!(test) {
        return true;
    }
    crate::v2_session_map::snapshot().into_iter().any(|(_, s)| s.session_id.to_string() == session_id)
}

/// `GET|POST /cli/db/upgrade`.
pub fn handle(who: Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !is_post {
        return handle_get(&who, params.get("db").map(String::as_str));
    }
    let req = match request::parse(body) {
        Ok(r) => r,
        Err(e) => return err(400, "usage", e),
    };
    if matches!(who, Who::Other | Who::Member { .. }) {
        return err(403, versions_access::OWNER_ONLY, "moving a database needs the owner, or an agent with \"Allow this agent to manage Postgres versions\"");
    }
    if !crate::sql::sql_supported() {
        return err(503, "unsupported", "Postgres runs on Linux deployments only");
    }
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
    // Boot recovery first (F6).
    if worker::recovering() {
        return err(409, "recovering", "K2 is finishing or undoing a move the last daemon was in; try again in a minute");
    }
    let is_agent = matches!(who, Who::Agent { .. });
    if is_agent && matches!(req.act, Act::Move | Act::Rollback) {
        for (db, _) in &dbs {
            if let Some(at) = recent_attempt(db, now_secs()) {
                return err(
                    429,
                    "cooldown",
                    format!("{db} moved or rolled back less than an hour ago (at {at}); an agent waits an hour between them"),
                );
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
                Err(e) => return err(409, worker::refusal_code(&e), e),
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
                Err(e) => return err(409, worker::refusal_code(&e), e),
            };
            let ops = super::helper_ops::HelperOps { door: &helper };
            let env = K2Env { actor: label.clone() };
            let owner = matches!(who, Who::Owner);
            match rollback(&ops, &env, &db, &RollbackOpts { actor: label, yes: req.yes, now: req.now, owner }) {
                Ok(p) => {
                    worker::settle(&p);
                    if p.state == MoveState::Succeeded {
                        ok(serde_json::to_value(&p).unwrap_or(Value::Null))
                    } else {
                        err(409, "rollback_stopped", p.last_error.unwrap_or_default())
                    }
                }
                Err(e) => err(409, e.split(':').next().unwrap_or("refused"), e.clone()),
            }
        }
        Act::ForgetOld => {
            let db = dbs[0].0.clone();
            // The move lock, held (F12).
            let _busy = match worker::begin(&db) {
                Ok(b) => b,
                Err(e) => return err(409, worker::refusal_code(&e), e),
            };
            let ops = super::helper_ops::HelperOps { door: &helper };
            let env = K2Env { actor: label.clone() };
            let owner = matches!(who, Who::Owner);
            match forget_old(&ops, &env, &db, &ForgetOpts { actor: label, owner, force_now: req.force_now }) {
                Ok(v) => ok(v),
                Err(e) => err(409, e.split(':').next().unwrap_or("refused"), e.clone()),
            }
        }
        Act::AcknowledgeFailed => unreachable!("handled above"),
    }
}

/// `GET /cli/db/upgrade[?db=]` (review #2 F3): the owner and Admin logins
/// see every move; an agent with the switch sees only its own databases'
/// rows; everyone else is refused. Verify's diff (policy bodies, counts)
/// is the owner's only.
fn handle_get(who: &Who, db: Option<&str>) -> CliResponse {
    let filter_project: Option<String> = match who {
        Who::Owner => None,
        Who::Admin { .. } => None,
        Who::Agent { project_id, .. } => {
            let on = versions_access::switch_on(project_id);
            if let Err(r) = versions_access::decide(who, VersionsAct::Check, Some(project_id), false, on) {
                return err(r.status, r.code, r.hint);
            }
            Some(project_id.clone())
        }
        Who::Member { .. } | Who::Other => {
            return err(403, versions_access::OWNER_ONLY, "move progress is the owner's, an Admin's, or an agent's own (with its switch)")
        }
    };
    let owner = matches!(who, Who::Owner);
    status(db, filter_project.as_deref(), owner)
}

/// The worker, the latch, recent moves (filtered to `project` when set).
pub fn status(db: Option<&str>, project: Option<&str>, owner: bool) -> CliResponse {
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    let mut moves = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT m.progress_json FROM sql_moves m LEFT JOIN sql_databases d ON d.id = m.database_id \
         WHERE (?1 IS NULL OR m.db = ?1) AND (?2 IS NULL OR d.project_id = ?2) ORDER BY m.started_at DESC LIMIT 20",
    ) {
        if let Ok(rows) = st.query_map(rusqlite::params![db, project], |r| r.get::<_, Option<String>>(0)) {
            for pj in rows.flatten().flatten() {
                if let Ok(mut v) = serde_json::from_str::<Value>(&pj) {
                    if !owner {
                        if let Some(o) = v.as_object_mut() {
                            o.remove("diff");
                        }
                    }
                    moves.push(v);
                }
            }
        }
    }
    let (busy, failed) = if project.is_none() { (worker::busy(), worker::failed()) } else { (None, None) };
    ok(json!({"busy": busy, "failed": failed, "recovering": worker::recovering(), "moves": moves}))
}

/// Once per boot, before any new move is taken (F6): the latch comes back
/// from the newest unacknowledged failed row; a move or rollback the last
/// daemon died in is undone before its switch, finished after it. Rollback
/// copies past the window are swept (F7).
pub fn recover_at_boot() {
    worker::set_recovering(true);
    {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        worker::load_latch(&conn);
    }
    let open: Vec<MoveProgress> = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let mut v = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT progress_json FROM sql_moves WHERE state IN ('queued','checking','copying','verifying','switching','interrupted') \
             ORDER BY started_at",
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
    let door = crate::root_helper::RealDoor;
    let ops = super::helper_ops::HelperOps { door: &door };
    for p in open {
        // Nothing else takes the worker while recovering; never skip a row.
        let busy = loop {
            match worker::begin_recovery(&p.db) {
                Ok(b) => break b,
                Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
            }
        };
        let env = K2Env { actor: format!("recovery ({})", p.actor) };
        let db = p.db.clone();
        let done = recover(&ops, &env, p);
        worker::settle(&done);
        drop(busy);
        eprintln!("[db] upgrade {db}: recovered after a restart → {:?}", done.state);
    }
    for d in sweep_rollback_copies(&ops, now_secs()) {
        eprintln!("[db] upgrade: dropped the rollback copy {d} (past 14 days)");
    }
    worker::set_recovering(false);
}

/// Boot: recover before anything else may move (F6). Waits for the sidecar
/// to report running; moves answer `409 recovering` meanwhile.
pub fn spawn_boot_recovery() {
    if !crate::sql::sql_supported() {
        return;
    }
    worker::set_recovering(true);
    let _ = std::thread::Builder::new().name("db-upgrade-recovery".into()).spawn(|| {
        loop {
            if crate::sql::supervisor::current_status().as_deref() == Some("running") {
                break;
            }
            // No sidecar: nothing can move either; no rows can be open.
            if crate::sql::supervisor::current_status().is_none() {
                worker::set_recovering(false);
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        if std::panic::catch_unwind(recover_at_boot).is_err() {
            eprintln!("[db] upgrade: boot recovery panicked; moves stay blocked (recovering)");
        }
    });
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
    fn the_guest_doors_pause_per_workspace_and_the_db_is_locked() {
        set_maintenance("w1", "ws_one", true);
        let r = maintenance_response("w1").expect("paused");
        assert!(r.status.starts_with("503"));
        assert!(r.body.contains("db_maintenance") && r.body.contains("retryAfter"));
        assert!(moving("ws_one"));
        assert!(maintenance_response("w2").is_none());
        assert!(!moving("ws_two"));
        set_maintenance("w1", "ws_one", false);
        assert!(maintenance_response("w1").is_none());
        assert!(!moving("ws_one"));
    }

    /// Shell tabs never get the agent's switch (F11).
    #[test]
    fn shell_tabs_are_not_the_agent() {
        use crate::compute_routes::Who as C;
        assert_eq!(classify(&C::Shell { project_id: "w".into(), session_id: "s".into() }, true), Who::Other);
        assert!(matches!(classify(&C::Agent { project_id: "w".into(), session_id: "s".into() }, true), Who::Agent { .. }));
        assert_eq!(classify(&C::Owner, false), Who::Owner);
        assert_eq!(classify(&C::Wrong, true), Who::Other);
        assert_eq!(classify(&C::Invalid, true), Who::Other);
    }

    fn seed_moves() -> (crate::test_support::TempHome, k2_core::test_env::SerialGuard) {
        let home = crate::test_support::TempHome::new();
        let g = crate::sql::sql_server_test_lock();
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, name, project) in [("d1", "ws_mine", "w-mine"), ("d2", "ws_theirs", "w-theirs")] {
            conn.execute(
                "INSERT INTO sql_databases (id, project_id, name, status, created_at) VALUES (?1, ?2, ?3, 'active', 1)",
                rusqlite::params![id, project, name],
            )
            .unwrap();
            let pj = json!({"id": format!("mv_{name}"), "db": name, "diff": ["policies public.t.p: secret qual"], "state": "aborted"});
            conn.execute(
                "INSERT INTO sql_moves (id, database_id, db, from_major, to_major, state, progress_json, actor, started_at) \
                 VALUES (?1, ?2, ?3, 16, 17, 'aborted', ?4, 'owner', 10)",
                rusqlite::params![format!("mv_{name}"), id, name, pj.to_string()],
            )
            .unwrap();
        }
        (home, g)
    }

    fn body(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).unwrap()
    }

    /// Review #2 F3: GET is gated and filtered.
    #[test]
    fn get_is_gated_and_filtered_to_the_caller() {
        let (_home, _g) = seed_moves();
        let params = HashMap::new();
        // No token, App pass, API key (Other) and Member logins: refused.
        for who in [Who::Other, Who::Member { username: "m".into() }] {
            let r = handle(who.clone(), false, b"", &params);
            assert!(r.status.starts_with("403"), "{who:?}: {}", r.status);
            assert!(!r.body.contains("ws_mine") && !r.body.contains("ws_theirs"), "{}", r.body);
        }
        // The owner sees everything, diff included.
        let v = body(&handle(Who::Owner, false, b"", &params));
        assert_eq!(v["moves"].as_array().unwrap().len(), 2);
        assert!(v.to_string().contains("secret qual"));
        // An Admin sees every row, never the diff.
        let v = body(&handle(Who::Admin { username: "a".into() }, false, b"", &params));
        assert_eq!(v["moves"].as_array().unwrap().len(), 2);
        assert!(!v.to_string().contains("secret qual"), "{v}");
        // An agent: refused without its switch, its own rows only with it.
        let agent = Who::Agent { project_id: "w-mine".into(), session_id: "s".into(), local: true, sandboxed: false };
        let r = handle(agent.clone(), false, b"", &params);
        assert!(r.status.starts_with("403"), "switch off: {}", r.status);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES ('w-mine', 'mine', '/tmp/k2-pgb-w-mine', 'mine')",
                [],
            )
            .unwrap();
            assert_eq!(
                conn.execute("UPDATE projects SET agents_can_upgrade_postgres = 1 WHERE id = 'w-mine'", []).unwrap(),
                1
            );
        }
        let v = body(&handle(agent, false, b"", &params));
        let moves = v["moves"].as_array().unwrap();
        assert_eq!(moves.len(), 1, "{v}");
        assert_eq!(moves[0]["db"], "ws_mine");
        assert!(!v.to_string().contains("ws_theirs") && !v.to_string().contains("secret qual"), "{v}");
        // A sandboxed or remote passport: refused.
        let remote = Who::Agent { project_id: "w-mine".into(), session_id: "s".into(), local: false, sandboxed: false };
        assert!(handle(remote, false, b"", &params).status.starts_with("403"));
        let boxed = Who::Agent { project_id: "w-mine".into(), session_id: "s".into(), local: true, sandboxed: true };
        assert!(handle(boxed, false, b"", &params).status.starts_with("403"));
        // POST from nobody: refused before anything.
        let r = handle(Who::Other, true, br#"{"db":"ws_mine","check":true}"#, &params);
        assert!(r.status.starts_with("403"), "{}", r.status);
    }
}
