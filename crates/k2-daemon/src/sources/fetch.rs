//! The source fetcher (contract §3.4, §10): `FetchPlan` →
//! `vault_fetch::execute` with the source's own allowlist (checked first,
//! on every hop [GV39]) and `Purpose::Source`; errors become fixed
//! `Reason`s; rate-limit headers become a park for the origin.
//!
//! Blocking: the poller and Try run it on worker threads.

use std::time::{Duration, Instant};

use k2_core::sources::{FetchOutcome, FetchPlan, Fetcher, Reason, UrlRef};
use k2_vault::redact::Redactor;
use k2_vault::VaultError;

use crate::egress::Purpose;
use crate::vault_fetch::{self, Allowlist, FetchSpec};

/// The daemon's [`Fetcher`].
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&self, plan: &FetchPlan) -> FetchOutcome {
        fetch_once(plan)
    }
}

/// One fetch's outcome plus what the budget should do with the origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    pub outcome: FetchOutcome,
    /// Park the origin this many seconds (`Retry-After`, or a spent
    /// `X-RateLimit-*` window), capped at `PARK_MAX_SECS`.
    pub park_secs: Option<u32>,
}

/// Run one request.
pub fn fetch_once(plan: &FetchPlan) -> FetchOutcome {
    fetch_detailed(plan).outcome
}

fn err(reason: Reason, http_status: Option<u16>, retry_after_secs: Option<u32>) -> FetchOutcome {
    FetchOutcome::Err { reason, http_status, retry_after_secs }
}

/// Run one request and report rate-limit parks.
pub fn fetch_detailed(plan: &FetchPlan) -> Fetched {
    let started = Instant::now();
    let resolved = match super::secrets::resolve_plan(plan) {
        Ok(r) => r,
        Err(reason) => return Fetched { outcome: err(reason, None, None), park_secs: None },
    };
    let allow = match Allowlist::for_source(std::slice::from_ref(&plan.origin), plan.lan_exact_ip) {
        Ok(a) => a,
        Err(_) => return Fetched { outcome: err(Reason::BlockedAddress, None, None), park_secs: None },
    };
    let url = match (&plan.url, resolved.url) {
        (_, Some(u)) => u,
        (UrlRef::Plain(u), None) => u.clone(),
        (UrlRef::Secret { .. }, None) => {
            return Fetched { outcome: err(Reason::SecretMissing, None, None), park_secs: None }
        }
    };
    let spec = FetchSpec {
        method: "GET".into(),
        url,
        headers: plan.headers.clone(),
        body: None,
        max_bytes: plan.max_bytes as usize,
        timeout: Duration::from_secs(u64::from(plan.timeout_secs.clamp(1, k2_core::sources::MAX_TIMEOUT_SECS))),
        include_headers: false,
        purpose: Purpose::Source,
    };
    let res = vault_fetch::execute(spec, &allow, resolved.injections, &resolved.redactor);
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    match res {
        Ok(mut r) => {
            if let Some(extra) = &resolved.extra_redact {
                r.body = extra.redact(&r.body);
            }
            from_response(r, elapsed_ms, plan.expect_status.as_deref())
        }
        Err(e) => Fetched { outcome: err(reason_for(&e), None, None), park_secs: None },
    }
}

/// A fetch refusal or transport failure as a fixed reason.
pub fn reason_for(e: &VaultError) -> Reason {
    match e.code {
        "airgap" => Reason::Airgap,
        "address_not_allowed" | "host_not_allowed" | "no_allowlist" | "lan_owner_only" | "no_allowed_hosts" => {
            Reason::BlockedAddress
        }
        "upstream_error" => {
            let h = e.hint.as_str();
            if h.starts_with("couldn't resolve") {
                Reason::Dns
            } else if h.starts_with("timed out") {
                Reason::Timeout
            } else if h.starts_with("TLS failed") {
                Reason::Tls
            } else {
                Reason::Connect
            }
        }
        _ => Reason::Connect,
    }
}

fn header<'a>(r: &'a vault_fetch::FetchResult, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// Seconds to wait from a `Retry-After` (delta seconds or an HTTP date).
fn retry_after(r: &vault_fetch::FetchResult) -> Option<u32> {
    let v = header(r, "retry-after")?.trim();
    if let Ok(n) = v.parse::<u64>() {
        return Some(n.min(u64::from(k2_core::sources::PARK_MAX_SECS)) as u32);
    }
    let when = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let secs = (when.timestamp() - chrono::Utc::now().timestamp()).max(1);
    Some(secs.min(i64::from(k2_core::sources::PARK_MAX_SECS)) as u32)
}

/// A spent rate-limit window: `X-RateLimit-Remaining: 0` (or
/// `RateLimit-Remaining: 0`) with a reset (epoch seconds or seconds left).
fn window_spent(r: &vault_fetch::FetchResult) -> Option<u32> {
    let remaining = header(r, "x-ratelimit-remaining").or_else(|| header(r, "ratelimit-remaining"))?;
    if remaining.trim().parse::<i64>().ok()? > 0 {
        return None;
    }
    let reset = header(r, "x-ratelimit-reset").or_else(|| header(r, "ratelimit-reset"))?.trim().parse::<i64>().ok()?;
    let now = chrono::Utc::now().timestamp();
    let secs = if reset > 1_000_000_000 { reset - now } else { reset };
    Some(secs.clamp(1, i64::from(k2_core::sources::PARK_MAX_SECS)) as u32)
}

#[cfg(test)]
thread_local! {
    /// Bodies parsed on this thread (a cut body must never be).
    pub(crate) static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn from_response(r: vault_fetch::FetchResult, elapsed_ms: u32, expect: Option<&[u16]>) -> Fetched {
    let status = r.status;
    let park = match status {
        429 | 503 => retry_after(&r).or_else(|| window_spent(&r)).or((status == 429).then_some(60)),
        _ => window_spent(&r),
    };
    let outcome = if expect.is_some_and(|x| x.contains(&status)) {
        // A status-only check (`expect.status`, 0.46.1): a listed status is
        // an answer. JSON is read when it parses (and wasn't cut); anything
        // else reads as null. A cut body is never parsed [GV41].
        let body = if r.truncated {
            serde_json::Value::Null
        } else {
            #[cfg(test)]
            PARSES.with(|c| c.set(c.get() + 1));
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap_or(serde_json::Value::Null)
        };
        FetchOutcome::Ok {
            status,
            body,
            not_modified: false,
            etag: header(&r, "etag").map(str::to_string),
            last_modified: header(&r, "last-modified").map(str::to_string),
            elapsed_ms,
        }
    } else if r.truncated {
        // Never parsed: a cut body is `too_large` [GV41].
        err(Reason::TooLarge, Some(status), None)
    } else if expect.is_some() && (200..=299).contains(&status) {
        // Expecting other statuses: an unlisted 2xx isn't the answer.
        err(Reason::UnexpectedStatus, Some(status), None)
    } else {
        match status {
            304 => FetchOutcome::Ok {
                status,
                body: serde_json::Value::Null,
                not_modified: true,
                etag: header(&r, "etag").map(str::to_string),
                last_modified: header(&r, "last-modified").map(str::to_string),
                elapsed_ms,
            },
            200..=299 => {
                #[cfg(test)]
                PARSES.with(|c| c.set(c.get() + 1));
                match serde_json::from_slice::<serde_json::Value>(&r.body) {
                    Ok(body) => FetchOutcome::Ok {
                        status,
                        body,
                        not_modified: false,
                        etag: header(&r, "etag").map(str::to_string),
                        last_modified: header(&r, "last-modified").map(str::to_string),
                        elapsed_ms,
                    },
                    Err(_) => err(Reason::NotJson, Some(status), None),
                }
            }
            300..=399 => err(Reason::RedirectOtherOrigin, Some(status), None),
            401 | 403 => err(Reason::HttpAuth, Some(status), None),
            404 => err(Reason::Http404, Some(status), None),
            429 => err(Reason::RateLimited, Some(status), park),
            400..=499 => err(Reason::Http4xx, Some(status), None),
            _ => err(Reason::Http5xx, Some(status), if status == 503 { park } else { None }),
        }
    };
    Fetched { outcome, park_secs: park }
}

/// What a plan's secrets resolve to (A1). Secretless plans resolve to
/// nothing.
pub struct Resolved {
    pub injections: Vec<vault_fetch::Injection>,
    /// The vault's redactor (every user secret's value and encodings).
    pub redactor: std::sync::Arc<Redactor>,
    /// A `urlSecret` URL (already checked against the pinned origin).
    pub url: Option<String>,
    /// The whole `urlSecret` URL, redacted from the body too.
    pub extra_redact: Option<Redactor>,
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
pub(crate) mod tests;
