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
//! - every POST gets its own fresh box token from the relay (0.46.18: the
//!   intake spends a token's `jti` on first sight, so a reused token is
//!   `401 replayed`). A `401 replayed` anyway mints a fresh one and sends
//!   the same row again at once, one time (safe: the intake keeps one row
//!   per clientId and answers a resend `200 {id, duplicate: true}`, which
//!   is sent). Any other `401` is a retryable failure, as before;
//! - at boot, once per server, rows an older drain left `failed` with
//!   `token_rejected: replayed` (it reused one token for a whole batch) go
//!   back to `queued` ([`requeue_replayed_once`]);
//! - `201`/`200 {id}` → `sent`; another `4xx` → `refused`, never retried;
//! - a row with a person's screenshot sends it in the same request (the
//!   stored bytes, every attempt, the same `clientId`), waiting up to 90 s;
//!   `sent` and `refused` delete the picture from the box, a retry or a
//!   timeout keeps it (prd-feedback-screenshot-v1 §5.5);
//! - anything else (the service not deployed yet, offline, `5xx`, `429`,
//!   `503 intake_paused`) → back to `queued`, the pass stops, and the next
//!   one waits: 30 s doubling to 2 h, ±20 %, or the intake's
//!   `retryAfterSecs`. After 7 days of that a row is `failed` (Retry puts
//!   it back);
//! - a row that failed because the service couldn't be reached (the codes
//!   in [`store::RESEND_BY_ITSELF`], created within 90 days) goes again by
//!   itself. While such rows wait and nothing else is queued, the drain asks
//!   the intake's `GET /health` at most once an hour (never while held:
//!   air-gap, sending off, no Connect). `open` → all of them go back to
//!   `queued` with a fresh 7-day window and leave oldest first, a batch a
//!   pass; `paused` → nothing, ask again in an hour; no answer or another
//!   status → nothing. The first real answer to a send (sent or refused)
//!   queues them again too. No row is sent to ask. A person's switch,
//!   air-gap, no K2 Connect pairing or a rejected token stay `failed` until
//!   Retry.
//!
//! Every state change is a `product_feedback_status` event. What the drain
//! last saw of the service is [`service_seen`] (the settings answer shows
//! it, so K2 says plainly when the service isn't reachable yet).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;

use k2_core::log_debug;
use k2_core::product_feedback::screenshot::Meta as ShotMeta;
use k2_core::product_feedback::store::{self, Row, State};
use k2_core::product_feedback::wire::{self, BoxToken, Health, Outcome, Screenshot};
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
/// The code migration that marks [`requeue_replayed_once`] done.
pub const REPLAYED_REQUEUE_ID: &str = "0.46.18-product-feedback-replayed-requeue";
/// The intake's health check is asked at most this often (only while rows
/// that couldn't reach the service wait and nothing else is queued).
const PROBE_EVERY_MS: i64 = 60 * 60_000;

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
    seen: Option<ServiceSeen>,
    tunnel_was_up: bool,
    /// No health check before this.
    next_probe_ms: i64,
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

/// Test hook: forget backoff and what was seen.
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

/// Boot: `sending` rows go back to `queued` (PF16), and, once per server,
/// [`requeue_replayed_once`].
pub fn recover_at_boot() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match store::recover_at_boot(&conn, now_ms()) {
        Ok(0) => {}
        Ok(n) => log_debug!("[product-feedback] boot: {n} row(s) were sending; queued again"),
        Err(e) => log_debug!("[product-feedback] boot recover failed: {e}"),
    }
    drop(conn);
    requeue_replayed_once();
}

/// Once per server (code migration [`REPLAYED_REQUEUE_ID`]): every `failed`
/// row whose last error is exactly `token_rejected: replayed` (what a drain
/// before 0.46.18 stored when it reused one box token for a batch), created
/// within 90 days, goes back to `queued` with a fresh 7-day window. Rows that
/// failed for any other reason stay failed. Returns the ids it queued.
pub fn requeue_replayed_once() -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if k2_core::db::has_code_migration_applied(&conn, REPLAYED_REQUEUE_ID) {
        return Vec::new();
    }
    let ids = match store::requeue_replayed_failed(&conn, now_ms()) {
        Ok(ids) => ids,
        Err(e) => {
            // Not marked: tried again at the next boot.
            log_debug!("[product-feedback] boot: requeue of replayed rows failed: {e}");
            return Vec::new();
        }
    };
    k2_core::db::mark_code_migration_applied(&conn, REPLAYED_REQUEUE_ID, Some(format!("{} row(s)", ids.len()).as_str()));
    drop(conn);
    if !ids.is_empty() {
        log_debug!("[product-feedback] boot: {} row(s) refused for a reused box token queued to send again", ids.len());
    }
    emit_requeued(&ids);
    ids
}

/// Why a pass was skipped or what it did (for tests and logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    InFlight,
    Empty,
    Held(&'static str),
    BackingOff,
    Ran { sent: usize, refused: usize, retried: usize },
    /// Nothing waited to go; the intake's health check said `health`
    /// (`open`, `paused`, `down`) and `requeued` failed rows went back in
    /// the queue.
    Probed { health: &'static str, requeued: usize },
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
        return probe_once(now);
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

/// Nothing waits to go, but rows failed because the service couldn't be
/// reached: ask the intake's health check, at most once an hour, never
/// inside a `retryAfterSecs` and never while sending is held (air-gap, off,
/// no Connect). `open` queues those rows again; `paused` and no answer
/// change nothing. No row is sent to ask.
fn probe_once(now: i64) -> Pass {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::pending_count(&conn) {
            Ok(0) => {}
            Ok(_) => return Pass::Empty,
            Err(e) => {
                log_debug!("[product-feedback] pending count failed: {e}");
                return Pass::Empty;
            }
        }
        match store::has_resendable_failed(&conn, now) {
            Ok(true) => {}
            Ok(false) => return Pass::Empty,
            Err(e) => {
                log_debug!("[product-feedback] failed-row query failed: {e}");
                return Pass::Empty;
            }
        }
    }
    if let Some(h) = hold() {
        return Pass::Held(h.code());
    }
    {
        let mut st = lock();
        if now < st.hold_until_ms || now < st.next_probe_ms {
            return Pass::BackingOff;
        }
        st.next_probe_ms = now + PROBE_EVERY_MS;
    }
    let health = wire::get_health(&pf::endpoint());
    let now = now_ms();
    let requeued = match &health {
        Health::Open => {
            let ids = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                requeue_after_answer(&conn, now)
            };
            let mut st = lock();
            st.consecutive_failures = 0;
            st.next_try_ms = 0;
            st.seen = Some(ServiceSeen { state: "ok", code: None, detail: None, at_ms: now });
            drop(st);
            emit_requeued(&ids);
            ids.len()
        }
        Health::Paused => {
            lock().seen = Some(ServiceSeen {
                state: "paused",
                code: Some(wire::E_PAUSED.into()),
                detail: Some("the feedback service's health check says it is paused".into()),
                at_ms: now,
            });
            log_debug!("[product-feedback] health check: the intake is paused; asking again in an hour");
            0
        }
        Health::Down(why) => {
            log_debug!("[product-feedback] health check: no answer ({why}); asking again in an hour");
            0
        }
    };
    Pass::Probed { health: health.label(), requeued }
}

/// The service answered (sent or refused a row): the rows that failed
/// because it couldn't be reached go back in the queue, oldest first, and
/// the normal drain sends them, a batch a pass. One log line says how many.
fn requeue_after_answer(conn: &rusqlite::Connection, now: i64) -> Vec<String> {
    match store::requeue_unreachable_failed(conn, now) {
        Ok(ids) => {
            if !ids.is_empty() {
                log_debug!(
                    "[product-feedback] the feedback service answers again: {} failed row(s) queued to send again",
                    ids.len()
                );
            }
            ids
        }
        Err(e) => {
            log_debug!("[product-feedback] queueing failed rows again failed: {e}");
            Vec::new()
        }
    }
}

enum Applied {
    Sent,
    Refused,
    Retry,
    Skipped,
}

/// A fresh box token from the relay: one per POST (its `jti` is spent on
/// first sight), never cached.
fn mint() -> Result<BoxToken, Outcome> {
    let now = now_ms();
    let (tunnel_token, label) = tunnel_creds().ok_or(Outcome::Retry {
        code: "needs_connect".into(),
        detail: Hold::NeedsConnect.message(),
        after_secs: None,
        reauth: false,
    })?;
    let base = k2_core::tunnel::subdomains::control_plane_base();
    wire::fetch_token(&base, &tunnel_token, &label, &pf::audience(), now).map_err(|e| e.as_retry())
}

/// The row's picture as it goes out, when it has one still on the box.
fn load_shot(conn: &rusqlite::Connection, row: &Row) -> Option<Screenshot> {
    let meta = row.screenshot_meta.as_deref().and_then(ShotMeta::parse)?;
    match store::screenshot(conn, &row.id) {
        Ok(Some(s)) => Some(Screenshot { png: s.png, meta: ShotMeta { sha256: s.sha256, bytes: meta.bytes, ..meta } }),
        Ok(None) => {
            log_debug!("[product-feedback] {}: its screenshot is no longer on the box; sending the text", row.id);
            None
        }
        Err(e) => {
            log_debug!("[product-feedback] {}: screenshot read failed: {e}", row.id);
            None
        }
    }
}

fn send_one(row: &Row) -> Applied {
    let now = now_ms();
    let shot = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::mark_sending(&conn, &row.id, now) {
            Ok(true) => {}
            // Cancelled or taken meanwhile.
            _ => return Applied::Skipped,
        }
        load_shot(&conn, row)
    };
    emit_row(&row.id, State::Sending, None);
    let endpoint = pf::endpoint();
    let outcome = match mint() {
        Err(o) => o,
        Ok(t) => match wire::post_submission(&endpoint, &t.token, row, shot.as_ref()) {
            // `401 replayed`: a fresh token, the same row, once.
            Outcome::Retry { reauth: true, .. } => match mint() {
                Ok(t2) => wire::post_submission(&endpoint, &t2.token, row, shot.as_ref()),
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
            let requeued = requeue_after_answer(&conn, now);
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
            emit_requeued(&requeued);
            Applied::Sent
        }
        Outcome::Refused { code, detail } => {
            let reason = format!("{code}: {detail}");
            let _ = store::mark_refused(&conn, &row.id, &reason, now);
            let requeued = requeue_after_answer(&conn, now);
            drop(conn);
            lock().seen = Some(ServiceSeen { state: "ok", code: None, detail: None, at_ms: now });
            emit_row(&row.id, State::Refused, Some(&reason));
            emit_requeued(&requeued);
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

fn emit_requeued(ids: &[String]) {
    for id in ids {
        emit_row(id, State::Queued, None);
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
