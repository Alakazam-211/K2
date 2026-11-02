//! Path B's claim on a custom name at K2 Connect's control plane (0.46.8,
//! `prd-custom-domains-path-b-0467-v1` PB-3/PB-4; vs-live PB-A5, PB-A15).
//!
//! A customer's name reaches this box through K2's relays only once the
//! control plane has verified it for this box's label: the relay's frps
//! plugin registers `k2-custom-<label>`'s `customDomains` as
//! `requested ∩ verified(label)`. The box asks with its tunnel token:
//!
//! ```text
//! POST {base}/api/custom-hostnames                    {hostname, verifyToken, capabilities:["customSni"]}
//! POST {base}/api/custom-hostnames/{hostname}/verify  {}
//! POST {base}/api/custom-hostnames/{hostname}/release {}
//! GET  {base}/api/custom-hostnames                    this label's names
//! ```
//!
//! `{base}` is K2 Connect's control plane (`K2_CUSTOM_HOSTNAMES_BASE`, else
//! `K2_CONNECT_BASE`, else `https://connect.k2.dev`). Answers carry
//! `{hostname, status: pending|verified|failing|released, lastError?}` (one,
//! or `{hostnames:[…]}` for the list); refusals `{error: <code>, detail?,
//! count?, cap?}`. The control plane does the authoritative TXT check
//! through public resolvers (never the box's word), uniqueness, the deny
//! list, the Pro gate and the caps (20 per box, 100 per account).
//!
//! The box's own checks before a claim (PB-4) are courtesy, for good words:
//! the box is Pro behind K2 Connect, and holds fewer than
//! [`k2_core::tunnel::custom_names::MAX_PER_BOX`] claims. Offline (the
//! plan can't be read) is "can't check your plan right now", never a pass.
//!
//! Every network call is behind [`RelayApi`]; tests install a fake
//! ([`set_test_api`]) and never dial anything.

use std::time::Duration;

use serde_json::{json, Value as J};

use k2_core::domains::{AppDomain, RELAY_CLAIMED, RELAY_RELEASED, RELAY_VERIFIED};

/// The control plane's answer for one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimAnswer {
    pub hostname: String,
    /// `pending` | `verified` | `failing` | `released`.
    pub status: String,
    pub last_error: Option<String>,
}

/// Why a call didn't give an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpError {
    /// The control plane said no: its code and K2's plain words.
    Refused { code: String, words: String },
    /// No answer (network, a 5xx, a body K2 can't read).
    Unreachable(String),
}

/// The four calls (production: [`LiveApi`]; tests: a fake).
pub trait RelayApi {
    fn claim(&self, hostname: &str, verify_token: &str) -> Result<ClaimAnswer, CpError>;
    fn verify(&self, hostname: &str) -> Result<ClaimAnswer, CpError>;
    fn release(&self, hostname: &str) -> Result<(), CpError>;
    fn list(&self) -> Result<Vec<ClaimAnswer>, CpError>;
}

/// Overrides the control plane's base for these calls (staging, tests).
pub const BASE_ENV: &str = "K2_CUSTOM_HOSTNAMES_BASE";
/// The capability the claim body announces: this daemon's E2E listener
/// fails the handshake for names it doesn't serve (PB-1, CD-A12), so the
/// relay may pass customer names to it.
pub const CAPABILITY: &str = "customSni";
/// Where a person upgrades to Pro.
pub const UPGRADE_URL: &str = "https://k2.dev/dashboard";

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// `{base}/api/custom-hostnames`.
pub fn endpoint() -> String {
    let base = match std::env::var(BASE_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => k2_core::tunnel::subdomains::control_plane_base(),
    };
    format!("{base}/api/custom-hostnames")
}

/// One hostname as a path segment (lowercase; only DNS name characters
/// ever reach here, but anything else is percent-encoded).
fn seg(hostname: &str) -> String {
    hostname
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Plain words for a control-plane refusal code (CLI, Settings).
pub fn words_for(code: &str, hostname: &str, v: &J) -> String {
    let detail = v.get("detail").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty());
    match code {
        "pro_required" => format!(
            "K2 Connect says this box's plan isn't Pro: your own domain through K2's relays needs Pro. Upgrade at {UPGRADE_URL}"
        ),
        "cap_reached" => {
            let cap = v.get("cap").and_then(J::as_u64);
            let count = v.get("count").and_then(J::as_u64);
            match (count, cap) {
                (Some(n), Some(c)) => format!(
                    "K2 Connect's limit is reached ({n} of {c} names): remove a name you no longer need (k2 app domain rm), then try again"
                ),
                _ => "K2 Connect's limit on custom names is reached (20 per server, 100 per account): remove a name you no \
                      longer need (k2 app domain rm), then try again"
                    .to_string(),
            }
        }
        "hostname_claimed_elsewhere" => format!(
            "{hostname} is already verified for another K2 account. Its owner removes it there first; K2 never takes over another account's name"
        ),
        "capability_missing" => {
            "K2 Connect says this K2 can't serve custom names yet: update K2 on this server (the owner runs k2 update), then try again".to_string()
        }
        "rate_limited" => "K2 Connect is taking claims slowly from this account right now (5 an hour): try again later".to_string(),
        "txt_missing" | "not_verified" | "pending" => format!(
            "K2 Connect can't see the TXT record yet (_k2-domain.{hostname}). It checks through public DNS, which can take a few minutes"
        ),
        other => match detail {
            Some(d) => format!("K2 Connect won't route {hostname}: {d} ({other})"),
            None => format!("K2 Connect won't route {hostname} ({other})"),
        },
    }
}

/// Parse one answer object.
fn answer(v: &J, hostname: &str) -> Result<ClaimAnswer, CpError> {
    let status = v.get("status").and_then(J::as_str).map(str::to_ascii_lowercase).ok_or_else(|| {
        CpError::Unreachable(format!("K2 Connect's answer for {hostname} has no status"))
    })?;
    Ok(ClaimAnswer {
        hostname: v.get("hostname").and_then(J::as_str).unwrap_or(hostname).to_ascii_lowercase(),
        status,
        last_error: v.get("lastError").and_then(J::as_str).map(str::to_string),
    })
}

/// Map an HTTP answer (status, body) to an answer or an error.
pub fn parse_reply(status: u16, body: &str, hostname: &str) -> Result<J, CpError> {
    let v: J = serde_json::from_str(body).unwrap_or(J::Null);
    if (200..300).contains(&status) {
        return Ok(v);
    }
    if let Some(code) = v.get("error").and_then(J::as_str).map(str::trim).filter(|c| !c.is_empty()) {
        if (400..500).contains(&status) {
            return Err(CpError::Refused { code: code.to_string(), words: words_for(code, hostname, &v) });
        }
    }
    if status == 401 {
        return Err(CpError::Refused {
            code: "unauthorized".into(),
            words: "K2 Connect refused this server's tunnel token: re-pair K2 Connect (k2 tunnel status)".into(),
        });
    }
    if status == 404 {
        return Err(CpError::Unreachable(
            "K2 Connect's custom-name service isn't open yet (HTTP 404): Path B starts working once K2's relays turn it on".into(),
        ));
    }
    Err(CpError::Unreachable(format!("K2 Connect answered HTTP {status}")))
}

/// Production: blocking HTTPS to the control plane with the tunnel token.
/// The token is never logged.
pub struct LiveApi;

impl LiveApi {
    fn call(&self, method: &str, url: &str, body: Option<J>, hostname: &str) -> Result<J, CpError> {
        k2_core::airgap::refuse().map_err(CpError::Unreachable)?;
        let token = crate::dns::proxy::tunnel_bearer_token().map_err(CpError::Unreachable)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CpError::Unreachable(format!("http client: {e}")))?;
        let mut b = match method {
            "GET" => client.get(url),
            _ => client.post(url),
        };
        b = b.header("Authorization", crate::dns::proxy::authorization_header(&token)).header("Accept", "application/json");
        if let Some(body) = body {
            b = b.header("Content-Type", "application/json").body(body.to_string());
        }
        let r = b.send().map_err(|e| CpError::Unreachable(format!("K2 Connect unreachable: {e}")))?;
        let status = r.status().as_u16();
        let text = r.text().map_err(|e| CpError::Unreachable(format!("read K2 Connect's answer: {e}")))?;
        parse_reply(status, &text, hostname)
    }
}

impl RelayApi for LiveApi {
    fn claim(&self, hostname: &str, verify_token: &str) -> Result<ClaimAnswer, CpError> {
        let body = json!({ "hostname": seg(hostname), "verifyToken": verify_token, "capabilities": [CAPABILITY] });
        let v = self.call("POST", &endpoint(), Some(body), hostname)?;
        answer(&v, hostname)
    }
    fn verify(&self, hostname: &str) -> Result<ClaimAnswer, CpError> {
        let v = self.call("POST", &format!("{}/{}/verify", endpoint(), seg(hostname)), Some(json!({})), hostname)?;
        answer(&v, hostname)
    }
    fn release(&self, hostname: &str) -> Result<(), CpError> {
        match self.call("POST", &format!("{}/{}/release", endpoint(), seg(hostname)), Some(json!({})), hostname) {
            Ok(_) => Ok(()),
            // Already gone there: released.
            Err(CpError::Refused { code, .. }) if code == "not_found" => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
        let v = self.call("GET", &endpoint(), None, "")?;
        let rows = v.get("hostnames").and_then(J::as_array).cloned().unwrap_or_default();
        rows.iter().map(|r| answer(r, r.get("hostname").and_then(J::as_str).unwrap_or(""))).collect()
    }
}

#[cfg(test)]
thread_local! {
    static TEST_API: std::cell::RefCell<Option<std::rc::Rc<dyn RelayApi>>> = const { std::cell::RefCell::new(None) };
    static TEST_PRO: std::cell::RefCell<Option<Result<bool, String>>> = const { std::cell::RefCell::new(None) };
    static TEST_READY: std::cell::RefCell<Option<Result<(), String>>> = const { std::cell::RefCell::new(None) };
}

/// Test seam: the control plane on this thread (`None` clears).
#[cfg(test)]
pub(crate) fn set_test_api(api: Option<std::rc::Rc<dyn RelayApi>>) {
    TEST_API.with(|c| *c.borrow_mut() = api);
}

/// Test seam: the plan answer (`Ok(true)` = Pro, `Err` = can't tell).
#[cfg(test)]
pub(crate) fn set_test_pro(v: Option<Result<bool, String>>) {
    TEST_PRO.with(|c| *c.borrow_mut() = v);
}

/// Test seam: whether this box is behind K2 Connect with E2E.
#[cfg(test)]
pub(crate) fn set_test_ready(v: Option<Result<(), String>>) {
    TEST_READY.with(|c| *c.borrow_mut() = v);
}

/// Run `f` with the control plane (tests: the fake; none installed → an
/// unreachable control plane, never the network).
pub fn with_api<T>(f: impl FnOnce(&dyn RelayApi) -> T) -> T {
    #[cfg(test)]
    {
        struct Down;
        impl RelayApi for Down {
            fn claim(&self, _: &str, _: &str) -> Result<ClaimAnswer, CpError> {
                Err(CpError::Unreachable("no control plane in tests".into()))
            }
            fn verify(&self, _: &str) -> Result<ClaimAnswer, CpError> {
                Err(CpError::Unreachable("no control plane in tests".into()))
            }
            fn release(&self, _: &str) -> Result<(), CpError> {
                Err(CpError::Unreachable("no control plane in tests".into()))
            }
            fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
                Err(CpError::Unreachable("no control plane in tests".into()))
            }
        }
        let api = TEST_API.with(|c| c.borrow().clone());
        return match api {
            Some(a) => f(a.as_ref()),
            None => f(&Down),
        };
    }
    #[cfg(not(test))]
    f(&LiveApi)
}

/// This box's K2 Connect label (`<sub>` of `<sub>.k2.dev`), or empty.
pub fn box_label() -> String {
    crate::apps::origins::box_subdomain()
}

/// Is this box behind K2 Connect with the E2E listener (Path B's way in)?
/// `Err` says what's missing, in plain words.
pub fn ready() -> Result<(), String> {
    #[cfg(test)]
    if let Some(v) = TEST_READY.with(|c| c.borrow().clone()) {
        return v;
    }
    if k2_core::airgap::enabled() {
        return Err("air-gap is on: no K2 Connect, so no Path B".into());
    }
    let cfg = k2_core::tunnel::config::load_without_token().map_err(|e| format!("K2 Connect isn't set up here ({e})"))?;
    if cfg.subdomain.trim().is_empty() {
        return Err("this server isn't on K2 Connect yet (no <name>.k2.dev): pair it first (k2 connect login)".into());
    }
    if !k2_core::tunnel::e2e_enabled(&cfg) {
        return Err("this server's K2 Connect tunnel has end-to-end TLS off: Path B needs it on (K2_E2E)".into());
    }
    Ok(())
}

/// Is this box on Pro? From the cached plan reading (5 min fresh, an hour
/// usable, `k2_core::tunnel::subdomains::tier_view`); with none, one live
/// read. `Err`: K2 can't tell right now (never a silent pass, PB-A5).
pub fn pro() -> Result<bool, String> {
    #[cfg(test)]
    if let Some(v) = TEST_PRO.with(|c| c.borrow().clone()) {
        return v;
    }
    #[cfg(test)]
    return Err("no plan in tests".into());
    #[cfg(not(test))]
    {
        use k2_core::tunnel::subdomains;
        let label = box_label();
        if let Some(r) = subdomains::tier_view(&label).reading() {
            return Ok(r.is_pro());
        }
        let token = crate::dns::proxy::tunnel_bearer_token().map_err(|e| format!("K2 can't check this server's plan right now: {e}"))?;
        subdomains::refresh_tier(&label, &token)
            .map(|r| r.is_pro())
            .map_err(|e| format!("K2 can't check this server's plan right now ({e}); try again in a minute"))
    }
}

/// The words for "not Pro" (with Path A where this box can serve it).
pub fn not_pro_words() -> String {
    let path_a = if cfg!(target_os = "linux") {
        " Or serve it from this server's own address (Path A, free): the owner turns on K2's :443 front (k2 front enable)."
    } else {
        ""
    };
    format!("Your own domain through K2's relays (Path B) needs K2 Connect Pro: upgrade at {UPGRADE_URL}.{path_a}")
}

/// What one claim step decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The control plane verified the name for this box: the relay routes it.
    Verified,
    /// Claimed; the control plane hasn't seen the TXT yet (plain words).
    Waiting(String),
    /// Refused, by the box's courtesy check or the control plane.
    Refused { code: String, words: String },
    /// No answer; nothing changed.
    Unreachable(String),
}

fn record(host: &str, state: Option<&str>, err: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = k2_core::domains::set_relay_state(&conn, host, state, err, chrono::Utc::now().timestamp());
}

fn claims_held() -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::relay_claims(&conn).unwrap_or(0)
}

/// The courtesy checks before a claim (PB-4): Path B's way in, Pro, the cap.
pub fn precheck(d: &AppDomain) -> Result<(), (String, String)> {
    ready().map_err(|w| ("connect_off".to_string(), w))?;
    match pro() {
        Ok(true) => {}
        Ok(false) => return Err(("pro_required".into(), not_pro_words())),
        Err(w) => return Err(("plan_unknown".into(), w)),
    }
    let cap = k2_core::tunnel::custom_names::MAX_PER_BOX;
    if !d.holds_relay_claim() && claims_held() >= cap {
        return Err((
            "cap_reached".into(),
            format!(
                "this server already has {cap} of {cap} names through K2's relays (the most on Pro): remove one you no longer \
                 need (k2 app domain rm <app-id> <name>), then try again"
            ),
        ));
    }
    Ok(())
}

/// Bring `d`'s claim to verified if it can (its TXT and DNS already pass on
/// the box). Records the claim on the row.
pub fn step(d: &AppDomain, api: &dyn RelayApi) -> Step {
    let host = d.hostname.as_str();
    if d.relay_state.as_deref() == Some(RELAY_VERIFIED) {
        return Step::Verified;
    }
    if let Err((code, words)) = precheck(d) {
        // Box-side: nothing was sent; the row keeps any earlier claim.
        return Step::Refused { code, words };
    }
    let claimed = if d.relay_state.as_deref() == Some(RELAY_CLAIMED) {
        api.verify(host)
    } else {
        match api.claim(host, &d.verify_token) {
            Ok(a) if a.status == "verified" => Ok(a),
            Ok(_) => api.verify(host),
            Err(e) => Err(e),
        }
    };
    match claimed {
        Ok(a) if a.status == "verified" || a.status == "failing" => {
            record(host, Some(RELAY_VERIFIED), a.last_error.as_deref());
            Step::Verified
        }
        Ok(a) if a.status == "released" => {
            let w = a.last_error.unwrap_or_else(|| "K2 Connect released it".into());
            record(host, Some(RELAY_RELEASED), Some(&w));
            Step::Refused { code: "released".into(), words: w }
        }
        Ok(a) => {
            let w = a.last_error.unwrap_or_else(|| words_for("pending", host, &J::Null));
            record(host, Some(RELAY_CLAIMED), Some(&w));
            Step::Waiting(w)
        }
        Err(CpError::Refused { code, words }) if code == "txt_missing" || code == "not_verified" => {
            record(host, Some(RELAY_CLAIMED), Some(&words));
            Step::Waiting(words)
        }
        Err(CpError::Refused { code, words }) => {
            record(host, Some(k2_core::domains::RELAY_REFUSED), Some(&words));
            Step::Refused { code, words }
        }
        Err(CpError::Unreachable(w)) => Step::Unreachable(w),
    }
}

/// Release `d`'s claim at the control plane (relay first, PB-3). `Ok`
/// also when it held none.
pub fn release(d: &AppDomain, api: &dyn RelayApi) -> Result<bool, String> {
    if !d.holds_relay_claim() {
        return Ok(false);
    }
    match api.release(&d.hostname) {
        Ok(()) => Ok(true),
        Err(CpError::Refused { words, .. }) | Err(CpError::Unreachable(words)) => Err(words),
    }
}

/// The control plane's list, applied (PB-A15: its `released` wins over a
/// passing box check). Returns the names it released.
pub fn sync(api: &dyn RelayApi) -> Result<Vec<String>, String> {
    let rows: Vec<AppDomain> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::app_domains(&conn).map_err(|e| e.to_string())?
    };
    if !rows.iter().any(AppDomain::holds_relay_claim) {
        return Ok(Vec::new());
    }
    let listed = match api.list() {
        Ok(v) => v,
        Err(CpError::Refused { words, .. }) | Err(CpError::Unreachable(words)) => return Err(words),
    };
    let mut released = Vec::new();
    for d in rows.iter().filter(|d| d.holds_relay_claim()) {
        let Some(a) = listed.iter().find(|a| a.hostname.eq_ignore_ascii_case(&d.hostname)) else { continue };
        match a.status.as_str() {
            "released" => {
                let w = format!(
                    "K2 Connect released it: {}",
                    a.last_error.as_deref().unwrap_or("its own re-check failed for 7 days")
                );
                record(&d.hostname, Some(RELAY_RELEASED), Some(&w));
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = k2_core::domains::set_app_state(&conn, &d.hostname, k2_core::domains::STATE_FAILING, Some(&w));
                released.push(d.hostname.clone());
            }
            "verified" | "failing" if d.relay_state.as_deref() != Some(RELAY_VERIFIED) || a.last_error != d.relay_error => {
                record(&d.hostname, Some(RELAY_VERIFIED), a.last_error.as_deref());
            }
            _ => {}
        }
    }
    Ok(released)
}

#[cfg(test)]
#[path = "relay_claims_tests.rs"]
mod tests;
