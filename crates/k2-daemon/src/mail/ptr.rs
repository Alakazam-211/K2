//! `GET|POST /cli/mail/ptr` (show) and `POST /cli/mail/ptr/set`.
//!
//! Show: origin IPv4 (what-is-my-ip), live rDNS, HELO from
//! `mail_server.hostname`, A of that hostname, and alignment flags.
//! No OVH on the box. Never Connect VIP.
//!
//! Set: normalize hostname (same as enable), refuse unless public A
//! includes origin, Stalwart `set_server_hostname` + UPDATE
//! `mail_server.hostname`, then CP `POST /api/dns/ptr` via
//! `dns::proxy` Bearer k2c_. Do not call CP if HELO fails. Do not use
//! raw `map_proxy_response` (it maps 501→502) — surface CP 501
//! `ptr_provider` and 403 tunnel_only/unbound as-is.
//!
//! Extra-gate is the quota pattern (`is_mail_manage_surface` +
//! `post_allowed`). GET `/cli/mail/ptr/set` → 405.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::cli_response::CliResponse;
use crate::dns::proxy::{self, DnsHttpResponse};
use crate::mail::dns_verify::{DnsError, DnsResolver, SystemResolver};
use crate::mail::preflight::{PreflightEnv, RealPreflightEnv};

/// Connect edge VIP — never treat as origin for PTR/HELO.
const CONNECT_VIP: Ipv4Addr = Ipv4Addr::new(178, 156, 232, 105);

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
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

fn http_status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        201 => "201 Created",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ if (500..600).contains(&code) => "502 Bad Gateway",
        _ => "502 Bad Gateway",
    }
}

/// Map a CP PTR response without `map_proxy_response` (which collapses
/// 501 → 502). Preserves 501 `ptr_provider` and 403 tunnel_only/unbound.
pub(crate) fn map_ptr_cp_response(resp: &DnsHttpResponse) -> CliResponse {
    match resp.status {
        200 | 201 => {
            let body = if resp.body.trim_start().starts_with('{')
                || resp.body.trim_start().starts_with('[')
            {
                resp.body.clone()
            } else {
                serde_json::json!({ "ok": true, "body": resp.body }).to_string()
            };
            CliResponse {
                status: http_status_line(resp.status),
                content_type: "application/json",
                body,
            }
        }
        501 => {
            // Pass CP JSON through (code/provider/hint).
            let body = if resp.body.trim_start().starts_with('{') {
                resp.body.clone()
            } else {
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "ptr_provider",
                        "hint": resp.body.trim(),
                    }
                })
                .to_string()
            };
            CliResponse {
                status: "501 Not Implemented",
                content_type: "application/json",
                body,
            }
        }
        403 => {
            let body = if resp.body.trim_start().starts_with('{') {
                resp.body.clone()
            } else {
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "forbidden",
                        "hint": resp.body.trim(),
                    }
                })
                .to_string()
            };
            CliResponse {
                status: "403 Forbidden",
                content_type: "application/json",
                body,
            }
        }
        401 => err_json(
            "401 Unauthorized",
            "unauthorized",
            "tunnel token rejected by PTR API — re-pair K2 Connect".into(),
        ),
        404 => {
            let hint = parse_cp_hint(&resp.body)
                .unwrap_or_else(|| "no K2X servers row for this box — PTR write refused".into());
            let body = if resp.body.trim_start().starts_with('{') {
                resp.body.clone()
            } else {
                serde_json::json!({
                    "ok": false,
                    "error": { "code": "not_found", "hint": hint },
                })
                .to_string()
            };
            CliResponse {
                status: "404 Not Found",
                content_type: "application/json",
                body,
            }
        }
        409 => {
            let body = if resp.body.trim_start().starts_with('{') {
                resp.body.clone()
            } else {
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "conflict",
                        "hint": parse_cp_hint(&resp.body).unwrap_or_else(|| resp.body.clone()),
                    }
                })
                .to_string()
            };
            CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body,
            }
        }
        429 => err_json(
            "429 Too Many Requests",
            "rate_limited",
            "PTR change rate limit reached — retry in a few minutes".into(),
        ),
        s => {
            let hint = parse_cp_hint(&resp.body)
                .unwrap_or_else(|| format!("PTR API error (HTTP {s})"));
            err_json(http_status_line(s), "upstream", hint)
        }
    }
}

fn parse_cp_hint(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.pointer("/error/hint")
        .and_then(|h| h.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("error")
                .and_then(|e| e.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            v.get("hint")
                .and_then(|h| h.as_str())
                .map(|s| s.to_string())
        })
}

fn mail_hostname() -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    crate::mail::domains::server_info(&conn)
        .and_then(|i| i.hostname)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn persist_hostname(hostname: &str) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let n = conn
        .execute(
            "UPDATE mail_server SET hostname = ?1, updated_at = ?2 WHERE id = 1",
            rusqlite::params![hostname, now],
        )
        .map_err(|e| format!("mail_server hostname update: {e}"))?;
    if n == 0 {
        return Err(
            "mail_server row missing — enable hostmail before ptr set".to_string(),
        );
    }
    Ok(())
}

fn origin_ipv4(env: &dyn PreflightEnv) -> Result<Ipv4Addr, CliResponse> {
    let raw = env.public_ip().ok_or_else(|| {
        err_json(
            "502 Bad Gateway",
            "origin_ip",
            "could not determine origin IPv4 (checkip.amazonaws.com / ipify)".into(),
        )
    })?;
    let ip: Ipv4Addr = raw.trim().parse().map_err(|_| {
        err_json(
            "502 Bad Gateway",
            "origin_ip",
            format!("public IP probe returned non-IPv4: {raw}"),
        )
    })?;
    if ip == CONNECT_VIP {
        return Err(err_json(
            "502 Bad Gateway",
            "origin_ip",
            "origin IPv4 resolved to Connect VIP 178.156.232.105 — refuse \
             (PTR must be the box public address, never the edge VIP)"
                .into(),
        ));
    }
    Ok(ip)
}

fn normalize_hostname(raw: &str) -> Result<String, CliResponse> {
    match k2_core::mail_domain::normalize_mail_domain(raw) {
        Ok(h) => Ok(h),
        Err(e) => Err(err_json(
            "400 Bad Request",
            "usage",
            format!("invalid hostname: {e}"),
        )),
    }
}

fn eq_host(a: &str, b: &str) -> bool {
    a.trim()
        .trim_end_matches('.')
        .eq_ignore_ascii_case(b.trim().trim_end_matches('.'))
}

/// Build the show payload (injectable resolver + origin).
pub(crate) fn ptr_show_json(
    origin: Ipv4Addr,
    helo: Option<&str>,
    resolver: &dyn DnsResolver,
) -> serde_json::Value {
    let ptr_names = match resolver.ptr(IpAddr::V4(origin)) {
        Ok(v) => v,
        Err(DnsError::NotFound) => Vec::new(),
        Err(_) => Vec::new(),
    };
    let ptr = ptr_names.first().cloned();

    let (a_records, a_lookup_ok) = match helo.filter(|h| !h.is_empty()) {
        Some(h) => match resolver.a(h) {
            Ok(v) => (v, true),
            Err(DnsError::NotFound) => (Vec::new(), true),
            Err(_) => (Vec::new(), false),
        },
        None => (Vec::new(), false),
    };

    let ptr_matches_helo = match (ptr.as_deref(), helo) {
        (Some(p), Some(h)) if !h.is_empty() => eq_host(p, h),
        _ => false,
    };
    let a_matches_origin = a_lookup_ok && a_records.iter().any(|a| *a == origin);
    let aligned = ptr_matches_helo && a_matches_origin;

    serde_json::json!({
        "ok": true,
        "originIpv4": origin.to_string(),
        "ptr": ptr,
        "helo": helo.filter(|h| !h.is_empty()),
        "hostname": helo.filter(|h| !h.is_empty()),
        "aRecords": a_records.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        "ptrMatchesHelo": ptr_matches_helo,
        "aMatchesOrigin": a_matches_origin,
        "aligned": aligned,
    })
}

/// GET|POST `/cli/mail/ptr` — show alignment (no OVH).
pub fn handle_ptr_show(_params: &HashMap<String, String>) -> CliResponse {
    let env = RealPreflightEnv;
    let resolver = match SystemResolver::public().or_else(|_| SystemResolver::new()) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                "502 Bad Gateway",
                "dns",
                format!("DNS resolver unavailable: {e}"),
            )
        }
    };
    handle_ptr_show_with(&env, &resolver)
}

pub(crate) fn handle_ptr_show_with(
    env: &dyn PreflightEnv,
    resolver: &dyn DnsResolver,
) -> CliResponse {
    let origin = match origin_ipv4(env) {
        Ok(ip) => ip,
        Err(r) => return r,
    };
    let helo = mail_hostname();
    let body = ptr_show_json(origin, helo.as_deref(), resolver);
    CliResponse::ok_json(body.to_string())
}

/// POST `/cli/mail/ptr` is also show (dual method, like a read that
/// agents may POST). Body ignored.
pub fn handle_ptr_show_post(_body: &[u8]) -> CliResponse {
    handle_ptr_show(&HashMap::new())
}

/// Injectable seams for set (tests never dial Stalwart or CP).
pub(crate) trait PtrSetDeps: Send {
    fn set_helo(&mut self, hostname: &str) -> Result<(), String>;
    fn persist_hostname(&mut self, hostname: &str) -> Result<(), String>;
    fn call_cp(&mut self, hostname: &str) -> Result<DnsHttpResponse, String>;
}

struct LivePtrSetDeps;

impl PtrSetDeps for LivePtrSetDeps {
    fn set_helo(&mut self, hostname: &str) -> Result<(), String> {
        let (engine, _) = crate::mail::domains::engine_from_db()?;
        engine.set_server_hostname(hostname)
    }

    fn persist_hostname(&mut self, hostname: &str) -> Result<(), String> {
        persist_hostname(hostname)
    }

    fn call_cp(&mut self, hostname: &str) -> Result<DnsHttpResponse, String> {
        let body = serde_json::json!({
            "hostname": hostname,
            "helo": hostname,
        })
        .to_string();
        proxy::proxy_request(
            "POST",
            "/api/dns/ptr",
            Some("k2-hostmail-ptr"),
            Some(&body),
        )
    }
}

/// POST `/cli/mail/ptr/set` `{hostname}`.
pub fn handle_ptr_set(body: &[u8]) -> CliResponse {
    let env = RealPreflightEnv;
    let resolver = match SystemResolver::public().or_else(|_| SystemResolver::new()) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                "502 Bad Gateway",
                "dns",
                format!("DNS resolver unavailable: {e}"),
            )
        }
    };
    let mut deps = LivePtrSetDeps;
    handle_ptr_set_with(body, &env, &resolver, &mut deps)
}

pub(crate) fn handle_ptr_set_with(
    body: &[u8],
    env: &dyn PreflightEnv,
    resolver: &dyn DnsResolver,
    deps: &mut dyn PtrSetDeps,
) -> CliResponse {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let raw = parsed
        .get("hostname")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'hostname' — the mail hostname to set as HELO/PTR \
             (e.g. mail.acme.dev)"
                .into(),
        );
    }
    let hostname = match normalize_hostname(raw) {
        Ok(h) => h,
        Err(r) => return r,
    };

    let origin = match origin_ipv4(env) {
        Ok(ip) => ip,
        Err(r) => return r,
    };

    let a_records = match resolver.a(&hostname) {
        Ok(v) => v,
        Err(DnsError::NotFound) => Vec::new(),
        Err(e) => {
            return err_json(
                "502 Bad Gateway",
                "dns",
                format!("A lookup for '{hostname}' failed: {e:?}"),
            )
        }
    };
    if !a_records.iter().any(|a| *a == origin) {
        return err_json(
            "400 Bad Request",
            "a_mismatch",
            format!(
                "public A({hostname}) does not include origin IPv4 {origin} \
                 (got [{}]) — plant the A first, then retry ptr set",
                a_records
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }

    if let Err(e) = deps.set_helo(&hostname) {
        return err_json(
            "502 Bad Gateway",
            "helo",
            format!("Stalwart set_server_hostname failed (CP not called): {e}"),
        );
    }
    if let Err(e) = deps.persist_hostname(&hostname) {
        return err_json(
            "502 Bad Gateway",
            "helo",
            format!("persisted HELO failed after Stalwart update (CP not called): {e}"),
        );
    }

    match deps.call_cp(&hostname) {
        Ok(resp) => {
            let mut mapped = map_ptr_cp_response(&resp);
            // On success, enrich with local alignment snapshot.
            if resp.status == 200 || resp.status == 201 {
                let show = ptr_show_json(origin, Some(&hostname), resolver);
                if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&mapped.body) {
                    if v.get("ok").is_none() {
                        v["ok"] = serde_json::json!(true);
                    }
                    v["hostname"] = serde_json::json!(hostname);
                    v["helo"] = serde_json::json!(hostname);
                    v["originIpv4"] = show["originIpv4"].clone();
                    v["ptr"] = show["ptr"].clone();
                    v["aRecords"] = show["aRecords"].clone();
                    v["ptrMatchesHelo"] = show["ptrMatchesHelo"].clone();
                    v["aMatchesOrigin"] = show["aMatchesOrigin"].clone();
                    v["aligned"] = show["aligned"].clone();
                    mapped.body = v.to_string();
                }
            }
            mapped
        }
        Err(e) => {
            if e.contains("no tunnel token") || e.contains("tunnel.json") {
                err_json(
                    "401 Unauthorized",
                    "unpaired",
                    format!(
                        "no tunnel token in ~/.k2/tunnel.json — pair K2 Connect \
                         before ptr set ({e})"
                    ),
                )
            } else {
                err_json(
                    "502 Bad Gateway",
                    "upstream",
                    format!("PTR CP proxy failed: {e}"),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::dns_verify::MxHost;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeEnv {
        ip: Option<String>,
    }
    impl PreflightEnv for FakeEnv {
        fn is_linux(&self) -> bool {
            true
        }
        fn port_listener(&self, _port: u16) -> Option<String> {
            None
        }
        fn public_ip(&self) -> Option<String> {
            self.ip.clone()
        }
        fn rdns(&self, _ip: &str) -> Option<String> {
            None
        }
        fn outbound_25(&self) -> Result<(), String> {
            Ok(())
        }
        fn disk_free_bytes(&self) -> Option<u64> {
            Some(50 << 30)
        }
        fn total_ram_bytes(&self) -> Option<u64> {
            Some(8 << 30)
        }
    }

    #[derive(Default)]
    struct FakeDns {
        a: HashMap<String, Vec<Ipv4Addr>>,
        ptr: HashMap<String, Vec<String>>,
        a_err: HashMap<String, DnsError>,
    }
    impl DnsResolver for FakeDns {
        fn mx(&self, _name: &str) -> Result<Vec<MxHost>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn txt(&self, _name: &str) -> Result<Vec<Vec<String>>, DnsError> {
            Err(DnsError::NotFound)
        }
        fn a(&self, name: &str) -> Result<Vec<Ipv4Addr>, DnsError> {
            let key = name.trim_end_matches('.').to_ascii_lowercase();
            if let Some(e) = self.a_err.get(&key) {
                return Err(e.clone());
            }
            self.a
                .get(&key)
                .cloned()
                .ok_or(DnsError::NotFound)
        }
        fn ptr(&self, ip: IpAddr) -> Result<Vec<String>, DnsError> {
            self.ptr
                .get(&ip.to_string())
                .cloned()
                .ok_or(DnsError::NotFound)
        }
    }

    struct RecordingDeps {
        helo_calls: Arc<AtomicUsize>,
        cp_calls: Arc<AtomicUsize>,
        helo_fail: bool,
        persist_fail: bool,
        cp_resp: Option<DnsHttpResponse>,
        cp_err: Option<String>,
        last_helo: Arc<Mutex<Option<String>>>,
    }
    impl PtrSetDeps for RecordingDeps {
        fn set_helo(&mut self, hostname: &str) -> Result<(), String> {
            self.helo_calls.fetch_add(1, Ordering::SeqCst);
            *self.last_helo.lock().unwrap() = Some(hostname.to_string());
            if self.helo_fail {
                return Err("stalwart down".into());
            }
            Ok(())
        }
        fn persist_hostname(&mut self, _hostname: &str) -> Result<(), String> {
            if self.persist_fail {
                return Err("db locked".into());
            }
            Ok(())
        }
        fn call_cp(&mut self, _hostname: &str) -> Result<DnsHttpResponse, String> {
            self.cp_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = &self.cp_err {
                return Err(e.clone());
            }
            Ok(self.cp_resp.clone().unwrap_or(DnsHttpResponse {
                status: 200,
                body: r#"{"ok":true}"#.into(),
            }))
        }
    }

    fn deps_ok() -> RecordingDeps {
        RecordingDeps {
            helo_calls: Arc::new(AtomicUsize::new(0)),
            cp_calls: Arc::new(AtomicUsize::new(0)),
            helo_fail: false,
            persist_fail: false,
            cp_resp: None,
            cp_err: None,
            last_helo: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn map_ptr_cp_preserves_501_ptr_provider() {
        let resp = DnsHttpResponse {
            status: 501,
            body: r#"{"ok":false,"error":{"code":"ptr_provider","provider":"hcloud","hint":"Hetzner later"}}"#.into(),
        };
        let r = map_ptr_cp_response(&resp);
        assert_eq!(r.status, "501 Not Implemented", "{}", r.body);
        assert!(r.body.contains("ptr_provider"), "{}", r.body);
        assert!(r.body.contains("hcloud"), "{}", r.body);
        assert!(!r.status.starts_with("502"), "must not collapse 501→502");
    }

    #[test]
    fn map_ptr_cp_preserves_403_tunnel_only() {
        let resp = DnsHttpResponse {
            status: 403,
            body: r#"{"ok":false,"error":{"code":"tunnel_only","hint":"cookie on bind is 403"}}"#
                .into(),
        };
        let r = map_ptr_cp_response(&resp);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("tunnel_only"), "{}", r.body);
    }

    #[test]
    fn show_alignment_flags() {
        let mut dns = FakeDns::default();
        let origin: Ipv4Addr = "203.0.113.7".parse().unwrap();
        dns.ptr
            .insert("203.0.113.7".into(), vec!["mail.acme.dev".into()]);
        dns.a
            .insert("mail.acme.dev".into(), vec![origin]);
        let v = ptr_show_json(origin, Some("mail.acme.dev"), &dns);
        assert_eq!(v["originIpv4"], "203.0.113.7");
        assert_eq!(v["ptr"], "mail.acme.dev");
        assert_eq!(v["helo"], "mail.acme.dev");
        assert_eq!(v["ptrMatchesHelo"], true);
        assert_eq!(v["aMatchesOrigin"], true);
        assert_eq!(v["aligned"], true);
    }

    #[test]
    fn show_misaligned_when_ptr_differs() {
        let mut dns = FakeDns::default();
        let origin: Ipv4Addr = "203.0.113.7".parse().unwrap();
        dns.ptr.insert(
            "203.0.113.7".into(),
            vec!["ns1003061.ip-40-160-53.us".into()],
        );
        dns.a
            .insert("mail.acme.dev".into(), vec![origin]);
        let v = ptr_show_json(origin, Some("mail.acme.dev"), &dns);
        assert_eq!(v["ptrMatchesHelo"], false);
        assert_eq!(v["aMatchesOrigin"], true);
        assert_eq!(v["aligned"], false);
    }

    #[test]
    fn set_a_mismatch_is_400_and_skips_helo_and_cp() {
        let env = FakeEnv {
            ip: Some("203.0.113.7".into()),
        };
        let mut dns = FakeDns::default();
        dns.a.insert(
            "mail.acme.dev".into(),
            vec!["198.51.100.9".parse().unwrap()],
        );
        let mut deps = deps_ok();
        let helo_n = Arc::clone(&deps.helo_calls);
        let cp_n = Arc::clone(&deps.cp_calls);
        let r = handle_ptr_set_with(
            br#"{"hostname":"mail.acme.dev"}"#,
            &env,
            &dns,
            &mut deps,
        );
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("a_mismatch"), "{}", r.body);
        assert_eq!(helo_n.load(Ordering::SeqCst), 0, "must not set HELO");
        assert_eq!(cp_n.load(Ordering::SeqCst), 0, "must not call CP");
    }

    #[test]
    fn set_helo_fail_skips_cp() {
        let env = FakeEnv {
            ip: Some("203.0.113.7".into()),
        };
        let mut dns = FakeDns::default();
        dns.a.insert(
            "mail.acme.dev".into(),
            vec!["203.0.113.7".parse().unwrap()],
        );
        let mut deps = deps_ok();
        deps.helo_fail = true;
        let cp_n = Arc::clone(&deps.cp_calls);
        let r = handle_ptr_set_with(
            br#"{"hostname":"mail.acme.dev"}"#,
            &env,
            &dns,
            &mut deps,
        );
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        assert!(r.body.contains("helo"), "{}", r.body);
        assert!(r.body.contains("CP not called"), "{}", r.body);
        assert_eq!(cp_n.load(Ordering::SeqCst), 0, "must not call CP after HELO fail");
    }

    #[test]
    fn set_unpaired_is_loud() {
        let env = FakeEnv {
            ip: Some("203.0.113.7".into()),
        };
        let mut dns = FakeDns::default();
        dns.a.insert(
            "mail.acme.dev".into(),
            vec!["203.0.113.7".parse().unwrap()],
        );
        let mut deps = deps_ok();
        deps.cp_err = Some("no tunnel token in ~/.k2/tunnel.json — pair K2 Connect first".into());
        let r = handle_ptr_set_with(
            br#"{"hostname":"mail.acme.dev"}"#,
            &env,
            &dns,
            &mut deps,
        );
        assert_eq!(r.status, "401 Unauthorized", "{}", r.body);
        assert!(r.body.contains("unpaired") || r.body.contains("tunnel"), "{}", r.body);
    }

    #[test]
    fn set_success_calls_helo_then_cp() {
        let env = FakeEnv {
            ip: Some("203.0.113.7".into()),
        };
        let mut dns = FakeDns::default();
        let origin: Ipv4Addr = "203.0.113.7".parse().unwrap();
        dns.a
            .insert("mail.acme.dev".into(), vec![origin]);
        dns.ptr
            .insert("203.0.113.7".into(), vec!["mail.acme.dev".into()]);
        let mut deps = deps_ok();
        let helo_n = Arc::clone(&deps.helo_calls);
        let cp_n = Arc::clone(&deps.cp_calls);
        let last = Arc::clone(&deps.last_helo);
        let r = handle_ptr_set_with(
            br#"{"hostname":"Mail.Acme.Dev."}"#,
            &env,
            &dns,
            &mut deps,
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(helo_n.load(Ordering::SeqCst), 1);
        assert_eq!(cp_n.load(Ordering::SeqCst), 1);
        assert_eq!(last.lock().unwrap().as_deref(), Some("mail.acme.dev"));
    }

    #[test]
    fn set_missing_hostname_is_usage() {
        let env = FakeEnv {
            ip: Some("203.0.113.7".into()),
        };
        let dns = FakeDns::default();
        let mut deps = deps_ok();
        let r = handle_ptr_set_with(br#"{}"#, &env, &dns, &mut deps);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("hostname"), "{}", r.body);
    }

    #[test]
    fn refuse_connect_vip_as_origin() {
        let env = FakeEnv {
            ip: Some("178.156.232.105".into()),
        };
        let dns = FakeDns::default();
        let r = handle_ptr_show_with(&env, &dns);
        assert_eq!(r.status, "502 Bad Gateway", "{}", r.body);
        assert!(r.body.contains("178.156.232.105"), "{}", r.body);
        assert!(r.body.contains("VIP") || r.body.contains("origin"), "{}", r.body);
    }

    #[test]
    fn get_set_path_is_method_not_allowed_in_dispatch() {
        // Pin the house rule: GET /cli/mail/ptr/set must 405 via the
        // mail_routes GET chain (wired in mail_routes::dispatch).
        let r = crate::mail_routes::dispatch(
            "/cli/mail/ptr/set",
            &HashMap::new(),
        )
        .expect("claimed");
        assert_eq!(r.status, "405 Method Not Allowed", "{}", r.body);
    }
}
