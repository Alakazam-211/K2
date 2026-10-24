//! `k2 app move-plan <service>` (0.46.4, NSI #2): every step that moves a
//! legacy `--cmd` service onto the App runtime, what's already done, and
//! ONE owner approval for the owner's steps.
//!
//! ```text
//! GET  /cli/app/move-plan          ?service=&app=            the plan (read only)
//! POST /cli/app/move-plan/request  {service, app?, reason?}  one held request for the owner's steps
//! ```
//!
//! **The plan**, in order (each step `done` | `todo` | `blocked` | `n/a`,
//! with who does it and the command):
//!
//! | step | who |
//! |---|---|
//! | `box_prep` (the doctor is ready; n/a off Linux or for a process-engine App) | owner |
//! | `engine` (the guarded engine answers; same n/a) | owner |
//! | `build_switch` ("Build and run Apps" on the workspace) | owner |
//! | `install` (the App exists) | agent |
//! | `deploy` (a live Seed, running) | agent |
//! | `copy_host` (box-login users get the same access on the App; lists them) | owner |
//! | `guest_rooms` (the `guests-rooms` grant, with the rooms the old service hands out when knowable) | owner |
//! | `data_put` (the App's audit shows a data copy) | agent |
//! | `label_take` (the service's own label) | agent |
//! | `domain_add` (each other subdomain pointing at the service's port) | agent |
//! | `stop_old` (the old service stopped) | agent |
//!
//! **The request** files one held `manage` request (the existing Approvals
//! flow: one card in Settings → App Approvals, `k2 app approvals`) whose
//! body is the owner steps not done yet, with their parameters fixed at
//! request time. The owner approves it once, in person (a fresh vault
//! confirm bound to the request; never a passport, so an agent never
//! approves its own plan). Approval runs the steps in order as the owner,
//! each audited (the App's deploy audit and the Access Audit); a step that
//! is done by then is skipped; the first failure stops the rest. A step
//! never does more than the request showed: copy-host refuses when the
//! box-login list grew since the request, and the rooms are the ones the
//! request named.

use std::collections::{BTreeSet, HashMap};

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{self, codes, store};
use k2_core::published_services as ps;
use serde_json::{json, Value as J};

use super::routes::{actor, db_err, json_body, need_credential, no, s, ws_handle, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/move-plan", "/cli/app/move-plan/request"];
pub const POST_ROUTES: &[&str] = &["/cli/app/move-plan/request"];

/// The `route` a held move-plan request carries (never an HTTP route: the
/// approval runs [`apply`]).
pub const APPLY_ROUTE: &str = "/cli/app/move-plan/apply";

pub const NOT_LEGACY: &str = "not_a_cmd_service";
pub const NOTHING_TO_REQUEST: &str = "nothing_for_the_owner";

const OWNER: &str = "owner";
const AGENT: &str = "agent";
const DONE: &str = "done";
const TODO: &str = "todo";
const BLOCKED: &str = "blocked";
const NA: &str = "n/a";

/// The owner steps, in the order an approval runs them.
pub const OWNER_STEPS: &[&str] = &["box_prep", "engine", "build_switch", "copy_host", "guest_rooms"];

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/move-plan" => {
            need_credential(who)?;
            let p = plan(who, params.get("service").map(String::as_str), params.get("app").map(String::as_str))?;
            Ok(p.answer())
        }
        "/cli/app/move-plan/request" => request(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

// ── the plan ─────────────────────────────────────────────────────────

struct Plan {
    service: ps::PublishedService,
    app_id: String,
    app: Option<store::AppRow>,
    steps: Vec<J>,
}

impl Plan {
    fn owner_todo(&self) -> Vec<&J> {
        self.steps.iter().filter(|s| s["who"] == OWNER && s["state"] == TODO).collect()
    }

    fn answer(&self) -> J {
        let next = self.steps.iter().find(|s| s["state"] == TODO).map(|s| s["id"].clone()).unwrap_or(J::Null);
        let owner: Vec<J> = self.owner_todo().iter().map(|s| s["id"].clone()).collect();
        json!({
            "ok": true,
            "service": {
                "name": self.service.name,
                "serviceId": self.service.id,
                "workspace": self.service.project_id,
                "workspaceHandle": ws_handle(&self.service.project_id),
                "port": self.service.port,
                "public": self.service.expose == ps::EXPOSE_TUNNEL,
                "desired": self.service.desired,
            },
            "app": {
                "id": self.app_id,
                "installed": self.app.is_some(),
                "state": self.app.as_ref().map(|a| a.state.clone()),
                "suggested": self.app.is_none(),
            },
            "steps": self.steps,
            "next": next,
            "ownerStepsTodo": owner,
            "request": if owner.is_empty() { J::Null } else { json!(format!("k2 app move-plan {} --app {} --request", self.service.name, self.app_id)) },
            "done": self.steps.iter().all(|s| s["state"] == DONE || s["state"] == NA),
        })
    }
}

/// A sorted user list's fingerprint: the approval copies exactly the users
/// the request saw, or refuses (`plan_changed`).
fn users_hash(users: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut v: Vec<&str> = users.iter().map(String::as_str).collect();
    v.sort_unstable();
    Sha256::digest(v.join("\n").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// May this caller see box-login usernames? A human (owner, a login, a
/// shell tab), or an agent that already manages its Apps' users (switch 1).
fn may_see_users(who: &Who) -> bool {
    match super::routes::agent_ws(who) {
        Some(ws) => super::access::level(ws) >= super::access::Level::Build,
        None => true,
    }
}

fn step(id: &str, who: &str, state: &str, title: &str, detail: impl Into<String>, command: Option<String>) -> J {
    json!({ "id": id, "who": who, "state": state, "title": title, "detail": detail.into(), "command": command })
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(db_err)
}

/// The legacy service a call names (a passport: its own workspace's).
fn legacy_service(who: &Who, raw: Option<&str>) -> R<ps::PublishedService> {
    let t = super::grace::target(who, raw)?;
    let row = db(|c| ps::get_by_id(c, &t.service_id))?.ok_or_else(|| no(codes::APP_NOT_FOUND, 404, "the service vanished"))?;
    if !super::legacy::is_legacy(&row) {
        return Err(no(
            NOT_LEGACY,
            409,
            format!("'{}' isn't a --cmd service (it is an App or a --skin App): there is nothing to move", row.name),
        ));
    }
    Ok(row)
}

/// The App the service moves into: `app` when given; else the App that
/// took its label; else the only App of its workspace; else a suggestion
/// (`<name>2`, the id rules: k2 study app-move step 2).
fn target_app(who: &Who, svc: &ps::PublishedService, app: Option<&str>) -> R<(String, Option<store::AppRow>)> {
    if let Some(a) = app.map(str::trim).filter(|a| !a.is_empty()) {
        let id = a.to_ascii_lowercase();
        let row = db(|c| store::get_app(c, &id))?;
        return match row {
            Some(r) if r.owner_workspace == svc.project_id => Ok((r.app_id.clone(), Some(r))),
            // Another workspace's App: the same answer as no App (no leak
            // to a passport), and never a plan across workspaces.
            Some(_) if super::routes::agent_ws(who).is_some() => Err(no(codes::APP_NOT_FOUND, 404, format!("no App '{id}' in your workspace (k2 app list)"))),
            Some(_) => Err(no(codes::BAD_REQUEST, 409, format!("App '{id}' belongs to another workspace: a move stays inside the service's workspace"))),
            None => {
                if let Some((code, msg)) = deploy::app_id_problem(&id) {
                    return Err(no(code, 400, msg));
                }
                Ok((id, None))
            }
        };
    }
    if let Some(moved) = super::labels::moved_to(&svc.id) {
        let row = db(|c| store::get_app(c, &moved))?;
        return Ok((moved, row));
    }
    let mine = db(|c| store::list_apps(c, Some(&svc.project_id)))?;
    if let [only] = mine.as_slice() {
        return Ok((only.app_id.clone(), Some(only.clone())));
    }
    Ok((format!("{}2", svc.name), None))
}

/// Rooms the old service hands its guests today, when K2 can tell: its
/// own `guests-rooms` grant, and the room lists on its guests' grants.
fn rooms_in_use(svc: &ps::PublishedService) -> (Vec<String>, Vec<&'static str>) {
    let mut rooms: BTreeSet<String> = BTreeSet::new();
    let mut from: Vec<&'static str> = Vec::new();
    if let Ok(Some(g)) = k2_core::skin::guest_rooms::get_grant(&svc.id) {
        if !g.rooms.is_empty() {
            from.push("its guests-rooms grant");
        }
        rooms.extend(g.rooms);
    }
    if let Ok(grants) = k2_core::skin::list_grants() {
        let mut any = false;
        for g in grants.into_iter().filter(|g| g.kind == "app" && g.target_id == svc.id && g.enabled) {
            if let Some(list) = g.scope.as_deref().and_then(|sc| serde_json::from_str::<Vec<String>>(sc).ok()) {
                any |= !list.is_empty();
                rooms.extend(list);
            }
        }
        if any {
            from.push("its guests' grants");
        }
    }
    rooms.remove(&svc.project_id);
    (rooms.into_iter().collect(), from)
}

/// Other nested labels pointing at the service's port (hand-made with
/// `k2 publish subdomain create … --target localhost:<port>`).
fn extra_labels(svc: &ps::PublishedService) -> Vec<String> {
    if svc.port <= 0 {
        return Vec::new();
    }
    let at_port = |t: &str| {
        let t = t.trim();
        t == format!("127.0.0.1:{}", svc.port) || t == format!("localhost:{}", svc.port)
    };
    let mut out: BTreeSet<String> = BTreeSet::new();
    for (label, target) in k2_core::tunnel::subdomains::current().targets.iter() {
        if at_port(target) {
            out.insert(label.clone());
        }
    }
    if let Ok(rows) = db(|c| k2_core::db::schema::SubdomainWorkspace::list_for_project(c, &svc.project_id)) {
        for (label, target) in rows {
            if at_port(&target) {
                out.insert(label);
            }
        }
    }
    out.remove(&svc.name);
    out.into_iter().collect()
}

fn plan(who: &Who, service: Option<&str>, app: Option<&str>) -> R<Plan> {
    let svc = legacy_service(who, service)?;
    let (app_id, app_row) = target_app(who, &svc, app)?;
    let ws = svc.project_id.clone();
    let name = svc.name.clone();
    let mut steps: Vec<J> = Vec::new();

    // Box prep and the engine: only for a walled App on a Linux box.
    let process = app_row.as_ref().is_some_and(|a| a.isolation == "process");
    if !cfg!(target_os = "linux") || process {
        let why = if process { "this App runs on the process engine: no wall to prepare" } else { "not a Linux box: Apps run walled on Linux only" };
        steps.push(step("box_prep", OWNER, NA, "Prepare the box for Apps", why, None));
        steps.push(step("engine", OWNER, NA, "Install the App engine", why, None));
    } else {
        let doc = super::box_prep::doctor(&super::box_prep::gather());
        let failing: Vec<String> = doc["checks"]
            .as_array()
            .map(|a| a.iter().filter(|c| c["status"] == "fail" || c["status"] == "unknown").filter_map(|c| c["id"].as_str().map(str::to_string)).filter(|id| id != "engine").collect())
            .unwrap_or_default();
        let ready = doc["ready"] == true;
        steps.push(step(
            "box_prep",
            OWNER,
            if ready { DONE } else { TODO },
            "Prepare the box for Apps",
            if ready { "k2 app doctor: ready".to_string() } else { format!("k2 app doctor: not ready ({})", failing.join(", ")) },
            (!ready).then(|| "k2 app box prepare".to_string()),
        ));
        let engine_ok = doc["checks"].as_array().is_some_and(|a| a.iter().any(|c| c["id"] == "engine" && c["status"] == "ok"));
        steps.push(step(
            "engine",
            OWNER,
            if engine_ok { DONE } else { TODO },
            "Install the App engine",
            if engine_ok { "k2-app-engine answers" } else { "k2-app-engine isn't installed or doesn't answer" },
            (!engine_ok).then(|| "k2 app engine install".to_string()),
        ));
    }

    let build_on = super::access::level(&ws) >= super::access::Level::Build;
    steps.push(step(
        "build_switch",
        OWNER,
        if build_on { DONE } else { TODO },
        "Turn on \"Build and run Apps\" for the workspace",
        if build_on { "on".to_string() } else { format!("off for {}: every App change its agent makes waits for the owner", ws_handle(&ws)) },
        (!build_on).then(|| format!("k2 app access {} --build on", ws_handle(&ws))),
    ));

    let installed = app_row.is_some();
    steps.push(step(
        "install",
        AGENT,
        if installed { DONE } else { TODO },
        "Install the App",
        if installed { format!("App {app_id} exists") } else { format!("no App yet; '{app_id}' is a suggestion (any free App id; --app to pick)") },
        (!installed).then(|| format!("k2 app install {app_id}")),
    ));

    let live = app_row.as_ref().is_some_and(|a| a.live_seed.is_some() && a.state == store::STATE_RUNNING);
    steps.push(step(
        "deploy",
        AGENT,
        if live { DONE } else if installed { TODO } else { BLOCKED },
        "Deploy it",
        match &app_row {
            Some(a) if live => format!("live ({})", a.live_deployment.clone().unwrap_or_default()),
            Some(a) => format!("{} is {}", a.app_id, a.state),
            None => "after install".to_string(),
        },
        Some(format!("k2 app deploy {app_id} <folder>")),
    ));

    // Copy-host: the users who'd be copied (a dry run). Their names go to
    // callers who may list App users (a human, an agent with switch 1);
    // everyone gets the count and a hash the approval binds to.
    match &app_row {
        Some(a) => match k2_core::skin::copy_host_grants_to_app(&a.service_id, &actor(who), true) {
            Ok(rep) => {
                let users: Vec<String> = rep.added.clone();
                let mut st = step(
                    "copy_host",
                    OWNER,
                    if users.is_empty() { DONE } else { TODO },
                    "Give box-login users the same access on the App",
                    if users.is_empty() { "every box-login user already has it".to_string() } else { format!("{} user(s) would be copied", users.len()) },
                    (!users.is_empty()).then(|| format!("k2 app grant copy-host {}", a.app_id)),
                );
                st["userCount"] = json!(users.len());
                st["usersHash"] = json!(users_hash(&users));
                if may_see_users(who) {
                    st["users"] = json!(users);
                }
                steps.push(st);
            }
            Err(e) => steps.push(step("copy_host", OWNER, BLOCKED, "Give box-login users the same access on the App", format!("can't list them: {e}"), None)),
        },
        None => steps.push(step("copy_host", OWNER, BLOCKED, "Give box-login users the same access on the App", "after install", None)),
    }

    // The guests-rooms grant.
    let (rooms, from) = rooms_in_use(&svc);
    let handles: Vec<String> = rooms.iter().map(|r| ws_handle(r)).collect();
    match &app_row {
        Some(a) => {
            let have = k2_core::skin::guest_rooms::get_grant(&a.service_id).ok().flatten();
            let covered = have.as_ref().is_some_and(|g| rooms.iter().all(|r| g.rooms.contains(r)));
            let mut st = step(
                "guest_rooms",
                OWNER,
                if covered { DONE } else { TODO },
                "Let the App set its guests' rooms (guests-rooms grant)",
                match (&have, rooms.is_empty()) {
                    (Some(_), _) if covered => "granted".to_string(),
                    (_, true) => "the App's own room; K2 can't tell which other rooms the old service hands out (add --room to the grant if it does)".to_string(),
                    _ => format!("rooms the old service hands out ({}): {}", from.join(", "), handles.join(", ")),
                },
                (!covered).then(|| {
                    let r: String = handles.iter().map(|h| format!(" --room {h}")).collect();
                    format!("k2 app grant {} --kind {}{r}", a.app_id, k2_core::skin::guest_rooms::GRANT_KIND)
                }),
            );
            st["rooms"] = json!(rooms);
            st["roomHandles"] = json!(handles);
            steps.push(st);
        }
        None => {
            let mut st = step("guest_rooms", OWNER, BLOCKED, "Let the App set its guests' rooms (guests-rooms grant)", "after install", None);
            st["rooms"] = json!(rooms);
            st["roomHandles"] = json!(handles);
            steps.push(st);
        }
    }

    // Data: the App's audit shows a copy.
    let copied = app_row.as_ref().and_then(|a| {
        let h = k2_core::db::shared();
        let conn = h.lock();
        deploy::audit::list_for(&conn, deploy::audit::SUBJECT_APP, &a.app_id, 500).ok()?.into_iter().find(|r| r.event == "data_put").map(|r| r.at)
    });
    steps.push(step(
        "data_put",
        AGENT,
        if copied.is_some() { DONE } else if live { TODO } else { BLOCKED },
        "Copy the service's data into the App",
        match copied {
            Some(at) => format!("copied (last at {at}); copy again right before the label moves if the old service still writes"),
            None if live => format!("nothing copied yet: the service's folder is {}", svc.cwd),
            None => "after the App is live".to_string(),
        },
        Some(format!("k2 app data put {app_id} <your-data-folder>")),
    ));

    // The labels.
    let public = svc.expose == ps::EXPOSE_TUNNEL;
    let took = super::labels::moved_to(&svc.id).is_some_and(|a| a == app_id);
    steps.push(step(
        "label_take",
        AGENT,
        if took { DONE } else if !public { NA } else if live { TODO } else { BLOCKED },
        "Move the service's own hostname to the App",
        if took {
            format!("{name} serves {app_id}")
        } else if !public {
            format!("{name} has no public hostname (--no-tunnel): point what calls 127.0.0.1:{} at the App instead", svc.port)
        } else if live {
            format!("{name} still serves the old service")
        } else {
            "after the App is live".to_string()
        },
        (public && !took).then(|| format!("k2 app label take {app_id} {name}")),
    ));
    let attached: Vec<String> = app_row
        .as_ref()
        .and_then(|a| super::labels::load_cfg(&a.app_id).ok())
        .map(|c| c.subdomains.into_iter().map(|d| d.label).collect())
        .unwrap_or_default();
    for label in extra_labels(&svc) {
        let done = attached.contains(&label);
        let mut st = step(
            "domain_add",
            AGENT,
            if done { DONE } else if live { TODO } else { BLOCKED },
            "Move another hostname pointing at the service's port",
            if done { format!("{label} serves {app_id}") } else { format!("{label} points at port {}", svc.port) },
            (!done).then(|| format!("k2 app domain add {app_id} {label}")),
        );
        st["label"] = json!(label);
        steps.push(st);
    }

    let stopped = svc.desired != "running";
    steps.push(step(
        "stop_old",
        AGENT,
        if stopped { DONE } else if took || !public { TODO } else { BLOCKED },
        "Stop the old service (the move is done after this)",
        if stopped { format!("{name} is stopped") } else { format!("{name} still runs (unrouted once its label moved)") },
        (!stopped).then(|| format!("k2 publish stop {name}")),
    ));

    Ok(Plan { service: svc, app_id, app: app_row, steps })
}

// ── one request for the owner's steps ────────────────────────────────

/// POST `/cli/app/move-plan/request {service, app?, reason?}`.
fn request(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let p = plan(who, s(&v, "service"), s(&v, "app"))?;
    let todo = p.owner_todo();
    if todo.is_empty() {
        let mut out = p.answer();
        out["status"] = json!("nothing_to_request");
        out["note"] = json!("every owner step is done, or waits on an earlier step (install, deploy): run --request again then");
        return Ok(out);
    }
    let steps: Vec<J> = todo
        .iter()
        .map(|s| {
            let mut x = json!({ "id": s["id"], "title": s["title"] });
            for k in ["users", "userCount", "usersHash", "rooms"] {
                if let Some(v) = s.get(k) {
                    x[k] = v.clone();
                }
            }
            x
        })
        .collect();
    let lines: Vec<String> = todo
        .iter()
        .map(|s| {
            let extra = match s["id"].as_str() {
                Some("copy_host") => match s["users"].as_array() {
                    Some(u) => format!(": {}", u.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", ")),
                    None => format!(": {} user(s) (k2 app move-plan {} names them)", s["userCount"], p.service.name),
                },
                Some("guest_rooms") => {
                    let h: Vec<&str> = s["roomHandles"].as_array().map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
                    if h.is_empty() { " (its own room)".to_string() } else { format!(": {}", h.join(", ")) }
                }
                _ => String::new(),
            };
            format!("{}{extra}", s["title"].as_str().unwrap_or(""))
        })
        .collect();
    let label = p.app_id.clone();
    let summary = format!("move {} into App {}: {} owner step(s) — {}", p.service.name, p.app_id, steps.len(), lines.join("; "));
    let r = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: p.app.as_ref(),
        label: &label,
        workspace: &p.service.project_id,
        body: json!({
            "route": APPLY_ROUTE,
            "body": {
                "service": p.service.id,
                "serviceName": p.service.name,
                "app": p.app_id,
                "workspace": p.service.project_id,
                "steps": steps,
            },
        }),
        diff: json!({ "lines": lines, "route": APPLY_ROUTE }),
        summary,
        reasons: vec!["the owner's steps of a move to the App runtime (k2 app move-plan)".into()],
        reason: s(&v, "reason"),
    })?;
    let mut out = super::approvals::held_answer(&r);
    out["plan"] = p.answer();
    out["hint"] = json!(format!(
        "one request for {} owner step(s): the owner approves it once, in person (k2 app approve {} from a terminal outside K2, \
         or Settings → App Approvals); the steps then run in order, each audited. Check with k2 app approvals.",
        steps.len(),
        r.id
    ));
    Ok(out)
}

// ── the approval ─────────────────────────────────────────────────────

fn audit_step(app: Option<&str>, request: &str, by: &str, step: &str, outcome: &J) {
    if let Some(a) = app {
        let h = k2_core::db::shared();
        let conn = h.lock();
        if let Err(e) = deploy::audit::append(
            &conn,
            deploy::audit::SUBJECT_APP,
            a,
            "move_plan_step",
            by,
            None,
            &json!({ "request": request, "step": step, "outcome": outcome }),
        ) {
            k2_core::log_debug!("[apps] audit {a} move_plan_step: {e}");
        }
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-move-plan",
        by,
        format!("{} {request} {step}", outcome["result"].as_str().unwrap_or("?")),
        "cli",
        "-",
        "apps",
    ));
}

fn str_list(v: &J) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// Run one approved step as the owner. `Ok(Some(what))` ran, `Ok(None)`
/// was already done.
fn run_step(st: &J, svc: &ps::PublishedService, app_id: &str, ws: &str, by: &str) -> R<Option<J>> {
    let id = st["id"].as_str().unwrap_or("");
    let app = db(|c| store::get_app(c, app_id))?;
    match id {
        "box_prep" => {
            let doc = super::box_prep::doctor(&super::box_prep::gather());
            if doc["ready"] == true {
                return Ok(None);
            }
            super::box_prep::prepare_route(&Who::Owner, &json!({})).map(Some)
        }
        "engine" => {
            let doc = super::box_prep::doctor(&super::box_prep::gather());
            if doc["checks"].as_array().is_some_and(|a| a.iter().any(|c| c["id"] == "engine" && c["status"] == "ok")) {
                return Ok(None);
            }
            super::engine_install::install_route(&Who::Owner, &json!({})).map(Some)
        }
        "build_switch" => {
            if super::access::level(ws) >= super::access::Level::Build {
                return Ok(None);
            }
            super::access::set_switches(ws, super::access::SwitchChange { build: Some(true), full: None }, true, by).map(Some)
        }
        "copy_host" => {
            let a = app.ok_or_else(|| no(codes::APP_NOT_FOUND, 409, format!("App {app_id} isn't installed")))?;
            let now = k2_core::skin::copy_host_grants_to_app(&a.service_id, by, true).map_err(|e| no("copy_host_failed", 409, e))?;
            if now.added.is_empty() {
                return Ok(None);
            }
            // Exactly the users the request saw (and the owner approved).
            if st["usersHash"].as_str() != Some(users_hash(&now.added).as_str()) {
                return Err(no(
                    "plan_changed",
                    409,
                    format!(
                        "the box-login users to copy changed since the request ({} then, {} now): nothing copied; request again so the owner sees them",
                        st["userCount"],
                        now.added.len()
                    ),
                ));
            }
            let rep = k2_core::skin::copy_host_grants_to_app(&a.service_id, by, false).map_err(|e| no("copy_host_failed", 409, e))?;
            Ok(Some(json!({ "added": rep.added })))
        }
        "guest_rooms" => {
            let a = app.ok_or_else(|| no(codes::APP_NOT_FOUND, 409, format!("App {app_id} isn't installed")))?;
            let rooms = str_list(&st["rooms"]);
            if let Ok(Some(g)) = k2_core::skin::guest_rooms::get_grant(&a.service_id) {
                if rooms.iter().all(|r| g.rooms.contains(r)) {
                    return Ok(None);
                }
            }
            let body = json!({ "app": a.app_id, "kind": k2_core::skin::guest_rooms::GRANT_KIND, "rooms": rooms }).to_string();
            let resp = super::guest_rooms::handle("/cli/app/grant", &Who::Owner, true, body.as_bytes(), &HashMap::new());
            let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
            if resp.status.starts_with('2') {
                Ok(Some(v))
            } else {
                Err(no(
                    "grant_failed",
                    409,
                    format!("{}: {}", v.pointer("/error/code").and_then(J::as_str).unwrap_or("failed"), v.pointer("/error/hint").and_then(J::as_str).unwrap_or("")),
                ))
            }
        }
        other => Err(no(codes::BAD_REQUEST, 400, format!("'{other}' isn't an owner step of a move ({svc_name})", svc_name = svc.name))),
    }
}

/// An approved move-plan request: the owner steps it names, in order, as
/// the owner; each audited; the first failure stops the rest.
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> R<J> {
    let b = r.body.get("body").cloned().unwrap_or(J::Null);
    let svc_id = b["service"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable move-plan request"))?;
    let app_id = b["app"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable move-plan request"))?;
    let ws = b["workspace"].as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable move-plan request"))?;
    let svc = db(|c| ps::get_by_id(c, svc_id))?.ok_or_else(|| no(codes::APP_NOT_FOUND, 409, "the service in this request is gone"))?;
    if svc.project_id != ws {
        return Err(no(codes::FORBIDDEN, 409, "the service changed workspace since the request: request again"));
    }
    if let Some(a) = db(|c| store::get_app(c, app_id))? {
        if a.owner_workspace != ws {
            return Err(no(codes::FORBIDDEN, 409, format!("App {app_id} belongs to another workspace now: request again")));
        }
    }
    let steps = b["steps"].as_array().cloned().unwrap_or_default();
    let mut done: Vec<J> = Vec::new();
    for st in &steps {
        let id = st["id"].as_str().unwrap_or("").to_string();
        if !OWNER_STEPS.contains(&id.as_str()) {
            return Err(no(codes::BAD_REQUEST, 400, format!("'{id}' isn't an owner step")));
        }
        let audited_app = db(|c| store::get_app(c, app_id))?.map(|a| a.app_id);
        match run_step(st, &svc, app_id, ws, by) {
            Ok(ran) => {
                let outcome = match ran {
                    Some(v) => json!({ "step": id, "result": "ran", "answer": v }),
                    None => json!({ "step": id, "result": "already done" }),
                };
                audit_step(audited_app.as_deref(), &r.id, by, &id, &outcome);
                done.push(outcome);
            }
            Err(e) => {
                let outcome = json!({ "step": id, "result": "failed", "code": e.code, "hint": e.hint });
                audit_step(audited_app.as_deref(), &r.id, by, &id, &outcome);
                let ran: Vec<String> = done.iter().filter_map(|d| d["step"].as_str().map(str::to_string)).collect();
                return Err(no(
                    "move_step_failed",
                    409,
                    format!(
                        "step {id} failed ({}: {}); {}; the rest did not run",
                        e.code,
                        e.hint,
                        if ran.is_empty() { "nothing ran before it".to_string() } else { format!("done before it: {}", ran.join(", ")) }
                    ),
                ));
            }
        }
    }
    Ok(json!({ "applied": "move-plan", "service": svc.name, "app": app_id, "steps": done }))
}
