//! Brief retries for the two dials a federated `k2 msg` makes (0.46.18):
//! the peer's roster GET and the message POST.
//!
//! A busy receiving server (a DB lock jam, a slow box behind K2 Connect)
//! sometimes drops or stalls a request for over 10 s. One failed roster
//! fetch used to end the send with nothing delivered, so a part of a
//! multi-part send was lost unless the sender noticed and sent it again.
//! Now each dial gets up to [`ATTEMPTS`] tries with a short backoff
//! (about 2 s, then 6 s, ±25% jitter), every try bounded by its own time
//! cap, the whole run by the request's deadline. Only failures that can
//! pass are retried: no answer, 429 and 5xx. A 4xx (a refusal: 401, 403,
//! 404, …) ends the run at once.
//!
//! A POST that may have reached the peer is retried only when the peer
//! says it drops a repeat it is still delivering ([`IN_FLIGHT_DEDUPE`] in
//! its roster). Every retry carries the same `msg_uuid` (resealed, fresh
//! nonce), so the receiver's dedupe (`federation::seen` after a delivery,
//! [`InFlight`] while one runs) keeps it to one delivery.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tries per dial (the first one included).
pub(crate) const ATTEMPTS: u32 = 3;

/// The pause before the 2nd and the 3rd try, before jitter.
pub(crate) const PAUSES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];

/// ± this fraction of jitter on each pause.
pub(crate) const JITTER_FRAC: f64 = 0.25;

/// One try's time cap: a slow peer gets this long to answer.
pub(crate) const PER_TRY: Duration = Duration::from_secs(15);

/// The whole roster lookup's cap. Under the CLI's 30 s request limit, so
/// the CLI always hears the outcome rather than its own timeout.
pub(crate) const ROSTER_BUDGET: Duration = Duration::from_secs(26);

/// The whole send request's cap (the send gate's roster check and the POST
/// tries together), under the CLI's 30 s request limit too.
pub(crate) const SEND_BUDGET: Duration = Duration::from_secs(26);

/// A try is not started with less time than this left.
pub(crate) const MIN_TRY: Duration = Duration::from_secs(1);

/// The roster field a receiver sets when it drops a repeat of a message it
/// is still delivering ([`InFlight`]). Only then does a sender resend a
/// POST that may have reached it.
pub(crate) const IN_FLIGHT_DEDUPE: (&str, &str) = ("inbound_dedupe", "in_flight");

/// What one try came to.
#[derive(Debug)]
pub(crate) enum Step<T> {
    /// Success: stop here.
    Done(T),
    /// A failure that can pass: try again while tries and time are left.
    Retry(String),
    /// A failure that won't pass by trying again (a 4xx): stop now.
    Stop(String),
}

/// How a run of tries failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failed {
    /// Tries made.
    pub attempts: u32,
    /// The last try's error.
    pub last: String,
    /// True when the last failure was one that is not retried ([`Step::Stop`]).
    pub refused: bool,
}

/// The pause before try `n + 2` (`n` = 0 for the 2nd), with jitter `r` in
/// [0, 1]. Pure.
pub(crate) fn pause(n: usize, r: f64) -> Duration {
    let base = PAUSES[n.min(PAUSES.len() - 1)];
    let factor = 1.0 - JITTER_FRAC + 2.0 * JITTER_FRAC * r.clamp(0.0, 1.0);
    base.mul_f64(factor)
}

/// Cheap non-crypto randomness for jitter (subsecond clock noise).
fn jitter_r() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

/// Tests run the same schedule 40 times faster.
#[cfg(not(test))]
fn real_pause(d: Duration) -> Duration {
    d
}
#[cfg(test)]
fn real_pause(d: Duration) -> Duration {
    d / 40
}

/// Run `try_once(attempt, time_cap)` up to [`ATTEMPTS`] times until it is
/// done, refused, or `deadline` leaves no room for another try. `attempt`
/// counts from 1; `time_cap` is [`PER_TRY`] or what is left before the
/// deadline, whichever is less. The first try always runs (with at least
/// [`MIN_TRY`]). Blocking: it sleeps between tries.
pub(crate) fn run<T>(
    what: &str,
    deadline: Instant,
    mut try_once: impl FnMut(u32, Duration) -> Step<T>,
) -> Result<T, Failed> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let left = deadline.saturating_duration_since(Instant::now());
        let cap = PER_TRY.min(left).max(MIN_TRY);
        let last = match try_once(attempt, cap) {
            Step::Done(v) => return Ok(v),
            Step::Stop(e) => return Err(Failed { attempts: attempt, last: e, refused: true }),
            Step::Retry(e) => e,
        };
        if attempt >= ATTEMPTS {
            return Err(Failed { attempts: attempt, last, refused: false });
        }
        let wait = real_pause(pause(attempt as usize - 1, jitter_r()));
        let left = deadline.saturating_duration_since(Instant::now());
        if left < wait + MIN_TRY {
            return Err(Failed { attempts: attempt, last, refused: false });
        }
        k2_core::log_debug!(
            "[federation] {what}: try {attempt} of {ATTEMPTS} failed ({last}); next in {:.1} s",
            wait.as_secs_f64()
        );
        std::thread::sleep(wait);
    }
}

/// "1 attempt" / "3 attempts".
pub(crate) fn attempts_text(n: u32) -> String {
    if n == 1 {
        "1 attempt".to_string()
    } else {
        format!("{n} attempts")
    }
}

/// True for an HTTP status worth another try: 429 and every 5xx.
pub(crate) fn status_can_pass(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

// ─────────────────────────────────────────────────────────────────────
// Which peers drop a repeat while it is in flight (learned from rosters)
// ─────────────────────────────────────────────────────────────────────

fn dedupe_peers() -> &'static Mutex<HashSet<String>> {
    static PEERS: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    PEERS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Note what peer `fp`'s roster body says about in-flight dedupe. A body
/// without the field (an older peer) clears it.
pub(crate) fn note_roster(fp: &str, roster: &serde_json::Value) {
    let says = roster
        .get(IN_FLIGHT_DEDUPE.0)
        .or_else(|| roster.get("roster").and_then(|r| r.get(IN_FLIGHT_DEDUPE.0)))
        .and_then(|v| v.as_str())
        == Some(IN_FLIGHT_DEDUPE.1);
    set_peer_dedupes(fp, says);
}

/// Record whether peer `fp` drops a repeat while it is in flight.
pub(crate) fn set_peer_dedupes(fp: &str, yes: bool) {
    let mut set = dedupe_peers().lock().unwrap_or_else(|p| p.into_inner());
    if yes {
        set.insert(fp.to_string());
    } else {
        set.remove(fp);
    }
}

/// True when peer `fp`'s last roster said it drops an in-flight repeat.
pub(crate) fn peer_dedupes(fp: &str) -> bool {
    dedupe_peers().lock().unwrap_or_else(|p| p.into_inner()).contains(fp)
}

// ─────────────────────────────────────────────────────────────────────
// Receiver side: one delivery of a msg_uuid at a time
// ─────────────────────────────────────────────────────────────────────

fn in_flight() -> &'static Mutex<HashSet<(String, String)>> {
    static SET: std::sync::OnceLock<Mutex<HashSet<(String, String)>>> = std::sync::OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A claim on delivering one `(peer, msg_uuid)`: while it is held, the same
/// message arriving again (a sender's retry after its own time cap ran
/// out) is answered 503 `in_flight` instead of being delivered a second
/// time. Released when dropped; the durable `federation::seen` record
/// covers every repeat after a successful delivery.
#[derive(Debug)]
pub(crate) struct InFlight {
    key: (String, String),
}

impl InFlight {
    /// Claim `(peer_fp, msg_uuid)`; `None` when a delivery of it is running.
    pub(crate) fn claim(peer_fp: &str, msg_uuid: &str) -> Option<InFlight> {
        let key = (peer_fp.to_string(), msg_uuid.to_string());
        let mut set = in_flight().lock().unwrap_or_else(|p| p.into_inner());
        if set.contains(&key) {
            return None;
        }
        set.insert(key.clone());
        Some(InFlight { key })
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn the_pauses_are_2_and_6_seconds_with_a_quarter_jitter() {
        assert_eq!(pause(0, 0.5), Duration::from_secs(2));
        assert_eq!(pause(1, 0.5), Duration::from_secs(6));
        assert_eq!(pause(0, 0.0), Duration::from_millis(1500));
        assert_eq!(pause(0, 1.0), Duration::from_millis(2500));
        assert_eq!(pause(1, 0.0), Duration::from_millis(4500));
        assert_eq!(pause(1, 1.0), Duration::from_millis(7500));
        // The whole run's worst case fits the budget only by the deadline:
        // two full tries and both longest pauses would not.
        assert!(PER_TRY * 2 + pause(0, 1.0) > ROSTER_BUDGET);
        assert!(ROSTER_BUDGET < Duration::from_secs(30), "under the CLI's request limit");
        assert!(SEND_BUDGET < Duration::from_secs(30), "under the CLI's request limit");
    }

    #[test]
    fn a_failure_that_passes_is_retried_until_done() {
        let tries = Cell::new(0);
        let got = run("test", far(), |n, cap| {
            tries.set(n);
            assert!(cap <= PER_TRY && cap >= MIN_TRY, "cap {cap:?}");
            if n < 3 { Step::Retry(format!("drop {n}")) } else { Step::Done("ok") }
        });
        assert_eq!(got, Ok("ok"));
        assert_eq!(tries.get(), 3);
    }

    #[test]
    fn three_tries_at_most_and_the_last_error_is_kept() {
        let tries = Cell::new(0);
        let got: Result<(), Failed> = run("test", far(), |n, _| {
            tries.set(n);
            Step::Retry(format!("drop {n}"))
        });
        assert_eq!(got, Err(Failed { attempts: 3, last: "drop 3".into(), refused: false }));
        assert_eq!(tries.get(), 3);
    }

    #[test]
    fn a_refusal_is_never_retried() {
        let tries = Cell::new(0);
        let got: Result<(), Failed> = run("test", far(), |n, _| {
            tries.set(n);
            Step::Stop("HTTP 403".into())
        });
        assert_eq!(got, Err(Failed { attempts: 1, last: "HTTP 403".into(), refused: true }));
        assert_eq!(tries.get(), 1);
    }

    #[test]
    fn no_try_starts_past_the_deadline() {
        // A deadline already gone: the first try still runs (with the
        // minimum cap), and no second one starts.
        let tries = Cell::new(0);
        let got: Result<(), Failed> = run("test", Instant::now(), |n, cap| {
            tries.set(n);
            assert_eq!(cap, MIN_TRY);
            Step::Retry("slow".into())
        });
        assert_eq!(got, Err(Failed { attempts: 1, last: "slow".into(), refused: false }));
        assert_eq!(tries.get(), 1);
    }

    #[test]
    fn which_statuses_can_pass() {
        for s in [429, 500, 502, 503, 504, 599] {
            assert!(status_can_pass(s), "{s}");
        }
        for s in [200, 400, 401, 403, 404, 409, 413, 422] {
            assert!(!status_can_pass(s), "{s}");
        }
    }

    #[test]
    fn an_in_flight_claim_blocks_a_second_until_dropped() {
        let uuid = uuid::Uuid::new_v4().to_string();
        let first = InFlight::claim("fp-a", &uuid).expect("first claim");
        assert!(InFlight::claim("fp-a", &uuid).is_none(), "a repeat while in flight");
        assert!(InFlight::claim("fp-b", &uuid).is_some(), "another peer's uuid is its own");
        drop(first);
        assert!(InFlight::claim("fp-a", &uuid).is_some(), "free again once delivered");
    }

    #[test]
    fn a_roster_says_whether_the_peer_drops_in_flight_repeats() {
        let fp = format!("fp-{}", uuid::Uuid::new_v4().simple());
        assert!(!peer_dedupes(&fp));
        note_roster(&fp, &serde_json::json!({ "agents": [], "inbound_dedupe": "in_flight" }));
        assert!(peer_dedupes(&fp));
        // The peer-roster wrapper reads the same.
        note_roster(&fp, &serde_json::json!({ "roster": { "agents": [] } }));
        assert!(!peer_dedupes(&fp), "an older peer's roster clears it");
        note_roster(&fp, &serde_json::json!({ "roster": { "inbound_dedupe": "in_flight" } }));
        assert!(peer_dedupes(&fp));
    }
}
