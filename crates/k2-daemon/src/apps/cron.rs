//! App cron (prd-app-supabase-compat-v1 §11.2, lane E): the daemon calls an
//! App's own HTTP route on a schedule. It replaces "a `k2 job` that curls
//! the App": a job is a host process outside the App's wall that can only
//! reach the public URL; this is the daemon, headless, calling the App's
//! backend port on the box.
//!
//! - `k2 app cron add <app> "<5-field cron>" --path /api/cron/x [--method
//!   GET|POST] [--missed skip|once] [--timeout 5m]`: box time, like `k2 job
//!   --cron`.
//! - Each call sends `Authorization: Bearer <CRON_SECRET>`. `CRON_SECRET`
//!   is a vault secret the App also gets as env: the one already granted to
//!   the App under that name, else one K2 makes (`app-cron.<app>.CRON_SECRET`)
//!   and grants on the first `add` (a running App is re-hosted so it sees
//!   it). The daemon reads it the way the App's service would; it is never
//!   printed or logged.
//! - Overlap: while a run of the same entry is still going, the next slot is
//!   skipped and logged `skipped_overlap`.
//! - Missed slots (the daemon was down): `skip` logs `skipped_missed`;
//!   `once` runs one catch-up if the slot is under 12 h old (older ones are
//!   always skipped, as for `k2 job`).
//! - Every run and every skip is a row in `app_cron_runs` (0328): status,
//!   HTTP status, time, a short detail. Three failures in a row tell the
//!   App's workspace (its agent is woken with the facts, once per streak).
//! - `k2 app cron list / rm / run-now / logs`.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use k2_core::deploy::store::{self, AppRow};
use k2_core::jobs::{evaluate, Due, Schedule};
use k2_vault::{Consumer, Delivery, GrantRequest, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore as _, Selector, Use};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_credential, no, Who, R};

mod codes {
    pub const BAD_REQUEST: &str = "app_cron_usage";
    pub const NOT_FOUND: &str = "app_cron_not_found";
    pub const VAULT_CLOSED: &str = "app_cron_vault_closed";
    pub const FAILED: &str = "app_cron_failed";
}

pub const LIST: &str = "/cli/app/cron";
pub const ADD: &str = "/cli/app/cron/add";
pub const RM: &str = "/cli/app/cron/rm";
pub const RUN_NOW: &str = "/cli/app/cron/run-now";
pub const LOGS: &str = "/cli/app/cron/logs";
pub const ROUTES: &[&str] = &[LIST, ADD, RM, RUN_NOW, LOGS];
pub const POST_ROUTES: &[&str] = &[ADD, RM, RUN_NOW];

/// The env name the App reads.
pub const SECRET_ENV: &str = "CRON_SECRET";
/// Three failures in a row tell the App's workspace.
pub const FAILURES_TO_TELL: i64 = 3;
pub const DEFAULT_TIMEOUT_SECS: i64 = 300;
pub const MAX_TIMEOUT_SECS: i64 = 3600;
/// Entries per App.
pub const MAX_PER_APP: usize = 50;
/// A failure's detail: at most this much of the App's answer.
const DETAIL_CAP: usize = 300;
/// How often the daemon looks (headless).
const TICK: Duration = Duration::from_secs(15);

fn now() -> i64 {
    Utc::now().timestamp()
}

/// `app-cron.<app>.CRON_SECRET`.
pub fn secret_name(app: &str) -> String {
    format!("app-cron.{app}.{SECRET_ENV}")
}

// ── one entry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub app_id: String,
    pub schedule: String,
    pub path: String,
    pub method: String,
    pub missed: String,
    pub timeout_secs: i64,
    pub enabled: bool,
    pub anchor_at: i64,
    pub running_run: Option<String>,
    pub failures: i64,
}

const COLS: &str = "id, app_id, schedule, path, method, missed, timeout_secs, enabled, anchor_at, running_run, failures";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        app_id: r.get(1)?,
        schedule: r.get(2)?,
        path: r.get(3)?,
        method: r.get(4)?,
        missed: r.get(5)?,
        timeout_secs: r.get(6)?,
        enabled: r.get::<_, i64>(7)? != 0,
        anchor_at: r.get(8)?,
        running_run: r.get(9)?,
        failures: r.get(10)?,
    })
}

pub fn entries(app: Option<&str>) -> Vec<Entry> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let sql = format!("SELECT {COLS} FROM app_cron WHERE (?1 IS NULL OR app_id = ?1) ORDER BY app_id, created_at, id");
    let Ok(mut st) = conn.prepare(&sql) else { return vec![] };
    st.query_map(rusqlite::params![app], from_row).map(|it| it.flatten().collect()).unwrap_or_default()
}

fn entry(app: &str, id: &str) -> Option<Entry> {
    entries(Some(app)).into_iter().find(|e| e.id == id)
}

fn entry_json(e: &Entry) -> J {
    let next = Schedule::parse_cron(&e.schedule).ok().and_then(|s| {
        let anchor = Utc.timestamp_opt(e.anchor_at, 0).single()?;
        k2_core::jobs::next_slot(&s, anchor, Utc::now())
    });
    let last = last_runs(&e.id, 1).into_iter().next();
    json!({
        "id": e.id, "app": e.app_id, "schedule": e.schedule, "path": e.path, "method": e.method,
        "missed": e.missed, "timeoutSecs": e.timeout_secs, "enabled": e.enabled,
        "running": e.running_run.is_some(), "failuresInARow": e.failures,
        "next": next.map(|t| t.to_rfc3339()),
        "last": last,
    })
}

// ── validation ──────────────────────────────────────────────────────────

/// An App route: `/` then printable ASCII, no spaces, no `#`, at most 512.
fn path_ok(p: &str) -> bool {
    p.starts_with('/') && !p.starts_with("//") && p.len() <= 512 && p.bytes().all(|b| (0x21..0x7f).contains(&b) && b != b'#')
}

fn parse_duration(s: &str) -> Option<i64> {
    let t = s.trim();
    let (n, mul) = match t.as_bytes().last()? {
        b's' => (&t[..t.len() - 1], 1),
        b'm' => (&t[..t.len() - 1], 60),
        b'h' => (&t[..t.len() - 1], 3600),
        _ => (t, 1),
    };
    let v: i64 = n.parse().ok()?;
    let secs = v.checked_mul(mul)?;
    (1..=MAX_TIMEOUT_SECS).contains(&secs).then_some(secs)
}

// ── the secret ──────────────────────────────────────────────────────────

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    crate::vault::get().map_err(|e| no(codes::VAULT_CLOSED, 503, format!("the vault is closed ({}): CRON_SECRET lives there. {}", e.code, e.hint)))
}

/// The secret the App gets as `CRON_SECRET`: the one already granted under
/// that env name, else a new random one K2 keeps and grants. `true` when
/// it was made now (the App needs a re-host to see it).
fn ensure_secret(app: &AppRow) -> R<(String, bool)> {
    #[cfg(test)]
    if let Some(s) = test_seam::secret(&app.app_id) {
        return Ok((s, false));
    }
    let v = vault()?;
    let consumer = Consumer::published_service(app.service_id.clone());
    let fail = |what: &'static str| move |e: k2_vault::VaultError| no(codes::FAILED, 502, format!("{what}: {} ({})", e.hint, e.code));
    let grants = v.grants_to(&consumer).map_err(fail("reading the App's grants"))?;
    if let Some((g, _)) = grants.iter().find(|(g, _)| g.env_name.as_deref() == Some(SECRET_ENV)) {
        return Ok((g.secret_id.clone(), false));
    }
    let name = secret_name(&app.app_id);
    let _ = v.enable(&Selector::user(Scope::server(), name.clone()), &k2_vault::Actor::Owner);
    let mut put = PutRequest::new(Namespace::User, Scope::server(), name.clone(), Kind::Generic);
    put.default_delivery = Some(Delivery::Env);
    put.delivery_ceiling = Some(Delivery::Env);
    put.origin = "system:app-cron".into();
    put.description = format!("The bearer token the daemon's cron sends to the App {} (it checks it).", app.app_id);
    put.mode = PutMode::Upsert;
    put.human_given_override = Some(false);
    let value = format!("{}{}", k2_core::hire::apps::random_password(), k2_core::hire::apps::random_password());
    v.put(put, SecretBytes::from_slice(value.as_bytes()), &k2_vault::Actor::Owner).map_err(fail("keeping CRON_SECRET"))?;
    v.grant(
        GrantRequest {
            selector: Selector::user(Scope::server(), name.clone()),
            consumer,
            delivery: Delivery::Env,
            env_name: Some(SECRET_ENV.to_string()),
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &k2_vault::Actor::Owner,
    )
    .map_err(fail("granting CRON_SECRET to the App"))?;
    let id = v
        .grants_to(&Consumer::published_service(app.service_id.clone()))
        .map_err(fail("reading the App's grants"))?
        .into_iter()
        .find(|(g, _)| g.env_name.as_deref() == Some(SECRET_ENV))
        .map(|(g, _)| g.secret_id)
        .ok_or_else(|| no(codes::FAILED, 502, "CRON_SECRET was granted but isn't listed"))?;
    Ok((id, true))
}

/// The value the App's service resolves for `CRON_SECRET`, now.
fn secret_value(app: &AppRow) -> Result<String, String> {
    #[cfg(test)]
    if let Some(s) = test_seam::secret(&app.app_id) {
        return Ok(s);
    }
    let v = crate::vault::get().map_err(|e| format!("the vault is closed ({})", e.code))?;
    let grants = v
        .grants_to(&Consumer::published_service(app.service_id.clone()))
        .map_err(|e| format!("reading the App's grants: {}", e.code))?;
    let (g, _) = grants
        .into_iter()
        .find(|(g, _)| g.env_name.as_deref() == Some(SECRET_ENV))
        .ok_or_else(|| concat!("the App has no CRON_SECRET (run `", k2_core::cli_name!(), " app cron add` again)").to_string())?;
    let who = Requester::PublishedService { id: app.service_id.clone(), workspace_id: app.owner_workspace.clone() };
    let r = v.resolve(&Selector::Id(g.secret_id), Use::Env, &who).map_err(|e| format!("CRON_SECRET: {}", e.code))?;
    std::str::from_utf8(r.value.expose()).map(str::to_string).map_err(|_| "CRON_SECRET isn't text".into())
}

// ── one call ────────────────────────────────────────────────────────────

fn app_port(app_id: &str) -> Option<u16> {
    #[cfg(test)]
    if let Some(p) = test_seam::port(app_id) {
        return Some(p);
    }
    super::supervisor::live_port(app_id)
}

/// What one call came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: &'static str,
    pub http: Option<u16>,
    pub detail: Option<String>,
}

fn call_app(app: &AppRow, e: &Entry) -> Outcome {
    let Some(port) = app_port(&app.app_id) else {
        return Outcome { status: "not_running", http: None, detail: Some("the App isn't running".into()) };
    };
    let secret = match secret_value(app) {
        Ok(s) => s,
        Err(m) => return Outcome { status: "failed", http: None, detail: Some(m) },
    };
    let client = match reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(e.timeout_secs.max(1) as u64))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(x) => return Outcome { status: "failed", http: None, detail: Some(format!("client: {x}")) },
    };
    let url = format!("http://127.0.0.1:{port}{}", e.path);
    let req = if e.method == "POST" { client.post(&url) } else { client.get(&url) };
    let r = req
        .header("Authorization", format!("Bearer {secret}"))
        .header("User-Agent", "k2-cron/1")
        .header("X-K2-Cron", e.id.as_str())
        .send();
    match r {
        Ok(resp) => {
            let code = resp.status().as_u16();
            if resp.status().is_success() {
                return Outcome { status: "ok", http: Some(code), detail: None };
            }
            let mut body = Vec::new();
            let _ = resp.take(DETAIL_CAP as u64).read_to_end(&mut body);
            let text = String::from_utf8_lossy(&body).replace(&secret, "[CRON_SECRET]");
            Outcome { status: "failed", http: Some(code), detail: Some(text.trim().to_string()).filter(|t| !t.is_empty()) }
        }
        Err(x) if x.is_timeout() => Outcome { status: "timeout", http: None, detail: Some(format!("no answer in {} s", e.timeout_secs)) },
        Err(x) => Outcome { status: "failed", http: None, detail: Some(x.to_string().replace(&secret, "[CRON_SECRET]")) },
    }
}

// ── the run log (0328 `app_cron_runs`) ───────────────────────────────────

fn log_skip(e: &Entry, slot: i64, trigger: &str, status: &str, detail: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let t = now();
    let _ = conn.execute(
        "INSERT INTO app_cron_runs (id, cron_id, app_id, slot_at, trigger, status, started_at, finished_at, duration_ms, detail) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 0, ?8)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), e.id, e.app_id, slot, trigger, status, t, detail],
    );
}

/// Claim the entry for a run (the overlap guard): `None` when a run of it
/// is still going.
fn claim(e: &Entry, slot: Option<i64>, trigger: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let run = uuid::Uuid::new_v4().to_string();
    let n = conn
        .execute("UPDATE app_cron SET running_run = ?2 WHERE id = ?1 AND running_run IS NULL", rusqlite::params![e.id, run])
        .unwrap_or(0);
    if n != 1 {
        return None;
    }
    let _ = conn.execute(
        "INSERT INTO app_cron_runs (id, cron_id, app_id, slot_at, trigger, status, started_at) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)",
        rusqlite::params![run, e.id, e.app_id, slot, trigger, now()],
    );
    Some(run)
}

/// Finish a run: its row, the streak, the entry free again. Answers the
/// failures in a row after it.
fn finish(e: &Entry, run: &str, o: &Outcome, ms: i64) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE app_cron_runs SET status = ?2, http_status = ?3, finished_at = ?4, duration_ms = ?5, detail = ?6 WHERE id = ?1",
        rusqlite::params![run, o.status, o.http, now(), ms, o.detail],
    );
    let failed = matches!(o.status, "failed" | "timeout");
    let sql = if failed {
        "UPDATE app_cron SET running_run = NULL, failures = failures + 1 WHERE id = ?1"
    } else if o.status == "ok" {
        "UPDATE app_cron SET running_run = NULL, failures = 0 WHERE id = ?1"
    } else {
        "UPDATE app_cron SET running_run = NULL WHERE id = ?1"
    };
    let _ = conn.execute(sql, rusqlite::params![e.id]);
    conn.query_row("SELECT failures FROM app_cron WHERE id = ?1", rusqlite::params![e.id], |r| r.get(0)).unwrap_or(0)
}

/// One run, start to end (on the caller's thread).
fn run_claimed(app: &AppRow, e: &Entry, run: &str) -> (Outcome, i64) {
    let t0 = Instant::now();
    let o = call_app(app, e);
    let streak = finish(e, run, &o, t0.elapsed().as_millis() as i64);
    if streak == FAILURES_TO_TELL {
        let text = format!(
            concat!(
                "App cron: {} {} on {} failed {} times in a row (last: {}{}). `",
                k2_core::cli_name!(),
                " app cron logs {} {}` shows the runs; `",
                k2_core::cli_name!(),
                " app cron run-now {} {}` tries again."
            ),
            e.method,
            e.path,
            e.app_id,
            FAILURES_TO_TELL,
            o.status,
            o.http.map(|h| format!(", HTTP {h}")).unwrap_or_default(),
            e.app_id,
            e.id,
            e.app_id,
            e.id
        );
        super::alerts::notify(&app.owner_workspace, &text);
    }
    (o, streak)
}

pub fn last_runs(cron_id: &str, limit: i64) -> Vec<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut st) = conn.prepare(
        "SELECT id, trigger, status, http_status, started_at, duration_ms, detail, slot_at FROM app_cron_runs \
         WHERE cron_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT ?2",
    ) else {
        return vec![];
    };
    st.query_map(rusqlite::params![cron_id, limit], |r| {
        let at: i64 = r.get(4)?;
        let slot: Option<i64> = r.get(7)?;
        Ok(json!({
            "run": r.get::<_, String>(0)?,
            "trigger": r.get::<_, String>(1)?,
            "status": r.get::<_, String>(2)?,
            "http": r.get::<_, Option<i64>>(3)?,
            "at": Utc.timestamp_opt(at, 0).single().map(|t| t.to_rfc3339()),
            "slot": slot.and_then(|s| Utc.timestamp_opt(s, 0).single()).map(|t| t.to_rfc3339()),
            "ms": r.get::<_, Option<i64>>(5)?,
            "detail": r.get::<_, Option<String>>(6)?,
        }))
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

// ── the scheduler (headless) ────────────────────────────────────────────

fn set_anchor(id: &str, at: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute("UPDATE app_cron SET anchor_at = ?2 WHERE id = ?1", rusqlite::params![id, at]);
}

/// What one look at `now` did to each entry (tests read it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    Started(String),
    SkippedOverlap(String),
    SkippedMissed(String),
    NotYet(String),
}

/// One look: every due slot of every enabled entry is started (each run on
/// its own thread through `spawn`), skipped for an overlap, or skipped as
/// missed by its policy. The anchor moves to the slot either way.
pub fn tick_at(now_s: i64, spawn: &dyn Fn(AppRow, Entry, String)) -> Vec<Tick> {
    let now_t = Utc.timestamp_opt(now_s, 0).single().unwrap_or_else(Utc::now);
    let mut out = Vec::new();
    for e in entries(None).into_iter().filter(|e| e.enabled) {
        let Ok(sched) = Schedule::parse_cron(&e.schedule) else { continue };
        let Some(anchor) = Utc.timestamp_opt(e.anchor_at, 0).single() else { continue };
        let (slot, trigger) = match evaluate(&sched, anchor, now_t) {
            Due::NotYet { .. } | Due::Invalid { .. } => {
                out.push(Tick::NotYet(e.id.clone()));
                continue;
            }
            Due::SkipMissed { slot } => {
                log_skip(&e, slot.timestamp(), "schedule", "skipped_missed", "the slot is over 12 h old (the daemon was down)");
                set_anchor(&e.id, slot.timestamp());
                out.push(Tick::SkippedMissed(e.id.clone()));
                continue;
            }
            Due::Fire { slot, catchup: true } if e.missed == "skip" => {
                log_skip(&e, slot.timestamp(), "catch_up", "skipped_missed", "missed while the daemon was down; this entry skips missed runs (--missed once runs one)");
                set_anchor(&e.id, slot.timestamp());
                out.push(Tick::SkippedMissed(e.id.clone()));
                continue;
            }
            Due::Fire { slot, catchup } => (slot.timestamp(), if catchup { "catch_up" } else { "schedule" }),
        };
        set_anchor(&e.id, slot);
        let app = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::get_app(&conn, &e.app_id)
        };
        let Ok(Some(app)) = app else { continue };
        match claim(&e, Some(slot), trigger) {
            Some(run) => {
                out.push(Tick::Started(e.id.clone()));
                spawn(app, e, run);
            }
            None => {
                log_skip(&e, slot, trigger, "skipped_overlap", "the previous run is still going");
                out.push(Tick::SkippedOverlap(e.id.clone()));
            }
        }
    }
    out
}

/// At boot: a run the daemon was in the middle of when it stopped is over.
pub fn boot() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE app_cron_runs SET status = 'interrupted', finished_at = ?1, detail = 'the daemon stopped during the run' WHERE status = 'running'",
        rusqlite::params![now()],
    );
    let _ = conn.execute("UPDATE app_cron SET running_run = NULL WHERE running_run IS NOT NULL", []);
}

/// The scheduler, headless: one look every 15 s; each run on a thread.
pub fn spawn_loop() {
    boot();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut iv = tokio::time::interval(TICK);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            iv.tick().await;
            let _ = tokio::task::spawn_blocking(|| {
                tick_at(now(), &|app, e, run| {
                    let _ = std::thread::Builder::new().name("app-cron".into()).spawn(move || {
                        run_claimed(&app, &e, &run);
                    });
                })
            })
            .await;
        }
    });
}

// ── routes ──────────────────────────────────────────────────────────────

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    match path {
        LIST => {
            let app = app_for(who, params.get("app").map(String::as_str))?;
            Ok(json!({"ok": true, "app": app.app_id, "crons": entries(Some(&app.app_id)).iter().map(entry_json).collect::<Vec<_>>()}))
        }
        LOGS => {
            let app = app_for(who, params.get("app").map(String::as_str))?;
            let limit = params.get("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(20).clamp(1, 500);
            let list: Vec<Entry> = match params.get("id") {
                Some(id) => vec![entry(&app.app_id, id).ok_or_else(|| no(codes::NOT_FOUND, 404, format!("no cron {id} on {}", app.app_id)))?],
                None => entries(Some(&app.app_id)),
            };
            let runs: Vec<J> = list.iter().map(|e| json!({"id": e.id, "path": e.path, "runs": last_runs(&e.id, limit)})).collect();
            Ok(json!({"ok": true, "app": app.app_id, "logs": runs}))
        }
        ADD => {
            super::routes::need_build(who)?;
            add(who, body)
        }
        RM => {
            super::routes::need_build(who)?;
            let v = json_body(body)?;
            let app = app_for(who, v.get("app").and_then(J::as_str))?;
            let id = v.get("id").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, concat!("name the cron (its id from `", k2_core::cli_name!(), " app cron list`)")))?;
            let e = entry(&app.app_id, id).ok_or_else(|| no(codes::NOT_FOUND, 404, format!("no cron {id} on {}", app.app_id)))?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM app_cron WHERE id = ?1", rusqlite::params![e.id]).map_err(|x| no(codes::FAILED, 500, x.to_string()))?;
            eprintln!("[apps] cron rm {} {} by {}", app.app_id, e.id, actor(who));
            Ok(json!({"ok": true, "app": app.app_id, "removed": e.id, "note": "CRON_SECRET stays granted to the App (its code may still check it)"}))
        }
        RUN_NOW => {
            super::routes::need_build(who)?;
            let v = json_body(body)?;
            let app = app_for(who, v.get("app").and_then(J::as_str))?;
            let id = v.get("id").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, concat!("name the cron (its id from `", k2_core::cli_name!(), " app cron list`)")))?;
            let e = entry(&app.app_id, id).ok_or_else(|| no(codes::NOT_FOUND, 404, format!("no cron {id} on {}", app.app_id)))?;
            let Some(run) = claim(&e, None, "run_now") else {
                log_skip(&e, now(), "run_now", "skipped_overlap", "the previous run is still going");
                return Err(no("cron_running", 409, format!(concat!("{} is still running; nothing started (`", k2_core::cli_name!(), " app cron logs {} {}`)"), e.id, app.app_id, e.id)));
            };
            let (o, streak) = run_claimed(&app, &e, &run);
            Ok(json!({"ok": o.status == "ok", "app": app.app_id, "id": e.id, "run": run, "status": o.status, "http": o.http, "detail": o.detail, "failuresInARow": streak}))
        }
        _ => Err(no(codes::NOT_FOUND, 404, "no such App cron route")),
    }
}

/// POST `/cli/app/cron/add {app, schedule, path, method?, missed?, timeout?}`.
fn add(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = app_for(who, v.get("app").and_then(J::as_str))?;
    super::suspend::refuse_if_suspended(&app.app_id)?;
    let s = |k: &str| v.get(k).and_then(J::as_str).map(str::trim).filter(|x| !x.is_empty());
    let usage = concat!(k2_core::cli_name!(), " app cron add <app> \"<min hour day month weekday>\" --path /api/cron/x [--method GET|POST] [--missed skip|once] [--timeout 5m]");
    let schedule = s("schedule").ok_or_else(|| no(codes::BAD_REQUEST, 400, usage))?;
    let sched = Schedule::parse_cron(schedule).map_err(|e| no(codes::BAD_REQUEST, 400, e.hint))?;
    let Schedule::Cron(schedule) = sched else { return Err(no(codes::BAD_REQUEST, 400, usage)) };
    let path = s("path").ok_or_else(|| no(codes::BAD_REQUEST, 400, usage))?;
    if !path_ok(path) {
        return Err(no(codes::BAD_REQUEST, 400, format!("--path {path:?}: the App's own route, starting with / (no host, no spaces, no #)")));
    }
    let method = s("method").unwrap_or("GET").to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST") {
        return Err(no(codes::BAD_REQUEST, 400, "--method takes GET or POST"));
    }
    let missed = s("missed").unwrap_or("skip");
    if !matches!(missed, "skip" | "once") {
        return Err(no(codes::BAD_REQUEST, 400, "--missed takes skip (default) or once"));
    }
    let timeout = match s("timeout") {
        None => DEFAULT_TIMEOUT_SECS,
        Some(t) => parse_duration(t).ok_or_else(|| no(codes::BAD_REQUEST, 400, "--timeout takes 30s, 5m or 1h (at most 1h)"))?,
    };
    let have = entries(Some(&app.app_id));
    if let Some(e) = have.iter().find(|e| e.path == path && e.schedule == schedule) {
        return Err(no("cron_exists", 409, format!("{} already calls {path} on {schedule} ({})", app.app_id, e.id)));
    }
    if have.len() >= MAX_PER_APP {
        return Err(no("cron_full", 409, format!("{} already has {MAX_PER_APP} crons", app.app_id)));
    }
    let (_, made) = ensure_secret(&app)?;
    let id = format!("c{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO app_cron (id, app_id, schedule, path, method, missed, timeout_secs, enabled, anchor_at, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8, ?9)",
            rusqlite::params![id, app.app_id, schedule, path, method, missed, timeout, now(), actor(who)],
        )
        .map_err(|x| no(codes::FAILED, 500, x.to_string()))?;
    }
    // A new CRON_SECRET reaches a running App at its next start: re-host now.
    let rehost = if made && app_port(&app.app_id).is_some() {
        match super::rehost::rehost_now_yes(&app) {
            super::rehost::Turn::Done(d) => json!({"result": "done", "deployment": d}),
            super::rehost::Turn::Failed(c, m) => json!({"result": "failed", "error": format!("{c}: {m}")}),
            super::rehost::Turn::Deferred(c, m) => json!({"result": "deferred", "why": format!("{c}: {m}")}),
            super::rehost::Turn::NotDue => json!({"result": "not_due"}),
        }
    } else {
        json!({"result": if made { "not_running" } else { "not_needed" }})
    };
    eprintln!("[apps] cron add {} {id} {method} {path} ({schedule}) by {}", app.app_id, actor(who));
    let e = entry(&app.app_id, &id).ok_or_else(|| no(codes::FAILED, 500, "the cron wasn't saved"))?;
    let mut out = entry_json(&e);
    out["ok"] = json!(true);
    out["secret"] = json!({"env": SECRET_ENV, "made": made, "rehost": rehost});
    Ok(out)
}

#[cfg(test)]
pub(crate) mod test_seam {
    //! Tests: the App's port and CRON_SECRET without a supervisor or vault.
    use std::collections::HashMap;
    use std::sync::Mutex;

    static PORTS: Mutex<Option<HashMap<String, u16>>> = Mutex::new(None);
    static SECRETS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    pub fn set(app: &str, port: Option<u16>, secret: &str) {
        let mut p = PORTS.lock().unwrap_or_else(|e| e.into_inner());
        let p = p.get_or_insert_with(HashMap::new);
        match port {
            Some(x) => {
                p.insert(app.into(), x);
            }
            None => {
                p.remove(app);
            }
        }
        SECRETS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(app.into(), secret.into());
    }
    pub fn port(app: &str) -> Option<u16> {
        PORTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(app).copied())
    }
    pub fn secret(app: &str) -> Option<String> {
        SECRETS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(app).cloned())
    }
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
