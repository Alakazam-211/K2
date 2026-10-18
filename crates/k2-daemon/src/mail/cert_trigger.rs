//! 0.46.1: re-request the mail certificate when the LIVE certificate is
//! wrong — self-signed (Stalwart's rcgen fallback), **expired**, or **not
//! covering the mail host** (mismatch) — not only at the boot reconcile's
//! one-shot "no real certificate served" retry.
//!
//! The live state comes from the probe in
//! [`crate::mail::supervisor::tls_cert_status`] (SNI = the mail host;
//! `issued` / `self-signed` / `mismatch` / `expired` / `unknown` /
//! `missing`). The trigger reads Stalwart's own listeners (465, 993) —
//! 443 can be fronted by something else, and re-ordering the mail
//! certificate would never fix a front's certificate. When neither of
//! those answers it falls back to the overall (worst-port) state.
//!
//! Who orders:
//! - **k2** box ([`CertOwner::K2`]): K2's own issuer
//!   ([`crate::domains::acme::issue_attached_core`]) — it re-plants the
//!   box store's certificate while that one is still good (no CA call)
//!   and orders a new one only inside the renewal window.
//! - **stalwart-acme** box on the tls-alpn plan: ONE `AcmeRenewal` task,
//!   only when the name list is locked to the mail host, the mail host
//!   points here and no Pending/Retry task exists
//!   ([`crate::mail::defaults::live_cert_order`]).
//! - anything else (owner unknown, Stalwart ACME on another plan, Manual):
//!   never orders.
//!
//! Backoff (never hammer Let's Encrypt): at most one order per window
//! ([`ORDER_WINDOW_SECS`]); an order that failed, or one that did not fix
//! the live certificate by the next eligible check, doubles the wait up to
//! [`BACKOFF_CAP_SECS`]; a live `issued` resets it. `unknown` / `missing`
//! (unparseable leaf, nothing answered) never order. State persists in
//! `~/.k2/certs/renewal.json` (`mailLiveReorder`) across restarts.
//!
//! Runs in the daemon (headless) on the certificate renewer's cadence
//! (~5 min after boot, then hourly — [`crate::domains::renew::spawn`]),
//! right after its scan. Skipped under air-gap and off Linux.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domains::renew::{IssueError, Issued};
use crate::mail::cert_owner::CertOwner;

/// One order per window: long enough for an ACME order (Stalwart's or
/// K2's) to finish and the hot reload to land before the next look.
pub const ORDER_WINDOW_SECS: i64 = 3_600;
/// The longest wait between orders while the live certificate stays wrong.
pub const BACKOFF_CAP_SECS: i64 = 24 * 3_600;
/// Stalwart's own implicit-TLS listeners (SMTP submission, IMAPS).
pub const STALWART_TLS_PORTS: [u64; 2] = [465, 993];

/// Why the live state needs a new certificate, or `None` (issued,
/// unknown, missing — never an order). Pure.
pub fn reorder_reason(state: &str) -> Option<&'static str> {
    match state {
        "self-signed" => Some("the mail host serves a self-signed certificate"),
        "expired" => Some("the served certificate has expired"),
        "mismatch" => Some("the served certificate does not cover the mail host"),
        _ => None,
    }
}

/// The state the trigger acts on, from a `tls_cert_status` body: the worst
/// ANSWERING Stalwart port (465/993); with neither answering, the overall
/// `state`. Pure.
pub fn trigger_state(cert: &Value) -> String {
    let own: Vec<&str> = cert["ports"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| p["port"].as_u64().is_some_and(|n| STALWART_TLS_PORTS.contains(&n)))
                .filter_map(|p| p["state"].as_str())
                .filter(|s| *s != "missing")
                .collect()
        })
        .unwrap_or_default();
    match own
        .into_iter()
        .max_by_key(|s| crate::mail::supervisor::verdict_rank(s))
    {
        Some(s) => s.to_string(),
        None => cert["state"].as_str().unwrap_or("missing").to_string(),
    }
}

/// Wait after an order, given the consecutive failures so far: the window,
/// doubled per failure, capped. Pure.
pub fn wait_after(failures: u32) -> i64 {
    ORDER_WINDOW_SECS
        .saturating_mul(1i64 << failures.min(20))
        .min(BACKOFF_CAP_SECS)
}

/// Which issuer re-orders the mail certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderPath {
    K2,
    StalwartAcme,
}

impl OrderPath {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderPath::K2 => "k2",
            OrderPath::StalwartAcme => "stalwart-acme",
        }
    }
}

/// Does K2 or Stalwart's ACME own (and so may re-order) the mail
/// certificate? `Err(why)` = neither: never order. Pure.
pub fn order_path(owner: CertOwner, port_plan: Option<&str>) -> Result<OrderPath, String> {
    match owner {
        CertOwner::K2 => Ok(OrderPath::K2),
        CertOwner::StalwartAcme if port_plan == Some("tls-alpn") => Ok(OrderPath::StalwartAcme),
        CertOwner::StalwartAcme => Err(format!(
            "port plan {} — Stalwart ACME runs on tls-alpn only and K2 does not own the \
             mail certificate",
            port_plan.unwrap_or("unset")
        )),
        CertOwner::Unknown => Err(
            "owner unknown — neither K2 nor Stalwart ACME owns the mail certificate".into(),
        ),
    }
}

/// What K2 last did about a wrong live certificate (`renewal.json`
/// `mailLiveReorder`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReorderState {
    /// The mail host this state is for (another host starts fresh).
    pub host: Option<String>,
    /// `k2` | `stalwart-acme`.
    pub path: Option<String>,
    /// When K2 last ordered; cleared once the live cert reads `issued`.
    pub last_order: Option<i64>,
    /// The live state that triggered it.
    pub last_state: Option<String>,
    /// `queued` | `renewed` | `planted` | `failed`.
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    /// Orders that failed or did not fix the live certificate.
    pub failures: u32,
    /// No order before this.
    pub next_attempt: Option<i64>,
}

/// The pure decision for one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The live certificate is issued, covers the host and is current.
    Healthy,
    /// Unreadable or no answer — never an order.
    NoAnswer(String),
    /// Neither K2 nor Stalwart ACME owns the certificate.
    NotOwned(String),
    /// Wrong, but an order was made inside the current wait.
    Backoff(i64),
    /// Order through this path.
    Order(OrderPath, &'static str),
}

pub fn decide(
    state: &str,
    owner: CertOwner,
    port_plan: Option<&str>,
    rs: &ReorderState,
    now: i64,
) -> Decision {
    if state == "issued" {
        return Decision::Healthy;
    }
    let Some(reason) = reorder_reason(state) else {
        return Decision::NoAnswer(format!("the live certificate reads {state}"));
    };
    let path = match order_path(owner, port_plan) {
        Ok(p) => p,
        Err(why) => return Decision::NotOwned(why),
    };
    if let Some(t) = rs.next_attempt.filter(|t| *t > now) {
        return Decision::Backoff(t);
    }
    Decision::Order(path, reason)
}

/// A live `issued`: the last order worked (or nothing was wrong). Returns
/// whether anything changed.
pub fn record_healthy(rs: &mut ReorderState) -> bool {
    if rs.failures == 0 && rs.next_attempt.is_none() && rs.last_order.is_none() {
        return false;
    }
    rs.failures = 0;
    rs.next_attempt = None;
    rs.last_order = None;
    rs.last_error = None;
    true
}

/// One order was made (`Ok(label)`) or failed (`Err(error)`). An order
/// still unresolved from before (the live cert never read `issued` since)
/// counts as a failure too.
pub fn record_order(
    rs: &mut ReorderState,
    host: &str,
    path: OrderPath,
    state: &str,
    now: i64,
    result: Result<&str, String>,
) {
    let unresolved = rs.last_order.is_some();
    rs.host = Some(host.to_string());
    rs.path = Some(path.as_str().into());
    rs.last_order = Some(now);
    rs.last_state = Some(state.to_string());
    match result {
        Ok(label) => {
            rs.last_result = Some(label.to_string());
            rs.last_error = None;
            if unresolved {
                rs.failures = rs.failures.saturating_add(1);
            }
        }
        Err(e) => {
            rs.last_result = Some("failed".into());
            rs.last_error = Some(e);
            rs.failures = rs.failures.saturating_add(1);
        }
    }
    rs.next_attempt = Some(now + wait_after(rs.failures));
}

/// The Stalwart-ACME order's outcome ([`crate::mail::defaults::live_cert_order`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalwartOrder {
    /// One `AcmeRenewal` task queued.
    Queued(String),
    /// Not Stalwart ACME's to order (Manual, another type, no Domain).
    NotEligible(String),
    /// Not now (name list not locked, host elsewhere, a live task, a read
    /// failed, the mail server busy). Not an attempt.
    NotNow(String),
    /// The queue call itself failed. An attempt.
    Failed(String),
}

/// Everything a check does besides the pure decision. Production =
/// [`LiveTrigger`]; tests inject a recording fake.
pub trait TriggerDeps {
    fn now(&self) -> i64;
    /// A `tls_cert_status` body for `host`.
    fn served(&self, host: &str) -> Value;
    fn owner(&self, host: &str) -> CertOwner;
    fn port_plan(&self) -> Option<String>;
    fn renew_k2(&mut self, host: &str) -> Result<Issued, IssueError>;
    fn order_stalwart(&mut self, host: &str) -> StalwartOrder;
    fn load_state(&self) -> ReorderState;
    fn save_state(&mut self, rs: &ReorderState) -> Result<(), String>;
}

/// What one check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// An order went out (K2 issuer run or one Stalwart task).
    pub ordered: bool,
    pub line: String,
}

const P: &str = "[mail/cert-trigger]";

/// One check for `host`. `k2_scan_touched` = the renewer's scan already
/// handled (or skipped) this name this pass — the K2 path then waits for
/// the next check instead of running the issuer twice.
pub fn run_once(deps: &mut dyn TriggerDeps, host: &str, k2_scan_touched: bool) -> CheckOutcome {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let now = deps.now();
    let mut rs = deps.load_state();
    if rs.host.as_deref() != Some(host.as_str()) {
        rs = ReorderState { host: Some(host.clone()), ..Default::default() };
    }
    let state = trigger_state(&deps.served(&host));
    let owner = deps.owner(&host);
    let plan = deps.port_plan();
    let done = |line: String| CheckOutcome { ordered: false, line };
    match decide(&state, owner, plan.as_deref(), &rs, now) {
        Decision::Healthy => {
            if record_healthy(&mut rs) {
                let saved = deps.save_state(&rs).err();
                return done(format!(
                    "{P} {host}: the live certificate is issued — backoff reset{}",
                    saved.map(|e| format!(" (could not record it: {e})")).unwrap_or_default()
                ));
            }
            done(format!("{P} {host}: the live certificate is issued"))
        }
        Decision::NoAnswer(why) => done(format!("{P} {host}: no order — {why}")),
        Decision::NotOwned(why) => done(format!("{P} {host}: {state}, no order — {why}")),
        Decision::Backoff(t) => done(format!(
            "{P} {host}: {state}, no order — the last order was at {} (backoff until {t})",
            rs.last_order.map(|a| a.to_string()).unwrap_or_else(|| "?".into())
        )),
        Decision::Order(path, reason) => {
            let result: Option<Result<&str, String>> = match path {
                OrderPath::K2 if k2_scan_touched => {
                    return done(format!(
                        "{P} {host}: {state}, no order — the renewer's scan handled {host} this pass"
                    ))
                }
                OrderPath::K2 => match deps.renew_k2(&host) {
                    Ok(i) if i.ordered => Some(Ok("renewed")),
                    Ok(_) => Some(Ok("planted")),
                    Err(IssueError::Busy(_)) | Err(IssueError::NotQualified(_)) => None,
                    Err(e) => Some(Err(e.to_string())),
                },
                OrderPath::StalwartAcme => match deps.order_stalwart(&host) {
                    StalwartOrder::Queued(_) => Some(Ok("queued")),
                    StalwartOrder::Failed(e) => Some(Err(e)),
                    StalwartOrder::NotEligible(why) | StalwartOrder::NotNow(why) => {
                        return done(format!("{P} {host}: {state}, no order — {why}"))
                    }
                },
            };
            let Some(result) = result else {
                return done(format!(
                    "{P} {host}: {state}, no order — K2's issuer is busy or {host} no longer \
                     qualifies"
                ));
            };
            let ok = result.is_ok();
            let label = match &result {
                Ok(l) => l.to_string(),
                Err(e) => format!("FAILED: {e}"),
            };
            record_order(&mut rs, &host, path, &state, now, result);
            let saved = deps.save_state(&rs).err();
            CheckOutcome {
                ordered: true,
                line: format!(
                    "{P} {host}: {reason} ({state}) — ONE {} order: {label}{}; next order not \
                     before {}{}",
                    path.as_str(),
                    if ok { "" } else { " (backoff)" },
                    rs.next_attempt.unwrap_or(now),
                    saved.map(|e| format!(" (could not record it: {e})")).unwrap_or_default()
                ),
            }
        }
    }
}

/// The boot reconcile's one guarded retry (HF8) queued a Stalwart order:
/// count it against the window so the hourly check does not order again
/// right behind it.
pub fn note_boot_order(host: &str, state: &str, now: i64) {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let r = crate::domains::renew::update(|f| {
        let mut rs = f.mail_live_reorder.take().unwrap_or_default();
        if rs.host.as_deref() != Some(host.as_str()) {
            rs = ReorderState::default();
        }
        record_order(&mut rs, &host, OrderPath::StalwartAcme, state, now, Ok("queued"));
        f.mail_live_reorder = Some(rs);
    });
    if let Err(e) = r {
        k2_core::log_debug!("{P} {host}: could not record the boot order: {e}");
    }
}

// ── Live wiring ─────────────────────────────────────────────────────────

/// Production [`TriggerDeps`].
pub struct LiveTrigger;

impl TriggerDeps for LiveTrigger {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
    fn served(&self, host: &str) -> Value {
        crate::mail::supervisor::tls_cert_status(Some(host))
    }
    fn owner(&self, host: &str) -> CertOwner {
        crate::mail::cert_owner::mail_cert_owner(host)
    }
    fn port_plan(&self) -> Option<String> {
        crate::mail::supervisor::row_field("port_plan")
    }
    fn renew_k2(&mut self, host: &str) -> Result<Issued, IssueError> {
        let result = crate::domains::acme::issue_attached_core(host);
        if !matches!(result, Err(IssueError::Busy(_)) | Err(IssueError::NotQualified(_))) {
            let now = self.now();
            let ordered = result.as_ref().map(|i| i.ordered).map_err(Clone::clone);
            let name = host.to_string();
            if let Err(e) = crate::domains::renew::update(|f| {
                crate::domains::renew::record_attempt(
                    f.names.entry(name).or_default(),
                    "live-cert",
                    now,
                    &ordered,
                )
            }) {
                k2_core::log_debug!("{P} {host}: could not record the issuer run: {e}");
            }
        }
        result
    }
    fn order_stalwart(&mut self, host: &str) -> StalwartOrder {
        use crate::mail::supervisor;
        let client = match supervisor::mgmt_client_from_row() {
            Ok(c) => c,
            Err(e) => return StalwartOrder::NotNow(format!("no management client: {e}")),
        };
        if !supervisor::try_begin_enable() {
            return StalwartOrder::NotNow(
                "an enable, upgrade or reconcile holds the mail server — next check".into(),
            );
        }
        struct EndEnable;
        impl Drop for EndEnable {
            fn drop(&mut self) {
                crate::mail::supervisor::end_enable();
            }
        }
        let _guard = EndEnable;
        let points = || crate::mail::defaults::live_points_here(host);
        crate::mail::defaults::live_cert_order(&client, host, &points)
    }
    fn load_state(&self) -> ReorderState {
        crate::domains::renew::load().mail_live_reorder.unwrap_or_default()
    }
    fn save_state(&mut self, rs: &ReorderState) -> Result<(), String> {
        let rs = rs.clone();
        crate::domains::renew::update(|f| f.mail_live_reorder = Some(rs))
    }
}

/// The renewer's hook (after each scan). `scan` = that scan's report
/// (`None` when it panicked: the K2 path then waits a pass). Linux +
/// installed + enable completed only; never under air-gap or while an
/// enable/upgrade runs.
pub fn check_live(scan: Option<&crate::domains::renew::ScanReport>) {
    use crate::mail::supervisor;
    use std::sync::atomic::Ordering;
    if cfg!(test) || !supervisor::mail_supported() || k2_core::airgap::enabled() {
        return;
    }
    if scan.is_some_and(|s| s.airgap) {
        return;
    }
    if crate::mail::defaults::gate(supervisor::current_status().as_deref(), supervisor::enable_completed())
        .is_some()
    {
        return;
    }
    if supervisor::enable_running().load(Ordering::SeqCst)
        || supervisor::upgrade_running().load(Ordering::SeqCst)
    {
        k2_core::log_debug!("{P} skipped — an enable or upgrade holds the mail server");
        return;
    }
    let Some(host) = supervisor::row_field("hostname")
        .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|h| !h.is_empty())
    else {
        return;
    };
    let touched = scan.is_none_or(|s| s.items.iter().any(|i| i.name == host));
    let out = run_once(&mut LiveTrigger, &host, touched);
    k2_core::log_debug!("{}", out.line);
    if out.ordered {
        crate::mail::defaults::invalidate_status_cache();
    }
}

#[cfg(test)]
mod tests;
