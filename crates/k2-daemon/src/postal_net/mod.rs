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
//! - [`live`]: the live listener's check, not mounted (S6 waits on the
//!   plane's P-6).
//!
//! The loop polls `/events` (25 s long polls) and does a full pass at
//! least every 60 s; when the plane can't be reached it backs off to 15
//! minutes, and every answer that would go out says the network isn't
//! connected ([`k2_core::postal::gate::network_available`]).

pub mod live;
pub mod pairing;
pub mod plane;
pub mod receive;
pub mod send;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::postal::sdk::plane::{self as sdk_plane, AfterEvents, Event};

use plane::{CallError, Plane};

static NET: Mutex<Option<Arc<Plane>>> = Mutex::new(None);
static LAST: Mutex<Option<(String, String)>> = Mutex::new(None);

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

/// The last network problem (code, hint), for `k2 postal status`.
pub fn last_problem() -> Option<(String, String)> {
    LAST.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn note(r: &Result<(), CallError>) {
    let mut g = LAST.lock().unwrap_or_else(|p| p.into_inner());
    match r {
        Ok(()) => {
            *g = None;
            k2_core::postal::gate::set_network_available(true);
        }
        Err(e) => {
            let new = (e.code(), e.hint());
            if g.as_ref() != Some(&new) {
                k2_core::log_debug!("[postal] network: {}", new.0);
            }
            *g = Some(new);
            k2_core::postal::gate::set_network_available(false);
        }
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
    let mut cursor: Option<String> = None;
    let mut backoff = Duration::from_secs(60);
    let mut last_full = Instant::now() - Duration::from_secs(3600);
    loop {
        if !k2_core::postal::enabled() {
            k2_core::postal::gate::set_network_available(false);
            cursor = None;
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        let Some(net) = net() else {
            std::thread::sleep(Duration::from_secs(60));
            continue;
        };
        let r = if last_full.elapsed() >= Duration::from_secs(60) || cursor.is_none() {
            last_full = Instant::now();
            tick(&net).and_then(|()| events(&net, &mut cursor, 0))
        } else {
            events(&net, &mut cursor, events_wait())
        };
        let failed = r.is_err();
        note(&r);
        if failed {
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(15 * 60));
            last_full = Instant::now() - Duration::from_secs(3600);
        } else {
            backoff = Duration::from_secs(60);
            if events_wait() == 0 {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
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
