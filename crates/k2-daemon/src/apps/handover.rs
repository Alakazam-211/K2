//! App handover: one App's maintenance moves from one workspace (agent)
//! to another on the same box (prd-app-handoff-v1 rev 2; Rosson
//! 2026-10-26: one verb, `k2 app handover`, improved).
//!
//! ```text
//! GET  /cli/app/handover/plan  ?app=&to=[&db=share&collisions=keep|use_b]   what would move (read only; --dry-run)
//! POST /cli/app/handover       {app, to, brief?, db?, collisions?, resetShared?, copySource?, reason?, dryRun?}
//! POST /cli/app/unpark         {app, to, …}                                   a parked App: the same card
//! ```
//!
//! **The plan** (`handover_plan.rs`) lists, from live rows, everything
//! §4.2 names: labels, domains, guests by class, rooms, secrets and
//! `.env.k2` references, the database case, App keys, A's waiting
//! requests, B's switch and budget, the re-key mode. Its `planHash` binds
//! the card to what it showed.
//!
//! **The card** (A18): every handover is a held `manage` request whose
//! apply route is [`APPLY_ROUTE`] (never an HTTP route; the move-plan
//! pattern). The owner approves it in person with a fresh `app-approve`
//! vault confirm (`k2 app approve`, Settings → App Approvals); there is no
//! "Always allow", and no passport ever approves. The approval recomputes
//! the plan: a different hash, A no longer the App's workspace, or B gone
//! makes the handover `stale` and nothing moves.
//!
//! **Apply** (§4.8), as the owner:
//! 0. re-plan + hash check; the App is marked busy (writes on it answer
//!    `409 handover_in_progress`, the re-host wave skips it);
//! 1. B's "Build and run Apps" turned on when off (the card said so);
//! 2. **the commit point**, one daemon-DB transaction: the App's and its
//!    service's workspace, every label (the App's own, taken and domain
//!    labels; taken labels become final), App-key room ceilings, A's
//!    waiting requests withdrawn (`handed_over`), `folder_path` cleared,
//!    the row `applied` with its undo journal, the audit row;
//! 3. skin.db: the App's room A → B (roles, grants, live sessions, the
//!    guests-rooms grant, default rooms, test guests); A's own agent
//!    logins lose this App;
//! 4. vault (S4);
//! 5. runtime: the link re-binds to B (a new pass, the old ones revoked),
//!    the alert map, the mirror, both workspaces refreshed;
//! 6. re-key (S4);
//! 7. notes to both agents.
//!
//! Steps 3–7 roll forward (each is idempotent: "already B" is success).
//!
//! **Who** may see a plan: the App's own agent (its passport; another
//! workspace's App is `app_not_found`, no existence leak), the owner, a
//! shell tab, an Owner or Admin login. A Member login is refused (the plan
//! names guests). Only the owner (the owner token or an Owner login) asks
//! for the card.
//!
//! **`K2_APP_HANDOVER`**: `legacy` keeps 0.46.3's handover for one release
//! (the new routes answer 404); `off` refuses every handover. Retirement
//! ledger: removal in 0.46.6.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::handover::{self as hs, Handover};
use k2_core::deploy::{self, codes, store, store::AppRow};
use serde_json::{json, Value as J};

use super::handover_plan::{self as hp, Options, Plan};
use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, ws_by_id, ws_by_token, ws_handle, Refusal, Who, Ws, R};

pub const ROUTES: &[&str] = &["/cli/app/handover/plan"];
pub const POST_ROUTES: &[&str] = &[];

/// The `route` the handover card's held request carries (never an HTTP
/// route: the approval runs [`apply`]).
pub const APPLY_ROUTE: &str = "/cli/app/handover/apply";

pub const HANDOVER_OFF: &str = "handover_off";
pub const SAME_WORKSPACE: &str = "handover_same_workspace";
pub const OPEN: &str = "handover_open";
pub const IN_PROGRESS: &str = "handover_in_progress";
pub const STALE: &str = "handover_stale";
pub const NOT_HELD: &str = "handover_not_held";

/// `K2_APP_HANDOVER`: `legacy` | `off` | anything else (the handover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    On,
    Legacy,
    Off,
}

#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<Mode>> = const { std::cell::Cell::new(None) };
}

/// Tests: this thread's handover mode (`None` = the env), never a
/// process-wide env var other tests would see.
#[cfg(test)]
pub(crate) fn set_test_mode(m: Option<Mode>) {
    TEST_MODE.with(|c| c.set(m));
}

/// Tests: mark `app` as applying until the guard drops.
#[cfg(test)]
pub(crate) fn test_busy(app: &str) -> impl Drop {
    Busy::enter(app, "a", "b")
}

pub fn mode() -> Mode {
    #[cfg(test)]
    if let Some(m) = TEST_MODE.with(|c| c.get()) {
        return m;
    }
    match std::env::var("K2_APP_HANDOVER").ok().map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("legacy") => Mode::Legacy,
        Some("off") => Mode::Off,
        _ => Mode::On,
    }
}

pub(crate) fn off_refusal() -> Refusal {
    no(HANDOVER_OFF, 403, "App handovers are off on this server (K2_APP_HANDOVER=off)")
}

// ── busy: an App whose handover is applying ──────────────────────────

fn handing() -> &'static Mutex<HashMap<String, (String, String)>> {
    static H: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Is `app_id`'s handover applying right now?
pub fn busy(app_id: &str) -> bool {
    let m = handing().lock().unwrap_or_else(|e| e.into_inner());
    !m.is_empty() && m.contains_key(app_id)
}

/// Any handover applying (a fast check before parsing a body).
pub fn any_busy() -> bool {
    !handing().lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

struct Busy(String);

impl Busy {
    fn enter(app: &str, from: &str, to: &str) -> Busy {
        handing().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), (from.to_string(), to.to_string()));
        Busy(app.to_string())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        handing().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// A write on an App whose handover is applying waits (`409
/// handover_in_progress`). `None` = go ahead.
pub(crate) fn refuse_if_busy(path: &str, body: &[u8]) -> Option<Refusal> {
    if !any_busy() || path.starts_with("/cli/app/handover") {
        return None;
    }
    let v: J = serde_json::from_slice(body).ok()?;
    let app = v.get("app").and_then(J::as_str)?.trim().to_ascii_lowercase();
    let id = app.rsplit('/').next().unwrap_or(&app).to_string();
    busy(&id).then(|| no(IN_PROGRESS, 409, format!("{id} is being handed over right now: try again in a moment")))
}

// ── routes ───────────────────────────────────────────────────────────

/// May `who` read a handover plan? The App's own agent, the owner, a
/// shell tab, an Owner or Admin login.
fn may_read(who: &Who) -> R<()> {
    need_credential(who)?;
    match who {
        Who::Login { role: Role::Member, .. } => {
            Err(no(codes::OWNER_ONLY, 403, "a Member login can't see a handover plan (it names the App's guests): ask the owner or an admin"))
        }
        _ => Ok(()),
    }
}

pub(crate) fn handle(path: &str, who: &Who, _body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match mode() {
        Mode::Legacy => return Err(no("not_found", 404, "no such App route (K2_APP_HANDOVER=legacy)")),
        Mode::Off => return Err(off_refusal()),
        Mode::On => {}
    }
    match path {
        "/cli/app/handover/plan" => plan_route(who, params),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// The options a request names (body or query).
pub(crate) fn options_from(get: impl Fn(&str) -> Option<String>, b: impl Fn(&str) -> bool) -> R<Options> {
    let db = get("db");
    if let Some(d) = db.as_deref() {
        if d != "share" && d != "wait" {
            return Err(no(codes::BAD_REQUEST, 400, format!("db is share or wait, not '{d}'")));
        }
    }
    let collisions = get("collisions");
    if let Some(c) = collisions.as_deref() {
        if c != "keep" && c != "use_b" {
            return Err(no(codes::BAD_REQUEST, 400, format!("collisions is keep or use_b, not '{c}'")));
        }
    }
    Ok(Options { db, collisions, reset_shared: b("resetShared"), copy_source: b("copySource") })
}

fn body_options(v: &J) -> R<Options> {
    options_from(|k| s(v, k).map(str::to_string), |k| v.get(k).and_then(J::as_bool).unwrap_or(false))
}

/// The App and the taking workspace a call names.
pub(crate) fn target(who: &Who, app: Option<&str>, to: Option<&str>) -> R<(AppRow, Ws)> {
    let app = app_for(who, app)?;
    let to_raw = to.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace that takes the App (--to)"))?;
    let to = ws_by_token(to_raw).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{to_raw}'")))?;
    if to.id == app.owner_workspace {
        return Err(no(SAME_WORKSPACE, 409, format!("{} already belongs to {}", app.app_id, ws_handle(&to.id))));
    }
    Ok((app, to))
}

/// GET `/cli/app/handover/plan?app=&to=`: the plan, read only.
fn plan_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    may_read(who)?;
    let (app, to) = target(who, param(params, "app"), param(params, "to"))?;
    let opts = options_from(|k| param(params, k).map(str::to_string), |k| matches!(param(params, k), Some("1" | "true" | "yes")))?;
    let p = hp::compute(&app, &to, &opts, None)?;
    Ok(plan_answer(&p))
}

fn plan_answer(p: &Plan) -> J {
    let mut out = p.json(true);
    out["ok"] = json!(true);
    out["ready"] = json!(p.blockers.is_empty());
    out
}

/// The brief A writes for B (markdown, at most 64 KiB).
fn brief_of(v: &J) -> R<Option<String>> {
    match s(v, "brief") {
        None => Ok(None),
        Some(b) if b.len() > hs::BRIEF_MAX => Err(no(codes::BAD_REQUEST, 413, format!("a brief is at most {} KiB of markdown", hs::BRIEF_MAX / 1024))),
        Some(b) => Ok(Some(b.to_string())),
    }
}

/// POST `/cli/app/handover` / `/cli/app/unpark`: the owner asks for the
/// card (or, with `dryRun`, sees the plan).
pub(crate) fn request_route(who: &Who, body: &[u8], unpark: bool) -> R<J> {
    super::ownership::need_owner(who, if unpark { "Unparking an App" } else { "Handing an App over" })?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    if unpark && app.state != store::STATE_PARKED {
        return Err(no(
            super::ownership::NOT_PARKED,
            409,
            format!("{} isn't parked; hand it over instead (k2 app handover {} --to <workspace>)", app.app_id, app.app_id),
        ));
    }
    let to_raw = s(&v, "to").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace that takes it (to)"))?;
    let to = ws_by_token(to_raw).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{to_raw}'")))?;
    if app.owner_workspace == to.id {
        return Ok(json!({ "ok": true, "app": app.app_id, "workspace": ws_handle(&to.id), "already": true }));
    }
    let opts = body_options(&v)?;
    let brief = brief_of(&v)?;
    if super::routes::b(&v, "dryRun") {
        return Ok(plan_answer(&hp::compute(&app, &to, &opts, brief.as_deref())?));
    }
    file_card(who, &app, &to, &opts, brief.as_deref(), s(&v, "reason"), &actor(who))
}

fn hov_db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(db_err)
}

/// One handover as the routes answer it.
pub(crate) fn row_json(h: &Handover) -> J {
    let mut v = serde_json::to_value(h).unwrap_or(J::Null);
    v["fromHandle"] = if h.from_workspace.is_empty() { J::Null } else { json!(ws_handle(&h.from_workspace)) };
    v["toHandle"] = json!(ws_handle(&h.to_workspace));
    v["hasBrief"] = json!(h.brief.is_some());
    v
}

/// Refuse with the plan's first blocker (all of them in `blockers`).
fn blocked(p: &Plan) -> Refusal {
    let first = &p.blockers[0];
    let mut r = no(first.code, 409, first.hint.clone());
    r.extra = Some(json!({ "blockers": p.blockers, "plan": p.json(true) }));
    r
}

/// File the owner's card for `app` → `to`: the plan, the row (`held`),
/// the held request. Idempotent: the same App and B with a card already
/// waiting answers that card.
pub(crate) fn file_card(who: &Who, app: &AppRow, to: &Ws, opts: &Options, brief: Option<&str>, reason: Option<&str>, started_by: &str) -> R<J> {
    let p = hp::compute(app, to, opts, brief)?;
    if !p.blockers.is_empty() {
        return Err(blocked(&p));
    }
    let hash = p.hash();
    let plan_json = p.json(true);
    let options = serde_json::to_value(opts).unwrap_or(J::Null);
    let n = hs::NewHandover {
        app_id: &app.app_id,
        service_id: &app.service_id,
        from: &app.owner_workspace,
        to: &to.id,
        state: hs::HELD,
        started_by,
        plan: &plan_json,
        plan_hash: &hash,
        options: &options,
        brief,
        reason,
    };
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        match hs::insert(&conn, &n, deploy::now()) {
            Ok(r) => r,
            Err(hs::InsertError::Conflict) => {
                let open = hs::open_for_app(&conn, &app.app_id).map_err(db_err)?;
                drop(conn);
                return match open {
                    Some(o) if o.state == hs::HELD && o.to_workspace == to.id => existing_card(&o),
                    Some(o) => {
                        let mut r = no(OPEN, 409, format!("{} already has an open handover ({}, {} to {}): cancel it first", app.app_id, o.id, o.state, ws_handle(&o.to_workspace)));
                        r.extra = Some(json!({ "handover": row_json(&o) }));
                        Err(r)
                    }
                    None => Err(no(OPEN, 409, "another handover of this App just started: try again")),
                };
            }
            Err(hs::InsertError::Db(e)) => return Err(db_err(e)),
        }
    };
    let lines = p.lines();
    let summary = format!("hand over {} from {} to {}", app.app_id, from_label(&p.room_from), ws_handle(&to.id));
    let req = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: Some(app),
        label: &app.app_id,
        workspace: if app.owner_workspace.is_empty() { &p.room_from } else { &app.owner_workspace },
        body: json!({
            "route": APPLY_ROUTE,
            "body": { "handover": row.id, "app": app.app_id, "serviceId": app.service_id, "from": app.owner_workspace, "to": to.id, "planHash": hash },
        }),
        diff: json!({ "lines": lines, "route": APPLY_ROUTE, "handover": row.id }),
        summary,
        reasons: vec!["an App handover moves guests, secrets and keys between agents: the owner approves it in person".into()],
        reason,
    });
    let req = match req {
        Ok(r) => r,
        Err(e) => {
            let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::HELD], hs::ABORTED, deploy::now()));
            return Err(e);
        }
    };
    hov_db(|c| hs::set_request_id(c, &row.id, &req.id, deploy::now()))?;
    let row = hov_db(|c| hs::get(c, &row.id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
    let mut out = super::approvals::held_answer(&req);
    out["handover"] = row_json(&row);
    out["plan"] = plan_json;
    out["hint"] = json!(format!(
        "the handover waits for the owner's approval in person: k2 app approve {} from a terminal outside K2, or Settings → App Approvals \
         (a fresh vault confirm; nothing moves until then)",
        req.id
    ));
    Ok(out)
}

fn existing_card(o: &Handover) -> R<J> {
    let rid = o.request_id.clone().ok_or_else(|| no(OPEN, 409, "the open handover has no card yet: try again"))?;
    let req = hov_db(|c| reqs::get(c, &rid))?.ok_or_else(|| no(OPEN, 409, "the open handover's card is gone"))?;
    let mut out = super::approvals::held_answer(&req);
    out["handover"] = row_json(o);
    out["plan"] = o.plan.clone();
    out["already"] = json!(true);
    Ok(out)
}

fn from_label(room_from: &str) -> String {
    if room_from.is_empty() {
        "the owner (parked)".into()
    } else {
        ws_handle(room_from)
    }
}

// ── the card closes without applying ─────────────────────────────────

/// The card's request was denied, withdrawn or expired: the handover ends
/// the same way (`denied` | `cancelled` | `expired`).
pub(crate) fn request_closed(r: &reqs::AppRequest) {
    if r.body.get("route").and_then(J::as_str) != Some(APPLY_ROUTE) {
        return;
    }
    let to = match r.status.as_str() {
        reqs::STATUS_DENIED => hs::DENIED,
        reqs::STATUS_WITHDRAWN => hs::CANCELLED,
        reqs::STATUS_EXPIRED => hs::EXPIRED,
        _ => return,
    };
    let Some(id) = r.body.pointer("/body/handover").and_then(J::as_str) else { return };
    if let Err(e) = hov_db(|c| hs::transition(c, id, &[hs::OFFERED, hs::ACCEPTED, hs::HELD], to, deploy::now())) {
        k2_core::log_debug!("[apps/handover] {id}: closing with its card: {}", e.hint);
    }
}

// ── apply (the owner approved the card) ──────────────────────────────

fn stale(row: &Handover, why: String) -> Refusal {
    let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::HELD], hs::STALE, deploy::now()));
    no(STALE, 409, format!("{why}: nothing moved. Ask again so the card shows what would move now (k2 app handover {} --to <workspace>)", row.app_id))
}

/// An approved handover card: re-plan, check the hash, then run every
/// step as the owner (`by`).
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> R<J> {
    match mode() {
        Mode::On => {}
        Mode::Legacy => return Err(no("not_found", 409, "App handovers are in legacy mode on this server (K2_APP_HANDOVER=legacy)")),
        Mode::Off => return Err(off_refusal()),
    }
    let id = r.body.pointer("/body/handover").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable handover card"))?;
    let row = hov_db(|c| hs::get(c, id))?.ok_or_else(|| no(NOT_HELD, 409, format!("no handover '{id}'")))?;
    if row.state != hs::HELD || row.request_id.as_deref() != Some(r.id.as_str()) {
        return Err(no(NOT_HELD, 409, format!("handover {} is {}, not waiting for this card", row.id, row.state)));
    }
    let Some(app) = hov_db(|c| store::get_app(c, &row.app_id))? else {
        let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::HELD], hs::ABORTED, deploy::now()));
        return Err(no(codes::APP_NOT_FOUND, 409, format!("App {} is gone", row.app_id)));
    };
    if app.owner_workspace != row.from_workspace {
        return Err(stale(&row, format!("{} belongs to {} now", app.app_id, from_label(&app.owner_workspace))));
    }
    let Some(to) = ws_by_id(&row.to_workspace) else {
        return Err(stale(&row, "the workspace that takes it is gone".into()));
    };
    let opts: Options = serde_json::from_value(row.options.clone()).unwrap_or_default();
    let plan = hp::compute(&app, &to, &opts, row.brief.as_deref())?;
    if plan.hash() != row.plan_hash || !plan.blockers.is_empty() {
        return Err(stale(&row, "what the handover would move changed since the card was asked for (a guest, a secret, a label or a request)".into()));
    }
    if !hov_db(|c| hs::set_approved(c, &row.id, by, deploy::now()))? {
        return Err(no(NOT_HELD, 409, format!("handover {} was answered already", row.id)));
    }
    run(&row.id, &plan, by)
}

/// The commit point and the steps after it.
fn run(id: &str, plan: &Plan, by: &str) -> R<J> {
    let _busy = Busy::enter(&plan.app.app_id, &plan.room_from, &plan.to.id);
    // Step 1: B needs switch 1 to start, deploy or restart the App (the
    // card said "turns on"); the owner's approval is the raise.
    let raised = if !plan.to_build {
        match super::access::set_switches(&plan.to.id, super::access::SwitchChange { build: Some(true), full: None }, true, by) {
            Ok(_) => true,
            Err(e) => {
                let _ = hov_db(|c| hs::transition(c, id, &[hs::APPLYING], hs::ABORTED, deploy::now()));
                return Err(e);
            }
        }
    } else {
        false
    };
    // Step 2: the commit point.
    if let Err(e) = commit(id, plan, by, raised) {
        if raised {
            let _ = super::access::set_switches(&plan.to.id, super::access::SwitchChange { build: Some(false), full: None }, true, by);
        }
        let _ = hov_db(|c| hs::transition(c, id, &[hs::APPLYING], hs::ABORTED, deploy::now()));
        return Err(e);
    }
    let summary = finish_steps(id, by);
    let row = hov_db(|c| hs::get(c, id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
    Ok(json!({
        "applied": "handover",
        "app": plan.app.app_id,
        "from": from_label(&plan.room_from),
        "to": ws_handle(&plan.to.id),
        "handover": row_json(&row),
        "steps": summary,
        "undoUntil": row.window_until,
        "note": format!(
            "{} is {}'s now. You can undo this until the window ends (k2 app handover undo {}).",
            plan.app.app_id,
            ws_handle(&plan.to.id),
            plan.app.app_id
        ),
    }))
}

/// Step 2: one daemon-DB transaction (§4.8). Writes the undo journal.
fn commit(id: &str, plan: &Plan, by: &str, raised: bool) -> R<()> {
    let app = &plan.app;
    let (from, room_from, to) = (plan.from.as_str(), plan.room_from.as_str(), plan.to.id.as_str());
    let now = deploy::now();
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut cfg = k2_core::app_grace::config::load(&conn, &app.app_id).map_err(db_err)?;
    let taken_before = serde_json::to_value(&cfg.labels).unwrap_or(J::Null);
    for t in cfg.labels.iter_mut() {
        // Q5: final. `label return` never points it back at A's service.
        t.service_id.clear();
    }
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let (state, reason) = if app.state == store::STATE_PARKED {
        (store::STATE_STOPPED, Some(format!("unparked by {by}; start it with k2 app start {}", app.app_id)))
    } else {
        (app.state.as_str(), app.state_reason.clone())
    };
    tx.execute(
        "UPDATE apps SET owner_workspace = ?2, state = ?3, state_reason = ?4, folder_path = NULL, config_json = ?5, updated_at = ?6 WHERE app_id = ?1",
        rusqlite::params![app.app_id, to, state, reason, serde_json::to_string(&cfg).unwrap_or_default(), now],
    )
    .map_err(db_err)?;
    tx.execute("UPDATE published_services SET project_id = ?2 WHERE id = ?1", rusqlite::params![app.service_id, to]).map_err(db_err)?;
    // P3: every label the App's workspace (or nobody, parked) holds.
    let mut labels = Vec::new();
    for l in &plan.labels {
        match l.holder.as_deref() {
            Some(holder) if holder == from => {
                tx.execute(
                    "UPDATE subdomain_workspaces SET project_id = ?3 WHERE label = ?1 AND project_id = ?2",
                    rusqlite::params![l.label, from, to],
                )
                .map_err(db_err)?;
                labels.push(json!({ "label": l.label, "before": holder }));
            }
            None => {
                k2_core::db::schema::SubdomainWorkspace::self_stamp(&tx, &l.label, to, None).map_err(db_err)?;
                labels.push(json!({ "label": l.label, "before": J::Null }));
            }
            Some(_) => {}
        }
    }
    // P12: the App keys' room ceilings name B.
    let mut key_rooms = serde_json::Map::new();
    for k in &plan.keys {
        let before: Option<String> = tx.query_row("SELECT app_key FROM api_keys WHERE id = ?1", rusqlite::params![k.id], |r| r.get(0)).map_err(db_err)?;
        let Some(before) = before else { continue };
        let Ok(mut g) = serde_json::from_str::<J>(&before) else { continue };
        if let Some(rooms) = g.get("rooms").and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect::<Vec<_>>()) {
            if rooms.iter().any(|r| r == room_from) {
                g["rooms"] = json!(k2_core::skin::handover::swap_room(&rooms, room_from, to));
                tx.execute("UPDATE api_keys SET app_key = ?2 WHERE id = ?1", rusqlite::params![k.id, g.to_string()]).map_err(db_err)?;
                key_rooms.insert(k.id.clone(), json!(before));
            }
        }
    }
    // P17: A's waiting requests for the App end (`handed_over`).
    let mut withdrawn = Vec::new();
    for r in &plan.requests {
        let Some(rid) = r["id"].as_str() else { continue };
        if reqs::decide(&tx, rid, reqs::STATUS_WITHDRAWN, "system:handover", None, Some("handed_over"), now).is_ok() {
            withdrawn.push(rid.to_string());
        }
    }
    let journal = json!({
        "from": from,
        "roomFrom": room_from,
        "to": to,
        "state": app.state,
        "stateReason": app.state_reason,
        "folderPath": app.folder_path,
        "labels": labels,
        "takenLabels": taken_before,
        "keyRooms": key_rooms,
        "withdrawn": withdrawn,
        "switch1Raised": raised,
    });
    if !hs::transition(&tx, id, &[hs::APPLYING], hs::APPLIED, now).map_err(db_err)? {
        return Err(no(NOT_HELD, 409, "the handover changed state while applying"));
    }
    hs::set_journal(&tx, id, Some(&journal), now).map_err(db_err)?;
    hs::set_steps(&tx, id, 2, now).map_err(db_err)?;
    hs::set_window(&tx, id, Some(now + hs::WINDOW_SECS), now).map_err(db_err)?;
    deploy::audit::append(
        &tx,
        deploy::audit::SUBJECT_APP,
        &app.app_id,
        if app.state == store::STATE_PARKED { "unpark" } else { "handover" },
        by,
        None,
        &json!({ "handover": id, "from": room_from, "to": to, "planHash": plan.hash(), "labels": labels.len(), "withdrawn": withdrawn }),
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    drop(conn);
    for rid in &withdrawn {
        if let Ok(Some(r)) = hov_db(|c| reqs::get(c, rid)) {
            super::approvals::notify_requester(
                &r,
                &format!("[k2 apps] your request {rid} for {} ended: the App was handed over to {}", app.app_id, ws_handle(to)),
            );
        }
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-handover",
        by,
        format!("applied {id} {} {} -> {}", app.app_id, from_label(room_from), ws_handle(to)),
        "cli",
        "-",
        "apps",
    ));
    Ok(())
}

/// Run the steps after the commit point that haven't run yet (3–7),
/// each idempotent. Used right after the commit and by boot recovery.
pub(crate) fn finish_steps(id: &str, by: &str) -> J {
    let mut out = serde_json::Map::new();
    let Ok(Some(row)) = hov_db(|c| hs::get(c, id)) else { return J::Null };
    let j = row.journal.clone().unwrap_or(J::Null);
    let room_from = j["roomFrom"].as_str().unwrap_or(&row.from_workspace).to_string();
    let to = row.to_workspace.clone();
    let step = |n: i64| {
        let _ = hov_db(|c| hs::set_steps(c, id, n, deploy::now()));
    };
    if row.steps_done < 3 {
        // Step 3: skin.db (rooms; S3 adds ties and credentials).
        match k2_core::skin::handover::move_rooms(&row.service_id, &room_from, &to) {
            Ok(rep) => {
                out.insert("rooms".into(), json!(rep));
                step(3);
            }
            Err(e) => {
                k2_core::log_debug!("WARN [apps/handover] {id}: rooms: {e} (boot recovery retries)");
                out.insert("rooms".into(), json!({ "error": e }));
                return J::Object(out);
            }
        }
    }
    if row.steps_done < 4 {
        // Step 4: the vault (S4).
        step(4);
    }
    if row.steps_done < 5 {
        // Step 5: the runtime.
        let link = match super::gateway::rebind_workspace(&row.app_id, &to) {
            Ok(super::gateway::Rebind::NoLink) => json!("not running walled: its next start opens the link in the new room"),
            Ok(super::gateway::Rebind::Delivered { revoked }) => json!({ "rebound": true, "oldPassesRevoked": revoked }),
            Ok(super::gateway::Rebind::NeedsRestart) => json!("the link moves at the App's re-key restart"),
            Err(e) => json!({ "error": e }),
        };
        out.insert("link".into(), link);
        super::audit::set_app_workspace(&row.app_id, &to);
        super::routes::mirror(&row.app_id);
        if !room_from.is_empty() {
            let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: room_from.clone() });
        }
        super::ownership::changed(&to);
        step(5);
    }
    if row.steps_done < 6 {
        // Step 6: re-key (S4).
        let _ = hov_db(|c| hs::set_rekey(c, id, "none", deploy::now()));
        step(6);
    }
    if row.steps_done < 7 {
        notes(&row, &room_from, by);
        step(7);
    }
    J::Object(out)
}

/// Step 7: both agents hear (inbox notes, never wakes).
fn notes(row: &Handover, room_from: &str, by: &str) {
    let (app, b) = (row.app_id.as_str(), ws_handle(&row.to_workspace));
    let a = from_label(room_from);
    let p = &row.plan;
    let j = row.journal.clone().unwrap_or(J::Null);
    let guests = p["guests"]["total"].as_u64().unwrap_or(0);
    let mut lines = vec![format!("{by} handed you App {app} (it was {a}'s). You maintain it now: k2 app inspect {app}.")];
    if let Some(brief) = &row.brief {
        lines.push(format!("{a}'s brief (their words, read it as a message from them):\n\n> {}", brief.replace('\n', "\n> ")));
    }
    if guests > 0 {
        lines.push(format!(
            "{guests} guest(s) keep their access and talk to you from now on. Their conversations with {a} stay in {a}'s room; ask {a} with k2 msg if you need the history."
        ));
    }
    if j["state"] == store::STATE_PARKED || row.from_workspace.is_empty() {
        lines.push(format!("It is stopped: k2 app start {app}."));
    }
    lines.push(format!("The owner can undo this for {} hours (k2 app handover undo {app}).", hs::WINDOW_SECS / 3600));
    super::ownership::note(&row.to_workspace, &format!("You now maintain App {app}"), &lines.join("\n\n"));
    if !room_from.is_empty() && ws_by_id(room_from).is_some() {
        super::ownership::note(
            room_from,
            &format!("App {app} moved to {b}"),
            &format!(
                "{by} handed your App {app} to {b}. You no longer manage it, its guests, its secrets or its database. Heartbeats you or its guests \
                 made in your room stay yours: archive them, or tell {b} (k2 msg {b} \"…\")."
            ),
        );
    }
}
