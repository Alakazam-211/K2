//! `/cli/jobs/*` — `k2 job` (prd-vault-gaps-v1 item 5).
//!
//! Callers (`vault::actor`):
//! - the owner token and Owner/Admin logins: any workspace (name it);
//!   `pause --all` / `resume --all` are theirs only;
//! - an agent or K2 shell-tab passport: its own workspace only (Q14: an
//!   agent may add jobs without a person; every add, edit and removal posts
//!   a line on the workspace's Thread and shows in `k2 job list`);
//! - a job run's own passport: refused (a job never makes jobs);
//! - Member logins, app passes, API keys: refused.
//!
//! Every POST route is on the dispatcher's POST list and 405s a GET
//! (`require_post`, mirrored by `route_policy`).

use std::collections::HashMap;

use chrono::Utc;
use k2_core::jobs::{self as store, Job, JobEdit, JobError, NewJob, Schedule};
use k2_vault::{Actor, Consumer, ConsumerKind, Role, SecretStore, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;

pub const LIST: &str = "/cli/jobs/list";
pub const SHOW: &str = "/cli/jobs/show";
pub const LOGS: &str = "/cli/jobs/logs";
pub const ADD: &str = "/cli/jobs/add";
pub const EDIT: &str = "/cli/jobs/edit";
pub const RUN: &str = "/cli/jobs/run";
pub const ENABLE: &str = "/cli/jobs/enable";
pub const DISABLE: &str = "/cli/jobs/disable";
pub const KILL: &str = "/cli/jobs/kill";
pub const REMOVE: &str = "/cli/jobs/remove";
pub const PAUSE: &str = "/cli/jobs/pause";
pub const RESUME: &str = "/cli/jobs/resume";

pub const GET_ROUTES: &[&str] = &[LIST, SHOW, LOGS];
pub const POST_ROUTES: &[&str] = &[ADD, EDIT, RUN, ENABLE, DISABLE, KILL, REMOVE, PAUSE, RESUME];

/// Bodies are small JSON (an argv is at most 16 KiB).
pub const MAX_BODY: usize = 32 * 1024;
/// `logs` returns at most this much of a log (the tail).
const LOGS_MAX: usize = 256 * 1024;

pub fn is_route(path: &str) -> bool {
    GET_ROUTES.contains(&path) || POST_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn refuse(status: u16, code: &str, hint: &str) -> CliResponse {
    let status = match status {
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        _ => "500 Internal Server Error",
    };
    CliResponse { status, content_type: "application/json", body: json!({ "error": { "code": code, "hint": hint } }).to_string() }
}

pub fn too_large() -> CliResponse {
    refuse(413, "body_too_large", "job requests are at most 32 KiB")
}

fn err(code: &'static str, status: u16, hint: impl Into<String>) -> JobError {
    JobError::new(code, status, hint)
}

/// Who is calling, in job terms.
#[derive(Debug, Clone)]
enum Who {
    /// The owner token or an Owner/Admin login.
    Person { stamp: String, label: String },
    /// An agent or shell-tab passport: its own workspace.
    Agent { workspace_id: String, stamp: String },
}

impl Who {
    fn stamp(&self) -> &str {
        match self {
            Who::Person { stamp, .. } | Who::Agent { stamp, .. } => stamp,
        }
    }
    fn label(&self) -> String {
        match self {
            Who::Person { label, .. } => label.clone(),
            Who::Agent { .. } => "this workspace's agent".into(),
        }
    }
}

fn who(c: &VaultCaller) -> Result<Who, JobError> {
    if c.is_job() {
        return Err(err("job_cannot_manage_jobs", 403, "a job run can't add, change or run jobs; its workspace's agent or a person can"));
    }
    match &c.actor {
        Actor::Owner => Ok(Who::Person { stamp: "owner".into(), label: "the owner".into() }),
        Actor::Login { username, role: Role::Owner | Role::Admin } => {
            Ok(Who::Person { stamp: c.actor.stamp(), label: username.clone() })
        }
        Actor::Agent(a) => Ok(Who::Agent { workspace_id: a.workspace_id.clone(), stamp: c.actor.stamp() }),
        _ => Err(err("forbidden", 403, "jobs are for the owner, an admin, or a workspace's own agent")),
    }
}

/// A workspace as people name it (handle, folder or id) → `projects.id`.
fn resolve_ws(given: &str) -> Option<String> {
    let g = given.trim();
    if g.is_empty() {
        return None;
    }
    if crate::vault::workspace_path(g).is_some() {
        return Some(g.to_string());
    }
    crate::vault::workspace_id(g)
}

/// The workspace a request acts on.
fn workspace(w: &Who, given: Option<&str>) -> Result<String, JobError> {
    let given = given.map(str::trim).filter(|s| !s.is_empty());
    match w {
        Who::Agent { workspace_id, .. } => {
            if let Some(g) = given {
                if resolve_ws(g).as_deref() != Some(workspace_id.as_str()) {
                    return Err(err("job_not_yours", 403, "an agent manages its own workspace's jobs only"));
                }
            }
            Ok(workspace_id.clone())
        }
        Who::Person { .. } => {
            let g = given.ok_or_else(|| err("workspace_required", 400, "name the workspace: --workspace <name>"))?;
            resolve_ws(g).ok_or_else(|| err("no_such_workspace", 404, format!("no workspace named {g} (run from its folder or pass --workspace <name>)")))
        }
    }
}

fn people_only(w: &Who) -> Result<(), JobError> {
    match w {
        Who::Person { .. } => Ok(()),
        Who::Agent { .. } => Err(err("owner_only", 403, "pausing every job is for the owner or an admin")),
    }
}

fn find(ws: &str, name: Option<&str>) -> Result<Job, JobError> {
    let name = name.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| err("name_required", 400, "which job? give its name"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_by_name(&conn, ws, name)?.ok_or_else(|| err("no_such_job", 404, format!("no job named {name} here (k2 job list)")))
}

// ── vault glue (`k2 vault grant NAME --to job:<name>`) ──────────────

/// `job:<name>` (the caller's workspace) or `job:<workspace>/<name>` →
/// (workspace id, job id). The job must exist.
pub fn job_for_grant(c: &VaultCaller, id: &str) -> Result<(String, String), VaultError> {
    let (ws, name) = match id.rsplit_once('/') {
        Some((w, n)) => {
            let ws = resolve_ws(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?;
            (ws, n.to_string())
        }
        None => {
            let ws = c
                .workspace_id()
                .map(str::to_string)
                .ok_or_else(|| VaultError::bad_request("bad_consumer", "name the job's workspace: job:<workspace>/<name>"))?;
            (ws, id.to_string())
        }
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let job = store::get_by_name(&conn, &ws, &name)
        .map_err(|e| VaultError::internal(e.hint))?
        .ok_or_else(|| VaultError::not_found(format!("no job named {name} (k2 job add first)")))?;
    Ok((ws, job.id))
}

/// How people see a job grant: `job:collect` in the caller's workspace,
/// else `job:<workspace>/collect`; a removed job's leftover grant says so.
pub fn grant_label(c: &VaultCaller, consumer: &Consumer) -> String {
    let Some((ws, job_id)) = consumer.id.split_once('/') else { return consumer.label() };
    let db = k2_core::db::shared();
    let name = {
        let conn = db.lock();
        store::get(&conn, job_id).ok().flatten().map(|j| j.name)
    };
    match name {
        Some(n) if c.workspace_id() == Some(ws) => format!("job:{n}"),
        Some(n) => format!("job:{}/{n}", crate::vault::workspace_label(ws)),
        None => format!("job:{job_id} (removed)"),
    }
}

/// The job's vault grants (names only), for `show`.
fn job_grants(job: &Job) -> Vec<Value> {
    let Ok(v) = crate::vault::get() else { return vec![] };
    v.grants_to(&Consumer::job(&job.workspace_id, &job.id))
        .unwrap_or_default()
        .into_iter()
        .map(|(g, name)| json!({ "secret": name, "delivery": g.delivery.as_str(), "envName": g.env_name, "fileName": g.file_name }))
        .collect()
}

/// A removed job's grants go with it (as K2, not the caller).
fn revoke_job_grants(job: &Job) -> usize {
    let Ok(v) = crate::vault::get() else { return 0 };
    let sys = Actor::System("jobs".into());
    let mut n = 0;
    for (g, _) in v.grants_to(&Consumer::job(&job.workspace_id, &job.id)).unwrap_or_default() {
        if g.consumer.kind == ConsumerKind::Job && v.revoke(&g.id, &sys).is_ok() {
            n += 1;
        }
    }
    n
}

// ── views ───────────────────────────────────────────────────────────

fn argv_line(argv: &[String]) -> String {
    let parts: Vec<String> = argv
        .iter()
        .map(|a| {
            if !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b)) {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect();
    crate::vault::redact_str(&parts.join(" "))
}

fn run_json(r: &store::Run) -> Value {
    json!({
        "id": r.id,
        "startedAt": r.started_at,
        "endedAt": r.ended_at,
        "outcome": r.outcome,
        "trigger": r.trigger,
        "exitCode": r.exit_code,
        "bytes": r.bytes,
        "startedBy": r.started_by,
        "hasLog": r.log_path.is_some(),
    })
}

fn job_json(job: &Job, runs: usize) -> Value {
    let db = k2_core::db::shared();
    let (recent, paused) = {
        let conn = db.lock();
        (store::list_runs(&conn, &job.id, runs.max(1)).unwrap_or_default(), store::paused(&conn).map(|p| p.0).unwrap_or(false))
    };
    let next = if job.enabled && !paused { store::next_slot(&job.schedule, job.anchor(), Utc::now()).map(store::ts) } else { None };
    let mut v = json!({
        "id": job.id,
        "name": job.name,
        "workspaceId": job.workspace_id,
        "workspace": crate::vault::workspace_label(&job.workspace_id),
        "schedule": job.schedule.describe(),
        "command": argv_line(&job.argv),
        "argv": job.argv.iter().map(|a| crate::vault::redact_str(a)).collect::<Vec<_>>(),
        "cwd": job.cwd,
        "timeoutSecs": job.timeout_secs,
        "enabled": job.enabled,
        "createdBy": job.created_by,
        "createdAt": job.created_at,
        "updatedBy": job.updated_by,
        "updatedAt": job.updated_at,
        "running": crate::jobs::running_run(&job.id),
        "next": next,
        "lastRun": recent.first().map(run_json),
    });
    if runs > 1 {
        v["runs"] = Value::Array(recent.iter().map(run_json).collect());
    }
    v
}

/// The Thread line for an add / edit / removal (Q14).
fn note(job: &Job, w: &Who, verb: &str) -> bool {
    let Some(path) = crate::vault::workspace_path(&job.workspace_id) else { return false };
    let body = match verb {
        "removed" => format!("K2: job **{}** removed by {}.", job.name, w.label()),
        _ => format!(
            "K2: job **{}** {verb} by {}: {}, runs `{}` (timeout {}). It runs unattended with only the secrets granted to job:{}. `k2 job disable {}` stops it.",
            job.name,
            w.label(),
            job.schedule.describe(),
            argv_line(&job.argv),
            store::human_secs(job.timeout_secs),
            job.name,
            job.name
        ),
    };
    match crate::overlay_routes::post_workspace_note(&path, &body, "k2-job") {
        Ok(()) => true,
        Err(e) => {
            k2_core::log_debug!("[jobs] Thread note for {}: {e}", job.name);
            false
        }
    }
}

// ── handlers ────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    workspace: Option<String>,
    name: Option<String>,
    every: Option<String>,
    cron: Option<String>,
    timeout: Option<String>,
    cwd: Option<String>,
    argv: Option<Vec<String>>,
    #[serde(default)]
    all: bool,
}

fn parse_body(body: &[u8]) -> Result<Body, JobError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Body::default());
    }
    serde_json::from_slice(body).map_err(|_| err("bad_json", 400, "bad request body"))
}

fn schedule_of(b: &Body) -> Result<Option<Schedule>, JobError> {
    match (b.every.as_deref().filter(|s| !s.is_empty()), b.cron.as_deref().filter(|s| !s.is_empty())) {
        (Some(_), Some(_)) => Err(err("bad_schedule", 400, "give --every or --cron, not both")),
        (Some(e), None) => Ok(Some(Schedule::parse_every(e)?)),
        (None, Some(c)) => Ok(Some(Schedule::parse_cron(c)?)),
        (None, None) => Ok(None),
    }
}

pub fn handle(path: &str, caller: Result<VaultCaller, VaultError>, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let c = match caller {
        Ok(c) => c,
        Err(e) => return refuse(e.status, e.code, &e.hint),
    };
    match dispatch(path, &c, body, params) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(e.status, e.code, &e.hint),
    }
}

fn dispatch(path: &str, c: &VaultCaller, body: &[u8], params: &HashMap<String, String>) -> Result<Value, JobError> {
    let w = who(c)?;
    let db = k2_core::db::shared();
    let now = Utc::now();
    let p = |k: &str| params.get(k).map(String::as_str);
    match path {
        LIST => {
            let all = matches!(p("all"), Some("1" | "true"));
            let ws = if all {
                people_only(&w).map_err(|_| err("owner_only", 403, "listing every workspace's jobs is for the owner or an admin"))?;
                None
            } else {
                Some(workspace(&w, p("workspace"))?)
            };
            let (jobs, paused) = {
                let conn = db.lock();
                (store::list(&conn, ws.as_deref())?, store::paused(&conn)?)
            };
            Ok(json!({
                "jobs": jobs.iter().map(|j| job_json(j, 1)).collect::<Vec<_>>(),
                "paused": paused.0,
                "pausedBy": if paused.0 { Some(paused.1) } else { None },
            }))
        }
        SHOW => {
            let ws = workspace(&w, p("workspace"))?;
            let job = find(&ws, p("name"))?;
            let mut v = job_json(&job, 10);
            v["grants"] = Value::Array(job_grants(&job));
            Ok(json!({ "job": v }))
        }
        LOGS => {
            let ws = workspace(&w, p("workspace"))?;
            let job = find(&ws, p("name"))?;
            let run = {
                let conn = db.lock();
                match p("run").filter(|s| !s.is_empty()) {
                    Some(id) => store::get_run(&conn, id)?.filter(|r| r.job_id == job.id),
                    None => store::list_runs(&conn, &job.id, 50)?.into_iter().find(|r| r.log_path.is_some()),
                }
            };
            let run = run.ok_or_else(|| err("no_such_run", 404, format!("{} has no run log yet", job.name)))?;
            let text = match &run.log_path {
                Some(lp) => read_tail(lp)?,
                None => (String::new(), false),
            };
            Ok(json!({ "job": job.name, "run": run_json(&run), "log": crate::vault::redact_str(&text.0), "truncated": text.1 }))
        }
        ADD => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let schedule = schedule_of(&b)?.ok_or_else(|| err("schedule_required", 400, "when does it run? --every 15m or --cron '*/15 * * * *'"))?;
            let timeout = match b.timeout.as_deref().filter(|s| !s.is_empty()) {
                Some(t) => store::parse_timeout(t)?,
                None => store::DEFAULT_TIMEOUT_SECS,
            };
            let argv = b.argv.clone().unwrap_or_default();
            let cwd = b.cwd.clone().unwrap_or_default();
            check_cwd(&ws, &cwd)?;
            let job = {
                let conn = db.lock();
                store::insert(
                    &conn,
                    NewJob {
                        workspace_id: ws,
                        name: b.name.clone().unwrap_or_default().trim().to_string(),
                        schedule,
                        argv,
                        cwd,
                        timeout_secs: timeout,
                        created_by: w.stamp().to_string(),
                    },
                    now,
                )?
            };
            let noted = note(&job, &w, "added");
            Ok(json!({ "ok": true, "job": job_json(&job, 1), "threadNote": noted }))
        }
        EDIT => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            let timeout = match b.timeout.as_deref().filter(|s| !s.is_empty()) {
                Some(t) => Some(store::parse_timeout(t)?),
                None => None,
            };
            if let Some(cwd) = &b.cwd {
                check_cwd(&ws, cwd)?;
            }
            let e = JobEdit { schedule: schedule_of(&b)?, argv: b.argv.clone().filter(|a| !a.is_empty()), cwd: b.cwd.clone(), timeout_secs: timeout };
            let job = {
                let conn = db.lock();
                store::edit(&conn, &job.id, e, w.stamp(), now)?
            };
            let noted = note(&job, &w, "changed");
            Ok(json!({ "ok": true, "job": job_json(&job, 1), "threadNote": noted }))
        }
        RUN => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            if !job.enabled {
                return Err(err("job_disabled", 409, format!("{} is off (k2 job enable {})", job.name, job.name)));
            }
            let paused = store::paused(&db.lock())?.0;
            if paused {
                return Err(err("jobs_paused", 409, "every job is paused (k2 job resume --all, people only)"));
            }
            let run = crate::jobs::start_run(&job, "manual", w.stamp())?;
            Ok(json!({ "ok": true, "job": job.name, "run": run }))
        }
        ENABLE | DISABLE => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            let job = {
                let conn = db.lock();
                store::set_enabled(&conn, &job.id, path == ENABLE, w.stamp(), now)?
            };
            Ok(json!({ "ok": true, "job": job_json(&job, 1) }))
        }
        KILL => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            let run = crate::jobs::kill(&job.id).ok_or_else(|| err("not_running", 409, format!("{} isn't running", job.name)))?;
            Ok(json!({ "ok": true, "job": job.name, "run": run }))
        }
        REMOVE => {
            let b = parse_body(body)?;
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            let killed = crate::jobs::kill(&job.id);
            let logs = {
                let conn = db.lock();
                store::remove(&conn, &job.id)?
            };
            for l in logs {
                let _ = std::fs::remove_file(l);
            }
            let revoked = revoke_job_grants(&job);
            let noted = note(&job, &w, "removed");
            Ok(json!({ "ok": true, "removed": job.name, "killedRun": killed, "grantsRevoked": revoked, "threadNote": noted }))
        }
        PAUSE | RESUME => {
            let b = parse_body(body)?;
            if b.all {
                people_only(&w)?;
                let conn = db.lock();
                store::set_paused(&conn, path == PAUSE, w.stamp(), now)?;
                return Ok(json!({ "ok": true, "paused": path == PAUSE }));
            }
            // `pause NAME` / `resume NAME` = disable / enable.
            let ws = workspace(&w, b.workspace.as_deref())?;
            let job = find(&ws, b.name.as_deref())?;
            let job = {
                let conn = db.lock();
                store::set_enabled(&conn, &job.id, path == RESUME, w.stamp(), now)?
            };
            Ok(json!({ "ok": true, "job": job_json(&job, 1) }))
        }
        _ => Err(err("not_found", 404, "no such job route")),
    }
}

/// A `--cwd` must be a folder inside the workspace now (checked again at
/// every run).
fn check_cwd(ws: &str, cwd: &str) -> Result<(), JobError> {
    let root = crate::vault::workspace_path(ws).ok_or_else(|| err("workspace_missing", 409, "the workspace folder isn't there"))?;
    super::runner::run_dir(std::path::Path::new(&root), cwd).map(|_| ())
}

/// The last [`LOGS_MAX`] bytes of a log, on a char boundary.
fn read_tail(path: &str) -> Result<(String, bool), JobError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), false)),
        Err(e) => return Err(err("log_unreadable", 500, format!("log: {}", e.kind()))),
    };
    if bytes.len() <= LOGS_MAX {
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), false));
    }
    let tail = &bytes[bytes.len() - LOGS_MAX..];
    Ok((String::from_utf8_lossy(tail).into_owned(), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(ws: &str, sid: &str) -> VaultCaller {
        VaultCaller {
            actor: Actor::Agent(k2_vault::AgentActor {
                workspace_id: ws.into(),
                session_id: Some(sid.into()),
                shell: false,
                vault_manage: false,
            }),
            channel: crate::vault::actor::Channel::Plain,
            session_id: Some(sid.into()),
        }
    }

    #[test]
    fn job_passports_and_members_are_refused() {
        let job = agent("ws-1", "job-run-1");
        assert_eq!(who(&job).expect_err("job").code, "job_cannot_manage_jobs");
        let member = VaultCaller {
            actor: Actor::Login { username: "m".into(), role: Role::Member },
            channel: crate::vault::actor::Channel::Login,
            session_id: None,
        };
        assert_eq!(who(&member).expect_err("member").code, "forbidden");
        assert!(matches!(who(&agent("ws-1", "s-1")).expect("agent"), Who::Agent { .. }));
        let owner = VaultCaller { actor: Actor::Owner, channel: crate::vault::actor::Channel::Plain, session_id: None };
        assert!(matches!(who(&owner).expect("owner"), Who::Person { .. }));
    }

    #[test]
    fn agents_stay_in_their_workspace_and_people_name_one() {
        let w = Who::Agent { workspace_id: "ws-own".into(), stamp: "agent:ws-own/s".into() };
        assert_eq!(workspace(&w, None).expect("own"), "ws-own");
        assert_eq!(workspace(&w, Some("")).expect("blank = own"), "ws-own");
        assert_eq!(workspace(&w, Some("somebody-else-entirely")).expect_err("other").code, "job_not_yours");
        assert_eq!(people_only(&w).expect_err("pause all").code, "owner_only");
        let p = Who::Person { stamp: "owner".into(), label: "the owner".into() };
        assert_eq!(workspace(&p, None).expect_err("must name").code, "workspace_required");
        people_only(&p).expect("people may pause all");
    }

    #[test]
    fn argv_lines_quote_what_needs_it() {
        assert_eq!(argv_line(&["./collect.sh".into(), "--since".into(), "15m".into()]), "./collect.sh --since 15m");
        assert_eq!(argv_line(&["sh".into(), "-c".into(), "echo $X | wc -c".into()]), "sh -c 'echo $X | wc -c'");
        assert_eq!(argv_line(&["it's".into()]), "'it'\\''s'");
    }

    #[test]
    fn schedules_are_one_or_the_other() {
        let b = Body { every: Some("15m".into()), cron: Some("* * * * *".into()), ..Default::default() };
        assert_eq!(schedule_of(&b).expect_err("both").code, "bad_schedule");
        let b = Body { every: Some("15m".into()), ..Default::default() };
        assert_eq!(schedule_of(&b).expect("every"), Some(Schedule::Every(900)));
        assert_eq!(schedule_of(&Body::default()).expect("none"), None);
    }
}
