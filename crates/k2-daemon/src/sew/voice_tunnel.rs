//! Voice through the tunnel: K2's `tunnel` TURN tier (voice rooms PRD v1.9
//! §6.1, §6.6 M1/M4–M9, slice S9a; 0.46.8, behind `voice_tunnel_preview`).
//!
//! sew-media runs Sew's `passthrough` TURN tier on 127.0.0.1:5349 (PROXY v2
//! from loopback only, TLS terminated by sew-media with its own key), and
//! the voice lane's frpc (`frpc-voice`, k2-core `tunnel::voice_lane`) carries
//! `turn.<label>.k2.dev` to it through the relay. Clients get exactly one ICE
//! server, `turns:turn.<label>.k2.dev:443?transport=tcp`, relay-only. The box
//! publishes no IP and opens no port: no `PUT /turn/dns`, no node IP, no NAT
//! check. (K2's other `passthrough` tier is front443's: the box's own :443.)
//!
//! - **Gates** ([`ready`]): the preview switch; root helper protocol 12 with
//!   `namespaces.sewtunnel` (the new media keys); Sew's feature
//!   `turn_relay_only` (Sew 0.7.0: no `node_ip` with relay-only passthrough).
//! - **Label:** the label the control plane confirmed for the tunnel token
//!   (relay name binding), never the requested `subdomain`.
//! - **Cert** ([`issue`]): broker mode `turn-tunnel`, body
//!   `{"subdomain": <bare label>, "mode": "turn-tunnel", "csr": …}`, one SAN
//!   `turn.<label>.k2.dev`, the box's own TURN key (the vault's, as for
//!   direct). `403 tier_required` → `not_pro`. Installed through `sew media
//!   install --reconfigure` (a tier move: the domain changes with it) or `sew
//!   media turn-cert --when-idle` (renewal, same key).
//! - **Sew settings** ([`media_keys`], Sew SW-9 11-01): `sew media install
//!   --relay-only --turn-tls passthrough` (`relay_only = true`, `turn_tls`),
//!   `turn_domain`, `bind_v4 = 127.0.0.1` (UDP 7882 / TCP 7881 off the
//!   network) and NO `node_ip` (Sew writes 127.0.0.1 itself; the engine's only
//!   host candidate is loopback, reachable by its own TURN). No
//!   `turn_trusted_cidrs`: not an install key on Sew 0.6.x or 0.7.0 (an unknown
//!   key fails the install with exit 2), and Sew's default is loopback only,
//!   which is where frpc-voice dials from. Back in direct mode:
//!   `relay_only = false`, `bind_v4 = 0.0.0.0`, the node IP. Sew refuses
//!   relay-only with tier plain/off (exit 78, [`sew_media`] names it); K2
//!   never sends it.
//! - **Served check** ([`served`]): through the relay, `turn.<label>.k2.dev:443`
//!   with that SNI, compared with the leaf K2 issued.

use serde_json::{json, Value};

use k2_root_proto::sew::{MEDIA_BIND_ANY, MEDIA_BIND_LOOPBACK, SEW_EXIT_RELAY_ONLY_REFUSED, UNIT_MEDIA};

use super::supervisor::{self as sup, no, Env, R};
use super::voice::{self, VoiceState, TIER_TUNNEL};

/// Sew's FEATURES flag for the relay-only passthrough tier (Sew 0.7.0).
pub const FEATURE_TURN_RELAY_ONLY: &str = "turn_relay_only";
/// The cert broker mode for the tunnel's TURN name (K2 Web, 11-01).
pub const CERT_MODE: &str = "turn-tunnel";
/// How long an install or tier move waits for the voice lane to come up
/// before its served check.
pub const LANE_WAIT_SECS: u32 = 20;
/// The public port clients reach TURN on (the relay's :443).
pub const PORT: u16 = 443;

/// `turn.<label>.k2.dev` (§6.6 M1).
pub fn host(label: &str) -> String {
    k2_core::tunnel::render::voice_turn_host(label)
}

/// The node IP the tunnel tier sends Sew: none (Sew 11-01: with
/// `--relay-only --turn-tls passthrough` Sew writes `node_ip = 127.0.0.1`
/// into media.toml itself). The one place to change if that ever moves.
pub fn node_ip() -> Option<String> {
    None
}

/// The label the control plane confirmed for this box's tunnel token.
pub fn label(env: &Env) -> R<String> {
    env.ops.confirmed_label().ok_or_else(|| {
        no(
            "no_confirmed_label",
            409,
            "the control plane has not confirmed this box's K2 Connect label yet (the tunnel needs to reach it once): \
             k2 tunnel status, then try again",
        )
    })
}

/// Sew's FEATURES list (live `GET /mgmt/v1/status`).
pub fn sew_features(env: &Env) -> Result<Vec<String>, String> {
    let st = sup::mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| e.to_string())?;
    Ok(st
        .get("features")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default())
}

/// Root helper protocol 12 with `namespaces.sewtunnel` (an older helper
/// refuses the new media keys).
pub fn helper_ok(env: &Env) -> bool {
    let st = env.ops.helper_state();
    st.usable(k2_root_proto::version::SEW_TUNNEL_MIN_PROTOCOL)
        && st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/sewtunnel")).and_then(Value::as_bool) == Some(true)
}

/// Everything the tunnel tier needs that K2 can't fix itself.
pub fn ready(env: &Env) -> R<()> {
    if !env.ops.tunnel_preview() {
        return Err(no(
            "voice_tunnel_preview_off",
            409,
            "voice through the tunnel is a preview: turn on voice_tunnel_preview (Settings) or set K2_VOICE_TUNNEL=1 in the daemon's environment",
        ));
    }
    // §7.3: a box under the voice floor runs chat only.
    let floor = super::autosetup::floor_now(env);
    if !floor.voice {
        return Err(no("voice_floor", 409, floor.line.unwrap_or_else(|| format!("this box is under the voice floor ({})", floor.code))));
    }
    if !helper_ok(env) {
        let st = env.ops.helper_state();
        return Err(no(
            "helper_outdated",
            409,
            crate::root_helper::helper_needed("Voice through the tunnel", k2_root_proto::version::SEW_TUNNEL_MIN_PROTOCOL, &st),
        ));
    }
    let features = sew_features(env).map_err(|e| no("mgmt_unreachable", 503, format!("Sew's features: {e}")))?;
    if !features.iter().any(|f| f == FEATURE_TURN_RELAY_ONLY) {
        return Err(no(
            "sew_too_old",
            409,
            format!("this Sew has no relay-only TURN tier (feature {FEATURE_TURN_RELAY_ONLY}, Sew 0.7.0): voice stays where it is"),
        ));
    }
    Ok(())
}

/// The Sew keys the tunnel tier sets (§6.6 M5/M9), plus the cert pair. Sew
/// refuses any other key: `voice_tunnel_tests` pin this list.
pub fn media_keys(host: &str, chain: &str, key_pem: &str) -> Value {
    let mut v = json!({
        "turn_tls": voice::TIER_PASSTHROUGH,
        "turn_domain": host,
        "relay_only": true,
        // UDP 7882 / TCP 7881 stay off the network (Sew 11-01; never empty).
        "bind_v4": MEDIA_BIND_LOOPBACK,
        "turn_cert_pem": chain,
        "turn_key_pem": key_pem,
    });
    if let Some(ip) = node_ip() {
        v["node_ip"] = json!(ip);
    }
    v
}

/// `sew media install [--reconfigure …]` through the helper, with Sew's
/// relay-only refusal (exit 78: relay-only with TURN tier plain or off)
/// named. K2 never sends that combination (the helper's MediaRequest
/// refuses it first), so seeing it means Sew and K2 disagree about the tier.
pub fn sew_media(env: &Env, tail: &[&str], body: &Value, code: &'static str) -> R<Value> {
    sup::sew_run(env, tail, Some(body.to_string().as_bytes())).map_err(|e| {
        if e.contains(&format!(": exit {SEW_EXIT_RELAY_ONLY_REFUSED}:")) {
            no(
                "sew_relay_only_refused",
                409,
                format!(
                    "Sew refused relay-only TURN for this TURN tier (exit {SEW_EXIT_RELAY_ONLY_REFUSED}: relay-only needs the \
                     passthrough tier, never plain or off); nothing changed: {e}"
                ),
            )
        } else {
            no(code, 500, e)
        }
    })
}

/// A cert for `turn.<label>.k2.dev` from broker mode `turn-tunnel`, for the
/// vault's TURN key (no DNS step: the name resolves to the relay). Records
/// it in `st`. Returns (chain, key PEM).
pub fn issue(env: &Env, label: &str, st: &mut VoiceState) -> R<(String, String)> {
    let h = host(label);
    let (key, new_ref) = voice::turn_key(env, st)?;
    if let Some(r) = new_ref {
        st.turn_key_ref = Some(r);
        voice::save(env, st);
    }
    let csr = voice::turn_csr_for(&h, &key).map_err(|e| no("turn_failed", 500, format!("CSR: {e}")))?;
    let body = json!({"subdomain": label, "mode": CERT_MODE, "csr": csr});
    let v = env.ops.cp("POST", "/cert", Some(&body)).map_err(|e| voice::cp_fail("TURN certificate", &e))?;
    let chain = v
        .get("cert")
        .and_then(Value::as_str)
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| no("turn_failed", 502, "the broker returned no certificate"))?;
    let not_after = voice::chain_matches(chain, &h, &key).map_err(|e| no("turn_failed", 502, e))?;
    st.cert_pem = Some(chain.to_string());
    st.cert_not_after = Some(not_after);
    st.cert_leaf_sha256 = crate::domains::status::pem_leaf_sha256(chain);
    st.cert_issued_at = Some(env.ops.now());
    st.turn_host = Some(h);
    st.label = Some(label.to_string());
    Ok((chain.to_string(), key.serialize_pem()))
}

/// The cert K2 holds for `turn.<label>` (outside the renewal window), else
/// a new one.
fn issue_or_reuse(env: &Env, label: &str, st: &mut VoiceState) -> R<(String, String)> {
    if let Some(pair) = voice::reusable_for(env, &host(label), st) {
        return Ok(pair);
    }
    issue(env, label, st)
}

/// `ok` / `mismatch` / `nothing`: what the relay serves for
/// `turn.<label>.k2.dev:443` (it resolves through the `*.k2.dev` wildcard to
/// the relay, which passes it down frpc-voice to sew-media).
pub fn served(env: &Env, host: &str, ours: Option<&str>) -> String {
    voice::served_check(env, host, host, ours, PORT)
}

/// Wait (up to `secs` one-second polls) for the voice lane to come up.
pub fn wait_lane(env: &Env, secs: u32) -> bool {
    for _ in 0..secs {
        if env.ops.voice_lane_up() {
            return true;
        }
        env.ops.sleep_ms(1000);
    }
    env.ops.voice_lane_up()
}

/// Arm the voice lane, wait for it, and check what the relay serves.
fn lane_and_served(env: &Env, h: &str, leaf: Option<&str>, warnings: &mut Vec<String>) -> String {
    if let Err(e) = env.ops.voice_lane(true) {
        warnings.push(format!("the voice lane did not start: {e}"));
        return "nothing".into();
    }
    if !wait_lane(env, LANE_WAIT_SECS) {
        warnings.push(format!(
            "the voice lane (frpc-voice) is not up after {LANE_WAIT_SECS} s: k2 tunnel status shows why (the relay must allow turn.<label> for this Pro label)"
        ));
    }
    let s = served(env, h, leaf);
    if s != "ok" {
        warnings.push(format!("TURN-TLS check through the relay: {h}:{PORT} served {s}"));
    }
    s
}

/// Preflight for the tunnel tier (V5): what it needs instead of public
/// ports and a node IP.
pub fn preflight(env: &Env) -> Vec<Value> {
    let check = |id: &str, status: &str, detail: String, hint: &str| json!({"id": id, "status": status, "detail": detail, "hint": hint});
    let mut c = Vec::new();
    match ready(env) {
        Ok(()) => c.push(check("tunnel_tier", "ok", "voice through the tunnel: preview on, helper protocol 12, Sew relay-only".into(), "")),
        Err(f) => c.push(check("tunnel_tier", "fail", f.code.to_string(), &f.hint)),
    }
    match env.ops.confirmed_label() {
        Some(l) => c.push(check("tunnel_label", "ok", format!("TURN host {}", host(&l)), "")),
        None => c.push(check("tunnel_label", "fail", "no confirmed K2 Connect label".into(), "the tunnel must reach the control plane once (k2 tunnel status)")),
    }
    // The cached plan (never the network here); unknown is only a note: the
    // cert broker's `403 tier_required` is the firm answer.
    let plan = env.ops.plan_tier();
    match plan.reading() {
        _ if plan.is_pro() => c.push(check("pro", "ok", "K2 Connect Pro".into(), "")),
        Some(r) => c.push(check(
            "pro",
            "fail",
            format!("K2 Connect {}", r.tier.as_deref().unwrap_or("plan not reported")),
            "voice through the tunnel needs K2 Connect Pro (nested names)",
        )),
        None => c.push(check("pro", "info", "K2 doesn't know this box's plan yet".into(), "the TURN certificate is refused below Pro")),
    }
    if env.ops.tunnel_e2e() {
        c.push(check("e2e", "ok", "the tunnel is end-to-end encrypted".into(), ""));
    } else {
        c.push(check("e2e", "fail", "the tunnel is not end-to-end encrypted".into(), "the relay passes turn.<label> only for E2E labels"));
    }
    if env.ops.frpc_installed() {
        c.push(check("frpc", "ok", "frpc found (the voice lane runs a second one)".into(), ""));
    } else {
        c.push(check("frpc", "fail", "frpc not installed".into(), "install frp's frpc (K2 Connect needs it anyway)"));
    }
    let media_up = env.ops.unit(UNIT_MEDIA).is_active();
    if media_up || !env.ops.port_taken("127.0.0.1", k2_root_proto::sew::SEW_TURN_TLS_PORT, false) {
        c.push(check("loopback_5349", "ok", "127.0.0.1:5349 free or held by sew-media".into(), ""));
    } else {
        c.push(check("loopback_5349", "fail", "127.0.0.1:5349 is taken".into(), "free it: sew-media's TURN listener goes there"));
    }
    c.push(check("ports", "info", "no public ports: TURN rides the relay's :443 and the voice lane".into(), ""));
    c
}

/// The tunnel tier's install, after staging (`voice::install_steps`):
/// cert (no DNS), `sew media install`, the voice label, start, the voice
/// lane and the served check through the relay.
pub fn install_steps(env: &Env, label: &str, url: &str, mut st: VoiceState) -> R<Value> {
    let h = host(label);
    voice::step(env, "turn-cert");
    let (chain, key_pem) = issue_or_reuse(env, label, &mut st)?;
    voice::save(env, &mut st);

    voice::step(env, "install");
    let mut body = media_keys(&h, &chain, &key_pem);
    body["public_url"] = json!(url);
    body["udp_port"] = json!(k2_root_proto::sew::SEW_MEDIA_UDP_PORT);
    body["tcp_port"] = json!(k2_root_proto::sew::SEW_MEDIA_TCP_PORT);
    body["sysctl"] = json!("apply");
    let out = sew_media(env, &["media", "install"], &body, "media_install_failed")?;
    let mut warnings: Vec<String> = out
        .pointer("/sew/warnings")
        .and_then(Value::as_array)
        .map(|w| w.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();

    voice::step(env, "label");
    let label_ok = voice::point_voice_label(env, label, url, st.voice_label, &mut warnings);

    voice::step(env, "start");
    sup::poll_units(env, &[UNIT_MEDIA], 60).map_err(|e| no("media_start_failed", 500, e))?;

    voice::step(env, "verify");
    voice::update(env, |s| {
        s.installed = true;
        s.turn_tier = Some(TIER_TUNNEL.to_string());
    });
    let served = lane_and_served(env, &h, st.cert_leaf_sha256.as_deref(), &mut warnings);

    let now = env.ops.now();
    st.installed = true;
    st.turn_tier = Some(TIER_TUNNEL.to_string());
    st.public_url = Some(url.to_string());
    st.node_ip = node_ip();
    st.node_ip_source = None;
    st.voice_label = label_ok;
    st.served = Some(served.clone());
    st.served_at = Some(now);
    st.installed_at = st.installed_at.or(Some(now));
    st.last_error = None;
    st.failures = 0;
    st.next_try_at = None;
    st.logins_at_failure = None;
    st.warnings = warnings.clone();
    st.progress = Some(json!({"step": null, "result": "done", "at": now}));
    voice::save(env, &mut st);
    Ok(json!({
        "ok": true,
        "state": "installed",
        "publicUrl": url,
        "turnHost": h,
        "nodeIp": null,
        "turnTier": TIER_TUNNEL,
        "relayOnly": true,
        "turnTls": {"port": PORT, "served": served, "certNotAfter": st.cert_not_after},
        "iceServers": [ice_url(label)],
        "ports": {"public": []},
        "warnings": warnings,
        "next": "test a call from another network (relay-only TURN over the tunnel), then k2 hostchat media probe --udp ok --tcp ok --turn-tls ok",
    }))
}

/// The one ICE server clients get in tunnel mode (§6.6 M1).
pub fn ice_url(label: &str) -> String {
    format!("turns:{}:443?transport=tcp", host(label))
}

/// Move an installed engine onto the tunnel tier: a `turn.<label>` cert,
/// `sew media install --reconfigure --restart --now` with the tunnel keys,
/// the voice lane, the served check; then the old `turn-<label>` A record
/// goes (it published the box's IP).
pub fn move_in(env: &Env, st: VoiceState) -> R<Value> {
    ready(env)?;
    let label = label(env)?;
    let h = host(&label);
    sup::helper_check(env)?;
    let _l = sup::begin(env)?;
    let mut st = st;
    // Read before the new cert replaces the recorded host.
    let from = voice::tier(&st).to_string();
    let had_dns = voice::has_turn_dns(&st);
    let dns_label = st.label.clone().unwrap_or_else(|| label.clone());
    let (chain, key_pem) = issue_or_reuse(env, &label, &mut st)?;
    voice::save(env, &mut st);
    let body = media_keys(&h, &chain, &key_pem);
    sew_media(env, &["media", "reconfigure", "restart-now"], &body, "media_reconfigure_failed")?;
    voice::update(env, |s| {
        s.turn_tier = Some(TIER_TUNNEL.to_string());
        s.node_ip = node_ip();
        s.node_ip_source = None;
    });
    // front443 stops routing the old passthrough name, if it did.
    env.ops.front_routes_changed();
    let mut warnings = Vec::new();
    if had_dns {
        if let Err(e) = env.ops.cp("DELETE", &format!("/turn/dns/{dns_label}"), None) {
            warnings.push(format!("the old turn-{dns_label}.k2.dev A record was not removed (it names this box's IP): {e}"));
        }
    }
    let served = lane_and_served(env, &h, st.cert_leaf_sha256.as_deref(), &mut warnings);
    let now = env.ops.now();
    voice::update(env, |s| {
        s.served = Some(served.clone());
        s.served_at = Some(now);
    });
    sup::changed("voice-tier");
    Ok(json!({
        "ok": true, "installed": true, "turnTier": TIER_TUNNEL, "from": from, "changed": true,
        "turnHost": h, "turnTlsPort": PORT, "served": served, "relayOnly": true,
        "restarted": [UNIT_MEDIA], "note": "sew-media restarted: calls in progress dropped", "warnings": warnings,
    }))
}

/// Leave the tunnel tier for `direct` (§6.5: public-IP boxes, the owner's
/// choice): the `turn-<label>` A record and cert, the node IP K2 Connect
/// sees, `relay_only = false`; then the voice lane stops.
pub fn move_out(env: &Env, st: VoiceState) -> R<Value> {
    sup::helper_check(env)?;
    if !helper_ok(env) {
        let hs = env.ops.helper_state();
        return Err(no("helper_outdated", 409, crate::root_helper::helper_needed("Leaving the voice tunnel", k2_root_proto::version::SEW_TUNNEL_MIN_PROTOCOL, &hs)));
    }
    let label = env.ops.box_label().ok_or_else(|| no("no_connect_label", 409, "no K2 Connect label"))?;
    let _l = sup::begin(env)?;
    let mut st = st;
    let (chain, key_pem, dns) = voice::issue_direct(env, &label, &mut st)?;
    voice::save(env, &mut st);
    let node_ip = voice::node_ip_of(&dns);
    let body = json!({
        "turn_tls": voice::TIER_DIRECT,
        "turn_domain": voice::turn_host(&label),
        "turn_advertise_port": k2_root_proto::sew::SEW_TURN_TLS_PORT,
        "node_ip": node_ip,
        "relay_only": false,
        "bind_v4": MEDIA_BIND_ANY,
        "turn_cert_pem": chain,
        "turn_key_pem": key_pem,
    });
    sew_media(env, &["media", "reconfigure", "restart-now"], &body, "media_reconfigure_failed")?;
    voice::update(env, |s| {
        s.turn_tier = Some(voice::TIER_DIRECT.to_string());
        s.node_ip = Some(node_ip.clone());
        s.node_ip_source = Some("control_plane".into());
    });
    env.ops.voice_lane(false).map_err(|e| no("voice_lane", 500, e))?;
    sup::changed("voice-tier");
    Ok(json!({
        "ok": true, "installed": true, "turnTier": voice::TIER_DIRECT, "from": TIER_TUNNEL, "changed": true,
        "turnHost": voice::turn_host(&label), "nodeIp": node_ip, "turnTlsPort": k2_root_proto::sew::SEW_TURN_TLS_PORT,
        "restarted": [UNIT_MEDIA], "note": "sew-media restarted: calls in progress dropped",
        "exposure": "Direct mode makes this box's address public: its IP is in DNS (turn-<label>.k2.dev), in public certificate logs, and visible to everyone who joins a call.",
    }))
}

/// Renewal in the tunnel tier (same key; no DNS, no node IP): a new chain
/// for `turn.<label>`, then `sew media turn-cert --when-idle`.
pub fn renew(env: &Env, st: &mut VoiceState) -> R<Value> {
    let label = label(env)?;
    let (chain, key_pem) = issue(env, &label, st)?;
    let body = json!({"cert_pem": chain, "key_pem": key_pem});
    sup::sew_run(env, &["media", "turn-cert"], Some(body.to_string().as_bytes())).map_err(|e| no("turn_cert_failed", 500, e))?;
    Ok(json!({"ok": true, "renewed": true, "certNotAfter": st.cert_not_after, "nodeIpMoved": null}))
}

#[cfg(all(test, unix))]
#[path = "voice_tunnel_tests.rs"]
mod tests;
