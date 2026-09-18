//! Autoconfig / Autodiscover DNS from `x:Domain` `dnsZoneFile`.
//!
//! show = advanced rows Stalwart already emits. apply = plant via
//! `POST /cli/dns/records/add` when the apex is on `GET /cli/dns/zones`.
//! Not A8 `zones/bind`. BYO prints.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::domains::{self, RecordRow, CAT_ADVANCED};
use crate::mail::hosted::{self, err_json};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ApplyBody {
    domain: Option<String>,
}

fn is_autoconfig_row(row: &RecordRow) -> bool {
    let n = row.name.to_ascii_lowercase();
    n.contains("autoconfig")
        || n.contains("autodiscover")
        || n.contains("_imap._tcp")
        || n.contains("_imaps._tcp")
        || n.contains("_submission._tcp")
        || n.contains("_submissions._tcp")
        || n.contains("_jmap._tcp")
        || n.contains("ua-auto-config")
}

/// Advanced autoconfig rows from a zone file — never invent records
/// the live zone did not emit (`ZONE_FIXTURE` has no `_imaps._tcp`).
pub(crate) fn autoconfig_rows(domain: &str, zone_text: &str) -> Vec<RecordRow> {
    let recs = domains::parse_zone_file(zone_text);
    domains::build_rows(domain, &recs, None)
        .into_iter()
        .filter(|r| r.category == CAT_ADVANCED && is_autoconfig_row(r))
        .collect()
}

fn record_json(row: &RecordRow) -> serde_json::Value {
    serde_json::json!({
        "name": row.name,
        "type": row.rtype,
        "value": row.expected,
        "display": row.expected_display,
        "purpose": row.purpose,
    })
}

fn show_payload(domain: &str, rows: &[RecordRow]) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "domain": domain,
        "records": rows.iter().map(record_json).collect::<Vec<_>>(),
        "thunderbirdUrl": format!("http://autoconfig.{domain}/mail/config-v1.1.xml"),
        "autodiscoverUrl": format!("https://autodiscover.{domain}/autodiscover/autodiscover.xml"),
    })
}

fn hosted_domain_zone(domain: &str) -> Result<(String, String, String), CliResponse> {
    let domain = k2_core::mail_domain::normalize_mail_domain(domain)
        .map_err(|h| err_json("400 Bad Request", "usage", h))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::load_domain(&conn, &domain)
    };
    let Some(row) = row else {
        return Err(err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted domain '{domain}'"),
        ));
    };
    let stw = row
        .stalwart_domain_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            err_json(
                "409 Conflict",
                "not_ready",
                format!("domain '{domain}' has no mail-server id"),
            )
        })?;
    Ok((domain, stw, row.dns_status_json.clone().unwrap_or_default()))
}

fn zonefile_for(domain: &str, stw_id: &str) -> Result<String, CliResponse> {
    let (engine, _) = hosted::engine()?;
    engine
        .domain_dns_zonefile(stw_id)
        .map_err(|e| err_json("502 Bad Gateway", "engine", e))
        .or_else(|e| {
            // Fall back to the stored table only if the engine is down
            // after we already required it — still fail loud.
            let _ = domain;
            Err(e)
        })
}

fn lookup_dns_zone_id(domain: &str) -> Option<String> {
    use crate::dns::proxy::proxy_request;
    let resp = proxy_request("GET", "/api/dns/zones", None, None).ok()?;
    if resp.status != 200 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&resp.body).ok()?;
    let want = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    v.get("zones")
        .and_then(|z| z.as_array())
        .into_iter()
        .flatten()
        .find_map(|z| {
            let d = z
                .get("domain")
                .or_else(|| z.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if d == want {
                z.get("id")
                    .or_else(|| z.get("zone"))
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            } else {
                None
            }
        })
}

fn plant_record(zone_id: &str, row: &RecordRow) -> Result<(), String> {
    // Same wire as POST /cli/dns/records/add (not A8 zones/bind).
    let body = serde_json::json!({
        "type": row.rtype,
        "name": row.name,
        "content": row.expected,
        "ttl": 3600,
    });
    let path = format!("/api/dns/zones/{zone_id}/records");
    let resp = crate::dns::proxy::proxy_request("POST", &path, None, Some(&body.to_string()))?;
    if resp.status == 200 || resp.status == 201 {
        Ok(())
    } else {
        Err(format!("HTTP {}: {}", resp.status, resp.body))
    }
}

/// GET `/cli/mail/autoconfig?domain=`
pub fn handle_autoconfig_get(params: &HashMap<String, String>) -> CliResponse {
    let domain = crate::cli::str_param(params, "domain");
    if domain.trim().is_empty() {
        return err_json("400 Bad Request", "usage", "missing 'domain'".to_string());
    }
    let (domain, stw, _) = match hosted_domain_zone(&domain) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let zone = match zonefile_for(&domain, &stw) {
        Ok(z) => z,
        Err(r) => return r,
    };
    let rows = autoconfig_rows(&domain, &zone);
    CliResponse::ok_json(show_payload(&domain, &rows).to_string())
}

/// POST `/cli/mail/autoconfig` `{domain}` — plant if we host NS.
pub fn handle_autoconfig_apply(body: &[u8]) -> CliResponse {
    let b: ApplyBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                "400 Bad Request",
                "usage",
                format!("invalid JSON body: {e}"),
            )
        }
    };
    let domain = b.domain.as_deref().unwrap_or("").trim();
    if domain.is_empty() {
        return err_json("400 Bad Request", "usage", "missing 'domain'".to_string());
    }
    let (domain, stw, _) = match hosted_domain_zone(domain) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let zone = match zonefile_for(&domain, &stw) {
        Ok(z) => z,
        Err(r) => return r,
    };
    let rows = autoconfig_rows(&domain, &zone);
    let mut payload = show_payload(&domain, &rows);
    payload["bind"] = serde_json::json!(false);
    let Some(zone_id) = lookup_dns_zone_id(&domain) else {
        payload["planted"] = serde_json::json!(false);
        payload["hint"] = serde_json::json!(
            "this server does not host NS for this zone — print these records at your registrar (not A8 bind)"
        );
        return CliResponse::ok_json(payload.to_string());
    };
    let mut planted = Vec::new();
    let mut errors = Vec::new();
    for row in &rows {
        match plant_record(&zone_id, row) {
            Ok(()) => planted.push(record_json(row)),
            Err(e) => errors.push(e),
        }
    }
    payload["planted"] = serde_json::json!(true);
    payload["plantedRecords"] = serde_json::json!(planted);
    if !errors.is_empty() {
        payload["ok"] = serde_json::json!(false);
        payload["error"] = serde_json::json!({
            "code": "engine",
            "hint": errors.join("; "),
        });
        return CliResponse {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: payload.to_string(),
        };
    }
    CliResponse::ok_json(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::domains::tests::ZONE_FIXTURE;

    #[test]
    fn fixture_rows_match_live_zone_not_invented_imaps() {
        let rows = autoconfig_rows("acme.dev", ZONE_FIXTURE);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("autoconfig")), "{names:?}");
        assert!(
            names.iter().any(|n| n.contains("autodiscover")),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.contains("_jmap._tcp")), "{names:?}");
        assert!(
            !names.iter().any(|n| n.contains("_imaps._tcp")),
            "do not invent _imaps._tcp: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("mta-sts")),
            "MTA-STS is advanced but not autoconfig apply: {names:?}"
        );
    }

    #[test]
    fn get_without_domain_is_usage_not_405() {
        let r = handle_autoconfig_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn apply_without_domain_is_usage() {
        let r = handle_autoconfig_apply(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn apply_unknown_domain_is_not_found_not_bind() {
        let r = handle_autoconfig_apply(br#"{"domain":"no-such-autoconfig.test"}"#);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        assert!(!r.body.contains("zones/bind"), "{}", r.body);
    }
}
