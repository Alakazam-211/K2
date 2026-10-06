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

/// Current mail hostname for autoconfig targets: attached
/// `domain_names.role=mail` on this apex, else `mail_server.hostname`.
/// Never leave `mail.<connect>.k2.dev` on a custom apex (L2).
fn mail_hostname_for_apex(apex: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(names) = k2_core::domains::list_names_for_apex(&conn, apex) {
        if let Some(n) = names
            .into_iter()
            .find(|n| n.role.eq_ignore_ascii_case("mail"))
        {
            let h = n.hostname.trim().trim_end_matches('.').to_string();
            if !h.is_empty() {
                return Some(h);
            }
        }
    }
    domains::server_info(&conn).and_then(|i| i.hostname)
}

/// Advanced autoconfig rows from a zone file — never invent records
/// the live zone did not emit (`ZONE_FIXTURE` has no `_imaps._tcp`).
pub(crate) fn autoconfig_rows(
    domain: &str,
    zone_text: &str,
    hostname: Option<&str>,
) -> Vec<RecordRow> {
    let recs = domains::parse_zone_file(zone_text);
    let mut rows = domains::build_rows(domain, &recs, hostname);
    domains::apply_current_hostname_advanced(&mut rows, hostname);
    rows.into_iter()
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

/// The k2.dev records-API body for one zone-file row in zone `apex`.
///
/// - `name` is zone-RELATIVE (`@` for the apex). Zone-file rows carry the
///   FQDN, and posting that verbatim planted `_imaps._tcp.<apex>.<apex>`
///   (IT2). A row outside the zone is refused, never posted.
/// - MX/SRV: the API keeps the priority in its own `prio` field and
///   defaults it to 10, then serves `prio content`. Zone-file rdata leads
///   with the priority (`0 1 993 mail.example.com.`), so it moves to
///   `prio` — otherwise the served SRV would read `10 0 1 993 …`.
fn records_api_body(apex: &str, row: &RecordRow) -> Result<serde_json::Value, String> {
    let name = crate::dns::relative_record_name(&row.name, apex)?;
    let rtype = row.rtype.trim().to_ascii_uppercase();
    let mut content = row.expected.trim().to_string();
    let mut prio: Option<u16> = None;
    if rtype == "MX" || rtype == "SRV" {
        let (head, rest) = content
            .split_once(char::is_whitespace)
            .ok_or_else(|| format!("{rtype} {}: rdata '{content}' has no priority", row.name))?;
        let p: u16 = head
            .parse()
            .map_err(|_| format!("{rtype} {}: priority '{head}' is not a number", row.name))?;
        prio = Some(p);
        content = rest.trim().to_string();
    }
    let mut body = serde_json::json!({
        "type": rtype,
        "name": name,
        "content": content,
        "ttl": 3600,
    });
    if let Some(p) = prio {
        body["prio"] = serde_json::json!(p);
    }
    Ok(body)
}

/// POST one row through `post(path, body)`. Same wire as
/// POST /cli/dns/records/add (not A8 zones/bind).
fn plant_record_via(
    zone_id: &str,
    apex: &str,
    row: &RecordRow,
    post: &mut dyn FnMut(&str, &str) -> Result<crate::dns::proxy::DnsHttpResponse, String>,
) -> Result<(), String> {
    let body = records_api_body(apex, row)?;
    let path = format!("/api/dns/zones/{zone_id}/records");
    let resp = post(&path, &body.to_string())?;
    if resp.status == 200 || resp.status == 201 {
        Ok(())
    } else {
        Err(format!("{} {}: HTTP {}: {}", row.rtype, row.name, resp.status, resp.body))
    }
}

fn plant_record(zone_id: &str, apex: &str, row: &RecordRow) -> Result<(), String> {
    plant_record_via(zone_id, apex, row, &mut |path, body| {
        crate::dns::proxy::proxy_request("POST", path, None, Some(body))
    })
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
    let hostname = mail_hostname_for_apex(&domain);
    let rows = autoconfig_rows(&domain, &zone, hostname.as_deref());
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
    let hostname = mail_hostname_for_apex(&domain);
    let rows = autoconfig_rows(&domain, &zone, hostname.as_deref());
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
        match plant_record(&zone_id, &domain, row) {
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
        let rows = autoconfig_rows("acme.dev", ZONE_FIXTURE, Some("mail.acme.dev"));
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

    /// IT2: every planted row goes out zone-relative (never
    /// `<x>.<apex>.<apex>`), and MX/SRV priority rides `prio`.
    #[test]
    fn plant_record_posts_zone_relative_names() {
        let rows = autoconfig_rows("acme.dev", ZONE_FIXTURE, Some("mail.acme.dev"));
        assert!(rows.len() >= 3, "{rows:?}");
        let mut sent: Vec<(String, serde_json::Value)> = Vec::new();
        for row in &rows {
            plant_record_via("zone-1", "acme.dev", row, &mut |path, body| {
                sent.push((path.to_string(), serde_json::from_str(body).expect("JSON body")));
                Ok(crate::dns::proxy::DnsHttpResponse { status: 201, body: "{}".into() })
            })
            .expect("plant");
        }
        assert_eq!(sent.len(), rows.len());
        for (path, body) in &sent {
            assert_eq!(path, "/api/dns/zones/zone-1/records");
            let name = body["name"].as_str().expect("name");
            assert!(!name.contains("acme.dev"), "name must be zone-relative: {body}");
            assert!(!name.ends_with('.'), "{body}");
        }
        let by_name = |n: &str| {
            sent.iter()
                .map(|(_, b)| b)
                .find(|b| b["name"] == n)
                .unwrap_or_else(|| panic!("no planted row named {n}: {sent:?}"))
        };
        assert_eq!(by_name("autoconfig")["type"], "CNAME");
        assert_eq!(by_name("autoconfig")["content"], "mail.acme.dev.");
        assert!(by_name("autoconfig").get("prio").is_none());
        assert_eq!(by_name("autodiscover")["type"], "CNAME");
        let srv = by_name("_jmap._tcp");
        assert_eq!(srv["type"], "SRV");
        assert_eq!(srv["prio"], 0);
        assert_eq!(srv["content"], "1 443 mail.acme.dev.");
    }

    #[test]
    fn records_api_body_handles_apex_case_and_refuses_foreign_rows() {
        let row = |name: &str, rtype: &str, expected: &str| RecordRow {
            id: "adv:0".into(),
            category: CAT_ADVANCED.into(),
            rtype: rtype.into(),
            name: name.into(),
            purpose: String::new(),
            expected: expected.into(),
            expected_display: expected.into(),
            chunks: Vec::new(),
            status: String::new(),
            live: None,
            checked_at: None,
        };
        let b = records_api_body("example.com", &row("Example.COM.", "MX", "10 mail.example.com."))
            .expect("apex MX");
        assert_eq!(b["name"], "@");
        assert_eq!(b["prio"], 10);
        assert_eq!(b["content"], "mail.example.com.");
        let b = records_api_body(
            "example.com",
            &row("_imaps._tcp.example.com", "srv", "0 1 993 mail.example.com."),
        )
        .expect("srv");
        assert_eq!(b["name"], "_imaps._tcp");
        assert_eq!(b["type"], "SRV");
        assert_eq!(b["prio"], 0);
        assert_eq!(b["content"], "1 993 mail.example.com.");

        // Not under the apex: refused, and never posted.
        let foreign = row("autoconfig.example.net", "CNAME", "mail.example.com.");
        let mut posted = 0;
        let err = plant_record_via("zone-1", "example.com", &foreign, &mut |_, _| {
            posted += 1;
            Ok(crate::dns::proxy::DnsHttpResponse { status: 201, body: "{}".into() })
        })
        .expect_err("foreign row must be refused");
        assert!(err.contains("example.net"), "{err}");
        assert_eq!(posted, 0);

        // SRV rdata without a numeric priority is refused, not guessed.
        let bad = row("_imaps._tcp.example.com", "SRV", "x 1 993 mail.example.com.");
        assert!(records_api_body("example.com", &bad).is_err());

        // A non-2xx from the API names the row.
        let ok_row = row("autoconfig.example.com", "CNAME", "mail.example.com.");
        let err = plant_record_via("zone-1", "example.com", &ok_row, &mut |_, _| {
            Ok(crate::dns::proxy::DnsHttpResponse { status: 400, body: "bad name".into() })
        })
        .expect_err("400 must fail");
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("autoconfig.example.com"), "{err}");
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

    #[test]
    fn autoconfig_rewrites_connect_host_to_attached_mail_hostname() {
        let zone = "\
autoconfig.lztek.io. IN CNAME mail.lztek.k2.dev.\n\
autodiscover.lztek.io. IN CNAME mail.lztek.k2.dev.\n\
_imaps._tcp.lztek.io. IN SRV 0 1 993 mail.lztek.k2.dev.\n\
_jmap._tcp.lztek.io. IN SRV 0 1 443 mail.lztek.k2.dev.\n\
_submissions._tcp.lztek.io. IN SRV 0 1 465 mail.lztek.k2.dev.\n";
        let rows = autoconfig_rows("lztek.io", zone, Some("mail.lztek.io"));
        assert!(!rows.is_empty(), "{rows:?}");
        for r in &rows {
            assert!(
                !r.expected.to_ascii_lowercase().contains("lztek.k2.dev"),
                "must not plant Connect host: {} {}",
                r.name,
                r.expected
            );
            assert!(
                r.expected.to_ascii_lowercase().contains("mail.lztek.io"),
                "attached mail hostname: {} {}",
                r.name,
                r.expected
            );
        }
    }
}
