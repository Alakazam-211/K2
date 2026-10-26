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

pub const ROUTES: &[&str] = &[
    "/cli/app/handover/plan",
    "/cli/app/handover/status",
    "/cli/app/handover/brief",
    "/cli/app/handover/accept",
    "/cli/app/handover/decline",
    "/cli/app/handover/cancel",
    "/cli/app/handover/rekey",
    "/cli/app/handover/undo",
    "/cli/app/handover/finish",
];
pub const POST_ROUTES: &[&str] = &["/cli/app/handover/accept", "/cli/app/handover/decline", "/cli/app/handover/cancel", "/cli/app/handover/rekey", "/cli/app/handover/undo", "/cli/app/handover/finish"];

/// The `route` an undo card carries (never an HTTP route).
pub const UNDO_ROUTE: &str = "/cli/app/handover/undo/apply";
pub const NOT_IN_WINDOW: &str = "handover_not_in_window";

/// Reads B may make on an App it accepted (shadowing, §4.9).
pub const SHADOW_READS: &[&str] = &[
    "/cli/app/inspect",
    "/cli/app/logs",
    "/cli/app/history",
    "/cli/app/errors",
    "/cli/app/link",
    "/cli/app/deployment",
    "/cli/app/handover/plan",
];

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
    /// T5.2: stop right after the commit point, as a daemon kill would.
    pub(crate) static CRASH_AFTER_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match mode() {
        Mode::Legacy => return Err(no("not_found", 404, "no such App route (K2_APP_HANDOVER=legacy)")),
        Mode::Off => return Err(off_refusal()),
        Mode::On => {}
    }
    match path {
        "/cli/app/handover/plan" => plan_route(who, params),
        "/cli/app/handover/status" => status_route(who, params),
        "/cli/app/handover/brief" => brief_route(who, params),
        "/cli/app/handover/accept" => accept_route(who, body),
        "/cli/app/handover/decline" => decline_route(who, body),
        "/cli/app/handover/cancel" => cancel_route(who, body),
        "/cli/app/handover/rekey" => rekey_route(who, body),
        "/cli/app/handover/undo" => undo_route(who, body),
        "/cli/app/handover/finish" => finish_route(who, body),
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
    let mut out = plan_answer(&p);
    scrub(who, &mut out);
    Ok(out)
}

/// A passport never sees a folder path (another workspace's, or the App's
/// source): the owner's card does.
pub(crate) fn scrub(who: &Who, v: &mut J) {
    if who.passport().is_none() {
        return;
    }
    fn one(plan: &mut J) {
        if let Some(to) = plan.get_mut("to").and_then(J::as_object_mut) {
            to.remove("folder");
        }
        if let Some(o) = plan.as_object_mut() {
            if o.contains_key("planHash") {
                o.remove("source");
            }
        }
    }
    if let Some(p) = v.get_mut("plan") {
        one(p);
    }
    one(v);
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
/// card; the App's own agent (or a shell tab of its workspace) offers it
/// to B (S2). With `dryRun`, the plan.
pub(crate) fn request_route(who: &Who, body: &[u8], unpark: bool) -> R<J> {
    need_credential(who)?;
    let owner = super::ownership::owner(who);
    if unpark || who.passport().is_none() {
        super::ownership::need_owner(who, if unpark { "Unparking an App" } else { "Handing an App over" })?;
    }
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
        let mut out = plan_answer(&hp::compute(&app, &to, &opts, brief.as_deref())?);
        scrub(who, &mut out);
        return Ok(out);
    }
    if owner {
        return file_card(who, &app, &to, &opts, brief.as_deref(), s(&v, "reason"), &actor(who));
    }
    let mut out = offer(who, &app, &to, &opts, brief.as_deref(), s(&v, "reason"), false).map_err(|mut e| {
        if let Some(x) = e.extra.as_mut() {
            scrub(who, x);
        }
        e
    })?;
    scrub(who, &mut out);
    Ok(out)
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

/// Insert a new handover row. `Err` answers an App that already has an
/// open one: the same waiting card (owner, same B), or `409 handover_open`.
#[allow(clippy::too_many_arguments)]
fn insert_row(app: &AppRow, to: &Ws, state: &str, started_by: &str, p: &Plan, opts: &Options, brief: Option<&str>, reason: Option<&str>) -> Result<Handover, R<J>> {
    let hash = p.hash();
    let plan_json = p.json(true);
    let options = serde_json::to_value(opts).unwrap_or(J::Null);
    let n = hs::NewHandover {
        app_id: &app.app_id,
        service_id: &app.service_id,
        from: &app.owner_workspace,
        to: &to.id,
        state,
        started_by,
        plan: &plan_json,
        plan_hash: &hash,
        options: &options,
        brief,
        reason,
    };
    let h = k2_core::db::shared();
    let conn = h.lock();
    match hs::insert(&conn, &n, deploy::now()) {
        Ok(r) => Ok(r),
        Err(hs::InsertError::Conflict) => {
            let open = match hs::open_for_app(&conn, &app.app_id) {
                Ok(o) => o,
                Err(e) => return Err(Err(db_err(e))),
            };
            drop(conn);
            Err(match open {
                Some(o) if o.state == hs::HELD && o.to_workspace == to.id && state == hs::HELD => existing_card(&o),
                Some(o) => {
                    let mut r = no(
                        OPEN,
                        409,
                        format!("{} already has an open handover ({}, {} to {}): cancel it first (k2 app handover cancel {})", app.app_id, o.id, o.state, ws_handle(&o.to_workspace), app.app_id),
                    );
                    r.extra = Some(json!({ "handover": row_json(&o) }));
                    Err(r)
                }
                None => Err(no(OPEN, 409, "another handover of this App just started: try again")),
            })
        }
        Err(hs::InsertError::Db(e)) => Err(Err(db_err(e))),
    }
}

/// File the card (the held request) for `row` with plan `p`. `who` is the
/// one asking (the owner, or B accepting an offer); the card is A's
/// workspace's. The row becomes `held`.
fn file_request(who: &Who, row: &Handover, app: &AppRow, p: &Plan) -> R<J> {
    let hash = p.hash();
    let plan_json = p.json(true);
    let summary = format!("hand over {} from {} to {}", app.app_id, from_label(&p.room_from), ws_handle(&p.to.id));
    let req = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: Some(app),
        label: &app.app_id,
        workspace: if app.owner_workspace.is_empty() { &p.room_from } else { &app.owner_workspace },
        body: json!({
            "route": APPLY_ROUTE,
            "body": { "handover": row.id, "app": app.app_id, "serviceId": app.service_id, "from": app.owner_workspace, "to": p.to.id, "planHash": hash },
        }),
        diff: json!({ "lines": p.lines(), "route": APPLY_ROUTE, "handover": row.id }),
        summary,
        reasons: vec!["an App handover moves guests, secrets and keys between agents: the owner approves it in person".into()],
        reason: row.reason.as_deref(),
    })?;
    let ok = hov_db(|c| {
        if row.state == hs::HELD {
            hs::set_request_id(c, &row.id, &req.id, deploy::now()).map(|_| true)
        } else {
            hs::set_held(c, &row.id, &req.id, &plan_json, &hash, deploy::now())
        }
    });
    if !matches!(ok, Ok(true)) {
        let _ = hov_db(|c| Ok(reqs::decide(c, &req.id, reqs::STATUS_WITHDRAWN, "system:handover", None, Some("handover_changed"), deploy::now())));
        return Err(no(NOT_HELD, 409, format!("handover {} changed state while its card was filed: ask again", row.id)));
    }
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

/// File the owner's card for `app` → `to`: the plan, the row (`held`),
/// the held request. Idempotent: the same App and B with a card already
/// waiting answers that card.
pub(crate) fn file_card(who: &Who, app: &AppRow, to: &Ws, opts: &Options, brief: Option<&str>, reason: Option<&str>, started_by: &str) -> R<J> {
    // A parked App's budget joins the box's pool again: a §4.3 check point.
    if app.state == store::STATE_PARKED {
        super::admission::check_memory(&app.app_id, app.memory_limit_bytes)?;
    }
    let p = hp::compute(app, to, opts, brief)?;
    if !p.blockers.is_empty() {
        return Err(blocked(&p));
    }
    let row = match insert_row(app, to, hs::HELD, started_by, &p, opts, brief, reason) {
        Ok(r) => r,
        Err(answer) => return answer,
    };
    match file_request(who, &row, app, &p) {
        Ok(out) => Ok(out),
        Err(e) => {
            let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::HELD], hs::ABORTED, deploy::now()));
            Err(e)
        }
    }
}

// ── S2: an agent offers its App; B accepts; then the card ───────────

/// A's offer of `app` to `to` (A's passport, switch 1). `auto_accept`: B
/// was just hired for it (`hire --take-app`, Q1), so the card is filed at
/// once.
pub(crate) fn offer(who: &Who, app: &AppRow, to: &Ws, opts: &Options, brief: Option<&str>, reason: Option<&str>, auto_accept: bool) -> R<J> {
    super::access::need_build(who)?;
    let p = hp::compute(app, to, opts, brief)?;
    if !p.blockers.is_empty() {
        return Err(blocked(&p));
    }
    let started_by = if auto_accept { format!("hire:{}", ws_handle(&app.owner_workspace)) } else { actor(who) };
    let row = match insert_row(app, to, hs::OFFERED, &started_by, &p, opts, brief, reason) {
        Ok(r) => r,
        Err(answer) => return answer,
    };
    let (a, b) = (ws_handle(&app.owner_workspace), ws_handle(&to.id));
    if auto_accept {
        hov_db(|c| hs::set_accepted(c, &row.id, &format!("hire:{b}"), deploy::now()))?;
        let row = hov_db(|c| hs::get(c, &row.id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
        let out = file_request(who, &row, app, &p);
        if out.is_err() {
            let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::ACCEPTED], hs::ABORTED, deploy::now()));
        }
        super::ownership::note(&app.owner_workspace, &format!("You offered App {} to {b}", app.app_id), &format!(
            "{b} was hired to take {} over, so the offer is accepted; your owner approves the handover card (k2 app approvals).",
            app.app_id
        ));
        return out;
    }
    super::ownership::note(
        &app.owner_workspace,
        &format!("You offered App {} to {b}", app.app_id),
        &format!(
            "{b} must accept (k2 app handover accept {app_id}), then your owner approves one card that lists what moves. You maintain {app_id} until then. \
             Cancel: k2 app handover cancel {app_id}.{}",
            if brief.is_none() { " Give them a brief: k2 app handover <app> --to <agent> --brief FILE (your notes for the next agent)." } else { "" },
            app_id = app.app_id
        ),
    );
    let mut lines = vec![format!(
        "{a} offers you its App {app_id}. Accept: k2 app handover accept {app_id}. Decline: k2 app handover decline {app_id} --reason \"…\". \
         What would move: k2 app handover status {app_id}.",
        app_id = app.app_id
    )];
    if let Some(bf) = brief {
        lines.push(format!("{a}'s brief (their words):\n\n> {}", bf.replace('\n', "\n> ")));
    }
    lines.push(format!("Once you accept, you can look at it (k2 app inspect {0}, k2 app logs {0}) until the owner answers the card.", app.app_id));
    super::ownership::note(&to.id, &format!("{a} offers you App {}", app.app_id), &lines.join("\n\n"));
    Ok(json!({
        "ok": true,
        "status": "held",
        "handover": row_json(&row),
        "plan": p.json(true),
        "app": app.app_id,
        "hint": format!(
            "offered to {b}: {b} accepts (k2 app handover accept {0}), then the owner approves the card. Nothing moves until then.{1}",
            app.app_id,
            if brief.is_none() { " Add a brief for the next agent with --brief FILE." } else { "" }
        ),
    }))
}

/// The open handover a call names (`handover` id, or `app`) that `who`
/// is on: A (the giver), B (the taker), or a person.
fn named_open(who: &Who, v: &J) -> R<Handover> {
    let row = if let Some(id) = s(v, "handover") {
        hov_db(|c| hs::get(c, id))?
    } else {
        let app = s(v, "app").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the App (or the handover id)"))?.to_ascii_lowercase();
        let id = app.rsplit('/').next().unwrap_or(&app).to_string();
        hov_db(|c| hs::open_for_app(c, &id))?
    };
    let row = row.ok_or_else(|| no(codes::APP_NOT_FOUND, 404, "no open handover by that name (k2 app handover status)"))?;
    if let Some((ws, _)) = who.passport() {
        if ws != row.from_workspace && ws != row.to_workspace {
            return Err(no(codes::APP_NOT_FOUND, 404, "no open handover by that name (k2 app handover status)"));
        }
    }
    Ok(row)
}

fn is_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

/// POST `/cli/app/handover/accept {app|handover}`: B (or the owner) agrees;
/// the owner's card is filed.
fn accept_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let row = named_open(who, &v)?;
    let mine = who.passport().is_some_and(|(ws, _)| ws == row.to_workspace);
    if !mine && !super::ownership::owner(who) {
        return Err(no(codes::OWNER_ONLY, 403, "only the agent the App is offered to, or the owner, accepts"));
    }
    if row.state != hs::OFFERED && row.state != hs::ACCEPTED {
        return Err(no(NOT_HELD, 409, format!("handover {} is {}: nothing to accept", row.id, row.state)));
    }
    let app = hov_db(|c| store::get_app(c, &row.app_id))?.ok_or_else(|| no(codes::APP_NOT_FOUND, 409, "the App is gone"))?;
    let to = ws_by_id(&row.to_workspace).ok_or_else(|| no(codes::APP_NOT_FOUND, 409, "the workspace that takes it is gone"))?;
    if app.owner_workspace != row.from_workspace {
        let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::OFFERED, hs::ACCEPTED], hs::STALE, deploy::now()));
        return Err(no(STALE, 409, "the App changed hands since the offer"));
    }
    if row.state == hs::OFFERED {
        hov_db(|c| hs::set_accepted(c, &row.id, &actor(who), deploy::now()))?;
    }
    let row = hov_db(|c| hs::get(c, &row.id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
    let opts: Options = serde_json::from_value(row.options.clone()).unwrap_or_default();
    let p = hp::compute(&app, &to, &opts, row.brief.as_deref())?;
    if !p.blockers.is_empty() {
        // Accepted, but the card waits until what blocks it is gone (B can
        // accept again to file it).
        return Err(blocked(&p));
    }
    let mut out = file_request(who, &row, &app, &p)?;
    scrub(who, &mut out);
    super::ownership::note(
        &row.from_workspace,
        &format!("{} accepted App {}", ws_handle(&row.to_workspace), row.app_id),
        &format!("{} accepted your offer of {}; the owner's card is filed. You maintain it until the owner approves.", ws_handle(&row.to_workspace), row.app_id),
    );
    Ok(out)
}

/// POST `/cli/app/handover/decline {app|handover, reason?}`: B (or the
/// owner) says no.
fn decline_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let row = named_open(who, &v)?;
    let mine = who.passport().is_some_and(|(ws, _)| ws == row.to_workspace);
    if !mine && !super::ownership::owner(who) {
        return Err(no(codes::OWNER_ONLY, 403, "only the agent the App is offered to, or the owner, declines"));
    }
    end(&row, hs::DECLINED, who, s(&v, "reason"))
}

/// POST `/cli/app/handover/cancel {app|handover}`: A, the owner or an
/// Admin login, before it applies.
fn cancel_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let row = named_open(who, &v)?;
    let giver = who.passport().is_some_and(|(ws, _)| ws == row.from_workspace);
    if !giver && !is_person(who) {
        return Err(no(codes::OWNER_ONLY, 403, "only the giving agent, the owner or an admin cancels a handover"));
    }
    end(&row, hs::CANCELLED, who, s(&v, "reason"))
}

/// Close an open handover (and its card) without applying.
fn end(row: &Handover, to: &str, who: &Who, reason: Option<&str>) -> R<J> {
    if row.state == hs::APPLYING {
        return Err(no(IN_PROGRESS, 409, "the handover is applying right now"));
    }
    if !hov_db(|c| hs::transition(c, &row.id, &[hs::OFFERED, hs::ACCEPTED, hs::HELD], to, deploy::now()))? {
        return Err(no(NOT_HELD, 409, format!("handover {} is no longer open", row.id)));
    }
    if let Some(rid) = row.request_id.as_deref() {
        let _ = hov_db(|c| Ok(reqs::decide(c, rid, reqs::STATUS_WITHDRAWN, &actor(who), None, Some(to), deploy::now())));
    }
    let why = reason.map(|r| format!(" ({})", r.chars().take(300).collect::<String>())).unwrap_or_default();
    let by = actor(who);
    if !row.from_workspace.is_empty() {
        super::ownership::note(&row.from_workspace, &format!("Handover of {} {to}", row.app_id), &format!("{by} {to} the handover of {} to {}{why}. You still maintain it.", row.app_id, ws_handle(&row.to_workspace)));
    }
    super::ownership::note(&row.to_workspace, &format!("Handover of {} {to}", row.app_id), &format!("{by} {to} the handover of {} to you{why}.", row.app_id));
    let row = hov_db(|c| hs::get(c, &row.id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
    Ok(json!({ "ok": true, "handover": row_json(&row) }))
}

/// GET `/cli/app/handover/status?app=`: open and recent handovers (a
/// passport sees those it gives or takes; a person sees all).
fn status_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    may_read(who)?;
    let ws = who.passport().map(|(w, _)| w.to_string());
    let app = param(params, "app").map(|a| a.to_ascii_lowercase());
    let rows = hov_db(|c| hs::list(c, ws.as_deref(), app.as_deref(), 50))?;
    let now = deploy::now();
    Ok(json!({
        "ok": true,
        "handovers": rows.iter().map(|r| {
            let mut v = row_json(r);
            v["inWindow"] = json!(r.in_window(now));
            v
        }).collect::<Vec<_>>(),
    }))
}

/// GET `/cli/app/handover/brief?app=` (or `handover=`): A's notes for B.
/// A and B while the handover is open or in its window; a person always.
fn brief_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    may_read(who)?;
    let row = match (param(params, "handover"), param(params, "app")) {
        (Some(id), _) => hov_db(|c| hs::get(c, id))?,
        (None, Some(app)) => hov_db(|c| hs::latest_for_app(c, &app.to_ascii_lowercase()))?,
        _ => return Err(no(codes::BAD_REQUEST, 400, "name the App (or the handover id)")),
    }
    .ok_or_else(|| no(codes::APP_NOT_FOUND, 404, "no handover by that name"))?;
    if let Some((ws, _)) = who.passport() {
        let party = ws == row.from_workspace || ws == row.to_workspace;
        if !party || !(row.is_open() || row.in_window(deploy::now())) {
            return Err(no(codes::APP_NOT_FOUND, 404, "no handover by that name"));
        }
    }
    Ok(json!({ "ok": true, "handover": row.id, "app": row.app_id, "from": ws_handle(&row.from_workspace), "to": ws_handle(&row.to_workspace), "brief": row.brief }))
}

/// B shadows the App it accepted (§4.9): reads only, until the card is
/// answered. `true` when `ws` has an accepted (or held) handover of `app`.
pub(crate) fn shadows(ws: &str, app_id: &str) -> bool {
    hov_db(|c| hs::open_for_app(c, app_id)).ok().flatten().is_some_and(|h| h.to_workspace == ws && (h.state == hs::ACCEPTED || h.state == hs::HELD))
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
    if app.state == store::STATE_PARKED {
        super::admission::check_memory(&app.app_id, app.memory_limit_bytes)?;
    }
    if !hov_db(|c| hs::set_approved(c, &row.id, by, deploy::now()))? {
        return Err(no(NOT_HELD, 409, format!("handover {} was answered already", row.id)));
    }
    run(&row.id, &plan, by)
}

/// The commit point and the steps after it.
fn run(id: &str, plan: &Plan, by: &str) -> R<J> {
    let _busy = Busy::enter(&plan.app.app_id, &plan.room_from, &plan.to.id);
    // §4.4: between the commit and the skin transaction a tie to A or B
    // counts as inside this App (no tied guest is refused in the gap).
    let _gap = k2_core::skin::handover::begin_handing_over(&plan.app.service_id, &plan.room_from, &plan.to.id);
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
    #[cfg(test)]
    if CRASH_AFTER_COMMIT.with(|c| c.get()) {
        return Err(no("test_crash", 500, "test hook: the daemon died right after the commit point"));
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
    // P5 (§4.6): the App's sole database moves to B; a shared one stays
    // A's and B gets a grant when the owner chose "share".
    let mut db_journal = J::Null;
    if let Some(name) = plan.db.name.as_deref() {
        match plan.db.case {
            "sole" => {
                let n = tx
                    .execute(
                        "UPDATE sql_databases SET project_id = ?3 WHERE name = ?1 AND project_id = ?2 AND status = 'active'",
                        rusqlite::params![name, room_from, to],
                    )
                    .map_err(db_err)?;
                db_journal = json!({ "case": "sole", "name": name, "moved": n });
            }
            "shared" if plan.options.db.as_deref() == Some("share") => {
                let dbid: Option<String> = tx
                    .query_row("SELECT id FROM sql_databases WHERE name = ?1 AND status = 'active' ORDER BY created_at DESC LIMIT 1", rusqlite::params![name], |r| r.get(0))
                    .map_err(db_err)?;
                if let Some(dbid) = dbid {
                    let n = tx
                        .execute(
                            "INSERT OR IGNORE INTO sql_grants (database_id, project_id, level, can_manage, created_at, updated_at) VALUES (?1, ?2, 'write', 1, ?3, ?3)",
                            rusqlite::params![dbid, to, now],
                        )
                        .map_err(db_err)?;
                    db_journal = json!({ "case": "shared", "name": name, "databaseId": dbid, "grantAdded": n == 1 });
                }
            }
            _ => {}
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
        "db": db_journal,
        "liveDeployment": app.live_deployment,
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
        // Step 3: skin.db in one transaction: rooms, ties and credentials,
        // A's agent logins, the journal. A's other Apps are read from the
        // daemon DB first (lock order: never skin → daemon DB).
        let others: Vec<String> = if room_from.is_empty() {
            Vec::new()
        } else {
            hov_db(|c| store::list_apps(c, Some(&room_from))).unwrap_or_default().into_iter().filter(|a| a.app_id != row.app_id).map(|a| a.service_id).collect()
        };
        let reset_shared = row.options.get("resetShared").and_then(J::as_bool).unwrap_or(false);
        match k2_core::skin::handover::apply_guests(&row.service_id, &room_from, &to, &others, &row.id, reset_shared) {
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
        // Step 4: the vault (§4.5): A's secrets the App uses move to B (or
        // are copied when A uses them too); the Seed's references resolve
        // through a per-service alias.
        match vault_step(&row, &room_from) {
            Ok(v) => {
                journal_put(id, "vault", v.clone());
                out.insert("secrets".into(), v);
                step(4);
            }
            Err(e) => {
                k2_core::log_debug!("WARN [apps/handover] {id}: vault: {e} (boot recovery retries)");
                out.insert("secrets".into(), json!({ "error": e }));
                return J::Object(out);
            }
        }
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
        refreshed(&room_from, &to);
        if row.options.get("copySource").and_then(J::as_bool) == Some(true) {
            out.insert("source".into(), copy_source(&row));
        }
        step(5);
    }
    if row.steps_done < 6 {
        // Step 6: re-key (§4.10): new App keys, new database passwords, and
        // the App on a fresh instance to pick them up.
        out.insert("rekey".into(), rekey(id, by));
        step(6);
    }
    if row.steps_done < 7 {
        notes(&row, &room_from, by);
        context_layers(&row.to_workspace, &room_from);
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

// ── S4: the vault, the database, the re-key ──────────────────────────

/// Merge `key` into the handover's journal.
fn journal_put(id: &str, key: &str, v: J) {
    let _ = hov_db(|c| {
        let mut j = hs::get(c, id)?.and_then(|r| r.journal).unwrap_or_else(|| json!({}));
        j[key] = v;
        hs::set_journal(c, id, Some(&j), deploy::now())
    });
}

/// Step 4 (§4.5). For each A-scope secret the plan listed:
/// - `move`: re-scoped to B (same id, versions and grants); renamed
///   `NAME__<app>` when B has that name (Q8, the default), or the App
///   uses B's own value (`collisions: use_b`);
/// - `copy` (A uses it too): a copy in B; the App's grant moves to it;
/// and an alias so the Seed's `k2v://ws/<A>/NAME` resolves in B for this
/// service only. Idempotent (a secret already in B, a copy already made).
fn vault_step(row: &Handover, room_from: &str) -> Result<J, String> {
    use k2_vault::{Actor, Consumer, GrantRequest, Scope, SecretStore, Selector};
    let items: Vec<J> = row.plan["secrets"].as_array().cloned().unwrap_or_default().into_iter().filter(|s| s["case"] != "untouched").collect();
    if items.is_empty() {
        return Ok(json!([]));
    }
    let vault = crate::vault::get().map_err(|e| format!("the vault isn't open ({})", e.code))?;
    let use_b = row.options.get("collisions").and_then(J::as_str) == Some("use_b");
    let to = row.to_workspace.clone();
    let me = Consumer::published_service(row.service_id.clone());
    let mut done = Vec::new();
    for s in items {
        let (id, name) = (s["secretId"].as_str().unwrap_or_default().to_string(), s["name"].as_str().unwrap_or_default().to_string());
        let collision = s["collision"].as_bool().unwrap_or(false);
        let case = s["case"].as_str().unwrap_or_default();
        let (new_id, to_name, how) = if collision && use_b {
            let m = vault.meta(&Selector::user(Scope::workspace(&to), &name), &Actor::Owner).map_err(|e| format!("{name}: {}", e.hint))?;
            (m.id, name.clone(), "use_b")
        } else {
            let target = if collision { format!("{name}__{}", row.app_id) } else { name.clone() };
            match case {
                "move" => {
                    let m = vault.meta(&Selector::Id(id.clone()), &Actor::Owner).map_err(|e| format!("{name}: {}", e.hint))?;
                    if m.scope == Scope::workspace(&to) {
                        (m.id, m.name, "moved")
                    } else {
                        let m = vault
                            .rescope(&Selector::Id(id.clone()), Scope::workspace(&to), collision.then_some(target.as_str()), &Actor::Owner)
                            .map_err(|e| format!("{name}: {}", e.hint))?;
                        (m.id, m.name, "moved")
                    }
                }
                _ => match vault.meta(&Selector::user(Scope::workspace(&to), &target), &Actor::Owner) {
                    Ok(m) if m.origin == "k2 app handover" => (m.id, target, "copied"),
                    _ => {
                        let m = vault.copy_to(&Selector::Id(id.clone()), Scope::workspace(&to), &target, &Actor::Owner).map_err(|e| format!("{name}: {}", e.hint))?;
                        (m.id, target, "copied")
                    }
                },
            }
        };
        // The App's grant follows a copy or B's own secret.
        if new_id != id {
            for (g, _) in vault.grants_to(&me).map_err(|e| e.hint)? {
                if g.secret_id != id {
                    continue;
                }
                let req = GrantRequest {
                    selector: Selector::Id(new_id.clone()),
                    consumer: me.clone(),
                    delivery: g.delivery,
                    env_name: g.env_name.clone(),
                    file_name: g.file_name.clone(),
                    hosts_narrowed: g.hosts_narrowed.clone(),
                    expires_at: g.expires_at.clone(),
                    version: None,
                };
                vault.grant(req, &Actor::Owner).map_err(|e| format!("{name}: {}", e.hint))?;
                vault.revoke(&g.id, &Actor::Owner).map_err(|e| format!("{name}: {}", e.hint))?;
            }
        }
        // The Seed's reference (harmless when no reference names it).
        hov_db(|c| {
            hs::put_alias(
                c,
                &hs::RefAlias {
                    service_id: row.service_id.clone(),
                    from_workspace: room_from.to_string(),
                    to_workspace: to.clone(),
                    name: name.clone(),
                    to_name: to_name.clone(),
                    handover_id: row.id.clone(),
                },
                deploy::now(),
            )
        })
        .map_err(|e| e.hint)?;
        done.push(json!({ "secretId": id, "name": name, "newId": new_id, "toName": to_name, "how": how, "key": s["key"] }));
    }
    Ok(J::Array(done))
}

/// Step 6 (§4.10): rotate what A knew and put the App on a fresh instance.
/// Synchronous parts first (keys minted, database passwords changed, the
/// DSN secret updated); the old keys are revoked once the new instance is
/// live and the old one drained (a watcher), or at once when the App
/// isn't running.
fn rekey(id: &str, by: &str) -> J {
    rekey_as(id, by, false)
}

/// The re-key mode now: the plan's, adjusted to whether the App runs.
fn mode_now(plan_mode: &str, app: Option<&AppRow>) -> String {
    let running = app.is_some_and(|a| a.state == store::STATE_RUNNING && a.live_seed.is_some());
    match plan_mode {
        "none" => "none".into(),
        _ if !running => "next_start".into(),
        "next_start" => "bluegreen".into(),
        m => m.into(),
    }
}

/// [`rekey`]; `undo`: after an undo the DSN secret is back in A under its
/// own name, and the passwords change again (A gets new ones, never the
/// old ones).
fn rekey_as(id: &str, by: &str, undo: bool) -> J {
    let Ok(Some(row)) = hov_db(|c| hs::get(c, id)) else { return J::Null };
    let app_now = hov_db(|c| store::get_app(c, &row.app_id)).ok().flatten();
    let mode = mode_now(row.plan["rekey"]["mode"].as_str().unwrap_or("none"), app_now.as_ref());
    if mode == "none" {
        let _ = hov_db(|c| hs::set_rekey(c, id, "none", deploy::now()));
        return json!({ "mode": "none" });
    }
    let _ = hov_db(|c| hs::set_rekey(c, id, "running", deploy::now()));
    let mut out = json!({ "mode": mode });
    let journal = row.journal.clone().unwrap_or(J::Null);
    // The database: new passwords, the App's DSN secret gets the new one.
    if journal["db"]["case"] == "sole" {
        let name = journal["db"]["name"].as_str().unwrap_or_default().to_string();
        match crate::sql::routes::rotate_for_handover(&name) {
            Ok((lines, dsn)) => {
                out["database"] = json!({ "rotated": lines });
                let (ws, field) = if undo {
                    (journal["roomFrom"].as_str().unwrap_or_default().to_string(), "name")
                } else {
                    (row.to_workspace.clone(), "toName")
                };
                if let Err(e) = put_dsn(&journal, &ws, field, &dsn) {
                    out["database"]["dsnError"] = json!(e);
                }
            }
            Err(e) => {
                out["database"] = json!({ "error": e });
                return rekey_failed(&row, out, &format!("the database passwords didn't change ({e})"));
            }
        }
    }
    // App keys: successors now, the old ones after the drain.
    let pairs = match super::grace::rekey_app_keys(&row.service_id) {
        Ok(p) => p,
        Err(e) => {
            out["keys"] = json!({ "error": e.hint });
            return rekey_failed(&row, out, &format!("the App's keys didn't change ({})", e.hint));
        }
    };
    let old_keys: Vec<String> = pairs.iter().map(|(o, _)| o.clone()).collect();
    out["keys"] = json!(pairs.iter().map(|(o, n)| json!({ "old": o, "new": n })).collect::<Vec<_>>());
    journal_put(id, "oldKeys", json!(old_keys));
    let Ok(Some(app)) = hov_db(|c| store::get_app(c, &row.app_id)) else {
        return rekey_failed(&row, out, "the App is gone");
    };
    match mode.as_str() {
        "defer" => {
            // Its keys change at B's first deploy (the window sweep sees
            // the new live deployment and revokes the old keys).
            let _ = hov_db(|c| hs::set_rekey(c, id, "deferred", deploy::now()));
            out["state"] = json!("deferred");
        }
        "next_start" => {
            revoke_keys(&old_keys);
            let _ = hov_db(|c| hs::set_rekey(c, id, "done", deploy::now()));
            out["state"] = json!("done");
        }
        _ => match super::supervisor::begin_handover_rekey(&app, by, id, mode == "pause") {
            Ok(dep) => {
                out["deployment"] = json!(dep);
                out["state"] = json!("running");
                watch_rekey(id.to_string(), row.app_id.clone(), dep, old_keys);
            }
            Err(f) => {
                out["error"] = json!({ "code": f.code, "hint": f.message });
                return rekey_failed(&row, out, &f.message);
            }
        },
    }
    out
}

fn rekey_failed(row: &Handover, mut out: J, why: &str) -> J {
    let _ = hov_db(|c| hs::set_rekey(c, &row.id, "failed", deploy::now()));
    out["state"] = json!("failed");
    super::ownership::note(
        &row.to_workspace,
        &format!("App {}: its keys didn't change yet", row.app_id),
        &format!("The handover moved {} to you, but {why}. It keeps running on its old instance. Try again: k2 app handover rekey {}.", row.app_id, row.app_id),
    );
    out
}

fn revoke_keys(ids: &[String]) {
    for k in ids {
        if let Err(e) = k2_core::api_keys::revoke_api_key(k) {
            k2_core::log_debug!("[apps/handover] revoke old App key {k}: {e}");
        }
    }
}

/// The App's DSN secret (moved to B by step 4) gets the new password.
fn put_dsn(journal: &J, to: &str, field: &str, dsn: &str) -> Result<(), String> {
    use k2_vault::{Actor, Scope, SecretBytes, SecretStore, Selector};
    let vault = crate::vault::get().map_err(|e| e.hint)?;
    let names: Vec<String> = journal["vault"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|v| v["key"] == crate::vault::publish_env::DATABASE_URL || v["name"] == crate::vault::publish_env::DATABASE_URL)
                .filter_map(|v| v[field].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for n in names {
        vault.rotate(&Selector::user(Scope::workspace(to), &n), SecretBytes::from_slice(dsn.as_bytes()), &Actor::System("db".into())).map_err(|e| e.hint)?;
    }
    Ok(())
}

/// Watch the re-key deployment: live → the old instance drains, then the
/// old keys are revoked (`done`); failed or rolled back → the App keeps
/// its old instance and keys (`failed`, B is told).
fn watch_rekey(id: String, app_id: String, dep: String, old_keys: Vec<String>) {
    let spawn = std::thread::Builder::new().name("app-handover-rekey".into()).spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30 * 60);
        loop {
            let d = hov_db(|c| store::get_deployment(c, &dep)).ok().flatten();
            match d.as_ref().map(|d| d.result.as_str()) {
                Some(store::RESULT_LIVE) => {
                    let drain = std::env::var("K2_APPS_DRAIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(30u64);
                    std::thread::sleep(std::time::Duration::from_secs(drain + 2));
                    revoke_keys(&old_keys);
                    let _ = hov_db(|c| hs::set_rekey(c, &id, "done", deploy::now()));
                    return;
                }
                Some(store::RESULT_FAILED | store::RESULT_ROLLED_BACK | store::RESULT_STALLED | store::RESULT_EXITED | store::RESULT_STOPPED) | None => {
                    if let Ok(Some(row)) = hov_db(|c| hs::get(c, &id)) {
                        rekey_failed(&row, json!({}), &format!("its re-key restart didn't go live ({})", d.map(|d| d.result).unwrap_or_else(|| "gone".into())));
                    }
                    return;
                }
                _ => {}
            }
            if std::time::Instant::now() > deadline {
                if let Ok(Some(row)) = hov_db(|c| hs::get(c, &id)) {
                    rekey_failed(&row, json!({}), "its re-key restart didn't go live within 30 minutes");
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
    if let Err(e) = spawn {
        k2_core::log_debug!("[apps/handover] {app_id}: re-key watcher: {e}");
    }
}

/// POST `/cli/app/handover/rekey {app}`: try a failed (or deferred) re-key
/// again: B's passport (switch 1) or the owner.
fn rekey_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    if who.passport().is_some() {
        super::access::need_build(who)?;
    } else if !super::ownership::owner(who) {
        return Err(no(codes::OWNER_ONLY, 403, "the App's agent or the owner re-keys it"));
    }
    let row = hov_db(|c| hs::latest_for_app(c, &app.app_id))?
        .filter(|r| r.state == hs::APPLIED && r.to_workspace == app.owner_workspace)
        .ok_or_else(|| no(NOT_HELD, 409, format!("{} has no handover in its window to re-key", app.app_id)))?;
    if !matches!(row.rekey_state.as_deref(), Some("failed" | "deferred")) {
        return Err(no(NOT_HELD, 409, format!("the re-key of {} is {}", app.app_id, row.rekey_state.as_deref().unwrap_or("not started"))));
    }
    let by = actor(who);
    let out = rekey(&row.id, &by);
    Ok(json!({ "ok": out["state"] != "failed", "app": app.app_id, "rekey": out }))
}

/// Both workspaces' clients re-read their services and settings (the App
/// left `gone`, joined `now`). Unit tests skip the process-wide bus: dozens
/// of handovers a second there would lag other tests' subscribers.
fn refreshed(gone: &str, now: &str) {
    if cfg!(test) {
        return;
    }
    if !gone.is_empty() {
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: gone.to_string() });
    }
    super::ownership::changed(now);
}

// ── S5: the undo window, finish, recovery ────────────────────────────

/// The applied handover of `app_id` still inside its window.
fn windowed(app_id: &str) -> R<Handover> {
    let row = hov_db(|c| hs::latest_for_app(c, app_id))?;
    match row {
        Some(r) if r.in_window(deploy::now()) => Ok(r),
        Some(r) if r.state == hs::APPLIED => Err(no(NOT_IN_WINDOW, 409, format!("the handover of {app_id} is past its undo window (it ended {})", r.window_until.unwrap_or(0)))),
        _ => Err(no(NOT_IN_WINDOW, 409, format!("{app_id} has no handover in its undo window"))),
    }
}

/// POST `/cli/app/handover/undo {app}`: the owner files the undo card (a
/// fresh `app-approve` confirm approves it, as the handover's own card).
fn undo_route(who: &Who, body: &[u8]) -> R<J> {
    super::ownership::need_owner(who, "Undoing a handover")?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let row = windowed(&app.app_id)?;
    if app.owner_workspace != row.to_workspace {
        return Err(no(STALE, 409, format!("{} moved again since the handover: undo isn't possible", app.app_id)));
    }
    let a = from_label(row.journal.as_ref().and_then(|j| j["roomFrom"].as_str()).unwrap_or(&row.from_workspace));
    let b = ws_handle(&row.to_workspace);
    let lines = vec![
        format!("Undo the handover of {} : it goes back to {a}.", app.app_id),
        format!("{a} maintains it again; guests talk to {a}; labels, secrets and the database go back."),
        "Passwords guests set since stay theirs. Its keys and database passwords change again (nobody gets the old ones back).".to_string(),
        format!("What {b} did meanwhile (deploys, new guests) stays with the App."),
    ];
    let req = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: Some(&app),
        label: &app.app_id,
        workspace: &app.owner_workspace,
        body: json!({ "route": UNDO_ROUTE, "body": { "handover": row.id, "app": app.app_id } }),
        diff: json!({ "lines": lines, "route": UNDO_ROUTE, "handover": row.id }),
        summary: format!("undo the handover of {} ({b} → {a})", app.app_id),
        reasons: vec!["undoing a handover moves guests, secrets and keys back: the owner approves it in person".into()],
        reason: s(&v, "reason"),
    })?;
    let mut out = super::approvals::held_answer(&req);
    out["handover"] = row_json(&row);
    Ok(out)
}

/// The approved undo card: the reverse handover, from the journal.
pub(crate) fn apply_undo(r: &reqs::AppRequest, by: &str) -> R<J> {
    let id = r.body.pointer("/body/handover").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable undo card"))?;
    let row = hov_db(|c| hs::get(c, id))?.ok_or_else(|| no(NOT_HELD, 409, format!("no handover '{id}'")))?;
    if !row.in_window(deploy::now()) {
        return Err(no(NOT_IN_WINDOW, 409, "the handover is past its undo window"));
    }
    let app = hov_db(|c| store::get_app(c, &row.app_id))?.ok_or_else(|| no(codes::APP_NOT_FOUND, 409, "the App is gone"))?;
    if app.owner_workspace != row.to_workspace {
        return Err(no(STALE, 409, format!("{} moved again since the handover", app.app_id)));
    }
    let j = row.journal.clone().unwrap_or(J::Null);
    let from = j["from"].as_str().unwrap_or(&row.from_workspace).to_string();
    let room_from = j["roomFrom"].as_str().unwrap_or(&from).to_string();
    let to = row.to_workspace.clone();
    let _busy = Busy::enter(&app.app_id, &to, &room_from);
    let _gap = k2_core::skin::handover::begin_handing_over(&app.service_id, &to, &room_from);
    // The daemon DB, one transaction.
    {
        let now = deploy::now();
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut cfg = k2_core::app_grace::config::load(&conn, &app.app_id).map_err(db_err)?;
        let before: Vec<k2_core::app_grace::config::TakenLabel> = serde_json::from_value(j["takenLabels"].clone()).unwrap_or_default();
        for t in cfg.labels.iter_mut() {
            if let Some(b) = before.iter().find(|b| b.label == t.label) {
                t.service_id = b.service_id.clone();
            }
        }
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let parked = from.is_empty();
        let (state, reason) = if parked {
            (store::STATE_PARKED.to_string(), Some("the handover that unparked it was undone".to_string()))
        } else {
            (app.state.clone(), app.state_reason.clone())
        };
        tx.execute(
            "UPDATE apps SET owner_workspace = ?2, state = ?3, state_reason = ?4, folder_path = ?5, config_json = ?6, updated_at = ?7 WHERE app_id = ?1",
            rusqlite::params![app.app_id, from, state, reason, j["folderPath"].as_str(), serde_json::to_string(&cfg).unwrap_or_default(), now],
        )
        .map_err(db_err)?;
        tx.execute("UPDATE published_services SET project_id = ?2 WHERE id = ?1", rusqlite::params![app.service_id, from]).map_err(db_err)?;
        for l in j["labels"].as_array().cloned().unwrap_or_default() {
            let (Some(label), before) = (l["label"].as_str(), l["before"].as_str()) else { continue };
            match before {
                Some(b) => {
                    tx.execute("UPDATE subdomain_workspaces SET project_id = ?3 WHERE label = ?1 AND project_id = ?2", rusqlite::params![label, to, b])
                        .map_err(db_err)?;
                }
                None => {
                    tx.execute("DELETE FROM subdomain_workspaces WHERE label = ?1 AND project_id = ?2", rusqlite::params![label, to]).map_err(db_err)?;
                }
            }
        }
        // App keys (the successors too): their room ceilings name A again.
        let keys: Vec<(String, Option<String>)> = {
            let mut st = tx.prepare("SELECT id, app_key FROM api_keys WHERE app_service_id = ?1 AND revoked_at IS NULL").map_err(db_err)?;
            let rows = st.query_map(rusqlite::params![app.service_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db_err)?.flatten().collect();
            rows
        };
        for (kid, raw) in keys {
            let Some(mut g) = raw.as_deref().and_then(|s| serde_json::from_str::<J>(s).ok()) else { continue };
            if let Some(rooms) = g.get("rooms").and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect::<Vec<_>>()) {
                if rooms.iter().any(|r| r == &to) {
                    g["rooms"] = json!(k2_core::skin::handover::swap_room(&rooms, &to, &room_from));
                    tx.execute("UPDATE api_keys SET app_key = ?2 WHERE id = ?1", rusqlite::params![kid, g.to_string()]).map_err(db_err)?;
                }
            }
        }
        match j["db"]["case"].as_str() {
            Some("sole") => {
                tx.execute(
                    "UPDATE sql_databases SET project_id = ?3 WHERE name = ?1 AND project_id = ?2 AND status = 'active'",
                    rusqlite::params![j["db"]["name"].as_str(), to, room_from],
                )
                .map_err(db_err)?;
            }
            Some("shared") if j["db"]["grantAdded"] == true => {
                tx.execute("DELETE FROM sql_grants WHERE database_id = ?1 AND project_id = ?2", rusqlite::params![j["db"]["databaseId"].as_str(), to]).map_err(db_err)?;
            }
            _ => {}
        }
        if !hs::transition(&tx, &row.id, &[hs::APPLIED], hs::UNDONE, now).map_err(db_err)? {
            return Err(no(NOT_IN_WINDOW, 409, "the handover changed state"));
        }
        deploy::audit::append(&tx, deploy::audit::SUBJECT_APP, &app.app_id, "handover_undo", by, None, &json!({ "handover": row.id, "from": to, "to": room_from }))
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
    }
    let mut out = serde_json::Map::new();
    // skin.db: rooms back, ties back, retired passwords restored where the
    // guest hasn't set a new one since (compare-and-set).
    match k2_core::skin::handover::undo_guests(&app.service_id, &room_from, &to, &row.id) {
        Ok(rep) => {
            out.insert("guests".into(), json!(rep));
        }
        Err(e) => {
            out.insert("guests".into(), json!({ "error": e }));
        }
    }
    // The vault: secrets back where they were; copies and aliases go.
    out.insert("secrets".into(), undo_vault(&row, &j, &room_from));
    // The runtime.
    let link = match super::gateway::rebind_workspace(&app.app_id, &room_from) {
        Ok(super::gateway::Rebind::Delivered { revoked }) => json!({ "rebound": true, "oldPassesRevoked": revoked }),
        Ok(other) => json!(format!("{other:?}")),
        Err(e) => json!({ "error": e }),
    };
    out.insert("link".into(), link);
    super::audit::set_app_workspace(&app.app_id, &room_from);
    super::routes::mirror(&app.app_id);
    refreshed(&to, &room_from);
    // New keys and database passwords again (B knew them now).
    out.insert("rekey".into(), rekey_as(&row.id, by, true));
    if !room_from.is_empty() {
        super::ownership::note(&room_from, &format!("App {} is yours again", app.app_id), &format!("{by} undid the handover of {} to {}: you maintain it again.", app.app_id, ws_handle(&to)));
    }
    super::ownership::note(&to, &format!("App {} went back", app.app_id), &format!("{by} undid the handover of {}: it is {}'s again. Your work on it stays with the App.", app.app_id, from_label(&room_from)));
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("app-handover-undo", by, format!("undone {} {}", row.id, app.app_id), "cli", "-", "apps"));
    Ok(json!({ "applied": "handover_undo", "app": app.app_id, "to": from_label(&room_from), "steps": J::Object(out) }))
}

/// The vault half of an undo, from step 4's journal.
fn undo_vault(row: &Handover, j: &J, room_from: &str) -> J {
    use k2_vault::{Actor, Consumer, GrantRequest, Scope, SecretStore, Selector};
    let Ok(vault) = crate::vault::get() else { return json!({ "error": "the vault isn't open" }) };
    let me = Consumer::published_service(row.service_id.clone());
    let mut out = Vec::new();
    for v in j["vault"].as_array().cloned().unwrap_or_default() {
        let (id, new_id, name) = (v["secretId"].as_str().unwrap_or_default(), v["newId"].as_str().unwrap_or_default(), v["name"].as_str().unwrap_or_default());
        let r: Result<(), String> = (|| {
            match v["how"].as_str() {
                Some("moved") => {
                    vault.rescope(&Selector::Id(id.to_string()), Scope::workspace(room_from), Some(name), &Actor::Owner).map_err(|e| e.hint)?;
                }
                Some("copied" | "use_b") => {
                    for (g, _) in vault.grants_to(&me).map_err(|e| e.hint)? {
                        if g.secret_id != new_id {
                            continue;
                        }
                        let req = GrantRequest {
                            selector: Selector::Id(id.to_string()),
                            consumer: me.clone(),
                            delivery: g.delivery,
                            env_name: g.env_name.clone(),
                            file_name: g.file_name.clone(),
                            hosts_narrowed: g.hosts_narrowed.clone(),
                            expires_at: g.expires_at.clone(),
                            version: None,
                        };
                        vault.grant(req, &Actor::Owner).map_err(|e| e.hint)?;
                        vault.revoke(&g.id, &Actor::Owner).map_err(|e| e.hint)?;
                    }
                    if v["how"] == "copied" {
                        vault.destroy(&Selector::Id(new_id.to_string()), &Actor::Owner).map_err(|e| e.hint)?;
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        out.push(json!({ "name": name, "ok": r.is_ok(), "error": r.err() }));
    }
    let dropped = hov_db(|c| hs::drop_aliases(c, &row.id)).unwrap_or(0);
    json!({ "secrets": out, "aliasesDropped": dropped })
}

/// POST `/cli/app/handover/finish {app}`: end the undo window now. The
/// owner; or B once it has deployed the App since the handover.
fn finish_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let row = windowed(&app.app_id)?;
    if !super::ownership::owner(who) {
        let mine = who.passport().is_some_and(|(ws, _)| ws == row.to_workspace);
        let deployed = app.live_deployment.is_some() && row.journal.as_ref().map(|j| j["liveDeployment"].as_str() != app.live_deployment.as_deref()).unwrap_or(false);
        if !mine {
            return Err(no(codes::OWNER_ONLY, 403, "the owner, or the agent that took the App, ends the undo window"));
        }
        if !deployed {
            return Err(no(NOT_IN_WINDOW, 409, format!("deploy {} once first (the undo window is the owner's until then)", app.app_id)));
        }
    }
    finalize(&row)?;
    let row = hov_db(|c| hs::get(c, &row.id))?.ok_or_else(|| no("db_error", 500, "the handover row vanished"))?;
    Ok(json!({ "ok": true, "handover": row_json(&row) }))
}

/// The window ended (or was finished): `final`, and the journal's
/// credential material goes (skin.db hashes, the k2.db before-values).
fn finalize(row: &Handover) -> R<()> {
    if !hov_db(|c| hs::transition(c, &row.id, &[hs::APPLIED], hs::FINAL, deploy::now()))? {
        return Ok(());
    }
    let _ = hov_db(|c| hs::set_window(c, &row.id, None, deploy::now()));
    k2_core::skin::handover::forget_journal(&row.id).map_err(|e| no("skin_error", 500, e))?;
    // Keep what undo no longer needs out of the row: only the vault list
    // (names, no values) stays for the record.
    let keep = row.journal.as_ref().map(|j| json!({ "vault": j["vault"], "db": j["db"] }));
    hov_db(|c| hs::set_journal(c, &row.id, keep.as_ref(), deploy::now()))?;
    Ok(())
}

/// The periodic sweep: windows that ended → `final`; a deferred re-key
/// whose App was deployed since → its old keys revoked; offers nobody
/// answered in 7 days → `expired`.
pub fn sweep() {
    let now = deploy::now();
    for row in hov_db(|c| hs::in_state(c, hs::APPLIED)).unwrap_or_default() {
        if row.rekey_state.as_deref() == Some("deferred") {
            let app = hov_db(|c| store::get_app(c, &row.app_id)).ok().flatten();
            let at_apply = row.journal.as_ref().and_then(|j| j["liveDeployment"].as_str().map(str::to_string));
            if app.is_some_and(|a| a.live_deployment.is_some() && a.live_deployment != at_apply) {
                let old: Vec<String> = row.journal.as_ref().and_then(|j| j["oldKeys"].as_array().cloned()).unwrap_or_default().into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                revoke_keys(&old);
                let _ = hov_db(|c| hs::set_rekey(c, &row.id, "done", now));
            }
        }
        if row.window_until.is_some_and(|w| w <= now) {
            if let Err(e) = finalize(&row) {
                k2_core::log_debug!("[apps/handover] {}: window end: {}", row.id, e.hint);
            }
        }
    }
    for state in [hs::OFFERED, hs::ACCEPTED] {
        for row in hov_db(|c| hs::in_state(c, state)).unwrap_or_default() {
            if row.created_at + hs::TTL_SECS <= now {
                let _ = hov_db(|c| hs::transition(c, &row.id, &[state], hs::EXPIRED, now));
            }
        }
    }
}

/// Boot (§4.8), before the re-host wave: a handover the daemon died in
/// the middle of. `applying` (the commit never happened: the commit sets
/// `applied` in the same transaction) → `aborted`; `applied` with steps
/// left → finished forward. Then the sweep runs every 10 minutes.
pub fn boot_recover() -> Vec<String> {
    let now = deploy::now();
    let mut out = Vec::new();
    for row in hov_db(|c| hs::in_state(c, hs::APPLYING)).unwrap_or_default() {
        let _ = hov_db(|c| hs::transition(c, &row.id, &[hs::APPLYING], hs::ABORTED, now));
        out.push(format!("{} aborted (before its commit)", row.id));
    }
    for row in hov_db(|c| hs::in_state(c, hs::APPLIED)).unwrap_or_default() {
        if row.steps_done < hs::STEPS_ALL {
            let _gap = k2_core::skin::handover::begin_handing_over(
                &row.service_id,
                row.journal.as_ref().and_then(|j| j["roomFrom"].as_str()).unwrap_or(&row.from_workspace),
                &row.to_workspace,
            );
            finish_steps(&row.id, "system");
            out.push(format!("{} finished after a restart", row.id));
        }
    }
    sweep();
    out
}

/// Start the sweep thread (daemon boot).
pub fn spawn_sweeper() {
    let _ = std::thread::Builder::new().name("app-handover-sweep".into()).spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(600));
        sweep();
    });
}

// ── S6: hire --take-app ──────────────────────────────────────────────

/// `k2 agent hire … --take-app <app>` (§4.13): after the hire succeeded,
/// the App's handover to the new agent. The caller maintains the App (its
/// passport: an offer, auto-accepted since B was hired for it) or is the
/// owner (the card at once). Only an agent this hire created accepts
/// automatically (Q1); a workspace that already existed gets an ordinary
/// offer. Answers `{id, state, request?}`.
pub(crate) fn hire_take(who: &Who, app: &str, new_ws: &str, brief: Option<&str>, created: bool) -> R<J> {
    match mode() {
        Mode::On => {}
        _ => return Err(off_refusal()),
    }
    need_credential(who)?;
    let row = app_for(who, Some(app))?;
    let to = ws_by_id(new_ws).ok_or_else(|| no(codes::BAD_REQUEST, 404, "the hired workspace isn't registered"))?;
    if to.id == row.owner_workspace {
        return Err(no(SAME_WORKSPACE, 409, format!("{} already belongs to {}", row.app_id, ws_handle(&to.id))));
    }
    if let Some(b) = brief {
        if b.len() > hs::BRIEF_MAX {
            return Err(no(codes::BAD_REQUEST, 413, format!("a brief is at most {} KiB of markdown", hs::BRIEF_MAX / 1024)));
        }
    }
    let opts = Options::default();
    let out = if super::ownership::owner(who) {
        file_card(who, &row, &to, &opts, brief, Some("hired to take it over"), &format!("hire:{}", actor(who)))?
    } else if who.passport().is_some() {
        offer(who, &row, &to, &opts, brief, Some("hired to take it over"), created)?
    } else {
        return Err(no(codes::OWNER_ONLY, 403, "the App's own agent or the owner hires into an App"));
    };
    Ok(json!({
        "id": out["handover"]["id"],
        "state": out["handover"]["state"],
        "status": out["status"],
        "request": out["request"]["id"],
        "hint": out["hint"],
    }))
}

/// §4.11: "Apps you maintain" is stacked on the agent that took the App
/// (once), and both agents' AGENTS.md are rewritten so the App leaves A's
/// list and joins B's. AGENT.md (the persona) is never touched.
fn context_layers(to: &str, from: &str) {
    use k2_core::workspace::context_layers as cl;
    let path_of = |ws: &str| ws_by_id(ws).map(|w| w.path).filter(|p| std::path::Path::new(p).is_dir());
    let Some(b_path) = path_of(to) else { return };
    let stacked = cl::list_layers(&b_path).map(|ls| ls.iter().any(|l| l.source == cl::APPS_MAINTAINED_SOURCE)).unwrap_or(true);
    if !stacked {
        if let Err(e) = cl::add_layer(&b_path, None, Some(cl::APPS_MAINTAINED_ID), None) {
            k2_core::log_debug!("[apps/handover] stack \"Apps you maintain\" on {to}: {e:?}");
        }
    }
    let mut paths = vec![b_path];
    if let Some(a) = path_of(from) {
        paths.push(a);
    }
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    cl::refresh_roster_after_live_kind_change(&refs, cl::LiveKind::AppsMaintained);
}

// ── S7: the source copy (opt-in, Q6) ─────────────────────────────────

/// What a source copy leaves out: K2's own folder, env files and anything
/// that could hold a secret value.
fn copy_skips(name: &str) -> bool {
    name == ".k2" || name == ".git" || name == "node_modules" || name.starts_with(".env") || name.ends_with(".pem") || name.ends_with(".key")
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path, budget: &mut u64) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if copy_skips(&name) {
            continue;
        }
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let dest = to.join(&name);
        if ft.is_dir() {
            n += copy_tree(&e.path(), &dest, budget)?;
        } else if ft.is_file() {
            let len = e.metadata()?.len();
            if len > *budget {
                return Err(std::io::Error::other("the source is larger than 512 MiB: copy it yourself (git)"));
            }
            *budget -= len;
            std::fs::copy(e.path(), &dest)?;
            n += 1;
        }
    }
    Ok(n)
}

/// S7 (§4.2 P21): copy the App's source folder from A into
/// `<B>/apps/<app>/` (never `.k2/`, `.env*`, keys or `.git`), and point
/// `apps.folder_path` at the copy. Folder trust scans it at B's next
/// launch (it is new content). Best effort: a failure is reported, the
/// handover stands.
fn copy_source(row: &Handover) -> J {
    let Some(src) = row.journal.as_ref().and_then(|j| j["folderPath"].as_str().map(str::to_string)) else {
        return json!({ "skipped": "the App had no source folder on record" });
    };
    let Some(b) = ws_by_id(&row.to_workspace) else { return json!({ "error": "the workspace that took it is gone" }) };
    let from = std::path::Path::new(&src);
    if !from.is_dir() {
        return json!({ "skipped": format!("{src} isn't there any more") });
    }
    let dest = std::path::Path::new(&b.path).join("apps").join(&row.app_id);
    if dest.exists() {
        return json!({ "skipped": format!("{} exists already: left as it is", dest.display()) });
    }
    let mut budget: u64 = 512 << 20;
    match copy_tree(from, &dest, &mut budget) {
        Ok(files) => {
            let _ = hov_db(|c| {
                c.execute("UPDATE apps SET folder_path = ?2 WHERE app_id = ?1", rusqlite::params![row.app_id, dest.to_string_lossy()])
            });
            super::routes::mirror(&row.app_id);
            json!({ "copied": files, "to": dest.to_string_lossy() })
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dest);
            json!({ "error": e.to_string() })
        }
    }
}
