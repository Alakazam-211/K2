//! How hard K2 may lean on one linked IMAP inbox (0.46.17).
//!
//! Field failure: an agent's script ran 24 parallel `k2 mail read` calls
//! with retries against one Gmail inbox (about 4,400 reads, 3,102 failed),
//! and Gmail then refused that account's IMAP logins (XOAUTH2 status 400)
//! even with a fresh token. Two rules now stand between agents and the
//! provider, keyed by the linked inbox's row id:
//!
//! * **At most [`MAX_SESSIONS_PER_INBOX`] sessions at once** (Gmail allows
//!   about 15 per account, shared with the person's own clients). A call
//!   past that waits up to [`QUEUE_WAIT`] for a session to end, then
//!   answers 429 `inbox_busy` with `retry_after_s`. No extra connection is
//!   ever opened.
//! * **A cool-down when the provider pushes back.** A throttle answer
//!   ("Too many simultaneous connections", "exceeded command or bandwidth
//!   limits"), or [`AUTH_FAILS_FOR_COOLDOWN`] refused XOAUTH2 logins within
//!   [`AUTH_FAIL_WINDOW`], stops new logins to that inbox for 5 minutes,
//!   doubling each time up to an hour. Meanwhile every call answers 503
//!   `inbox_throttled` with `retry_after_s`; nothing reaches the provider.
//!   A re-link changes the token, not the account, so it doesn't end a
//!   cool-down (the re-link route refuses while one runs).
//!
//! A refusal travels as a tagged error string (the IMAP seam speaks
//! `String`); [`rewrite_response`] turns the tag into the 429 / 503 answer
//! at the mail route boundary, whatever route it came through.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;

/// Sessions one linked inbox may have open at once.
pub const MAX_SESSIONS_PER_INBOX: usize = 4;
/// How long a call past the cap waits for a session to end.
pub const QUEUE_WAIT: Duration = Duration::from_secs(10);
/// The first cool-down; each one after doubles, up to [`COOLDOWN_MAX`].
pub const COOLDOWN_START: Duration = Duration::from_secs(300);
pub const COOLDOWN_MAX: Duration = Duration::from_secs(3600);
/// Refused XOAUTH2 logins within [`AUTH_FAIL_WINDOW`] that start one.
pub const AUTH_FAILS_FOR_COOLDOWN: usize = 3;
pub const AUTH_FAIL_WINDOW: Duration = Duration::from_secs(120);
/// A quiet stretch after a cool-down that resets the doubling.
const LEVEL_RESET_AFTER: Duration = Duration::from_secs(6 * 3600);

const TAG_BUSY: &str = "[k2:inbox_busy:";
const TAG_THROTTLED: &str = "[k2:inbox_throttled:";

#[derive(Default)]
struct InboxState {
    active: usize,
    cool_until: Option<Instant>,
    /// Cool-downs so far in this stretch (0 = the next is COOLDOWN_START).
    level: u32,
    last_cooldown: Option<Instant>,
    auth_fails: Vec<Instant>,
}

struct Guard {
    state: Mutex<HashMap<String, InboxState>>,
    freed: Condvar,
}

fn guard() -> &'static Guard {
    static G: OnceLock<Guard> = OnceLock::new();
    G.get_or_init(|| Guard { state: Mutex::new(HashMap::new()), freed: Condvar::new() })
}

fn secs_ceil(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

/// The tagged error for a call past the session cap.
pub fn busy_error(retry_after_s: u64) -> String {
    format!(
        "{TAG_BUSY}{retry_after_s}] this linked inbox already has {MAX_SESSIONS_PER_INBOX} mail \
         sessions open — no more are opened, so the provider won't lock the account. Run your \
         mail calls one at a time (or a few), and retry this one in {retry_after_s}s"
    )
}

/// The tagged error for a call during a cool-down.
pub fn throttled_error(retry_after_s: u64) -> String {
    let mins = retry_after_s.div_ceil(60);
    format!(
        "{TAG_THROTTLED}{retry_after_s}] the mail provider is limiting this account (too many \
         connections or failed logins), so it is paused for about {mins} min. Wait, then try \
         again; don't link it again, a new sign-in won't lift the limit. Run fewer mail calls at once"
    )
}

/// One open session's place under the cap. Dropping it frees the place.
pub struct Permit {
    inbox_id: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let g = guard();
        let mut st = g.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = st.get_mut(&self.inbox_id) {
            s.active = s.active.saturating_sub(1);
        }
        drop(st);
        g.freed.notify_all();
    }
}

/// A session that holds its [`Permit`] for as long as it lives.
pub struct Guarded<S> {
    inner: S,
    _permit: Permit,
}

impl<S> Guarded<S> {
    pub fn new(inner: S, permit: Permit) -> Self {
        Self { inner, _permit: permit }
    }
}

impl<S> std::ops::Deref for Guarded<S> {
    type Target = S;
    fn deref(&self) -> &S {
        &self.inner
    }
}

impl<S> std::ops::DerefMut for Guarded<S> {
    fn deref_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

/// Seconds left in `inbox_id`'s cool-down, if one is running.
pub fn cooldown_left(inbox_id: &str) -> Option<u64> {
    let st = guard().state.lock().unwrap_or_else(|p| p.into_inner());
    let until = st.get(inbox_id)?.cool_until?;
    let now = Instant::now();
    (until > now).then(|| secs_ceil(until - now).max(1))
}

/// The tagged throttled error when a cool-down is running (callers that
/// open a session without a permit, like the mail watcher, check this).
pub fn check_cooldown(inbox_id: &str) -> Result<(), String> {
    match cooldown_left(inbox_id) {
        Some(left) => Err(throttled_error(left)),
        None => Ok(()),
    }
}

/// Take a session place, waiting up to [`QUEUE_WAIT`].
pub fn acquire(inbox_id: &str) -> Result<Permit, String> {
    acquire_within(inbox_id, QUEUE_WAIT)
}

/// [`acquire`] with the wait given (tests).
pub fn acquire_within(inbox_id: &str, wait: Duration) -> Result<Permit, String> {
    let g = guard();
    let deadline = Instant::now() + wait;
    let mut st = g.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        let now = Instant::now();
        let s = st.entry(inbox_id.to_string()).or_default();
        if let Some(until) = s.cool_until {
            if until > now {
                return Err(throttled_error(secs_ceil(until - now).max(1)));
            }
            s.cool_until = None;
        }
        if s.active < MAX_SESSIONS_PER_INBOX {
            s.active += 1;
            return Ok(Permit { inbox_id: inbox_id.to_string() });
        }
        if now >= deadline {
            return Err(busy_error(5));
        }
        let (next, _) = g.freed.wait_timeout(st, deadline - now).unwrap_or_else(|p| p.into_inner());
        st = next;
    }
}

/// Is `msg` the provider saying "too much"?
pub fn is_throttle(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    [
        "too many simultaneous connections",
        "exceeded command or bandwidth limits",
        "too many connections",
        "[throttled]",
        "temporarily unavailable due to too many",
    ]
    .iter()
    .any(|n| m.contains(n))
}

/// Is `msg` Gmail refusing an XOAUTH2 login?
pub fn is_xoauth2_refusal(msg: &str) -> bool {
    msg.contains("Gmail rejected the OAuth access token")
}

fn start_cooldown(s: &mut InboxState, now: Instant) {
    if s.last_cooldown.is_some_and(|t| now.duration_since(t) > LEVEL_RESET_AFTER) {
        s.level = 0;
    }
    let d = COOLDOWN_START.saturating_mul(1u32 << s.level.min(8)).min(COOLDOWN_MAX);
    s.cool_until = Some(now + d);
    s.last_cooldown = Some(now);
    s.level = s.level.saturating_add(1);
    s.auth_fails.clear();
}

/// Record a failed login or session: a throttle answer starts a cool-down
/// at once; refused XOAUTH2 logins start one after
/// [`AUTH_FAILS_FOR_COOLDOWN`] within [`AUTH_FAIL_WINDOW`]. Answers
/// whether a cool-down started.
pub fn note_failure(inbox_id: &str, msg: &str) -> bool {
    note_failure_at(inbox_id, msg, Instant::now())
}

fn note_failure_at(inbox_id: &str, msg: &str, now: Instant) -> bool {
    let throttle = is_throttle(msg);
    let refused = is_xoauth2_refusal(msg);
    if !throttle && !refused {
        return false;
    }
    let mut st = guard().state.lock().unwrap_or_else(|p| p.into_inner());
    let s = st.entry(inbox_id.to_string()).or_default();
    if throttle {
        start_cooldown(s, now);
        k2_core::log_debug!("[mail/imap-guard] {inbox_id}: provider throttle, pausing logins");
        return true;
    }
    s.auth_fails.retain(|t| now.duration_since(*t) <= AUTH_FAIL_WINDOW);
    s.auth_fails.push(now);
    if s.auth_fails.len() >= AUTH_FAILS_FOR_COOLDOWN {
        start_cooldown(s, now);
        k2_core::log_debug!("[mail/imap-guard] {inbox_id}: repeated XOAUTH2 refusals, pausing logins");
        return true;
    }
    false
}

/// A login worked: refused-login counting starts over.
pub fn note_success(inbox_id: &str) {
    let mut st = guard().state.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = st.get_mut(inbox_id) {
        s.auth_fails.clear();
    }
}

/// The tag in `text`, as `(code, retry_after_s, clean hint)`.
pub fn parse_tag(text: &str) -> Option<(&'static str, u64, String)> {
    for (tag, code) in [(TAG_BUSY, "inbox_busy"), (TAG_THROTTLED, "inbox_throttled")] {
        if let Some(at) = text.find(tag) {
            let rest = &text[at + tag.len()..];
            let close = rest.find(']')?;
            let secs = rest[..close].parse::<u64>().ok()?;
            return Some((code, secs, rest[close + 1..].trim_start().to_string()));
        }
    }
    None
}

/// Turn a mail route's error that carries a guard tag into the 429
/// `inbox_busy` / 503 `inbox_throttled` answer with `retry_after_s`.
/// Anything else passes through unchanged.
pub fn rewrite_response(resp: CliResponse) -> CliResponse {
    if resp.status.starts_with('2') || !resp.body.contains("[k2:inbox_") {
        return resp;
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&resp.body) else {
        return resp;
    };
    let Some(hint) = v["error"]["hint"].as_str().map(str::to_string) else {
        return resp;
    };
    let Some((code, secs, clean)) = parse_tag(&hint) else {
        return resp;
    };
    v["error"]["code"] = serde_json::json!(code);
    v["error"]["hint"] = serde_json::json!(clean);
    v["error"]["retry_after_s"] = serde_json::json!(secs);
    v["retry_after_s"] = serde_json::json!(secs);
    CliResponse {
        status: if code == "inbox_busy" { "429 Too Many Requests" } else { "503 Service Unavailable" },
        content_type: "application/json",
        body: v.to_string(),
    }
}

#[cfg(test)]
pub(crate) fn reset_for_test(inbox_id: &str) {
    guard().state.lock().unwrap_or_else(|p| p.into_inner()).remove(inbox_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbox() -> String {
        format!("guard-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn the_fifth_session_waits_then_is_busy() {
        let id = inbox();
        let held: Vec<Permit> = (0..MAX_SESSIONS_PER_INBOX).map(|_| acquire_within(&id, Duration::ZERO).expect("place")).collect();
        let err = acquire_within(&id, Duration::from_millis(50)).err().expect("busy");
        assert_eq!(parse_tag(&err).map(|t| t.0), Some("inbox_busy"), "{err}");
        drop(held);
        let _again = acquire_within(&id, Duration::ZERO).expect("free again");
    }

    #[test]
    fn a_waiting_call_gets_the_place_a_session_frees() {
        let id = inbox();
        let mut held: Vec<Permit> = (0..MAX_SESSIONS_PER_INBOX).map(|_| acquire_within(&id, Duration::ZERO).expect("place")).collect();
        let id2 = id.clone();
        let waiter = std::thread::spawn(move || acquire_within(&id2, Duration::from_secs(5)).map(|_| ()));
        std::thread::sleep(Duration::from_millis(100));
        held.pop();
        waiter.join().expect("join").expect("got the freed place");
    }

    #[test]
    fn a_throttle_answer_pauses_the_inbox_and_doubles() {
        let id = inbox();
        let t0 = Instant::now();
        assert!(note_failure_at(&id, "login: No Response: [ALERT] Too many simultaneous connections. (Failure)", t0));
        let err = acquire_within(&id, Duration::ZERO).err().expect("throttled");
        let (code, secs, hint) = parse_tag(&err).expect("tag");
        assert_eq!(code, "inbox_throttled");
        assert!(secs > 290 && secs <= 300, "{secs}");
        assert!(hint.contains("don't link it again"), "{hint}");
        // The next one is ten minutes.
        assert!(note_failure_at(&id, "Account exceeded command or bandwidth limits", Instant::now()));
        let left = cooldown_left(&id).expect("running");
        assert!(left > 590 && left <= 600, "{left}");
        reset_for_test(&id);
    }

    #[test]
    fn three_refused_xoauth2_logins_in_two_minutes_pause_the_inbox() {
        let id = inbox();
        let refused = "Gmail rejected the OAuth access token (XOAUTH2 status 400) — the inbox must be re-linked";
        let t0 = Instant::now();
        assert!(!note_failure_at(&id, refused, t0));
        assert!(!note_failure_at(&id, refused, t0 + Duration::from_secs(10)));
        // An unrelated failure doesn't count.
        assert!(!note_failure_at(&id, "connect: connection refused", t0 + Duration::from_secs(11)));
        assert!(note_failure_at(&id, refused, t0 + Duration::from_secs(20)));
        assert!(cooldown_left(&id).is_some());
        reset_for_test(&id);
        // Spread out, they don't.
        let id = inbox();
        for i in 0..5 {
            assert!(!note_failure_at(&id, refused, t0 + Duration::from_secs(200 * i)));
        }
        assert!(cooldown_left(&id).is_none());
    }

    #[test]
    fn tagged_errors_become_429_and_503_with_retry_after() {
        let body = |hint: &str| serde_json::json!({ "ok": false, "error": { "code": "engine", "hint": hint } }).to_string();
        let r = rewrite_response(CliResponse {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: body(&format!("draft APPEND failed: {}", busy_error(5))),
        });
        assert_eq!(r.status, "429 Too Many Requests");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "inbox_busy");
        assert_eq!(v["error"]["retry_after_s"], 5);
        assert!(!v["error"]["hint"].as_str().expect("hint").contains("[k2:"), "{v}");
        let r = rewrite_response(CliResponse { status: "502 Bad Gateway", content_type: "application/json", body: body(&throttled_error(240)) });
        assert_eq!(r.status, "503 Service Unavailable");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "inbox_throttled");
        assert_eq!(v["retry_after_s"], 240);
        let plain = CliResponse { status: "502 Bad Gateway", content_type: "application/json", body: body("imap down") };
        assert_eq!(rewrite_response(plain).status, "502 Bad Gateway");
    }
}
