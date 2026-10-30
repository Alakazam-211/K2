//! `/cli/publish/visibility`, `/open` and `/sessions`: who may open a published
//! site, and the sign-in a person uses for a private one
//! (prd-publish-visibility-and-dev-mode-v1 PV-5, PV-7, PV-10, PV-12, DM-3).
//!
//! The store is `k2_core::site_visibility`; the gate that enforces it is
//! `site_gate`. These routes are the gestures. The rules:
//! - **Private:** an agent on its own site, or any person (P-D8).
//! - **PIN, public:** the owner or an Owner/Admin login. An agent (or a K2 shell
//!   tab, which carries a passport too) gets K2's card and never handles a PIN.
//! - **Public dev:** a person only, with `devIsPublic` and a reason. An agent
//!   can't even ask (`dev_public_person_only`).
//! - **A label an agent's workspace doesn't hold** answers exactly as an unknown
//!   one: another workspace's site is not its business (PV-14's leak tests).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::db::schema::SubdomainWorkspace;
use k2_core::site_visibility::{self as sv, Mode};
use serde_json::{json, Value as J};

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

fn refusal(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": message.into(), "code": code }).to_string(),
    }
}

fn not_found(label: &str) -> CliResponse {
    refusal(
        "404 Not Found",
        "label_not_found",
        format!("no site {label:?} here (k2 publish ps, k2 publish subdomain list): nothing changed"),
    )
}

/// A `site_visibility` refusal (`<code>: <plain words>`) as an answer.
fn store_refusal(e: String) -> CliResponse {
    let (code, words) = e.split_once(": ").unwrap_or(("site_visibility_failed", e.as_str()));
    let status = match code {
        "site_visibility_db" => "500 Internal Server Error",
        sv::DOOR_UNSUPPORTED => "409 Conflict",
        _ => "400 Bad Request",
    };
    refusal(status, code, words)
}

fn resolve_project_id(project: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM projects WHERE path = ?1 OR id = ?1", rusqlite::params![project], |r| r.get(0)).ok()
}

/// The passport's workspace, for a GET (the POST routes have [`Who`]).
fn passport_workspace(params: &HashMap<String, String>) -> Option<String> {
    if let Some(p) = crate::caller_workspace::request_principal() {
        return Some(p.workspace_uuid);
    }
    if !params.contains_key(crate::caller_workspace::PRINCIPAL_BOUND_KEY) {
        return None;
    }
    let stamped = opt_param(params, "project_id").filter(|p| !p.trim().is_empty());
    // A stamped call with no workspace in it resolves to nothing, not to "no
    // passport": the caller stays scoped.
    Some(stamped.or_else(|| need_project(params).ok().and_then(|p| resolve_project_id(&p))).unwrap_or_default())
}

/// Does this box publish `label`? The routing map, the 0074 stamps and the
/// services.
fn label_known(label: &str) -> bool {
    if k2_core::tunnel::subdomains::current().targets.contains_key(label) {
        return true;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if SubdomainWorkspace::map(&conn).is_ok_and(|m| m.contains_key(label)) {
        return true;
    }
    k2_core::published_services::list_all(&conn).is_ok_and(|rows| rows.iter().any(|r| r.name == label && r.expose == k2_core::published_services::EXPOSE_TUNNEL))
}

/// Does workspace `ws` hold `label` (its service, or its 0074 stamp)?
fn workspace_holds(ws: &str, label: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if SubdomainWorkspace::owner_of(&conn, label).ok().flatten().as_deref() == Some(ws) {
        return true;
    }
    k2_core::published_services::get_by_project_name(&conn, ws, label).ok().flatten().is_some()
}

/// An App's own logins decide for an App (P-D3). A folder App (what a new
/// `k2 publish run --cmd` becomes on a box with the open path) has no logins of
/// its own, so it is a site like any other, and the gate covers it.
pub(crate) fn is_real_app(label: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::deploy::store::get_app(&conn, label) {
        Ok(Some(a)) => !(a.source == "folder" && a.path == "open"),
        _ => false,
    }
}

/// The label as this request may use it: normal form, covered by the table,
/// known here, and (for a passport) its own workspace's.
fn usable_label(raw: &str, passport_ws: Option<&str>) -> Result<String, CliResponse> {
    let label = k2_core::tunnel::client_ip_labels::normalize_label(raw).map_err(|e| refusal("400 Bad Request", "bad_label", e))?;
    if let Some(ws) = passport_ws {
        // Before anything that could tell a foreign label from an unknown one.
        if !workspace_holds(ws, &label) {
            return Err(not_found(&label));
        }
    }
    if !sv::is_covered(&label) {
        return Err(refusal("400 Bad Request", "visibility_not_applicable", format!("'{label}' is one of K2's own front doors: its visibility isn't set here.")));
    }
    if is_real_app(&label) {
        return Err(refusal(
            "400 Bad Request",
            "visibility_not_applicable",
            format!("'{label}' is an App: its own logins decide who gets in (k2 app user list)."),
        ));
    }
    if !label_known(&label) {
        return Err(not_found(&label));
    }
    Ok(label)
}

fn site_url(label: &str) -> Option<String> {
    let public_url = k2_core::tunnel::tunnel_status().public_url;
    crate::publish_routes::nested_public_url(label, &crate::publish_runtime::primary_host(), public_url.as_deref())
}

/// What a site listens on beyond loopback, right now (PV-4, PA-5): its service
/// when it has one, else whatever listens on the port its hostname points at.
fn exposed_now(label: &str) -> Vec<String> {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::list_all(&conn).ok().and_then(|rows| rows.into_iter().find(|r| r.name == label))
    };
    let (pid, port) = match &row {
        Some(r) => (r.pid.and_then(|p| i32::try_from(p).ok()), Some(r.port as u16)),
        None => {
            let target = k2_core::tunnel::subdomains::current().targets.get(label).cloned().unwrap_or_default();
            (None, target.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()))
        }
    };
    let Some(port) = port else { return Vec::new() };
    crate::publish_dev::listeners(pid, port).map(|l| crate::publish_dev::exposed_addrs(&l)).unwrap_or_default()
}

/// A site's visibility on the wire.
fn view(v: &sv::SiteVisibility, warnings: Vec<String>) -> J {
    let door = if v.mode == Mode::Public { None } else { crate::site_gate::door_refusal(&v.label) };
    let mut warnings = warnings;
    if v.mode != Mode::Public {
        let exposed = exposed_now(&v.label);
        if !exposed.is_empty() {
            warnings.push(crate::publish_dev::bind_warning(&v.label, &exposed));
        }
        if let Some(d) = &door {
            warnings.push(d.split_once(": ").map(|(_, w)| w).unwrap_or(d).to_string());
        }
    }
    json!({
        "label": v.label,
        "mode": v.mode.as_str(),
        "dev": v.dev,
        "pinSet": v.pin_set,
        "pinDays": v.pin_days,
        "idleSecs": v.idle_secs,
        "reason": v.public_reason,
        "setBy": v.set_by,
        "setAt": v.set_at,
        "explicit": v.explicit,
        "enforced": door.is_none(),
        "url": site_url(&v.label),
        "warnings": warnings,
    })
}

/// `GET /cli/publish/visibility?label=`.
pub(crate) fn handle_show(params: &HashMap<String, String>) -> CliResponse {
    let raw = str_param(params, "label");
    if raw.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    let ws = passport_workspace(params);
    let label = match usable_label(&raw, ws.as_deref()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let got = {
        let conn = db.lock();
        sv::effective(&conn, &label)
    };
    match got {
        Ok(v) => CliResponse::ok_json(view(&v, Vec::new()).to_string()),
        Err(e) => store_refusal(e),
    }
}

fn truthy(params: &HashMap<String, String>, key: &str) -> bool {
    params.get(key).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "on" | "yes"))
}

/// A person who may set a PIN or make a site public.
fn is_manager(who: &Who) -> bool {
    who.is_admin()
}

/// A passport: an agent or a K2 shell tab.
fn passport_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    }
}

const PARSER_WARNING: &str = "is public today, and K2 passes its bytes straight through. Private or PIN means K2 reads it as HTTP/1.1, which breaks a raw-TCP or HTTP/2-only service (one of those should stay public).";

fn audit(event: &str, who: &Who, label: &str, detail: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        &who.label(),
        format!("kind=site label={label} {detail}"),
        "-",
        "-",
        "cli",
    ));
}

/// `POST /cli/publish/visibility` `{label, mode, reason?, pin?, pinDays?, yes?,
/// devIsPublic?, check?}`. `check`: answer who may and what it needs, change
/// nothing and take no PIN (the CLI asks before anyone types one).
pub(crate) fn handle_set(params: &HashMap<String, String>, who: &Who) -> CliResponse {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return refusal("403 Forbidden", "person_only", "this credential can't change who may open a site");
    }
    let raw = str_param(params, "label");
    if raw.trim().is_empty() {
        return refusal("400 Bad Request", "usage", "Missing label");
    }
    let Some(mode) = Mode::parse(str_param(params, "mode").trim()) else {
        return refusal("400 Bad Request", "usage", "the mode is private, pin or public");
    };
    let passport = passport_of(who);
    let label = match usable_label(&raw, passport) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let current = {
        let conn = db.lock();
        match sv::effective(&conn, &label) {
            Ok(v) => v,
            Err(e) => return store_refusal(e),
        }
    };
    let reason = opt_param(params, "reason").map(|r| r.trim().to_string()).filter(|r| !r.is_empty());

    // Who may. A passport's PIN never reaches anything: it isn't read.
    match mode {
        Mode::Private => {}
        Mode::Pin => {
            if let Some(ws) = passport {
                return crate::publish_cards::present_pin(ws, &label);
            }
            if !is_manager(who) {
                return refusal("403 Forbidden", "owner_or_admin", "only the owner or an admin can set a site's PIN");
            }
        }
        Mode::Public => {
            if passport.is_some() && current.dev {
                return refusal(
                    "403 Forbidden",
                    "dev_public_person_only",
                    format!("'{label}' is a dev server: only the owner or an admin can make it public, from a terminal outside K2. For a friend, ask for a PIN instead (publish.pin)."),
                );
            }
            if let Some(ws) = passport {
                let Some(why) = reason.as_deref() else {
                    return refusal("400 Bad Request", "reason_required", "say why this site should be public (--reason)");
                };
                return crate::publish_cards::present_public(ws, &label, why);
            }
            if !is_manager(who) {
                return refusal("403 Forbidden", "owner_or_admin", "only the owner or an admin can make a site public");
            }
            if reason.is_none() {
                return refusal("400 Bad Request", "reason_required", "say why this site is public (--reason)");
            }
            if current.dev && !truthy(params, "devIsPublic") {
                return refusal(
                    "400 Bad Request",
                    "dev_public_needs_flag",
                    format!("'{label}' is a dev server. {} To make it public anyway: --dev-is-public.", crate::publish_dev::DEV_WARNING),
                );
            }
        }
    }

    // PV-4 / PA-3: a door the gate doesn't cover can't promise this.
    if mode != Mode::Public {
        if let Some(e) = crate::site_gate::door_refusal(&label) {
            return store_refusal(e);
        }
    }
    // PV-3: moving a public site behind the gate changes how it is carried.
    let parsed_already = k2_core::tunnel::client_ip_labels::load().contains(&label);
    if mode != Mode::Public && current.mode == Mode::Public && !current.dev && !parsed_already && !truthy(params, "yes") {
        return refusal("409 Conflict", "needs_yes", format!("'{label}' {PARSER_WARNING} Pass --yes to go ahead."));
    }

    let pin_days = match opt_param(params, "pinDays").map(|d| d.trim().parse::<i64>()) {
        None => None,
        Some(Ok(d)) if (sv::PIN_DAYS_MIN..=sv::PIN_DAYS_MAX).contains(&d) => Some(d),
        Some(_) => return refusal("400 Bad Request", "usage", "--pin-days is a number from 1 to 30"),
    };
    if truthy(params, "check") {
        return CliResponse::ok_json(json!({ "check": true, "label": label, "mode": mode.as_str() }).to_string());
    }

    let by = who.label();
    let mut warnings = Vec::new();
    let done = match mode {
        Mode::Private => {
            let conn = db.lock();
            sv::set_private(&conn, &label, &by)
        }
        Mode::Public => {
            if current.dev {
                warnings.push(crate::publish_dev::DEV_WARNING.to_string());
            }
            let conn = db.lock();
            sv::set_public(&conn, &label, reason.as_deref().unwrap_or_default(), &by)
        }
        Mode::Pin => {
            let Some(pin) = params.get("pin").map(String::as_str).filter(|p| !p.is_empty()) else {
                return refusal("400 Bad Request", "pin_required", "no PIN came with this: give it on stdin, or type it when asked");
            };
            // Slow on purpose: never under the database lock.
            let hash = match sv::hash_pin(pin) {
                Ok(h) => h,
                Err(e) => return store_refusal(e),
            };
            if hash.short {
                warnings.push(format!("that PIN is under {} characters: anyone who gets a few tries has a better chance. Use a longer one for anything that matters.", sv::PIN_WARN_BELOW));
            }
            if current.dev {
                warnings.push(format!("{} That includes the person with the PIN.", crate::publish_dev::DEV_WARNING));
            }
            let conn = db.lock();
            sv::set_pin(&conn, &label, &hash, pin_days, &by)
        }
    };
    match done {
        Ok(v) => {
            let detail = match (&mode, &reason) {
                (Mode::Public, Some(r)) => format!("mode=public reason={r:?}"),
                _ => format!("mode={}", mode.as_str()),
            };
            audit("site-visibility", who, &label, &detail);
            crate::session_events::emit_tunnel_subdomains_changed();
            crate::publish_runtime::emit_label_changed(&label);
            CliResponse::ok_json(view(&v, warnings).to_string())
        }
        Err(e) => store_refusal(e),
    }
}

/// The label and path a `url` names, when it is one of this server's sites.
fn label_of_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().strip_prefix("https://")?;
    let (host, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = host.to_ascii_lowercase();
    let primary = crate::publish_runtime::primary_host();
    if primary.is_empty() {
        return None;
    }
    let label = host.strip_suffix(&format!(".{primary}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST))?;
    if label.is_empty() || label.contains('.') {
        return None;
    }
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    Some((label.to_string(), path))
}

/// `POST /cli/publish/open` `{label, path?}` or `{url}` (PV-5, PA-2): a one-time
/// link for a signed-in person. `{url}` is the no-step swap in a K2 tab: for
/// anything that isn't one of this server's private or PIN sites it answers
/// `{swap: false}`, and the client just navigates.
pub(crate) fn handle_open(params: &HashMap<String, String>, who: &Who) -> CliResponse {
    if matches!(who, Who::Agent { .. } | Who::Shell { .. }) {
        return refusal(
            "403 Forbidden",
            "person_only",
            "an agent doesn't open a private site through its address: test it on the local port (http://127.0.0.1:<port>)",
        );
    }
    if !is_manager(who) {
        return refusal("403 Forbidden", "owner_or_admin", "only the owner or an admin can open a private site");
    }
    let swap = opt_param(params, "url");
    let (raw, path) = if let Some(url) = &swap {
        match label_of_url(url) {
            Some(v) => v,
            None => return CliResponse::ok_json(json!({ "swap": false }).to_string()),
        }
    } else {
        (str_param(params, "label"), opt_param(params, "path").unwrap_or_default())
    };
    if raw.trim().is_empty() {
        return refusal("400 Bad Request", "usage", "Missing label");
    }
    let label = match usable_label(&raw, None) {
        Ok(l) => l,
        Err(_) if swap.is_some() => return CliResponse::ok_json(json!({ "swap": false }).to_string()),
        Err(r) => return r,
    };
    let Some(path) = sv::safe_path(&path) else {
        return refusal("400 Bad Request", "bad_path", "--path must start with / and stay on the site");
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mode = match sv::effective_mode(&conn, &label) {
        Ok(m) => m,
        Err(e) => return store_refusal(e),
    };
    if mode == Mode::Public {
        if swap.is_some() {
            return CliResponse::ok_json(json!({ "swap": false }).to_string());
        }
        let url = site_url(&label).map(|u| format!("{u}{}", if path == "/" { "" } else { path.as_str() }));
        return CliResponse::ok_json(json!({ "label": label, "mode": "public", "url": url, "oneTime": false }).to_string());
    }
    let user = match who {
        Who::Login { username, .. } => username.clone(),
        _ => sv::USER_OWNER_TOKEN.to_string(),
    };
    let code = match sv::mint_open_code(&conn, &label, &path, &user) {
        Ok(c) => c,
        Err(e) => return store_refusal(e),
    };
    drop(conn);
    let Some(url) = sv::open_url(&label, &code.code) else {
        return refusal("409 Conflict", "no_public_address", "this server has no public address yet (k2 publish status)");
    };
    audit("site-open-link", who, &label, "minted");
    CliResponse::ok_json(
        json!({
            "label": label,
            "mode": mode.as_str(),
            "url": url,
            "oneTime": true,
            "swap": true,
            "expiresAt": code.expires_at,
            "expiresInSecs": sv::OPEN_CODE_SECS,
        })
        .to_string(),
    )
}

/// `GET /cli/publish/sessions?label=`.
pub(crate) fn handle_sessions(params: &HashMap<String, String>) -> CliResponse {
    let raw = str_param(params, "label");
    if raw.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    let ws = passport_workspace(params);
    let label = match usable_label(&raw, ws.as_deref()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match sv::list_sessions(&conn, &label) {
        Ok(rows) => CliResponse::ok_json(json!({ "label": label, "sessions": rows }).to_string()),
        Err(e) => store_refusal(e),
    }
}

/// `POST /cli/publish/sessions/revoke` `{label}`: end every session. It only
/// tightens, so an agent may on its own site.
pub(crate) fn handle_revoke(params: &HashMap<String, String>, who: &Who) -> CliResponse {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return refusal("403 Forbidden", "person_only", "this credential can't end a site's sessions");
    }
    let raw = str_param(params, "label");
    if raw.trim().is_empty() {
        return refusal("400 Bad Request", "usage", "Missing label");
    }
    let label = match usable_label(&raw, passport_of(who)) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match sv::revoke_sessions(&conn, &label) {
        Ok(n) => {
            drop(conn);
            audit("site-sessions-revoked", who, &label, &format!("count={n}"));
            CliResponse::ok_json(json!({ "label": label, "revoked": n }).to_string())
        }
        Err(e) => store_refusal(e),
    }
}

/// The caller the routes need, from a card's answerer.
pub(crate) fn who_of(answerer: &crate::permission::actions::Answerer) -> Who {
    use crate::permission::actions::Answerer;
    match answerer {
        Answerer::OwnerToken => Who::Owner,
        Answerer::Login { user, role } => Who::Login {
            username: user.clone(),
            role: match role.as_str() {
                "owner" => Role::Owner,
                "admin" => Role::Admin,
                _ => Role::Member,
            },
        },
    }
}

#[cfg(test)]
#[path = "publish_site_routes_tests.rs"]
mod tests;
