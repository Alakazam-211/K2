//! `GET|POST /cli/mail/dkim` — DKIM show / rotate; POST `/cli/mail/dkim/retire`.
//!
//! Extra-gate is the quota pattern (`is_mail_manage_surface`). Domain is a
//! hosted `mail_domains` row. Rotate mints Stalwart's Ed25519+RSA pair
//! (selector template, not `k2`+date) and does **not** destroy the previous
//! ids. Plant is the ACME records proxy iff `dnsWrite=1` already — never
//! A8 bind, never `/cli/dns/records/add`.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::dns::proxy::proxy_request;
use crate::mail::dns_verify::{self, DnsResolver, SystemResolver};
use crate::mail::domains::{self, RecordRow};
use crate::mail::jmap::{DkimSignature, StalwartClient};
use k2_core::db::schema::MailDomain;

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct DomainBody {
    domain: Option<String>,
    selector: Option<String>,
}

pub(crate) fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
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

pub(crate) fn hosted_domain(raw: &str) -> Result<MailDomain, CliResponse> {
    if raw.trim().is_empty() {
        return Err(err_json(
            "400 Bad Request",
            "usage",
            "missing 'domain' — which hosted mail domain?".to_string(),
        ));
    }
    let domain = match k2_core::mail_domain::normalize_mail_domain(raw) {
        Ok(d) => d,
        Err(h) => return Err(err_json("400 Bad Request", "usage", h)),
    };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::load_domain(&conn, &domain)
    };
    let Some(row) = row else {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("domain '{domain}' is not hosted here"),
        ));
    };
    let ready = row
        .stalwart_domain_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if ready.is_none() {
        return Err(err_json(
            "409 Conflict",
            "not_ready",
            format!("domain '{domain}' has no mail-server id yet"),
        ));
    }
    Ok(row)
}

pub(crate) fn engine() -> Result<StalwartClient, CliResponse> {
    match domains::engine_from_db() {
        Ok((c, _)) => Ok(c),
        Err(hint) => Err(err_json("503 Service Unavailable", "not_ready", hint)),
    }
}

fn engine_err(e: String) -> CliResponse {
    err_json("502 Bad Gateway", "engine", e)
}

fn dkim_row_for<'a>(rows: &'a [RecordRow], selector: &str) -> Option<&'a RecordRow> {
    let id = format!("dkim:{selector}");
    rows.iter().find(|r| r.id == id)
}

fn live_txt_match(name: &str, expected: &str) -> (Option<Vec<String>>, &'static str) {
    let Ok(resolver) = SystemResolver::new() else {
        return (None, "unknown");
    };
    match resolver.txt(name) {
        Ok(records) => {
            let live: Vec<String> = records.iter().map(|r| dns_verify::join_chunks(r)).collect();
            let hit = live
                .iter()
                .any(|l| dns_verify::norm_dkim(l) == dns_verify::norm_dkim(expected));
            (Some(live), if hit { "valid" } else { "wrong" })
        }
        Err(dns_verify::DnsError::NotFound) => (None, "missing"),
        Err(dns_verify::DnsError::Other(_)) => (None, "unknown"),
    }
}

fn sig_json(sig: &DkimSignature, rows: &[RecordRow]) -> serde_json::Value {
    let rec = dkim_row_for(rows, &sig.selector);
    let expected = rec.map(|r| r.expected.clone()).unwrap_or_default();
    let expected_display = rec.map(|r| r.expected_display.clone()).unwrap_or_default();
    let name = rec
        .map(|r| r.name.clone())
        .unwrap_or_else(|| format!("{}._domainkey", sig.selector));
    let (live, live_match) = if expected.is_empty() {
        (None, "unknown")
    } else {
        live_txt_match(&name, &expected)
    };
    serde_json::json!({
        "id": sig.id,
        "selector": sig.selector,
        "type": sig.type_name,
        "stage": sig.stage,
        "name": name,
        "expected": expected,
        "expectedDisplay": expected_display,
        "live": live,
        "match": live_match,
    })
}

/// GET `/cli/mail/dkim?domain=` — selector/stage table + expected TXT + live match.
pub fn handle_dkim_get(params: &HashMap<String, String>) -> CliResponse {
    let raw = crate::cli::str_param(params, "domain");
    let row = match hosted_domain(&raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = row.stalwart_domain_id.clone().unwrap_or_default();
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let sigs = match engine.dkim_list(&domain_id) {
        Ok(s) => s,
        Err(e) => return engine_err(e),
    };
    let recs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::effective_rows(&conn, &row)
    };
    let signatures: Vec<serde_json::Value> = sigs.iter().map(|s| sig_json(s, &recs)).collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "signatures": signatures,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/dkim` and `/cli/mail/dkim/rotate` `{domain}`.
pub fn handle_dkim_rotate(body: &[u8]) -> CliResponse {
    let b: DomainBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let raw = b.domain.as_deref().unwrap_or("");
    let row = match hosted_domain(raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = row.stalwart_domain_id.clone().unwrap_or_default();
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let plan = match engine.dkim_force_rotate(&domain_id) {
        Ok(p) => p,
        Err(e) => return engine_err(e),
    };
    if plan.minted.is_empty() {
        return engine_err("dkim rotate did not mint a new selector pair".to_string());
    }

    let zone = match poll_zone_for_selectors(&engine, &domain_id, &plan.minted) {
        Ok(z) => z,
        Err(e) => return engine_err(e),
    };
    let recs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match domains::refresh_stored_zone(&conn, &row, &zone) {
            Ok(st) => st.records,
            Err(e) => return engine_err(e),
        }
    };

    let mut txt_records = Vec::new();
    for sig in &plan.minted {
        let rec = dkim_row_for(&recs, &sig.selector);
        let Some(rec) = rec else {
            return engine_err(format!(
                "dnsZoneFile has no TXT for selector '{}' — not activating",
                sig.selector
            ));
        };
        txt_records.push(serde_json::json!({
            "name": rec.name,
            "type": "TXT",
            "value": rec.expected,
            "expectedDisplay": rec.expected_display,
        }));
    }

    let mut planted = false;
    let mut plant_error: Option<String> = None;
    let writable = apex_writable(&row.domain);
    if writable {
        planted = true;
        for rec in txt_records.iter() {
            let name = rec["name"].as_str().unwrap_or("");
            let value = rec["value"].as_str().unwrap_or("");
            let selector = name
                .split("._domainkey")
                .next()
                .unwrap_or("")
                .trim_end_matches('.');
            match plant_dkim_txt(&row.domain, selector, value) {
                Ok(true) => {}
                Ok(false) => {
                    planted = false;
                }
                Err(e) => {
                    planted = false;
                    plant_error = Some(e);
                    break;
                }
            }
        }
        if !planted {
            return CliResponse {
                status: "502 Bad Gateway",
                content_type: "application/json",
                body: serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "engine",
                        "hint": plant_error.unwrap_or_else(|| {
                            "dnsWrite apex did not plant DKIM TXT".to_string()
                        }),
                    },
                    "planted": false,
                    "records": txt_records,
                })
                .to_string(),
            };
        }
    }

    // TXT is in hand (zone file). Activate new; retire previous actives.
    let mut updates: Vec<(String, serde_json::Value)> = plan
        .minted
        .iter()
        .map(|s| (s.id.clone(), serde_json::json!({ "stage": "active" })))
        .collect();
    for old in plan
        .previous
        .iter()
        .filter(|s| s.stage == "active" || s.stage.is_empty())
    {
        updates.push((old.id.clone(), serde_json::json!({ "stage": "retiring" })));
    }
    if let Err(e) = engine.dkim_update(&updates) {
        return engine_err(e);
    }

    let signatures: Vec<serde_json::Value> = plan
        .minted
        .iter()
        .map(|s| {
            let mut v = sig_json(s, &recs);
            v["stage"] = serde_json::json!("active");
            v
        })
        .collect();
    let retiring: Vec<serde_json::Value> = plan
        .previous
        .iter()
        .filter(|s| s.stage == "active" || s.stage.is_empty())
        .map(|s| {
            let mut v = sig_json(s, &recs);
            v["stage"] = serde_json::json!("retiring");
            v
        })
        .collect();

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "planted": planted,
            "usedPemFallback": plan.used_pem_fallback,
            "signatures": signatures,
            "retiring": retiring,
            "records": txt_records,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/dkim/retire` `{domain, selector}`.
pub fn handle_dkim_retire(body: &[u8]) -> CliResponse {
    let b: DomainBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let raw = b.domain.as_deref().unwrap_or("");
    let selector = b.selector.as_deref().map(str::trim).unwrap_or("");
    if selector.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'selector' — which DKIM selector to retire?".to_string(),
        );
    }
    let row = match hosted_domain(raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = row.stalwart_domain_id.clone().unwrap_or_default();
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let sigs = match engine.dkim_list(&domain_id) {
        Ok(s) => s,
        Err(e) => return engine_err(e),
    };
    let Some(hit) = sigs.iter().find(|s| s.selector == selector) else {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("no DKIM selector '{selector}' on {}", row.domain),
        );
    };
    if let Err(e) = engine.dkim_destroy(&[hit.id.clone()]) {
        return engine_err(e);
    }
    let mut unplanted = false;
    if apex_writable(&row.domain) {
        match unplant_dkim_txt(&row.domain, selector) {
            Ok(b) => unplanted = b,
            Err(_) => unplanted = false,
        }
    }
    if let Ok(zone) = engine.domain_dns_zonefile(&domain_id) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = domains::refresh_stored_zone(&conn, &row, &zone);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "selector": selector,
            "retired": true,
            "planted": unplanted,
        })
        .to_string(),
    )
}

fn poll_zone_for_selectors(
    engine: &StalwartClient,
    domain_id: &str,
    minted: &[DkimSignature],
) -> Result<String, String> {
    let attempts = std::env::var("K2_DKIM_POLL_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &u32| *n > 0)
        .unwrap_or(20);
    let sleep_ms = std::env::var("K2_DKIM_POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500u64);
    let mut last = String::new();
    for i in 0..attempts {
        match engine.domain_dns_zonefile(domain_id) {
            Ok(zone) => {
                last = zone.clone();
                let ok = minted.iter().all(|s| {
                    zone.contains(&format!("{}._domainkey", s.selector))
                });
                if ok {
                    return Ok(zone);
                }
            }
            Err(e) => last = e,
        }
        if i + 1 < attempts && sleep_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
        }
    }
    if last.contains("._domainkey") {
        Ok(last)
    } else {
        Err(format!("dnsZoneFile missing new DKIM selectors: {last}"))
    }
}

fn apex_writable(domain: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::apex_attached_for_write(&conn, domain).unwrap_or(false)
}

fn plant_relative_name(selector: &str) -> String {
    format!("{selector}._domainkey")
}

fn plant_records_path(zone_id: &str) -> String {
    format!("/api/dns/zones/{zone_id}/records")
}

fn unplant_record_path(record_id: &str) -> String {
    format!("/api/dns/records/{record_id}")
}

/// ACME-shaped records proxy. Never `/api/dns/zones/bind`, never
/// `/cli/dns/records/add`.
fn plant_dkim_txt(domain: &str, selector: &str, value: &str) -> Result<bool, String> {
    let (zone_id, writable) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let b = k2_core::domains::get_binding(&conn, domain)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no domain binding".to_string())?;
        (b.zone_id.clone(), b.dns_write)
    };
    if !writable {
        return Ok(false);
    }
    let Some(zone_id) = zone_id.filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let body = serde_json::json!({
        "type": "TXT",
        "name": plant_relative_name(selector),
        "content": value,
        "ttl": 3600,
    })
    .to_string();
    let path = plant_records_path(&zone_id);
    let resp = proxy_request("POST", &path, Some("k2-hostmail-dkim"), Some(&body))?;
    if resp.status != 200 && resp.status != 201 {
        return Err(format!(
            "plant DKIM TXT failed HTTP {}: {}",
            resp.status, resp.body
        ));
    }
    Ok(true)
}

fn unplant_dkim_txt(domain: &str, selector: &str) -> Result<bool, String> {
    let (zone_id, writable) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let b = k2_core::domains::get_binding(&conn, domain)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no domain binding".to_string())?;
        (b.zone_id.clone(), b.dns_write)
    };
    if !writable {
        return Ok(false);
    }
    let Some(zone_id) = zone_id.filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let path = format!("/api/dns/zones/{zone_id}");
    let resp = proxy_request("GET", &path, Some("k2-hostmail-dkim"), None)?;
    if resp.status != 200 {
        return Ok(false);
    }
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or(serde_json::json!({}));
    let want = plant_relative_name(selector);
    let want_fqdn = format!("{want}.{domain}");
    let recs = v
        .get("records")
        .or_else(|| v.pointer("/zone/records"))
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let mut deleted = false;
    for rec in recs {
        let name = rec
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim_end_matches('.');
        let rtype = rec.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if rtype != "TXT" {
            continue;
        }
        if name != want && name != want_fqdn && name != selector {
            continue;
        }
        let Some(id) = rec.get("id").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) else {
            continue;
        };
        let del = proxy_request(
            "DELETE",
            &unplant_record_path(id),
            Some("k2-hostmail-dkim"),
            None,
        )?;
        if del.status == 200 || del.status == 204 || del.status == 404 {
            deleted = true;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dkim_get_without_domain_is_usage() {
        let r = handle_dkim_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
        assert!(r.body.contains("domain"), "{}", r.body);
    }

    #[test]
    fn dkim_get_unknown_domain_is_not_found() {
        let mut params = HashMap::new();
        params.insert(
            "domain".to_string(),
            "no-such-dkim-domain.example.test".to_string(),
        );
        let r = handle_dkim_get(&params);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
    }

    #[test]
    fn dkim_rotate_without_domain_is_usage() {
        let r = handle_dkim_rotate(b"{}");
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("domain"), "{}", r.body);
    }

    #[test]
    fn dkim_retire_without_selector_is_usage() {
        let r = handle_dkim_retire(br#"{"domain":"acme.dev"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("selector"), "{}", r.body);
    }

    #[test]
    fn hosted_domain_not_ready_without_stalwart_id() {
        let _g = crate::mail::mail_server_test_lock();
        let domain = format!("dkim-nr-{}.test", &uuid::Uuid::new_v4().to_string()[..8]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, status, send_mode, created_at) \
                 VALUES (?1, ?2, 'pending', 'receive-only', 1)",
                rusqlite::params![format!("dom-{domain}"), domain],
            )
            .expect("seed");
        }
        let err = hosted_domain(&domain).expect_err("not_ready");
        assert_eq!(err.status, "409 Conflict", "{}", err.body);
        assert!(err.body.contains("not_ready"), "{}", err.body);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", [&domain]);
        }
    }

    #[test]
    fn plant_relative_name_is_selector_domainkey() {
        assert_eq!(plant_relative_name("v1-ed25519-20260918"), "v1-ed25519-20260918._domainkey");
        assert!(!plant_relative_name("v1-rsa-20260918").contains("k2"));
    }

    #[test]
    fn plant_paths_are_records_proxy_not_bind_or_cli_dns() {
        let p = plant_records_path("zone-xyz");
        assert_eq!(p, "/api/dns/zones/zone-xyz/records");
        assert!(!p.contains("/bind"));
        assert!(!p.contains("/cli/dns"));
        let d = unplant_record_path("rec-1");
        assert_eq!(d, "/api/dns/records/rec-1");
        assert!(!d.contains("/bind"));
        assert!(!d.contains("/cli/dns/records/add"));
    }

    fn jmap_ok(method: &str, args: serde_json::Value) -> String {
        serde_json::json!({ "methodResponses": [[method, args, "0"]] }).to_string()
    }

    #[test]
    fn rotate_handler_byo_prints_txt_does_not_destroy_or_leak_pem() {
        let _g = crate::mail::mail_server_test_lock();
        std::env::set_var("K2_DKIM_POLL_ATTEMPTS", "1");
        std::env::set_var("K2_DKIM_POLL_MS", "0");
        std::env::set_var("K2_TEST_MAIL_API_KEY", "k2-test-key");
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let ed_sel = format!("v1-ed25519-{date}");
        let rsa_sel = format!("v1-rsa-{date}");
        let domain = format!("dkim-rot-{}.test", &uuid::Uuid::new_v4().to_string()[..8]);
        let session = r#"{"apiUrl":"/jmap/","accounts":{"b":{}},"primaryAccounts":{"urn:stalwart:jmap":"b"}}"#.to_string();
        let query = jmap_ok("x:DkimSignature/query", serde_json::json!({ "ids": ["dk1", "dk2"] }));
        let get = jmap_ok(
            "x:DkimSignature/get",
            serde_json::json!({
                "list": [
                    { "id": "dk1", "selector": "v1-ed25519-20260101", "@type": "Dkim1Ed25519Sha256", "stage": "active" },
                    { "id": "dk2", "selector": "v1-rsa-20260101", "@type": "Dkim1RsaSha256", "stage": "active" }
                ]
            }),
        );
        let update = jmap_ok(
            "x:DkimSignature/set",
            serde_json::json!({ "updated": { "dk1": null, "dk2": null } }),
        );
        let task_unknown = serde_json::json!({
            "methodResponses": [["error", { "type": "unknownMethod" }, "0"]]
        })
        .to_string();
        let create_ed = jmap_ok(
            "x:DkimSignature/set",
            serde_json::json!({ "created": { "k2": { "id": "dk3" } } }),
        );
        let create_rsa = jmap_ok(
            "x:DkimSignature/set",
            serde_json::json!({ "created": { "k2": { "id": "dk4" } } }),
        );
        let get_new = jmap_ok(
            "x:DkimSignature/get",
            serde_json::json!({
                "list": [
                    { "id": "dk3", "selector": ed_sel, "@type": "Dkim1Ed25519Sha256", "stage": "pending" },
                    { "id": "dk4", "selector": rsa_sel, "@type": "Dkim1RsaSha256", "stage": "pending" }
                ]
            }),
        );
        let zone = format!(
            "{ed_sel}._domainkey.{domain}. IN TXT \"v=DKIM1; k=ed25519; p=edkey\"\n\
             {rsa_sel}._domainkey.{domain}. IN TXT \"v=DKIM1; k=rsa; p=rsakey\"\n"
        );
        let zone_get = jmap_ok(
            "x:Domain/get",
            serde_json::json!({ "list": [{ "id": "sw-d", "dnsZoneFile": zone }] }),
        );
        let activate = jmap_ok(
            "x:DkimSignature/set",
            serde_json::json!({ "updated": { "dk3": null, "dk4": null, "dk1": null, "dk2": null } }),
        );
        let (port, rx) = crate::mail::jmap::tests::spawn_mock_server(vec![
            session, query, get, update, task_unknown, create_ed, create_rsa, get_new, zone_get,
            activate,
        ]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, api_url, api_key_ref, updated_at) \
                 VALUES (1, 'running', '0.16.10', ?1, 'env:K2_TEST_MAIL_API_KEY', 1)",
                rusqlite::params![format!("http://127.0.0.1:{port}")],
            )
            .expect("mail_server");
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, send_mode, created_at) \
                 VALUES (?1, ?2, 'sw-d', 'pending', 'receive-only', 1)",
                rusqlite::params![format!("dom-{domain}"), domain],
            )
            .expect("domain");
        }
        let body = serde_json::json!({ "domain": domain });
        let r = handle_dkim_rotate(body.to_string().as_bytes());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", [&domain]);
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        }
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert!(!r.body.contains("BEGIN"), "private key must not leak: {}", r.body);
        assert!(!r.body.contains("privateKey"), "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["planted"], false, "BYO / no dnsWrite: {}", r.body);
        assert_eq!(v["usedPemFallback"], true);
        let sel = v["signatures"][0]["selector"].as_str().unwrap_or("");
        assert!(!sel.starts_with("k2") || sel.contains('-'), "stalwart template, not k2+date: {sel}");
        assert!(!sel.chars().all(|c| c.is_ascii_digit() || c == 'k' || c == '2'));
        let k2date = format!("k2{date}");
        assert_ne!(sel, k2date);
        let mut saw_destroy = false;
        let mut saw_bind = false;
        let mut saw_cli_dns = false;
        let _ = rx.recv(); // session
        while let Ok(req) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
            if req.contains("\"destroy\"") && req.contains("x:DkimSignature/set") {
                saw_destroy = true;
            }
            if req.contains("/api/dns/zones/bind") {
                saw_bind = true;
            }
            if req.contains("/cli/dns/records/add") {
                saw_cli_dns = true;
            }
        }
        assert!(!saw_destroy, "rotate must not destroy the active pair");
        assert!(!saw_bind, "must not A8 bind");
        assert!(!saw_cli_dns, "must not /cli/dns/records/add");
    }
}
