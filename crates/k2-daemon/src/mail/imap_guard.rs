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
//!
//! **Stuck places (0.46.18).** Field failure: reads that stalled on the
//! provider never returned, so their places were never freed and the inbox
//! answered `inbox_busy` until the server restarted. The IMAP connection
//! now times out ([`crate::mail::imap_tls::IO_TIMEOUT`]), which frees the
//! place. As a second line, every place records when it was taken and a
//! handle on its session's socket ([`Permit::attach`]); a place older than
//! [`MAX_PLACE_AGE`] is taken back by the next [`acquire`]: its socket is
//! shut down first (which also wakes the thread blocked on it), so the
//! count of usable connections never passes [`MAX_SESSIONS_PER_INBOX`]. A
//! session whose place was taken back can't attach a socket any more; its
//! connect fails instead. Sessions under a place are request-scoped: none
//! may IDLE (the mail watcher keeps its one session outside the cap).

use std::collections::HashMap;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;

/// Sessions one linked inbox may have open at once.
pub const MAX_SESSIONS_PER_INBOX: usize = 4;
/// A place held longer than this is stuck: the next [`acquire`] for the
/// inbox shuts its socket and takes it back. Far past any one request
/// (each read or write is bounded by the IMAP timeout; a very large
/// message on a slow link is the longest honest case).
pub const MAX_PLACE_AGE: Duration = Duration::from_secs(600);
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

/// What one place knows about its session's socket.
enum PlaceSocket {
    /// Not connected yet.
    Pending,
    /// A second handle on the session's socket.
    Open(TcpStream),
    /// Taken back as stuck: the socket (if any) is shut down and no new
    /// one may attach.
    Revoked,
}

/// One taken place.
struct Place {
    since: Instant,
    socket: Arc<Mutex<PlaceSocket>>,
}

impl Place {
    fn revoke(&self) {
        let mut sock = self.socket.lock().unwrap_or_else(|p| p.into_inner());
        if let PlaceSocket::Open(s) = &*sock {
            let _ = s.shutdown(Shutdown::Both);
        }
        *sock = PlaceSocket::Revoked;
    }
}

#[derive(Default)]
struct InboxState {
    /// The places taken, by permit id.
    places: HashMap<u64, Place>,
    /// A stuck place was taken back here already (logged loudly once).
    stale_logged: bool,
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
    let stuck_min = MAX_PLACE_AGE.as_secs() / 60;
    format!(
        "{TAG_BUSY}{retry_after_s}] this linked inbox already has {MAX_SESSIONS_PER_INBOX} mail \
         sessions open — no more are opened, so the provider won't lock the account. Run your \
         mail calls one at a time (or a few), and retry this one in {retry_after_s}s. A stalled \
         session is closed on its own (within {stuck_min} min); if this still says busy after \
         that with nothing reading, restarting the K2 server clears stuck sessions"
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

/// One open session's place under the cap. Dropping it frees the place
/// (unless it was already taken back as stuck).
pub struct Permit {
    inbox_id: String,
    id: u64,
    socket: Arc<Mutex<PlaceSocket>>,
}

impl Permit {
    /// Hand the guard a second handle on this session's socket, so a stuck
    /// place can be shut from outside. Fails (and closes `socket`) when
    /// the place was already taken back: the caller must not use the
    /// connection.
    pub fn attach(&self, socket: TcpStream) -> Result<(), String> {
        let mut slot = self.socket.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(*slot, PlaceSocket::Revoked) {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(
                "this mail session waited too long and its place was given up; try again".to_string()
            );
        }
        *slot = PlaceSocket::Open(socket);
        Ok(())
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let g = guard();
        let mut st = g.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = st.get_mut(&self.inbox_id) {
            s.places.remove(&self.id);
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
    acquire_aged(inbox_id, wait, MAX_PLACE_AGE)
}

/// [`acquire_within`] with the stuck age given (tests).
pub(crate) fn acquire_aged(inbox_id: &str, wait: Duration, max_age: Duration) -> Result<Permit, String> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
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
        take_back_stuck(inbox_id, s, now, max_age);
        if s.places.len() < MAX_SESSIONS_PER_INBOX {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let socket = Arc::new(Mutex::new(PlaceSocket::Pending));
            s.places.insert(id, Place { since: now, socket: socket.clone() });
            return Ok(Permit { inbox_id: inbox_id.to_string(), id, socket });
        }
        if now >= deadline {
            return Err(busy_error(5));
        }
        // Wake in time to take back a place that turns stuck meanwhile.
        let next_stuck = s
            .places
            .values()
            .map(|p| (p.since + max_age).saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::MAX);
        let nap = (deadline - now).min(next_stuck.max(Duration::from_millis(1)));
        let (next, _) = g.freed.wait_timeout(st, nap).unwrap_or_else(|p| p.into_inner());
        st = next;
    }
}

/// Take back every place older than `max_age`: shut its socket (waking the
/// thread stuck on it) and forget it, so a new session may use the place.
fn take_back_stuck(inbox_id: &str, s: &mut InboxState, now: Instant, max_age: Duration) {
    let stuck: Vec<u64> = s
        .places
        .iter()
        .filter(|(_, p)| now.saturating_duration_since(p.since) >= max_age)
        .map(|(id, _)| *id)
        .collect();
    if stuck.is_empty() {
        return;
    }
    for id in &stuck {
        if let Some(place) = s.places.remove(id) {
            place.revoke();
        }
    }
    let first = !s.stale_logged;
    s.stale_logged = true;
    if first {
        k2_core::log_debug!(
            "[mail/imap-guard] WARN {inbox_id}: took back {} mail session place(s) held over {}s \
             (stalled connection; the socket was shut down)",
            stuck.len(),
            max_age.as_secs()
        );
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
    fn a_place_held_past_the_max_age_is_taken_back_and_its_socket_shut() {
        use std::io::Read;
        let id = inbox();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        // The cap's worth of stuck sessions, each blocked reading a socket
        // nobody writes to.
        let mut held = Vec::new();
        let mut readers = Vec::new();
        for _ in 0..MAX_SESSIONS_PER_INBOX {
            let permit = acquire_within(&id, Duration::ZERO).expect("place");
            let sock = TcpStream::connect(addr).expect("connect");
            let mut reader = sock.try_clone().expect("clone");
            reader.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
            permit.attach(sock).expect("attach");
            readers.push(std::thread::spawn(move || {
                let t0 = Instant::now();
                let mut b = [0u8; 1];
                let r = reader.read(&mut b).map_err(|e| e.kind());
                (r, t0.elapsed())
            }));
            held.push(permit);
        }
        // Young places are left alone.
        let busy = acquire_aged(&id, Duration::ZERO, MAX_PLACE_AGE).err().expect("busy");
        assert_eq!(parse_tag(&busy).map(|t| t.0), Some("inbox_busy"), "{busy}");
        assert!(busy.contains("restarting the K2 server clears stuck sessions"), "{busy}");
        std::thread::sleep(Duration::from_millis(100));
        let fresh = acquire_aged(&id, Duration::ZERO, Duration::from_millis(50)).expect("stuck places are taken back");
        // Every stuck socket was shut down, which wakes its reader at once.
        for r in readers {
            let (res, took) = r.join().expect("reader");
            assert!(took < Duration::from_secs(5), "the stuck read woke: {took:?} {res:?}");
            assert!(
                !matches!(res, Err(std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
                "woken by the shutdown, not its own timeout: {res:?}"
            );
        }
        // A taken-back session can't attach a new connection.
        let late = TcpStream::connect(addr).expect("connect");
        assert!(held[0].attach(late).is_err(), "a taken-back place refuses a new socket");
        // Dropping the taken-back permits frees nothing twice: the cap holds.
        drop(held);
        let more: Vec<Permit> =
            (1..MAX_SESSIONS_PER_INBOX).map(|_| acquire_within(&id, Duration::ZERO).expect("place")).collect();
        assert!(acquire_within(&id, Duration::ZERO).is_err(), "never more than the cap");
        drop((more, fresh, listener));
        reset_for_test(&id);
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
