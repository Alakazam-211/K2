//! `/cli/app/egress*` and the held-widening routes (`prd-k2-app-runtime-v1`
//! §5.6, §8.1, S15). `k2 app egress …`, `k2 app approvals|approve|deny`.
//!
//! Who may do what:
//! - **Read** (`egress`, `egress/log`, `approvals`): anyone who sees the
//!   App. A passport sees only its own workspace's Apps.
//! - **Propose** (`egress/add`, `egress/mode broad`): the same. Always a
//!   held request, whoever asks, the owner token included (§5.6 "always
//!   per event", K25, AR14): a new egress host is never live until the
//!   owner approves it with a fresh owner confirm.
//! - **Narrow** (`egress/rm`, `egress/mode allowlist`): the same, at once,
//!   no switch needed (narrowing never waits). Open tunnels to a removed
//!   host are cut.
//! - **Approve**: the server's owner only (owner token or an Owner login),
//!   never a passport (a K2 shell tab included), with a vault confirm
//!   token for action `app-approve` bound to the request id (Touch ID or
//!   the login/vault password; 60 s, single use).
//! - **Deny**: an Owner/Admin human; the requester (its own workspace's
//!   passport, or the same human) withdraws instead.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::deploy::egress::{self as pol, HostSpec};
use k2_core::deploy::{self, codes as dcodes};
use serde_json::{json, Value as J};

use super::routes::{actor, agent_ws, app_for, db_err, json_body, need_credential, no, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &[
    "/cli/app/egress",
    "/cli/app/egress/log",
    "/cli/app/egress/add",
    "/cli/app/egress/rm",
    "/cli/app/egress/mode",
    "/cli/app/approvals",
    "/cli/app/approve",
    "/cli/app/deny",
];

pub const POST_ROUTES: &[&str] = &["/cli/app/egress/add", "/cli/app/egress/rm", "/cli/app/egress/mode", "/cli/app/approve", "/cli/app/deny"];

/// The vault confirm action for approving a held App widening (§5.6 step 4).
pub const CONFIRM_ACTION: &str = "app-approve";

pub mod codes {
    pub const HELD: &str = "held";
    pub const OWNER_ONLY: &str = "owner_only";
    pub const OWNER_CONFIRM_REQUIRED: &str = "owner_confirm_required";
    pub const REQUEST_NOT_FOUND: &str = "request_not_found";
    pub const NOT_PENDING: &str = "not_pending";
    pub const REQUEST_EXPIRED: &str = "request_expired";
    pub const EGRESS_HOST_INVALID: &str = "egress_host_invalid";
    pub const EGRESS_SELF: &str = "egress_self";
    pub const OPEN_PATH_ONLY: &str = "open_path_only";
    pub const VAULT_CLOSED: &str = "vault_closed";
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/egress" => show(who, params),
        "/cli/app/egress/log" => log(who, params),
        "/cli/app/egress/add" => add(who, body),
        "/cli/app/egress/rm" => rm(who, body),
        "/cli/app/egress/mode" => set_mode(who, body),
        "/cli/app/approvals" => approvals(who, params),
        "/cli/app/approve" => approve(who, body),
        "/cli/app/deny" => deny(who, body),
        _ => Err(no("not_found", 404, "no such App egress route")),
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(db_err)
}

fn audit(app: &str, event: &str, who_label: &str, payload: J) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app, event, who_label, None, &payload) {
        k2_core::log_debug!("[apps] audit {app} {event}: {e}");
    }
}

fn changed(app: &str) {
    super::egress_proxy::reload(app);
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "apps": "egress", "app": app }));
}

/// The confirm stamp for an owner (the vault actor the token was minted for).
fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: Role::Owner } => Some(format!("connect:{username}")),
        _ => None,
    }
}

fn is_admin_human(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

fn session_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { session_id, .. } | Who::Shell { session_id, .. } => Some(session_id.as_str()),
        _ => None,
    }
}

fn workspace_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    }
}

/// Specs from `hosts` (array) or `host` (one), each checked: parseable,
/// never this box's own names.
fn specs_from(v: &J) -> R<Vec<HostSpec>> {
    let mut raw: Vec<String> = match v.get("hosts") {
        Some(J::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    if let Some(h) = s(v, "host") {
        raw.push(h.to_string());
    }
    if raw.is_empty() {
        return Err(no(dcodes::BAD_REQUEST, 400, "name a host (host[:port], 443 by default)"));
    }
    let specs = pol::parse_specs(&raw).map_err(|e| no(codes::EGRESS_HOST_INVALID, 400, e))?;
    for sp in &specs {
        if !sp.is_ip() && super::egress_proxy::is_own_name(&sp.host) {
            return Err(no(
                codes::EGRESS_SELF,
                400,
                format!("{sp} is this box: an App reaches K2 through its gateway, never through the egress proxy"),
            ));
        }
        if let Ok(ip) = sp.host.parse::<std::net::IpAddr>() {
            if k2_vault::hosts::is_blocked_ip(k2_vault::hosts::canonical_ip(ip)) {
                return Err(no(
                    codes::EGRESS_HOST_INVALID,
                    400,
                    format!("{sp} is a private, local or metadata address; Apps never reach those"),
                ));
            }
        }
    }
    Ok(specs)
}

/// Is the App's egress enforced by the proxy (a walled engine)?
fn enforced(app: &deploy::store::AppRow) -> bool {
    app.isolation == "container" || app.isolation == "vm"
}

fn request_json(w: &pol::Widening) -> J {
    let mut v = serde_json::to_value(w).unwrap_or(J::Null);
    v["hosts"] = json!(w.hosts().iter().map(HostSpec::display).collect::<Vec<_>>());
    v
}

fn held_answer(w: &pol::Widening, already: &[HostSpec]) -> J {
    json!({
        "ok": true,
        "status": codes::HELD,
        "request": request_json(w),
        "alreadyAllowed": already.iter().map(HostSpec::display).collect::<Vec<_>>(),
        "hint": format!(
            "held for the owner: they approve it in Settings → Apps → Approvals, or k2 app approve {} from a terminal outside K2. \
             Until then the App can't reach it.",
            w.id
        ),
    })
}

// ── read ─────────────────────────────────────────────────────────────

/// GET `/cli/app/egress?app=` → mode, host list, pending requests, proxy.
fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let (mode, hosts, pending) = db(|c| {
        let _ = pol::expire_due(c, deploy::now());
        Ok((
            pol::mode(c, &app.app_id)?,
            pol::list_hosts(c, &app.app_id)?,
            pol::list_requests(c, Some(&app.app_id), Some(pol::REQ_PENDING), None, 50)?,
        ))
    })?;
    let enf = enforced(&app);
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "mode": mode,
        "enforced": enf,
        "note": if enf {
            "Guarded: the App reaches only the allowed hosts, through its own proxy (DNS in the proxy). Private, local and metadata addresses and this box's own names are always refused."
        } else {
            "This App runs on the process engine (preview, unwalled): it has the daemon's network and the list is not enforced. Deploy with --engine guarded to enforce it."
        },
        "hosts": hosts,
        "pending": pending.iter().map(request_json).collect::<Vec<_>>(),
        "proxy": super::egress_proxy::status_json(&app.app_id),
    }))
}

/// GET `/cli/app/egress/log?app=&tail=` → the last connections (host and
/// port only; hosts are validated names, never raw App text).
fn log(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let n = params.get("tail").and_then(|t| t.parse::<usize>().ok()).unwrap_or(50);
    Ok(json!({ "ok": true, "app": app.app_id, "lines": super::egress_proxy::tail(&app.app_id, n) }))
}

/// GET `/cli/app/approvals?app=&state=pending|all|<state>`.
fn approvals(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = match params.get("app").map(|a| a.trim()).filter(|a| !a.is_empty()) {
        Some(a) => Some(app_for(who, Some(a))?.app_id),
        None => None,
    };
    let state = match params.get("state").map(String::as_str).unwrap_or("pending") {
        "all" => None,
        s @ ("pending" | "approved" | "denied" | "withdrawn" | "expired") => Some(s.to_string()),
        other => return Err(no(dcodes::BAD_REQUEST, 400, format!("state is pending, approved, denied, withdrawn, expired or all, not '{other}'"))),
    };
    let ws = agent_ws(who).map(str::to_string);
    let rows = db(|c| {
        for w in pol::expire_due(c, deploy::now())? {
            notify_requester(&w, &format!("[k2 apps] request {} expired after 7 days without an answer: {}", w.id, w.summary));
        }
        pol::list_requests(c, app.as_deref(), state.as_deref(), ws.as_deref(), 200)
    })?;
    Ok(json!({ "ok": true, "requests": rows.iter().map(request_json).collect::<Vec<_>>() }))
}

// ── propose ──────────────────────────────────────────────────────────

/// POST `/cli/app/egress/add {app, host|hosts, reason?}` → held (or
/// "already allowed"). Never live without the owner's approval.
fn add(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let specs = specs_from(&v)?;
    let by = actor(who);
    let ask = pol::Ask {
        source: pol::SOURCE_REQUEST,
        reason: s(&v, "reason"),
        deployment_id: None,
        by: &by,
        workspace: workspace_of(who),
        session: session_of(who),
    };
    let (w, already) = db(|c| Ok(pol::request_hosts(c, &app.app_id, &specs, &ask, deploy::now())))?.map_err(|e| no(codes::EGRESS_HOST_INVALID, 409, e))?;
    let Some(w) = w else {
        return Ok(json!({
            "ok": true,
            "status": "allowed",
            "alreadyAllowed": already.iter().map(HostSpec::display).collect::<Vec<_>>(),
            "hint": "already on the App's allowlist",
        }));
    };
    audit(&app.app_id, "egress_requested", &by, json!({ "request": w.id, "hosts": w.hosts().iter().map(HostSpec::display).collect::<Vec<_>>(), "reason": w.reason }));
    changed(&app.app_id);
    Ok(held_answer(&w, &already))
}

/// File the new hosts a Seed's manifest asks for, at deploy (the deploy
/// itself goes on: the App just can't reach them until approved). The
/// S20 deploy hold builds on this request.
pub fn request_manifest_hosts(who: &Who, app: &str, seed_hash: &str, deployment: &str) -> Option<J> {
    let text = std::fs::read_to_string(deploy::seed::seed_path(seed_hash).join(deploy::manifest::MANIFEST_FILE)).ok()?;
    let m = deploy::manifest::parse(&text).ok()?;
    let raw = m.egress.map(|e| e.hosts).unwrap_or_default();
    if raw.is_empty() {
        return None;
    }
    let specs: Vec<HostSpec> = pol::parse_specs(&raw).ok()?.into_iter().filter(|s| s.is_ip() || !super::egress_proxy::is_own_name(&s.host)).collect();
    let by = actor(who);
    let ask = pol::Ask {
        source: pol::SOURCE_MANIFEST,
        reason: Some("the Seed's manifest asks for it"),
        deployment_id: Some(deployment),
        by: &by,
        workspace: workspace_of(who),
        session: session_of(who),
    };
    let r = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        pol::request_hosts(&conn, app, &specs, &ask, deploy::now())
    };
    match r {
        Ok((Some(w), already)) => {
            audit(app, "egress_requested", &by, json!({ "request": w.id, "hosts": w.hosts().iter().map(HostSpec::display).collect::<Vec<_>>(), "deployment": deployment }));
            changed(app);
            Some(held_answer(&w, &already))
        }
        Ok((None, already)) => Some(json!({ "status": "allowed", "alreadyAllowed": already.iter().map(HostSpec::display).collect::<Vec<_>>() })),
        Err(e) => Some(json!({ "status": "error", "hint": e })),
    }
}

// ── narrow ───────────────────────────────────────────────────────────

/// POST `/cli/app/egress/rm {app, host}`: off the list at once (allowed
/// or pending); open tunnels to it are cut.
fn rm(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let raw = s(&v, "host").ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "name the host to remove"))?;
    let spec = pol::parse_spec(raw).map_err(|e| no(codes::EGRESS_HOST_INVALID, 400, e))?;
    let row = db(|c| pol::get_host(c, &app.app_id, &spec))?;
    let removed = db(|c| pol::remove(c, &app.app_id, &spec))?;
    if !removed {
        return Ok(json!({ "ok": true, "removed": false, "hint": format!("{spec} isn't on App {}'s list", app.app_id) }));
    }
    // A pending request that now asks for nothing more is withdrawn.
    if let Some(req) = row.as_ref().and_then(|r| r.request_id.clone()) {
        let left = db(|c| pol::list_hosts(c, &app.app_id))?.into_iter().any(|h| h.request_id.as_deref() == Some(req.as_str()));
        if !left {
            let _ = db(|c| Ok(pol::decide(c, &req, pol::REQ_WITHDRAWN, &actor(who), None, Some("its hosts were removed"), deploy::now())));
        }
    }
    audit(&app.app_id, "egress_removed", &actor(who), json!({ "host": spec.display(), "was": row.map(|r| r.status) }));
    changed(&app.app_id);
    Ok(json!({ "ok": true, "removed": true, "host": spec.display() }))
}

/// POST `/cli/app/egress/mode {app, mode}`: `allowlist` at once; `broad`
/// is the open path's (0.46.1) and always a held request.
fn set_mode(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let mode = s(&v, "mode").unwrap_or("");
    let by = actor(who);
    match mode {
        pol::MODE_ALLOWLIST => {
            let was = db(|c| pol::mode(c, &app.app_id))?;
            db(|c| pol::set_mode(c, &app.app_id, pol::MODE_ALLOWLIST, &by))?;
            if was != pol::MODE_ALLOWLIST {
                audit(&app.app_id, "egress_mode", &by, json!({ "mode": pol::MODE_ALLOWLIST, "was": was }));
            }
            changed(&app.app_id);
            Ok(json!({ "ok": true, "mode": pol::MODE_ALLOWLIST }))
        }
        pol::MODE_BROAD => {
            if app.path != "open" {
                return Err(no(
                    codes::OPEN_PATH_ONLY,
                    409,
                    format!(
                        "broad egress is the open path's (it turns the label to Open); App {} is guarded. Ask for the hosts it needs instead: \
                         k2 app egress add {} <host[:port]>",
                        app.app_id, app.app_id
                    ),
                ));
            }
            let w = db(|c| {
                pol::file_request(
                    c,
                    &pol::NewWidening {
                        app_id: &app.app_id,
                        kind: pol::KIND_EGRESS_MODE,
                        diff: json!({ "egressMode": pol::MODE_BROAD }),
                        summary: format!("let App {} reach any public host (broad egress, every first-seen host audited)", app.app_id),
                        reason: s(&v, "reason"),
                        deployment_id: None,
                        requested_by: &by,
                        requester_workspace: workspace_of(who),
                        requester_session: session_of(who),
                    },
                    deploy::now(),
                )
            })?;
            audit(&app.app_id, "egress_mode_requested", &by, json!({ "request": w.id, "mode": pol::MODE_BROAD }));
            Ok(held_answer(&w, &[]))
        }
        other => Err(no(dcodes::BAD_REQUEST, 400, format!("mode is allowlist or broad, not '{other}'"))),
    }
}

// ── decide ───────────────────────────────────────────────────────────

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    #[cfg(test)]
    if let Some(v) = test_seams::vault() {
        return Ok(v);
    }
    crate::vault::get().map_err(|e| no(codes::VAULT_CLOSED, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

fn find(token: Option<&str>) -> R<pol::Widening> {
    let t = token.ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "name the request (its w_… id)"))?;
    db(|c| pol::find_request(c, t))?.ok_or_else(|| no(codes::REQUEST_NOT_FOUND, 404, format!("no request '{t}' (k2 app approvals)")))
}

fn decide_err(code: &'static str) -> Refusal {
    match code {
        "request_expired" => no(codes::REQUEST_EXPIRED, 410, "this request expired (7 days); ask again"),
        "not_pending" => no(codes::NOT_PENDING, 409, "this request was already decided"),
        _ => no(codes::REQUEST_NOT_FOUND, 404, "no such request"),
    }
}

/// POST `/cli/app/approve {request, confirmToken}`: the owner, in person.
fn approve(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let Some(stamp) = confirm_stamp(who) else {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "only the server's owner approves App widenings, in person: Settings → Apps → Approvals, or k2 app approve <id> \
             from a terminal outside K2. An agent can never approve.",
        ));
    };
    let v = json_body(body)?;
    let w = find(s(&v, "request"))?;
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(
            codes::OWNER_CONFIRM_REQUIRED,
            403,
            "approving needs a fresh owner confirm (Touch ID, your login or vault password): k2 app approve <id> asks for it",
        ));
    };
    let vault = vault()?;
    let proof = vault
        .confirm
        .take(token, CONFIRM_ACTION, Some(&w.id), &stamp)
        .map_err(|_| no(codes::OWNER_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this request, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(codes::OWNER_CONFIRM_REQUIRED, 403, "no owner confirm")),
    };
    let by = actor(who);
    let d = db(|c| Ok(pol::decide(c, &w.id, pol::REQ_APPROVED, &by, Some(&via), None, deploy::now())))?.map_err(decide_err)?;
    audit(&d.app_id, "widening_approved", &by, json!({ "request": d.id, "kind": d.kind, "diff": d.diff, "via": via }));
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-approve",
        &by,
        format!("ok {} ({}) via {via}", d.id, d.summary),
        "cli",
        "-",
        "apps",
    ));
    changed(&d.app_id);
    notify_requester(&d, &format!("[k2 apps] the owner approved {}: {}. It applies now.", d.id, d.summary));
    Ok(json!({ "ok": true, "request": request_json(&d), "note": "approved; it applies to the App's next connection" }))
}

/// POST `/cli/app/deny {request, reason?}`: an admin denies; the
/// requester withdraws.
fn deny(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let w = find(s(&v, "request"))?;
    let by = actor(who);
    let reason = s(&v, "reason").map(|r| r.chars().take(500).collect::<String>());
    let mine = match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => w.requester_workspace.as_deref() == Some(project_id.as_str()),
        _ => w.requested_by == by,
    };
    let status = if who.passport().is_none() && is_admin_human(who) && !mine {
        pol::REQ_DENIED
    } else if mine {
        pol::REQ_WITHDRAWN
    } else {
        // Hide requests on other workspaces' Apps from passports.
        if agent_ws(who).is_some() {
            return Err(no(codes::REQUEST_NOT_FOUND, 404, "no such request"));
        }
        return Err(no(codes::OWNER_ONLY, 403, "only the owner or an admin denies a request (the requester withdraws it)"));
    };
    let d = db(|c| Ok(pol::decide(c, &w.id, status, &by, None, reason.as_deref(), deploy::now())))?.map_err(decide_err)?;
    let event = if status == pol::REQ_DENIED { "widening_denied" } else { "widening_withdrawn" };
    audit(&d.app_id, event, &by, json!({ "request": d.id, "kind": d.kind, "reason": reason }));
    changed(&d.app_id);
    if status == pol::REQ_DENIED {
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
    Ok(json!({ "ok": true, "request": request_json(&d) }))
}

fn notify_requester(w: &pol::Widening, text: &str) {
    if let Some(ws) = w.requester_workspace.as_deref() {
        super::alerts::notify(ws, text);
    }
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::sync::{Arc, OnceLock};

    static VAULT: OnceLock<(k2_vault::fsutil::TestDir, Arc<k2_vault::LocalVault>)> = OnceLock::new();

    pub fn vault_handle() -> Arc<k2_vault::LocalVault> {
        VAULT
            .get_or_init(|| {
                let dir = k2_vault::fsutil::TestDir::new("app-egress");
                let ks: Arc<dyn k2_vault::KeyStore> = Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
                let v = Arc::new(
                    k2_vault::LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
                        .expect("open test vault"),
                );
                (dir, v)
            })
            .1
            .clone()
    }

    pub(super) fn vault() -> Option<Arc<k2_vault::LocalVault>> {
        Some(vault_handle())
    }
}

#[cfg(test)]
#[path = "egress_routes_tests.rs"]
mod tests;
