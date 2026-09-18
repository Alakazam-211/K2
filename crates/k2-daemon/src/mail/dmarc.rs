//! `GET|POST /cli/mail/dmarc` — DMARC show / report-to.
//!
//! `report-to` writes `x:Domain/set` `reportAddressUri` only. Dest must
//! be an active hosted row on **that** domain (password-rotate lookup).
//! Does not rewrite `_dmarc` TXT.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::addresses::{self, AddrError};
use crate::mail::dkim::{self, err_json, hosted_domain};
use crate::mail::dns_verify::{self, DnsResolver, SystemResolver};
use crate::mail::domains::{self, CAT_REQUIRED};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct ReportToBody {
    domain: Option<String>,
    address: Option<String>,
}

fn dmarc_live(name: &str, expected: &str) -> (Option<Vec<String>>, &'static str) {
    let Ok(resolver) = SystemResolver::new() else {
        return (None, "unknown");
    };
    match resolver.txt(name) {
        Ok(records) => {
            let live: Vec<String> = records.iter().map(|r| dns_verify::join_chunks(r)).collect();
            let hit = live.iter().any(|l| {
                (dns_verify::dmarc_p_none(l) && dns_verify::dmarc_p_none(expected))
                    || dns_verify::norm_txt(l) == dns_verify::norm_txt(expected)
            });
            (Some(live), if hit { "valid" } else { "wrong" })
        }
        Err(dns_verify::DnsError::NotFound) => (None, "missing"),
        Err(dns_verify::DnsError::Other(_)) => (None, "unknown"),
    }
}

fn suggested_rua(domain: &str, report_uri: Option<&str>) -> String {
    let rua = match report_uri {
        Some(u) if u.starts_with("mailto:") && u != "mailto:postmaster" => u,
        _ => {
            return format!("v=DMARC1; p=none; rua=mailto:postmaster@{domain}");
        }
    };
    format!("v=DMARC1; p=none; rua={rua}")
}

fn uri_is_minted_inbox(uri: Option<&str>) -> bool {
    let Some(u) = uri else {
        return false;
    };
    let Some(addr) = u.strip_prefix("mailto:") else {
        return false;
    };
    if addr == "postmaster" || !addr.contains('@') {
        return false;
    }
    let Ok(norm) = addresses::normalize_address(addr) else {
        return false;
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    addresses::load_address(&conn, &norm).is_some_and(|r| r.status == "active")
}

/// GET `/cli/mail/dmarc?domain=` — live `_dmarc` + reportAddressUri +
/// suggested `rua=` line (not planted).
pub fn handle_dmarc_get(params: &HashMap<String, String>) -> CliResponse {
    let raw = crate::cli::str_param(params, "domain");
    let row = match hosted_domain(&raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = row.stalwart_domain_id.clone().unwrap_or_default();
    let engine = match dkim::engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let report_uri = match engine.domain_get_report_address_uri(&domain_id) {
        Ok(u) => u,
        Err(e) => {
            return err_json("502 Bad Gateway", "engine", e);
        }
    };
    let recs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::effective_rows(&conn, &row)
    };
    let dmarc = recs.iter().find(|r| r.id == "dmarc" && r.category == CAT_REQUIRED);
    let name = dmarc
        .map(|r| r.name.clone())
        .unwrap_or_else(|| format!("_dmarc.{}", row.domain));
    let expected = dmarc.map(|r| r.expected.clone()).unwrap_or_else(|| {
        format!("v=DMARC1; p=none; rua=mailto:postmaster@{}", row.domain)
    });
    let (live, live_match) = dmarc_live(&name, &expected);
    let uri_str = report_uri.clone().unwrap_or_else(|| "mailto:postmaster".to_string());
    let minted = uri_is_minted_inbox(report_uri.as_deref());
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "dmarc": {
                "name": name,
                "expected": expected,
                "live": live,
                "match": live_match,
            },
            "reportAddressUri": uri_str,
            "reportAddressMinted": minted,
            "suggestedRua": suggested_rua(&row.domain, report_uri.as_deref()),
            "note": if minted {
                None
            } else {
                Some("reportAddressUri is not a minted inbox on this domain (Stalwart default mailto:postmaster). Use k2 hostmail dmarc report-to.")
            },
        })
        .to_string(),
    )
}

/// POST `/cli/mail/dmarc` and `/cli/mail/dmarc/report-to`
/// `{domain, address}`.
pub fn handle_dmarc_report_to(body: &[u8]) -> CliResponse {
    let b: ReportToBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let raw_domain = b.domain.as_deref().unwrap_or("");
    let raw_addr = b.address.as_deref().unwrap_or("");
    if raw_addr.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'address' — minted inbox that receives DMARC reports".to_string(),
        );
    }
    let row = match hosted_domain(raw_domain) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let address = match addresses::normalize_address(raw_addr) {
        Ok(a) => a,
        Err(AddrError::Usage(h)) => return err_json("400 Bad Request", "usage", h),
        Err(e) => return err_json("400 Bad Request", "usage", format!("{e:?}")),
    };
    let addr_row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        addresses::load_address(&conn, &address)
    };
    let Some(addr_row) = addr_row else {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted address '{address}'"),
        );
    };
    if addr_row.status != "active" {
        return err_json(
            "404 Not Found",
            "not_found",
            format!("no hosted address '{address}'"),
        );
    }
    if addr_row.domain_id != row.id {
        return err_json(
            "400 Bad Request",
            "usage",
            format!("address '{address}' is not on hosted domain '{}'", row.domain),
        );
    }

    let dmarc_expected_before = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::effective_rows(&conn, &row)
            .into_iter()
            .find(|r| r.id == "dmarc")
            .map(|r| r.expected)
    };

    let domain_id = row.stalwart_domain_id.clone().unwrap_or_default();
    let engine = match dkim::engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let uri = format!("mailto:{address}");
    if let Err(e) = engine.domain_set_report_address_uri(&domain_id, Some(&uri)) {
        return err_json("502 Bad Gateway", "engine", e);
    }

    let dmarc_expected_after = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::effective_rows(&conn, &row)
            .into_iter()
            .find(|r| r.id == "dmarc")
            .map(|r| r.expected)
    };
    debug_assert_eq!(dmarc_expected_before, dmarc_expected_after);

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "reportAddressUri": uri,
            "suggestedRua": suggested_rua(&row.domain, Some(&uri)),
            "dmarcExpected": dmarc_expected_after,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmarc_get_without_domain_is_usage() {
        let r = handle_dmarc_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn dmarc_get_unknown_domain_is_not_found() {
        let mut params = HashMap::new();
        params.insert(
            "domain".to_string(),
            "no-such-dmarc-domain.example.test".to_string(),
        );
        let r = handle_dmarc_get(&params);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }

    #[test]
    fn report_to_without_address_is_usage() {
        let r = handle_dmarc_report_to(br#"{"domain":"acme.dev"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn report_to_unminted_is_not_found() {
        let _g = crate::mail::mail_server_test_lock();
        let domain = format!("dmarc-um-{}.test", &uuid::Uuid::new_v4().to_string()[..8]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, send_mode, created_at) \
                 VALUES (?1, ?2, 'sw-d', 'pending', 'receive-only', 1)",
                rusqlite::params![format!("dom-{domain}"), domain],
            )
            .expect("seed domain");
        }
        let body = serde_json::json!({
            "domain": domain,
            "address": format!("nobody@{domain}"),
        });
        let r = handle_dmarc_report_to(body.to_string().as_bytes());
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", [&domain]);
        }
    }

    #[test]
    fn report_to_other_domain_is_usage() {
        let _g = crate::mail::mail_server_test_lock();
        let a = format!("dmarc-a-{}.test", &uuid::Uuid::new_v4().to_string()[..8]);
        let b = format!("dmarc-b-{}.test", &uuid::Uuid::new_v4().to_string()[..8]);
        let addr = format!("bot@{b}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, send_mode, created_at) \
                 VALUES (?1, ?2, 'sw-a', 'pending', 'receive-only', 1)",
                rusqlite::params![format!("dom-{a}"), a],
            )
            .expect("seed a");
            conn.execute(
                "INSERT INTO mail_domains (id, domain, stalwart_domain_id, status, send_mode, created_at) \
                 VALUES (?1, ?2, 'sw-b', 'pending', 'receive-only', 1)",
                rusqlite::params![format!("dom-{b}"), b],
            )
            .expect("seed b");
            conn.execute(
                "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
                 owner_project_id, status, created_at) VALUES (?1, ?2, ?3, 'acc', 'p', 'active', 1)",
                rusqlite::params![format!("addr-{addr}"), addr, format!("dom-{b}")],
            )
            .expect("seed addr");
        }
        let body = serde_json::json!({ "domain": a, "address": addr });
        let r = handle_dmarc_report_to(body.to_string().as_bytes());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("not on hosted domain"), "{}", r.body);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_addresses WHERE address = ?1", [&addr]);
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", [&a]);
            let _ = conn.execute("DELETE FROM mail_domains WHERE domain = ?1", [&b]);
        }
    }

    #[test]
    fn suggested_rua_uses_mailto_and_leaves_p_none() {
        let s = suggested_rua("acme.dev", Some("mailto:dmarc@acme.dev"));
        assert!(s.contains("p=none"), "{s}");
        assert!(s.contains("rua=mailto:dmarc@acme.dev"), "{s}");
        let def = suggested_rua("acme.dev", Some("mailto:postmaster"));
        assert!(def.contains("postmaster@acme.dev"), "{def}");
    }
}
