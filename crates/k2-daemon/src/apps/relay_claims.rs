//! Path B's claim on a custom name at K2 Connect's control plane (0.46.8,
//! `prd-custom-domains-path-b-0467-v1` PB-3/PB-4; vs-live PB-A5, PB-A15).
//!
//! A customer's name reaches this box through K2's relays only once the
//! control plane has verified it for this box's label: the relay's frps
//! plugin registers `k2-custom-<label>`'s `customDomains` as
//! `requested ∩ verified(label)`. The box asks with its tunnel token:
//!
//! ```text
//! POST {base}/api/custom-hostnames               {hostname, verifyToken, capabilities:["customSni"], httpRedirect}
//! POST {base}/api/custom-hostnames/verify        {hostname}
//! POST {base}/api/custom-hostnames/release       {hostname}
//! POST {base}/api/custom-hostnames/http-redirect {hostname?, enabled}   the relay's :80 (no hostname: all of them)
//! GET  {base}/api/custom-hostnames               this label's names (released ones too), counts, limits
//! ```
//!
//! `{base}` is K2 Connect's control plane (`K2_CUSTOM_HOSTNAMES_BASE`, else
//! `K2_CONNECT_BASE`, else `https://connect.k2.dev`); k2-connect's
//! `control-plane/src/custom_hostnames.rs`. Answers carry `{hostname, status:
//! pending|verified|failing|released, lastError?, …}` (one, or `{names:[…],
//! counts, limits}` for the list; a new claim is 201, repeating a held one
//! 200); refusals `{error: <code>, detail, scope?, count?, limit?}`. It does
//! the authoritative TXT check through public resolvers (never the box's
//! word), uniqueness, the deny list, the Pro gate, the caps (20 held names per
//! box, 100 per account; held = pending, verified or failing) and 5 new claims
//! an hour per account. `failing` still routes (`verified` here). Release,
//! list and the :80 switch work without Pro, so a downgraded box can clean up.
//!
//! None of these refusals is about this agent's rights: those stop earlier,
//! at the one broad App switch (`apps_manage_off`, `Ask with: k2 permission
//! request apps.build`). These are about the server's plan, the name or K2
//! Connect, and each one's words say who can act.
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
    /// K2 Connect answered, and it may well go through later (its rate
    /// limit, a race, its DNS or database down): plain words; nothing sticks.
    Retry(String),
    /// No answer (network, a body K2 can't read).
    Unreachable(String),
}

/// The four calls (production: [`LiveApi`]; tests: a fake).
pub trait RelayApi {
    fn claim(&self, hostname: &str, verify_token: &str, http_redirect: bool) -> Result<ClaimAnswer, CpError>;
    fn verify(&self, hostname: &str) -> Result<ClaimAnswer, CpError>;
    fn release(&self, hostname: &str) -> Result<(), CpError>;
    fn list(&self) -> Result<Vec<ClaimAnswer>, CpError>;
    /// The relay's :80 redirect for one name, or all of this box's (`None`).
    fn http_redirect(&self, hostname: Option<&str>, enabled: bool) -> Result<(), CpError>;
}

/// Overrides the control plane's base for these calls (staging, tests).
pub const BASE_ENV: &str = "K2_CUSTOM_HOSTNAMES_BASE";
/// The capability the claim body announces: this daemon's E2E listener
/// fails the handshake for names it doesn't serve (PB-1, CD-A12), so the
/// relay may pass customer names to it.
pub const CAPABILITY: &str = "customSni";
/// What K2 says while frps refuses the custom proxy: not verified on the
/// relay yet, or another box's live proxy still holds the name. Not a failure.
pub const WAITING_TO_TAKE_OVER: &str = "waiting to take over at K2's relay (it isn't routing this name here yet; if another server held it, that server's tunnel must let go first; K2 tries again)";
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

/// A hostname as the API takes it: lowercase, no trailing dot.
fn host_arg(hostname: &str) -> String {
    hostname.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Plain words for a control-plane refusal code (CLI, Settings). `v` is its
/// body (`detail`, and for `cap_reached` `scope`, `count`, `limit`).
pub fn words_for(code: &str, hostname: &str, v: &J) -> String {
    let detail = v.get("detail").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty());
    let d = detail.map(|d| format!(": {d}")).unwrap_or_default();
    match code {
        "pro_required" => format!(
            "K2 Connect says this server's plan isn't Pro: your own domain through K2's relays needs Pro. The owner upgrades at {UPGRADE_URL}"
        ),
        "cap_reached" => {
            let n = v.get("count").and_then(J::as_u64);
            let l = v.get("limit").and_then(J::as_u64);
            let (what, default_l) = match v.get("scope").and_then(J::as_str) {
                Some("account") => ("this K2 account", 100),
                _ => ("this server", 20),
            };
            let of = match (n, l) {
                (Some(n), Some(l)) => format!("{n} of {l}"),
                _ => format!("the most ({default_l})"),
            };
            format!(
                "{what} already holds {of} names through K2's relays: remove one you no longer need (k2 app domain rm <app-id> <name>), then try again"
            )
        }
        "hostname_claimed_elsewhere" => match detail {
            // k2-connect names the case: another box of this account, or another account.
            Some(d) if d.contains("your account") => format!("{d}. K2 never takes a name over"),
            _ => format!(
                "{hostname} is already verified for another K2 account. Its owner removes it there first; K2 never takes over another account's name"
            ),
        },
        "capability_missing" => {
            "K2 Connect says this K2 can't serve custom names yet: update K2 on this server (the owner runs k2 update), then try again".to_string()
        }
        "invalid_token" => {
            "K2 Connect refused this server's tunnel token: the owner re-pairs this server (k2 tunnel status shows how)".to_string()
        }
        "nested_label" => {
            "this server's tunnel token is for a nested label, which has no tunnel of its own: the owner re-pairs it under the server's main name".to_string()
        }
        "hostname_invalid" => format!(
            "K2 Connect says {hostname} isn't a valid domain name{d}. Check the spelling: k2 app domain rm, then add it again"
        ),
        "hostname_not_allowed" => format!(
            "K2 Connect won't route {hostname}{d}. Its k2.dev zones, public suffixes and its deny list can't be claimed; use another name"
        ),
        "bad_verify_token" => format!(
            "K2 Connect refused this name's proof token (16 to 128 letters, digits, _ or -): remove the name and add it again to get a new one: k2 app domain rm <app-id> {hostname}, then k2 app domain add <app-id> {hostname}"
        ),
        "not_claimed" => format!("K2 Connect has no claim on {hostname} from this server; K2 claims it again at the next check"),
        "verification_failed" => format!(
            "K2 Connect can't see the right TXT record yet{d}. It checks through public DNS, which can take a few minutes; K2 tries again"
        ),
        "txt_missing" | "not_verified" | "pending" => format!(
            "K2 Connect can't see the TXT record yet (_k2-domain.{hostname}). It checks through public DNS, which can take a few minutes"
        ),
        "rate_limited" => "K2 Connect takes 5 new names an hour per account and this account is over that now: K2 tries again later".to_string(),
        "conflict" => format!("another change to {hostname} reached K2 Connect at the same moment: K2 tries again"),
        "supabase_error" => "K2 Connect's database didn't answer: K2 tries again".to_string(),
        "dns_unavailable" => "K2 Connect couldn't look up the TXT record right now: K2 tries again".to_string(),
        "custom_hostnames_disabled" => {
            "K2 Connect hasn't turned on custom names yet: Path B starts working once K2's relays do; K2 tries again".to_string()
        }
        other => format!("K2 Connect won't route {hostname} ({other}){d}"),
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
        let words = words_for(code, hostname, &v);
        // These may well go through later: nothing sticks to the name.
        if status >= 500 || matches!(code, "rate_limited" | "conflict") {
            return Err(CpError::Retry(words));
        }
        if (400..500).contains(&status) {
            return Err(CpError::Refused { code: code.to_string(), words });
        }
    }
    if status == 401 {
        return Err(CpError::Refused { code: "invalid_token".into(), words: words_for("invalid_token", hostname, &J::Null) });
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
    fn claim(&self, hostname: &str, verify_token: &str, http_redirect: bool) -> Result<ClaimAnswer, CpError> {
        let body = json!({
            "hostname": host_arg(hostname),
            "verifyToken": verify_token,
            "capabilities": [CAPABILITY],
            "httpRedirect": http_redirect,
        });
        let v = self.call("POST", &endpoint(), Some(body), hostname)?;
        answer(&v, hostname)
    }
    fn verify(&self, hostname: &str) -> Result<ClaimAnswer, CpError> {
        let v = self.call("POST", &format!("{}/verify", endpoint()), Some(json!({ "hostname": host_arg(hostname) })), hostname)?;
        answer(&v, hostname)
    }
    fn release(&self, hostname: &str) -> Result<(), CpError> {
        match self.call("POST", &format!("{}/release", endpoint()), Some(json!({ "hostname": host_arg(hostname) })), hostname) {
            Ok(_) => Ok(()),
            // Already gone there: released.
            Err(CpError::Refused { code, .. }) if code == "not_claimed" => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
        let v = self.call("GET", &endpoint(), None, "")?;
        let rows = v.get("names").and_then(J::as_array).cloned().unwrap_or_default();
        rows.iter().map(|r| answer(r, r.get("hostname").and_then(J::as_str).unwrap_or(""))).collect()
    }
    fn http_redirect(&self, hostname: Option<&str>, enabled: bool) -> Result<(), CpError> {
        let mut body = json!({ "enabled": enabled });
        if let Some(h) = hostname {
            body["hostname"] = json!(host_arg(h));
        }
        self.call("POST", &format!("{}/http-redirect", endpoint()), Some(body), hostname.unwrap_or("this server's names")).map(|_| ())
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
            fn claim(&self, _: &str, _: &str, _: bool) -> Result<ClaimAnswer, CpError> {
                Err(CpError::Unreachable("no control plane in tests".into()))
            }
            fn http_redirect(&self, _: Option<&str>, _: bool) -> Result<(), CpError> {
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
    // The relay's :80 follows the owner's switch (`k2 front http`).
    let redirect = k2_core::front443::load().http_on();
    let claim_then_verify = || match api.claim(host, &d.verify_token, redirect) {
        Ok(a) if a.status == "verified" || a.status == "failing" => Ok(a),
        Ok(_) => api.verify(host),
        Err(e) => Err(e),
    };
    let claimed = if d.relay_state.as_deref() == Some(RELAY_CLAIMED) {
        match api.verify(host) {
            // K2 Connect dropped an old claim (7 days unverified): claim again.
            Err(CpError::Refused { code, .. }) if code == "not_claimed" => claim_then_verify(),
            other => other,
        }
    } else {
        claim_then_verify()
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
        // Claimed; its own TXT check hasn't passed yet.
        Err(CpError::Refused { code, words }) if code == "verification_failed" => {
            record(host, Some(RELAY_CLAIMED), Some(&words));
            Step::Waiting(words)
        }
        Err(CpError::Refused { code, words }) => {
            record(host, Some(k2_core::domains::RELAY_REFUSED), Some(&words));
            Step::Refused { code, words }
        }
        Err(CpError::Retry(w)) => Step::Waiting(w),
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
        Err(CpError::Refused { words, .. }) | Err(CpError::Retry(words)) | Err(CpError::Unreachable(words)) => Err(words),
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
        Err(CpError::Refused { words, .. }) | Err(CpError::Retry(words)) | Err(CpError::Unreachable(words)) => return Err(words),
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

/// What frps last said about the tunnel's custom proxy (`setup.pathB.proxy`).
pub fn proxy_json() -> J {
    use k2_core::tunnel::custom_names::{proxy_state, ProxyState};
    match proxy_state() {
        ProxyState::Unknown => json!({ "state": "unknown" }),
        ProxyState::Registered => json!({ "state": "registered" }),
        ProxyState::Waiting(why) => json!({ "state": "waiting", "words": WAITING_TO_TAKE_OVER, "why": why }),
    }
}

/// The owner's :80 switch at the relay, for all this box's names
/// (`k2 front http`, no Pro needed). `Ok(false)`: this box holds none.
pub fn set_relay_redirect(enabled: bool, api: &dyn RelayApi) -> Result<bool, String> {
    if claims_held() == 0 {
        return Ok(false);
    }
    match api.http_redirect(None, enabled) {
        Ok(()) => Ok(true),
        Err(CpError::Refused { words, .. }) | Err(CpError::Retry(words)) | Err(CpError::Unreachable(words)) => Err(words),
    }
}

#[cfg(test)]
#[path = "relay_claims_tests.rs"]
mod tests;
