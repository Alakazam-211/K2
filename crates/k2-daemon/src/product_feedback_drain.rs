//! Send feedback to K2 — the outbox DRAIN (prd-k2-product-feedback-v1
//! PF16), copied from `federation_drain.rs`'s shape.
//!
//! Single-flight. Triggers: a new row ([`kick`]), the tunnel coming up (the
//! sweep notices), a 60 s sweep, and boot (the first sweep runs right
//! away, after `sending` rows went back to `queued`).
//!
//! Each pass, oldest first:
//! - nothing leaves while air-gap is on, sending is turned off, or the box
//!   isn't paired with K2 Connect: rows stay `queued`, the reason shown
//!   (`airgap`, `disabled`, `needs_connect`), no attempt counted;
//! - a box token comes from the relay (cached until shortly before it
//!   expires); `401` from the intake drops it and fetches a new one once;
//! - `201`/`200 {id}` → `sent`; another `4xx` → `refused`, never retried;
//! - anything else (the service not deployed yet, offline, `5xx`, `429`,
//!   `503 intake_paused`) → back to `queued`, the pass stops, and the next
//!   one waits: 30 s doubling to 2 h, ±20 %, or the intake's
//!   `retryAfterSecs`. After 7 days of that a row is `failed` (Retry puts
//!   it back).
//!
//! Every state change is a `product_feedback_status` event. What the drain
//! last saw of the service is [`service_seen`] (the settings answer shows
//! it, so K2 says plainly when the service isn't reachable yet).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;

use k2_core::log_debug;
use k2_core::product_feedback::store::{self, Row, State};
use k2_core::product_feedback::wire::{self, BoxToken, Outcome};
use k2_core::product_feedback::{self as pf, now_ms};

use crate::session_events::{emit, SessionEvent};

const BACKOFF_BASE_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 2 * 60 * 60;
const JITTER_FRAC: f64 = 0.2;
const TICK_SECS: u64 = 60;
/// A held row (no Connect, air-gap, off) is looked at again this soon.
const HOLD_RECHECK_MS: i64 = 60_000;
/// Rows per pass.
const BATCH: i64 = 20;
/// A cached token is used only while it has this long left.
const TOKEN_MARGIN_MS: i64 = 30_000;

/// Sweep cadence; `K2_PRODUCT_FEEDBACK_TICK_SECS` for tests.
fn tick() -> Duration {
    let secs = std::env::var("K2_PRODUCT_FEEDBACK_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(TICK_SECS);
    Duration::from_secs(secs)
}

/// 30 s, 60 s, … capped at 2 h for the Nth consecutive failure. Pure.
pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    let n = consecutive_failures.max(1) - 1;
    let secs = if n >= 32 { BACKOFF_CAP_SECS } else { BACKOFF_BASE_SECS.saturating_mul(1u64 << n).min(BACKOFF_CAP_SECS) };
    Duration::from_secs(secs)
}

/// ±20 % of `base` for `r` in [0,1). Pure.
pub fn with_jitter(base: Duration, r: f64) -> Duration {
    let r = r.clamp(0.0, 1.0);
    Duration::from_secs_f64(base.as_secs_f64() * (1.0 - JITTER_FRAC + 2.0 * JITTER_FRAC * r))
}

fn jitter_r() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

/// What the drain last saw of the feedback service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSeen {
    /// `ok` | `unreachable` | `paused` | `limited` | `held`.
    pub state: &'static str,
    /// The stored code (`service_unreachable`, `relay_not_ready`, …).
    pub code: Option<String>,
    pub detail: Option<String>,
    pub at_ms: i64,
}

#[derive(Debug, Default)]
struct DrainState {
    consecutive_failures: u32,
    /// Exponential backoff: no sweep pass before this.
    next_try_ms: i64,
    /// The intake's `retryAfterSecs`: nothing at all before this.
    hold_until_ms: i64,
    token: Option<BoxToken>,
    seen: Option<ServiceSeen>,
    tunnel_was_up: bool,
}

fn state() -> &'static Mutex<DrainState> {
    static S: std::sync::OnceLock<Mutex<DrainState>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(DrainState::default()))
}

fn lock() -> std::sync::MutexGuard<'static, DrainState> {
    state().lock().unwrap_or_else(|p| p.into_inner())
}

static BUSY: AtomicBool = AtomicBool::new(false);

struct Claim;
impl Claim {
    fn acquire() -> Option<Claim> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok().map(|_| Claim)
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// What the drain last saw (for the settings answer).
pub fn service_seen() -> Option<ServiceSeen> {
    lock().seen.clone()
}

/// Test hook: forget backoff, the cached token and what was seen.
pub fn reset_for_tests() {
    *lock() = DrainState::default();
}

/// Why nothing may leave right now, or `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    Airgap,
    Disabled,
    NeedsConnect,
}

impl Hold {
    pub fn code(&self) -> &'static str {
        match self {
            Hold::Airgap => "airgap",
            Hold::Disabled => "disabled",
            Hold::NeedsConnect => "needs_connect",
        }
    }
    pub fn message(&self) -> String {
        match self {
            Hold::Airgap => "Air-gap is on: this server sends nothing to the internet, feedback included.".into(),
            Hold::Disabled => {
                "Sending feedback to K2 is turned off on this server (Settings → General → Send feedback).".into()
            }
            Hold::NeedsConnect => format!(
                "This server isn't paired with K2 Connect, so it can't prove which server it is and can't send feedback. Copy your text and email {}.",
                pf::CONTACT_ADDRESS
            ),
        }
    }
}

/// The air-gap check every outbound send passes (push's, so a test can
/// turn it on for its own thread).
fn airgap_on() -> bool {
    crate::push_routes::airgap_blocks()
}

/// The tunnel's token and label, when paired.
pub fn tunnel_creds() -> Option<(String, String)> {
    let cfg = k2_core::tunnel::config::load().ok()?;
    let token = cfg.token.trim().to_string();
    let label = cfg.subdomain.trim().to_string();
    (!token.is_empty() && !label.is_empty()).then_some((token, label))
}

/// Whether this server may send now (PF7, PF14, PF19).
pub fn hold() -> Option<Hold> {
    if airgap_on() {
        return Some(Hold::Airgap);
    }
    if !pf::settings::load().enabled {
        return Some(Hold::Disabled);
    }
    if tunnel_creds().is_none() {
        return Some(Hold::NeedsConnect);
    }
    None
}

fn emit_row(id: &str, st: State, error: Option<&str>) {
    let _ = emit(SessionEvent::ProductFeedbackStatus {
        id: id.to_string(),
        state: st.as_str().to_string(),
        error: error.map(str::to_string),
    });
}

/// Boot: `sending` rows go back to `queued` (PF16).
pub fn recover_at_boot() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::recover_at_boot(&conn, now_ms()) {
        Ok(0) => {}
        Ok(n) => log_debug!("[product-feedback] boot: {n} row(s) were sending; queued again"),
        Err(e) => log_debug!("[product-feedback] boot recover failed: {e}"),
    }
}

/// Why a pass was skipped or what it did (for tests and logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    InFlight,
    Empty,
    Held(&'static str),
    BackingOff,
    Ran { sent: usize, refused: usize, retried: usize },
}

/// A new row: drain now, off the caller's thread, past the exponential
/// backoff (a person just pressed Send) but never past `retryAfterSecs`.
pub fn kick() {
    std::thread::Builder::new()
        .name("product-feedback-kick".into())
        .spawn(|| {
            let p = drain_once(true);
            log_debug!("[product-feedback] kick: {p:?}");
        })
        .map(|_| ())
        .unwrap_or_else(|e| log_debug!("[product-feedback] kick spawn failed: {e}"));
}

/// One pass. Blocking (network + disk).
pub fn drain_once(urgent: bool) -> Pass {
    let Some(_claim) = Claim::acquire() else { return Pass::InFlight };
    let now = now_ms();
    let due = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::give_up_stale(&conn, now) {
            Ok(ids) => {
                for id in ids {
                    let err = store::get(&conn, &id).ok().flatten().and_then(|r| r.last_error);
                    emit_row(&id, State::Failed, err.as_deref());
                }
            }
            Err(e) => log_debug!("[product-feedback] give-up pass failed: {e}"),
        }
        if let Err(e) = store::prune(&conn, now) {
            log_debug!("[product-feedback] prune failed: {e}");
        }
        match store::due(&conn, now, BATCH) {
            Ok(rows) => rows,
            Err(e) => {
                log_debug!("[product-feedback] due query failed: {e}");
                return Pass::Empty;
            }
        }
    };
    if due.is_empty() {
        return Pass::Empty;
    }
    if let Some(h) = hold() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let reason = format!("{}: {}", h.code(), h.message());
        for r in &due {
            let changed = r.last_error.as_deref() != Some(reason.as_str());
            let _ = store::mark_held(&conn, &r.id, now + HOLD_RECHECK_MS, &reason, now);
            if changed {
                emit_row(&r.id, State::Queued, Some(&reason));
            }
        }
        lock().seen = Some(ServiceSeen { state: "held", code: Some(h.code().into()), detail: Some(h.message()), at_ms: now });
        return Pass::Held(h.code());
    }
    {
        let st = lock();
        if now < st.hold_until_ms || (!urgent && now < st.next_try_ms) {
            return Pass::BackingOff;
        }
    }
    let (mut sent, mut refused, mut retried) = (0, 0, 0);
    for row in due {
        match send_one(&row) {
            Applied::Sent => sent += 1,
            Applied::Refused => refused += 1,
            Applied::Retry => {
                retried += 1;
                break;
            }
            Applied::Skipped => {}
        }
    }
    Pass::Ran { sent, refused, retried }
}

enum Applied {
    Sent,
    Refused,
    Retry,
    Skipped,
}

fn token(force_new: bool) -> Result<BoxToken, Outcome> {
    let now = now_ms();
    if !force_new {
        if let Some(t) = lock().token.clone().filter(|t| t.expires_at_ms - TOKEN_MARGIN_MS > now) {
            return Ok(t);
        }
    }
    lock().token = None;
    let (tunnel_token, label) = tunnel_creds().ok_or(Outcome::Retry {
        code: "needs_connect".into(),
        detail: Hold::NeedsConnect.message(),
        after_secs: None,
        reauth: false,
    })?;
    let base = k2_core::tunnel::subdomains::control_plane_base();
    match wire::fetch_token(&base, &tunnel_token, &label, &pf::audience(), now) {
        Ok(t) => {
            lock().token = Some(t.clone());
            Ok(t)
        }
        Err(e) => Err(e.as_retry()),
    }
}

fn send_one(row: &Row) -> Applied {
    let now = now_ms();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::mark_sending(&conn, &row.id, now) {
            Ok(true) => {}
            // Cancelled or taken meanwhile.
            _ => return Applied::Skipped,
        }
    }
    emit_row(&row.id, State::Sending, None);
    let endpoint = pf::endpoint();
    let outcome = match token(false) {
        Err(o) => o,
        Ok(t) => match wire::post_submission(&endpoint, &t.token, row) {
            Outcome::Retry { reauth: true, .. } => match token(true) {
                Ok(t2) => wire::post_submission(&endpoint, &t2.token, row),
                Err(o) => o,
            },
            other => other,
        },
    };
    apply(row, outcome)
}

fn apply(row: &Row, outcome: Outcome) -> Applied {
    let now = now_ms();
    let db = k2_core::db::shared();
    let conn = db.lock();
    match outcome {
        Outcome::Sent { remote_id, duplicate } => {
            let _ = store::mark_sent(&conn, &row.id, &remote_id, now);
            drop(conn);
            let mut st = lock();
            st.consecutive_failures = 0;
            st.next_try_ms = 0;
            st.seen = Some(ServiceSeen { state: "ok", code: None, detail: None, at_ms: now });
            drop(st);
            if duplicate {
                log_debug!("[product-feedback] {} was already received (duplicate)", row.id);
            }
            emit_row(&row.id, State::Sent, None);
            Applied::Sent
        }
        Outcome::Refused { code, detail } => {
            let reason = format!("{code}: {detail}");
            let _ = store::mark_refused(&conn, &row.id, &reason, now);
            drop(conn);
            lock().seen = Some(ServiceSeen { state: "ok", code: None, detail: None, at_ms: now });
            emit_row(&row.id, State::Refused, Some(&reason));
            Applied::Refused
        }
        Outcome::Retry { code, detail, after_secs, .. } => {
            let mut st = lock();
            st.consecutive_failures = st.consecutive_failures.saturating_add(1);
            let delay_ms = match after_secs {
                Some(s) => {
                    let ms = (s.max(1) as i64) * 1000;
                    st.hold_until_ms = now + ms;
                    ms
                }
                None => with_jitter(backoff_delay(st.consecutive_failures), jitter_r()).as_millis() as i64,
            };
            st.next_try_ms = now + delay_ms;
            let seen_state = match code.as_str() {
                wire::E_PAUSED => "paused",
                wire::E_RATE_LIMITED => "limited",
                _ => "unreachable",
            };
            st.seen = Some(ServiceSeen { state: seen_state, code: Some(code.clone()), detail: Some(detail.clone()), at_ms: now });
            drop(st);
            let reason = format!("{code}: {detail}");
            let _ = store::mark_retry(&conn, &row.id, now + delay_ms, &reason, now);
            drop(conn);
            log_debug!("[product-feedback] {} not sent ({reason}); next try in {}s", row.id, delay_ms / 1000);
            emit_row(&row.id, State::Queued, Some(&reason));
            Applied::Retry
        }
    }
}

/// One sweep: notices the tunnel coming up (resets the backoff), then a
/// pass. Blocking.
pub fn sweep_once() -> Pass {
    let up = k2_core::tunnel::connector::status().running;
    {
        let mut st = lock();
        if up && !st.tunnel_was_up {
            st.consecutive_failures = 0;
            st.next_try_ms = 0;
        }
        st.tunnel_was_up = up;
    }
    drain_once(false)
}

/// Spawn the sweeper (boot: recover `sending` rows, then sweep at once).
pub fn spawn() -> JoinHandle<()> {
    crate::product_feedback_diag::note_boot();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(recover_at_boot).await;
        let tick = tick();
        log_debug!("[product-feedback] drain started — tick={}s", tick.as_secs());
        loop {
            let _ = tokio::task::spawn_blocking(sweep_once).await;
            tokio::time::sleep(tick).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_30s_and_caps_at_2h() {
        assert_eq!(backoff_delay(1), Duration::from_secs(30));
        assert_eq!(backoff_delay(2), Duration::from_secs(60));
        assert_eq!(backoff_delay(9), Duration::from_secs(BACKOFF_CAP_SECS));
        assert_eq!(backoff_delay(0), Duration::from_secs(30));
        assert_eq!(with_jitter(Duration::from_secs(100), 0.0), Duration::from_secs(80));
        assert_eq!(with_jitter(Duration::from_secs(100), 1.0), Duration::from_secs(120));
    }

    #[test]
    fn single_flight() {
        let a = Claim::acquire().expect("first");
        assert!(Claim::acquire().is_none());
        drop(a);
        assert!(Claim::acquire().is_some());
    }
}
