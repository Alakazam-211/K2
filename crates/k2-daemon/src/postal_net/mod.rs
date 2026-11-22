//! Postal's network half in the daemon (prd-postal-in-k2-v1 S3 network,
//! S4, S5): one background loop that keeps this box's Postal handles,
//! pairings, held mail and outbox in step with the plane, behind the
//! server's Postal switch (Beta, off by default).
//!
//! - [`plane`]: HTTPS to K2 Web's `/api/postal/v1`, the box's `pst_` session
//!   from its K2 Connect credential, the owner's `k2c_pd_` decide token.
//! - [`pairing`]: SAS v2 over the plane, K2's handshake object, cards on
//!   both boxes, the owner-only confirm.
//! - [`receive`]: hold pull, open and verify, store before ACK, land.
//! - [`send`]: the outbox, hold only.
//! - [`decide_token`]: the owner's decide token, minted on the owner's
//!   switch-on (or once at start, or before an invite, a claim or an
//!   accept that would otherwise be stuck) and revoked on switch-off.
//! - [`live`]: the live listener's check, not mounted (S6 waits on the
//!   plane's P-6).
//!
//! The loop polls `/events` (25 s long polls) and does a full pass at
//! least every 60 s; when the plane can't be reached it backs off to 15
//! minutes, and every answer that would go out says the network isn't
//! connected ([`k2_core::postal::gate::network_available`]). A person's
//! `postal claim`, `enable` or `status` while it isn't connected wakes the
//! loop for a pass now and resets the backoff ([`wake`]), at most once per
//! [`WAKE_EVERY`].

pub mod decide_token;
pub mod live;
pub mod pairing;
pub mod plane;
pub mod receive;
pub mod send;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use k2_core::postal::sdk::plane::{self as sdk_plane, AfterEvents, Event};

use plane::{CallError, Plane};

static NET: Mutex<Option<Arc<Plane>>> = Mutex::new(None);
static LAST: Mutex<Option<Problem>> = Mutex::new(None);
/// When the loop tries again after a failed pass (unix seconds).
static NEXT: Mutex<Option<i64>> = Mutex::new(None);
/// A wake the loop hasn't taken yet, and the bell it waits on.
static WAKE: Mutex<bool> = Mutex::new(false);
static BELL: Condvar = Condvar::new();
/// When a person last woke the loop.
static LAST_WAKE: Mutex<Option<Instant>> = Mutex::new(None);

/// At most one woken pass per this long, however often people ask.
pub const WAKE_EVERY: Duration = Duration::from_secs(30);

/// The last network problem: its code, its hint, and when the pass that
/// met it ran (unix seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: String,
    pub hint: String,
    pub at: i64,
}

/// This box's plane access, made on first use.
pub fn net() -> Option<Arc<Plane>> {
    let mut g = NET.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_none() {
        match Plane::daemon() {
            Ok(p) => *g = Some(Arc::new(p)),
            Err(e) => k2_core::log_debug!("[postal] {e}"),
        }
    }
    g.clone()
}

/// Tests: talk to another plane.
pub fn install(p: Arc<Plane>) {
    *NET.lock().unwrap_or_else(|p| p.into_inner()) = Some(p);
}

/// Tests: forget the plane, so the next [`net`] makes the daemon's.
#[cfg(test)]
pub(crate) fn forget_for_test() {
    *NET.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Tests: is there a plane (installed, or made by [`net`])?
#[cfg(test)]
pub(crate) fn made_for_test() -> bool {
    NET.lock().unwrap_or_else(|p| p.into_inner()).is_some()
}

/// Run `f` where reqwest's blocking client may run, and wait for it. The
/// plane's HTTP client is that one ([`plane::Https`]), and a plane call
/// can be reached from inside the tokio runtime: a card's Allow or Deny,
/// a route. The client must not run there (a debug build panics with
/// "Cannot drop a runtime in a context where blocking is not allowed").
/// Inside the runtime `f` runs on a plain thread; anywhere else, such as
/// the Postal loop's own thread, it runs here.
pub(crate) fn off_async<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    if tokio::runtime::Handle::try_current().is_err() {
        return f();
    }
    std::thread::scope(|s| s.spawn(f).join()).unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// The last network problem, for `k2 postal status`.
pub fn last_problem() -> Option<Problem> {
    LAST.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// When the loop tries again on its own after a failed pass, if it is
/// waiting to (unix seconds).
pub fn next_try() -> Option<i64> {
    *NEXT.lock().unwrap_or_else(|p| p.into_inner())
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn note(r: &Result<(), CallError>) {
    let mut g = LAST.lock().unwrap_or_else(|p| p.into_inner());
    match r {
        Ok(()) => {
            *g = None;
            k2_core::postal::gate::set_network_available(true);
        }
        Err(e) => {
            let (code, hint) = (e.code(), e.hint());
            if g.as_ref().map(|p| (&p.code, &p.hint)) != Some((&code, &hint)) {
                k2_core::log_debug!("[postal] network: {code}");
            }
            *g = Some(Problem { code, hint, at: unix_now() });
            k2_core::postal::gate::set_network_available(false);
        }
    }
}

/// May a person's wake at `now` start a pass, the last one having been at
/// `last`? At most one per [`WAKE_EVERY`].
fn wake_allowed(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|l| now.saturating_duration_since(l) >= WAKE_EVERY)
}

/// A person's `postal claim`, `enable` or `status` (the routes check it
/// is a person, not an agent's passport): while Postal is on and the
/// network isn't connected, the loop stops waiting out its backoff, makes
/// a pass now and starts its backoff again from a minute. `true` when this
/// call started that pass; `false` when the network is connected, Postal
/// is off, or a person woke it less than [`WAKE_EVERY`] ago (so a script
/// asking in a loop can't hammer the plane).
pub fn wake() -> bool {
    if !k2_core::postal::enabled() || k2_core::postal::gate::network_available() {
        return false;
    }
    let now = Instant::now();
    {
        let mut last = LAST_WAKE.lock().unwrap_or_else(|p| p.into_inner());
        if !wake_allowed(*last, now) {
            return false;
        }
        *last = Some(now);
    }
    ring();
    true
}

/// Wake the loop (no person's limit: [`wake`] holds that).
fn ring() {
    *WAKE.lock().unwrap_or_else(|p| p.into_inner()) = true;
    BELL.notify_all();
}

/// Wait up to `d`; `true` when woken before it ran out.
fn pause(d: Duration) -> bool {
    let until = Instant::now() + d;
    let mut woken = WAKE.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if *woken {
            *woken = false;
            return true;
        }
        let now = Instant::now();
        if now >= until {
            return false;
        }
        woken = match BELL.wait_timeout(woken, until - now) {
            Ok((g, _)) => g,
            Err(p) => p.into_inner().0,
        };
    }
}

/// One full pass: handles, requests, pairs, held mail, landings, outbox.
pub fn tick(net: &Plane) -> Result<(), CallError> {
    pairing::register_handles(net)?;
    for a in k2_core::postal::gate::take_answers() {
        if let k2_core::postal::gate::Answer::Revoke { friend_id, .. } = a {
            if let Some(f) = k2_core::db::read(|c| k2_core::postal::store::friend(c, &friend_id)) {
                if let Err(e) = pairing::revoke(net, &f) {
                    if e.is_retryable() {
                        k2_core::postal::gate::record_answer(k2_core::postal::gate::Answer::Revoke {
                            friend_id: f.id.clone(),
                            pair_id: f.pair_id.clone(),
                            peer: f.peer_address.clone(),
                            block: f.status == "blocked",
                        });
                    }
                    k2_core::log_debug!("[postal] revoke {}: {}", f.id, e.code());
                }
            }
        }
    }
    pairing::send_unsent(net)?;
    pairing::sync(net)?;
    receive::pull(net)?;
    receive::land_waiting();
    send::advance_all(net)?;
    Ok(())
}

/// One events long poll and what it points at. `cursor` is kept across.
fn events(net: &Plane, cursor: &mut Option<String>, wait: u32) -> Result<(), CallError> {
    let (s, b) = net.box_call(&sdk_plane::events(cursor.as_deref(), wait))?;
    match sdk_plane::after_events(s, &b) {
        AfterEvents::Next(page) => {
            let first = cursor.is_none();
            *cursor = Some(page.cursor);
            let (mut hold, mut pairs) = (first, first);
            for e in page.events {
                match e {
                    Event::Hold { .. } => hold = true,
                    Event::Pair { .. } => pairs = true,
                    Event::Receipt { client_id, .. } => send::on_receipt_event(net, &client_id),
                    Event::Unknown => {}
                }
            }
            if pairs {
                pairing::sync(net)?;
            }
            if hold {
                receive::pull(net)?;
            }
            Ok(())
        }
        AfterEvents::Restart => {
            *cursor = None;
            tick(net)
        }
        AfterEvents::BackOff(e) | AfterEvents::Stop(e) => Err(CallError::Plane(e)),
    }
}

/// The events long poll's wait (whole seconds, at most the plane's 25).
fn events_wait() -> u32 {
    std::env::var("K2_POSTAL_EVENTS_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(25).min(25)
}

/// Start the loop (once, at boot). It idles while the switch is off.
pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("postal-net".into()).spawn(run);
    });
}

fn run() {
    static NEVER: AtomicBool = AtomicBool::new(false);
    run_until(&NEVER);
}

/// The loop, until `stop` (tests stop theirs; the daemon's never stops).
fn run_until(stop: &AtomicBool) {
    let mut l = Loop::new();
    while !stop.load(Ordering::SeqCst) {
        let d = l.turn();
        if pause(d) {
            l.woken();
        }
    }
}

/// The loop's state between turns.
struct Loop {
    cursor: Option<String>,
    backoff: Duration,
    /// When the last full pass ran (`None`: one is due).
    last_full: Option<Instant>,
}

/// The first wait after a failed pass, and where the backoff starts again.
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

impl Loop {
    fn new() -> Self {
        Self { cursor: None, backoff: FIRST_BACKOFF, last_full: None }
    }

    /// One turn (a pass, or the idle check) and how long to wait after it.
    fn turn(&mut self) -> Duration {
        if !k2_core::postal::enabled() {
            k2_core::postal::gate::set_network_available(false);
            self.cursor = None;
            return Duration::from_secs(2);
        }
        let Some(net) = net() else {
            return Duration::from_secs(60);
        };
        *NEXT.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let full_due = self.last_full.is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
        let r = if full_due || self.cursor.is_none() {
            self.last_full = Some(Instant::now());
            tick(&net).and_then(|()| events(&net, &mut self.cursor, 0))
        } else {
            events(&net, &mut self.cursor, events_wait())
        };
        // The next try is set before the problem, so a status that sees
        // the problem also sees when the loop tries again.
        let pause = if r.is_err() {
            let d = self.backoff;
            self.backoff = (d * 2).min(MAX_BACKOFF);
            self.last_full = None;
            *NEXT.lock().unwrap_or_else(|p| p.into_inner()) = Some(unix_now() + d.as_secs() as i64);
            d
        } else {
            self.backoff = FIRST_BACKOFF;
            if events_wait() == 0 {
                Duration::from_millis(500)
            } else {
                Duration::ZERO
            }
        };
        let good = r.is_ok();
        note(&r);
        // A server that had Postal on before it started never sees an
        // off-to-on switch: its approval token is asked for once, here,
        // on the first good pass (no token here only; never a storm).
        if good {
            decide_token::on_start(&net);
        }
        pause
    }

    /// A person woke the loop: a full pass now, the backoff from the start.
    fn woken(&mut self) {
        self.backoff = FIRST_BACKOFF;
        self.last_full = None;
    }
}

/// Run one full pass now (a route after a person's answer; tests).
pub fn kick() -> Result<(), CallError> {
    let net = net().ok_or_else(|| CallError::Transport("no Postal plane client".into()))?;
    let r = tick(&net);
    note(&r);
    r
}

/// The owner's postal-decide link: start the device grant for this box's
/// label and poll it in the background until the owner approves (the token
/// goes to the vault) or it ends.
pub fn owner_link_start() -> Result<serde_json::Value, CallError> {
    let net = net().ok_or_else(|| CallError::Transport("no Postal plane client".into()))?;
    let label = net.label()?;
    let (device, user, url, interval) = net.owner_link_start(&label)?;
    let poll_label = label.clone();
    let _ = std::thread::Builder::new().name("postal-owner-link".into()).spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(interval));
            match net.owner_link_poll(&device) {
                Ok(Some(token)) => {
                    match k2_core::postal::keys::set_decide_token(&poll_label, &token) {
                        Ok(()) => crate::hire_routes::audit("postal-owner-link", "owner", &format!("label={poll_label} linked")),
                        Err(e) => k2_core::log_debug!("[postal] owner link: {e}"),
                    }
                    return;
                }
                Ok(None) => {}
                Err(e) => {
                    k2_core::log_debug!("[postal] owner link for {poll_label} ended: {e}");
                    return;
                }
            }
        }
    });
    Ok(serde_json::json!({ "ok": true, "label": label, "url": url, "userCode": user, "interval": interval }))
}

/// Is the owner's decide token linked for this box's label?
pub fn owner_linked() -> Option<bool> {
    let label = net()?.label().ok()?;
    k2_core::postal::keys::decide_token(&label).ok().map(|t| t.is_some())
}

#[cfg(test)]
#[path = "wake_tests.rs"]
mod wake_tests;

#[cfg(test)]
#[path = "short_label_tests.rs"]
mod short_label_tests;
