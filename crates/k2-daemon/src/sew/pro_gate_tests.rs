//! Voice rooms S7 (PRD v1.9 §8, §6.4): the host's Pro gate on the cached
//! tier, the direct-mode exposure note, and the ports file.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use k2_core::tunnel::subdomains::{TierReading, TierView};
use serde_json::json;

use super::*;
use crate::sew::ops::fake::FakeOps;
use crate::sew::secrets::MemSecrets;

fn env() -> (Env<'static>, &'static FakeOps) {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps::new()));
    let env = Env {
        ops,
        secrets: Box::leak(Box::new(MemSecrets::default())),
        latch: Box::leak(Box::new(AtomicBool::new(false))),
        token: Box::leak(Box::new(Mutex::new(None))),
    };
    (env, ops)
}

/// Hosting needs Pro: unknown (no reading, or past the 1 h grace) and any
/// other tier answer `not_pro` and refuse with `403 host_not_pro`; a fresh
/// or stale (under 1 h) Pro reading passes.
#[test]
fn hosting_needs_a_pro_reading() {
    let (env, ops) = env();
    let h = hosting(&env);
    assert_eq!((h["state"].as_str(), h["reading"].as_str()), (Some("not_pro"), Some("unknown")), "{h}");
    assert!(h["reason"].as_str().unwrap().contains("doesn't know this box's plan yet"), "{h}");
    let f = require_pro(&env).unwrap_err();
    assert_eq!((f.code, f.status), (HOST_NOT_PRO, 403));
    for tier in ["free", "single"] {
        ops.set_tier(Some(tier));
        let h = hosting(&env);
        assert_eq!((h["state"].as_str(), h["tier"].as_str(), h["reading"].as_str()), (Some("not_pro"), Some(tier), Some("fresh")), "{h}");
        assert!(h["reason"].as_str().unwrap().contains(&format!("this box's plan is {tier}")), "{h}");
        assert_eq!(require_pro(&env).unwrap_err().code, HOST_NOT_PRO);
    }
    ops.set_tier(Some("pro"));
    require_pro(&env).expect("pro");
    let h = hosting(&env);
    assert_eq!(h["state"], "pro");
    assert!(h.get("reason").is_none(), "{h}");
    let reading = TierReading { primary: "lztek".into(), tier: Some("pro".into()), fetched_at: 1 };
    *ops.tier.lock().unwrap() = TierView::Stale(reading);
    require_pro(&env).expect("a stale Pro reading still hosts (a control-plane blip closes nothing)");
    assert_eq!(hosting(&env)["reading"], "stale");
    *ops.tier.lock().unwrap() = TierView::Unknown;
    assert_eq!(require_pro(&env).unwrap_err().code, HOST_NOT_PRO);
}

fn voice_record(ops: &FakeOps, installed: bool, tier: Option<&str>) {
    *ops.voice.lock().unwrap() = Some(json!({"installed": installed, "turnTier": tier, "label": "lztek"}));
}

/// §6.4: direct mode (and passthrough behind the :443 front) publishes the
/// box's address and opens TURN-TLS, ICE-TCP and media; the tunnel tier and
/// no voice open nothing.
#[test]
fn exposure_and_ports_follow_the_voice_tier() {
    let (env, ops) = env();
    assert_eq!(exposure(&env), json!({"mode": "off", "public": false, "ports": []}));
    voice_record(ops, true, Some("direct"));
    let e = exposure(&env);
    assert_eq!((e["mode"].as_str(), e["public"].as_bool()), (Some("direct"), Some(true)), "{e}");
    assert_eq!(
        e["note"],
        "Direct mode makes this box's address public: its IP is in DNS (turn-lztek.k2.dev), in public certificate logs, and \
         visible to everyone who joins a call."
    );
    assert_eq!(
        e["ports"],
        json!([
            {"proto": "tcp", "port": 5349, "why": "TURN over TLS"},
            {"proto": "tcp", "port": 7881, "why": "ICE over TCP"},
            {"proto": "udp", "port": 7882, "why": "voice media"},
        ])
    );
    voice_record(ops, true, None);
    assert_eq!(exposure(&env)["ports"][0]["port"], 5349, "no tier recorded = direct (pre-0.46.1)");
    voice_record(ops, true, Some("passthrough"));
    let e = exposure(&env);
    assert_eq!((e["mode"].as_str(), e["ports"][0]["port"].as_u64()), (Some("direct"), Some(443)), "{e}");
    voice_record(ops, true, Some("tunnel"));
    assert_eq!(exposure(&env), json!({"mode": "tunnel", "public": false, "ports": []}));
    voice_record(ops, false, Some("direct"));
    assert_eq!(exposure(&env)["mode"], "off");
}

/// The ports file: dark with the preview off; never made for a box that
/// never had voice; written only when it changed; `off` closes everything.
#[test]
fn the_ports_file_is_written_when_it_changes() {
    let (env, ops) = env();
    let file = |ops: &FakeOps| ops.records.lock().unwrap().get(PORTS_RECORD).cloned();
    voice_record(ops, true, Some("direct"));
    assert_eq!(sync_ports_file(&env), None, "preview off: nothing");
    assert!(file(ops).is_none());
    *ops.preview.lock().unwrap() = true;
    *ops.voice.lock().unwrap() = None;
    assert_eq!(sync_ports_file(&env), None, "never had voice: no file");
    assert!(file(ops).is_none());
    voice_record(ops, true, Some("direct"));
    let w = sync_ports_file(&env).expect("written");
    assert_eq!((w["version"].as_u64(), w["mode"].as_str()), (Some(1), Some("direct")));
    assert_eq!(w["open"].as_array().unwrap().len(), 3);
    assert_eq!(w["updatedAt"], *ops.clock.lock().unwrap());
    assert_eq!(file(ops), Some(w));
    *ops.clock.lock().unwrap() += 60;
    assert_eq!(sync_ports_file(&env), None, "unchanged: not rewritten");
    voice_record(ops, true, Some("tunnel"));
    let w = sync_ports_file(&env).expect("tunnel");
    assert_eq!((w["mode"].as_str(), w["open"].clone()), (Some("tunnel"), json!([])));
    voice_record(ops, false, None);
    let w = sync_ports_file(&env).expect("off");
    assert_eq!((w["mode"].as_str(), w["open"].clone()), (Some("off"), json!([])));
    assert_eq!(sync_ports_file(&env), None);
}
