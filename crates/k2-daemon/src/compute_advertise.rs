//! The node machine's advertise loop (Settings → Compute, "Machines you
//! can use"; Rosson 2026-10-11). While "Make this computer a node" is on,
//! this daemon tells K2 Connect it offers compute, so the owner's other
//! servers list it; the answer may carry one-click enroll offers from a
//! controller on the same account, which this loop applies through the
//! same `local/enable` path as Settings (a ticket → the node's enroll
//! request → `pending`; the owner still confirms on the controller).
//!
//! Daemon-owned and headless: runs with no client attached. Off while the
//! compute preview is off, under air-gap, or without a tunnel login. A
//! control plane without `/compute/advertise` (404/405) is left alone for
//! hours. Contract: `.k2/prds/compute-oneclick-k2-connect-spec-v2.md`.

use std::time::{Duration, Instant};

use k2_core::compute::{self as compute, advertise};

#[derive(Default)]
struct LoopState {
    /// A row is live on the control plane (send `enabled:false` on stop).
    advertised: bool,
    /// Answers to offers, sent on the next call.
    acks: Vec<advertise::Ack>,
    /// No `/compute/advertise` on this control plane until then.
    unsupported_until: Option<Instant>,
    /// Logged the last error already (don't repeat it every tick).
    last_error: Option<String>,
}

/// Start the loop (once, at boot).
pub fn spawn() {
    if std::env::var("K2_COMPUTE_ADVERTISE").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let _ = std::thread::Builder::new().name("compute-advertise".into()).spawn(|| {
        let mut st = LoopState::default();
        // Let the tunnel come up first.
        std::thread::sleep(Duration::from_secs(30));
        loop {
            let wait = tick(&mut st);
            std::thread::sleep(wait);
        }
    });
}

fn tick(st: &mut LoopState) -> Duration {
    let idle = Duration::from_secs(advertise::ENROLLED_POLL_SECS);
    if !compute::enabled() || k2_core::airgap::enabled() {
        return idle;
    }
    if st.unsupported_until.is_some_and(|t| Instant::now() < t) {
        return idle;
    }
    let Ok(cfg) = k2_core::tunnel::config::load() else { return idle };
    let sub = cfg.subdomain.trim().to_ascii_lowercase();
    let token = cfg.token.trim().to_string();
    if sub.is_empty() || token.is_empty() {
        return idle;
    }
    let status = compute::local::status();
    let hw = advertise::probe(&compute::local::node_home());
    let body = match advertise::payload(&status, &sub, &hw, &st.acks, env!("CARGO_PKG_VERSION")) {
        Some(b) => b,
        None => {
            if st.advertised {
                match advertise::post(&token, &advertise::off_payload()) {
                    Ok(_) => st.advertised = false,
                    Err(e) => note_error(st, e),
                }
            }
            return idle;
        }
    };
    let posted = advertise::post(&token, &body);
    handle(st, posted, &status, apply)
}

/// Act on one advertise result. `apply_offer` enrolls from an offer (the
/// real one is [`apply`]; tests pass a stub).
fn handle(
    st: &mut LoopState,
    posted: Result<advertise::Posted, String>,
    status: &serde_json::Value,
    apply_offer: impl Fn(&advertise::Offer) -> advertise::Ack,
) -> Duration {
    let idle = Duration::from_secs(advertise::ENROLLED_POLL_SECS);
    let answer = match posted {
        Ok(advertise::Posted::Unsupported) => {
            k2_core::log_debug!("[compute] K2 Connect has no /compute/advertise yet; checking again in a few hours");
            st.unsupported_until = Some(Instant::now() + Duration::from_secs(advertise::UNSUPPORTED_BACKOFF_SECS));
            return idle;
        }
        Ok(advertise::Posted::Refused(e)) => {
            // The control plane won't take this body. The acks are the
            // only part carried between calls, so drop them; otherwise one
            // refused ack would refuse every later advertise until restart.
            st.acks.clear();
            note_error(st, e);
            return advertise::next_poll(None, status);
        }
        Ok(advertise::Posted::Answered(a)) => a,
        Err(e) => {
            note_error(st, e);
            return advertise::next_poll(None, status);
        }
    };
    st.advertised = true;
    st.acks.clear();
    st.last_error = None;
    for offer in &answer.offers {
        let ack = apply_offer(offer);
        st.acks.push(ack);
    }
    if !st.acks.is_empty() {
        // Report what happened (and the new state) right away.
        return Duration::from_secs(advertise::MIN_POLL_SECS.min(5));
    }
    advertise::next_poll(Some(&answer), status)
}

fn note_error(st: &mut LoopState, e: String) {
    if st.last_error.as_deref() != Some(e.as_str()) {
        eprintln!("[compute] advertise: {e}");
        st.last_error = Some(e);
    }
}

/// Apply one offer (or say why not).
fn apply(offer: &advertise::Offer) -> advertise::Ack {
    let status = compute::local::status();
    match advertise::decide(offer, &status, compute::now()) {
        advertise::Decision::Skip { ok, code } => advertise::Ack { id: offer.id.clone(), ok, code: Some(code.to_string()) },
        advertise::Decision::Apply { controller_url, name, ticket } => {
            let req = crate::compute_local::EnableReq {
                controller: Some(controller_url.clone()),
                enroll: None,
                ticket: Some(ticket),
                name: Some(name),
                private_home: None,
                allow_prompt: false,
                // An offer only reaches an installed node (it has a node
                // key), so setup never runs here; nothing to set (B-I5).
                remote_admin: None,
            };
            match crate::compute_local::enable(&req, "connect-offer") {
                Ok(v) if v["step"] == "needs_root" => {
                    advertise::Ack { id: offer.id.clone(), ok: false, code: Some("needs_root".into()) }
                }
                Ok(_) => {
                    k2_core::log_debug!("[compute] enroll offer from {controller_url} applied");
                    advertise::Ack { id: offer.id.clone(), ok: true, code: None }
                }
                Err((code, _, hint)) => {
                    eprintln!("[compute] enroll offer from {controller_url} failed: {hint} ({code})");
                    advertise::Ack { id: offer.id.clone(), ok: false, code: Some(code.to_string()) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status() -> serde_json::Value {
        json!({"installed": true, "node": {"state": "unenrolled"}})
    }

    fn ack(id: &str) -> advertise::Ack {
        advertise::Ack { id: id.into(), ok: false, code: Some("busy".into()) }
    }

    fn no_offers(_: &advertise::Offer) -> advertise::Ack {
        panic!("no offers in these answers")
    }

    #[test]
    fn a_refused_advertise_drops_pending_acks() {
        let mut st = LoopState { acks: vec![ack("o1"), ack("o2")], ..Default::default() };
        handle(&mut st, Ok(advertise::Posted::Refused("HTTP 400: bad ack".into())), &status(), no_offers);
        assert!(st.acks.is_empty(), "a 400 must not resend the same acks forever");
        assert_eq!(st.last_error.as_deref(), Some("HTTP 400: bad ack"));
        assert!(!st.advertised);
    }

    #[test]
    fn a_transient_error_keeps_pending_acks() {
        let mut st = LoopState { acks: vec![ack("o1")], ..Default::default() };
        handle(&mut st, Err("HTTP 502: store unavailable".into()), &status(), no_offers);
        assert_eq!(st.acks, vec![ack("o1")], "502/timeout: retry the same acks");
    }

    #[test]
    fn an_answer_clears_acks_and_records_new_ones() {
        let mut st = LoopState { acks: vec![ack("old")], ..Default::default() };
        let answer = advertise::Answer {
            next_poll_secs: None,
            offers: vec![advertise::Offer { id: "new".into(), controller_sub: "rosson".into(), ticket: "t".into(), exp: 0 }],
        };
        handle(&mut st, Ok(advertise::Posted::Answered(answer)), &status(), |o| ack(&o.id));
        assert_eq!(st.acks, vec![ack("new")]);
        assert!(st.advertised);
    }
}
