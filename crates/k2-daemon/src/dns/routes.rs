//! `/cli/dns/*` handlers — params-driven, principal-bound, toggle-gated.
//!
//! Identity always comes from Wave 0
//! [`crate::caller_workspace::resolve_caller_workspace_from_params`] (or the
//! request-principal slot for POST JSON bodies). Client `project=` is never
//! self-identity when a scoped principal is present.

use std::collections::HashMap;

use crate::caller_workspace::{
    resolve_caller_workspace_from_params, CallerWorkspace, ResolveCallerError,
};
use crate::cli_response::CliResponse;
use crate::dns::proxy::{map_proxy_response, proxy_request};
use crate::dns::{
    managed_by_touchable, normalize_record_type, record_type_allowed, DNS_DENIED_HINT,
    ZONE_LIFECYCLE_HINT,
};

// ── Shared helpers ────────────────────────────────────────────────────

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

fn resolve_caller(
    params: &HashMap<String, String>,
) -> Result<CallerWorkspace, CliResponse> {
    resolve_caller_workspace_from_params(params).map_err(|e| match e {
        ResolveCallerError::MissingClaim => error_response(
            "400 Bad Request",
            "usage",
            &e.hint(),
        ),
        ResolveCallerError::NotFound(_)
        | ResolveCallerError::Unregistered(_)
        | ResolveCallerError::UnresolvablePrincipal => {
            error_response("403 Forbidden", "forbidden", &e.hint())
        }
    })
}

/// Gate: resolve workspace + effective DNS-manage toggle (fail-closed).
fn gate_dns_manage(params: &HashMap<String, String>) -> Result<CallerWorkspace, CliResponse> {
    let ws = resolve_caller(params)?;
    if !k2_core::workspace::settings::dns_manage_allowed_for_path(&ws.path) {
        return Err(error_response(
            "403 Forbidden",
            "dns_manage_disabled",
            DNS_DENIED_HINT,
        ));
    }
    Ok(ws)
}

/// Agent name for audit header: stamped `from`, else empty.
fn agent_name(params: &HashMap<String, String>) -> Option<String> {
    params
        .get("from")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Merge a JSON object body into a params map (string values only).
/// Identity keys (`project`, `project_path`, `project_id`, `from`,
/// `principal_bound`) are **never** taken from the body — they must come
/// from the server stamp / request principal.
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

fn proxy_to_cli(
    method: &str,
    path: &str,
    agent: Option<&str>,
    body: Option<&str>,
) -> CliResponse {
    match proxy_request(method, path, agent, body) {
        Ok(resp) => {
            let (status, body) = map_proxy_response(&resp);
            CliResponse {
                status,
                content_type: "application/json",
                body,
            }
        }
        Err(e) => error_response("503 Service Unavailable", "proxy", &e),
    }
}

// ── Access / capability ───────────────────────────────────────────────

/// GET|POST `/cli/dns/access` — capability envelope + zones summary.
///
/// When the local toggle is OFF, returns `allowed: false` without dialing
/// the control plane (no existence oracle for zones). When ON, proxies
/// `GET /api/dns/zones` which already carries the capability payload.
pub fn handle_access(params: &HashMap<String, String>) -> CliResponse {
    let ws = match resolve_caller(params) {
        Ok(ws) => ws,
        Err(r) => return r,
    };
    let allowed = k2_core::workspace::settings::dns_manage_allowed_for_path(&ws.path);
    if !allowed {
        return CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "allowed": false,
                "zones": [],
                "record_types": crate::dns::AGENT_RECORD_TYPES,
                "hint": DNS_DENIED_HINT,
                "workspace": { "id": ws.workspace_uuid, "path": ws.path },
            })
            .to_string(),
        );
    }
    // Toggle ON → live capability from control plane.
    let agent = agent_name(params);
    let mut resp = proxy_to_cli("GET", "/api/dns/zones", agent.as_deref(), None);
    // Annotate with local workspace identity (never from claim alone).
    if resp.status.starts_with("200") {
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&resp.body) {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("ok".into(), serde_json::json!(true));
                obj.insert(
                    "workspace".into(),
                    serde_json::json!({
                        "id": ws.workspace_uuid,
                        "path": ws.path,
                    }),
                );
                // Promote capability.allowed if present.
                if let Some(cap) = obj.get("capability") {
                    if let Some(a) = cap.get("allowed").and_then(|x| x.as_bool()) {
                        obj.insert("allowed".into(), serde_json::json!(a));
                    } else {
                        obj.insert("allowed".into(), serde_json::json!(true));
                    }
                } else {
                    obj.insert("allowed".into(), serde_json::json!(true));
                }
            }
            resp.body = v.to_string();
        }
    }
    resp
}

// ── Zones list ────────────────────────────────────────────────────────

/// GET|POST `/cli/dns/zones` — list zones for the account.
pub fn handle_zones(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_dns_manage(params) {
        return r;
    }
    let agent = agent_name(params);
    proxy_to_cli("GET", "/api/dns/zones", agent.as_deref(), None)
}

// ── Records list ──────────────────────────────────────────────────────

/// GET|POST `/cli/dns/records` — list records for a zone.
/// Params: `zone` (id) or `domain` (resolved via zones list).
pub fn handle_records(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_dns_manage(params) {
        return r;
    }
    let zone_id = match resolve_zone_id(params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let agent = agent_name(params);
    let path = format!("/api/dns/zones/{zone_id}");
    proxy_to_cli("GET", &path, agent.as_deref(), None)
}

// ── Record add ────────────────────────────────────────────────────────

/// POST `/cli/dns/records/add` — insert a user-managed record.
/// Params: type, name, value|content, ttl?, priority|prio?, zone|domain.
pub fn handle_record_add(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_dns_manage(params) {
        return r;
    }

    let rtype_raw = params
        .get("type")
        .map(|s| s.as_str())
        .unwrap_or("");
    let Some(rtype) = normalize_record_type(rtype_raw) else {
        return error_response(
            "400 Bad Request",
            "type_not_allowed",
            &if rtype_raw.trim().is_empty() {
                "missing 'type' (A|AAAA|CNAME|TXT|MX|SRV|CAA)".to_string()
            } else {
                crate::dns::unsupported_type_hint(rtype_raw)
            },
        );
    };

    // Defense in depth: never allow NS even if the allowlist is later widened
    // by a partial edit.
    if rtype == "NS" || !record_type_allowed(&rtype) {
        return error_response(
            "400 Bad Request",
            "type_not_allowed",
            &crate::dns::unsupported_type_hint(&rtype),
        );
    }

    let name = params
        .get("name")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("@");
    let content = params
        .get("value")
        .or_else(|| params.get("content"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let Some(content) = content else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'value' (or 'content') for the record",
        );
    };

    let ttl: u64 = params
        .get("ttl")
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let prio = params
        .get("priority")
        .or_else(|| params.get("prio"))
        .and_then(|s| s.parse::<u64>().ok());

    let zone_id = match resolve_zone_id(params) {
        Ok(id) => id,
        Err(r) => return r,
    };

    let mut body = serde_json::json!({
        "type": rtype,
        "name": name,
        "content": content,
        "ttl": ttl,
    });
    if let Some(p) = prio {
        body["prio"] = serde_json::json!(p);
    }

    let agent = agent_name(params);
    let path = format!("/api/dns/zones/{zone_id}/records");
    proxy_to_cli(
        "POST",
        &path,
        agent.as_deref(),
        Some(&body.to_string()),
    )
}

// ── Record remove ─────────────────────────────────────────────────────

/// POST `/cli/dns/records/remove` — delete a user-managed record by id.
/// Params: `id` or `record` (record id). Optional `managed_by` for local
/// pre-check (control plane also enforces managed_by='user').
pub fn handle_record_remove(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_dns_manage(params) {
        return r;
    }

    if let Some(mb) = params.get("managed_by") {
        if !managed_by_touchable(Some(mb)) {
            return error_response(
                "403 Forbidden",
                "managed_by",
                &format!(
                    "this record is managed by '{mb}' automation and can't be edited by agents"
                ),
            );
        }
    }

    let record_id = params
        .get("id")
        .or_else(|| params.get("record"))
        .or_else(|| params.get("record_id"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let Some(record_id) = record_id else {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing record 'id'",
        );
    };

    let agent = agent_name(params);
    let path = format!("/api/dns/records/{record_id}");
    proxy_to_cli("DELETE", &path, agent.as_deref(), None)
}

// ── Verify ────────────────────────────────────────────────────────────

/// POST `/cli/dns/verify` — on-demand delegation check.
/// Params: zone (id) or domain.
pub fn handle_verify(params: &HashMap<String, String>) -> CliResponse {
    if let Err(r) = gate_dns_manage(params) {
        return r;
    }
    let zone_id = match resolve_zone_id(params) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let agent = agent_name(params);
    let path = format!("/api/dns/zones/{zone_id}/verify");
    proxy_to_cli("POST", &path, agent.as_deref(), Some("{}"))
}

// ── Owner-only zone lifecycle (local reject for agents) ───────────────

/// POST `/cli/dns/zones/create` — always local-reject for agent path.
/// (Owner zone lifecycle is dashboard-only in v1; this path exists so
/// `is_agent_verb` can deny it and handlers fail closed if reached.)
pub fn handle_zones_create(_params: &HashMap<String, String>) -> CliResponse {
    error_response("403 Forbidden", "owner_only", ZONE_LIFECYCLE_HINT)
}

/// POST `/cli/dns/zones/delete` — always local-reject (agents never delete zones).
pub fn handle_zones_delete(_params: &HashMap<String, String>) -> CliResponse {
    error_response("403 Forbidden", "owner_only", ZONE_LIFECYCLE_HINT)
}

// ── Zone id resolution ────────────────────────────────────────────────

fn resolve_zone_id(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    if let Some(z) = params
        .get("zone")
        .or_else(|| params.get("zone_id"))
        .or_else(|| params.get("id"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        // If it looks like a domain (contains a dot) and no explicit zone id
        // key was used, treat as domain lookup. Prefer explicit zone/zone_id.
        if params.get("zone").is_some() || params.get("zone_id").is_some() {
            return Ok(z.to_string());
        }
        if params.get("id").is_some() && !z.contains('.') {
            return Ok(z.to_string());
        }
        if !z.contains('.') {
            return Ok(z.to_string());
        }
        // fall through to domain lookup using `z`
        return resolve_zone_id_by_domain(params, z);
    }
    if let Some(d) = params
        .get("domain")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return resolve_zone_id_by_domain(params, d);
    }
    Err(error_response(
        "400 Bad Request",
        "usage",
        "missing 'zone' (id) or 'domain'",
    ))
}

fn resolve_zone_id_by_domain(
    params: &HashMap<String, String>,
    domain: &str,
) -> Result<String, CliResponse> {
    let agent = agent_name(params);
    let resp = match proxy_request("GET", "/api/dns/zones", agent.as_deref(), None) {
        Ok(r) => r,
        Err(e) => {
            return Err(error_response(
                "503 Service Unavailable",
                "proxy",
                &e,
            ))
        }
    };
    if resp.status != 200 {
        let (status, body) = map_proxy_response(&resp);
        return Err(CliResponse {
            status,
            content_type: "application/json",
            body,
        });
    }
    let v: serde_json::Value = serde_json::from_str(&resp.body).map_err(|e| {
        error_response(
            "502 Bad Gateway",
            "upstream",
            &format!("parse zones list: {e}"),
        )
    })?;
    let domain_lc = domain.trim().to_ascii_lowercase();
    let zones = v
        .get("zones")
        .and_then(|z| z.as_array())
        .cloned()
        .unwrap_or_default();
    for z in zones {
        let d = z
            .get("domain")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if d == domain_lc {
            if let Some(id) = z.get("id").and_then(|x| x.as_str()) {
                return Ok(id.to_string());
            }
        }
    }
    Err(error_response(
        "404 Not Found",
        "not_found",
        &format!("no DNS zone for domain '{domain}'"),
    ))
}

// ── POST body entry points ────────────────────────────────────────────

/// Build params from stamped map + JSON body (identity keys never from body).
pub fn params_from_post(
    base: &HashMap<String, String>,
    body: &[u8],
) -> HashMap<String, String> {
    let mut params = base.clone();
    merge_json_params(&mut params, body);
    // Re-apply principal stamp if present in request slot so body merge
    // can't leave a stale claim (stamp already happened on GET; for POST
    // the dispatcher installs with_request_principal).
    if let Some(p) = crate::caller_workspace::request_principal() {
        crate::caller_workspace::stamp_principal(&mut params, &p);
    }
    params
}

pub fn handle_access_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_access(&params)
}

pub fn handle_zones_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_zones(&params)
}

pub fn handle_records_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_records(&params)
}

pub fn handle_record_add_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_record_add(&params)
}

pub fn handle_record_remove_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_record_remove(&params)
}

pub fn handle_verify_post(body: &[u8]) -> CliResponse {
    let params = params_from_post(&HashMap::new(), body);
    handle_verify(&params)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::proxy::DnsHttpClient;
    use crate::session_token::HookPrincipal;

    fn seed_project(id: &str, path: &str, name: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, path, name],
        )
        .expect("seed");
    }

    fn cleanup(id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
    }

    fn enable_dns(path: &str) {
        k2_core::workspace::settings::update_project_setting(path, "dns_manage_enabled", "1")
            .expect("enable dns");
    }

    #[test]
    fn record_add_rejects_ns_type_before_proxy() {
        let _ = k2_core::db::init_for_tests();
        // 36-char UUID shape — resolve_workspace only does id lookup on that form.
        let id = "a1a1a1a1-a1a1-a1a1-a1a1-a1a1a1a1a1a1";
        let path = "/tmp/k2-dns-ns-reject";
        seed_project(id, path, "dns-ns");
        enable_dns(path);

        let principal = HookPrincipal {
            workspace_uuid: id.to_string(),
            agent_address: "agent-ns".to_string(),
        };
        let mut params = HashMap::new();
        crate::caller_workspace::stamp_principal(&mut params, &principal);
        params.insert("type".into(), "NS".into());
        params.insert("name".into(), "@".into());
        params.insert("value".into(), "ns1.example.com".into());
        params.insert("zone".into(), "zone-1".into());

        let resp = handle_record_add(&params);
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            resp.body.contains("type_not_allowed") || resp.body.contains("NS"),
            "{}",
            resp.body
        );

        cleanup(id);
    }

    #[test]
    fn principal_a_cannot_claim_b_for_toggle_path() {
        let _ = k2_core::db::init_for_tests();
        let a_id = "b2b2b2b2-b2b2-b2b2-b2b2-b2b2b2b2b2b2";
        let b_id = "c3c3c3c3-c3c3-c3c3-c3c3-c3c3c3c3c3c3";
        let a_path = "/tmp/k2-dns-princ-A";
        let b_path = "/tmp/k2-dns-princ-B";
        seed_project(a_id, a_path, "ws-A");
        seed_project(b_id, b_path, "ws-B");
        // Only B has DNS enabled — principal is A, so gate must deny even
        // when the client claims B's project path.
        enable_dns(b_path);

        let principal = HookPrincipal {
            workspace_uuid: a_id.to_string(),
            agent_address: "agent-A".to_string(),
        };
        let mut params = HashMap::new();
        crate::caller_workspace::stamp_principal(&mut params, &principal);
        // Hostile claim: pretend to be B.
        params.insert("project".into(), b_path.to_string());

        let resp = handle_access(&params);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v.get("allowed").and_then(|x| x.as_bool()),
            Some(false),
            "principal A must not inherit B's toggle via claim: {}",
            resp.body
        );
        // Workspace identity is A.
        assert_eq!(
            v.pointer("/workspace/id").and_then(|x| x.as_str()),
            Some(a_id)
        );

        cleanup(a_id);
        cleanup(b_id);
    }

    #[test]
    fn zones_create_is_local_owner_only_reject() {
        let resp = handle_zones_create(&HashMap::new());
        assert_eq!(resp.status, "403 Forbidden");
        assert!(resp.body.contains("owner_only"), "{}", resp.body);
    }

    #[test]
    fn merge_json_params_ignores_identity_keys() {
        let mut params = HashMap::new();
        params.insert("from".into(), "stamped-agent".into());
        params.insert("project_id".into(), "stamped-uuid".into());
        merge_json_params(
            &mut params,
            br#"{"type":"A","value":"1.2.3.4","from":"spoof","project":"/evil","project_id":"evil-id"}"#,
        );
        assert_eq!(params.get("from").map(String::as_str), Some("stamped-agent"));
        assert_eq!(params.get("project_id").map(String::as_str), Some("stamped-uuid"));
        assert!(!params.contains_key("project"));
        assert_eq!(params.get("type").map(String::as_str), Some("A"));
        assert_eq!(params.get("value").map(String::as_str), Some("1.2.3.4"));
    }

    /// Injectable client: record-add with allowed type builds POST to the
    /// right control-plane path (no live network beyond loopback mock).
    #[test]
    fn record_add_allowed_type_hits_proxy_path() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(String::new()));
        let seen_c = Arc::clone(&seen);
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).unwrap_or(0);
                *seen_c.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = r#"{"record":{"id":"r1","type":"A"},"propagation":"~10s"}"#;
                let resp = format!(
                    "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        // Serialize with other tests that mutate K2_DNS_API_BASE.
        static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        let _g = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(crate::dns::proxy::DNS_API_BASE_ENV);
        std::env::set_var(
            crate::dns::proxy::DNS_API_BASE_ENV,
            format!("http://127.0.0.1:{port}"),
        );

        let _ = k2_core::db::init_for_tests();
        let id = "d4d4d4d4-d4d4-d4d4-d4d4-d4d4d4d4d4d4";
        let path = "/tmp/k2-dns-add-path";
        seed_project(id, path, "dns-add");
        enable_dns(path);

        let principal = HookPrincipal {
            workspace_uuid: id.to_string(),
            agent_address: "agent-add".to_string(),
        };
        let mut params = HashMap::new();
        crate::caller_workspace::stamp_principal(&mut params, &principal);
        params.insert("type".into(), "A".into());
        params.insert("name".into(), "www".into());
        params.insert("value".into(), "203.0.113.10".into());
        params.insert("zone".into(), "zone-xyz".into());
        params.insert("ttl".into(), "60".into());

        let body = serde_json::json!({
            "type": "A", "name": "www", "content": "203.0.113.10", "ttl": 60
        });
        let resp = match crate::dns::proxy::ReqwestDnsClient.request(
            "POST",
            "/api/dns/zones/zone-xyz/records",
            "k2c_fake_token_for_test",
            Some("agent-add"),
            Some(&body.to_string()),
        ) {
            Ok(r) => {
                let (status, body) = map_proxy_response(&r);
                CliResponse {
                    status,
                    content_type: "application/json",
                    body,
                }
            }
            Err(e) => error_response("503 Service Unavailable", "proxy", &e),
        };

        match prev {
            Some(p) => std::env::set_var(crate::dns::proxy::DNS_API_BASE_ENV, p),
            None => std::env::remove_var(crate::dns::proxy::DNS_API_BASE_ENV),
        }

        assert!(
            resp.status.starts_with("201") || resp.status.starts_with("200"),
            "{}",
            resp.body
        );
        let req = seen.lock().unwrap().clone();
        assert!(
            req.contains("POST /api/dns/zones/zone-xyz/records"),
            "path:\n{req}"
        );
        assert!(
            req.to_ascii_lowercase()
                .contains("authorization: bearer k2c_fake_token_for_test"),
            "auth:\n{req}"
        );

        // Local NS reject still works independent of proxy.
        params.insert("type".into(), "NS".into());
        let deny = handle_record_add(&params);
        assert_eq!(deny.status, "400 Bad Request");

        cleanup(id);
    }
}
