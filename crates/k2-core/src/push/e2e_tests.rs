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
            s2: false,
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
    (sk, NewKey { key_id: seal::key_id(&pk), public_key: pk, psk, v: 1 })
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
    "out_canary7q",
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
            stale: Default::default(),
        }),
        // Sew S30: an agent's chat post waits for approval (no body ever).
        PushEvent::ChatApprovalRequested {
            agent_name: "CanaryAgentZx".into(),
            outbox_id: "out_canary7q".into(),
            approvers: vec!["owner".into()],
        },
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
                e2e::note_relay_caps(&url, true, false);
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
            e2e::note_relay_caps(&url, true, false);
            tunnel_cloud(&url, relay.clone())
                .dispatch_in_mode(&event, "0123456789abcdef0123456789abcdef", &unkeyed, PushE2eMode::SealedOnly)
                .expect("generic");
            for (_, body, sealed) in relay.calls() {
                assert!(!sealed);
                let wire = body.to_string();
                if matches!(event, PushEvent::Badge(_)) {
                    // This badge has no `stale`, so it's today's payload.
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
    e2e::note_relay_caps(&url, true, false);
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

/// push-e2e-v2 (0.46.7, K2): a `v: 2` phone on an `s2` relay gets a
/// format-2 alert (`v: "2"`, a 3951-character blob) that carries a
/// 2,000-character reply WHOLE; a `v: 1` phone of the same person gets
/// today's format in its own relay call; a relay without `s2` keeps the
/// `v: 2` phone on format 1.
#[test]
fn a_v2_phone_on_an_s2_relay_gets_the_bigger_bucket_and_the_whole_reply() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (sk2, mut k2) = phone_key(41);
    k2.v = 2;
    let (sk1, k1) = phone_key(42);
    let devices = vec![
        register("v2-new", "apns", "tok-v2-new", true, Some(&k2)),
        register("v2-old", "fcm", "tok-v2-old", true, Some(&k1)),
    ];
    let reply: String = "All 14 checks passed; the release notes are drafted. ".repeat(38);
    assert!(reply.len() > 1900 && reply.len() < 2100, "{}", reply.len());
    let event = msg_thread(&reply);

    let url = unique_url("v2");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, true);
    let cloud = tunnel_cloud(&url, relay.clone());
    cloud.dispatch_in_mode(&event, "bk-test", &devices, PushE2eMode::SealedWhenSupported).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "one relay call per format");
    let by_v = |v: &str| calls.iter().find(|c| c.1["payload"]["data"]["v"] == v).expect("call").1.clone();
    let v2 = by_v("2");
    let v1 = by_v("1");
    assert_eq!(v2["payload"], serde_json::json!({ "data": { "kind": "e2e", "v": "2" } }));
    let blob2 = v2["devices"][0]["e2e"].as_str().expect("e2e");
    assert_eq!(v2["devices"].as_array().expect("devs").len(), 1);
    assert_eq!(blob2.len(), seal::Format::V2.b64_len(SealKind::Alert));
    assert_eq!(blob2.len(), 3951);
    let (format, json) = seal::open_format(SealKind::Alert, &sk2, &k2.psk, blob2).expect("open v2");
    assert_eq!(format, seal::Format::V2);
    let pt: serde_json::Value = serde_json::from_slice(&json).expect("json");
    assert_eq!(pt["msg"]["text"], reply.as_str(), "the whole reply rides");
    assert_eq!(pt["msg"]["truncated"], false);
    // The v1 phone: today's 2159 blob, the reply cut.
    let blob1 = v1["devices"][0]["e2e"].as_str().expect("e2e");
    assert_eq!(blob1.len(), 2159);
    let pt1: serde_json::Value =
        serde_json::from_slice(&seal::open(SealKind::Alert, &sk1, &k1.psk, blob1).expect("open v1")).expect("json");
    assert_eq!(pt1["msg"]["truncated"], true, "1536 bytes can't hold it");

    // A relay that never said s2: the v2 phone stays on format 1.
    let url = unique_url("v2-no-s2");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, false);
    let cloud = tunnel_cloud(&url, relay.clone());
    cloud.dispatch_in_mode(&event, "bk-test", &devices[..1], PushE2eMode::SealedWhenSupported).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["payload"]["data"]["v"], "1");
    assert_eq!(calls[0].1["devices"][0]["e2e"].as_str().expect("e2e").len(), 2159);
    // The caps are stored with s2 and survive a restart.
    assert_eq!(e2e::load_relay_caps_full(&url).expect("load"), Some(e2e::RelayCaps { s1: true, s2: false }));
}

/// push-content-available-v1 (0.46.7, K3): the stale hint names chats by
/// id and seq only. A format-2 nudge seals it (no plaintext anywhere on
/// the wire); a format-1 nudge never carries it; today's badge push
/// carries `stale` in clear and nothing else new; a hint too big for the
/// bucket loses pairs (`staleMore`), never the totals.
#[test]
fn the_stale_hint_carries_ids_and_seqs_only() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let stale = StaleHint::of(vec![("conv-canary-77".into(), 42), ("c-9".into(), 7)]);
    assert_eq!(stale.wire(), "conv-canary-77:42,c-9:7");
    let badge = PushEvent::Badge(BadgePush {
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "3".into(),
        waiting_total: "1".into(),
        stale: stale.clone(),
    });
    let (sk2, mut k2) = phone_key(51);
    k2.v = 2;
    let (sk1, k1) = phone_key(52);
    let devices = vec![
        register("st-v2", "apns", "tok-st-v2", true, Some(&k2)),
        register("st-v1", "apns", "tok-st-v1", true, Some(&k1)),
        register("st-legacy", "fcm", "tok-st-legacy", true, None),
    ];
    let url = unique_url("stale");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, true);
    let cloud = tunnel_cloud(&url, relay.clone());
    cloud.dispatch_in_mode(&badge, "bk-test", &devices, PushE2eMode::SealedWhenSupported).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 3, "legacy, v1 nudge, v2 nudge");
    for (_, body, sealed) in &calls {
        if *sealed {
            // Sealed calls: no id, seq or count anywhere outside the blob.
            let mut outside = body.clone();
            for d in outside["devices"].as_array_mut().expect("devs") {
                d["e2e"] = "".into();
            }
            let text = outside.to_string();
            assert!(!text.contains("conv-canary-77") && !text.contains(":42"), "{text}");
        }
    }
    let nudge = |v: &str| calls.iter().find(|c| c.2 && c.1["payload"]["data"]["v"] == v).expect("nudge").1.clone();
    let n2 = nudge("2");
    let blob2 = n2["devices"][0]["e2e"].as_str().expect("e2e");
    assert_eq!(blob2.len(), 1476);
    let pt2: serde_json::Value =
        serde_json::from_slice(&seal::open(SealKind::Nudge, &sk2, &k2.psk, blob2).expect("open")).expect("json");
    assert_eq!(
        pt2["data"],
        serde_json::json!({ "unreadTotal": "3", "waitingTotal": "1", "stale": "conv-canary-77:42,c-9:7" }),
        "ids and seqs only"
    );
    let n1 = nudge("1");
    let pt1: serde_json::Value = serde_json::from_slice(
        &seal::open(SealKind::Nudge, &sk1, &k1.psk, n1["devices"][0]["e2e"].as_str().expect("e2e")).expect("open"),
    )
    .expect("json");
    assert_eq!(pt1["data"], serde_json::json!({ "unreadTotal": "3", "waitingTotal": "1" }), "format 1: never ids");
    let legacy = calls.iter().find(|c| !c.2).expect("legacy").1.clone();
    let data = legacy["payload"]["data"].clone();
    assert_eq!(data["stale"], "conv-canary-77:42,c-9:7");
    assert!(data.get("staleMore").is_none());
    assert!(legacy["payload"].get("title").is_none() && legacy["payload"].get("body").is_none(), "silent");
    for (k, v) in data.as_object().expect("data") {
        assert!(v.is_string(), "{k}: every data value is a string");
        assert!(
            ["kind", "unreadTotal", "waitingTotal", "badgeKey", "subdomain", "instanceId", "stale"].contains(&k.as_str()),
            "unexpected key {k}"
        );
    }

    // Too many pairs for the bucket: pairs go from the end, the totals stay.
    let many: Vec<(String, i64)> = (0..8).map(|i| (format!("{i}-{}", "x".repeat(120)), 1_000_000 + i)).collect();
    let b = BadgePush {
        principal: "owner".into(),
        device_usernames: vec![],
        unread_total: "9".into(),
        waiting_total: "0".into(),
        stale: StaleHint::of(many),
    };
    let ctx = e2e::SealCtx { kid: "k".into(), ctr: 1, ts: 1, stamp: PushStamp::default(), badge_key: "b".into() };
    let fitted = e2e::fit_nudge_in(&e2e::nudge_plaintext_in(&b, &ctx, seal::Format::V2), seal::Format::V2).expect("fits");
    assert!(fitted.len() <= seal::Format::V2.max_json(SealKind::Nudge));
    let out: serde_json::Value = serde_json::from_slice(&fitted).expect("json");
    assert_eq!(out["data"]["unreadTotal"], "9");
    assert_eq!(out["data"]["staleMore"], "1");
    let kept = out["data"]["stale"].as_str().expect("some pairs kept").split(',').count();
    assert!((1..8).contains(&kept), "{kept}");
    assert!(out["data"]["stale"].as_str().expect("stale").starts_with("0-"), "newest first are kept");
    // More than 8 pairs: 8 kept, `more` set.
    let nine = StaleHint::of((0..9).map(|i| (format!("c{i}"), i)).collect());
    assert_eq!(nine.pairs.len(), STALE_MAX_PAIRS);
    assert!(nine.more);
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
        e2e::note_relay_caps(&url, caps, false);
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
        e2e::note_relay_caps(&url, true, false);
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
    e2e::note_relay_caps(&url2, true, false);
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
    e2e::note_relay_caps(&url3, true, false);
    let relay3 = Relay::new(true);
    relay3.script.lock().expect("relay").push_back(GatewayResponse {
        status: 401,
        body: "{\"error\":{\"code\":\"secret-detail-canary\"},\"results\":[{\"error\":\"/3/device/tok\"}]}".into(),
        s0: false,
        s1: false,
        s2: false,
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
    e2e::note_relay_caps(&url, true, false);
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
/// is refused, that push goes with no content under sealed-when-supported,
/// the caps are cleared (memory and store), and the next push is legacy
/// from the start. Under sealed-only nothing else goes.
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
        s2: false,
    };

    // sealed-when-supported: refused, then a push with no content.
    let url = unique_url("stale");
    e2e::note_relay_caps(&url, true, false);
    e2e::forget_relay_caps_memory(&url);
    let relay = Relay::new(false);
    relay.script.lock().expect("relay").push_back(old_relay_refusal());
    tunnel_cloud(&url, relay.clone())
        .dispatch_in_mode(&event, "bk", &d, PushE2eMode::SealedWhenSupported)
        .expect("delivered without content");
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "sealed (refused), then the push with no content");
    assert!(calls[0].2 && !calls[1].2);
    assert!(calls[1].0.is_tunnel(), "it still goes per-daemon first");
    assert_eq!(calls[1].1["payload"]["body"], "New message", "never today's payload");
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
    e2e::note_relay_caps(&url_ov, true, false);
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
    e2e::note_relay_caps(&url2, true, false);
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
    e2e::note_relay_caps(&url, true, false);
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
    e2e::note_relay_caps(&url, true, false);
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
    e2e::note_relay_caps(&url, true, false);
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
    let badge = BadgePush { principal: "o".into(), device_usernames: vec![], unread_total: "1".into(), waiting_total: "0".into(), stale: Default::default() };
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
        s2: false,
    });
    e2e::note_relay_caps(&url, true, false);
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
        s2: false,
    });
    e2e::note_relay_caps(&url, true, false);
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
        s2: false,
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
                s2: false,
            });
            e2e::note_relay_caps(&url, true, false);
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

// ── A refused sealed send never goes in the clear ─────────────────────────

const LEAK_BK: &str = "0123456789abcdef0123456789abcdef";

/// Every relay answer that isn't the old relay's 400: (status, S0 marker,
/// `s1` in the caps). Unmarked ones are what a proxy in front of the relay
/// says.
const NOT_400: &[(u16, bool, bool)] = &[
    (429, true, true),
    (429, false, false),
    (401, true, true),
    (401, false, false),
    (403, true, true),
    (403, false, false),
    (404, false, false),
    (413, false, false),
    (500, true, true),
    (502, false, false),
    (503, true, true),
    (503, false, false),
    (504, false, false),
];

fn relay_answer(status: u16, s0: bool, s1: bool) -> GatewayResponse {
    GatewayResponse { status, body: "{\"error\":\"refused\"}".into(), s0, s1, s2: false }
}

fn alert_events() -> Vec<PushEvent> {
    canary_events().into_iter().filter(|e| !matches!(e, PushEvent::Badge(_))).collect()
}

/// A Thread alert with a preview and its `msg`: everything a leak would show.
fn canary_thread() -> PushEvent {
    let event = canary_events().into_iter().nth(1).expect("the text alert");
    assert!(event.has_preview(), "{event:?}");
    assert!(matches!(&event, PushEvent::ThreadPost(t) if t.msg.is_some()), "{event:?}");
    event
}

/// An alert that carries the person's totals.
fn has_totals(event: &PushEvent) -> bool {
    matches!(event, PushEvent::ThreadPost(_) | PushEvent::ProjectPost(_))
}

/// A badge whose `stale` names a chat, with more left out.
fn stale_badge() -> BadgePush {
    let mut pairs: Vec<(String, i64)> = vec![("conv-canary-77".into(), 42)];
    pairs.extend((0..8).map(|i| (format!("grp-canary-9-{i}"), i)));
    BadgePush {
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "3".into(),
        waiting_total: "1".into(),
        stale: StaleHint::of(pairs),
    }
}

/// `plain` on the wire holds nothing but the server stamp: no name, id or
/// preview (the stamp is what a phone with no key routes on).
fn assert_no_content(plain: &str, what: &str) {
    for c in CANARIES.iter().filter(|c| **c != "rosson" && **c != "9f2c0a1b3d4e5f60") {
        assert!(!plain.contains(c), "{what}: {c:?} in the clear: {plain}");
    }
}

/// The exact bare badge: what every phone got before `stale` existed.
fn assert_bare_badge(body: &serde_json::Value, what: &str) {
    let wire = body.to_string();
    assert_no_content(&wire, what);
    assert!(!wire.contains("stale"), "{what}: {wire}");
    let bare = PushEvent::Badge(BadgePush { stale: StaleHint::default(), ..stale_badge() });
    let stamp = PushStamp { subdomain: "rosson".into(), instance_id: instance_id() };
    assert_eq!(body["payload"], K2Cloud::payload(&bare, &stamp, LEAK_BK), "{what}: today's badge, no hints");
    let data = body["payload"]["data"].as_object().expect("data");
    let mut keys: Vec<&str> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["badgeKey", "instanceId", "kind", "subdomain", "unreadTotal", "waitingTotal"], "{what}: {wire}");
    assert_eq!(data["kind"], "badge", "{what}");
    assert_eq!(data["unreadTotal"], "3", "{what}");
    assert_eq!(data["waitingTotal"], "1", "{what}");
    assert_eq!(body["payload"].as_object().expect("payload").len(), 1, "{what}: silent, data only: {wire}");
}

/// The exact generic alert: "K2 / New message", no thread or collapse id.
fn assert_generic_alert(body: &serde_json::Value, what: &str) {
    let wire = body.to_string();
    assert_no_content(&wire, what);
    assert_eq!(body["payload"]["title"], "K2", "{what}: {wire}");
    assert_eq!(body["payload"]["body"], "New message", "{what}: {wire}");
    assert!(body["payload"].get("threadId").is_none() && body["payload"].get("collapseId").is_none(), "{what}: {wire}");
    assert!(body["payload"]["data"].get("msg").is_none(), "{what}: {wire}");
}

/// Sends a sealed alert into every answer but the old relay's 400. Nothing
/// plain leaves, ever: no title, preview, name, id, count or stamp. A 429
/// on an alert with totals sends one sealed nudge with the totals alone.
#[test]
fn no_relay_answer_sends_a_sealed_alert_in_the_clear() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (sk, k) = phone_key(71);
    let (_, k2) = phone_key(72);
    let devices = vec![
        register("rf-on", "apns", "tok-rf-on", true, Some(&k)),
        register("rf-off", "fcm", "tok-rf-off", false, Some(&k2)),
    ];
    let mut nudged = 0;
    for &(status, s0, s1) in NOT_400 {
        for event in alert_events() {
            for mode in [PushE2eMode::SealedWhenSupported, PushE2eMode::SealedOnly] {
                let what = format!("HTTP {status} s0={s0} s1={s1} {mode:?} {event:?}");
                super::reset_relay_state_for_test();
                let url = unique_url("refused-alert");
                let relay = Relay::new(true);
                e2e::note_relay_caps(&url, true, false);
                relay.script.lock().expect("relay").push_back(relay_answer(status, s0, s1));
                let sent = tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, &devices, mode);
                assert!(sent.is_err(), "the caller hears the alert was refused: {what}");
                let calls = relay.calls();
                for (_, body, sealed) in &calls {
                    assert!(*sealed, "a plain call went out: {what}: {body}");
                    assert_no_canary(&body.to_string(), &what);
                    assert_eq!(body["payload"]["data"].as_object().expect("data").len(), 2, "{what}: {body}");
                }
                assert_eq!(calls[0].1["payload"]["data"]["kind"], "e2e", "{what}");
                if status == 429 && has_totals(&event) {
                    assert_eq!(calls.len(), 2, "{what}");
                    let nudge = &calls[1].1;
                    assert_eq!(nudge["payload"]["data"]["kind"], "nudge", "{what}");
                    assert_eq!(nudge["devices"].as_array().expect("devices").len(), 2, "both phones: {what}");
                    let mine = nudge["devices"].as_array().expect("devices").iter().find(|d| d["token"] == "tok-rf-on").expect("its entry");
                    let blob = mine["e2e"].as_str().expect("e2e");
                    let pt: serde_json::Value =
                        serde_json::from_slice(&seal::open(SealKind::Nudge, &sk, &k.psk, blob).expect("opens")).expect("json");
                    let data = pt["data"].as_object().expect("data");
                    assert_eq!(data.len(), 2, "totals only, no chat named: {what}: {pt}");
                    assert!(data["unreadTotal"].is_string() && data["waitingTotal"].is_string(), "{what}: {pt}");
                    nudged += 1;
                } else {
                    assert_eq!(calls.len(), 1, "no second call: {what}");
                }
            }
        }
    }
    assert!(nudged > 0, "the 429 path ran");
    super::reset_relay_state_for_test();

    // The relay's sealed window and its nudge window both full: still nothing plain.
    let url = unique_url("refused-both");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, false);
    for _ in 0..2 {
        relay.script.lock().expect("relay").push_back(relay_answer(429, true, true));
    }
    let event = canary_thread();
    assert!(tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, &devices, PushE2eMode::SealedWhenSupported).is_err());
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "the alert, then its nudge, and nothing after");
    assert!(calls.iter().all(|c| c.2), "all sealed");

    // A `sealed` override under a server `off` is held to the same rule.
    keys::set_override("rf-on", Some(DeviceOverride::Sealed)).expect("override");
    let url = unique_url("refused-override");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, false);
    relay.script.lock().expect("relay").push_back(relay_answer(429, true, true));
    let one = std::slice::from_ref(&devices[0]);
    assert!(tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, one, PushE2eMode::Off).is_err());
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls.iter().all(|c| c.2), "all sealed: {calls:?}");
    keys::set_override("rf-on", None).expect("clear");
}

/// A sealed nudge refused with anything but a 400 is dropped: no plain
/// badge, so no chat ids, and no second call of any kind.
#[test]
fn a_refused_sealed_nudge_is_dropped_with_its_chat_ids() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, mut k) = phone_key(73);
    k.v = 2;
    let d = vec![register("rn-v2", "apns", "tok-rn-v2", true, Some(&k))];
    let badge = PushEvent::Badge(stale_badge());
    for &(status, s0, s1) in NOT_400 {
        let what = format!("HTTP {status} s0={s0} s1={s1}");
        super::reset_relay_state_for_test();
        let url = unique_url("refused-nudge");
        let relay = Relay::new(true);
        e2e::note_relay_caps(&url, true, true);
        relay.script.lock().expect("relay").push_back(relay_answer(status, s0, s1));
        let sent = tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&badge, LEAK_BK, &d, PushE2eMode::SealedWhenSupported);
        assert!(sent.is_err(), "{what}");
        let calls = relay.calls();
        assert_eq!(calls.len(), 1, "dropped: {what}: {calls:?}");
        assert!(calls[0].2, "{what}");
        assert_no_canary(&calls[0].1.to_string(), &what);
    }
    super::reset_relay_state_for_test();
}

/// A relay we can't reach at all: no other way is tried.
struct SealedSendsFail(Arc<Relay>);

impl DispatchSender for SealedSendsFail {
    fn post(&self, url: &str, auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.0.post(url, auth, body)
    }
    fn post_sealed(&self, _url: &str, auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.0.calls.lock().expect("relay").push((auth.clone(), body.clone(), true));
        Err(PushError::Transport("connection reset".into()))
    }
}

#[test]
fn a_sealed_send_that_cannot_connect_tries_nothing_else() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (_, k) = phone_key(74);
    let d = vec![register("rt-key", "apns", "tok-rt-key", true, Some(&k))];
    for event in canary_events().into_iter().chain([PushEvent::Badge(stale_badge())]) {
        let url = unique_url("transport");
        let relay = Relay::new(true);
        e2e::note_relay_caps(&url, true, false);
        let cloud = K2Cloud::with_config(
            GatewayConfig {
                url: url.clone(),
                tunnel: Some(("tunnel-tok".into(), "rosson".into())),
                shared_token: Some("shared-tok".into()),
                subdomain: "rosson".into(),
            },
            Arc::new(SealedSendsFail(relay.clone())),
        );
        assert!(cloud.dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).is_err(), "{event:?}");
        let calls = relay.calls();
        assert_eq!(calls.len(), 1, "{event:?}: {calls:?}");
        assert!(calls[0].2, "{event:?}");
        assert!(e2e::relay_s1(&url), "a failed connection says nothing about the relay");
    }
}

/// The old relay's 400 (a stale stored `s1`): sealed-when-supported phones
/// get the push with no content, and sealed-only phones get nothing.
#[test]
fn a_400_sends_only_a_contentless_push() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k) = phone_key(75);
    let (_, mut k2) = phone_key(76);
    k2.v = 2;
    let d = vec![
        register("r4-v1", "apns", "tok-r4-v1", true, Some(&k)),
        register("r4-v2", "apns", "tok-r4-v2", true, Some(&k2)),
    ];
    let old_relay = || GatewayResponse {
        status: 400,
        body: "{\"error\":\"invalid_title\"}".into(),
        s0: true,
        s1: false,
        s2: false,
    };
    let run = |event: &PushEvent, mode: PushE2eMode| {
        let url = unique_url("refused-400");
        let relay = Relay::new(false);
        e2e::note_relay_caps(&url, true, false);
        relay.script.lock().expect("relay").push_back(old_relay());
        let sent = tunnel_cloud(&url, relay.clone()).dispatch_in_mode(event, LEAK_BK, &d, mode);
        (sent, relay.calls())
    };
    for event in alert_events() {
        let (sent, calls) = run(&event, PushE2eMode::SealedWhenSupported);
        sent.expect("the phone still heard something");
        assert_eq!(calls.len(), 2, "{event:?}");
        assert!(calls[0].2 && !calls[1].2, "{event:?}");
        assert!(calls[1].0.is_tunnel(), "{event:?}");
        assert_generic_alert(&calls[1].1, &format!("{event:?}"));
        let (sent, calls) = run(&event, PushE2eMode::SealedOnly);
        assert!(sent.is_err(), "{event:?}");
        assert_eq!(calls.len(), 1, "sealed-only: nothing plain: {event:?}");
    }
    let badge = PushEvent::Badge(stale_badge());
    let (sent, calls) = run(&badge, PushE2eMode::SealedWhenSupported);
    sent.expect("bare badge");
    assert_eq!(calls.len(), 2);
    assert!(calls[0].2 && !calls[1].2);
    assert_bare_badge(&calls[1].1, "refused nudge");
    let (sent, calls) = run(&badge, PushE2eMode::SealedOnly);
    assert!(sent.is_err());
    assert_eq!(calls.len(), 1, "sealed-only: nothing plain");
}

/// `sealed-only`, phone with no key (and a keyed one on a relay without
/// `s1`): the badge carries no chat ids, and is exactly what an old phone
/// has always had, so it keeps working.
#[test]
fn a_sealed_only_phone_with_no_key_gets_a_bare_badge() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k) = phone_key(77);
    let d = vec![
        register("bb-apns", "apns", "tok-bb-apns", true, None),
        register("bb-fcm", "fcm", "tok-bb-fcm", false, None),
        register("bb-keyed", "apns", "tok-bb-keyed", true, Some(&k)),
    ];
    let badge = PushEvent::Badge(stale_badge());
    // Keyless phones next to a keyed one on a sealing relay.
    let url = unique_url("bare");
    let relay = Relay::new(true);
    e2e::note_relay_caps(&url, true, false);
    tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&badge, LEAK_BK, &d, PushE2eMode::SealedOnly).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1].2, "the keyed phone is sealed");
    assert!(!calls[0].2);
    assert_bare_badge(&calls[0].1, "sealed-only, no key");
    let mut tokens: Vec<&str> = calls[0].1["devices"].as_array().expect("d").iter().map(|d| d["token"].as_str().expect("t")).collect();
    tokens.sort_unstable();
    assert_eq!(tokens, vec!["tok-bb-apns", "tok-bb-fcm"]);
    // A relay without `s1`: the keyed phone is on the same lane.
    let url = unique_url("bare-no-s1");
    let relay = Relay::new(false);
    e2e::note_relay_caps(&url, false, false);
    tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&badge, LEAK_BK, &d, PushE2eMode::SealedOnly).expect("dispatch");
    let calls = relay.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(!calls[0].2);
    assert_bare_badge(&calls[0].1, "sealed-only, no s1");
    assert_eq!(calls[0].1["devices"].as_array().expect("d").len(), 3);
}

/// A 429 or 5xx from something in front of the relay carries no caps. It
/// mustn't read as "this relay can't seal", or the next push to a keyed
/// phone goes plain. A marked answer without `s1` still means a rollback.
#[test]
fn an_answer_from_in_front_of_the_relay_does_not_turn_sealing_off() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k) = phone_key(78);
    let keyed = vec![register("cp-key", "apns", "tok-cp-key", true, Some(&k))];
    let old = vec![register("cp-old", "apns", "tok-cp-old", true, None)];
    let event = canary_thread();
    for status in [429, 500, 502, 503, 504] {
        // The sealed call gets it, then the legacy call of a keyless phone.
        for lane_devices in [&keyed, &old] {
            let what = format!("HTTP {status} on {}", lane_devices[0].device_id);
            let url = unique_url("caps");
            e2e::note_relay_caps(&url, true, false);
            let relay = Relay::new(true);
            relay.script.lock().expect("relay").push_back(relay_answer(status, false, false));
            let cloud = tunnel_cloud(&url, relay.clone());
            assert!(cloud.dispatch_in_mode(&event, LEAK_BK, lane_devices, PushE2eMode::SealedWhenSupported).is_err(), "{what}");
            assert!(e2e::relay_s1(&url), "{what}");
            assert_eq!(e2e::load_relay_caps(&url).expect("load"), Some(true), "{what}");
            relay.clear();
            cloud.dispatch_in_mode(&event, LEAK_BK, &keyed, PushE2eMode::SealedWhenSupported).expect("next push");
            let calls = relay.calls();
            assert_eq!(calls.len(), 1, "{what}");
            assert!(calls[0].2, "the next push is still sealed: {what}");
            assert_no_canary(&calls[0].1.to_string(), &what);
        }
    }
    // The relay itself saying no `s1`, on any status, is a rollback.
    for status in [200, 429, 503] {
        let url = unique_url("caps-rollback");
        e2e::note_relay_caps(&url, true, false);
        let relay = Relay::new(false);
        relay.script.lock().expect("relay").push_back(GatewayResponse { body: "{}".into(), ..relay_answer(status, true, false) });
        let sent = tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, &keyed, PushE2eMode::SealedWhenSupported);
        assert_eq!(sent.is_ok(), status == 200, "HTTP {status}");
        assert!(!e2e::relay_s1(&url), "HTTP {status}");
    }
}

/// K2 can't read which phones hold a key (a database error). Nothing with
/// content goes: not knowing must not read as "no key, so plain".
#[test]
fn an_unreadable_key_table_sends_only_contentless_pushes() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k) = phone_key(79);
    let d = vec![register("kt-key", "apns", "tok-kt-key", true, Some(&k))];
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("DROP TABLE push_device_keys", []).expect("drop");
    }
    assert!(keys::states(&["kt-key"]).is_err(), "the read fails");
    for mode in [PushE2eMode::SealedWhenSupported, PushE2eMode::SealedOnly] {
        for event in alert_events() {
            let url = unique_url("key-read");
            e2e::note_relay_caps(&url, true, false);
            let relay = Relay::new(true);
            tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, &d, mode).expect("generic");
            let calls = relay.calls();
            assert_eq!(calls.len(), 1, "{event:?}");
            assert_generic_alert(&calls[0].1, &format!("{mode:?} {event:?}"));
        }
        let url = unique_url("key-read-badge");
        e2e::note_relay_caps(&url, true, false);
        let relay = Relay::new(true);
        tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&PushEvent::Badge(stale_badge()), LEAK_BK, &d, mode).expect("bare");
        let calls = relay.calls();
        assert_eq!(calls.len(), 1);
        assert_bare_badge(&calls[0].1, "key table unreadable");
    }
}

/// The real sender against a real socket on loopback: the request bytes,
/// the real header parsing, and what the daemon does next.
struct LoopbackRelay {
    url: String,
    done: Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<Vec<String>>,
}

impl LoopbackRelay {
    /// Answers each request with the next of `answers`, then a plain 200.
    fn start(answers: Vec<String>) -> Self {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = done.clone();
        let thread = std::thread::spawn(move || {
            let mut answers: std::collections::VecDeque<String> = answers.into();
            let mut seen = Vec::new();
            loop {
                let mut sock = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if stop.load(std::sync::atomic::Ordering::SeqCst) {
                            return seen;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept: {e}"),
                };
                sock.set_nonblocking(false).expect("blocking");
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = sock.read(&mut buf).expect("read");
                    assert!(n > 0, "the request ended early");
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().expect("length"))
                            })
                            .expect("content-length");
                        if raw.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let answer = answers.pop_front().unwrap_or_else(|| http_answer(200, &["X-K2-Push-Gateway: s0", "X-K2-Push-Caps: s1"]));
                sock.write_all(answer.as_bytes()).expect("answer");
                seen.push(String::from_utf8(raw).expect("utf8"));
            }
        });
        Self { url, done, thread }
    }

    /// Stop, and hand back each request, whole, as (head, JSON body).
    fn requests(self) -> Vec<(String, serde_json::Value)> {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        self.thread
            .join()
            .expect("relay thread")
            .into_iter()
            .map(|raw| {
                let (head, body) = raw.split_once("\r\n\r\n").expect("a head and a body");
                (head.to_string(), serde_json::from_str(body).expect("json body"))
            })
            .collect()
    }
}

fn http_answer(status: u16, headers: &[&str]) -> String {
    let body = if status == 200 { "{\"ok\":true,\"results\":[]}" } else { "{\"error\":\"refused\"}" };
    let mut out = format!("HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for h in headers {
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

fn loopback_cloud(url: &str) -> K2Cloud {
    K2Cloud::with_config(
        GatewayConfig {
            url: url.into(),
            tunnel: Some(("tunnel-tok".into(), "rosson".into())),
            shared_token: Some("shared-tok".into()),
            subdomain: "rosson".into(),
        },
        Arc::new(HttpDispatchSender),
    )
}

#[test]
fn over_a_real_socket_a_refused_sealed_alert_shows_nothing() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    set_instance_id(legacy_golden::GOLDEN_INSTANCE);
    let (_, k) = phone_key(80);
    let d = vec![register("sk-key", "apns", "tok-sk-key", true, Some(&k))];
    let event = canary_thread();
    let marked = ["X-K2-Push-Gateway: s0", "X-K2-Push-Caps: s1"];

    // The relay's own 429: the alert, then one sealed nudge. Caps stay.
    let relay = LoopbackRelay::start(vec![http_answer(429, &marked)]);
    let url = relay.url.clone();
    e2e::note_relay_caps(&url, true, false);
    assert!(loopback_cloud(&url).dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).is_err());
    assert!(e2e::relay_s1(&url), "the 429 carried s1");
    let seen = relay.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].1["payload"], serde_json::json!({ "data": { "kind": "e2e", "v": "1" } }));
    assert_eq!(seen[1].1["payload"], serde_json::json!({ "data": { "kind": "nudge", "v": "1" } }));
    for (head, body) in &seen {
        assert!(head.starts_with("POST /push/dispatch "), "{head}");
        assert!(head.to_ascii_lowercase().contains("x-k2-subdomain: rosson"), "the per-daemon path: {head}");
        assert!(!head.contains("shared-tok"), "{head}");
        assert_no_content(head, "headers");
        assert_no_canary(&body.to_string(), "429 body");
    }

    // A proxy's bare 502: dropped, and the next push is still sealed.
    let relay = LoopbackRelay::start(vec![http_answer(502, &[])]);
    let url = relay.url.clone();
    e2e::note_relay_caps(&url, true, false);
    let cloud = loopback_cloud(&url);
    assert!(cloud.dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).is_err());
    cloud.dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).expect("next push");
    let seen = relay.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for (_, body) in &seen {
        assert_eq!(body["payload"], serde_json::json!({ "data": { "kind": "e2e", "v": "1" } }));
        assert_no_canary(&body.to_string(), "502 body");
    }

    // The old relay's 400: then the contentless alert.
    let relay = LoopbackRelay::start(vec![http_answer(400, &["X-K2-Push-Gateway: s0"])]);
    let url = relay.url.clone();
    e2e::note_relay_caps(&url, true, false);
    loopback_cloud(&url).dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).expect("generic");
    let seen = relay.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].1["payload"]["data"]["kind"], "e2e");
    assert_generic_alert(&seen[1].1, "400 body");
    assert_no_content(&seen[1].0, "400 headers");
}

/// K2 Companion's A3: a delivered sealed alert is matched on `kind` inside
/// `data`, not only the blob's top-level `kind`. On every alert kind, in
/// both formats, whole or cut down to fit, through the real dispatch.
#[test]
fn sealed_alerts_keep_kind_inside_data() {
    let _db = crate::db::scoped_for_test();
    let _relay = super::relay_state_test_lock();
    let (sk1, k1) = phone_key(81);
    let (sk2, mut k2) = phone_key(82);
    k2.v = 2;
    let d = vec![
        register("kd-v1", "apns", "tok-kd-v1", true, Some(&k1)),
        register("kd-v2", "apns", "tok-kd-v2", true, Some(&k2)),
    ];
    let long = "Very-long-name-".repeat(400);
    let oversized = PushEvent::ProjectPost(ProjectPush {
        project_name: long.clone(),
        group_id: "pg-9".into(),
        principal: "owner".into(),
        device_usernames: vec!["owner".into()],
        unread_total: "7".into(),
        waiting_total: "2".into(),
    });
    let PushEvent::ThreadPost(mut long_thread) = msg_thread(&long) else { unreachable!("a thread push") };
    long_thread.agent_name = long.clone();
    long_thread.preview = long.clone();
    let mut events = alert_events();
    events.extend([oversized, PushEvent::ThreadPost(long_thread)]);
    for event in events {
        let want = e2e::alert_kind(&event).expect("an alert");
        let url = unique_url("kind");
        e2e::note_relay_caps(&url, true, true);
        let relay = Relay::new(true);
        tunnel_cloud(&url, relay.clone()).dispatch_in_mode(&event, LEAK_BK, &d, PushE2eMode::SealedWhenSupported).expect("dispatch");
        let calls = relay.calls();
        assert_eq!(calls.len(), 2, "one call per format: {event:?}");
        for (_, body, sealed) in &calls {
            assert!(*sealed);
            let (sk, psk) = if body["payload"]["data"]["v"] == "2" { (&sk2, &k2.psk) } else { (&sk1, &k1.psk) };
            let blob = body["devices"][0]["e2e"].as_str().expect("e2e");
            let pt: serde_json::Value =
                serde_json::from_slice(&seal::open(SealKind::Alert, sk, psk, blob).expect("opens")).expect("json");
            assert_eq!(pt["kind"], want, "{event:?}: {pt}");
            assert_eq!(pt["data"]["kind"], want, "kind inside data: {event:?}: {pt}");
        }
    }
}
