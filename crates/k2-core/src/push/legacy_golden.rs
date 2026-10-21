//! The legacy push bodies, pinned byte for byte (prd-e2e-push-v1 rev 2
//! §4, §8; Rosson 2026-10-20: "keep the current code we have").
//!
//! `tests/data/push_legacy_golden_r3.json` was generated ONCE from the
//! unmodified 0.46.3-r3 code (`origin/integ/0.46.3-r3` @ `6fc93c301`) and
//! is never regenerated: every dispatch body today's path builds, for
//! every kind × previews combination, must equal it exactly. The E2E work
//! is additive; this test is what proves an unkeyed device (or any device
//! with the switch `off`) still gets 0.46.3-r3's pushes.

#![cfg(test)]

use super::*;
use std::sync::{Arc, Mutex};

pub(crate) const GOLDEN: &str = include_str!("../../tests/data/push_legacy_golden_r3.json");

/// Fixed stamp values (the push tests' own constants).
pub(crate) const GOLDEN_INSTANCE: &str = "feedfacefeedface";
pub(crate) const GOLDEN_BADGE_KEY: &str = "0123456789abcdef0123456789abcdef";
pub(crate) const GOLDEN_URL: &str = "https://push-golden.k2.dev";

/// Records every body; answers 200 with no results.
#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) bodies: Mutex<Vec<String>>,
}

impl DispatchSender for Recorder {
    fn post(&self, _url: &str, _auth: &GatewayAuth, body: &serde_json::Value) -> Result<GatewayResponse, PushError> {
        self.bodies.lock().expect("recorder").push(body.to_string());
        Ok(golden_ok())
    }
}

fn golden_ok() -> GatewayResponse {
    GatewayResponse { status: 200, body: "{\"results\":[]}".into(), s0: false, s1: false }
}

pub(crate) fn golden_cloud(sender: Arc<dyn DispatchSender>) -> K2Cloud {
    K2Cloud::with_config(
        GatewayConfig {
            url: GOLDEN_URL.into(),
            tunnel: None,
            shared_token: Some("gw-golden".into()),
            subdomain: "rosson".into(),
        },
        sender,
    )
}

pub(crate) fn golden_device(id: &str, platform: &str, token: &str, previews: bool) -> crate::push_devices::PushDevice {
    crate::push_devices::PushDevice {
        device_id: id.into(),
        platform: platform.into(),
        token: token.into(),
        username: "owner".into(),
        created_at: 1,
        updated_at: 1,
        last_seen_at: 1,
        previews,
    }
}

/// Every event kind × previews combination, named.
pub(crate) fn golden_events() -> Vec<(String, PushEvent)> {
    let thread = |kind: ThreadMsgKind, count: u32, preview: &str| {
        PushEvent::ThreadPost(ThreadPush {
            agent_name: "Cortana".into(),
            conversation_id: "c-77".into(),
            addr: "k2/reviewer".into(),
            seq: 418,
            project_id: "proj-1".into(),
            msg_kind: kind,
            count,
            principal: "owner".into(),
            device_usernames: vec!["owner".into()],
            unread_total: "4".into(),
            waiting_total: "1".into(),
            avatar_rev: "ba7816bf8f01cfea".into(),
            agent_color: "#3b82f6".into(),
            preview: preview.into(),
        })
    };
    let mut out = vec![
        (
            "agent-needs-attention".to_string(),
            PushEvent::AgentNeedsAttention {
                agent: "cortana".into(),
                summary: "Weekly report drafted".into(),
                action_url: "k2so://agents/cortana".into(),
            },
        ),
        (
            "heartbeat-failed".to_string(),
            PushEvent::HeartbeatFailed { heartbeat: "nightly".into(), reason: "timeout".into() },
        ),
        (
            "ticket-created".to_string(),
            PushEvent::FeedbackCreated { agent_name: "Cortana".into(), feedback_id: "fb-42".into(), assignees: vec![] },
        ),
        (
            "ticket-created-assigned".to_string(),
            PushEvent::FeedbackCreated {
                agent_name: "Cortana".into(),
                feedback_id: "fb-43".into(),
                assignees: vec!["owner".into()],
            },
        ),
        (
            "ticket-commented".to_string(),
            PushEvent::FeedbackCommented { feedback_id: "fb-42".into(), assignees: vec![] },
        ),
        (
            "project-message-created".to_string(),
            PushEvent::ProjectMessageCreated { project_name: "Apollo".into(), group_id: "pg-9".into() },
        ),
        (
            "project-post".to_string(),
            PushEvent::ProjectPost(ProjectPush {
                project_name: "Apollo".into(),
                group_id: "pg-9".into(),
                principal: "owner".into(),
                device_usernames: vec!["owner".into()],
                unread_total: "7".into(),
                waiting_total: "2".into(),
            }),
        ),
        (
            "badge".to_string(),
            PushEvent::Badge(BadgePush {
                principal: "owner".into(),
                device_usernames: vec!["owner".into()],
                unread_total: "3".into(),
                waiting_total: "1".into(),
            }),
        ),
    ];
    for (kname, kind) in [("text", ThreadMsgKind::Text), ("choice", ThreadMsgKind::Choice), ("secret", ThreadMsgKind::Secret)] {
        for count in [1u32, 3] {
            for (pname, preview) in [("nopreview", ""), ("preview", "Deploy finished, 3 checks green 🚀")] {
                out.push((format!("thread-{kname}-x{count}-{pname}"), thread(kind, count, preview)));
            }
        }
    }
    out
}

/// The device sets every event goes to: previews on only, off only, and
/// a mixed pair (the preview split).
pub(crate) fn golden_device_sets() -> Vec<(String, Vec<crate::push_devices::PushDevice>)> {
    vec![
        ("on".into(), vec![golden_device("golden-dev-on", "apns", "tok-golden-on", true)]),
        ("off".into(), vec![golden_device("golden-dev-off", "fcm", "tok-golden-off", false)]),
        (
            "mixed".into(),
            vec![
                golden_device("golden-dev-on", "apns", "tok-golden-on", true),
                golden_device("golden-dev-off", "fcm", "tok-golden-off", false),
            ],
        ),
    ]
}

/// Run every case through `dispatch` and collect `{case: [body, …]}`.
pub(crate) fn collect(dispatch: &dyn Fn(&K2Cloud, &PushEvent, &[crate::push_devices::PushDevice])) -> serde_json::Value {
    collect_with(&golden_cloud, dispatch)
}

/// [`collect`] with the cloud built by `make` (e.g. a per-daemon config).
pub(crate) fn collect_with(
    make: &dyn Fn(Arc<dyn DispatchSender>) -> K2Cloud,
    dispatch: &dyn Fn(&K2Cloud, &PushEvent, &[crate::push_devices::PushDevice]),
) -> serde_json::Value {
    set_instance_id(GOLDEN_INSTANCE);
    let mut map = serde_json::Map::new();
    for (ename, event) in golden_events() {
        for (dname, devices) in golden_device_sets() {
            let rec = Arc::new(Recorder::default());
            let cloud = make(rec.clone());
            dispatch(&cloud, &event, &devices);
            let bodies = rec.bodies.lock().expect("recorder").clone();
            map.insert(format!("{ename}/{dname}"), serde_json::json!(bodies));
        }
    }
    serde_json::Value::Object(map)
}

/// Compare a collected run with the r3 golden, case by case, byte for byte.
pub(crate) fn assert_matches_golden(got: &serde_json::Value, label: &str) {
    let want: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden JSON parses");
    let want = want.as_object().expect("golden is an object");
    let got = got.as_object().expect("collected is an object");
    assert_eq!(
        want.keys().collect::<Vec<_>>(),
        got.keys().collect::<Vec<_>>(),
        "{label}: the set of golden cases changed"
    );
    for (case, bodies) in want {
        let want_bodies: Vec<&str> = bodies.as_array().expect("array").iter().map(|b| b.as_str().expect("str")).collect();
        let got_bodies: Vec<&str> = got[case].as_array().expect("array").iter().map(|b| b.as_str().expect("str")).collect();
        assert_eq!(want_bodies, got_bodies, "{label}: case {case} differs from 0.46.3-r3 byte for byte");
    }
}

#[test]
fn legacy_bodies_are_byte_identical_to_r3() {
    // Through the production entry point (the server mode as configured:
    // the default `sealed-when-supported`), devices with no key.
    let got = collect(&|cloud, event, devices| {
        cloud.dispatch(event, GOLDEN_BADGE_KEY, devices).expect("dispatch");
    });
    assert_matches_golden(&got, "unkeyed devices");
    // And with the switch in every mode: no key means today's path in
    // `off` and `sealed-when-supported`.
    for mode in [super::e2e::PushE2eMode::Off, super::e2e::PushE2eMode::SealedWhenSupported] {
        let got = collect(&|cloud, event, devices| {
            cloud.dispatch_in_mode(event, GOLDEN_BADGE_KEY, devices, mode).expect("dispatch");
        });
        assert_matches_golden(&got, &format!("unkeyed devices, {mode:?}"));
    }
}
