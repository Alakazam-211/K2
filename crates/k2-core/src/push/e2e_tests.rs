//! The tests that guard the E2E push claim (prd-e2e-push-v1 rev 2 §8):
//! the no-leak canary test, the legacy-unchanged golden with keys present,
//! the switch modes, the `s1` relay gate, the dispatch round trip and the
//! phone-side replay check (a test helper, as the phones implement it).

#![cfg(test)]

use super::attempts;
use super::e2e::{self, PushE2eMode};
use super::legacy_golden;
use super::seal::{self, SealKind};
use super::*;
use crate::push_device_keys::{self as keys, DeviceOverride, E2eChange, NewKey};
use std::sync::{Arc, Mutex};

/// Records every call (auth + body); answers 200 with the S0 marker and,
/// when `caps_s1`, `X-K2-Push-Caps: s1`. Scripted responses go first.
struct Relay {
    calls: Mutex<Vec<(GatewayAuth, serde_json::Value, bool)>>,
    caps_s1: Mutex<bool>,
    script: Mutex<std::collections::VecDeque<GatewayResponse>>,
}

impl Relay {
    fn new(caps_s1: bool) -> Arc<Self> {
        Arc::new(Self { calls: Mutex::new(Vec::new()), caps_s1: Mutex::new(caps_s1), script: Mutex::new(Default::default()) })
    }
    fn calls(&self) -> Vec<(GatewayAuth, serde_json::Value, bool)> {
        self.calls.lock().expect("relay").clone()
    }
    fn clear(&self) {
        self.calls.lock().expect("relay").clear();
    }
    fn respond(&self) -> GatewayResponse {
        if let Some(r) = self.script.lock().expect("relay").pop_front() {
            return r;
        }
        GatewayResponse {
            status: 200,
            body: "{\"ok\":true,\"results\":[]}".into(),
            s0: true,
            s1: *self.caps_s1.lock().expect("relay"),
        }
    }
}

impl DispatchSender for Relay {
    fn post(&self, _url: &str, auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.calls.lock().expect("relay").push((auth.clone(), body.clone(), false));
        Ok(self.respond())
    }
    fn post_sealed(&self, _url: &str, auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.calls.lock().expect("relay").push((auth.clone(), body.clone(), true));
        Ok(self.respond())
    }
}

fn unique_url(label: &str) -> String {
    format!("https://push-{label}-{}.k2.dev", uuid::Uuid::new_v4().simple())
}

/// A per-daemon (tunnel) cloud, `subdomain` = rosson.
fn tunnel_cloud(url: &str, relay: Arc<Relay>) -> K2Cloud {
    K2Cloud::with_config(
        GatewayConfig {
            url: url.into(),
            tunnel: Some(("tunnel-tok".into(), "rosson".into())),
            shared_token: Some("shared-tok".into()),
            subdomain: "rosson".into(),
        },
        relay,
    )
}

/// The phone's key for a seed: (private key, NewKey).
fn phone_key(seed: u8) -> (Vec<u8>, NewKey) {
    let (sk, pk) = seal::derive_keypair(&[seed; 32]);
    let psk = vec![seed ^ 0x5a; 32];
    (sk, NewKey { key_id: seal::key_id(&pk), public_key: pk, psk })
}

/// Register a device in the (scoped) DB, optionally with a key.
fn register(id: &str, platform: &str, token: &str, previews: bool, key: Option<&NewKey>) -> crate::push_devices::PushDevice {
    let change = match key {
        Some(k) => E2eChange::Set(k.clone()),
        None => E2eChange::Keep,
    };
    crate::push_devices::upsert_device_e2e(id, platform, token, "owner", Some(previews), None, change).expect("register")
}

const CANARIES: &[&str] = &[
    "CanaryAgentZx",
    "CanaryProjectQv",
    "canary-preview-text",
    "canary-msg-fulltext",
    "item-canary-5",
    "conv-canary-77",
    "addr-canary/rev",
    "fb-canary-42",
    "grp-canary-9",
    "proj-canary-1",
    "rosson",
    "9f2c0a1b3d4e5f60",
    // A permission card's push (prd-agent-settings-cards-v1 rev 3.1 §17.7).
    "pr_c4a2f00d",
    "CanarySettingWv",
];

fn canary_events() -> Vec<PushEvent> {
    let thread = |kind: ThreadMsgKind, count: u32| {
        PushEvent::ThreadPost(ThreadPush {
            agent_name: "CanaryAgentZx".into(),
            conversation_id: "conv-canary-77".into(),
            addr: "addr-canary/rev".into(),
            seq: 7,
            project_id: "proj-canary-1".into(),
            msg_kind: kind,
            count,
            principal: "owner".into(),
            device_usernames: vec!["owner".into()],
            unread_total: "4".into(),
            waiting_total: "1".into(),
            avatar_rev: "ba7816bf8f01cfea".into(),
            agent_color: "#3b82f6".into(),
            preview: "canary-preview-text".into(),
            msg: Some(ThreadMsg::new("item-canary-5", 7, "conv-canary-77", "addr-canary/rev", 1_760_960_000, kind, "canary-msg-fulltext and more")),
            budget: crate::push::ThreadBudget::Thread,
        })
    };
    let permission = match thread(ThreadMsgKind::Choice, 1) {
        PushEvent::ThreadPost(t) => PushEvent::ThreadPost(ThreadPush {
            preview: String::new(),
            msg: None,
            budget: crate::push::ThreadBudget::Permission { request_id: "pr_c4a2f00d".into(), setting: "CanarySettingWv".into() },
            ..t
        }),
        _ => unreachable!(),
    };
    vec![
        permission,
        thread(ThreadMsgKind::Text, 1),
        thread(ThreadMsgKind::Text, 3),
        thread(ThreadMsgKind::Choice, 1),
        thread(ThreadMsgKind::Secret, 1),
        PushEvent::FeedbackCreated { agent_name: "CanaryAgentZx".into(), feedback_id: "fb-canary-42".into(), assignees: vec![] },
        PushEvent::FeedbackCommented { feedback_id: "fb-canary-42".into(), assignees: vec![] },
        PushEvent::ProjectMessageCreated { project_name: "CanaryProjectQv".into(), group_id: "grp-canary-9".into() },
        PushEvent::ProjectPost(ProjectPush {
            project_name: "CanaryProjectQv".into(),
            group_id: "grp-canary-9".into(),
            principal: "owner".into(),
            device_usernames: vec!["owner".into()],
            unread_total: "7".into(),
            waiting_total: "2".into(),
        }),
        PushEvent::Badge(BadgePush {
            principal: "owner".into(),
            device_usernames: vec!["owner".into()],
            unread_total: "3".into(),
            waiting_total: "1".into(),
        }),
    ]
}

fn assert_no_canary(wire: &str, what: &str) {
    for c in CANARIES {
        assert!(!wire.contains(c), "{what}: canary {c:?} on the wire: {wire}");
    }
}

/// The canary invariant (§8): for every mobile kind × (sealed, legacy-
/// generic) × device previews × owner previews switch, the full dispatch
/// body carries none of the plaintext fields: agent/project names, the
/// preview, conversationId, addr, feedbackId, groupId, projectId, and
/// (sealed kinds) not even the server name or instance id.
#[test]
fn no_plaintext_field_ever_rides_a_sealed_or_generic_push() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k1) = phone_key(1);
    let (_, k2) = phone_key(2);
    let keyed = vec![
        register("nl-on", "apns", "tok-nl-on", true, Some(&k1)),
        register("nl-off", "fcm", "tok-nl-off", false, Some(&k2)),
    ];
    let unkeyed = vec![register("nl-old-on", "apns", "tok-nl-old-on", true, None), register("nl-old-off", "fcm", "tok-nl-old-off", false, None)];
    for owner_previews in [true, false] {
        for event in canary_events() {
            let event = if owner_previews { event } else { event.without_preview() };
            // Sealed: keyed devices, sealed-when-supported and sealed-only.
            for mode in [PushE2eMode::SealedWhenSupported, PushE2eMode::SealedOnly] {
                let url = unique_url("noleak");
                let relay = Relay::new(true);
                e2e::note_relay_caps(&url, true);
                tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, "0123456789abcdef0123456789abcdef", &keyed, mode).expect("sealed");
                let calls = relay.calls();
                assert_eq!(calls.len(), 1, "one relay call for the person's keyed phones: {event:?}");
                let (auth, body, sealed) = &calls[0];
                assert!(*sealed && auth.is_tunnel(), "sealed rides the per-daemon path");
                let wire = body.to_string();
                assert_no_canary(&wire, &format!("sealed {mode:?} {event:?}"));
                assert!(!wire.contains("badgeKey") && !wire.contains("0123456789abcdef0123456789abcdef"), "no badgeKey in clear");
                assert!(!wire.contains(legacy_golden::GOLDEN_INSTANCE) && !wire.contains("instanceId"), "no instance id in clear");
                assert!(!wire.contains("subdomain") && !wire.contains("unreadTotal"), "no stamp or counts in clear");
                let payload = body["payload"].as_object().expect("payload");
                assert_eq!(payload.keys().collect::<Vec<_>>(), vec!["data"], "payload is data only: {wire}");
                let data = body["payload"]["data"].as_object().expect("data");
                assert_eq!(data.len(), 2, "data is exactly {{kind, v}}: {wire}");
                assert_eq!(data["v"], "1");
                let want_kind = if matches!(event, PushEvent::Badge(_)) { "nudge" } else { "e2e" };
                assert_eq!(data["kind"], want_kind);
                for d in body["devices"].as_array().expect("devices") {
                    let obj = d.as_object().expect("device");
                    assert_eq!(obj.len(), 3, "device = platform, token, e2e: {d}");
                    let len = d["e2e"].as_str().expect("e2e").len();
                    assert_eq!(len, if want_kind == "nudge" { 452 } else { 2159 });
                }
            }
            // Legacy-generic: unkeyed devices at sealed-only.
            let url = unique_url("noleak-generic");
            let relay = Relay::new(true);
            e2e::note_relay_caps(&url, true);
            tunnel_cloud(&url, relay.clone())
                .dispatch_in_mode(&event, "0123456789abcdef0123456789abcdef", &unkeyed, PushE2eMode::SealedOnly)
                .expect("generic");
            for (_, body, sealed) in relay.calls() {
                assert!(!sealed);
                let wire = body.to_string();
                if matches!(event, PushEvent::Badge(_)) {
                    // A badge push is today's badge payload (no names or ids).
                    assert_eq!(body["payload"], K2Cloud::payload(&event, &PushStamp { subdomain: "rosson".into(), instance_id: legacy_golden::GOLDEN_INSTANCE.into() }, "0123456789abcdef0123456789abcdef"));
                    continue;
                }
                for c in CANARIES.iter().filter(|c| **c != "rosson" && **c != "9f2c0a1b3d4e5f60") {
                    assert!(!wire.contains(c), "generic: canary {c:?}: {wire}");
                }
                assert_eq!(body["payload"]["title"], "K2");
                assert_eq!(body["payload"]["body"], "New message");
                assert!(body["payload"].get("threadId").is_none() && body["payload"].get("collapseId").is_none());
            }
        }
    }
}

/// Opening what dispatch sent: each device's own blob opens with its own
/// key only, carries today's title/body/data for that device's previews,
/// the server stamp sealed, and a fresh counter.
#[test]
fn dispatch_seals_per_device_and_the_phone_opens_it() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (sk1, k1) = phone_key(11);
    let (sk2, k2) = phone_key(12);
    let devices = vec![
        register("rt-on", "apns", "tok-rt-on", true, Some(&k1)),
        register("rt-off", "fcm", "tok-rt-off", false, Some(&k2)),
    ];
    let url = unique_url("roundtrip");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true);
    let cloud = tunnel_cloud(&url, relay.clone());
    let event = legacy_golden::golden_events()
        .into_iter()
        .find(|(n, _)| n == "thread-text-x1-preview")
        .expect("case")
        .1;
    cloud.dispatch_in_mode(&event, "bk-test", &devices, PushE2eMode::SealedWhenSupported).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 1);
    let devs = calls[0].1["devices"].as_array().expect("devices").clone();
    assert_eq!(devs.len(), 2);
    let blob_on = devs[0]["e2e"].as_str().expect("e2e");
    let blob_off = devs[1]["e2e"].as_str().expect("e2e");
    assert!(seal::open(SealKind::Alert, &sk2, &k2.psk, blob_on).is_err(), "another phone's key can't open it");
    let on: serde_json::Value = serde_json::from_slice(&seal::open(SealKind::Alert, &sk1, &k1.psk, blob_on).expect("open")).expect("json");
    let off: serde_json::Value = serde_json::from_slice(&seal::open(SealKind::Alert, &sk2, &k2.psk, blob_off).expect("open")).expect("json");
    assert_eq!(on["v"], 1);
    assert_eq!(on["kind"], "thread");
    assert_eq!(on["kid"], k1.key_id);
    assert_eq!(on["srv"], serde_json::json!({ "sub": "rosson", "iid": legacy_golden::GOLDEN_INSTANCE }));
    assert_eq!(on["bk"], "bk-test");
    assert_eq!(on["title"], "Cortana");
    assert_eq!(on["body"], "Deploy finished, 3 checks green 🚀");
    assert_eq!(off["body"], "New message in Thread", "previews off: today's generic line, inside the blob");
    assert_eq!(on["group"], "thread:c-77");
    assert_eq!(on["data"], event.data().to_value(), "today's data keys for the router");
    assert!(on["ctr"].as_u64().expect("ctr") > 1_700_000_000_000, "ctr = max(next_ctr, unix_ms)");
    // A second dispatch: a bigger counter.
    let first = on["ctr"].as_u64().expect("ctr");
    relay.clear();
    cloud.dispatch_in_mode(&event, "bk-test", &devices[..1], PushE2eMode::SealedWhenSupported).expect("again");
    let again = relay.calls()[0].1["devices"][0]["e2e"].as_str().expect("e2e").to_string();
    let again: serde_json::Value = serde_json::from_slice(&seal::open(SealKind::Alert, &sk1, &k1.psk, &again).expect("open")).expect("json");
    assert!(again["ctr"].as_u64().expect("ctr") > first);

    // The nudge: totals only.
    relay.clear();
    let badge = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "badge").expect("badge").1;
    cloud.dispatch_in_mode(&badge, "bk-test", &devices[..1], PushE2eMode::SealedWhenSupported).expect("nudge");
    let calls = relay.calls();
    assert_eq!(calls[0].1["payload"], serde_json::json!({ "data": { "kind": "nudge", "v": "1" } }), "no title, no body");
    let n: serde_json::Value = serde_json::from_slice(
        &seal::open(SealKind::Nudge, &sk1, &k1.psk, calls[0].1["devices"][0]["e2e"].as_str().expect("e2e")).expect("open"),
    )
    .expect("json");
    assert_eq!(n["kind"], "nudge");
    assert_eq!(n["data"], serde_json::json!({ "unreadTotal": "3", "waitingTotal": "1" }));
    assert!(n.get("group").is_none() && n.get("title").is_none());
}

/// Today's bodies, byte for byte, with keys registered: the switch `off`,
/// a device override `off`, and a relay that never said `s1`.
#[test]
fn legacy_bodies_stay_byte_identical_with_keys_present() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k1) = phone_key(21);
    let (_, k2) = phone_key(22);
    // The golden devices, now keyed in the DB.
    register("golden-dev-on", "apns", "tok-golden-on", true, Some(&k1));
    register("golden-dev-off", "fcm", "tok-golden-off", false, Some(&k2));
    let run = |mode: PushE2eMode, caps: bool| {
        let url = unique_url("golden-keys");
        e2e::note_relay_caps(&url, caps);
        legacy_golden::collect_with(
            &|sender| {
                K2Cloud::with_config(
                    GatewayConfig {
                        url: url.clone(),
                        tunnel: Some(("tunnel-tok".into(), "rosson".into())),
                        shared_token: Some("gw".into()),
                        subdomain: "rosson".into(),
                    },
                    sender,
                )
            },
            &|cloud, event, devices| {
                cloud.dispatch_in_mode(event, legacy_golden::GOLDEN_BADGE_KEY, devices, mode).expect("dispatch");
            },
        )
    };
    legacy_golden::assert_matches_golden(&run(PushE2eMode::Off, true), "pushE2E off, keys present, relay s1");
    legacy_golden::assert_matches_golden(&run(PushE2eMode::SealedWhenSupported, false), "relay without s1");
    keys::set_override("golden-dev-on", Some(DeviceOverride::Off)).expect("override");
    keys::set_override("golden-dev-off", Some(DeviceOverride::Off)).expect("override");
    legacy_golden::assert_matches_golden(&run(PushE2eMode::SealedWhenSupported, true), "device override off");
    legacy_golden::assert_matches_golden(&run(PushE2eMode::SealedOnly, true), "override off beats sealed-only");
}

/// §4's table, end to end through dispatch.
#[test]
fn switch_modes_and_overrides_follow_the_table() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(31);
    let keyed = register("sw-key", "apns", "tok-sw-key", true, Some(&k));
    let old = register("sw-old", "apns", "tok-sw-old", true, None);
    let event = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "ticket-created").expect("t").1;
    let run = |mode: PushE2eMode| -> Vec<(bool, serde_json::Value)> {
        let url = unique_url("switch");
        let relay = Relay::new(true);
        e2e::note_relay_caps(&url, true);
        tunnel_cloud(&url, relay.clone())
            .dispatch_in_mode(&event, "bk", &[keyed.clone(), old.clone()], mode)
            .expect("dispatch");
        relay.calls().into_iter().map(|(_, b, s)| (s, b)).collect()
    };
    let tokens = |b: &serde_json::Value| -> Vec<String> {
        b["devices"].as_array().expect("d").iter().map(|d| d["token"].as_str().expect("t").to_string()).collect()
    };
    // off: one legacy call for both.
    let c = run(PushE2eMode::Off);
    assert_eq!(c.len(), 1);
    assert!(!c[0].0);
    assert_eq!(tokens(&c[0].1), vec!["tok-sw-key", "tok-sw-old"]);
    // sealed-when-supported: legacy for the old phone, sealed for the keyed.
    let c = run(PushE2eMode::SealedWhenSupported);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].0, tokens(&c[0].1)), (false, vec!["tok-sw-old".to_string()]));
    assert_eq!((c[1].0, tokens(&c[1].1)), (true, vec!["tok-sw-key".to_string()]));
    assert_eq!(c[0].1["payload"]["body"], "Cortana needs you on a ticket", "old phone: today's push");
    // sealed-only: generic for the old phone.
    let c = run(PushE2eMode::SealedOnly);
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].1["payload"]["body"], "New message");
    assert!(c[1].0);
    // Override sealed beats a server `off`.
    keys::set_override("sw-key", Some(DeviceOverride::Sealed)).expect("o");
    let c = run(PushE2eMode::Off);
    assert_eq!(c.len(), 2);
    assert!(c.iter().any(|(s, b)| *s && tokens(b) == vec!["tok-sw-key"]));
    // Override off beats sealed-when-supported.
    keys::set_override("sw-key", Some(DeviceOverride::Off)).expect("o");
    let c = run(PushE2eMode::SealedWhenSupported);
    assert_eq!(c.len(), 1);
    assert!(!c[0].0);
    // An expired key is no key.
    keys::set_override("sw-key", None).expect("o");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE push_device_keys SET expires_at = 1 WHERE device_id = 'sw-key'", []).expect("expire");
    }
    let c = run(PushE2eMode::SealedWhenSupported);
    assert_eq!(c.len(), 1, "expired key: today's path");
    assert!(!c[0].0);
    // The pure table.
    use e2e::{effective, lane, Effective, Lane};
    assert_eq!(effective(PushE2eMode::Off, None), Effective::Off);
    assert_eq!(lane(Effective::Sealed, true, false), Lane::Legacy);
    assert_eq!(lane(Effective::SealedOnly, false, true), Lane::Generic);
    assert_eq!(lane(Effective::SealedOnly, true, false), Lane::Generic);
    assert!(e2e::enforce(PushE2eMode::SealedOnly, None));
    assert!(!e2e::enforce(PushE2eMode::SealedOnly, Some(DeviceOverride::Off)));
    assert!(!e2e::enforce(PushE2eMode::SealedWhenSupported, None));
}

/// The `s1` gate: sealed only after the relay said `X-K2-Push-Caps: s1`;
/// a response without it sends the next dispatch back to today's path; no
/// tunnel (shared token only) is never sealed; a sealed refusal never
/// falls back to the shared token or retries.
#[test]
fn sealed_waits_for_s1_and_never_falls_back() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    crate::push::reset_relay_state_for_test();
    let (_, k) = phone_key(41);
    let d = vec![register("s1-key", "apns", "tok-s1-key", true, Some(&k))];
    let event = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "project-post").expect("p").1;
    let url = unique_url("s1");
    let relay = Relay::new(true);
    let cloud = tunnel_cloud(&url, relay.clone());
    assert!(!e2e::relay_s1(&url), "unknown until seen");
    cloud.dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported).expect("first");
    assert!(!relay.calls()[0].2, "first push before s1 was seen: today's path");
    assert!(e2e::relay_s1(&url), "learned from the legacy response");
    relay.clear();
    cloud.dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported).expect("second");
    assert!(relay.calls()[0].2, "now sealed");
    // Relay rollback: a response without s1.
    *relay.caps_s1.lock().expect("relay") = false;
    relay.clear();
    cloud.dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported).expect("third");
    assert!(relay.calls()[0].2, "this one was decided before the rollback was seen");
    assert!(!e2e::relay_s1(&url), "the response without s1 is cached");
    relay.clear();
    cloud.dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported).expect("fourth");
    assert!(!relay.calls()[0].2, "back on today's path");

    // Shared token only: never sealed, whatever the cache says.
    let url2 = unique_url("s1-shared");
    e2e::note_relay_caps(&url2, true);
    let relay2 = Relay::new(true);
    K2Cloud::with_config(
        GatewayConfig { url: url2.clone(), tunnel: None, shared_token: Some("gw".into()), subdomain: "rosson".into() },
        relay2.clone(),
    )
    .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
    .expect("shared");
    assert!(!relay2.calls()[0].2, "the relay refuses sealed kinds on the shared token");

    // A sealed refusal under sealed-only: no shared fallback, no retry, no
    // legacy fallback, error class only.
    let url3 = unique_url("s1-refused");
    e2e::note_relay_caps(&url3, true);
    let relay3 = Relay::new(true);
    relay3.script.lock().expect("relay").push_back(GatewayResponse {
        status: 401,
        body: "{\"error\":{\"code\":\"secret-detail-canary\"},\"results\":[{\"error\":\"/3/device/tok\"}]}".into(),
        s0: false,
        s1: false,
    });
    let err = tunnel_cloud(&url3, relay3.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedOnly)
        .expect_err("refused");
    assert_eq!(relay3.calls().len(), 1, "exactly one call: no shared-token retry, no legacy fallback");
    assert!(relay3.calls()[0].2, "the one call was the sealed one");
    assert!(!err.to_string().contains("/3/device"), "the error carries the class only: {err}");
    crate::push::reset_relay_state_for_test();
}

/// A restart: the stored `s1` seals the FIRST push (no plaintext to a
/// keyed phone after a routine restart). A fresh install (nothing stored)
/// keeps today's behaviour.
#[test]
fn restart_with_stored_s1_seals_the_first_push() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(61);
    let d = vec![register("rs-key", "apns", "tok-rs-key", true, Some(&k))];
    let event = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "thread-text-x1-preview").expect("e").1;
    // Before the restart: a per-daemon response said s1 (persisted).
    let url = unique_url("restart");
    e2e::note_relay_caps(&url, true);
    assert_eq!(e2e::load_relay_caps(&url).expect("load"), Some(true), "stored");
    // The restart: memory is empty, the stored value stays.
    e2e::forget_relay_caps_memory(&url);
    let relay = Relay::new(true);
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
        .expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].2, "the first push after a restart is sealed");
    assert!(!calls[0].1.to_string().contains("Cortana"), "no plaintext");

    // A fresh install (nothing stored): today's path first.
    let fresh = unique_url("fresh");
    assert_eq!(e2e::load_relay_caps(&fresh).expect("load"), None);
    let relay = Relay::new(true);
    tunnel_cloud(&fresh, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
        .expect("dispatch");
    assert!(!relay.calls()[0].2, "nothing stored: the first push takes today's path");
    assert_eq!(e2e::load_relay_caps(&fresh).expect("load"), Some(true), "learned and stored");
}

/// A stale stored `s1` and a relay that no longer has it: the sealed send
/// is refused, that push goes today's way under sealed-when-supported, the
/// caps are cleared (memory and store), and the next push is legacy from
/// the start. Under sealed-only it never falls back.
#[test]
fn stale_stored_s1_falls_back_once_and_clears_the_caps() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(62);
    let d = vec![register("st-key", "apns", "tok-st-key", true, Some(&k))];
    let event = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "ticket-created").expect("e").1;
    let old_relay_refusal = || GatewayResponse {
        status: 400,
        body: "{\"error\":{\"code\":\"invalid_title\"}}".into(),
        s0: true,
        s1: false,
    };

    // sealed-when-supported: refused, then today's push.
    let url = unique_url("stale");
    e2e::note_relay_caps(&url, true);
    e2e::forget_relay_caps_memory(&url);
    let relay = Relay::new(false);
    relay.script.lock().expect("relay").push_back(old_relay_refusal());
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
        .expect("delivered the legacy way");
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "sealed (refused), then today's push");
    assert!(calls[0].2 && !calls[1].2);
    assert!(calls[1].0.is_tunnel(), "the legacy push still goes per-daemon first");
    assert_eq!(calls[1].1["payload"]["body"], "Cortana needs you on a ticket", "today's payload");
    assert!(!e2e::relay_s1(&url), "caps cleared in memory");
    assert_eq!(e2e::load_relay_caps(&url).expect("load"), Some(false), "and in the store");
    relay.clear();
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
        .expect("legacy");
    assert_eq!(relay.calls().len(), 1);
    assert!(!relay.calls()[0].2, "next push: today's path from the start");

    // A sealed override behaves like sealed-when-supported.
    let url_ov = unique_url("stale-ov");
    e2e::note_relay_caps(&url_ov, true);
    keys::set_override("st-key", Some(DeviceOverride::Sealed)).expect("override");
    let relay = Relay::new(false);
    relay.script.lock().expect("relay").push_back(old_relay_refusal());
    tunnel_cloud(&url_ov, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::Off)
        .expect("delivered");
    assert_eq!(relay.calls().len(), 2);
    keys::set_override("st-key", None).expect("clear");

    // sealed-only: refused, nothing else is sent, caps cleared.
    let url2 = unique_url("stale-only");
    e2e::note_relay_caps(&url2, true);
    e2e::forget_relay_caps_memory(&url2);
    let relay = Relay::new(false);
    relay.script.lock().expect("relay").push_back(old_relay_refusal());
    let err = tunnel_cloud(&url2, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedOnly)
        .expect_err("sealed-only never falls back");
    assert_eq!(relay.calls().len(), 1, "no legacy send under sealed-only");
    assert!(relay.calls()[0].2);
    assert!(err.to_string().contains("invalid_title"), "{err}");
    assert_eq!(e2e::load_relay_caps(&url2).expect("load"), Some(false));
}

/// A nudge that can't fit (a very long server name) is skipped, logged
/// once per device; alerts still go.
#[test]
fn an_oversized_nudge_is_skipped_not_sent_in_clear() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(63);
    let d = vec![register("nz-key", "apns", "tok-nz-key", true, Some(&k))];
    let url = unique_url("nudge-long");
    e2e::note_relay_caps(&url, true);
    let relay = Relay::new(true);
    let long = "a".repeat(63);
    let cloud = K2Cloud::with_config(
        GatewayConfig {
            url: url.clone(),
            tunnel: Some(("tunnel-tok".into(), long.clone())),
            shared_token: None,
            subdomain: long,
        },
        relay.clone(),
    );
    let badge = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "badge").expect("b").1;
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let bk = legacy_golden::GOLDEN_BADGE_KEY;
    cloud.dispatch_in_mode(&badge, bk, &d, PushE2eMode::SealedWhenSupported).expect("no error");
    assert!(relay.calls().is_empty(), "skipped: no sealed call, and never a plaintext one");
    let ticket = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "ticket-created").expect("t").1;
    cloud.dispatch_in_mode(&ticket, bk, &d, PushE2eMode::SealedWhenSupported).expect("alert");
    assert_eq!(relay.calls().len(), 1);
    assert!(relay.calls()[0].2, "alerts still seal");
}

/// The capability header name and parsing are the relay contract.
#[test]
fn caps_header_contract() {
    assert_eq!(e2e::CAPS_HEADER, "X-K2-Push-Caps");
    assert_eq!(S0_MARKER_HEADER, "X-K2-Push-Gateway");
    assert_eq!(S0_MARKER_VALUE, "s0", "the S0 marker never changes");
    assert!(e2e::caps_has_s1("s1"));
    assert!(e2e::caps_has_s1("s1, s2"));
    assert!(e2e::caps_has_s1("x, S1"));
    assert!(!e2e::caps_has_s1("s0"));
    assert!(!e2e::caps_has_s1(""));
    assert!(!e2e::caps_has_s1("s10"));
    assert_eq!(seal::SealKind::Alert.b64_len(), 2159);
    assert_eq!(seal::SealKind::Nudge.b64_len(), 452);
}

/// The truncation rule (§3): a too-long body is cut with "…" to fit;
/// names are capped at 64 only when the body alone can't make room; the
/// bucket never grows.
#[test]
fn alert_truncation_rule() {
    let ctx = e2e::SealCtx {
        kid: "0011223344556677".into(),
        ctr: u64::MAX,
        ts: i64::MAX,
        stamp: PushStamp { subdomain: "s".repeat(63), instance_id: "f".repeat(16) },
        badge_key: "b".repeat(32),
    };
    let ev = |preview: &str, agent: &str| {
        let (_, e) = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "thread-text-x1-preview").expect("e");
        let PushEvent::ThreadPost(mut t) = e else { unreachable!() };
        t.preview = preview.into();
        t.agent_name = agent.into();
        PushEvent::ThreadPost(t)
    };
    let max = seal::max_json(SealKind::Alert);
    // Short: unchanged.
    let v = e2e::alert_plaintext(&ev("hi", "Cortana"), &ctx).expect("v");
    assert_eq!(e2e::fit_alert(&v).expect("fit"), serde_json::to_vec(&v).expect("json"));
    // 140 chars of 4-byte emoji fits as is.
    let emoji = "🚀".repeat(140);
    let v = e2e::alert_plaintext(&ev(&emoji, "Cortana"), &ctx).expect("v");
    let out: serde_json::Value = serde_json::from_slice(&e2e::fit_alert(&v).expect("fit")).expect("json");
    assert_eq!(out["body"].as_str().expect("b").chars().count(), 140);
    // A Thread preview is already capped at 140: a long one fits as is.
    let long = "word ".repeat(1000);
    let agent70 = "A".repeat(70);
    let v = e2e::alert_plaintext(&ev(&long, &agent70), &ctx).expect("v");
    let out: serde_json::Value = serde_json::from_slice(&e2e::fit_alert(&v).expect("fit")).expect("json");
    assert_eq!(out["title"], agent70, "fits: a 70-char name is not capped");
    // A body far over the bucket (a huge project name): cut with "…",
    // fills the bucket.
    let project = PushEvent::ProjectPost(ProjectPush {
        project_name: "Apollo 🚀 ".repeat(400),
        group_id: "pg-9".into(),
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "7".into(),
        waiting_total: "2".into(),
    });
    let v = e2e::alert_plaintext(&project, &ctx).expect("v");
    let bytes = e2e::fit_alert(&v).expect("fit");
    assert!(bytes.len() <= max && bytes.len() > max - 6, "fills the bucket: {}", bytes.len());
    let out: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert!(out["body"].as_str().expect("b").starts_with("New message in Apollo"));
    assert!(out["body"].as_str().expect("b").ends_with('…'));
    assert_eq!(out["title"], "K2");
    assert_eq!(out["data"]["groupId"], "pg-9", "ids are never cut");
    let _ = &long;
    // A huge name too: capped at 64 (63 + "…").
    let agent2k = "N".repeat(2000);
    let v = e2e::alert_plaintext(&ev(&long, &agent2k), &ctx).expect("v");
    let out: serde_json::Value = serde_json::from_slice(&e2e::fit_alert(&v).expect("fit")).expect("json");
    assert_eq!(out["title"].as_str().expect("t").chars().count(), 64);
    assert_eq!(out["data"]["agentName"].as_str().expect("a").chars().count(), 64);
    assert!(out["data"].get("avatarRev").is_some(), "avatarRev dropped only as the last step");
    // An id that can't fit at all: an error, never a bigger bucket.
    let mut v = e2e::alert_plaintext(&ev("x", "C"), &ctx).expect("v");
    v["data"]["conversationId"] = "c".repeat(2000).into();
    assert!(matches!(e2e::fit_alert(&v), Err(seal::SealError::TooLarge { .. })));
}

// ── Phone-side replay check (a test helper, as the phones implement it) ──

/// §3 Replay, phone side: per key, the last 256 accepted `ctr` values. A
/// push is fresh if its `ctr` isn't in the set, the set isn't full or the
/// `ctr` is above the set's minimum, and `ts` is within 30 days and not
/// more than 1 day ahead.
struct ReplayGuard {
    seen: std::collections::BTreeSet<u64>,
}

impl ReplayGuard {
    const CAP: usize = 256;

    fn new() -> Self {
        Self { seen: Default::default() }
    }

    fn accept(&mut self, ctr: u64, ts: i64, now: i64) -> bool {
        if ts < now - 30 * 86_400 || ts > now + 86_400 {
            return false;
        }
        if self.seen.contains(&ctr) {
            return false;
        }
        if self.seen.len() >= Self::CAP {
            let min = *self.seen.iter().next().expect("non-empty");
            if ctr <= min {
                return false;
            }
            self.seen.remove(&min);
        }
        self.seen.insert(ctr);
        true
    }
}

#[test]
fn replayed_pushes_are_rejected_on_the_open_side() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (sk, k) = phone_key(51);
    let d = vec![register("rp-key", "apns", "tok-rp-key", true, Some(&k))];
    let url = unique_url("replay");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true);
    let cloud = tunnel_cloud(&url, relay.clone());
    let event = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "ticket-commented").expect("t").1;
    let mut blobs = Vec::new();
    for _ in 0..3 {
        relay.clear();
        cloud.dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported).expect("dispatch");
        blobs.push(relay.calls()[0].1["devices"][0]["e2e"].as_str().expect("e2e").to_string());
    }
    let open = |b: &str| -> (u64, i64) {
        let v: serde_json::Value = serde_json::from_slice(&seal::open(SealKind::Alert, &sk, &k.psk, b).expect("open")).expect("json");
        (v["ctr"].as_u64().expect("ctr"), v["ts"].as_i64().expect("ts"))
    };
    let now = unix_now();
    let mut guard = ReplayGuard::new();
    for b in &blobs {
        let (ctr, ts) = open(b);
        assert!(guard.accept(ctr, ts, now), "a fresh push is accepted");
    }
    let (ctr0, ts0) = open(&blobs[0]);
    assert!(!guard.accept(ctr0, ts0, now), "a replayed push is rejected");
    assert!(!guard.accept(ctr0 + 10_000_000, now - 31 * 86_400, now), "older than 30 days");
    assert!(!guard.accept(ctr0 + 10_000_001, now + 2 * 86_400, now), "more than a day ahead");
    // A full set: anything at or below its minimum is stale.
    let mut g = ReplayGuard::new();
    for c in 1000..1256u64 {
        assert!(g.accept(c, now, now));
    }
    assert!(!g.accept(999, now, now), "below the window minimum");
    assert!(g.accept(5000, now, now));
    assert!(!g.accept(1000, now, now), "evicted minimum can't come back");
}

// ── `msg`: the post itself, sealed alerts only (follow-up 2026-10-21) ────

const MSG_CANARY: &str = "FULLTEXT-CANARY-31337";

fn msg_thread(text: &str) -> PushEvent {
    let (_, e) = legacy_golden::golden_events().into_iter().find(|(n, _)| n == "thread-text-x1-preview").expect("e");
    let PushEvent::ThreadPost(mut t) = e else { unreachable!() };
    t.msg = Some(ThreadMsg::new("item-9", t.seq, &t.conversation_id, "cortana", 1_760_960_000, ThreadMsgKind::Text, text));
    PushEvent::ThreadPost(t)
}

/// A sealed alert carries `msg` with the FULL text, for previews on and
/// off alike (previews decide only the visible body). Today's path, the
/// generic path and the nudge never carry it.
#[test]
fn msg_rides_only_inside_sealed_alerts() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (sk1, k1) = phone_key(71);
    let (sk2, k2) = phone_key(72);
    let keyed = vec![
        register("msg-on", "apns", "tok-msg-on", true, Some(&k1)),
        register("msg-off", "fcm", "tok-msg-off", false, Some(&k2)),
    ];
    let unkeyed = vec![register("msg-old", "apns", "tok-msg-old", true, None)];
    let text = format!("{MSG_CANARY}: the whole message, well past the 140-character preview. ").repeat(4);
    let event = msg_thread(&text);
    let url = unique_url("msg");
    e2e::note_relay_caps(&url, true);
    let relay = Relay::new(true);
    let cloud = tunnel_cloud(&url, relay.clone());

    // Sealed: both phones get msg.text in full; the previews-off phone's
    // visible body stays generic.
    cloud.dispatch_in_mode(&event, "bk", &keyed, PushE2eMode::SealedWhenSupported).expect("sealed");
    let calls = relay.calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].1.to_string().contains(MSG_CANARY), "never in clear");
    let open = |i: usize, sk: &[u8], psk: &[u8]| -> serde_json::Value {
        let b = calls[0].1["devices"][i]["e2e"].as_str().expect("e2e");
        assert_eq!(b.len(), 2159, "the blob length never changes");
        serde_json::from_slice(&seal::open(SealKind::Alert, sk, psk, b).expect("open")).expect("json")
    };
    let on = open(0, &sk1, &k1.psk);
    let off = open(1, &sk2, &k2.psk);
    for v in [&on, &off] {
        assert_eq!(v["msg"]["text"], text.as_str(), "full text, whatever the previews");
        assert_eq!(v["msg"]["truncated"], false);
        assert_eq!(v["msg"]["id"], "item-9");
        assert_eq!(v["msg"]["seq"], 418);
        assert_eq!(v["msg"]["conversationId"], "c-77");
        assert_eq!(v["msg"]["from"], "cortana");
        assert_eq!(v["msg"]["at"], 1_760_960_000);
        assert_eq!(v["msg"]["kind"], "text");
    }
    assert_eq!(off["body"], "New message in Thread", "previews off: the visible body hides it");
    assert!(!off["body"].as_str().expect("b").contains(MSG_CANARY));
    assert!(!off["title"].as_str().expect("t").contains(MSG_CANARY));
    assert!(!off["data"].to_string().contains(MSG_CANARY), "data never carries it");

    // Today's path (unkeyed, and keyed with the switch off): no msg at all.
    for (devices, mode) in [(&unkeyed, PushE2eMode::SealedWhenSupported), (&keyed, PushE2eMode::Off)] {
        relay.clear();
        cloud.dispatch_in_mode(&event, "bk", devices, mode).expect("legacy");
        for (_, body, sealed) in relay.calls() {
            assert!(!sealed);
            let wire = body.to_string();
            assert!(!wire.contains(MSG_CANARY) && !wire.contains("\"msg\"") && !wire.contains("item-9"), "{wire}");
        }
    }
    // And the legacy payload itself is unchanged by msg.
    let without = match &event {
        PushEvent::ThreadPost(t) => PushEvent::ThreadPost(ThreadPush { msg: None, ..t.clone() }),
        _ => unreachable!(),
    };
    let stamp = PushStamp { subdomain: "rosson".into(), instance_id: "x".into() };
    assert_eq!(K2Cloud::payload(&event, &stamp, "bk"), K2Cloud::payload(&without, &stamp, "bk"));
    // Generic (sealed-only, unkeyed): no msg.
    relay.clear();
    cloud.dispatch_in_mode(&event, "bk", &unkeyed, PushE2eMode::SealedOnly).expect("generic");
    let wire = relay.calls()[0].1.to_string();
    assert!(!wire.contains(MSG_CANARY) && !wire.contains("\"msg\""), "{wire}");
    // The nudge plaintext never has msg (a badge push has no post).
    let ctx = e2e::SealCtx { kid: "k".into(), ctr: 1, ts: 1, stamp: PushStamp::default(), badge_key: "b".into() };
    let badge = BadgePush { principal: "o".into(), device_usernames: vec![], unread_total: "1".into(), waiting_total: "0".into() };
    assert!(e2e::nudge_plaintext(&badge, &ctx).get("msg").is_none());
}

/// A text too long for the bucket is cut at a character boundary with
/// `truncated: true`, filling the bucket; the visible body is untouched;
/// a text that fits exactly is not marked truncated.
#[test]
fn msg_text_is_cut_at_the_bucket_edge() {
    let ctx = e2e::SealCtx {
        kid: "0011223344556677".into(),
        ctr: 1_760_960_000_123,
        ts: 1_760_960_000,
        stamp: PushStamp { subdomain: "rosson".into(), instance_id: "9f2c0a1b3d4e5f60".into() },
        badge_key: "0123456789abcdef0123456789abcdef".into(),
    };
    let max = seal::max_json(SealKind::Alert);
    let long = "Long message 🚀 with emoji and words. ".repeat(80);
    let event = msg_thread(&long);
    let v = e2e::alert_plaintext(&event, &ctx).expect("v");
    let bytes = e2e::fit_alert(&v).expect("fit");
    assert!(bytes.len() <= max && bytes.len() > max - 6, "fills the bucket: {}", bytes.len());
    let out: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(out["msg"]["truncated"], true);
    let cut = out["msg"]["text"].as_str().expect("text");
    assert!(!cut.is_empty() && long.starts_with(cut), "a prefix, cut at a character boundary");
    assert_eq!(out["body"], v["body"], "the visible body is not cut first");
    assert_eq!(out["title"], v["title"]);
    // Sealed, the blob is exactly 2159.
    let (_, pk) = seal::derive_keypair(&[3; 32]);
    assert_eq!(seal::seal(SealKind::Alert, &pk, &[4; 32], &bytes).expect("seal").len(), 2159);
    // A text at the edge (the cut less one character, since `false` is a
    // byte longer than `true`): fits whole, not truncated.
    let mut edge: Vec<char> = cut.chars().collect();
    edge.pop();
    let edge: String = edge.into_iter().collect();
    let v = e2e::alert_plaintext(&msg_thread(&edge), &ctx).expect("v");
    let bytes = e2e::fit_alert(&v).expect("fit");
    assert!(bytes.len() > max - 12, "right at the edge: {}", bytes.len());
    let out: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(out["msg"]["text"], edge.as_str());
    assert_eq!(out["msg"]["truncated"], false, "fits: not truncated");
    // The hold cap (and a secret never carries text).
    let held = ThreadMsg::new("i", 1, "c", "f", 1, ThreadMsgKind::Text, &"x".repeat(5000));
    assert_eq!(held.text.chars().count(), ThreadMsg::HOLD_MAX_CHARS);
    assert!(held.truncated);
    let secret = ThreadMsg::new("i", 1, "c", "f", 1, ThreadMsgKind::Secret, "hunter2");
    assert_eq!(secret.text, "");
    assert!(!secret.truncated);
}

// ── Attempt log (0.46.3, the phone team's e2e push tests) ─────────────

/// A device id unique to this run whose first 8 characters are too, so
/// the process-wide ring can be filtered to one test's records.
fn ring_device(label: &str) -> String {
    format!("{}-{label}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

fn golden(name: &str) -> PushEvent {
    legacy_golden::golden_events().into_iter().find(|(n, _)| n == name).expect("golden case").1
}

/// A sealed push logs device (8 chars), kind, lane, the blob's bytes, the
/// key fingerprint, the relay status, the `s1` path, the relay's markers,
/// the per-device outcome and a latency.
#[test]
fn attempt_line_for_a_sealed_push() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(41);
    let id = ring_device("sealed");
    let dev = register(&id, "apns", "tok-attempt-sealed", true, Some(&k));
    let url = unique_url("attempt-sealed");
    let relay = Relay::new(true);
    relay.script.lock().expect("relay").push_back(GatewayResponse {
        status: 200,
        body: r#"{"ok":true,"results":[{"token":"tok-attempt-sealed","sent":true,"dead":false}]}"#.into(),
        s0: true,
        s1: true,
    });
    e2e::note_relay_caps(&url, true);
    let event = golden("thread-text-x1-preview");
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&event, "bk", std::slice::from_ref(&dev), PushE2eMode::SealedWhenSupported)
        .expect("dispatch");
    let blob_len = relay.calls()[0].1["devices"][0]["e2e"].as_str().expect("e2e").len();
    let recs = attempts::recent(Some(&id));
    assert_eq!(recs.len(), 1, "one attempt for one device: {recs:?}");
    let a = &recs[0];
    assert_eq!(a.device, id[..8]);
    assert_eq!(a.kind, "thread_message");
    assert_eq!(a.lane, "sealed");
    assert_eq!(a.bytes, blob_len);
    assert_eq!(a.bytes, 2159, "the padded alert blob");
    assert_eq!(a.key.as_deref(), Some(&k.key_id[..8]));
    assert_eq!(a.status, Some(200));
    assert_eq!(a.error, None);
    assert_eq!(a.path, "s1");
    assert_eq!(a.relay, "s0+s1");
    assert_eq!(a.vendor, Some("sent"));
    let want = format!(
        "[push] attempt device={} kind=thread_message lane=sealed bytes=2159 key={} status=200 path=s1 relay=s0+s1 vendor=sent ms={}",
        &id[..8],
        &k.key_id[..8],
        a.ms
    );
    assert_eq!(a.line(), want);

    // The badge on the sealed lane is a nudge.
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&golden("badge"), "bk", &[dev], PushE2eMode::SealedWhenSupported)
        .expect("nudge");
    let recs = attempts::recent(Some(&id));
    assert_eq!(recs.len(), 2);
    assert_eq!((recs[1].kind, recs[1].lane, recs[1].bytes), ("nudge", "sealed", 452));
}

/// A legacy push logs the payload's bytes and no key; a relay refusal
/// logs its status and error code; a dead token says so; a push held back
/// by a fresh refusal says nothing went out.
#[test]
fn attempt_line_for_a_legacy_push_and_a_refusal() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    super::reset_relay_state_for_test();
    let (_, k) = phone_key(42);
    let id = ring_device("legacy");
    // Keyed, but the switch is off: today's path.
    let dev = register(&id, "fcm", "tok-attempt-legacy", false, Some(&k));
    let url = unique_url("attempt-legacy");
    let relay = Relay::new(true);
    relay.script.lock().expect("relay").push_back(GatewayResponse {
        status: 200,
        body: r#"{"ok":true,"results":[{"token":"tok-attempt-legacy","sent":false,"dead":true,"error":"fcm 404: https://fcm.example/x"}]}"#.into(),
        s0: true,
        s1: false,
    });
    e2e::note_relay_caps(&url, true);
    let event = golden("thread-text-x1-preview");
    tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, "bk", &[dev], PushE2eMode::Off).expect("dispatch");
    let payload_len = relay.calls()[0].1["payload"].to_string().len();
    let recs = attempts::recent(Some(&id));
    assert_eq!(recs.len(), 1, "{recs:?}");
    let a = &recs[0];
    assert_eq!((a.kind, a.lane, a.path, a.relay), ("thread_message", "legacy", "s0", "s0"));
    assert_eq!(a.bytes, payload_len);
    assert_eq!(a.key, None, "no fingerprint off the sealed lane");
    assert_eq!(a.status, Some(200));
    assert_eq!(a.vendor, Some("dead"));
    let want = format!(
        "[push] attempt device={} kind=thread_message lane=legacy bytes={payload_len} status=200 path=s0 relay=s0 vendor=dead ms={}",
        &id[..8],
        a.ms
    );
    assert_eq!(a.line(), want);

    // A refusal (S0-marked 403): status + the relay's error code.
    let id2 = ring_device("refused");
    let dev2 = register(&id2, "apns", "tok-attempt-refused", true, None);
    let relay = Relay::new(false);
    relay.script.lock().expect("relay").push_back(GatewayResponse {
        status: 403,
        body: r#"{"error":{"code":"subdomain_not_owned"}}"#.into(),
        s0: true,
        s1: false,
    });
    let sent = tunnel_cloud(&unique_url("attempt-refused"), relay)
        .dispatch_in_mode(&event, "bk", std::slice::from_ref(&dev2), PushE2eMode::Off);
    assert!(sent.is_err());
    let a = attempts::recent(Some(&id2)).pop().expect("record");
    assert_eq!((a.status, a.error.as_deref(), a.path, a.vendor), (Some(403), Some("subdomain_not_owned"), "s0", None));
    assert!(a.line().contains(" status=403 error=subdomain_not_owned path=s0 relay=s0 ms="), "{}", a.line());

    // While that rejection is fresh nothing goes out, and the line says so.
    let relay = Relay::new(false);
    let held = tunnel_cloud(&unique_url("attempt-refused-2"), relay.clone()).dispatch_in_mode(&event, "bk", &[dev2], PushE2eMode::Off);
    assert!(held.is_err());
    assert!(relay.calls().is_empty());
    let a = attempts::recent(Some(&id2)).pop().expect("record");
    assert_eq!((a.status, a.error.as_deref(), a.path), (None, Some("not_sent:rejected_recently"), "none"));
    assert!(a.line().contains(" status=- error=not_sent:rejected_recently path=none relay=- ms="), "{}", a.line());
    super::reset_relay_state_for_test();
}

/// Never content: for every canary event, on every lane, with previews on,
/// neither a line nor a ring record carries the preview, the title, names,
/// ids, the badge key, a device token, a relay credential or the vendor's
/// error text.
#[test]
fn attempt_lines_and_ring_carry_no_content_or_tokens() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    super::reset_relay_state_for_test();
    let (_, k) = phone_key(43);
    let keyed_id = ring_device("noleak-k");
    let old_id = ring_device("noleak-o");
    let keyed = register(&keyed_id, "apns", "tok-ATTEMPT-CANARY-keyed-0123456789", true, Some(&k));
    let old = register(&old_id, "fcm", "tok-ATTEMPT-CANARY-old-0123456789", true, None);
    let mut events = canary_events();
    events.push(PushEvent::ThreadPost(ThreadPush {
        agent_name: "TitleCanaryAgent".into(),
        conversation_id: "conv-canary-88".into(),
        addr: "addr-canary/x".into(),
        seq: 1,
        project_id: "proj-canary-2".into(),
        msg_kind: ThreadMsgKind::Text,
        count: 1,
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "1".into(),
        waiting_total: "0".into(),
        avatar_rev: String::new(),
        agent_color: String::new(),
        preview: "ZEBRA-PREVIEW-distinctive-text".into(),
        msg: Some(ThreadMsg::new(
            "item-canary-9",
            1,
            "conv-canary-88",
            "addr-canary/x",
            1,
            ThreadMsgKind::Text,
            "ZEBRA-PREVIEW-distinctive-text",
        )),
        budget: ThreadBudget::Thread,
    }));
    let mut recs = Vec::new();
    for event in &events {
        for mode in [PushE2eMode::Off, PushE2eMode::SealedWhenSupported, PushE2eMode::SealedOnly] {
            let url = unique_url("attempt-noleak");
            let relay = Relay::new(true);
            relay.script.lock().expect("relay").push_back(GatewayResponse {
                status: 200,
                body: r#"{"ok":true,"results":[{"token":"tok-ATTEMPT-CANARY-keyed-0123456789","sent":true,"dead":false},{"token":"tok-ATTEMPT-CANARY-old-0123456789","sent":false,"dead":false,"error":"apns 400: ZEBRA-PREVIEW-distinctive-text"}]}"#.into(),
                s0: true,
                s1: true,
            });
            e2e::note_relay_caps(&url, true);
            tunnel_cloud(&url, relay)
                .dispatch_in_mode(event, "0123456789abcdef0123456789abcdef", &[keyed.clone(), old.clone()], mode)
                .expect("dispatch");
            // The ring is process-wide (other tests write to it too), so
            // each push's records are read right after it.
            for id in [&keyed_id, &old_id] {
                let mine = attempts::recent(Some(id));
                let last = mine.last().unwrap_or_else(|| panic!("no attempt for {id}: {event:?} {mode:?}"));
                recs.push(last.clone());
            }
        }
    }
    assert_eq!(recs.len(), events.len() * 3 * 2);
    let lanes: std::collections::BTreeSet<&str> = recs.iter().map(|r| r.lane).collect();
    assert_eq!(lanes.into_iter().collect::<Vec<_>>(), vec!["generic", "legacy", "sealed"], "every lane exercised");
    let mut forbidden: Vec<&str> = CANARIES.to_vec();
    forbidden.extend([
        "ZEBRA",
        "distinctive",
        "TitleCanaryAgent",
        "conv-canary-88",
        "item-canary-9",
        "tok-",
        "ATTEMPT-CANARY",
        "0123456789abcdef0123456789abcdef",
        "tunnel-tok",
        "shared-tok",
        "New message",
        "noleak",
    ]);
    for r in &recs {
        let line = r.line();
        let json = serde_json::to_string(r).expect("json");
        for f in &forbidden {
            assert!(!line.contains(f), "line carries {f:?}: {line}");
            assert!(!json.contains(f), "ring record carries {f:?}: {json}");
        }
        assert_eq!(r.device.chars().count(), 8);
        // The ring's JSON is exactly the documented fields.
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        let mut keys: Vec<&String> = v.as_object().expect("obj").keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["atMs", "bytes", "device", "error", "key", "kind", "lane", "ms", "path", "relay", "status", "vendor"]);
    }
    super::reset_relay_state_for_test();
}
