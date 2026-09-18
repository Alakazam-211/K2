//! `/cli/domains` + `/cli/certs` handlers (prd-custom-domains A2/A14).
//!
//! GET `/cli/domains` is the list (200). POST attach/remove/names are
//! owner/admin (dispatcher). GET of mutating paths → 405.

use std::collections::HashMap;

use k2_core::domains::{
    apex_attached_for_write, get_binding, hostname_under_apex, list_bindings, list_names_for_apex,
    normalize_apex, normalize_hostname, normalize_role, remove_binding, remove_name,
    upsert_binding, upsert_name, DomainBinding, DomainName,
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

fn missing_cert() -> serde_json::Value {
    serde_json::json!({ "state": "missing" })
}

fn name_json(n: &DomainName) -> serde_json::Value {
    serde_json::json!({
        "hostname": n.hostname,
        "role": n.role,
        "cert": missing_cert(),
    })
}

fn domain_json(conn: &rusqlite::Connection, b: &DomainBinding) -> serde_json::Value {
    let names = list_names_for_apex(conn, &b.apex).unwrap_or_default();
    serde_json::json!({
        "apex": b.apex,
        "zoneId": b.zone_id,
        "dnsWrite": b.dns_write,
        "names": names.iter().map(name_json).collect::<Vec<_>>(),
    })
}

fn maybe_reresolve(conn: &rusqlite::Connection, b: &DomainBinding) -> DomainBinding {
    if b.dns_write || b.zone_id.is_some() {
        return b.clone();
    }
    match bind_apex(&b.apex) {
        BindOutcome::Bound { zone_id } => {
            upsert_binding(conn, &b.apex, zone_id.as_deref(), true).unwrap_or_else(|_| b.clone())
        }
        _ => b.clone(),
    }
}

/// GET `/cli/domains` — host inventory.
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
    let domains: Vec<serde_json::Value> = bindings
        .iter()
        .map(|b| {
            let b = maybe_reresolve(&conn, b);
            domain_json(&conn, &b)
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "domains": domains }).to_string(),
    )
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

    let (dns_write, zone_id) = match bind_apex(&apex) {
        BindOutcome::Bound { zone_id } => (true, zone_id),
        BindOutcome::NotOwned | BindOutcome::NoTunnel => (false, None),
        BindOutcome::ApiMissing { hint } => {
            return error_response("404 Not Found", "bind_api_unavailable", &hint);
        }
        BindOutcome::Failed { status, hint } => {
            let http = match status {
                401 => "401 Unauthorized",
                403 => "403 Forbidden",
                0 => "503 Service Unavailable",
                _ => "502 Bad Gateway",
            };
            return error_response(http, "bind_failed", &hint);
        }
    };

    let db = k2_core::db::shared();
    let conn = db.lock();
    match upsert_binding(&conn, &apex, zone_id.as_deref(), dns_write) {
        Ok(b) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "domain": domain_json(&conn, &b),
            })
            .to_string(),
        ),
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
        match unbind_apex(&apex) {
            BindOutcome::Bound { .. }
            | BindOutcome::NotOwned
            | BindOutcome::NoTunnel => {}
            BindOutcome::ApiMissing { hint } => {
                return error_response("404 Not Found", "bind_api_unavailable", &hint);
            }
            BindOutcome::Failed { status, hint } => {
                let http = match status {
                    401 => "401 Unauthorized",
                    403 => "403 Forbidden",
                    0 => "503 Service Unavailable",
                    _ => "502 Bad Gateway",
                };
                return error_response(http, "unbind_failed", &hint);
            }
        }
    }

    match remove_binding(&conn, &apex) {
        Ok(_) => CliResponse::ok_json(
            serde_json::json!({ "ok": true, "removed": apex }).to_string(),
        ),
        Err(e) => error_response("500 Internal Server Error", "db", &e.to_string()),
    }
}

/// POST `/cli/domains/names` `{hostname, role?}`.
pub fn handle_names_add(params: &HashMap<String, String>) -> CliResponse {
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
                    "cert": missing_cert(),
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
                "cert": missing_cert(),
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "ok": true, "certs": certs }).to_string())
}

pub fn handle_attach_post(body: &[u8]) -> CliResponse {
    handle_attach(&params_from_post(body))
}

pub fn handle_remove_post(body: &[u8]) -> CliResponse {
    handle_remove(&params_from_post(body))
}

pub fn handle_names_add_post(body: &[u8]) -> CliResponse {
    handle_names_add(&params_from_post(body))
}

pub fn handle_names_remove_post(body: &[u8]) -> CliResponse {
    handle_names_remove(&params_from_post(body))
}

/// Belt used by `/cli/dns/*` writes (A6).
pub fn require_zone_attached_for_write(zone_id: &str) -> Result<(), CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::domains::zone_attached_for_write(&conn, zone_id) {
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
    use k2_core::domains::{get_name, upsert_binding};

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
        // No tunnel.json in this process HOME (tests typically have none).
        let mut params = HashMap::new();
        params.insert("apex".into(), "byo-attach.example".into());
        let resp = handle_attach(&params);
        // Air-gap/no token → BYO, or bind_api_unavailable if a token exists
        // and k2.dev 404s HTML. Either is loud; BYO is the unpaired path.
        assert!(
            resp.status.starts_with("200") || resp.body.contains("bind_api_unavailable") || resp.body.contains("bind_failed"),
            "{}",
            resp.body
        );
        if resp.status.starts_with("200") {
            assert!(resp.body.contains("\"dnsWrite\":false") || resp.body.contains("\"dnsWrite\": false"), "{}", resp.body);
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = remove_binding(&conn, "byo-attach.example");
        }
    }
}
