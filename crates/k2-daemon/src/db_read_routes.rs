//! Read-only access to another workspace's database, on a card (0.46.17).
//!
//! An agent that needs to read an App's Postgres database owned by another
//! workspace (a verifier, a report) had no way to find it or ask for it:
//! `k2 db list` shows only its own and granted databases, and the only path
//! was a person running `k2 db grant <ws> --db <name> --level read`.
//!
//! - **`GET /cli/db/browse [?app=<app>]`** (`k2 db browse`, `k2 db list
//!   --all`): every database on this server an agent could ask for, by name,
//!   id, owner workspace and the Apps that use it, and what the caller holds
//!   on it. Never a DSN, a credential, a size or a row count.
//! - **`POST /cli/db/request-read {db | app [+db], why?, assign?, connect?,
//!   dryRun?}`** (`k2 db request-read`, `k2 app db connect <app>
//!   --read-only`): an agent's passport only. It names the database (by
//!   name or id, or by the App that uses it), and files the `db.read`
//!   permission card (`permission::actions::db_read`) on the agent's Thread
//!   for an owner or admin. Allow runs `k2 db grant <reader> --db <it>
//!   --level read` through `/cli/db/grant`'s own entry, as that person;
//!   Deny changes nothing. A database the caller already reads answers at
//!   once; with `connect` (the `--read-only` App verb) the answer carries
//!   its read-only DSN, without the password, as `k2 db dsn --db` does.
//!
//! The card's `db=` value is the database's name when that name is unique
//! on the server, else its id; the card's words also carry the id, so a
//! name that comes to mean another database between the ask and the click
//! makes the card stale, never a grant on the other one.

use std::collections::HashMap;

use k2_core::brand::CLI_NAME;
use serde_json::{json, Map, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::permission::answer::err;

pub const BROWSE: &str = "/cli/db/browse";
pub const REQUEST_READ: &str = "/cli/db/request-read";
/// The card's action id (`permission::actions::db_read`).
pub const ACTION: &str = "db.read";

/// Error codes (the agent reads them; exit codes in the CLI).
pub mod codes {
    pub const USAGE: &str = "usage";
    /// No active database by that name or id.
    pub const NOT_FOUND: &str = "not_found";
    /// Two or more databases have that name: name one by its id.
    pub const AMBIGUOUS: &str = "db_ambiguous";
    /// The caller's workspace owns it: it reads and writes it already.
    pub const OWN: &str = "db_read_own";
    /// No App by that id on this server.
    pub const APP_NOT_FOUND: &str = "app_not_found";
    /// The App uses no database this server has a record of.
    pub const APP_NO_DB: &str = "app_db_none";
    /// The App uses more than one: name one with --db.
    pub const APP_DB_AMBIGUOUS: &str = "app_db_ambiguous";
    /// `--db` names a database the App doesn't use.
    pub const APP_DB_NOT_ITS: &str = "app_db_not_its";
    /// The agent can't ask with cards (a switch is off).
    pub const CARDS_OFF: &str = "permission_cards_off";
    /// A person called an agent's ask.
    pub const AGENT_VERB: &str = "agent_verb";
    /// `--read-only` on a database the caller may write: no read-only login.
    pub const READ_ONLY_UNAVAILABLE: &str = "read_only_unavailable";
}

/// One active database on this server (never a test or dropped one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Db {
    pub id: String,
    pub name: String,
    pub owner_id: String,
}

impl Db {
    pub(crate) fn owner_handle(&self) -> String {
        crate::permission::present::workspace_handle(&self.owner_id)
    }
}

/// A refusal: (HTTP status, code, hint).
pub(crate) type Refusal = (u16, &'static str, String);

fn refuse(r: Refusal) -> CliResponse {
    err(r.0, r.1, r.2)
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> Result<T, Refusal> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn).map_err(|e| (500, "db", format!("reading the database list: {e}")))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Db> {
    Ok(Db { id: r.get(0)?, name: r.get(1)?, owner_id: r.get(2)? })
}

/// Every active database, oldest first.
pub(crate) fn all() -> Result<Vec<Db>, Refusal> {
    with_conn(|c| {
        let mut st = c.prepare("SELECT id, name, project_id FROM sql_databases WHERE status = 'active' ORDER BY created_at, name")?;
        let rows = st.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// The active database whose id, or else whose name, is `spec`.
pub(crate) fn find(spec: &str) -> Result<Db, Refusal> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err((400, codes::USAGE, format!("name the database: --db <name> ({CLI_NAME} db browse lists them)")));
    }
    let (by_id, by_name) = with_conn(|c| {
        let by_id = c
            .query_row("SELECT id, name, project_id FROM sql_databases WHERE id = ?1 AND status = 'active'", [spec], row)
            .ok();
        let mut st = c.prepare("SELECT id, name, project_id FROM sql_databases WHERE name = ?1 AND status = 'active' ORDER BY created_at")?;
        let by_name = st.query_map([spec], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((by_id, by_name))
    })?;
    if let Some(d) = by_id {
        return Ok(d);
    }
    match by_name.len() {
        0 => Err((404, codes::NOT_FOUND, format!("this server has no database {spec} ({CLI_NAME} db browse lists them)"))),
        1 => Ok(by_name.into_iter().next().expect("one row")),
        n => {
            let owners: Vec<String> = by_name.iter().map(|d| format!("{} ({})", d.id, d.owner_handle())).collect();
            Err((
                409,
                codes::AMBIGUOUS,
                format!("{n} databases are named {spec}: {}. Name one by its id ({CLI_NAME} db browse shows them)", owners.join(", ")),
            ))
        }
    }
}

/// How a card names `db`: its name when no other active database has it,
/// else its id.
pub(crate) fn card_value(db: &Db) -> String {
    match find(&db.name) {
        Ok(d) if d.id == db.id => db.name.clone(),
        _ => db.id.clone(),
    }
}

/// The Apps that use `db`: connected to it (`k2 app db connect`), or the
/// one `--new` made it for (`client_id` `app-<app>`).
pub(crate) fn apps_of(db: &Db) -> Vec<String> {
    let mut out: Vec<String> = with_conn(|c| {
        let mut st = c.prepare(
            "SELECT DISTINCT app_id FROM app_db_connections \
             WHERE database_id = ?1 OR (database_id IS NULL AND database_name = ?2 AND database_project = ?3)",
        )?;
        let mut apps = st.query_map(rusqlite::params![db.id, db.name, db.owner_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let made_for: Option<String> = c
            .query_row("SELECT client_id FROM sql_databases WHERE id = ?1", [&db.id], |r| r.get::<_, Option<String>>(0))
            .ok()
            .flatten();
        if let Some(app) = made_for.and_then(|cid| cid.strip_prefix("app-").map(str::to_string)) {
            if k2_core::deploy::store::get_app(c, &app).ok().flatten().is_some() {
                apps.push(app);
            }
        }
        Ok(apps)
    })
    .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// The databases App `app` uses (see [`apps_of`]).
pub(crate) fn dbs_of_app(app: &str) -> Result<Vec<Db>, Refusal> {
    let mut out: Vec<Db> = with_conn(|c| {
        let mut st = c.prepare(
            "SELECT d.id, d.name, d.project_id FROM app_db_connections a JOIN sql_databases d \
               ON (d.id = a.database_id OR (a.database_id IS NULL AND d.name = a.database_name AND d.project_id = a.database_project)) \
             WHERE a.app_id = ?1 AND d.status = 'active' \
             UNION \
             SELECT id, name, project_id FROM sql_databases WHERE client_id = 'app-' || ?1 AND status = 'active'",
        )?;
        let rows = st.query_map([app], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

/// What workspace `ws` holds on `db`: `owner`, `write`, `read`, or nothing.
pub(crate) fn access_of(ws: &str, db: &Db) -> Option<&'static str> {
    if ws == db.owner_id {
        return Some("owner");
    }
    let level: Option<String> = with_conn(|c| {
        Ok(c.query_row("SELECT level FROM sql_grants WHERE database_id = ?1 AND project_id = ?2", rusqlite::params![db.id, ws], |r| r.get(0)).ok())
    })
    .ok()
    .flatten();
    match level.as_deref() {
        Some("write") => Some("write"),
        Some(_) => Some("read"),
        None => None,
    }
}

/// `who owns it`, as the card says it: `ws`, `ws, App jobs` or `ws, Apps a, b`.
pub(crate) fn owner_words(db: &Db) -> String {
    let apps = apps_of(db);
    match apps.len() {
        0 => db.owner_handle(),
        1 => format!("{}, App {}", db.owner_handle(), apps[0]),
        _ => format!("{}, Apps {}", db.owner_handle(), apps.join(", ")),
    }
}

// ── GET /cli/db/browse ───────────────────────────────────────────────

/// `{databases: [{id, name, owner, apps, yours}], hint}`; `yours` is what the
/// caller's workspace holds (`owner`, `write`, `read`, or null; always null
/// for a person). `app`: only that App's databases.
pub fn handle_browse(params: &HashMap<String, String>) -> CliResponse {
    let viewer = crate::caller_workspace::principal_from_params(params)
        .or_else(crate::caller_workspace::request_principal)
        .map(|p| p.workspace_uuid);
    let list = match params.get("app").map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
        Some(app) => {
            if crate::apps::reach::app_row(&app).is_none() {
                return refuse((404, codes::APP_NOT_FOUND, format!("this server has no App {app} ({CLI_NAME} db browse lists the Apps that use a database)")));
            }
            dbs_of_app(&app)
        }
        None => all(),
    };
    let list = match list {
        Ok(l) => l,
        Err(r) => return refuse(r),
    };
    let rows: Vec<Value> = list
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "owner": d.owner_handle(),
                "apps": apps_of(d),
                "yours": viewer.as_deref().and_then(|v| access_of(v, d)),
            })
        })
        .collect();
    CliResponse::ok_json(
        json!({
            "ok": true,
            "databases": rows,
            "hint": format!(
                "names only: no DSN, credential or data. To read one you don't: {CLI_NAME} db request-read --db <name> (a card for an owner or admin); once allowed, {CLI_NAME} db dsn --db <name>"
            ),
        })
        .to_string(),
    )
}

// ── POST /cli/db/request-read ────────────────────────────────────────

fn str_of<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn truthy(m: &Map<String, Value>, k: &str) -> bool {
    match m.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true" | "yes"),
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

/// The database the ask names: `db` alone, `app` alone (its one database),
/// or both (that App's database of that name or id).
pub(crate) fn target(db: Option<&str>, app: Option<&str>) -> Result<Db, Refusal> {
    let Some(app) = app.map(str::to_ascii_lowercase) else {
        return match db {
            Some(d) => find(d),
            None => Err((
                400,
                codes::USAGE,
                format!("name the database: --db <name> ({CLI_NAME} db browse lists them), or --app <app> for the one an App uses"),
            )),
        };
    };
    if crate::apps::reach::app_row(&app).is_none() {
        return Err((404, codes::APP_NOT_FOUND, format!("this server has no App {app} ({CLI_NAME} db browse lists the Apps that use a database)")));
    }
    let dbs = dbs_of_app(&app)?;
    let names = dbs.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join(", ");
    if let Some(spec) = db.map(str::trim) {
        return dbs.iter().find(|d| d.id == spec || d.name == spec).cloned().ok_or_else(|| {
            (
                404,
                codes::APP_DB_NOT_ITS,
                if dbs.is_empty() {
                    format!("App {app} uses no database this server has a record of")
                } else {
                    format!("App {app} doesn't use a database {spec}: it uses {names}")
                },
            )
        });
    }
    match dbs.len() {
        0 => Err((
            404,
            codes::APP_NO_DB,
            format!("App {app} uses no database this server has a record of (it connects with {CLI_NAME} app db connect): name one with --db ({CLI_NAME} db browse lists them)"),
        )),
        1 => Ok(dbs.into_iter().next().expect("one")),
        n => Err((409, codes::APP_DB_AMBIGUOUS, format!("App {app} uses {n} databases: {names}. Name one: --db <name>"))),
    }
}

/// The caller's read-only DSN for `db` (it holds a read grant), without the
/// password: what `k2 db dsn --db <name>` answers.
fn read_only_dsn(ws: &str, db: &Db) -> CliResponse {
    let path: String = with_conn(|c| Ok(c.query_row("SELECT path FROM projects WHERE id = ?1", [ws], |r| r.get(0)).unwrap_or_default())).unwrap_or_default();
    let cap = k2_core::workspace::settings::db_active_cap_for_path(&path);
    let secrets = crate::sql::secrets::FileSecretStore::default();
    match crate::sql::ops::dsn_for_db(crate::sql::routes::current_ops(), &secrets, ws, cap, &db.id) {
        Ok(v) => {
            let mut v = crate::sql::routes::dsn_answer(v, false);
            v["status"] = json!("read_only");
            v["readOnly"] = json!(true);
            v["owner"] = json!(owner_words(db));
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => {
            let status = e.status().get(..3).and_then(|s| s.parse().ok()).unwrap_or(500);
            err(status, e.code(), e.hint())
        }
    }
}

/// `POST /cli/db/request-read`. `who` is the caller as its door classified
/// it; only an agent's passport asks.
pub fn handle_request_read(who: &Who, is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let (ws, session) = match who {
        Who::Agent { project_id, session_id } | Who::Shell { project_id, session_id } => (project_id.clone(), session_id.clone()),
        Who::Owner | Who::Login { .. } => {
            return err(
                403,
                codes::AGENT_VERB,
                format!(
                    "{CLI_NAME} db request-read is an agent's ask. A person gives a workspace read access with {CLI_NAME} db grant <workspace> --db <name> --level read"
                ),
            )
        }
        Who::Wrong | Who::Invalid => return err(403, "forbidden", "an agent's passport asks for read access; this credential can't"),
    };
    let b: Map<String, Value> = if body.iter().all(u8::is_ascii_whitespace) {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(m)) => m,
            _ => return err(400, codes::USAGE, "the body is a JSON object: {\"db\": \"<name>\"} or {\"app\": \"<app>\"}"),
        }
    };
    let connect = truthy(&b, "connect");
    let db = match target(str_of(&b, "db"), str_of(&b, "app")) {
        Ok(d) => d,
        Err(r) => return refuse(r),
    };
    let me = crate::permission::present::workspace_handle(&ws);
    match access_of(&ws, &db) {
        Some("owner") => {
            return err(
                409,
                codes::OWN,
                format!(
                    "{me} owns {}: it reads and writes it already ({CLI_NAME} db dsn --db {}); there is no read-only login for a workspace's own database",
                    db.name, db.name
                ),
            )
        }
        Some("read") if connect => return read_only_dsn(&ws, &db),
        Some("write") if connect => {
            return err(
                409,
                codes::READ_ONLY_UNAVAILABLE,
                format!(
                    "{me} may write {}, so there is no read-only login for it here: {CLI_NAME} db dsn --db {} is its read and write one",
                    db.name, db.name
                ),
            )
        }
        Some(level) => {
            return CliResponse::ok_json(
                json!({
                    "status": "already_done",
                    "db": db.name,
                    "dbId": db.id,
                    "level": level,
                    "text": format!("{me} can already read {} ({level}): {CLI_NAME} db dsn --db {} gives its DSN. Nothing to ask.", db.name, db.name),
                })
                .to_string(),
            )
        }
        None => {}
    }
    if !k2_core::permission::gate::enabled_for(&ws) {
        return err(
            403,
            codes::CARDS_OFF,
            format!(
                "{me} can't ask on a card yet: an owner or admin turns on \"Let this agent ask for permission\" (Settings → Workspaces / Agents → {me} → Permissions, or {CLI_NAME} permission agent {me} on), or gives it read access themselves: {CLI_NAME} db grant {me} --db {} --level read",
                db.name
            ),
        );
    }
    let mut ask = Map::new();
    ask.insert("action".into(), json!(ACTION));
    ask.insert("params".into(), json!({ "db": card_value(&db) }));
    let why = str_of(&b, "why").map(str::to_string).unwrap_or_else(|| format!("to read the {} database", db.name));
    ask.insert("why".into(), json!(why));
    // `--assign` (one owner or approver) rides the cards' `--to`.
    if let Some(to) = b.get("assign").filter(|v| !v.is_null()).or_else(|| b.get("to")) {
        ask.insert("to".into(), to.clone());
    }
    if truthy(&b, "dryRun") {
        ask.insert("dryRun".into(), json!(true));
    }
    let r = crate::permission::routes::request_for(&ws, &session, &ask);
    let mut v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("db".into(), json!(db.name));
        o.insert("dbId".into(), json!(db.id));
        o.insert("owner".into(), json!(owner_words(&db)));
        if r.status.starts_with("200") {
            o.insert(
                "next".into(),
                json!(format!(
                    "check once: {CLI_NAME} db requests (or {CLI_NAME} permission status <id>); once allowed, {CLI_NAME} db dsn --db {} gives your read-only DSN",
                    db.name
                )),
            );
        }
    }
    CliResponse { status: r.status, content_type: "application/json", body: v.to_string() }
}

/// The TCP door's `/cli/db/*` POST arm (`sql_routes::dispatch_post`): the
/// passport it bound, as an agent with no session (the card goes on the
/// workspace's pinned Thread); no passport is a person.
pub fn handle_request_read_tcp(body: &[u8]) -> CliResponse {
    let who = match crate::caller_workspace::request_principal() {
        Some(p) => Who::Agent { project_id: p.workspace_uuid, session_id: String::new() },
        None => Who::Owner,
    };
    handle_request_read(&who, true, body)
}

#[cfg(test)]
#[path = "db_read_routes_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "db_read_real_pg.rs"]
mod real_pg;
