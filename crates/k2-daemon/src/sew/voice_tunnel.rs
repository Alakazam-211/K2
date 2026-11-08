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
//!   host candidate is loopback, reachable by its own TURN). On Sew 0.7.1 and
//!   later also `turn_trusted_cidrs` ([`TUNNEL_TRUSTED_CIDRS`], M7: the PROXY
//!   sources, where frpc-voice dials from). Not on an older Sew: an unknown key
//!   fails the install with exit 2, and its default is loopback anyway. A
//!   reconfigure keeps the list, so the way back to direct leaves it.
//! - **Direct mode's leftovers** ([`TunnelSew::clears`], 0.46.10, Sew 0.8.2):
//!   Sew's reconfigure merges, so a box that ran direct keeps its `node_ip`
//!   (refused on the loopback-bound tunnel tier: exit 78), its
//!   `turn_advertise_port = 5349` (clients would get 5349) and its `bind_v6 =
//!   "::"`. Every move to the tunnel tier (install, move in, rebind) sends
//!   `turn_advertise_port: 0` (the listener's port), `bind_v6: ""` and
//!   `clear_node_ip: true`, which the root helper (protocol 14,
//!   `namespaces.sewtunnelclear`) turns into `--node-ip ''` on Sew's argv:
//!   0.8.2 refuses `"node_ip": ""` in the JSON. A helper older than 14 is
//!   asked to update first; nothing is sent.
//! - Back in direct mode: `relay_only = false`, `bind_v4 = 0.0.0.0`,
//!   `turn_advertise_port = 5349`, the node IP. Sew's exit 78 is "config
//!   refused" ([`config_refusal`]): its last stderr line says why, and K2
//!   passes it on verbatim.
//! - **Served check** ([`served`]): through the relay, `turn.<label>.k2.dev:443`
//!   with that SNI, compared with the leaf K2 issued.

use serde_json::{json, Value};

use k2_root_proto::sew::{sew_takes_trusted_cidrs, MEDIA_BIND_ANY, MEDIA_BIND_LOOPBACK, SEW_EXIT_CONFIG_REFUSED, TUNNEL_TRUSTED_CIDRS, UNIT_MEDIA};

use super::supervisor::{self as sup, no, Env, Fail, R};
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

/// What the Sew running here takes on a move to the tunnel tier. From its
/// live `GET /mgmt/v1/status`, never the pin: a box mid-way between two pins
/// answers for what it actually runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSew {
    /// Its version, as `GET /mgmt/v1/status` reports it.
    pub version: String,
    /// `turn_trusted_cidrs` (Sew 0.7.1 on; an older Sew refuses the key).
    pub trusted: bool,
    /// The clears of direct mode's leftovers ([`TUNNEL_CLEARS_MIN_SEW`] on):
    /// `--node-ip ''`, `turn_advertise_port: 0`, `bind_v6: ""`.
    pub clears: bool,
}

/// The first Sew confirmed to clear direct mode's leftovers the way K2 asks
/// (Sew 0.8.2: an empty `--node-ip` beside the JSON is "no node IP"; `"bind_v6":
/// ""` and `"turn_advertise_port": 0` in it). An older Sew gets today's
/// tunnel keys only.
pub const TUNNEL_CLEARS_MIN_SEW: &str = "0.8.2";
/// The minimum this build checks against: the unit tests run K2's fixture
/// releases (0.0.x), which stand in for a Sew that clears.
#[cfg(not(test))]
const TUNNEL_CLEARS_GATE: &str = TUNNEL_CLEARS_MIN_SEW;
#[cfg(test)]
const TUNNEL_CLEARS_GATE: &str = "0.0.2";

/// Sew's live `GET /mgmt/v1/status`.
fn sew_status(env: &Env) -> R<Value> {
    sup::mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| no("mgmt_unreachable", 503, format!("Sew's status: {e}")))
}

/// [`TunnelSew`] from Sew's status. Refused (`helper_outdated`, nothing sent)
/// when this Sew takes the clears but the root helper predates them
/// (protocol 14): a 13 helper refuses the keys, and a move without them
/// fails on a box that ran direct.
pub fn tunnel_sew_from(env: &Env, status: &Value) -> R<TunnelSew> {
    let version = status.get("version").and_then(Value::as_str).unwrap_or("").to_string();
    let clears = at_least(&version, TUNNEL_CLEARS_GATE);
    if clears && !helper_clears_ok(env) {
        let hs = env.ops.helper_state();
        return Err(no(
            "helper_outdated",
            409,
            format!(
                "{} Moving voice onto the tunnel clears the settings direct mode left in Sew (node IP, TURN port, IPv6 \
                 bind), which this root helper can't pass on yet. Nothing changed.",
                crate::root_helper::helper_needed("Voice through the tunnel", k2_root_proto::version::SEW_TUNNEL_CLEAR_MIN_PROTOCOL, &hs)
            ),
        ));
    }
    Ok(TunnelSew { trusted: sew_takes_trusted_cidrs(&version), clears, version })
}

/// [`tunnel_sew_from`] with a fresh status read.
pub fn tunnel_sew(env: &Env) -> R<TunnelSew> {
    tunnel_sew_from(env, &sew_status(env)?)
}

/// Root helper protocol 12 with `namespaces.sewtunnel` (an older helper
/// refuses the new media keys).
pub fn helper_ok(env: &Env) -> bool {
    helper_has(env, k2_root_proto::version::SEW_TUNNEL_MIN_PROTOCOL, "sewtunnel")
}

/// Root helper protocol 14 with `namespaces.sewtunnelclear` (`bind_v6`,
/// `clear_node_ip`; a 13 helper refuses both keys).
pub fn helper_clears_ok(env: &Env) -> bool {
    helper_has(env, k2_root_proto::version::SEW_TUNNEL_CLEAR_MIN_PROTOCOL, "sewtunnelclear")
}

fn helper_has(env: &Env, min: u32, ns: &str) -> bool {
    let st = env.ops.helper_state();
    st.usable(min) && st.version_json.as_ref().and_then(|v| v.get("namespaces")).and_then(|n| n.get(ns)).and_then(Value::as_bool) == Some(true)
}

/// Everything the tunnel tier needs that K2 can't fix itself.
pub fn ready(env: &Env) -> R<()> {
    if !env.ops.tunnel_preview() {
        return Err(no(
            "voice_tunnel_preview_off",
            409,
            "voice through the tunnel is a preview: the owner turns it on (k2 settings set voiceTunnelPreview true, or K2_VOICE_TUNNEL=1 in the daemon's environment)",
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
    let status = sew_status(env)?;
    let features: Vec<&str> = status.get("features").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !features.contains(&FEATURE_TURN_RELAY_ONLY) {
        return Err(no(
            "sew_too_old",
            409,
            format!("this Sew has no relay-only TURN tier (feature {FEATURE_TURN_RELAY_ONLY}, Sew 0.7.0): voice stays where it is"),
        ));
    }
    tunnel_sew_from(env, &status)?;
    Ok(())
}

/// The Sew keys the tunnel tier sets (§6.6 M5/M7/M9), plus the cert pair,
/// plus (Sew 0.8.2 on) the three clears of direct mode's leftovers:
/// `turn_advertise_port: 0`, `bind_v6: ""` and `clear_node_ip` (the helper's
/// `--node-ip ''`). Sew refuses any other key: `voice_tunnel_tests` pin
/// this list.
pub fn media_keys(host: &str, chain: &str, key_pem: &str, sew: &TunnelSew) -> Value {
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
    if sew.trusted {
        v["turn_trusted_cidrs"] = json!(TUNNEL_TRUSTED_CIDRS);
    }
    if sew.clears && node_ip().is_none() {
        // The listener's own port: a leftover 5349 would reach clients'
        // TURN URLs (Sew 0.8.2).
        v["turn_advertise_port"] = json!(0);
        // IPv6 off (Sew would rewrite "::" itself once node_ip is empty).
        v["bind_v6"] = json!("");
        // The helper says it on argv: `--node-ip ''`.
        v["clear_node_ip"] = json!(true);
    }
    v
}

/// `sew media install [--reconfigure …]` through the helper. Sew's exit 78
/// (config refused) comes back as [`config_refusal`], with Sew's last
/// stderr line; any other non-zero exit as `code`.
pub fn sew_media(env: &Env, tail: &[&str], body: &Value, code: &'static str) -> R<Value> {
    let v = sup::helper_stdin(env, tail, body.to_string().as_bytes()).map_err(|e| no(code, 500, e))?;
    match sup::sew_exit(&v) {
        Some(0) | None => Ok(v),
        Some(n) => Err(exit_refusal(tail, n, &v, code)),
    }
}

/// A non-zero `sew media install` exit, from the helper's answer.
pub fn exit_refusal(tail: &[&str], exit: i64, v: &Value, code: &'static str) -> Fail {
    let message = sup::sew_message(v);
    if exit != SEW_EXIT_CONFIG_REFUSED {
        return no(code, 500, format!("sew {}: exit {exit}: {message}", tail.join(" ")));
    }
    let line = last_line(v["stderrTail"].as_str().unwrap_or("")).or_else(|| last_line(&message)).unwrap_or_else(|| message.clone());
    config_refusal(&line)
}

/// The last non-empty line of `text`, trimmed.
fn last_line(text: &str) -> Option<String> {
    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// Sew's exit 78 (`sew-media --check-config` refused the settings; Sew
/// 0.8.2 gives no machine-readable reason) from its last stderr line, which
/// goes into the hint verbatim. Named only when the line says so:
/// `sew_tunnel_bind_refused` when it names `bind_v6`,
/// `sew_relay_only_refused` when it is about relay-only and passthrough;
/// anything else is `sew_config_refused`. Nothing changed in each case.
pub fn config_refusal(line: &str) -> Fail {
    let x = SEW_EXIT_CONFIG_REFUSED;
    let (code, hint) = if line.contains("bind_v6") {
        (
            "sew_tunnel_bind_refused",
            format!(
                "Sew refused the tunnel tier's settings (exit {x}): voice.ops.bind_v6 in Sew's media settings is set by \
                 hand to an address that isn't loopback. Voice through the tunnel allows only \"\" (IPv6 off) or \"::1\". \
                 Set it back to \"\" or remove it, then run k2 hostchat media rebind. Nothing changed. Sew said: {line}"
            ),
        )
    } else if (line.contains("relay_only") || line.contains("relay-only")) && line.contains("passthrough") {
        (
            "sew_relay_only_refused",
            format!(
                "Sew refused relay-only TURN for this TURN tier (exit {x}: relay-only needs the passthrough tier, never \
                 plain or off). Nothing changed. Sew said: {line}"
            ),
        )
    } else {
        ("sew_config_refused", format!("Sew refused the voice settings (exit {x}, config refused). Nothing changed. Sew said: {line}"))
    };
    let mut f = no(code, 409, hint);
    f.extra = Some(json!({"sewExit": x, "sewSaid": line}));
    f
}

/// On a Sew below [`TUNNEL_CLEARS_MIN_SEW`] (no clears sent), a refusal that
/// names the node IP also says how to get past it.
fn note_old_sew(mut f: Fail, sew: &TunnelSew) -> Fail {
    if f.code == "sew_config_refused" && !sew.clears && f.hint.contains("node_ip") {
        f.hint.push_str(&format!(
            " Sew {} can't clear the node IP direct mode left: run k2 hostchat upgrade (Sew {TUNNEL_CLEARS_MIN_SEW} or later), then try again.",
            if sew.version.is_empty() { "?" } else { &sew.version }
        ));
    }
    f
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
    // `token` is a required field in the relay's CertRequest (k2-connect
    // control-plane cert.rs): without it the relay answers 400 before any
    // auth. Send it empty, as the direct tier does (voice.rs); the relay
    // authenticates this mode by the tunnel's own identity (K2 Web, T4 11-04).
    let body = json!({"subdomain": label, "mode": CERT_MODE, "csr": csr, "token": ""});
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
        Ok(()) => c.push(check("tunnel_tier", "ok", "voice through the tunnel: preview on, root helper ready, Sew relay-only".into(), "")),
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
    let sew = tunnel_sew(env)?;
    let mut body = media_keys(&h, &chain, &key_pem, &sew);
    body["public_url"] = json!(url);
    body["udp_port"] = json!(k2_root_proto::sew::SEW_MEDIA_UDP_PORT);
    body["tcp_port"] = json!(k2_root_proto::sew::SEW_MEDIA_TCP_PORT);
    body["sysctl"] = json!("apply");
    let out = sew_media(env, &["media", "install"], &body, "media_install_failed").map_err(|f| note_old_sew(f, &sew))?;
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
    let sew = tunnel_sew(env)?;
    let body = media_keys(&h, &chain, &key_pem, &sew);
    sew_media(env, &["media", "reconfigure", "restart-now"], &body, "media_reconfigure_failed").map_err(|f| note_old_sew(f, &sew))?;
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

// ── the tunnel tier's IPv6 bind (Sew 0.7.4 on, 11-05) ─────────────────────

/// The first Sew whose `sew media install --reconfigure` rewrites a tunnel
/// box's stored `voice.ops.bind_v6 = "::"` to `""` (IPv6 off).
pub const TUNNEL_BIND_MIN_SEW: &str = "0.7.4";
/// The minimum this build checks against: the unit tests run K2's fixture
/// releases (0.0.x), which stand in for a Sew that has the step.
#[cfg(not(test))]
const TUNNEL_BIND_GATE: &str = TUNNEL_BIND_MIN_SEW;
#[cfg(test)]
const TUNNEL_BIND_GATE: &str = "0.0.2";
/// `sew doctor`'s check that the tunnel tier's engine listens on no public
/// IPv6 address.
pub const DOCTOR_TUNNEL_BIND: &str = "media.tunnel_bind";
/// Sew's warning when the reconfigure rewrote `"::"`.
pub const WARNING_BIND_V6_REWRITTEN: &str = "tunnel_bind_v6_rewritten";

/// Does this Sew (`GET /mgmt/v1/status` version, a build suffix allowed)
/// rewrite the tunnel tier's IPv6 bind on a reconfigure?
pub fn sew_rewrites_tunnel_bind(version: &str) -> bool {
    at_least(version, TUNNEL_BIND_GATE)
}

/// `version` (a build suffix allowed) is `min` or later.
pub fn at_least(version: &str, min: &str) -> bool {
    let plain = version.split('+').next().unwrap_or(version);
    matches!(k2_root_proto::sew::version_cmp(plain, min), Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater))
}

/// `sew doctor`'s [`DOCTOR_TUNNEL_BIND`] check from its answer (as the helper
/// wraps it), or a `missing` row when Sew didn't report one.
pub fn tunnel_bind_check(doctor: &Value) -> Value {
    doctor
        .pointer("/sew/checks")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|c| c["id"] == DOCTOR_TUNNEL_BIND).cloned())
        .unwrap_or_else(|| json!({"id": DOCTOR_TUNNEL_BIND, "status": "missing", "detail": "sew doctor did not report it"}))
}

/// Sew's steps 3 and 4 on a tunnel box (Sew 11-05): `sew media install
/// --reconfigure --restart` with the tunnel tier's own keys, which rewrites
/// a stored `bind_v6 = "::"` to `""` (an upgrade never rewrites Sew's media
/// settings, so a box installed before Sew 0.7.4 keeps listening on every IPv6
/// address; from Sew 0.8.2 K2 also sends `""` itself, with the other clears),
/// then `sew doctor`'s `media.tunnel_bind` must be ok. The caller holds the
/// latch. Restarts sew-media: calls in progress drop. Sew's exit 78 is
/// [`config_refusal`] (a hand-set, non-loopback `bind_v6` on an older Sew is
/// `sew_tunnel_bind_refused`).
pub fn rebind_steps(env: &Env, st: &VoiceState) -> R<Value> {
    let sew = tunnel_sew(env)?;
    let (h, chain, key_pem) = voice::held_material(env, st)?;
    let body = media_keys(&h, &chain, &key_pem, &sew);
    let v = sew_media(env, &["media", "reconfigure", "restart-now"], &body, "media_reconfigure_failed").map_err(|f| note_old_sew(f, &sew))?;
    let rewritten = v.pointer("/sew/warnings").is_some_and(|w| w.to_string().contains(WARNING_BIND_V6_REWRITTEN));
    let doctor = sup::helper_json(env, &["doctor"]).map_err(|e| no("doctor_failed", 500, format!("sew-media was reconfigured, but sew doctor didn't answer: {e}")))?;
    let check = tunnel_bind_check(&doctor);
    if check["status"] != "ok" {
        let say = |k: &str| check[k].as_str().unwrap_or("").to_string();
        let mut f = no(
            "tunnel_bind_not_ok",
            500,
            format!(
                "sew-media was reconfigured, but sew doctor's {DOCTOR_TUNNEL_BIND} is {}: {} {}",
                check["status"].as_str().unwrap_or("?"),
                say("detail"),
                say("hint")
            )
            .trim_end()
            .to_string(),
        );
        f.extra = Some(json!({"tunnelBind": check, "rewritten": rewritten}));
        return Err(f);
    }
    Ok(json!({"rewritten": rewritten, "tunnelBind": check}))
}

/// `POST /cli/sew/media/rebind` (`k2 hostchat media rebind`): run Sew's steps 3
/// and 4 on an already-upgraded tunnel box. The direct tier listens publicly on
/// purpose (Sew: skip it there). Refused on a Sew older than 0.7.4, whose
/// reconfigure keeps `"::"`: `k2 hostchat upgrade` first, which runs it too.
pub fn rebind(env: &Env) -> R<Value> {
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Chat / Voice Hosting runs on Linux boxes only"));
    }
    let st = voice::load(env);
    if !st.installed {
        return Err(no("voice_not_installed", 409, "voice is not installed: k2 hostchat media install"));
    }
    let tier = voice::tier(&st).to_string();
    if tier != TIER_TUNNEL {
        return Ok(json!({
            "ok": true, "turnTier": tier, "changed": false,
            "hint": "only voice through the tunnel needs this: the direct tier listens on the box's public addresses on purpose",
        }));
    }
    sup::helper_check(env)?;
    if !helper_ok(env) {
        let hs = env.ops.helper_state();
        return Err(no("helper_outdated", 409, crate::root_helper::helper_needed("Closing the voice tunnel's IPv6 bind", k2_root_proto::version::SEW_TUNNEL_MIN_PROTOCOL, &hs)));
    }
    let status = sup::mgmt(env, "GET", "/mgmt/v1/status", None).map_err(|e| no("mgmt_unreachable", 503, format!("Sew's version: {e}")))?;
    let version = status.get("version").and_then(Value::as_str).unwrap_or("").to_string();
    if !sew_rewrites_tunnel_bind(&version) {
        return Err(no(
            "sew_upgrade_needed",
            409,
            format!(
                "Sew {version} keeps its IPv6 bind on a reconfigure; Sew {TUNNEL_BIND_MIN_SEW} and later close it. Run k2 \
                 hostchat upgrade: it does this step itself on a tunnel box"
            ),
        ));
    }
    let _l = sup::begin(env)?;
    let mut out = rebind_steps(env, &st)?;
    sup::changed("voice-tier");
    out["ok"] = json!(true);
    out["turnTier"] = json!(TIER_TUNNEL);
    out["changed"] = json!(true);
    out["restarted"] = json!([UNIT_MEDIA]);
    out["note"] = json!("sew-media restarted: calls in progress dropped");
    Ok(out)
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
