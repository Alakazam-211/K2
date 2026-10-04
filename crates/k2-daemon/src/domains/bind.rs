//! Control-plane bind/unbind for custom-domain attach (A8 / A8.1 / A12 / A15).
//!
//! Attach POSTs `POST /api/dns/zones/bind` `{domain}` with the existing
//! tunnel `k2c_` token. The k2.dev A8.1 contract:
//!
//! - **200** `{ok, zoneId, status:"active"|"pending_ns", nameservers,
//!   dnsWrite, created?}` → [`BindOutcome::Bound`]. `created:true` means
//!   k2.dev auto-added the zone to this account; a new zone starts
//!   `pending_ns` with `dnsWrite:false` until its NS point at k2.dev. The
//!   older A8 shape `{zoneId}` (no `status`) means active.
//! - **409** `{error:"zone_owned_elsewhere", hint}` → another k2.dev
//!   account owns the apex ([`BindOutcome::OwnedElsewhere`]).
//! - **404 JSON** (A8 legacy) → not owned → BYO `dnsWrite=0`.
//! - **405**, an empty / non-JSON 404, or **any HTML reply** → the route is
//!   not deployed on k2.dev ([`BindOutcome::ApiMissing`]) — fail loud as
//!   `bind_api_unavailable`, never silently BYO.
//!
//! Unbind on remove uses the same classifier.

use k2_core::domains::{BoundZone, ZONE_STATUS_ACTIVE, ZONE_STATUS_PENDING_NS};

use crate::dns::proxy::{proxy_request, DnsHttpResponse};

/// Plain words for the "route isn't deployed" case (UI + CLI copy).
pub const BIND_API_UNAVAILABLE_MESSAGE: &str = "Domain linking isn't live on k2.dev yet";

/// Default copy when k2.dev 409s without a hint.
pub const ZONE_OWNED_ELSEWHERE_MESSAGE: &str = "This domain belongs to another k2.dev account.";

/// Result of talking to `/api/dns/zones/bind` (or unbind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    /// k2.dev bound this box to the apex. `dns_write` follows the
    /// response (`pending_ns` is always false).
    Bound(BoundZone),
    /// JSON 404: this Connect account does not own the apex → BYO inventory.
    NotOwned,
    /// No tunnel token (air-gap / unpaired) — local BYO only, no bind call.
    NoTunnel,
    /// 409: another k2.dev account owns the apex.
    OwnedElsewhere { hint: String },
    /// Bind routes are not shipped (405 / empty or HTML 404 / any HTML) —
    /// fail loud, do not silently treat as BYO while k2c_ still lists the
    /// account.
    ApiMissing { hint: String },
    /// Any other failure (401/403/5xx/network) — fail loud.
    Failed { status: u16, hint: String },
}

/// POST bind/unbind. `unbind` is the same contract with a different path.
pub fn bind_apex(apex: &str) -> BindOutcome {
    call_bind_path("/api/dns/zones/bind", apex)
}

pub fn unbind_apex(apex: &str) -> BindOutcome {
    call_bind_path("/api/dns/zones/unbind", apex)
}

fn call_bind_path(path: &str, apex: &str) -> BindOutcome {
    let body = serde_json::json!({ "domain": apex }).to_string();
    match send(path, &body) {
        Ok(resp) => classify_bind_response(&resp),
        Err(e) => {
            if e.contains("no tunnel token") || e.contains("tunnel.json") {
                BindOutcome::NoTunnel
            } else if k2_core::airgap::refuse().is_err() {
                BindOutcome::NoTunnel
            } else {
                BindOutcome::Failed {
                    status: 0,
                    hint: e,
                }
            }
        }
    }
}

#[cfg(not(test))]
fn send(path: &str, body: &str) -> Result<DnsHttpResponse, String> {
    proxy_request("POST", path, None, Some(body))
}

/// Tests never dial k2.dev: with no fake installed the call behaves as an
/// unpaired box (no tunnel token).
#[cfg(test)]
fn send(path: &str, body: &str) -> Result<DnsHttpResponse, String> {
    let _ = proxy_request; // keep the production import live under cfg(test)
    TEST_BIND_CALLS.with(|c| c.borrow_mut().push((path.to_string(), body.to_string())));
    match TEST_BIND_RESPONSE.with(|c| c.borrow().clone()) {
        Some(r) => r,
        None => Err("no tunnel token in ~/.k2/tunnel.json (test: no fake bind response)".into()),
    }
}

#[cfg(test)]
thread_local! {
    static TEST_BIND_RESPONSE: std::cell::RefCell<Option<Result<DnsHttpResponse, String>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_BIND_CALLS: std::cell::RefCell<Vec<(String, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) struct FakeBindGuard;

#[cfg(test)]
impl Drop for FakeBindGuard {
    fn drop(&mut self) {
        TEST_BIND_RESPONSE.with(|c| *c.borrow_mut() = None);
        TEST_BIND_CALLS.with(|c| c.borrow_mut().clear());
    }
}

/// Test seam: every bind/unbind on this thread answers `resp`.
#[cfg(test)]
pub(crate) fn set_fake_bind(resp: Result<DnsHttpResponse, String>) -> FakeBindGuard {
    TEST_BIND_RESPONSE.with(|c| *c.borrow_mut() = Some(resp));
    TEST_BIND_CALLS.with(|c| c.borrow_mut().clear());
    FakeBindGuard
}

/// Test seam: swap the fake answer without clearing the call log.
#[cfg(test)]
pub(crate) fn replace_fake_bind(resp: Result<DnsHttpResponse, String>) {
    TEST_BIND_RESPONSE.with(|c| *c.borrow_mut() = Some(resp));
}

/// Serializes tests that put `pending_ns` rows in the shared test DB:
/// `POST /cli/domains/refresh` with no apex re-binds every pending row,
/// so those tests must not overlap.
#[cfg(test)]
pub(crate) fn pending_rows_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Test seam: `(path, body)` of every bind/unbind call on this thread.
#[cfg(test)]
pub(crate) fn fake_bind_calls() -> Vec<(String, String)> {
    TEST_BIND_CALLS.with(|c| c.borrow().clone())
}

/// Classify a bind/unbind HTTP response. Pure so tests never dial k2.dev.
pub fn classify_bind_response(resp: &DnsHttpResponse) -> BindOutcome {
    let body = resp.body.trim();
    match resp.status {
        401 => {
            return BindOutcome::Failed {
                status: 401,
                hint: "tunnel token rejected by DNS bind API — re-pair K2 Connect".into(),
            }
        }
        405 => return api_missing(&format!("HTTP 405{}", body_note(body))),
        _ => {}
    }
    if looks_like_html(body) {
        return api_missing(&format!("HTTP {} with an HTML page", resp.status));
    }
    match resp.status {
        200 | 201 => match parse_object(body) {
            Some(v) => BindOutcome::Bound(parse_bound_zone(&v)),
            None => api_missing(&format!("HTTP {}{}", resp.status, body_note(body))),
        },
        404 => {
            // A JSON object is the locked A8 "account does not own this
            // apex" contract. Empty / plain text is a missing route.
            if parse_object(body).is_some() {
                BindOutcome::NotOwned
            } else {
                api_missing(&format!("HTTP 404{}", body_note(body)))
            }
        }
        409 => {
            let code = parse_error_code(body);
            if code.as_deref().map_or(true, |c| c == "zone_owned_elsewhere") {
                BindOutcome::OwnedElsewhere {
                    hint: parse_hint(body)
                        .filter(|h| h != "zone_owned_elsewhere")
                        .unwrap_or_else(|| ZONE_OWNED_ELSEWHERE_MESSAGE.into()),
                }
            } else {
                BindOutcome::Failed {
                    status: 409,
                    hint: parse_hint(body).unwrap_or_else(|| "DNS bind conflict (HTTP 409)".into()),
                }
            }
        }
        s => BindOutcome::Failed {
            status: s,
            hint: parse_hint(body).unwrap_or_else(|| format!("DNS bind API error (HTTP {s})")),
        },
    }
}

fn api_missing(detail: &str) -> BindOutcome {
    BindOutcome::ApiMissing {
        hint: format!(
            "{BIND_API_UNAVAILABLE_MESSAGE} (POST /api/dns/zones/bind answered {detail}). \
Try again after k2.dev ships it; the domain was not attached."
        ),
    }
}

fn body_note(body: &str) -> &'static str {
    if body.is_empty() {
        " with an empty body"
    } else {
        " with a non-JSON body"
    }
}

fn looks_like_html(body: &str) -> bool {
    let head: String = body.chars().take(64).collect::<String>().to_ascii_lowercase();
    head.starts_with("<!doctype") || head.starts_with("<html") || head.starts_with('<')
}

fn parse_object(body: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if !body.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(body).ok()? {
        serde_json::Value::Object(m) => Some(m),
        _ => None,
    }
}

fn parse_bound_zone(v: &serde_json::Map<String, serde_json::Value>) -> BoundZone {
    let status = match v.get("status").and_then(|s| s.as_str()).map(str::trim) {
        // A8 legacy `{zoneId}`: no status means active.
        None | Some("") => ZONE_STATUS_ACTIVE.to_string(),
        Some(s) => s.to_ascii_lowercase(),
    };
    let pending = status == ZONE_STATUS_PENDING_NS;
    let dns_write = match v.get("dnsWrite").and_then(|b| b.as_bool()) {
        // A pending zone can never write, whatever the flag says.
        Some(b) => b && !pending,
        None => status == ZONE_STATUS_ACTIVE,
    };
    let nameservers = v
        .get("nameservers")
        .and_then(|n| n.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    BoundZone {
        zone_id: parse_zone_id(v),
        status: Some(status),
        nameservers,
        dns_write,
        auto_created: v.get("created").and_then(|b| b.as_bool()).unwrap_or(false),
    }
}

fn non_empty_str(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn parse_zone_id(v: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    non_empty_str(v.get("zoneId"))
        .or_else(|| non_empty_str(v.get("zone_id")))
        .or_else(|| non_empty_str(v.get("zone").and_then(|z| z.get("id"))))
        .or_else(|| non_empty_str(v.get("id")))
}

fn parse_error_code(body: &str) -> Option<String> {
    let v = parse_object(body)?;
    non_empty_str(v.get("error")).or_else(|| non_empty_str(v.get("error").and_then(|e| e.get("code"))))
}

fn parse_hint(body: &str) -> Option<String> {
    let v = parse_object(body)?;
    non_empty_str(v.get("hint"))
        .or_else(|| non_empty_str(v.get("error").and_then(|e| e.get("hint"))))
        .or_else(|| non_empty_str(v.get("error")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::proxy::DnsHttpResponse;

    fn resp(status: u16, body: &str) -> DnsHttpResponse {
        DnsHttpResponse {
            status,
            body: body.into(),
        }
    }

    fn bound(r: BindOutcome) -> BoundZone {
        match r {
            BindOutcome::Bound(z) => z,
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    fn missing_hint(r: BindOutcome) -> String {
        match r {
            BindOutcome::ApiMissing { hint } => hint,
            other => panic!("expected ApiMissing, got {other:?}"),
        }
    }

    #[test]
    fn a81_200_active() {
        let z = bound(classify_bind_response(&resp(
            200,
            r#"{"ok":true,"zoneId":"z-act","status":"active","nameservers":["ns1.k2.dev","ns2.k2.dev"],"dnsWrite":true}"#,
        )));
        assert_eq!(z.zone_id.as_deref(), Some("z-act"));
        assert_eq!(z.status.as_deref(), Some("active"));
        assert!(z.dns_write);
        assert!(!z.auto_created);
        assert_eq!(z.nameservers, vec!["ns1.k2.dev", "ns2.k2.dev"]);
    }

    #[test]
    fn a81_200_pending_created() {
        let z = bound(classify_bind_response(&resp(
            200,
            r#"{"ok":true,"zoneId":"z-new","status":"pending_ns","nameservers":["ns1.k2.dev.","NS2.k2.dev"],"dnsWrite":false,"created":true}"#,
        )));
        assert_eq!(z.zone_id.as_deref(), Some("z-new"));
        assert_eq!(z.status.as_deref(), Some("pending_ns"));
        assert!(!z.dns_write);
        assert!(z.auto_created);
        assert_eq!(z.nameservers, vec!["ns1.k2.dev", "ns2.k2.dev"]);
    }

    #[test]
    fn pending_never_writes_even_if_flag_says_true() {
        let z = bound(classify_bind_response(&resp(
            200,
            r#"{"zoneId":"z","status":"pending_ns","dnsWrite":true}"#,
        )));
        assert!(!z.dns_write);
    }

    #[test]
    fn a8_legacy_200_without_status_is_active() {
        let z = bound(classify_bind_response(&resp(200, r#"{"ok":true,"zoneId":"56fb29e4"}"#)));
        assert_eq!(z.zone_id.as_deref(), Some("56fb29e4"));
        assert_eq!(z.status.as_deref(), Some("active"));
        assert!(z.dns_write);
        assert!(z.nameservers.is_empty());
        let z = bound(classify_bind_response(&resp(200, r#"{"zone":{"id":"abc"}}"#)));
        assert_eq!(z.zone_id.as_deref(), Some("abc"));
        assert!(z.dns_write);
    }

    #[test]
    fn conflict_409_is_owned_elsewhere_with_hint() {
        let r = classify_bind_response(&resp(
            409,
            r#"{"error":"zone_owned_elsewhere","hint":"example.com is in another k2.dev account"}"#,
        ));
        assert_eq!(
            r,
            BindOutcome::OwnedElsewhere {
                hint: "example.com is in another k2.dev account".into()
            }
        );
        let r = classify_bind_response(&resp(409, r#"{"error":"zone_owned_elsewhere"}"#));
        assert_eq!(
            r,
            BindOutcome::OwnedElsewhere {
                hint: ZONE_OWNED_ELSEWHERE_MESSAGE.into()
            }
        );
    }

    #[test]
    fn json_404_is_not_owned_byo() {
        let r = classify_bind_response(&resp(404, r#"{"error":"not found"}"#));
        assert_eq!(r, BindOutcome::NotOwned);
    }

    #[test]
    fn html_404_is_api_missing() {
        let h = missing_hint(classify_bind_response(&resp(
            404,
            "<!DOCTYPE html><html><body>Not Found</body></html>",
        )));
        assert!(h.contains(BIND_API_UNAVAILABLE_MESSAGE), "{h}");
    }

    #[test]
    fn empty_404_is_api_missing() {
        let h = missing_hint(classify_bind_response(&resp(404, "")));
        assert!(h.contains("empty body"), "{h}");
    }

    #[test]
    fn plain_text_404_is_api_missing() {
        missing_hint(classify_bind_response(&resp(404, "Not Found")));
    }

    #[test]
    fn empty_405_is_api_missing() {
        let h = missing_hint(classify_bind_response(&resp(405, "")));
        assert!(h.contains(BIND_API_UNAVAILABLE_MESSAGE), "{h}");
        assert!(h.contains("405"), "{h}");
    }

    #[test]
    fn json_405_is_still_api_missing() {
        missing_hint(classify_bind_response(&resp(405, r#"{"error":"method not allowed"}"#)));
    }

    #[test]
    fn html_on_any_status_is_api_missing() {
        for s in [200u16, 500, 502, 403] {
            missing_hint(classify_bind_response(&resp(s, "<html><body>oops</body></html>")));
        }
    }

    #[test]
    fn empty_200_is_api_missing() {
        missing_hint(classify_bind_response(&resp(200, "")));
    }

    #[test]
    fn unauthorized_fails_loud() {
        let r = classify_bind_response(&resp(401, r#"{"error":"unauthorized"}"#));
        assert!(matches!(r, BindOutcome::Failed { status: 401, .. }), "{r:?}");
        // Even an HTML 401 is an auth failure, not a missing route.
        let r = classify_bind_response(&resp(401, "<html>401</html>"));
        assert!(matches!(r, BindOutcome::Failed { status: 401, .. }), "{r:?}");
    }

    #[test]
    fn forbidden_fails_loud_with_hint() {
        let r = classify_bind_response(&resp(403, r#"{"error":"forbidden","hint":"plan required"}"#));
        assert_eq!(
            r,
            BindOutcome::Failed {
                status: 403,
                hint: "plan required".into()
            }
        );
    }

    #[test]
    fn no_fake_installed_never_dials_and_reads_as_unpaired() {
        let _g = set_fake_bind(Err("unused".into()));
        drop(_g);
        assert_eq!(bind_apex("example.com"), BindOutcome::NoTunnel);
    }

    #[test]
    fn fake_bind_records_path_and_body() {
        let _g = set_fake_bind(Ok(resp(200, r#"{"zoneId":"z"}"#)));
        bind_apex("example.com");
        unbind_apex("example.com");
        let calls = fake_bind_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "/api/dns/zones/bind");
        assert_eq!(calls[1].0, "/api/dns/zones/unbind");
        assert!(calls[0].1.contains("\"domain\":\"example.com\""), "{}", calls[0].1);
    }
}
