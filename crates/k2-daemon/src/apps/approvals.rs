//! Held App requests: one list, one approve, one deny for every kind
//! (`prd-k2-app-runtime-v1` §5.6). The 0.46.0 grace kinds (`config`,
//! `publish_run`, `app_key`, `clean_mode`), S15's egress kinds
//! (`egress`, `egress_mode`) and box services (`box_service`, 0171) share
//! the `app_widenings` table, these routes,
//! `k2 app approvals|approve|deny` and the vault confirm action
//! `app-approve`. Served with the grace routes, so not behind the runtime
//! preview flag (`--cmd` services have requests too).
//!
//! ```text
//! GET  /cli/app/approvals  [app, state=pending|all|…, all=1]
//! POST /cli/app/approve    {request, confirmToken}   the owner, in person
//! POST /cli/app/deny       {request, reason?}        an admin; the requester withdraws
//! ```
//!
//! The stricter rule from each lane holds:
//! - **Approve**: the server's owner only (the owner token or an Owner
//!   login), never a passport (a K2 shell tab included), with a fresh
//!   vault confirm for `app-approve` bound to the request id (Touch ID or
//!   a password; 60 s, single use). The owner token alone is not an
//!   approval. An expired request can't be approved.
//! - **Deny**: an Owner/Admin human denies someone else's request; the
//!   requester (its own workspace's passport, or the same human) withdraws
//!   its own. A passport never sees another workspace's requests.
//! - Requests expire after 7 days; the requester is told.

use std::collections::HashMap;

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{self, codes as dcodes};
use serde_json::{json, Value as J};

use super::egress_routes::codes;
use crate::cli_response::CliResponse;
use super::routes::{actor, db_err, json_body, need_credential, no, s, ws_handle, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/approvals", "/cli/app/approvals/revoke", "/cli/app/approve", "/cli/app/deny"];
pub const POST_ROUTES: &[&str] = &["/cli/app/approvals/revoke", "/cli/app/approve", "/cli/app/deny"];

/// The vault confirm action for approving any held App request.
pub const CONFIRM_ACTION: &str = "app-approve";

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/approvals" => approvals(who, params),
        "/cli/app/approvals/revoke" => revoke(who, body),
        "/cli/app/approve" => approve(who, body),
        "/cli/app/deny" => deny(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(db_err)
}

/// The workspace a passport (an agent, or a human in a K2 shell tab)
/// belongs to.
pub(crate) fn own_ws(who: &Who) -> Option<&str> {
    who.passport().map(|(ws, _)| ws)
}

pub(crate) fn session_of(who: &Who) -> Option<&str> {
    who.passport().map(|(_, s)| s)
}

/// The confirm stamp for an owner (the vault actor the token was minted for).
fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: k2_core::connect_users::Role::Owner } => Some(format!("connect:{username}")),
        _ => None,
    }
}

/// One request as the routes and the CLI see it.
pub(crate) fn request_json(r: &reqs::AppRequest) -> J {
    let mut v = serde_json::to_value(r).unwrap_or(J::Null);
    v["workspaceHandle"] = json!(ws_handle(&r.workspace_id));
    if r.kind == reqs::KIND_EGRESS {
        v["hosts"] = json!(r.hosts().iter().map(k2_core::deploy::egress::HostSpec::display).collect::<Vec<_>>());
    }
    if r.kind == reqs::KIND_BOX_SERVICE {
        v["services"] = json!(r.box_services());
    }
    v
}

/// [`request_json`] as `who` sees it: a passport (an agent, or a K2 shell
/// tab) sees a copy-host request's count, never who (review 0.46.4).
pub(crate) fn request_json_for(who: &Who, r: &reqs::AppRequest) -> J {
    let mut v = request_json(r);
    if who.passport().is_some() {
        super::agent_grants::redact_request(&mut v);
    }
    v
}

/// The answer for a change held for the owner (`202`, exit 2 in the CLI).
pub(crate) fn held_answer(r: &reqs::AppRequest) -> J {
    json!({
        "ok": true,
        "status": "held",
        "request": request_json(r),
        "kind": r.kind,
        "app": r.app_label,
        "diff": r.diff,
        "reasons": r.reasons,
        "expiresAt": r.expires_at,
        "hint": format!(
            "held for the owner: they approve it in person (k2 app approve {} from a terminal outside K2, or Settings → App \
             Approvals), or deny it. Check with k2 app approvals.",
            r.id
        ),
    })
}

/// Asked again within this many seconds is the same ask (not "reused").
const REUSE_AFTER_SECS: i64 = 2;

fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 90 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 36 * 3600 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

/// Where the owner sees one held request, and whether a permission card
/// is up for it (0.46.4 field report, NSI #9/#11: a held `k2 app install`
/// printed only "held for the owner: w_…", reused an old request without
/// saying so, and never said whether the owner would see a card):
/// - `reused` / `askedAgo`: this call found the same request pending from
///   earlier (`requests::create` returns it) instead of filing a new one;
/// - `card`: for an agent with permission cards on, the `apps.approve`
///   card for this request is raised now (or the open one is found, so a
///   repeat after the owner turned cards on raises it rather than staying
///   silent); cards off, or a person: where the owner approves it.
pub(crate) fn held_note(who: &Who, r: &reqs::AppRequest) -> J {
    let age = deploy::now() - r.requested_at;
    let reused = age > REUSE_AFTER_SECS;
    let approvals = format!("Settings → App Approvals (k2 app approvals; the owner approves with k2 app approve {})", r.id);
    let card = match who {
        Who::Agent { project_id, session_id } if k2_core::permission::gate::enabled_for(project_id) => {
            let why: String = r
                .reason
                .as_deref()
                .filter(|w| !w.trim().is_empty())
                .unwrap_or(r.summary.as_str())
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(k2_core::permission::request::limits::WHY_MAX_CHARS)
                .collect();
            let (status, v) = crate::permission::routes::raise_held_card(project_id, session_id, &r.id, why.trim());
            let st = v.get("status").and_then(J::as_str).unwrap_or("");
            let id = v.get("id").and_then(J::as_str).unwrap_or("");
            let thread = v.get("addr").and_then(J::as_str).unwrap_or("");
            if status == 200 && (st == "pending" || st == "already_pending") {
                let new = st == "pending";
                let to: Vec<String> = v
                    .get("routedTo")
                    .and_then(J::as_array)
                    .map(|a| a.iter().filter_map(|x| x.get("login").and_then(J::as_str).map(str::to_string)).collect())
                    .unwrap_or_default();
                json!({
                    "raised": true,
                    "new": new,
                    "id": id,
                    "thread": thread,
                    "routedTo": to,
                    "where": format!(
                        "{} permission card {id} is on {thread}'s Thread{}: the owner approves it there, or in {approvals}",
                        if new { "a" } else { "the" },
                        if to.is_empty() { String::new() } else { format!(" for {}", to.join(", ")) },
                    ),
                })
            } else {
                // The card couldn't go up (no Thread yet, the ask limit…):
                // the request still waits in App Approvals; the agent may ask.
                let e = &v["error"];
                json!({
                    "raised": false,
                    "error": { "code": e.get("code").and_then(J::as_str).unwrap_or(st), "hint": e.get("hint").and_then(J::as_str).unwrap_or("") },
                    "ask": format!("k2 permission request apps.approve request={} --why \"<what you need it for>\"", r.id),
                    "where": format!("no permission card went up; the request waits in {approvals}"),
                })
            }
        }
        Who::Agent { .. } => json!({
            "raised": false,
            "cardsOff": true,
            "where": format!("permission cards are off for this agent, so no card went up: the request waits in {approvals}"),
        }),
        _ => json!({ "raised": false, "where": format!("it waits in {approvals}") }),
    };
    json!({ "reused": reused, "askedAgo": if reused { Some(ago(age)) } else { None }, "requestedAt": r.requested_at, "card": card })
}

/// Add [`held_note`] to every held request in a `202` answer from the App
/// routes (one chokepoint for every hold: limits, disk, engine, caps,
/// egress, box services, the switch, grace kinds).
pub(crate) fn enrich_held(who: &Who, mut resp: CliResponse) -> CliResponse {
    if !resp.status.starts_with("202") {
        return resp;
    }
    let Ok(mut v) = serde_json::from_str::<J>(&resp.body) else { return resp };
    let mut first: Option<J> = None;
    for at in ["", "/held", "/egressHeld"] {
        let id = v.pointer(&format!("{at}/request/id")).and_then(J::as_str).map(str::to_string);
        let Some(id) = id else { continue };
        let Some(r) = db(|c| reqs::get(c, &id)).ok().flatten() else { continue };
        if r.status != reqs::STATUS_PENDING {
            continue;
        }
        let note = held_note(who, &r);
        let slot = if at.is_empty() { Some(&mut v) } else { v.pointer_mut(at) };
        if let Some(J::Object(o)) = slot {
            for (k, x) in note.as_object().into_iter().flatten() {
                o.insert(k.clone(), x.clone());
            }
        }
        first.get_or_insert(note);
    }
    if let (Some(note), Some(o)) = (first, v.as_object_mut()) {
        for k in ["reused", "askedAgo", "card"] {
            o.entry(k.to_string()).or_insert(note[k].clone());
        }
    }
    resp.body = v.to_string();
    resp
}

/// Tell the requesting workspace (a `k2 msg` wake of its agent).
pub(crate) fn notify_requester(r: &reqs::AppRequest, text: &str) {
    if let Some(ws) = r.requester_workspace.as_deref() {
        super::alerts::notify(ws, text);
    }
}

/// Egress kinds changed the App's host list or mode: the proxy reloads.
/// A box service request closed: the App's nft rule reloads.
fn egress_changed(r: &reqs::AppRequest) {
    if r.kind == reqs::KIND_BOX_SERVICE {
        if let Some(app) = r.app_id.as_deref() {
            if let Err(e) = super::box_service_routes::changed(app) {
                k2_core::log_debug!("[apps] {app}: box service door reload: {e}");
            }
        }
    }
    if r.kind == reqs::KIND_EGRESS || r.kind == reqs::KIND_EGRESS_MODE {
        if let Some(app) = r.app_id.as_deref() {
            super::egress_proxy::reload(app);
            k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "apps": "egress", "app": app }));
        }
    }
}

fn app_audit(r: &reqs::AppRequest, event: &str, by: &str, payload: J) {
    let Some(app) = r.app_id.as_deref() else { return };
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app, event, by, None, &payload) {
        k2_core::log_debug!("[apps] audit {app} {event}: {e}");
    }
}

pub(crate) fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    #[cfg(test)]
    if let Some(v) = super::egress_routes::test_seams::vault() {
        return Ok(v);
    }
    crate::vault::get().map_err(|e| no(codes::VAULT_CLOSED, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

fn find(token: Option<&str>) -> R<reqs::AppRequest> {
    let t = token.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "name the request (its w_… id; k2 app approvals)"))?;
    db(|c| reqs::find(c, t))?.ok_or_else(|| no(codes::REQUEST_NOT_FOUND, 404, format!("no App request '{t}' (k2 app approvals)")))
}

fn decide_err(code: &'static str) -> Refusal {
    match code {
        "request_expired" => no(codes::REQUEST_EXPIRED, 410, "this request expired (7 days); ask again"),
        "not_pending" => no(codes::NOT_PENDING, 409, "this request was already decided (or expired after 7 days)"),
        _ => no(codes::REQUEST_NOT_FOUND, 404, "no such request"),
    }
}

/// Expire what is due and tell the requesters.
fn expire_and_tell() -> R<()> {
    let gone = db(|c| reqs::expire_due(c, deploy::now()))?;
    for r in &gone {
        let mut closed = r.clone();
        closed.status = reqs::STATUS_EXPIRED.to_string();
        super::handover::request_closed(&closed);
        notify_requester(r, &format!("[k2 apps] request {} expired after 7 days without an answer: {}", r.id, r.summary));
        egress_changed(r);
    }
    if !gone.is_empty() {
        changed(None);
    }
    Ok(())
}

/// How many held App requests wait on this box (the owner's list).
pub(crate) fn pending_count() -> u64 {
    db(|c| reqs::list(c, &reqs::Filter { status: Some(reqs::STATUS_PENDING), ..Default::default() }, 1000))
        .map(|v| v.len() as u64)
        .unwrap_or(0)
}

/// The one chokepoint for "the held App requests changed" (filed or asked
/// again, approved, denied, withdrawn, expired; 0.46.4 field report: the
/// owner's App Approvals never refreshed). Local windows get
/// `sync:settings` `{apps: approvals}`; every client, K2 Connect included,
/// gets `app_approvals_changed` with the pending count (the badge).
pub(crate) fn changed(request: Option<&str>) {
    let pending = pending_count();
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        json!({ "apps": "approvals", "request": request, "pending": pending }),
    );
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::AppApprovalsChanged { pending });
}

// ── list ─────────────────────────────────────────────────────────────

/// GET `/cli/app/approvals?app=&state=pending|all|<status>` (or `all=1`).
fn approvals(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let (mut app_id, mut service_id) = (None, None);
    if let Some(raw) = params.get("app").map(|a| a.trim()).filter(|a| !a.is_empty()) {
        match super::grace::target(who, Some(raw)) {
            Ok(t) => service_id = Some(t.service_id),
            // A runtime App the caller sees but with no service row.
            Err(e) => match super::routes::app_for(who, Some(raw)) {
                Ok(a) => app_id = Some(a.app_id),
                Err(_) => return Err(e),
            },
        }
    }
    let all = params.get("all").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    let state = match params.get("state").map(String::as_str) {
        _ if all => None,
        None | Some("") | Some("pending") => Some(reqs::STATUS_PENDING),
        Some("all") => None,
        Some(s @ ("approved" | "denied" | "withdrawn" | "expired" | "failed")) => Some(s),
        Some(other) => {
            return Err(no(dcodes::BAD_REQUEST, 400, format!("state is pending, approved, denied, withdrawn, expired, failed or all, not '{other}'")))
        }
    };
    expire_and_tell()?;
    let f = reqs::Filter { app_id: app_id.as_deref(), service_id: service_id.as_deref(), workspace: own_ws(who), status: state };
    let rows = db(|c| reqs::list(c, &f, 200))?;
    // The badge count for the caller's own view (the owner: the whole box).
    let pending = if state == Some(reqs::STATUS_PENDING) {
        rows.len()
    } else {
        rows.iter().filter(|r| r.status == reqs::STATUS_PENDING).count()
    };
    Ok(json!({ "ok": true, "requests": rows.iter().map(|r| request_json_for(who, r)).collect::<Vec<_>>(), "pendingCount": pending,
        "ownerConfirm": owner_confirm_status() }))
}

// ── approve ──────────────────────────────────────────────────────────

/// POST `/cli/app/approve {request, confirmToken}`: the owner, in person.
fn approve(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let Some(stamp) = confirm_stamp(who) else {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner approves App requests, in person: k2 app approve <id> from a terminal outside K2, \
             or Settings → App Approvals. An agent can propose, never approve.",
        ));
    };
    let v = json_body(body)?;
    let r = find(s(&v, "request"))?;
    // The confirm comes first (bound to this request, single use); the
    // decision then refuses a request that is no longer pending.
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(
            codes::VAULT_CONFIRM_REQUIRED,
            403,
            "approving needs a fresh owner confirm (Touch ID, your login or vault password): k2 app approve <id> asks for it",
        ));
    };
    let proof = vault()?
        .confirm
        .take(token, CONFIRM_ACTION, Some(&r.id), &stamp)
        .map_err(|_| no(codes::VAULT_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this request, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(codes::VAULT_CONFIRM_REQUIRED, 403, "no owner confirm")),
    };
    apply_as(&r, &actor(who), &via)
}

/// Decide `r` approved by `by` (`via`: the confirm method, or `switch-2`
/// for an agent's own non-loosening widening, `access::self_apply`) and
/// carry it out: one path for every approval.
pub(crate) fn apply_as(r: &reqs::AppRequest, by: &str, via: &str) -> R<J> {
    let by = by.to_string();
    let own = via == super::access::SELF_VIA;
    let decided = db(|c| Ok(reqs::decide(c, &r.id, reqs::STATUS_APPROVED, &by, Some(via), None, deploy::now())))?.map_err(decide_err)?;
    changed(Some(&decided.id));
    // Egress kinds applied with the decision (the host list); the door
    // kinds (0.46.1) and the grace kinds are carried out now; a kind
    // another lane owns runs its registered applier.
    let applied = if decided.kind == reqs::KIND_EGRESS || decided.kind == reqs::KIND_EGRESS_MODE {
        egress_changed(&decided);
        Ok(json!({ "applied": decided.summary, "note": "it applies to the App's next connection" }))
    } else if decided.kind == reqs::KIND_BOX_SERVICE {
        // The list changed with the decision; the App's door reloads now.
        Ok(super::box_service_routes::applied_note(&decided))
    } else if DOOR_KINDS.contains(&decided.kind.as_str()) {
        apply_door(&decided, &by)
    } else if let Some(f) = applier(&decided.kind) {
        f(&decided, &by)
    } else {
        super::grace::apply_request(&decided, &by)
    };
    let (status, result) = match &applied {
        Ok(v) => (reqs::STATUS_APPROVED, v.clone()),
        Err(e) => (reqs::STATUS_FAILED, json!({ "code": e.code, "hint": e.hint })),
    };
    db(|c| reqs::set_result(c, &decided.id, status, &result))?;
    let done = db(|c| reqs::get(c, &decided.id))?.unwrap_or(decided);
    app_audit(
        &done,
        if own { "widening_self_applied" } else { "widening_approved" },
        &by,
        json!({ "request": done.id, "kind": done.kind, "diff": done.diff, "via": via, "status": status }),
    );
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        if own { "app-self-apply" } else { "app-approve" },
        &by,
        format!("{} {} {} ({}) via {via}", status, done.id, done.kind, done.summary),
        "cli",
        "-",
        "apps",
    ));
    if !own {
        notify_requester(
            &done,
            &format!(
                "[k2 apps] the owner approved {}: {}{}",
                done.id,
                done.summary,
                if status == reqs::STATUS_FAILED { ", but it failed to apply" } else { ". It applies now." }
            ),
        );
    }
    let mut out = json!({
        "ok": status == reqs::STATUS_APPROVED,
        "request": request_json(&done),
        "status": status,
        "result": result,
        "note": if own { "applied under \"…and manage them end to end\"" } else { "approved; it applies now" },
    });
    if let Err(e) = applied {
        out["error"] = json!({ "code": e.code, "hint": e.hint });
        out["note"] = json!("approved, but applying it failed");
    }
    Ok(out)
}

// ── revoke a standing run approval ───────────────────────────────────

/// POST `/cli/app/approvals/revoke {request}` or `{app}` (0.46.6, NSI
/// papercut 13): the owner withdraws an approved `k2 publish run`'s
/// standing, so the next identical re-run is held again. `app` revokes
/// every standing approval of that App label. The owner only (the owner
/// token or an Owner login), never a passport. Each revoke is audited
/// (who, when, which). The decisions stay `approved`: they applied.
fn revoke(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    if confirm_stamp(who).is_none() {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner revokes an approval (k2 app approvals revoke <id> from a terminal outside K2)",
        ));
    }
    let v = json_body(body)?;
    let by = actor(who);
    let now = deploy::now();
    let targets: Vec<reqs::AppRequest> = match (s(&v, "request"), s(&v, "app")) {
        (Some(id), _) => vec![find(Some(id))?],
        (None, Some(app)) => db(|c| {
            let mut st = c.prepare(
                "SELECT id FROM app_widenings WHERE kind = ?1 AND app_label = ?2 AND status = 'approved' \
                 AND json_extract(result_json, '$.standingRevoked') IS NULL ORDER BY requested_at",
            )?;
            let ids = st.query_map(rusqlite::params![reqs::KIND_PUBLISH_RUN, app], |r| r.get::<_, String>(0))?;
            let ids = ids.collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter().map(|id| reqs::get(c, id).map(|o| o.expect("row just listed"))).collect()
        })?,
        (None, None) => return Err(no(dcodes::BAD_REQUEST, 400, "name the approval (request: its w_… id) or the App (app)")),
    };
    if targets.is_empty() {
        return Err(no(codes::REQUEST_NOT_FOUND, 404, "no standing run approval for that App (k2 app approvals --state approved)"));
    }
    let mut revoked = Vec::new();
    for r in &targets {
        let done = db(|c| Ok(reqs::revoke_standing(c, &r.id, &by, now)))?.map_err(|e| match e {
            "not_standing" => no(codes::NOT_PENDING, 409, format!("{} is not an approved k2 publish run: only those stand for re-runs", r.id)),
            "already_revoked" => no(codes::NOT_PENDING, 409, format!("{} was revoked already", r.id)),
            _ => no(codes::REQUEST_NOT_FOUND, 404, format!("no App request '{}'", r.id)),
        })?;
        app_audit(&done, "run_approval_revoked", &by, json!({ "request": done.id, "kind": done.kind, "at": now }));
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "app-approval-revoke",
            &by,
            format!("revoked {} {} ({})", done.id, done.kind, done.summary),
            "cli",
            "-",
            "apps",
        ));
        changed(Some(&done.id));
        revoked.push(request_json(&done));
    }
    Ok(json!({
        "ok": true,
        "revoked": revoked,
        "note": "the next identical k2 publish run of this App asks the owner again",
    }))
}

// ── deny / withdraw ──────────────────────────────────────────────────

/// POST `/cli/app/deny {request, reason?}`: an Owner/Admin human denies;
/// the requester withdraws its own.
fn deny(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let r = find(s(&v, "request"))?;
    let by = actor(who);
    let reason = s(&v, "reason").map(|r| r.chars().take(500).collect::<String>());
    let mine = match own_ws(who) {
        Some(ws) => r.requester_workspace.as_deref() == Some(ws),
        None => r.requested_by == by,
    };
    let status = if who.passport().is_none() && who.is_admin() && !mine {
        reqs::STATUS_DENIED
    } else if mine {
        reqs::STATUS_WITHDRAWN
    } else if who.passport().is_some() {
        // Another workspace's request, or one this passport didn't file.
        return Err(no(codes::REQUEST_NOT_FOUND, 404, format!("no App request '{}' you can withdraw (k2 app approvals)", r.id)));
    } else {
        return Err(no(codes::OWNER_ONLY, 403, "only the owner or an admin denies a request (the requester withdraws it)"));
    };
    let d = db(|c| Ok(reqs::decide(c, &r.id, status, &by, None, reason.as_deref(), deploy::now())))?.map_err(decide_err)?;
    changed(Some(&d.id));
    super::handover::request_closed(&d);
    let event = if status == reqs::STATUS_DENIED { "widening_denied" } else { "widening_withdrawn" };
    app_audit(&d, event, &by, json!({ "request": d.id, "kind": d.kind, "reason": reason }));
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("app-deny", &by, format!("{status} {}", d.id), "cli", "-", "apps"));
    egress_changed(&d);
    if status == reqs::STATUS_DENIED {
        notify_requester(
            &d,
            &format!(
                "[k2 apps] the owner denied {}{}: {}",
                d.id,
                reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default(),
                d.summary
            ),
        );
    }
    Ok(json!({ "ok": true, "request": request_json_for(who, &d), "status": status }))
}

// ── 0.46.1 door kinds (S19, S20, S42) ────────────────────────────────

/// Kinds this module applies on approval.
pub(crate) const DOOR_KINDS: &[&str] = &[reqs::KIND_DEPLOY, reqs::KIND_INSTALL, reqs::KIND_MANAGE, reqs::KIND_QUOTA];

/// How another lane applies an approved request of a kind it owns
/// (`db_raw`, `overcommit`): register at boot with [`register_applier`].
pub(crate) type Applier = fn(&reqs::AppRequest, &str) -> R<J>;

fn appliers() -> &'static std::sync::RwLock<HashMap<&'static str, Applier>> {
    static A: std::sync::OnceLock<std::sync::RwLock<HashMap<&'static str, Applier>>> = std::sync::OnceLock::new();
    A.get_or_init(|| std::sync::RwLock::new(HashMap::new()))
}

/// Register the applier for `kind` (one of `reqs::ALL_KINDS`). Until one
/// is registered, approving `db_raw` / `overcommit` records the decision
/// and answers `failed` with `no_applier`.
#[allow(dead_code)] // registered at boot by the lanes that own those kinds
pub(crate) fn register_applier(kind: &'static str, f: Applier) {
    appliers().write().unwrap_or_else(|e| e.into_inner()).insert(kind, f);
}

fn applier(kind: &str) -> Option<Applier> {
    if let Some(f) = appliers().read().unwrap_or_else(|e| e.into_inner()).get(kind) {
        return Some(*f);
    }
    (kind == reqs::KIND_DB_RAW || kind == reqs::KIND_OVERCOMMIT).then_some(no_applier as Applier)
}

fn no_applier(r: &reqs::AppRequest, _by: &str) -> R<J> {
    Err(no("no_applier", 409, format!("nothing on this daemon applies '{}' requests yet; ask again after the update", r.kind)))
}

/// A held request to file for a runtime App (or an App id not yet
/// installed: `app = None`).
pub(crate) struct Filing<'a> {
    pub who: &'a Who,
    pub kind: &'static str,
    pub app: Option<&'a k2_core::deploy::store::AppRow>,
    pub label: &'a str,
    pub workspace: &'a str,
    pub body: J,
    pub diff: J,
    pub summary: String,
    pub reasons: Vec<String>,
    pub reason: Option<&'a str>,
}

/// File (or find the same pending) request.
pub(crate) fn file(f: &Filing<'_>) -> R<reqs::AppRequest> {
    let by = actor(f.who);
    let n = reqs::NewRequest {
        kind: f.kind,
        app_id: f.app.map(|a| a.app_id.as_str()),
        service_id: f.app.map(|a| a.service_id.as_str()),
        app_label: f.label,
        workspace_id: f.workspace,
        body: f.body.clone(),
        diff: f.diff.clone(),
        summary: f.summary.clone(),
        reasons: f.reasons.clone(),
        reason: f.reason,
        deployment_id: None,
        requested_by: &by,
        requester_workspace: own_ws(f.who),
        requester_session: session_of(f.who),
    };
    let r = db(|c| reqs::create(c, &n, deploy::now()))?;
    app_audit(&r, "widening_held", &by, json!({ "request": r.id, "kind": r.kind, "diff": r.diff }));
    changed(Some(&r.id));
    Ok(r)
}

/// Carry out an approved door request.
fn apply_door(r: &reqs::AppRequest, by: &str) -> R<J> {
    match r.kind.as_str() {
        reqs::KIND_DEPLOY | reqs::KIND_INSTALL => {
            let app_id = r.app_id.as_deref().ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "the held deploy names no App"))?;
            let seed = r.body.get("seed").and_then(J::as_str).ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "the held deploy names no Seed"))?;
            let app = super::routes::app_for(&Who::Owner, Some(app_id))?;
            super::suspend::refuse_if_suspended(&app.app_id)?;
            let set: Vec<String> = r
                .diff
                .pointer("/caps/approvedAfter")
                .and_then(J::as_array)
                .map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            // The owner approved this set; anything approved since stays
            // (one approval never narrows another).
            let now_set = {
                let h = k2_core::db::shared();
                let conn = h.lock();
                let mut all = super::grants::recorded(&conn, &app.app_id).map_err(db_err)?.unwrap_or_default();
                all.extend(set);
                super::grants::record(&conn, &app.app_id, &all).map_err(db_err)?;
                super::grants::recorded(&conn, &app.app_id).map_err(db_err)?.unwrap_or_default()
            };
            let reason = r.body.get("reason").and_then(J::as_str);
            let dep = super::supervisor::begin(&app, seed, k2_core::deploy::store::KIND_DEPLOY, &r.requested_by, reason)?;
            Ok(json!({ "applied": "deploy", "deployment": dep, "approvedCaps": now_set, "approvedBy": by }))
        }
        reqs::KIND_MANAGE => {
            let route = r.body.get("route").and_then(J::as_str).ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "unreadable held request"))?;
            // 0.46.4: a move plan's owner steps, run as the owner who
            // approved them (never replayed as the requesting agent).
            if route == super::move_plan::APPLY_ROUTE {
                return super::move_plan::apply(r, by);
            }
            // 0.46.5: an App handover card (prd-app-handoff-v1), run as
            // the owner who approved it, never replayed as an agent.
            if route == super::handover::APPLY_ROUTE {
                return super::handover::apply(r, by);
            }
            if route == super::handover::UNDO_ROUTE {
                return super::handover::apply_undo(r, by);
            }
            // 0.46.4: an agent's App-grant owner steps (copy-host, the
            // guests-rooms grant, a label transfer), run as the owner with
            // exactly the values the request showed.
            if let Some(done) = super::agent_grants::apply(r, by) {
                return done;
            }
            let route = MANAGE_HELD
                .iter()
                .chain(std::iter::once(&super::agent_grants::TEST_GUEST_ADD))
                .find(|p| **p == route)
                .ok_or_else(|| no(dcodes::BAD_REQUEST, 400, format!("'{route}' is not a held App change")))?;
            // An agent's request replays as that agent. A person's (the
            // owner token at critical, an Admin login, a K2 shell tab) has
            // no agent to replay as: the owner approved it in person, so
            // it runs as the owner (as `publish_routes::run_approved` does;
            // prd-app-resources-disk-guard-v1 §8 step 4).
            let who = if r.requested_by.starts_with("agent:") {
                let ws = r.requester_workspace.clone().ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "the held request has no requesting workspace"))?;
                Who::Agent { project_id: ws, session_id: r.requester_session.clone().unwrap_or_default() }
            } else {
                Who::Owner
            };
            let body = r.body.get("body").cloned().unwrap_or_else(|| json!({}));
            let resp = with_manage_approved(|| super::routes::handle(route, who, body.to_string().as_bytes(), &HashMap::new()));
            let mut v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
            if *route == super::agent_grants::TEST_GUEST_ADD {
                // Never kept with the request (Settings and the audit show
                // it): the agent sets the test guest's password itself.
                if let Some(o) = v.as_object_mut() {
                    if o.remove("password").is_some() {
                        o.insert(
                            "passwordNote".into(),
                            json!("not kept: the agent sets it once, k2 app user password <name> --test --password-stdin"),
                        );
                    }
                }
            }
            if resp.status.starts_with('2') {
                Ok(json!({ "applied": route, "status": resp.status, "answer": v }))
            } else {
                let hint = v.pointer("/error/hint").and_then(J::as_str).unwrap_or("").to_string();
                let code = v.pointer("/error/code").and_then(J::as_str).unwrap_or("failed");
                Err(no("replay_failed", 409, format!("{route}: {code}: {hint}")))
            }
        }
        reqs::KIND_QUOTA => {
            let app_id = r.app_id.as_deref().ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "the held quota request names no App"))?;
            let o: super::quotas::Override = serde_json::from_value(r.body.get("override").cloned().unwrap_or(J::Null))
                .map_err(|_| no(dcodes::BAD_REQUEST, 400, "unreadable quota request"))?;
            super::quotas::apply(app_id, &o, by)?;
            Ok(json!({ "applied": "quota", "quotas": o.apply(super::quotas::DEFAULTS) }))
        }
        _ => Err(no(dcodes::BAD_REQUEST, 400, "not a door request")),
    }
}

// ── S42: agents without "Build and run Apps" (switch 1) ──────────────

/// Changes an agent without the switch asks for become a held `manage`
/// request (§5.11.1: "every change is a held request"), instead of the
/// old `apps_manage_off` refusal. Reads stay open; `suspend` narrows and
/// never waits.
pub(crate) const MANAGE_HELD: &[&str] = &[
    "/cli/app/build",
    "/cli/app/install",
    "/cli/app/deploy",
    "/cli/app/rollback",
    "/cli/app/stop",
    "/cli/app/start",
    "/cli/app/restart",
    "/cli/app/config",
    "/cli/app/uninstall",
    "/cli/app/link/rotate",
    "/cli/app/exec",
    "/cli/app/quota/set",
    // 0.46.3: moving a --cmd service in (its data, its public label).
    "/cli/app/data/put",
    // 0.46.4: tidying an App's data (ls and get are reads: refused
    // without the switch, since a held read would answer nobody).
    "/cli/app/data/rm",
    "/cli/app/data/mv",
    "/cli/app/label/take",
    "/cli/app/label/return",
    // 0.46.4: extra subdomains onto an App.
    "/cli/app/domain/add",
    "/cli/app/domain/rm",
    // 0.46.6: custom domains (check, primary).
    "/cli/app/domain/check",
    "/cli/app/domain/primary",
];

thread_local! {
    static MANAGE_APPROVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` as the replay of an approved `manage` request: the switch
/// check passes for the requesting agent (every other check still runs,
/// the always-per-event holds included).
fn with_manage_approved<T>(f: impl FnOnce() -> T) -> T {
    // Restored on every exit, a panic included (review 0.46.4 B-4): the
    // flag never outlives this replay on this worker thread.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let before = self.0;
            MANAGE_APPROVED.with(|c| c.set(before));
        }
    }
    let _restore = Restore(MANAGE_APPROVED.with(|c| c.replace(true)));
    f()
}

/// Inside the replay of an approved `manage` request?
pub(crate) fn manage_approved_now() -> bool {
    MANAGE_APPROVED.with(|c| c.get())
}

/// For an agent without the switch, file `path` + `body` as a held
/// `manage` request. `None` = not held (a human, the switch is on, a read,
/// or the replay of an approved request).
pub(crate) fn hold_without_switch(path: &str, who: &Who, body: &[u8]) -> Option<R<J>> {
    if !MANAGE_HELD.contains(&path) || manage_approved_now() {
        return None;
    }
    let ws = super::routes::agent_ws(who)?;
    super::routes::ws_by_id(ws)?;
    if super::access::level(ws) >= super::access::Level::Build {
        return None;
    }
    Some((|| {
        let v = json_body(body)?;
        let label = s(&v, "app")
            .map(str::to_ascii_lowercase)
            .or_else(|| s(&v, "folder").and_then(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string())))
            .unwrap_or_else(|| "(new App)".into());
        let app = s(&v, "app").and_then(|a| super::routes::app_for(who, Some(a)).ok());
        let verb = path.trim_start_matches("/cli/app/").replace('/', " ");
        // The hostname a domain or label change is about (security review):
        // in the summary and the diff, so the owner sees what they approve.
        let host = (path.starts_with("/cli/app/domain/") || path.starts_with("/cli/app/label/"))
            .then(|| s(&v, "label").or_else(|| s(&v, "host")))
            .flatten()
            .map(|raw| match super::domains::label_of(raw) {
                Ok(l) => l,
                // 0.46.6: a custom domain is shown as the name it is.
                Err(_) if super::domains::is_custom_host(raw) => match k2_core::domains::normalize_hostname(raw) {
                    Ok(h) => h,
                    Err(_) => format!("{:?}", raw.chars().take(80).collect::<String>()),
                },
                Err(_) => format!("{:?}", raw.chars().take(80).collect::<String>()),
            });
        let summary = match &host {
            Some(h) => format!("k2 app {verb} {label} {h}"),
            None => format!("k2 app {verb} {label}"),
        };
        let mut lines = vec![summary.clone()];
        if let Some(h) = &host {
            let sub = super::origins::box_subdomain();
            let sub = if sub.is_empty() { "<this box>".to_string() } else { sub };
            lines.push(format!("hostname: {h}.{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
        }
        let r = file(&Filing {
            who,
            kind: reqs::KIND_MANAGE,
            app: app.as_ref(),
            label: &label,
            workspace: ws,
            body: json!({ "route": path, "body": v }),
            diff: json!({ "lines": lines, "route": path }),
            summary,
            reasons: vec!["this agent's workspace doesn't have \"Build and run Apps\" on".into()],
            reason: s(&v, "reason"),
        })?;
        let mut out = held_answer(&r);
        out["hint"] = json!(format!(
            "held for the owner: \"Build and run Apps\" is off for this workspace, so every App change waits for \
             the owner (k2 app approve {} outside K2, or Settings → App Approvals). To stop a misbehaving App now: k2 app suspend <app>.",
            r.id
        ));
        Ok(out)
    })())
}

// ── AR43: can the owner confirm on this box? ─────────────────────────

/// How the owner can confirm an approval here (`ownerConfirm` on
/// `/cli/app/approvals`; the Settings banner). Off a Mac the only ways are
/// the vault password and an Owner Connect login: with neither, approvals
/// stay held until one is set.
pub(crate) fn owner_confirm_status() -> J {
    if cfg!(target_os = "macos") {
        return json!({ "available": true, "methods": ["touch-id", "login-password"] });
    }
    let vault_pw = crate::vault::get().map(|v| v.has_confirm_password()).unwrap_or(false);
    let owner_login = k2_core::connect_users::list_users()
        .map(|u| u.iter().any(|u| !u.disabled && u.role == k2_core::connect_users::Role::Owner))
        .unwrap_or(false);
    let mut methods = Vec::new();
    if vault_pw {
        methods.push("vault-password");
    }
    if owner_login {
        methods.push("connect-password");
    }
    let mut v = json!({ "available": vault_pw || owner_login, "methods": methods });
    if !vault_pw && !owner_login {
        v["hint"] = json!(
            "no vault password and no Owner Connect login on this box: App approvals stay held until one is set (k2 vault password set)"
        );
    }
    v
}
