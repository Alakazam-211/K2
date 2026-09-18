//! Control-plane bind/unbind for custom-domain attach (A8 / A12 / A15).
//!
//! Attach POSTs `POST /api/dns/zones/bind` `{domain}` with the existing
//! tunnel `k2c_` token. 404 JSON (account does not own the apex) → BYO
//! `dnsWrite=0`. 404 HTML / Next missing-route → fail loud
//! (`bind_api_unavailable`). Unbind on remove.

use crate::dns::proxy::{proxy_request, DnsHttpResponse};

/// Result of talking to `/api/dns/zones/bind` (or unbind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    /// CP bound this label to the apex. `zone_id` may be None if the
    /// response omitted it; `dns_write` is still true.
    Bound { zone_id: Option<String> },
    /// JSON 404: this Connect account does not own the apex → BYO inventory.
    NotOwned,
    /// No tunnel token (air-gap / unpaired) — local BYO only, no bind call.
    NoTunnel,
    /// Bind routes are not shipped (HTML/Next 404) — fail loud, do not
    /// silently treat as BYO while k2c_ still lists the account.
    ApiMissing { hint: String },
    /// Any other failure (401/5xx/network) — fail loud.
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
    match proxy_request("POST", path, None, Some(&body)) {
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

/// Classify a bind/unbind HTTP response. Pure so tests never dial k2.dev.
pub fn classify_bind_response(resp: &DnsHttpResponse) -> BindOutcome {
    match resp.status {
        200 | 201 => BindOutcome::Bound {
            zone_id: parse_zone_id(&resp.body),
        },
        404 => classify_404(&resp.body),
        401 => BindOutcome::Failed {
            status: 401,
            hint: "tunnel token rejected by DNS bind API — re-pair K2 Connect".into(),
        },
        s => BindOutcome::Failed {
            status: s,
            hint: parse_hint(&resp.body)
                .unwrap_or_else(|| format!("DNS bind API error (HTTP {s})")),
        },
    }
}

fn classify_404(body: &str) -> BindOutcome {
    let trimmed = body.trim();
    // Next missing-route is HTML. A JSON object is the locked A8
    // "account does not own this apex" contract.
    if looks_like_json_object(trimmed) {
        BindOutcome::NotOwned
    } else {
        BindOutcome::ApiMissing {
            hint: "POST /api/dns/zones/bind is not live on k2.dev yet \
(got a non-JSON 404). Attach cannot silently fall back to a daemon-only \
filter while the tunnel token still lists the account."
                .into(),
        }
    }
}

fn looks_like_json_object(body: &str) -> bool {
    let t = body.trim_start();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(body).is_ok()
}

fn parse_zone_id(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("zoneId")
        .or_else(|| v.get("zone_id"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            v.get("zone")
                .and_then(|z| z.get("id"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            v.get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
}

fn parse_hint(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("error")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("error")
                .and_then(|e| e.get("hint"))
                .and_then(|h| h.as_str())
                .map(|s| s.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::proxy::DnsHttpResponse;

    #[test]
    fn json_404_is_not_owned_byo() {
        let r = classify_bind_response(&DnsHttpResponse {
            status: 404,
            body: r#"{"error":"not found"}"#.into(),
        });
        assert_eq!(r, BindOutcome::NotOwned);
    }

    #[test]
    fn html_404_is_api_missing() {
        let r = classify_bind_response(&DnsHttpResponse {
            status: 404,
            body: "<!DOCTYPE html><html><body>Not Found</body></html>".into(),
        });
        match r {
            BindOutcome::ApiMissing { hint } => {
                assert!(hint.contains("bind"), "{hint}");
            }
            other => panic!("expected ApiMissing, got {other:?}"),
        }
    }

    #[test]
    fn empty_404_is_api_missing() {
        let r = classify_bind_response(&DnsHttpResponse {
            status: 404,
            body: String::new(),
        });
        assert!(matches!(r, BindOutcome::ApiMissing { .. }));
    }

    #[test]
    fn bind_200_parses_zone_id_shapes() {
        let r = classify_bind_response(&DnsHttpResponse {
            status: 200,
            body: r#"{"ok":true,"zoneId":"56fb29e4"}"#.into(),
        });
        assert_eq!(
            r,
            BindOutcome::Bound {
                zone_id: Some("56fb29e4".into())
            }
        );
        let r = classify_bind_response(&DnsHttpResponse {
            status: 200,
            body: r#"{"zone":{"id":"abc"}}"#.into(),
        });
        assert_eq!(
            r,
            BindOutcome::Bound {
                zone_id: Some("abc".into())
            }
        );
    }

    #[test]
    fn unauthorized_fails_loud() {
        let r = classify_bind_response(&DnsHttpResponse {
            status: 401,
            body: r#"{"error":"unauthorized"}"#.into(),
        });
        assert!(matches!(r, BindOutcome::Failed { status: 401, .. }));
    }
}
