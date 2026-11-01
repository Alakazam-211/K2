//! S7 of the voice rooms PRD (v1.9 §8, §6.4): the Pro gate in the
//! daemon, the direct-mode exposure note, and the ports file.
//!
//! - **Pro gate** ([`hosting`], [`require_pro`]): hosting voice rooms needs
//!   a Pro label. The host daemon answers `hosting.state = not_pro` and
//!   refuses joins with `host_not_pro` unless the cached plan tier says
//!   `pro` ([`super::ops::SewOps::plan_tier`]; the refresh rule is at
//!   `k2_core::tunnel::subdomains::TIER_FRESH`: fresh 5 min, a stale
//!   reading still counts up to 1 h, then unknown = not Pro). Joining
//!   depends only on the host; nothing checks the joiner's own box. The
//!   gate is advisory under MIT; K2 Connect's relay and cert broker are the
//!   firm side (§8). Lanes B and C call these for `/cli/voice/*` and the
//!   rooms; this module never reaches the network.
//! - **Exposure** ([`exposure`]): voice in direct mode (`direct`, or
//!   `passthrough` behind K2's :443 front) makes the box's address public:
//!   its IP in DNS (`turn-<label>.k2.dev`), in certificate logs, and to
//!   everyone who joins. `/cli/sew/status` carries the standing note as
//!   `voice.exposure`; the tunnel tier publishes no box address.
//! - **Ports file** ([`sync_ports_file`], §6.4 mitigation 3): K2 opens no
//!   firewall itself (`voice.rs`), so the daemon writes the ports voice
//!   needs to `~/.k2/sew/ports.json` for K2 Web's host firewall
//!   (`k2-host-fw`) to read. Shape (version 1; to agree with K2 Web):
//!   `{"version": 1, "mode": "direct"|"tunnel"|"off", "open": [{"proto":
//!   "tcp"|"udp", "port": n, "why": "…"}], "updatedAt": unix}`. `off` (or
//!   `tunnel`) opens nothing. Written only when it changed, and only with
//!   the voice rooms preview on (dark in 0.46.8).

use serde_json::{json, Value};

use k2_root_proto::sew::{SEW_MEDIA_TCP_PORT, SEW_MEDIA_UDP_PORT, SEW_TURN_TLS_PORT};

use super::supervisor::{no, Env, R};
use super::voice::{self, TIER_DIRECT, TIER_PASSTHROUGH};

/// The refusal when this box can't host voice rooms below Pro.
pub const HOST_NOT_PRO: &str = "host_not_pro";

/// `hosting` as clients and `k2 voice status` show it: `{state: pro |
/// not_pro, tier, reading: fresh | stale | unknown, fetchedAt, reason?}`.
pub fn hosting(env: &Env) -> Value {
    let view = env.ops.plan_tier();
    let r = view.reading();
    let mut v = json!({
        "state": if view.is_pro() { "pro" } else { "not_pro" },
        "tier": r.and_then(|r| r.tier.clone()),
        "reading": view.state(),
        "fetchedAt": r.map(|r| r.fetched_at),
    });
    if !view.is_pro() {
        v["reason"] = json!(not_pro_reason(&view));
    }
    v
}

fn not_pro_reason(view: &k2_core::tunnel::subdomains::TierView) -> String {
    match view.reading() {
        Some(r) => format!(
            "voice rooms need a Pro plan; this box's plan is {} (https://k2.dev/dashboard)",
            r.tier.as_deref().unwrap_or("not reported")
        ),
        None => "voice rooms need a Pro plan, and K2 doesn't know this box's plan yet (no K2 Connect subdomain, or no answer \
                 from K2 Connect in the last hour); it checks again every minute"
            .into(),
    }
}

/// The host-side Pro gate: `Ok` on a Pro box, else `403 host_not_pro`.
pub fn require_pro(env: &Env) -> R<()> {
    let view = env.ops.plan_tier();
    if view.is_pro() {
        Ok(())
    } else {
        Err(no(HOST_NOT_PRO, 403, not_pro_reason(&view)))
    }
}

/// The standing note for direct mode (§6.4).
pub fn direct_note(label: Option<&str>) -> String {
    format!(
        "Direct mode makes this box's address public: its IP is in DNS ({}), in public certificate logs, and visible to \
         everyone who joins a call.",
        voice::turn_host(label.unwrap_or("<label>"))
    )
}

/// The ports voice needs open on this box for `tier` (`installed`: voice is
/// set up). The tunnel tier (and no voice) needs none.
pub fn ports_for(installed: bool, tier: &str) -> Vec<Value> {
    if !installed {
        return Vec::new();
    }
    let turn = match tier {
        TIER_DIRECT => SEW_TURN_TLS_PORT,
        // TURN rides K2's :443 front.
        TIER_PASSTHROUGH => voice::FRONT_PORT,
        // `tunnel` (lane A) and anything else: frpc dials out, nothing opens.
        _ => return Vec::new(),
    };
    vec![
        json!({"proto": "tcp", "port": turn, "why": "TURN over TLS"}),
        json!({"proto": "tcp", "port": SEW_MEDIA_TCP_PORT, "why": "ICE over TCP"}),
        json!({"proto": "udp", "port": SEW_MEDIA_UDP_PORT, "why": "voice media"}),
    ]
}

/// `voice.exposure` for `/cli/sew/status`: `{mode: direct|tunnel|off,
/// public, note?, ports}`.
pub fn exposure(env: &Env) -> Value {
    let st = voice::load(env);
    let tier = voice::tier(&st);
    let ports = ports_for(st.installed, tier);
    let mode = mode_of(st.installed, tier);
    let mut v = json!({"mode": mode, "public": mode == "direct", "ports": ports});
    if mode == "direct" {
        v["note"] = json!(direct_note(st.label.as_deref()));
    }
    v
}

fn mode_of(installed: bool, tier: &str) -> &'static str {
    match (installed, tier) {
        (false, _) => "off",
        (true, TIER_DIRECT | TIER_PASSTHROUGH) => "direct",
        (true, _) => "tunnel",
    }
}

/// The ports file's record name (`~/.k2/sew/ports.json`).
pub const PORTS_RECORD: &str = "ports";

/// Write the ports file when what it says changed (health loop). Only
/// with the voice rooms preview on; never creates one for a box that never
/// had voice. Returns the new body when it wrote.
pub fn sync_ports_file(env: &Env) -> Option<Value> {
    if !env.ops.linux() || !env.ops.voice_preview() {
        return None;
    }
    let st = voice::load(env);
    let tier = voice::tier(&st);
    let have = env.ops.record_load(PORTS_RECORD);
    if have.is_none() && !st.installed {
        return None;
    }
    let want = json!({"version": 1, "mode": mode_of(st.installed, tier), "open": ports_for(st.installed, tier)});
    if let Some(h) = &have {
        if h.get("version") == want.get("version") && h.get("mode") == want.get("mode") && h.get("open") == want.get("open") {
            return None;
        }
    }
    let mut body = want;
    body["updatedAt"] = json!(env.ops.now());
    if let Err(e) = env.ops.record_save(PORTS_RECORD, Some(&body)) {
        k2_core::log_debug!("[sew] ports file: {e}");
        return None;
    }
    Some(body)
}

// Server-only (Linux boxes).
#[cfg(test)]
#[cfg(unix)]
#[path = "pro_gate_tests.rs"]
mod tests;
