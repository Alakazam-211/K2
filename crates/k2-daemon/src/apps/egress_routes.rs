//! `/cli/app/egress*` (`prd-k2-app-runtime-v1` §5.6, §8.1, S15).
//! `k2 app egress …`. Its held widenings are ordinary held App requests:
//! `k2 app approvals|approve|deny` (`super::approvals`, one set of routes
//! for every kind).
//!
//! Who may do what:
//! - **Read** (`egress`, `egress/log`): anyone who sees the App. A
//!   passport sees only its own workspace's Apps.
//! - **Propose** (`egress/add`, `egress/mode broad`): the same. A held
//!   request, the owner token included: a new egress host is never live
//!   until the owner approves it with a fresh owner confirm — except that
//!   an agent with "…and manage them end to end" (switch 2,
//!   prd-agent-app-permissions-v1 §2.1) adds hosts to its own Apps itself.
//!   Broad mode is the open path: held for the person from every caller.
//! - **Narrow** (`egress/rm`, `egress/mode allowlist`): the same, at once,
//!   no switch needed (narrowing never waits). Open tunnels to a removed
//!   host are cut.
//! - **Approve / deny**: `super::approvals` (the owner in person with a
//!   confirm bound to the request; an admin denies; the requester
//!   withdraws its own).

use std::collections::HashMap;

use k2_core::deploy::egress::{self as pol, HostSpec};
use k2_core::deploy::{self, codes as dcodes};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/egress", "/cli/app/egress/log", "/cli/app/egress/add", "/cli/app/egress/rm", "/cli/app/egress/mode"];

pub const POST_ROUTES: &[&str] = &["/cli/app/egress/add", "/cli/app/egress/rm", "/cli/app/egress/mode"];

/// The vault confirm action for approving any held App request.
pub use super::approvals::CONFIRM_ACTION;

pub mod codes {
    pub const HELD: &str = "held";
    pub const OWNER_ONLY: &str = "owner_only";
    /// The house code for "this needs a fresh vault confirm".
    pub const VAULT_CONFIRM_REQUIRED: &str = "vault_confirm_required";
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
        // The held-request routes are served with the grace routes; the
        // same handlers answer here (tests call this module directly).
        p if super::approvals::ROUTES.contains(&p) => super::approvals::handle(p, who, body, params),
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

/// Specs from `hosts` (array) or `host` (one), each checked.
fn specs_from(v: &J) -> R<Vec<HostSpec>> {
    let mut raw: Vec<String> = match v.get("hosts") {
        Some(J::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    if let Some(h) = s(v, "host") {
        raw.push(h.to_string());
    }
    check_specs(&raw)
}

/// Hosts an App may ask for: parseable, never this box's own names, never
/// a private, local or metadata address. Shared with `/cli/app/config`.
pub(crate) fn check_specs(raw: &[String]) -> R<Vec<HostSpec>> {
    if raw.is_empty() {
        return Err(no(dcodes::BAD_REQUEST, 400, "name a host (host[:port], 443 by default)"));
    }
    let specs = pol::parse_specs(raw).map_err(|e| no(codes::EGRESS_HOST_INVALID, 400, e))?;
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
    super::approvals::request_json(w)
}

/// The held answer (`202`, the one shape every held App request has),
/// plus the hosts that were already allowed.
fn held_answer(w: &pol::Widening, already: &[HostSpec]) -> J {
    let mut v = super::approvals::held_answer(w);
    v["alreadyAllowed"] = json!(already.iter().map(HostSpec::display).collect::<Vec<_>>());
    v["hint"] = json!(format!(
        "held for the owner: they approve it in person (k2 app approve {} from a terminal outside K2, or Settings → Apps → \
         Approvals). Until then the App can't reach it.",
        w.id
    ));
    v
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
            "This App runs on the unwalled process engine (the owner's dev opt-in): it has the daemon's network and the list is not enforced. Deploy with --engine guarded to enforce it."
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

// ── propose ──────────────────────────────────────────────────────────

/// POST `/cli/app/egress/add {app, host|hosts, reason?}` → held (or
/// "already allowed"). Never live without the owner's approval.
fn add(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let specs = specs_from(&v)?;
    ask_specs(who, &app, &specs, s(&v, "reason"), pol::SOURCE_REQUEST, None)
}

/// Ask for hosts on `app` (`/cli/app/egress/add`, `/cli/app/config`
/// `egressAdd`): a held `egress` request, or "already allowed".
pub(crate) fn ask_hosts(who: &Who, app: &deploy::store::AppRow, raw: &[String], reason: Option<&str>, deployment: Option<&str>) -> R<J> {
    let specs = check_specs(raw)?;
    ask_specs(who, app, &specs, reason, pol::SOURCE_REQUEST, deployment)
}

fn ask_specs(who: &Who, app: &deploy::store::AppRow, specs: &[HostSpec], reason: Option<&str>, source: &str, deployment: Option<&str>) -> R<J> {
    let by = actor(who);
    let ask = pol::Ask { source, reason, deployment_id: deployment, by: &by, workspace: workspace_of(who), session: session_of(who) };
    let (w, already) = db(|c| Ok(pol::request_hosts(c, &app.app_id, specs, &ask, deploy::now())))?.map_err(|e| no(codes::EGRESS_HOST_INVALID, 409, e))?;
    let Some(w) = w else {
        return Ok(json!({
            "ok": true,
            "status": "allowed",
            "alreadyAllowed": already.iter().map(HostSpec::display).collect::<Vec<_>>(),
            "hint": "already on the App's allowlist",
        }));
    };
    audit(&app.app_id, "egress_requested", &by, json!({ "request": w.id, "hosts": w.hosts().iter().map(HostSpec::display).collect::<Vec<_>>(), "reason": w.reason }));
    if super::access::self_widens(who) {
        return self_applied(&w, who, &already);
    }
    changed(&app.app_id);
    Ok(held_answer(&w, &already))
}

/// An agent with "…and manage them end to end" adds the hosts itself
/// (prd-agent-app-permissions-v1 §2.1): the request it just filed is
/// applied at once, audited, never queued.
fn self_applied(w: &pol::Widening, who: &Who, already: &[HostSpec]) -> R<J> {
    let out = super::access::self_apply(w, who)?;
    Ok(json!({
        "ok": out["ok"],
        "status": if out["status"] == "approved" { "allowed" } else { "failed" },
        "request": out["request"],
        "added": w.hosts().iter().map(HostSpec::display).collect::<Vec<_>>(),
        "alreadyAllowed": already.iter().map(HostSpec::display).collect::<Vec<_>>(),
        "result": out["result"],
        "hint": "added under \"…and manage them end to end\": it applies to the App's next connection",
    }))
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
            if super::access::self_widens(who) {
                return Some(self_applied(&w, who, &already).unwrap_or_else(|e| json!({ "status": "error", "code": e.code, "hint": e.hint })));
            }
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
    remove_host(who, &app, raw)
}

/// Take one host off an App's list at once (`/cli/app/egress/rm`,
/// `/cli/app/config` `egressRm`).
pub(crate) fn remove_host(who: &Who, app: &deploy::store::AppRow, raw: &str) -> R<J> {
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
    change_mode(who, &app, s(&v, "mode").unwrap_or(""), s(&v, "reason"))
}

/// Set an App's egress mode (`/cli/app/egress/mode`, `/cli/app/config`
/// `egressMode`).
pub(crate) fn change_mode(who: &Who, app: &deploy::store::AppRow, mode: &str, reason: Option<&str>) -> R<J> {
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
                        reason,
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

    pub(crate) fn vault() -> Option<Arc<k2_vault::LocalVault>> {
        Some(vault_handle())
    }
}

#[cfg(test)]
#[path = "egress_routes_tests.rs"]
mod tests;
