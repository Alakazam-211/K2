//! The VoIP lane (0.46.17, K2 Companion's CallKit ring): a DM call's ring
//! carries each phone's PushKit VoIP token (`devices[].voipToken`) to the
//! relay, which sends it with APNs push type `voip`. iOS ends an app that
//! gets a VoIP push without reporting a call, so these tests pin that ONLY
//! a ring ever carries one, on every lane, ONLY to a relay that lists
//! `call-voip` (a relay listing just `call-aps` sends no VoIP push), and
//! that a dead VoIP token clears that token alone.

#![cfg(test)]

use super::e2e::{self, PushE2eMode};
use super::seal;
use super::*;
use crate::push_device_keys::{E2eChange, NewKey};
use crate::push_devices::VoipChange;
use std::sync::{Arc, Mutex};

/// Records every relay call's body; answers with the scripted response,
/// else a 200 with no results. Its answers list `s1`, `call-aps` and,
/// when `call_voip`, `call-voip`.
struct Relay {
    bodies: Mutex<Vec<serde_json::Value>>,
    script: Mutex<std::collections::VecDeque<GatewayResponse>>,
    call_voip: bool,
}

impl Relay {
    /// A relay with the VoIP lane (`call-voip`).
    fn new() -> Arc<Self> {
        Arc::new(Self { bodies: Mutex::new(Vec::new()), script: Mutex::new(Default::default()), call_voip: true })
    }
    /// Today's relays: `call-aps`, no `call-voip`.
    fn aps_only() -> Arc<Self> {
        Arc::new(Self { bodies: Mutex::new(Vec::new()), script: Mutex::new(Default::default()), call_voip: false })
    }
    fn take(&self) -> Vec<serde_json::Value> {
        std::mem::take(&mut *self.bodies.lock().expect("relay"))
    }
    fn answer(&self, body: serde_json::Value) {
        self.script.lock().expect("relay").push_back(GatewayResponse {
            call_voip: self.call_voip,
            call_aps: true,
            status: 200,
            body: body.to_string(),
            s0: true,
            s1: true,
            s2: false,
        });
    }
    fn respond(&self) -> GatewayResponse {
        if let Some(r) = self.script.lock().expect("relay").pop_front() {
            return r;
        }
        GatewayResponse { call_voip: self.call_voip, call_aps: true, status: 200, body: "{\"ok\":true,\"results\":[]}".into(), s0: true, s1: true, s2: false }
    }
}

impl DispatchSender for Relay {
    fn post(&self, _url: &str, _auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.bodies.lock().expect("relay").push(body.clone());
        Ok(self.respond())
    }
    fn post_sealed(&self, _url: &str, _auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.bodies.lock().expect("relay").push(body.clone());
        Ok(self.respond())
    }
}

fn cloud(url: &str, relay: Arc<Relay>) -> K2Cloud {
    K2Cloud::with_config(
        GatewayConfig {
            url: url.into(),
            tunnel: Some(("tunnel-tok".into(), "owner-sub".into())),
            shared_token: None,
            subdomain: "owner-sub".into(),
        },
        relay,
    )
}

fn unique_url() -> String {
    format!("https://push-voip-{}.k2.dev", uuid::Uuid::new_v4().simple())
}

fn voip_hex() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn phone_key(seed: u8) -> NewKey {
    let (_sk, pk) = seal::derive_keypair(&[seed; 32]);
    NewKey { key_id: seal::key_id(&pk), public_key: pk, psk: vec![seed ^ 0x5a; 32], v: 1 }
}

/// Register an `apns` phone with a VoIP token (and a push key when `key`).
fn phone(id: &str, voip: &str, key: Option<NewKey>) -> crate::push_devices::PushDevice {
    let e2e = key.map_or(E2eChange::Keep, E2eChange::Set);
    crate::push_devices::upsert_device_voip(
        id,
        "apns",
        &format!("tok-{id}"),
        "owner",
        Some(true),
        None,
        e2e,
        VoipChange::Set(voip.to_string()),
    )
    .expect("register")
}

fn call(state: CallPushState) -> PushEvent {
    PushEvent::Call(CallPush {
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        call_id: "88001".into(),
        channel_id: "900".into(),
        caller: "Alex Doe".into(),
        state,
    })
}

fn badge() -> PushEvent {
    PushEvent::Badge(BadgePush {
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "3".into(),
        waiting_total: "1".into(),
        stale: Default::default(),
        read: Default::default(),
        home_unread: String::new(),
    })
}

/// Every push that is not a ring: the call's own missed / ended /
/// answered pushes, a Thread message, a badge (a nudge on the sealed
/// lane), a ticket, a project post.
fn not_rings() -> Vec<PushEvent> {
    let thread = super::legacy_golden::golden_events()
        .into_iter()
        .find(|(n, _)| n == "thread-text-x1-preview")
        .expect("golden thread")
        .1;
    vec![
        call(CallPushState::Missed),
        call(CallPushState::Ended),
        call(CallPushState::Answered),
        thread,
        badge(),
        PushEvent::FeedbackCreated { agent_name: "agent-a".into(), feedback_id: "fb-1".into(), assignees: vec!["owner".into()] },
        PushEvent::ProjectMessageCreated { project_name: "Apollo".into(), group_id: "pg-1".into() },
    ]
}

/// The lanes a phone can be on: (label, mode, has a key).
const LANES: &[(&str, PushE2eMode, bool)] = &[
    ("sealed", PushE2eMode::SealedWhenSupported, true),
    ("legacy", PushE2eMode::Off, false),
    ("generic", PushE2eMode::SealedOnly, false),
];

/// On a relay that lists `call-voip`, a ring carries the phone's
/// `voipToken` on the sealed, legacy and generic lanes, with the same body it carried before (the sealed blob,
/// the clear payload, the fixed words) and the ring's clear category. A
/// phone without one carries none.
#[test]
fn a_ring_carries_the_voip_token_on_every_lane() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    for (i, (lane, mode, keyed)) in LANES.iter().enumerate() {
        let vt = voip_hex();
        let with = phone(&format!("voip-ring-{lane}"), &vt, keyed.then(|| phone_key(80 + i as u8)));
        let plain = crate::push_devices::upsert_device_e2e(
            &format!("voip-none-{lane}"),
            "apns",
            &format!("tok-voip-none-{lane}"),
            "owner",
            Some(true),
            None,
            keyed.then(|| E2eChange::Set(phone_key(90 + i as u8))).unwrap_or(E2eChange::Keep),
        )
        .expect("register");
        let url = unique_url();
        let relay = Relay::new();
        e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: false, call_aps: true, call_voip: true });
        cloud(&url, relay.clone())
            .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", &[with.clone(), plain.clone()], *mode)
            .expect("dispatch");
        let bodies = relay.take();
        assert_eq!(bodies.len(), 1, "{lane}: one relay call: {bodies:?}");
        let body = &bodies[0];
        let devices = body["devices"].as_array().expect("devices");
        let entry = |tok: &str| devices.iter().find(|d| d["token"] == tok).unwrap_or_else(|| panic!("{lane}: {tok} rides: {body}")).clone();
        let w = entry(&with.token);
        assert_eq!(w["voipToken"], vt.as_str(), "{lane}: {body}");
        assert!(entry(&plain.token).get("voipToken").is_none(), "{lane}: no VoIP token, none sent: {body}");
        assert_eq!(body["payload"]["category"], CALL_CATEGORY_RING, "{lane}: the relay's ring check: {body}");
        assert_eq!(body["payload"]["interruptionLevel"], INTERRUPTION_TIME_SENSITIVE, "{lane}");
        match *lane {
            "sealed" => {
                assert!(w["e2e"].as_str().is_some_and(|b| !b.is_empty()), "the same sealed blob: {w}");
                assert_eq!(body["payload"]["data"]["kind"], "e2e");
            }
            "legacy" => {
                assert_eq!(body["payload"]["data"]["kind"], PUSH_KIND_CALL);
                assert_eq!(body["payload"]["data"]["callState"], "ringing");
            }
            _ => {
                assert_eq!(body["payload"]["data"]["kind"], PUSH_KIND_CALL);
                assert_eq!(body["payload"]["title"], e2e::GENERIC_TITLE);
            }
        }
        let keys: Vec<&String> = w.as_object().expect("entry").keys().collect();
        let want: &[&str] = if *keyed { &["e2e", "platform", "token", "voipToken"] } else { &["platform", "token", "voipToken"] };
        assert_eq!(keys, want, "{lane}: nothing else rides the device entry");
    }
}

/// CRITICAL (iOS ends an app that gets a VoIP push it can't report as a
/// call): nothing but a ring ever carries a `voipToken`, on any lane: not
/// the missed, ended or answered push, not a badge or nudge, not a Thread,
/// ticket or project push. A ring carries one only when the relay listed
/// `call-voip` (it sends VoIP pushes) AND `call-aps` (without the ring's
/// clear fields it could not tell a sealed ring from any other push).
#[test]
fn nothing_but_a_ring_ever_carries_a_voip_token() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    super::set_instance_id("voip-test-instance");
    for (i, (lane, mode, keyed)) in LANES.iter().enumerate() {
        let vt = voip_hex();
        let p = phone(&format!("voip-never-{lane}"), &vt, keyed.then(|| phone_key(100 + i as u8)));
        let url = unique_url();
        let relay = Relay::new();
        let c = cloud(&url, relay.clone());
        let mut events = not_rings();
        events.push(call(CallPushState::Ringing));
        for event in events {
            for (call_aps, call_voip) in [(true, true), (true, false), (false, true), (false, false)] {
                e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: false, call_aps, call_voip });
                c.dispatch_in_mode(&event, "bk-voip", std::slice::from_ref(&p), *mode).expect("dispatch");
                let bodies = relay.take();
                assert!(!bodies.is_empty(), "{lane} {event:?}: something was sent");
                let wire = serde_json::to_string(&bodies).expect("json");
                let ring = event.is_voip_ring() && call_aps && call_voip;
                let what = format!("{lane} {event:?} call-aps {call_aps} call-voip {call_voip}");
                assert_eq!(wire.contains("voipToken"), ring, "{what}: {wire}");
                assert_eq!(wire.contains(&vt), ring, "{what}: {wire}");
            }
        }
    }
    // The one predicate, pinned.
    assert!(call(CallPushState::Ringing).is_voip_ring());
    for e in not_rings() {
        assert!(!e.is_voip_ring(), "{e:?}");
        assert!(!K2Cloud::voip_lane(&e, true), "{e:?}");
    }
    assert!(!K2Cloud::voip_lane(&call(CallPushState::Ringing), false), "never without call-voip");
}

/// Today's relays list `call-aps` and no `call-voip`: they copy a ring's
/// clear fields into a normal alert push and send no VoIP push. A relay
/// advertising `s1,s2,call-aps` gets NO `voipToken` on a ring, on any
/// lane, and the ring goes exactly as before the VoIP lane: its clear
/// `category` and `interruptionLevel` still ride (`call-aps` keeps its
/// meaning).
#[test]
fn a_call_aps_relay_without_call_voip_gets_no_voip_token() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    super::set_instance_id("voip-test-instance");
    for (i, (lane, mode, keyed)) in LANES.iter().enumerate() {
        let vt = voip_hex();
        let p = phone(&format!("voip-apsonly-{lane}"), &vt, keyed.then(|| phone_key(120 + i as u8)));
        let url = unique_url();
        e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: true, call_aps: true, call_voip: false });
        assert!(e2e::relay_call_aps(&url), "{lane}: call-aps still says the clear fields ride");
        assert!(!e2e::relay_call_voip(&url), "{lane}: call-aps is not call-voip");
        let relay = Relay::aps_only();
        cloud(&url, relay.clone())
            .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", std::slice::from_ref(&p), *mode)
            .expect("dispatch");
        let bodies = relay.take();
        assert_eq!(bodies.len(), 1, "{lane}: one relay call: {bodies:?}");
        let wire = serde_json::to_string(&bodies).expect("json");
        assert!(!wire.contains("voipToken") && !wire.contains(&vt), "{lane}: no VoIP token on a call-aps relay: {wire}");
        let body = &bodies[0];
        assert_eq!(body["payload"]["category"], CALL_CATEGORY_RING, "{lane}: call-aps still copies the ring's fields: {body}");
        assert_eq!(body["payload"]["interruptionLevel"], INTERRUPTION_TIME_SENSITIVE, "{lane}: {body}");
        let keys: Vec<&String> = body["devices"][0].as_object().expect("entry").keys().collect();
        let want: &[&str] = if *keyed { &["e2e", "platform", "token"] } else { &["platform", "token"] };
        assert_eq!(keys, want, "{lane}: the device entry as before the VoIP lane");
        // The relay's answer (call-aps, no call-voip) keeps it that way.
        assert!(!e2e::relay_call_voip(&url), "{lane}");
    }
    assert!(e2e::caps_has_call_voip("s1, s2, call-aps, call-voip"));
    assert!(e2e::caps_has_call_voip("CALL-VOIP"));
    assert!(!e2e::caps_has_call_voip("s1,s2,call-aps"));
    assert!(!e2e::caps_has_call_voip("call-voip2"));
    assert!(e2e::caps_has_call_aps("s1,s2,call-aps"));
    assert_eq!(e2e::CAPS_CALL_VOIP, "call-voip", "the relay contract");
    assert_eq!(e2e::CAPS_CALL_APS, "call-aps", "unchanged");
}

/// The real sender on a real socket: the relay's own `X-K2-Push-Caps`
/// header decides. With nothing heard yet, the first ring probes the
/// relay; a relay answering `s1,s2,call-aps` gets the ring with its clear
/// fields and no `voipToken`; one answering `s1,s2,call-aps,call-voip`
/// gets the phone's `voipToken`.
#[test]
fn over_a_real_socket_only_the_call_voip_header_opens_the_voip_lane() {
    use super::e2e_tests::{http_answer, loopback_cloud, LoopbackRelay};
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    super::set_instance_id("voip-test-instance");
    for (caps, want) in [("s1,s2,call-aps", false), ("s1,s2,call-aps,call-voip", true)] {
        let vt = voip_hex();
        let p = phone(&format!("voip-socket-{want}"), &vt, None);
        let header = format!("X-K2-Push-Caps: {caps}");
        let marked = ["X-K2-Push-Gateway: s0", header.as_str()];
        // The probe's answer (the relay's own 400), then the ring's.
        let relay = LoopbackRelay::start(vec![http_answer(400, &marked), http_answer(200, &marked)]);
        let url = relay.url.clone();
        assert!(!e2e::relay_caps_heard(&url), "{caps}: nothing heard yet");
        loopback_cloud(&url)
            .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", std::slice::from_ref(&p), PushE2eMode::Off)
            .expect("dispatch");
        assert!(e2e::relay_call_aps(&url), "{caps}");
        assert_eq!(e2e::relay_call_voip(&url), want, "{caps}");
        let seen = relay.requests();
        assert_eq!(seen.len(), 2, "{caps}: the probe, then the ring: {seen:?}");
        assert_eq!(seen[0].1, e2e::caps_probe_body(), "{caps}: the probe goes first");
        let ring = &seen[1].1;
        assert_eq!(ring["payload"]["category"], CALL_CATEGORY_RING, "{caps}: {ring}");
        assert_eq!(ring["payload"]["data"]["callState"], "ringing", "{caps}: {ring}");
        let entry = &ring["devices"][0];
        assert_eq!(entry["token"], p.token.as_str(), "{caps}: {ring}");
        if want {
            assert_eq!(entry["voipToken"], vt.as_str(), "{caps}: {ring}");
        } else {
            assert!(entry.get("voipToken").is_none(), "{caps}: {ring}");
            assert!(!ring.to_string().contains(&vt), "{caps}: {ring}");
        }
    }
}

/// An `fcm` row can't hold a VoIP token (refused at register), so no fcm
/// entry ever carries one, and `upsert_device_e2e` (every caller but the
/// phone's register) keeps the stored token.
#[test]
fn only_apns_rows_hold_a_voip_token_and_other_writers_keep_it() {
    let _db = crate::db::scoped_for_test();
    let err = crate::push_devices::upsert_device_voip(
        "voip-fcm",
        "fcm",
        "tok-voip-fcm",
        "owner",
        None,
        None,
        E2eChange::Keep,
        VoipChange::Set(voip_hex()),
    )
    .expect_err("fcm with a VoIP token");
    assert!(err.contains("apns"), "{err}");
    assert!(crate::push_devices::get_device("voip-fcm").expect("get").is_none(), "nothing written");
    let bads: Vec<String> = vec![
        "".into(),
        " ".into(),
        "xyz".into(),
        "a".repeat(63),
        "a".repeat(201),
        format!(" {} ", voip_hex()),
        format!("{} ", voip_hex()),
        "g".repeat(64),
    ];
    for bad in bads {
        let r = crate::push_devices::upsert_device_voip(
            "voip-bad",
            "apns",
            "tok-voip-bad",
            "owner",
            None,
            None,
            E2eChange::Keep,
            VoipChange::Set(bad.clone()),
        );
        assert!(r.is_err(), "{bad:?} refused");
        assert!(crate::push_devices::get_device("voip-bad").expect("get").is_none(), "{bad:?}: nothing written");
    }
    let vt = voip_hex();
    let p = phone("voip-keep", &vt, None);
    let again = crate::push_devices::upsert_device_e2e(&p.device_id, "apns", &p.token, "owner", None, None, E2eChange::Keep).expect("re-upsert");
    assert_eq!(again.voip_token.as_deref(), Some(vt.as_str()), "Keep keeps it");
    let cleared = crate::push_devices::upsert_device_voip(&p.device_id, "apns", &p.token, "owner", None, None, E2eChange::Keep, VoipChange::Clear)
        .expect("clear");
    assert_eq!(cleared.voip_token, None);
    assert_eq!(crate::push_devices::validate_voip_token("apns", Some(&vt)), Ok(VoipChange::Set(vt.clone())));
    assert_eq!(crate::push_devices::validate_voip_token("apns", Some("")), Ok(VoipChange::Clear));
    assert_eq!(crate::push_devices::validate_voip_token("apns", None), Ok(VoipChange::Clear));
    assert_eq!(crate::push_devices::validate_voip_token("fcm", None), Ok(VoipChange::Clear));
    assert_eq!(crate::push_devices::validate_voip_token("fcm", Some("")), Ok(VoipChange::Clear));
    assert!(crate::push_devices::validate_voip_token("fcm", Some(&vt)).is_err());
    // Never serialized: a stored row's JSON has no VoIP token.
    let p = phone("voip-json", &vt, None);
    let json = serde_json::to_string(&p).expect("json");
    assert!(!json.contains(&vt) && !json.contains("voipToken") && !json.contains("voip_token"), "{json}");
}

/// The relay's bounds: exactly 64-200 hex characters, either case. 63 and
/// 201 are refused, and so is anything with a space (leading, trailing or
/// only spaces), a slash or another non-hex character. Nothing is trimmed:
/// a token is stored exactly as sent. Only `None` and `""` clear it.
#[test]
fn a_voip_token_is_64_to_200_hex_characters_exactly() {
    use crate::push_devices::{validate_voip_token as v, VOIP_TOKEN_MAX, VOIP_TOKEN_MIN};
    assert_eq!((VOIP_TOKEN_MIN, VOIP_TOKEN_MAX), (64, 200), "the relay's bounds");
    let mixed64 = format!("{}{}", "aB3f".repeat(15), "0F9e");
    assert_eq!(mixed64.len(), 64);
    for ok in ["a".repeat(64), "F".repeat(64), "0".repeat(200), "c".repeat(137), mixed64, format!("{}{}", "AbCdEf".repeat(33), "12")] {
        assert_eq!(v("apns", Some(&ok)), Ok(VoipChange::Set(ok.clone())), "{} chars accepted, stored as sent", ok.len());
    }
    let hex64 = "a".repeat(64);
    let bads = [
        "a".repeat(63),
        "a".repeat(201),
        "A".repeat(63),
        "F".repeat(201),
        format!("{hex64} "),
        format!(" {hex64}"),
        format!("{hex64}\n"),
        format!("{}\t{}", "a".repeat(32), "a".repeat(32)),
        format!("{}/{}", "a".repeat(32), "a".repeat(32)),
        format!("{}-{}", "a".repeat(32), "a".repeat(32)),
        format!("{}g", "a".repeat(64)),
        format!("0x{}", "a".repeat(64)),
        " ".into(),
        "   ".into(),
    ];
    for bad in bads {
        let e = v("apns", Some(&bad)).expect_err(&format!("{bad:?} ({} chars) is refused", bad.len()));
        assert!(e.starts_with("usage:") && e.contains("64-200"), "{bad:?}: {e}");
    }
    assert_eq!(v("apns", None), Ok(VoipChange::Clear));
    assert_eq!(v("apns", Some("")), Ok(VoipChange::Clear));
}

/// The relay's `results[].voip.dead` (Apple refused the VoIP token)
/// clears that phone's VoIP token and nothing else: the row and its alert
/// token stay, so the next ring is the alert. The attempt record says
/// `voip=dead`, and the change is logged with no token. A plain `dead`
/// (the alert token) still removes the row as before.
#[test]
fn a_dead_voip_token_is_cleared_and_the_phone_stays() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let vt = voip_hex();
    let short = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let p = phone(&format!("{short}-voipdead"), &vt, None);
    let url = unique_url();
    let relay = Relay::new();
    relay.answer(serde_json::json!({
        "ok": true,
        "results": [{ "token": p.token, "sent": true, "dead": false, "voip": { "sent": false, "dead": true, "error": "apns 410: Unregistered" } }],
    }));
    e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: false, call_aps: true, call_voip: true });
    cloud(&url, relay.clone())
        .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", std::slice::from_ref(&p), PushE2eMode::Off)
        .expect("dispatch");
    assert_eq!(relay.take()[0]["devices"][0]["voipToken"], vt.as_str());
    let row = crate::push_devices::get_device(&p.device_id).expect("get").expect("the phone stays");
    assert_eq!(row.voip_token, None, "the dead VoIP token is cleared");
    assert_eq!(row.token, p.token, "the alert token stays");
    let logged = device_log::recent(Some(&short));
    let line = logged
        .iter()
        .find(|e| e.reason == "vendor_dead" && e.action == "update")
        .unwrap_or_else(|| panic!("the clear is logged: {logged:?}"));
    assert_eq!(line.detail.as_deref(), Some("voip=cleared"));
    assert!(!format!("{logged:?}").contains(&vt), "never the token");
    let att = attempts::recent(Some(&short));
    let a = att.last().expect("an attempt record");
    assert_eq!((a.vendor, a.voip), (Some("sent"), Some("dead")), "{a:?}");
    assert!(a.line().contains(" voip=dead"), "{}", a.line());
    assert!(!a.line().contains(&vt));

    // The next ring goes with no VoIP token (the alert lane).
    e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: false, call_aps: true, call_voip: true });
    cloud(&url, relay.clone())
        .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", &[row.clone()], PushE2eMode::Off)
        .expect("dispatch");
    assert!(relay.take()[0]["devices"][0].get("voipToken").is_none());

    // The alert token dead: the row goes, as before.
    let p2 = phone(&format!("{short}-alertdead"), &voip_hex(), None);
    relay.answer(serde_json::json!({ "ok": true, "results": [{ "token": p2.token, "sent": false, "dead": true }] }));
    e2e::note_relay_caps_full(&url, e2e::RelayCaps { s1: true, s2: false, call_aps: true, call_voip: true });
    cloud(&url, relay.clone())
        .dispatch_in_mode(&call(CallPushState::Ringing), "bk-voip", std::slice::from_ref(&p2), PushE2eMode::Off)
        .expect("dispatch");
    assert!(crate::push_devices::get_device(&p2.device_id).expect("get").is_none(), "pruned");
}
