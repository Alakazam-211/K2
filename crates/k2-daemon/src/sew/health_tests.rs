//! Sew 0.3.1's health contract and K2's reading of it.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::json;

use super::*;
use crate::sew::ops::fake::FakeOps;
use crate::sew::secrets::MemSecrets;
use crate::sew::supervisor::Env;

fn env_with(ops: &'static FakeOps) -> Env<'static> {
    let secrets: &'static MemSecrets = Box::leak(Box::new(MemSecrets::default()));
    let latch: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let token: &'static Mutex<Option<String>> = Box::leak(Box::new(Mutex::new(None)));
    Env { ops, secrets, latch, token }
}

#[test]
fn ready_and_the_older_ok_only_answer_both_read_as_ready() {
    assert!(parse(Ok((200, json!({"ok": true, "ready": true})))).ready);
    assert!(parse(Ok((200, json!({"ok": true})))).ready, "Sew 0.3.0 has no ready field");
    let h = parse(Ok((200, json!({"ok": true, "ready": false, "reason": "db"}))));
    assert!(!h.ready, "ready:false wins even on a 200");
    assert_eq!(h.reason.as_deref(), Some("db"));
}

#[test]
fn reasons_since_and_unknowns() {
    let up = parse(Ok((503, json!({"ok": false, "error": {"code": "not_ready", "hint": "upgrading"}, "ready": false, "reason": "upgrading", "since": "2026-10-27T10:00:00Z"}))));
    assert_eq!((up.ready, up.reason.as_deref(), up.since.clone()), (false, Some("upgrading"), Some(json!("2026-10-27T10:00:00Z"))));
    assert!(up.waits(), "K2 waits on upgrading");
    // Sew's exact body (2026-10-27).
    let stop = parse(Ok((503, json!({"ok": false, "ready": false, "reason": "stopping", "error": {"code": "unavailable", "hint": "draining", "details": {"reason": "stopping"}}}))));
    assert_eq!(stop.detail.as_deref(), Some("draining"));
    assert_eq!((stop.reason.as_deref(), stop.since.clone()), (Some("stopping"), None), "stopping has no since");
    assert!(!stop.waits(), "K2 never waits on stopping");
    for r in ["db", "migrations", "cosmic_rays"] {
        let h = parse(Ok((503, json!({"ready": false, "reason": r, "since": 1}))));
        assert_eq!(h.reason.as_deref(), Some(r));
        assert!(!h.waits(), "{r}: plain not-ready, no wait");
    }
    let bare = parse(Ok((503, json!({"ok": false}))));
    assert_eq!(bare.reason.as_deref(), Some("not_ready"));
    let down = parse(Err("connection refused".into()));
    assert_eq!((down.ready, down.reason.as_deref(), down.status), (false, Some("down"), None));
    assert!(!down.waits());
}

#[test]
fn the_refusal_carries_state_since_retry_and_the_right_hint() {
    let stop = parse(Ok((503, json!({"ready": false, "reason": "stopping"}))));
    let d = unavailable_details(&stop);
    assert_eq!(d["state"], "stopping");
    assert_eq!(d["since"], serde_json::Value::Null);
    assert!(d["hint"].as_str().unwrap().contains("won't wait"), "{d}");
    assert!(d["retryAfterMs"].as_u64().unwrap() > 0);
    let f = unavailable(&stop);
    assert_eq!((f.code, f.status), ("sew_unavailable", 503));
    let up = unavailable_details(&parse(Ok((503, json!({"ready": false, "reason": "upgrading", "since": 5})))));
    assert_eq!((up["state"].clone(), up["since"].clone()), (json!("upgrading"), json!(5)));
}

#[test]
fn a_mint_waits_through_an_upgrade_but_not_through_a_stop() {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps::new()));
    let env = env_with(ops);
    // Upgrading, then ready: the wait polls until ready.
    ops.queue_api_health(vec![
        Ok((503, json!({"ready": false, "reason": "upgrading", "since": 1}))),
        Ok((503, json!({"ready": false, "reason": "upgrading", "since": 1}))),
        Ok((200, json!({"ok": true, "ready": true}))),
    ]);
    let h = wait_ready(&env, MINT_WAIT_MS);
    assert!(h.ready, "{h:?}");
    assert_eq!(ops.api_health_calls(), 3);
    // Upgrading past the cap: K2 gives up after MINT_WAIT_MS.
    let n = (MINT_WAIT_MS / POLL_MS) as usize + 5;
    ops.queue_api_health((0..n).map(|_| Ok((503, json!({"ready": false, "reason": "upgrading"})))).collect());
    let before = ops.api_health_calls();
    let h = wait_ready(&env, MINT_WAIT_MS);
    assert_eq!(h.reason.as_deref(), Some("upgrading"));
    assert_eq!(ops.api_health_calls() - before, (MINT_WAIT_MS / POLL_MS) as usize + 1, "one probe, then one per poll");
    // Stopping: one probe, no wait.
    ops.queue_api_health(vec![Ok((503, json!({"ready": false, "reason": "stopping"})))]);
    let before = ops.api_health_calls();
    let h = wait_ready(&env, MINT_WAIT_MS);
    assert_eq!((h.reason.as_deref(), ops.api_health_calls() - before), (Some("stopping"), 1));
}

#[test]
fn doctor_warns_degrade_fails_fail_and_unknowns_only_list() {
    let r = json!({"checks": [
        {"id": "db.reachable", "status": "ok"},
        {"id": "media.reachability", "status": "warn", "detail": "UDP 7882 not reachable from outside"},
        {"id": "creds.files", "status": "unknown", "detail": "not root"},
        {"id": "sysctl.udp_buffers", "status": "unknown"},
        {"id": "upgrade.state", "status": "warn", "detail": "stale: phase swap, 3 h old, writer gone"},
    ]});
    let v = doctor_verdict(&r);
    assert_eq!(v["verdict"], "degraded", "media.reachability warn is degraded, never failed: {v}");
    assert_eq!(v["unknown"], json!(["creds.files", "sysctl.udp_buffers"]));
    assert_eq!(v["upgradeState"]["detail"], "stale: phase swap, 3 h old, writer gone");
    let bad = json!({"checks": [{"id": "db.reachable", "status": "fail"}, {"id": "media.reachability", "status": "warn"}]});
    assert_eq!(doctor_verdict(&bad)["verdict"], "failed");
    assert_eq!(doctor_verdict(&json!({"checks": [{"id": "a", "status": "unknown"}]}))["verdict"], "ok");
}

#[test]
fn media_turn_maps_sews_block_and_older_sew_has_none() {
    let e = json!({"state": "running", "turn": {"tier": "passthrough", "ice_urls": ["turns:turn.box.k2.dev:443"], "source": "config"}});
    assert_eq!(
        media_turn(Some(&e)),
        Some(json!({"tier": "passthrough", "iceUrls": ["turns:turn.box.k2.dev:443"], "source": "config"}))
    );
    assert_eq!(media_turn(Some(&json!({"state": "running"}))), None);
    assert_eq!(media_turn(None), None);
}
