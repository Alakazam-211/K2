//! `/cli/domains` + `/cli/certs` handlers (prd-custom-domains A2/A14).
//!
//! GET `/cli/domains` is the list (200). POST apex attach/remove are
//! owner/admin (dispatcher). POST names add/remove is dns_manage.
//! GET of mutating paths → 405.

use std::collections::HashMap;

use k2_core::domains::{
    apex_attached_for_write, get_binding, get_binding_by_zone_id, get_name, hostname_under_apex,
    list_bindings, list_names_for_apex, normalize_apex, normalize_hostname, normalize_role,
    remove_binding, remove_name, upsert_binding, upsert_binding_zone, upsert_name, DomainBinding,
    DomainName,
};

use crate::cli_response::CliResponse;
use crate::domains::bind::{bind_apex, unbind_apex, BindOutcome};

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn merge_json_params(params: &mut HashMap<String, String>, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return;
    };
    let Some(obj) = v.as_object() else {
        return;
    };
    const IDENTITY_KEYS: &[&str] = &[
        "project",
        "project_path",
        "project_id",
        "from",
        "principal_bound",
        "token",
    ];
    for (k, val) in obj {
        if IDENTITY_KEYS.iter().any(|ik| *ik == k.as_str()) {
            continue;
        }
        let s = match val {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => {
                if *b {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            serde_json::Value::Null => continue,
            other => other.to_string(),
        };
        if !s.is_empty() {
            params.insert(k.clone(), s);
        }
    }
}

fn params_from_post(body: &[u8]) -> HashMap<String, String> {
    let mut params = HashMap::new();
    merge_json_params(&mut params, body);
    params
}

fn body_str<'a>(params: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(v) = params.get(*k).map(|s| s.trim()).filter(|s| !s.is_empty()) {
            return Some(v);
        }
    }
    None
}

/// Agent GET `/cli/domains` needs a workspace so `dns_manage` can run.
/// Owner GET is host-wide (D4 / A12) — no project=.
fn gate_agent_list() -> Result<(), CliResponse> {
    let Some(p) = crate::caller_workspace::request_principal() else {
        return Ok(());
    };
    let ws = crate::caller_workspace::resolve_caller_workspace(Some(&p), None).map_err(|e| {
        error_response("403 Forbidden", "forbidden", &e.hint())
    })?;
    if !k2_core::workspace::settings::dns_manage_allowed_for_path(&ws.path) {
        return Err(error_response(
            "403 Forbidden",
            "dns_manage_disabled",
            crate::dns::DNS_DENIED_HINT,
        ));
    }
    Ok(())
}

fn name_json(n: &DomainName) -> serde_json::Value {
    serde_json::json!({
        "hostname": n.hostname,
        "role": n.role,
        "cert": crate::domains::status::cert_json_for(&n.hostname, &n.role),
        "renewal": crate::domains::renew::name_json(&n.hostname),
    })
}

fn domain_json(conn: &rusqlite::Connection, b: &DomainBinding) -> serde_json::Value {
    let names = list_names_for_apex(conn, &b.apex).unwrap_or_default();
    serde_json::json!({
        "apex": b.apex,
        "zoneId": b.zone_id,
        "dnsWrite": b.dns_write,
        // A8.1: `active` | `pending_ns` from k2.dev; null for BYO rows.
        "status": b.status,
        "pendingNs": b.is_pending_ns(),
        "nameservers": b.nameservers,
        // k2.dev auto-added the zone to this account on bind.
        "created": b.auto_created,
        // `winddown` / `suspended` on k2.dev: records serve, no writes.
        "readOnly": b.is_readonly(),
        // DN12d: the zone id was not in k2.dev's last zones list for this
        // server (row kept). In memory only; false for BYO rows.
        "zoneMissing": b.zone_id.is_some() && crate::domains::pending_watch::zone_missing(&b.apex),
        // Last time this box read k2.dev's zones list (P1/P2), unix secs.
        "checkedAt": b
            .zone_id
            .as_ref()
            .and_then(|_| crate::domains::pending_watch::last_check()),
        // The box's last failed re-check of a pending zone, if any since
        // the last good one (e.g. k2.dev unreachable).
        "checkError": b
            .is_pending_ns()
            .then(crate::domains::pending_watch::last_error)
            .flatten(),
        "names": names.iter().map(name_json).collect::<Vec<_>>(),
    })
}

/// Teaching text for a `pending_ns` zone (CLI + record-write refusal).
pub fn pending_ns_hint(b: &DomainBinding) -> String {
    let ns = if b.nameservers.is_empty() {
        "the k2.dev nameservers shown in your k2.dev dashboard".to_string()
    } else {
        b.nameservers.join(", ")
    };
    format!(
        "Pending: point nameservers to {ns} at the registrar for {apex}. This server re-checks \
with k2.dev by itself and opens DNS record writes once the zone is active (to check now: \
Settings → K2 Server → Domains → Check again, or `k2 domain refresh {apex}`).",
        apex = b.apex
    )
}

/// Teaching text for a `winddown` / `suspended` zone (DN12c).
pub fn zone_readonly_hint(b: &DomainBinding) -> String {
    let state = b.status.as_deref().unwrap_or("read-only");
    format!(
        "{apex} is {state} on k2.dev, so its DNS records are read-only. The owner can check the \
plan in the k2.dev dashboard, then `k2 domain refresh {apex}`.",
        apex = b.apex
    )
}

/// Map a failed bind/unbind to the CLI response. `None` = proceed.
///
/// API-missing (405 / empty or HTML 404 / any HTML) is **503**
/// `bind_api_unavailable`, never 502 — the daemon is fine, k2.dev has not
/// shipped the route.
fn bind_error_response(outcome: &BindOutcome, failed_code: &str) -> Option<CliResponse> {
    match outcome {
        BindOutcome::Bound(_) | BindOutcome::NotOwned | BindOutcome::NoTunnel => None,
        BindOutcome::OwnedElsewhere { hint } => Some(error_response(
            "409 Conflict",
            "zone_owned_elsewhere",
            hint,
        )),
        BindOutcome::ApiMissing { hint } => Some(error_response(
            "503 Service Unavailable",
            "bind_api_unavailable",
            hint,
        )),
        BindOutcome::Failed { status, hint } => {
            let http = match status {
                401 => "401 Unauthorized",
                403 => "403 Forbidden",
                409 => "409 Conflict",
                0 => "503 Service Unavailable",
                _ => "502 Bad Gateway",
            };
            Some(error_response(http, failed_code, hint))
        }
    }
}

/// Write what a bind said onto the local row. `Ok(None)` = unpaired, the
/// row is left as it was (an unreachable tunnel never downgrades a zone).
fn store_bind_outcome(
    conn: &rusqlite::Connection,
    apex: &str,
    outcome: &BindOutcome,
    unpaired_is_byo: bool,
) -> rusqlite::Result<Option<DomainBinding>> {
    match outcome {
        BindOutcome::Bound(zone) => upsert_binding_zone(conn, apex, zone).map(Some),
        BindOutcome::NotOwned => upsert_binding(conn, apex, None, false).map(Some),
        BindOutcome::NoTunnel if unpaired_is_byo => upsert_binding(conn, apex, None, false).map(Some),
        _ => Ok(None),
    }
}

/// GET `/cli/domains` — host inventory. Pure read: never dials k2.dev
/// (A8.1 bind can auto-add a zone to the account, so a GET must not
/// bind). Re-check is `POST /cli/domains/refresh`.
pub fn handle_list(_params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_agent_list() {
        return r;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let bindings = match list_bindings(&conn) {
        Ok(v) => v,
        Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
    };
    let domains: Vec<serde_json::Value> =
        bindings.iter().map(|b| domain_json(&conn, b)).collect();
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "domains": domains }).to_string(),
    )
}

/// POST `/cli/domains/refresh` `{apex?}` — re-bind on demand so a
/// `pending_ns` zone flips to `active` once its nameservers point at
/// k2.dev. With `apex`: that binding, whatever its state (404 when not
/// attached) — also how a BYO row picks up a zone k2.dev now owns.
/// Without: every `pending_ns` binding. Owner/admin (a re-bind may
/// auto-add the zone to the k2.dev account).
///
/// `checked:false` = no tunnel token; rows are left unchanged.
pub fn handle_refresh(params: &HashMap<String, String>) -> CliResponse {
    let single = body_str(params, &["apex", "domain"]).map(str::to_string);
    let targets: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(raw) = single.as_deref() {
            let apex = match normalize_apex(raw) {
                Ok(a) => a,
                Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
            };
            match get_binding(&conn, &apex) {
                Ok(Some(_)) => vec![apex],
                Ok(None) => {
                    return error_response(
                        "404 Not Found",
                        "not_found",
                        &format!("no domain binding for '{apex}' — k2 domain add {apex} first"),
                    )
                }
                Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
            }
        } else {
            match list_bindings(&conn) {
                Ok(v) => v
                    .into_iter()
                    .filter(|b| b.is_pending_ns())
                    .map(|b| b.apex)
                    .collect(),
                Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
            }
        }
    };

    // Dial k2.dev without holding the DB lock.
    let mut outcomes: Vec<(String, BindOutcome)> = Vec::with_capacity(targets.len());
    for apex in targets {
        let outcome = bind_apex(&apex);
        if let Some(r) = bind_error_response(&outcome, "bind_failed") {
            return r;
        }
        outcomes.push((apex, outcome));
    }

    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut checked = true;
    let mut domains = Vec::with_capacity(outcomes.len());
    for (apex, outcome) in &outcomes {
        if matches!(outcome, BindOutcome::NoTunnel) {
            checked = false;
        }
        if matches!(outcome, BindOutcome::Bound(_)) {
            crate::domains::pending_watch::clear_missing(apex);
        }
        let row = match store_bind_outcome(&conn, apex, outcome, false) {
            Ok(Some(b)) => Some(b),
            Ok(None) => get_binding(&conn, apex).ok().flatten(),
            Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
        };
        if let Some(b) = row {
            domains.push(domain_json(&conn, &b));
        }
    }
    drop(conn);
    if checked && !outcomes.is_empty() {
        // P1: the loop re-reads the rows (a row still pending keeps its
        // schedule; nothing pending parks it). Other clients repaint.
        crate::domains::pending_watch::wake();
        let one = if outcomes.len() == 1 { Some(outcomes[0].0.as_str()) } else { None };
        crate::session_events::emit_domains_changed("refreshed", one);
        if outcomes.iter().any(|(_, o)| matches!(o, BindOutcome::Bound(z) if z.status.as_deref() == Some(k2_core::domains::ZONE_STATUS_ACTIVE))) {
            crate::domains::renew::wake();
        }
    }
    let mut body = serde_json::json!({
        "ok": true,
        "checked": checked,
        "domains": domains,
    });
    if !checked {
        body["hint"] = serde_json::json!(
            "no K2 Connect tunnel token on this server — pair K2 Connect, then check again"
        );
    }
    if single.is_some() {
        if let Some(first) = body["domains"].get(0).cloned() {
            body["domain"] = first;
        }
    }
    CliResponse::ok_json(body.to_string())
}

/// POST `/cli/domains` `{apex}` — attach (idempotent). Owner/admin.
pub fn handle_attach(params: &HashMap<String, String>) -> CliResponse {
    let Some(raw) = body_str(params, &["apex", "domain"]) else {
        return error_response("400 Bad Request", "usage", "missing 'apex'");
    };
    let apex = match normalize_apex(raw) {
        Ok(a) => a,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };

    let outcome = bind_apex(&apex);
    if let Some(r) = bind_error_response(&outcome, "bind_failed") {
        return r;
    }

    if matches!(outcome, BindOutcome::Bound(_)) {
        crate::domains::pending_watch::clear_missing(&apex);
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store_bind_outcome(&conn, &apex, &outcome, true) {
        Ok(None) => error_response(
            "500 Internal Server Error",
            "bind_failed",
            &format!("unexpected bind outcome for '{apex}': {outcome:?}"),
        ),
        Ok(Some(b)) => {
            let body = serde_json::json!({
                "ok": true,
                "domain": domain_json(&conn, &b),
            })
            .to_string();
            drop(conn);
            // P1 / DN12a: a new pending row starts the 1-minute step now
            // instead of waiting out a parked loop.
            crate::domains::pending_watch::wake();
            crate::session_events::emit_domains_changed("attached", Some(&apex));
            CliResponse::ok_json(body)
        }
        Err(e) => error_response("500 Internal Server Error", "db", &e.to_string()),
    }
}

/// POST `/cli/domains/remove` `{apex}`. Unbind then cascade names.
pub fn handle_remove(params: &HashMap<String, String>) -> CliResponse {
    let Some(raw) = body_str(params, &["apex", "domain"]) else {
        return error_response("400 Bad Request", "usage", "missing 'apex'");
    };
    let apex = match normalize_apex(raw) {
        Ok(a) => a,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };

    let db = k2_core::db::shared();
    let conn = db.lock();
    let existing = match get_binding(&conn, &apex) {
        Ok(v) => v,
        Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
    };
    if existing.is_none() {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!("no domain binding for '{apex}'"),
        );
    }

    if existing.as_ref().map(|b| b.dns_write).unwrap_or(false)
        || existing.as_ref().and_then(|b| b.zone_id.as_ref()).is_some()
    {
        let outcome = unbind_apex(&apex);
        // Another account owning the apex means there is nothing of ours
        // to unbind — local remove proceeds.
        if !matches!(outcome, BindOutcome::OwnedElsewhere { .. }) {
            if let Some(r) = bind_error_response(&outcome, "unbind_failed") {
                return r;
            }
        }
    }

    match remove_binding(&conn, &apex) {
        Ok(_) => {
            drop(conn);
            crate::domains::pending_watch::clear_missing(&apex);
            crate::session_events::emit_domains_changed("removed", Some(&apex));
            CliResponse::ok_json(serde_json::json!({ "ok": true, "removed": apex }).to_string())
        }
        Err(e) => error_response("500 Internal Server Error", "db", &e.to_string()),
    }
}

/// POST `/cli/domains/names` `{hostname, role?}`.
pub fn handle_names_add(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_agent_list() {
        return r;
    }
    let Some(raw_host) = body_str(params, &["hostname", "name"]) else {
        return error_response("400 Bad Request", "usage", "missing 'hostname'");
    };
    let hostname = match normalize_hostname(raw_host) {
        Ok(h) => h,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };
    let role = match normalize_role(body_str(params, &["role"])) {
        Ok(r) => r,
        Err(e) => return error_response("400 Bad Request", "invalid_role", &e),
    };

    let db = k2_core::db::shared();
    let conn = db.lock();

    let apex = if let Some(raw_apex) = body_str(params, &["apex"]) {
        match normalize_apex(raw_apex) {
            Ok(a) => a,
            Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
        }
    } else {
        match infer_apex(&conn, &hostname) {
            Ok(a) => a,
            Err(r) => return r,
        }
    };

    if get_binding(&conn, &apex).ok().flatten().is_none() {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!("apex '{apex}' is not attached — k2 domain add {apex} first"),
        );
    }
    if !hostname_under_apex(&hostname, &apex) {
        return error_response(
            "400 Bad Request",
            "hostname_not_under_apex",
            &format!("hostname '{hostname}' is not {apex} or a label under it"),
        );
    }

    match upsert_name(&conn, &hostname, &apex, &role) {
        Ok(n) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "name": {
                    "hostname": n.hostname,
                    "apex": n.apex,
                    "role": n.role,
                    "cert": crate::domains::status::cert_json_for(&n.hostname, &n.role),
                }
            })
            .to_string(),
        ),
        Err(e) => error_response("500 Internal Server Error", "db", &e.to_string()),
    }
}

fn infer_apex(conn: &rusqlite::Connection, hostname: &str) -> Result<String, CliResponse> {
    let bindings = list_bindings(conn).map_err(|e| {
        error_response("500 Internal Server Error", "db", &e.to_string())
    })?;
    let mut matches: Vec<&str> = bindings
        .iter()
        .map(|b| b.apex.as_str())
        .filter(|apex| hostname_under_apex(hostname, apex))
        .collect();
    matches.sort_by_key(|a| std::cmp::Reverse(a.len()));
    match matches.first() {
        Some(a) => Ok((*a).to_string()),
        None => Err(error_response(
            "404 Not Found",
            "not_found",
            &format!("no attached apex covers hostname '{hostname}'"),
        )),
    }
}

/// POST `/cli/domains/names/remove` `{hostname}`.
pub fn handle_names_remove(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_agent_list() {
        return r;
    }
    let Some(raw_host) = body_str(params, &["hostname", "name"]) else {
        return error_response("400 Bad Request", "usage", "missing 'hostname'");
    };
    let hostname = match normalize_hostname(raw_host) {
        Ok(h) => h,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match remove_name(&conn, &hostname) {
        Ok(true) => CliResponse::ok_json(
            serde_json::json!({ "ok": true, "removed": hostname }).to_string(),
        ),
        Ok(false) => error_response(
            "404 Not Found",
            "not_found",
            &format!("no hostname '{hostname}'"),
        ),
        Err(e) => error_response("500 Internal Server Error", "db", &e.to_string()),
    }
}

/// GET `/cli/certs` — inventory with cert column (missing until Slice B).
pub fn handle_certs_list(_params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_agent_list() {
        return r;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let names = match k2_core::domains::list_names(&conn) {
        Ok(v) => v,
        Err(e) => return error_response("500 Internal Server Error", "db", &e.to_string()),
    };
    let certs: Vec<serde_json::Value> = names
        .iter()
        .map(|n| {
            serde_json::json!({
                "hostname": n.hostname,
                "apex": n.apex,
                "role": n.role,
                "cert": crate::domains::status::cert_json_for(&n.hostname, &n.role),
                "renewal": crate::domains::renew::name_json(&n.hostname),
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "ok": true, "certs": certs }).to_string())
}

fn gate_agent_issue() -> Result<(), CliResponse> {
    gate_agent_list()
}

/// POST `/cli/certs/issue` `{hostname}`.
pub fn handle_issue(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_agent_issue() {
        return r;
    }
    let Some(raw) = body_str(params, &["hostname", "name"]) else {
        return error_response("400 Bad Request", "usage", "missing 'hostname'");
    };
    let hostname = match normalize_hostname(raw) {
        Ok(h) => h,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };
    match crate::domains::acme::issue_attached(&hostname) {
        Ok(pem) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let role = get_name(&conn, &hostname)
                .ok()
                .flatten()
                .map(|n| n.role)
                .unwrap_or_else(|| "other".into());
            // Inventory PEM, not live TLS — a CNAME to mail.lztek.io
            // presents mail.discover-nocode.com (scratch-le3 false reuse).
            let cert = crate::domains::status::from_pem_file(&hostname)
                .map(|p| p.to_json())
                .unwrap_or_else(|| crate::domains::status::cert_json_for(&hostname, &role));
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "hostname": pem.hostname,
                    "cert": cert,
                    "renewal": crate::domains::renew::name_json(&hostname),
                })
                .to_string(),
            )
        }
        Err(e) => {
            let not_attached = e.contains("not attached");
            let busy = e.contains("already in progress");
            error_response(
                if not_attached {
                    "404 Not Found"
                } else if busy {
                    "409 Conflict"
                } else {
                    "400 Bad Request"
                },
                if not_attached {
                    "not_found"
                } else if busy {
                    "busy"
                } else {
                    "acme"
                },
                &e,
            )
        }
    }
}

/// POST `/cli/certs/renew` `{hostname}`. Never hostmail disable/enable.
pub fn handle_renew(params: &HashMap<String, String>) -> CliResponse {
    handle_issue(params)
}

/// POST `/cli/certs/upload` `{hostname, certPem, keyPem}` — owner-only.
pub fn handle_upload(params: &HashMap<String, String>) -> CliResponse {
    let Some(raw) = body_str(params, &["hostname", "name"]) else {
        return error_response("400 Bad Request", "usage", "missing 'hostname'");
    };
    let hostname = match normalize_hostname(raw) {
        Ok(h) => h,
        Err(e) => return error_response("400 Bad Request", "invalid_domain", &e),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(name) = get_name(&conn, &hostname).ok().flatten() else {
        return error_response(
            "404 Not Found",
            "not_found",
            &format!("hostname '{hostname}' is not attached"),
        );
    };
    let cert_pem = body_str(params, &["certPem", "cert", "certificate"]).unwrap_or("");
    let key_pem = body_str(params, &["keyPem", "key", "privateKey"]).unwrap_or("");
    match crate::domains::store::install(&hostname, cert_pem, key_pem) {
        Ok(_) => {
            // A hand-supplied certificate: the K2 renewer leaves it alone.
            crate::domains::renew::mark_uploaded(&hostname);
            if name.role == k2_core::domains::ROLE_MAIL {
                if let Err(e) = crate::domains::store::plant_mail_pem(&hostname, cert_pem, key_pem) {
                    return error_response("400 Bad Request", "plant", &e);
                }
                if let Err(e) = crate::domains::status::reject_live_self_signed(&hostname) {
                    return error_response("400 Bad Request", "plant", &e);
                }
            }
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "hostname": hostname,
                    "cert": crate::domains::status::cert_json_for(&hostname, &name.role),
                })
                .to_string(),
            )
        }
        Err(e) => error_response("400 Bad Request", "upload", &e),
    }
}

/// POST `/cli/certs/config` `{email?, directory?}` — owner ACME override.
pub fn handle_config_set(params: &HashMap<String, String>) -> CliResponse {
    let mut cfg = crate::domains::store::load_acme_config();
    if let Some(e) = body_str(params, &["email"]) {
        cfg.email = if e.is_empty() { None } else { Some(e.to_string()) };
    }
    if let Some(d) = body_str(params, &["directory"]) {
        cfg.directory = if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        };
    }
    match crate::domains::store::save_acme_config(&cfg) {
        Ok(()) => CliResponse::ok_json(serde_json::json!({ "ok": true, "config": cfg }).to_string()),
        Err(e) => error_response("500 Internal Server Error", "config", &e),
    }
}

pub fn handle_issue_post(body: &[u8]) -> CliResponse {
    handle_issue(&params_from_post(body))
}
pub fn handle_renew_post(body: &[u8]) -> CliResponse {
    handle_renew(&params_from_post(body))
}
pub fn handle_upload_post(body: &[u8]) -> CliResponse {
    handle_upload(&params_from_post(body))
}
pub fn handle_config_post(body: &[u8]) -> CliResponse {
    handle_config_set(&params_from_post(body))
}

pub fn handle_attach_post(body: &[u8]) -> CliResponse {
    handle_attach(&params_from_post(body))
}

pub fn handle_remove_post(body: &[u8]) -> CliResponse {
    handle_remove(&params_from_post(body))
}

pub fn handle_refresh_post(body: &[u8]) -> CliResponse {
    handle_refresh(&params_from_post(body))
}

pub fn handle_names_add_post(body: &[u8]) -> CliResponse {
    handle_names_add(&params_from_post(body))
}

pub fn handle_names_remove_post(body: &[u8]) -> CliResponse {
    handle_names_remove(&params_from_post(body))
}

fn zone_not_attached() -> CliResponse {
    error_response(
        "403 Forbidden",
        "dns_zone_not_attached",
        crate::dns::DNS_ZONE_NOT_ATTACHED_HINT,
    )
}

/// Belt used by `/cli/dns/*` record writes (A6). A `pending_ns` zone
/// (A8.1) refuses with `zone_pending_ns` + the nameservers to set; a
/// `winddown` / `suspended` zone refuses with `zone_readonly` (DN12c).
/// Production callers use [`require_zone_attached_for_write_with`].
#[cfg(test)]
pub fn require_zone_attached_for_write(zone_id: &str) -> Result<(), CliResponse> {
    require_zone_attached_for_write_with(zone_id, None)
}

/// [`require_zone_attached_for_write`] with P2 (prd-dns-pending-and-
/// cutover-safety-v1): when the local row still reads `pending_ns`, ask
/// k2.dev once before refusing, store the answer, then decide. `zones` is
/// a zones list the caller already fetched (a by-domain lookup): zero
/// extra calls. Otherwise one `GET /api/dns/zones`. The DB lock is never
/// held across that call. If k2.dev can't be asked (unpaired, air-gap,
/// down) the row is kept and the refusal stands.
pub fn require_zone_attached_for_write_with(
    zone_id: &str,
    zones: Option<&[crate::domains::pending_watch::RemoteZone]>,
) -> Result<(), CliResponse> {
    let row = read_zone_row(zone_id)?;
    if row.as_ref().is_some_and(DomainBinding::is_pending_ns) {
        if let Err(e) = crate::domains::pending_watch::recheck_now(zones) {
            k2_core::log_debug!(
                "[domains] pending zone {zone_id}: re-check before write failed ({e}) — keeping the stored status"
            );
        }
        return decide_write(read_zone_row(zone_id)?);
    }
    decide_write(row)
}

fn read_zone_row(zone_id: &str) -> Result<Option<DomainBinding>, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    get_binding_by_zone_id(&conn, zone_id)
        .map_err(|e| error_response("500 Internal Server Error", "db", &e.to_string()))
}

fn decide_write(row: Option<DomainBinding>) -> Result<(), CliResponse> {
    match row {
        Some(b) if b.is_pending_ns() => Err(error_response(
            "403 Forbidden",
            "zone_pending_ns",
            &pending_ns_hint(&b),
        )),
        Some(b) if b.is_readonly() => Err(error_response(
            "403 Forbidden",
            "zone_readonly",
            &zone_readonly_hint(&b),
        )),
        Some(b) if b.dns_write => Ok(()),
        _ => Err(zone_not_attached()),
    }
}

/// Belt for `/cli/dns/*` reads (records list, delegation verify): a
/// `pending_ns` zone may be read and verified — that is how it turns
/// active — but not written.
pub fn require_zone_attached_for_read(zone_id: &str) -> Result<(), CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match get_binding_by_zone_id(&conn, zone_id) {
        // A read-only (winddown/suspended) zone still serves; it may be read.
        Ok(Some(b)) if b.dns_write || b.is_pending_ns() || b.is_readonly() => Ok(()),
        Ok(_) => Err(zone_not_attached()),
        Err(e) => Err(error_response(
            "500 Internal Server Error",
            "db",
            &e.to_string(),
        )),
    }
}

#[allow(dead_code)]
pub fn require_apex_attached_for_write(apex: &str) -> Result<(), CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match apex_attached_for_write(&conn, apex) {
        Ok(true) => Ok(()),
        Ok(false) => Err(error_response(
            "403 Forbidden",
            "dns_zone_not_attached",
            crate::dns::DNS_ZONE_NOT_ATTACHED_HINT,
        )),
        Err(e) => Err(error_response(
            "500 Internal Server Error",
            "db",
            &e.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::domains::upsert_binding;

    fn init() {
        let _ = k2_core::db::init_for_tests();
    }

    #[test]
    fn hostname_under_wrong_apex_is_400() {
        init();
        let db = k2_core::db::shared();
        {
            let conn = db.lock();
            let _ = remove_binding(&conn, "other.com");
            let _ = remove_binding(&conn, "example.com");
            upsert_binding(&conn, "other.com", None, false).unwrap();
        }
        let mut params = HashMap::new();
        params.insert("hostname".into(), "mail.example.com".into());
        params.insert("apex".into(), "other.com".into());
        let resp = handle_names_add(&params);
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            resp.body.contains("hostname_not_under_apex"),
            "{}",
            resp.body
        );
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = remove_binding(&conn, "other.com");
    }

    #[test]
    fn remove_apex_cascades_names() {
        init();
        let db = k2_core::db::shared();
        {
            let conn = db.lock();
            upsert_binding(&conn, "cascade.test", None, false).unwrap();
            upsert_name(&conn, "mail.cascade.test", "cascade.test", "mail").unwrap();
        }
        let mut params = HashMap::new();
        params.insert("apex".into(), "cascade.test".into());
        let resp = handle_remove(&params);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(get_name(&conn, "mail.cascade.test").unwrap().is_none());
        assert!(get_binding(&conn, "cascade.test").unwrap().is_none());
    }

    #[test]
    fn attach_byo_without_tunnel_is_dns_write_false() {
        init();
        // No fake bind installed → the test seam reads as an unpaired box
        // (never dials k2.dev) → BYO.
        let mut params = HashMap::new();
        params.insert("apex".into(), "byo-attach.example".into());
        let resp = handle_attach(&params);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = json(&resp);
        assert_eq!(v.pointer("/domain/dnsWrite"), Some(&serde_json::json!(false)), "{v}");
        assert_eq!(v.pointer("/domain/status"), Some(&serde_json::Value::Null), "{v}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = remove_binding(&conn, "byo-attach.example");
    }

    // ── A8.1 bind contract → route mapping ──────────────────────────

    use crate::dns::proxy::DnsHttpResponse;
    use crate::domains::bind::{
        fake_bind_calls, pending_rows_test_lock, replace_fake_bind, set_fake_bind,
    };

    fn json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body)
            .unwrap_or_else(|e| panic!("non-JSON body ({e}): {}", resp.body))
    }

    fn reply(status: u16, body: &str) -> Result<DnsHttpResponse, String> {
        Ok(DnsHttpResponse {
            status,
            body: body.into(),
        })
    }

    fn attach(apex: &str) -> CliResponse {
        let mut params = HashMap::new();
        params.insert("apex".into(), apex.into());
        handle_attach(&params)
    }

    fn refresh(apex: Option<&str>) -> CliResponse {
        let body = match apex {
            Some(a) => serde_json::json!({ "apex": a }).to_string(),
            None => "{}".to_string(),
        };
        handle_refresh_post(body.as_bytes())
    }

    fn binding(apex: &str) -> Option<DomainBinding> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        get_binding(&conn, apex).expect("db read")
    }

    fn drop_binding(apex: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        remove_binding(&conn, apex).expect("cleanup");
    }

    fn error_code(resp: &CliResponse) -> String {
        json(resp)
            .pointer("/error/code")
            .and_then(|c| c.as_str())
            .unwrap_or_else(|| panic!("no error.code: {}", resp.body))
            .to_string()
    }

    const PENDING_CREATED: &str = r#"{"ok":true,"zoneId":"zone-a81-pending","status":"pending_ns","nameservers":["ns1.k2.dev","ns2.k2.dev"],"dnsWrite":false,"created":true}"#;
    const FLIP_PENDING: &str = r#"{"ok":true,"zoneId":"zone-a81-flip","status":"pending_ns","nameservers":["ns1.k2.dev","ns2.k2.dev"],"dnsWrite":false,"created":true}"#;
    const FLIP_ACTIVE: &str = r#"{"ok":true,"zoneId":"zone-a81-flip","status":"active","nameservers":["ns1.k2.dev","ns2.k2.dev"],"dnsWrite":true}"#;

    #[test]
    fn attach_api_missing_is_503_never_502() {
        init();
        let cases: &[(u16, &str)] = &[
            (405, ""),
            (404, ""),
            (404, "<!DOCTYPE html><html><body>404</body></html>"),
            (200, "<html><body>catch-all</body></html>"),
            (502, "<html><body>Bad gateway</body></html>"),
        ];
        for (status, body) in cases {
            let _g = set_fake_bind(reply(*status, body));
            let resp = attach("api-missing.example");
            assert_eq!(
                resp.status, "503 Service Unavailable",
                "HTTP {status} {body:?} → {}",
                resp.body
            );
            assert_ne!(resp.status, "502 Bad Gateway");
            assert_eq!(error_code(&resp), "bind_api_unavailable");
            let hint = json(&resp)["error"]["hint"].as_str().expect("hint").to_string();
            assert!(hint.starts_with("Domain linking isn't live on k2.dev yet"), "{hint}");
            assert!(binding("api-missing.example").is_none(), "nothing stored on API-missing");
            assert_eq!(fake_bind_calls().len(), 1, "one bind call");
        }
    }

    #[test]
    fn attach_409_is_zone_owned_elsewhere_with_hint() {
        init();
        let _g = set_fake_bind(reply(
            409,
            r#"{"error":"zone_owned_elsewhere","hint":"owned-else.example is in another k2.dev account"}"#,
        ));
        let resp = attach("owned-else.example");
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        assert_eq!(error_code(&resp), "zone_owned_elsewhere");
        assert_eq!(
            json(&resp)["error"]["hint"],
            "owned-else.example is in another k2.dev account"
        );
        assert!(binding("owned-else.example").is_none());
    }

    #[test]
    fn attach_auth_and_upstream_failures_keep_their_status() {
        init();
        let _g = set_fake_bind(reply(401, r#"{"error":"unauthorized"}"#));
        let r = attach("auth-fail.example");
        assert_eq!(r.status, "401 Unauthorized", "{}", r.body);
        assert_eq!(error_code(&r), "bind_failed");
        replace_fake_bind(reply(403, r#"{"error":"forbidden","hint":"plan required"}"#));
        let r = attach("auth-fail.example");
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(json(&r)["error"]["hint"], "plan required");
        // A real JSON 5xx from a deployed route is still a bad gateway.
        replace_fake_bind(reply(500, r#"{"error":"db down"}"#));
        let r = attach("auth-fail.example");
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        assert!(binding("auth-fail.example").is_none());
    }

    #[test]
    fn attach_json_404_is_byo() {
        init();
        let _g = set_fake_bind(reply(404, r#"{"error":"not found"}"#));
        let r = attach("json404-byo.example");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let b = binding("json404-byo.example").expect("stored");
        assert!(!b.dns_write);
        assert_eq!(b.status, None);
        assert!(b.zone_id.is_none());
        drop_binding("json404-byo.example");
    }

    #[test]
    fn attach_pending_created_is_stored_and_listed() {
        init();
        let _lock = pending_rows_test_lock();
        let apex = "a81-pending.example";
        let _g = set_fake_bind(reply(200, PENDING_CREATED));
        let r = attach(apex);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r);
        assert_eq!(v["domain"]["status"], "pending_ns");
        assert_eq!(v["domain"]["pendingNs"], true);
        assert_eq!(v["domain"]["dnsWrite"], false);
        assert_eq!(v["domain"]["created"], true);
        assert_eq!(v["domain"]["zoneId"], "zone-a81-pending");
        assert_eq!(v["domain"]["nameservers"], serde_json::json!(["ns1.k2.dev", "ns2.k2.dev"]));

        // GET list exposes the same fields and never dials k2.dev.
        let before = fake_bind_calls().len();
        let list = handle_list(&HashMap::new());
        assert_eq!(list.status, "200 OK", "{}", list.body);
        assert_eq!(fake_bind_calls().len(), before, "GET /cli/domains must not bind");
        let lv = json(&list);
        let row = lv["domains"]
            .as_array()
            .expect("domains")
            .iter()
            .find(|d| d["apex"] == apex)
            .unwrap_or_else(|| panic!("{apex} missing from list: {lv}"))
            .clone();
        assert_eq!(row["status"], "pending_ns");
        assert_eq!(row["created"], true);
        assert_eq!(row["nameservers"], serde_json::json!(["ns1.k2.dev", "ns2.k2.dev"]));
        drop_binding(apex);
    }

    #[test]
    fn refresh_flips_pending_zone_to_active() {
        init();
        let _lock = pending_rows_test_lock();
        let apex = "a81-flip.example";
        let _g = set_fake_bind(reply(200, FLIP_PENDING));
        assert_eq!(attach(apex).status, "200 OK");
        assert!(binding(apex).expect("stored").is_pending_ns());

        // Writes are refused while pending.
        let w = match require_zone_attached_for_write("zone-a81-flip") {
            Err(r) => r,
            Ok(()) => panic!("pending zone must refuse writes"),
        };
        assert_eq!(error_code(&w), "zone_pending_ns");

        // Nameservers now point at k2.dev → Check again.
        replace_fake_bind(reply(200, FLIP_ACTIVE));
        let r = refresh(Some(apex));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r);
        assert_eq!(v["checked"], true);
        assert_eq!(v["domain"]["status"], "active");
        assert_eq!(v["domain"]["dnsWrite"], true);
        assert_eq!(v["domain"]["pendingNs"], false);
        // `created` stays sticky across a re-bind that omits it.
        assert_eq!(v["domain"]["created"], true);
        let b = binding(apex).expect("stored");
        assert_eq!(b.status.as_deref(), Some("active"));
        assert!(b.dns_write);
        let calls = fake_bind_calls();
        assert_eq!(calls.last().map(|c| c.0.as_str()), Some("/api/dns/zones/bind"));

        if let Err(r) = require_zone_attached_for_write("zone-a81-flip") {
            panic!("active zone must accept writes: {}", r.body);
        }
        drop_binding(apex);
    }

    /// DN12a/e: attach, refresh and remove announce `domains_changed`
    /// (other clients repaint), and the list carries the new fields.
    #[test]
    fn attach_refresh_remove_emit_domains_changed() {
        init();
        let _lock = pending_rows_test_lock();
        let apex = "dc-events.example";
        let mut rx = crate::session_events::subscribe();
        let frames = |rx:&mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>| {
            let mut out = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                if let crate::session_events::SessionEvent::DomainsChanged { reason, apex: Some(a) } = ev {
                    if a == apex {
                        out.push(reason);
                    }
                }
            }
            out
        };
        let pending = r#"{"ok":true,"zoneId":"zone-dc-events","status":"pending_ns","nameservers":["ns1.k2.dev"],"dnsWrite":false,"created":true}"#;
        let _g = set_fake_bind(reply(200, pending));
        let r = attach(apex);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r);
        assert_eq!(v["domain"]["readOnly"], false);
        assert_eq!(v["domain"]["zoneMissing"], false);
        assert!(v["domain"].get("checkedAt").is_some(), "{v}");
        assert_eq!(frames(&mut rx), vec!["attached".to_string()]);

        replace_fake_bind(reply(
            200,
            r#"{"ok":true,"zoneId":"zone-dc-events","status":"winddown","dnsWrite":false}"#,
        ));
        let r = refresh(Some(apex));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r);
        assert_eq!(v["domain"]["status"], "winddown");
        assert_eq!(v["domain"]["readOnly"], true);
        assert_eq!(frames(&mut rx), vec!["refreshed".to_string()]);
        // DN12c: a winddown zone refuses writes with zone_readonly.
        let w = require_zone_attached_for_write("zone-dc-events").expect_err("winddown refuses");
        assert_eq!(error_code(&w), "zone_readonly");

        replace_fake_bind(reply(200, r#"{"ok":true}"#));
        let mut params = HashMap::new();
        params.insert("apex".into(), apex.into());
        let r = handle_remove(&params);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(frames(&mut rx), vec!["removed".to_string()]);
    }

    #[test]
    fn refresh_all_rebinds_only_pending_rows() {
        init();
        let _lock = pending_rows_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            upsert_binding(&conn, "a81-all-byo.example", None, false).unwrap();
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            upsert_binding_zone(
                &conn,
                "a81-all-active.example",
                &k2_core::domains::BoundZone {
                    zone_id: Some("z-all-active".into()),
                    status: Some("active".into()),
                    nameservers: vec![],
                    dns_write: true,
                    auto_created: false,
                },
            )
            .unwrap();
            upsert_binding_zone(
                &conn,
                "a81-all-pending.example",
                &k2_core::domains::BoundZone {
                    zone_id: Some("z-all-pending".into()),
                    status: Some("pending_ns".into()),
                    nameservers: vec!["ns1.k2.dev".into()],
                    dns_write: false,
                    auto_created: true,
                },
            )
            .unwrap();
        }
        let _g = set_fake_bind(reply(
            200,
            r#"{"zoneId":"z-all-pending","status":"active","dnsWrite":true}"#,
        ));
        let r = refresh(None);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let bodies: Vec<String> = fake_bind_calls().into_iter().map(|c| c.1).collect();
        assert!(
            bodies.iter().any(|b| b.contains("a81-all-pending.example")),
            "{bodies:?}"
        );
        assert!(
            !bodies.iter().any(|b| b.contains("a81-all-active.example")),
            "active rows are not re-bound: {bodies:?}"
        );
        assert!(
            !bodies.iter().any(|b| b.contains("a81-all-byo.example")),
            "BYO rows are re-checked by apex only: {bodies:?}"
        );
        assert_eq!(
            binding("a81-all-pending.example").expect("row").status.as_deref(),
            Some("active")
        );
        assert_eq!(binding("a81-all-byo.example").expect("row").status, None);
        drop_binding("a81-all-byo.example");
        drop_binding("a81-all-active.example");
        drop_binding("a81-all-pending.example");
    }

    #[test]
    fn refresh_errors_and_unpaired() {
        init();
        let _lock = pending_rows_test_lock();
        // Not attached → 404.
        let r = refresh(Some("never-attached.example"));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);

        let apex = "a81-refresh-err.example";
        {
            let _g = set_fake_bind(reply(200, PENDING_CREATED));
            assert_eq!(attach(apex).status, "200 OK");
        }
        // API missing on refresh → 503, row untouched.
        {
            let _g = set_fake_bind(reply(405, ""));
            let r = refresh(Some(apex));
            assert_eq!(r.status, "503 Service Unavailable", "{}", r.body);
            assert_eq!(error_code(&r), "bind_api_unavailable");
            assert!(binding(apex).expect("row").is_pending_ns());
        }
        // 409 on refresh → 409, row untouched.
        {
            let _g = set_fake_bind(reply(409, r#"{"error":"zone_owned_elsewhere"}"#));
            let r = refresh(Some(apex));
            assert_eq!(r.status, "409 Conflict", "{}", r.body);
            assert_eq!(error_code(&r), "zone_owned_elsewhere");
            assert!(binding(apex).expect("row").is_pending_ns());
        }
        // Unpaired (no fake) → checked:false, row unchanged, never BYO.
        let r = refresh(Some(apex));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r);
        assert_eq!(v["checked"], false);
        assert_eq!(v["domain"]["status"], "pending_ns");
        drop_binding(apex);
    }

    #[test]
    fn remove_pending_unbinds_and_maps_api_missing_to_503() {
        init();
        let _lock = pending_rows_test_lock();
        let apex = "a81-remove.example";
        let _g = set_fake_bind(reply(200, PENDING_CREATED));
        assert_eq!(attach(apex).status, "200 OK");
        replace_fake_bind(reply(405, ""));
        let mut params = HashMap::new();
        params.insert("apex".into(), apex.into());
        let r = handle_remove(&params);
        assert_eq!(r.status, "503 Service Unavailable", "{}", r.body);
        assert_eq!(error_code(&r), "bind_api_unavailable");
        assert!(binding(apex).is_some(), "row kept when unbind can't run");

        replace_fake_bind(reply(200, r#"{"ok":true}"#));
        let r = handle_remove(&params);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            fake_bind_calls().last().map(|c| c.0.as_str()),
            Some("/api/dns/zones/unbind")
        );
        assert!(binding(apex).is_none());
    }

    #[test]
    fn issue_unattached_is_404() {
        init();
        let mut params = HashMap::new();
        params.insert("hostname".into(), "nope.example.com".into());
        let resp = handle_issue(&params);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
        assert!(resp.body.contains("not_found") || resp.body.contains("not attached"), "{}", resp.body);
    }

    #[test]
    fn issue_attached_fake_writes_0600_not_rcgen() {
        init();
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_ACME_FAKE", "1");
        std::env::set_var("K2_ACME_DNS_WAIT_SECS", "0");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            upsert_binding(&conn, "example.com", None, false).unwrap();
            upsert_name(&conn, "app.example.com", "example.com", "other").unwrap();
        }
        std::env::set_var("K2_ACME_HTTP01", "1");
        let mut params = HashMap::new();
        params.insert("hostname".into(), "app.example.com".into());
        let resp = handle_issue(&params);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert!(
            crate::domains::store::key_mode_is_0600("app.example.com"),
            "privkey must be 0600"
        );
        let body = resp.body.to_ascii_lowercase();
        assert!(!body.contains("rcgen"), "{body}");
        assert!(body.contains("issued") || body.contains("let's encrypt"), "{body}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = remove_binding(&conn, "example.com");
        std::env::remove_var("K2_ACME_HTTP01");
    }

    #[test]
    fn upload_unattached_is_404() {
        init();
        let mut params = HashMap::new();
        params.insert("hostname".into(), "missing.example.com".into());
        params.insert("certPem".into(), "-----BEGIN CERTIFICATE-----\nM\n-----END CERTIFICATE-----\n".into());
        params.insert("keyPem".into(), "-----BEGIN PRIVATE KEY-----\nM\n-----END PRIVATE KEY-----\n".into());
        let resp = handle_upload(&params);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
    }

    #[test]
    fn renew_is_the_same_issuer_as_issue() {
        // C8: renew retries issue. Never hostmail disable/enable.
        init();
        let mut params = HashMap::new();
        params.insert("hostname".into(), "nope.example.com".into());
        let issue = handle_issue(&params);
        let renew = handle_renew(&params);
        assert_eq!(issue.status, renew.status);
        assert_eq!(issue.status, "404 Not Found");
    }
}
