//! Voice through the tunnel (0.46.8, S9a) and the media-path loop over a
//! fake box, a fake K2 Connect control plane and a fake Sew mgmt socket.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;
use crate::sew::media_path::{self, Machine, PathState};
use crate::sew::ops::fake::{helper_ok, FakeOps};
use crate::sew::ops::CpError;
use crate::sew::secrets::MemSecrets;
use crate::sew::supervisor::{enable_begin, enable_machine, EnableRequest, Fail};
use crate::sew::voice::{install_begin, install_machine, load, set_tier, set_tier_named, InstallRequest, TIER_DIRECT};

const TARBALL: &str = "/home/k2/sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz";
const MEDIA: &str = "/home/k2/sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz";

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

struct T {
    ops: FakeOps,
    secrets: MemSecrets,
    latch: AtomicBool,
    token: Mutex<Option<String>>,
    _db: k2_core::db::TestDbGuard,
}

/// A box with chat running, root helper protocol 14 (sewtunnel and
/// sewtunnelclear), Sew listing `turn_relay_only` and `voice_media_path`, the
/// preview on.
fn t() -> T {
    let ops = FakeOps::new();
    ops.files.lock().unwrap().insert(TARBALL.into(), fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    ops.files.lock().unwrap().insert(MEDIA.into(), fixture("sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    let t = T { ops, secrets: MemSecrets::default(), latch: AtomicBool::new(false), token: Mutex::new(None), _db: k2_core::db::scoped_for_test() };
    let env = t.env();
    let req = EnableRequest { tarball_path: Some(TARBALL.into()), ..Default::default() };
    let latch = match enable_begin(&env, &req) {
        Ok(Ok(l)) => l,
        Ok(Err(v)) => panic!("no machine: {v}"),
        Err(f) => panic!("{}: {}", f.code, f.hint),
    };
    enable_machine(&env, &req).expect("enable");
    drop(latch);
    t.helper(14, true);
    t.features(&[FEATURE_TURN_RELAY_ONLY, media_path::FEATURE_VOICE_MEDIA_PATH]);
    t.ops.set_tier(Some("pro"));
    *t.ops.tunnel_preview.lock().unwrap() = true;
    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();
    t
}

impl T {
    fn env(&self) -> Env<'_> {
        Env { ops: &self.ops, secrets: &self.secrets, latch: &self.latch, token: &self.token }
    }
    /// `sewtunnelclear` comes with `sewtunnel` from protocol 14 on.
    fn helper(&self, protocol: u32, sewtunnel: bool) {
        let mut h = helper_ok(protocol, true, k2_root_proto::sew::pinned().version);
        h.version_json.as_mut().unwrap()["namespaces"]["sewtunnel"] = json!(sewtunnel);
        h.version_json.as_mut().unwrap()["namespaces"]["sewtunnelclear"] = json!(sewtunnel && protocol >= 14);
        *self.ops.helper.lock().unwrap() = h;
    }
    fn features(&self, f: &[&str]) {
        self.ops.mgmt_status.lock().unwrap()["features"] = json!(f);
    }
    fn install(&self) -> Result<Value, Fail> {
        let env = self.env();
        let req = InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() };
        let latch = install_begin(&env, &req)?;
        let r = install_machine(&env, &req);
        drop(latch);
        r
    }
    fn stdin_of(&self, argv: &str) -> Value {
        let calls = self.ops.calls.lock().unwrap();
        let c = calls.iter().rev().find(|c| c.argv == argv).unwrap_or_else(|| panic!("no `{argv}` call"));
        serde_json::from_slice(c.stdin.as_ref().expect("stdin")).unwrap()
    }
    fn cp_body(&self, call: &str) -> Value {
        let calls = self.ops.cp_calls.lock().unwrap();
        calls.iter().rev().find(|(c, _)| c == call).and_then(|(_, b)| b.clone()).unwrap_or_else(|| panic!("no `{call}`"))
    }
}

fn csr_sans(csr_pem: &str) -> Vec<String> {
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::FromDer;
    let mut reader = csr_pem.as_bytes();
    let (pem, _) = x509_parser::pem::Pem::read(std::io::Cursor::new(&mut reader)).unwrap();
    let (_, csr) = x509_parser::certification_request::X509CertificationRequest::from_der(&pem.contents).unwrap();
    let mut sans = vec![];
    for ext in csr.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext {
            for n in &san.general_names {
                if let GeneralName::DNSName(d) = n {
                    sans.push(d.to_string());
                }
            }
        }
    }
    sans
}

const TUNNEL_INSTALL: &str = "engine sew media install";
const RECONF: &str = "engine sew media reconfigure restart-now";

/// A body's keys, sorted (`serde_json` maps may not be).
fn keys_of(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
    k.sort();
    k
}

/// The test Sew refuses what real Sew refuses, with its exit 2: this is what
/// keeps the key lists above honest.
#[test]
fn the_test_sew_refuses_a_key_real_sew_would() {
    let t = t();
    for body in [
        json!({"public_url": "wss://voice.lumon.k2.dev", "turn_trusted_cidrs": ["127.0.0.1/32"]}),
        json!({"public_url": "wss://voice.lumon.k2.dev", "tier": "passthrough"}),
        json!({"public_url": "wss://voice.lumon.k2.dev", "relay_only": "true"}),
    ] {
        let e = sew_media(&t.env(), &["media", "install"], &body, "media_install_failed").expect_err("refused");
        assert_eq!(e.code, "media_install_failed", "{body}");
        assert!(e.hint.contains("exit 2"), "{}", e.hint);
    }
}

/// Sew 0.7.1 and later get the tunnel tier's PROXY sources, on a first install
/// and on a move in, as the exact pair the helper takes; an older Sew never
/// sees the key (it would fail the install with exit 2).
#[test]
fn the_trusted_cidrs_go_to_sew_0_7_1_and_later_only() {
    let pair = json!(["127.0.0.1/32", "::1/128"]);
    for v in ["0.7.1", "0.7.1+g77c5e1f", "0.7.2", "0.8.0"] {
        let t = t();
        *t.ops.mgmt_version.lock().unwrap() = Some(v.into());
        t.install().unwrap_or_else(|e| panic!("{v}: {}: {}", e.code, e.hint));
        let m = t.stdin_of(TUNNEL_INSTALL);
        assert_eq!(m["turn_trusted_cidrs"], pair, "{v}: {m}");
        let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(m).expect("the helper's MediaRequest parses it");
        req.validate().expect("valid for protocol 12");
        assert_eq!(
            req.sew_inputs(Some("/s/m.tgz"), None)["turn_trusted_cidrs"],
            pair,
            "{v}: and it reaches the JSON Sew reads"
        );
    }
    for v in ["0.7.0", "0.6.1", "not-a-version"] {
        let t = t();
        *t.ops.mgmt_version.lock().unwrap() = Some(v.into());
        t.install().unwrap_or_else(|e| panic!("{v}: {}: {}", e.code, e.hint));
        let m = t.stdin_of(TUNNEL_INSTALL);
        assert!(m.get("turn_trusted_cidrs").is_none(), "{v}: {m}");
    }

    // Moving an installed engine onto the tunnel tier sends it too; the way
    // back to direct leaves it (a reconfigure keeps it, direct doesn't read it).
    let t = t();
    *t.ops.mgmt_version.lock().unwrap() = Some("0.7.1".into());
    *t.ops.tunnel_preview.lock().unwrap() = false;
    t.install().expect("direct install");
    let direct = t.stdin_of(TUNNEL_INSTALL);
    assert!(direct.get("turn_trusted_cidrs").is_none(), "the direct tier never sends it: {direct}");
    *t.ops.tunnel_preview.lock().unwrap() = true;
    t.ops.clear();
    set_tier_named(&t.env(), "tunnel").expect("to tunnel");
    assert_eq!(t.stdin_of(RECONF)["turn_trusted_cidrs"], pair);
    t.ops.clear();
    set_tier_named(&t.env(), TIER_DIRECT).expect("to direct");
    let back = t.stdin_of(RECONF);
    assert!(back.get("turn_trusted_cidrs").is_none(), "{back}");
}

/// A daemon and helper on different Sew pins never reach Sew: the tunnel
/// install and the move in both stop at the helper check and say what to do.
#[test]
fn a_helper_on_another_sew_pin_stops_the_tunnel_tier_cleanly() {
    let t = t();
    *t.ops.tunnel_preview.lock().unwrap() = false;
    t.install().expect("direct install");
    *t.ops.tunnel_preview.lock().unwrap() = true;
    t.ops.clear();
    let mut h = helper_ok(14, true, "0.6.1");
    h.version_json.as_mut().unwrap()["namespaces"]["sewtunnel"] = json!(true);
    h.version_json.as_mut().unwrap()["namespaces"]["sewtunnelclear"] = json!(true);
    *t.ops.helper.lock().unwrap() = h;
    let e = set_tier_named(&t.env(), "tunnel").expect_err("another pin");
    assert_eq!(e.code, "helper_outdated", "{}", e.hint);
    assert!(e.hint.contains("pins Sew 0.6.1") && e.hint.contains("k2 root-helper update"), "{}", e.hint);
    let calls: Vec<String> = t.ops.calls.lock().unwrap().iter().map(|c| c.argv.clone()).collect();
    assert!(!calls.iter().any(|c| c.contains("media")), "nothing ran: {calls:?}");
}

/// S9a end to end: no TURN DNS, a `turn-tunnel` cert for `turn.<label>`
/// with the bare label, Sew's M5/M7/M9 keys and no node IP, the voice lane
/// armed, the served check through the relay.
#[test]
fn a_tunnel_install_publishes_no_ip_and_hands_sew_relay_only_passthrough() {
    let t = t();
    let out = t.install().expect("tunnel install");
    assert_eq!(out["turnTier"], "tunnel");
    assert_eq!(out["turnHost"], "turn.lumon.k2.dev");
    assert_eq!(out["nodeIp"], Value::Null);
    assert_eq!(out["relayOnly"], true);
    assert_eq!(out["iceServers"], json!(["turns:turn.lumon.k2.dev:443?transport=tcp"]));
    assert_eq!(out["turnTls"]["served"], "ok", "{out}");

    let cp = t.ops.cp_log();
    assert!(!cp.iter().any(|c| c.contains("/turn/dns")), "no DNS in the tunnel tier: {cp:?}");
    let cert = t.cp_body("POST /cert");
    assert_eq!(cert["mode"], "turn-tunnel");
    assert_eq!(cert["subdomain"], "lumon", "the bare label");
    assert_eq!(csr_sans(cert["csr"].as_str().unwrap()), vec!["turn.lumon.k2.dev".to_string()]);

    let m = t.stdin_of(TUNNEL_INSTALL);
    assert_eq!(m["turn_tls"], "passthrough");
    assert_eq!(m["turn_domain"], "turn.lumon.k2.dev");
    assert_eq!(m["relay_only"], true);
    assert_eq!(m["bind_v4"], "127.0.0.1", "media sockets off the network");
    assert!(m.get("node_ip").is_none(), "no node IP key: the helper clears it on argv: {m}");
    // The three clears of direct mode's leftovers (Sew 0.8.2), on a first
    // install too: the listener's own port, IPv6 off, no node IP.
    assert_eq!((m["turn_advertise_port"].as_u64(), m["bind_v6"].as_str(), m["clear_node_ip"].as_bool()), (Some(0), Some(""), Some(true)), "{m}");
    // Sew's `MediaInstallInputs` refuses anything else (exit 2): the exact keys.
    assert_eq!(
        keys_of(&m),
        [
            "bind_v4",
            "bind_v6",
            "clear_node_ip",
            "public_url",
            "relay_only",
            "sysctl",
            "tcp_port",
            "turn_advertise_port",
            "turn_cert_pem",
            "turn_domain",
            "turn_key_pem",
            "turn_tls",
            "udp_port"
        ],
        "{m}"
    );
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(m).expect("the helper's MediaRequest parses it");
    req.validate().expect("valid for protocol 14");
    let argv = t.ops.media_argv.lock().unwrap().last().cloned().expect("Sew ran");
    assert_eq!(argv, ["media", "install", "--config-json", "/stage/media.json", "--node-ip", ""], "Sew gets --node-ip ''");

    assert_eq!(*t.ops.lane_calls.lock().unwrap(), vec![true], "the voice lane is armed");
    assert_eq!(t.ops.served_connects.lock().unwrap().last().map(String::as_str), Some("turn.lumon.k2.dev"));
    assert_eq!(t.ops.served_ports.lock().unwrap().last(), Some(&443));
    let st = load(&t.env());
    assert_eq!((st.installed, voice::tier(&st), st.node_ip.as_deref()), (true, "tunnel", None));
}

#[test]
fn the_tunnel_tier_needs_the_preview_helper_12_and_a_relay_only_sew() {
    // Preview off: today's direct install, untouched.
    let t = t();
    *t.ops.tunnel_preview.lock().unwrap() = false;
    let out = t.install().expect("direct install");
    assert_eq!(out["turnHost"], "turn-lumon.k2.dev");
    assert!(t.ops.lane_calls.lock().unwrap().is_empty(), "no voice lane without the preview");
    assert!(t.stdin_of(TUNNEL_INSTALL).get("relay_only").is_none(), "protocol-11 clean");

    let refused = |t: &T, code: &str| {
        let e = t.install().expect_err("refused");
        assert_eq!(e.code, "preflight_failed", "{}", e.hint);
        let report = e.extra.unwrap()["report"].clone();
        let tt = report.as_array().unwrap().iter().find(|c| c["id"] == "tunnel_tier").cloned().unwrap();
        assert_eq!((tt["status"].as_str(), tt["detail"].as_str()), (Some("fail"), Some(code)), "{report}");
    };
    let t2 = self::t();
    t2.helper(11, false);
    refused(&t2, "helper_outdated");
    let t3 = self::t();
    t3.features(&[media_path::FEATURE_VOICE_MEDIA_PATH]);
    refused(&t3, "sew_too_old");
    assert!(t3.ops.cp_log().is_empty(), "nothing issued");
    // PRD §7.3: a 2–4 GiB box runs chat only (lane E's floor).
    let t4 = self::t();
    *t4.ops.mem.lock().unwrap() = Some(3 * crate::sew::ops::fake::GIB);
    refused(&t4, "voice_floor");
    assert_eq!(set_tier_named(&t4.env(), "tunnel").unwrap_err().code, "voice_not_installed");
}

#[test]
fn below_pro_the_broker_says_tier_required_and_k2_says_not_pro() {
    let t = t();
    t.ops.queue_cp("POST /cert", Err(CpError { status: 403, code: "tier_required".into(), detail: "nested names need Pro".into() }));
    let e = t.install().expect_err("not Pro");
    assert_eq!(e.code, "not_pro", "{}", e.hint);
    assert_eq!(e.status, 403);
}

/// Sew's exit 78 (relay-only with tier plain/off) comes back named.
#[test]
fn sews_relay_only_refusal_is_named() {
    let t = t();
    t.ops.queue(TUNNEL_INSTALL, Ok(json!({"sewExit": 78, "reason": "relay_only requires turn tier direct, external or passthrough"})));
    let e = t.install().expect_err("Sew refused");
    assert_eq!(e.code, "sew_relay_only_refused", "{}", e.hint);
    assert!(e.hint.contains("exit 78"), "{}", e.hint);
}

#[test]
fn renewal_in_the_tunnel_tier_skips_dns_and_the_node_ip() {
    let t = t();
    t.install().expect("install");
    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();
    let v = voice::renew(&t.env(), true).expect("renew");
    assert_eq!(v["renewed"], true);
    assert_eq!(t.ops.cp_log(), vec!["POST /cert".to_string()], "only the cert");
    assert_eq!(t.cp_body("POST /cert")["mode"], "turn-tunnel");
    let calls: Vec<String> = t.ops.calls.lock().unwrap().iter().map(|c| c.argv.clone()).collect();
    assert!(calls.contains(&"engine sew media turn-cert".to_string()), "{calls:?}");
    assert!(!calls.iter().any(|c| c.contains("reconfigure")), "no node-IP reconfigure: {calls:?}");
}

/// `k2 hostchat media tier`: direct → tunnel drops the old A record and
/// arms the lane; tunnel → direct gets DNS, a node IP and relay-only off,
/// and stops the lane. front443's set_tier leaves the tunnel tier alone.
#[test]
fn tier_moves_both_ways_and_the_front_never_moves_the_tunnel() {
    let t = t();
    *t.ops.tunnel_preview.lock().unwrap() = false;
    t.install().expect("direct install");
    *t.ops.tunnel_preview.lock().unwrap() = true;
    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();

    let v = set_tier_named(&t.env(), "tunnel").expect("to tunnel");
    assert_eq!((v["turnTier"].as_str(), v["changed"].as_bool()), (Some("tunnel"), Some(true)), "{v}");
    let r = t.stdin_of(RECONF);
    assert_eq!((r["turn_tls"].as_str(), r["turn_domain"].as_str(), r["relay_only"].as_bool()), (Some("passthrough"), Some("turn.lumon.k2.dev"), Some(true)));
    assert_eq!(r["bind_v4"], "127.0.0.1");
    // Exactly the tunnel keys plus the three clears (Sew 0.8.2).
    assert_eq!(
        keys_of(&r),
        ["bind_v4", "bind_v6", "clear_node_ip", "relay_only", "turn_advertise_port", "turn_cert_pem", "turn_domain", "turn_key_pem", "turn_tls"],
        "{r}"
    );
    assert_eq!((r["turn_advertise_port"].as_u64(), r["bind_v6"].as_str(), r["clear_node_ip"].as_bool()), (Some(0), Some(""), Some(true)), "{r}");
    assert!(t.ops.cp_log().contains(&"DELETE /turn/dns/lumon".to_string()), "the box IP leaves DNS: {:?}", t.ops.cp_log());
    assert_eq!(*t.ops.lane_calls.lock().unwrap(), vec![true]);

    // The front's own convergence never moves voice off the tunnel.
    assert_eq!(set_tier(&t.env(), false).unwrap()["changed"], false);
    assert_eq!(set_tier(&t.env(), true).unwrap()["changed"], false);
    assert_eq!(voice::tier(&load(&t.env())), "tunnel");

    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();
    let v = set_tier_named(&t.env(), TIER_DIRECT).expect("to direct");
    assert_eq!(v["turnTier"], "direct");
    let r = t.stdin_of(RECONF);
    assert_eq!((r["turn_tls"].as_str(), r["turn_domain"].as_str(), r["relay_only"].as_bool()), (Some("direct"), Some("turn-lumon.k2.dev"), Some(false)));
    assert_eq!(r["node_ip"], "203.0.113.7");
    assert_eq!(r["bind_v4"], "0.0.0.0", "media sockets back on the network");
    assert_eq!(r["turn_advertise_port"], 5349, "the direct tier's TURN port");
    assert_eq!(
        keys_of(&r),
        ["bind_v4", "node_ip", "relay_only", "turn_advertise_port", "turn_cert_pem", "turn_domain", "turn_key_pem", "turn_tls"],
        "{r}"
    );
    assert!(t.ops.cp_log().contains(&"PUT /turn/dns/lumon".to_string()));
    assert_eq!(*t.ops.lane_calls.lock().unwrap(), vec![true, false], "the lane stops");
    assert_eq!(set_tier_named(&t.env(), "front").unwrap_err().code, "bad_request");
}

#[test]
fn uninstall_in_the_tunnel_tier_deletes_no_dns_and_stops_the_lane() {
    let t = t();
    t.install().expect("install");
    t.ops.cp_calls.lock().unwrap().clear();
    voice::uninstall(&t.env()).expect("uninstall");
    assert!(!t.ops.cp_log().iter().any(|c| c.contains("/turn/dns")), "{:?}", t.ops.cp_log());
    assert_eq!(t.ops.lane_calls.lock().unwrap().last(), Some(&false));
}

#[test]
fn the_tunnel_preflight_checks_the_tunnel_not_public_ports() {
    let t = t();
    *t.ops.e2e.lock().unwrap() = false;
    t.ops.set_tier(Some("single"));
    let c = voice::preflight(&t.env());
    let get = |id: &str| c.iter().find(|x| x["id"] == id).cloned().unwrap_or_else(|| panic!("no {id}: {c:?}"));
    assert_eq!(get("tunnel_tier")["status"], "ok");
    assert_eq!(get("e2e")["status"], "fail");
    assert_eq!(get("pro")["status"], "fail");
    assert_eq!(get("tunnel_label")["detail"], "TURN host turn.lumon.k2.dev");
    assert!(!get("ports")["detail"].as_str().unwrap().contains("7881"), "no public ports: {c:?}");
    *t.ops.confirmed.lock().unwrap() = None;
    let c = voice::preflight(&t.env());
    assert_eq!(c.iter().find(|x| x["id"] == "tunnel_label").unwrap()["status"], "fail");
}

// ── the media-path loop against the fake Sew ─────────────────────────────

const ROUTE_CALL: &str = "POST /mgmt/v1/voice/media-path";

fn route_bodies(t: &T) -> Vec<Value> {
    let calls = t.ops.mgmt_calls.lock().unwrap().clone();
    let bodies = t.ops.mgmt_bodies.lock().unwrap().clone();
    calls.iter().zip(bodies).filter(|(c, _)| c.as_str() == ROUTE_CALL).map(|(_, b)| b.expect("a body")).collect()
}

fn queue_route_ok(t: &T, n: usize) {
    let mut q = t.ops.mgmt_answers.lock().unwrap();
    for _ in 0..n {
        q.entry(ROUTE_CALL.to_string()).or_default().push_back(Ok(json!({"state": "x", "since": 0, "ended_calls": 0})));
    }
}

/// §6.6 M11 over the fake Sew: 20 s of lane-down sends one `down` with
/// reason `voice_lane_down`; the lane coming back sends `up` at once.
#[test]
fn the_loop_tells_sew_down_after_20_s_and_up_at_once() {
    let t = t();
    t.install().expect("install");
    *t.ops.lane_comes_up.lock().unwrap() = false;
    t.ops.clear();
    t.ops.mgmt_bodies.lock().unwrap().clear();
    queue_route_ok(&t, 4);
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    let s = media_path::drive(&t.env(), &mut m, true, t0);
    assert_eq!(s.state, PathState::Reconnecting);
    assert_eq!(media_path::drive(&t.env(), &mut m, true, t0 + Duration::from_secs(19)).state, PathState::Reconnecting);
    assert!(route_bodies(&t).is_empty(), "nothing to Sew under 20 s");
    let s = media_path::drive(&t.env(), &mut m, true, t0 + Duration::from_secs(20));
    assert_eq!(s.state, PathState::Down);
    assert_eq!(route_bodies(&t), vec![json!({"state": "down", "reason": "voice_lane_down"})]);
    *t.ops.lane_comes_up.lock().unwrap() = true;
    let s = media_path::drive(&t.env(), &mut m, true, t0 + Duration::from_secs(21));
    assert_eq!(s.state, PathState::Up);
    assert_eq!(route_bodies(&t).last(), Some(&json!({"state": "up"})));
    assert_eq!(route_bodies(&t).len(), 2);
}

/// Gated on Sew's `voice_media_path`: without it K2 calls nothing (and
/// still tracks the state for its own join gate).
#[test]
fn without_the_feature_the_loop_never_calls_the_route() {
    let t = t();
    t.install().expect("install");
    *t.ops.lane_comes_up.lock().unwrap() = false;
    t.ops.clear();
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    media_path::drive(&t.env(), &mut m, false, t0);
    let s = media_path::drive(&t.env(), &mut m, false, t0 + Duration::from_secs(25));
    assert_eq!(s.state, PathState::Down);
    assert!(!t.ops.mgmt_calls.lock().unwrap().iter().any(|c| c.contains("media-path")));
}

/// With the preview off the loop does nothing to the lane at all.
#[test]
fn with_the_preview_off_the_loop_leaves_the_lane_alone() {
    let t = t();
    t.install().expect("install");
    *t.ops.tunnel_preview.lock().unwrap() = false;
    t.ops.lane_calls.lock().unwrap().clear();
    *t.ops.lane_armed.lock().unwrap() = false;
    let t0 = Instant::now();
    let mut m = Machine::new(t0);
    let s = media_path::drive(&t.env(), &mut m, true, t0);
    assert_eq!(s.state, PathState::Off);
    assert!(t.ops.lane_calls.lock().unwrap().is_empty());
}

// ── the tunnel tier's IPv6 bind (Sew 11-05) ──────────────────────────────

const MEDIA_UPGRADE: &str = "engine sew media upgrade";
const DOCTOR: &str = "engine sew doctor";

/// Sew `0.0.1` on disk with voice installed, then `k2 hostchat upgrade` to
/// the pin with both tarballs. Returns the upgrade's final progress.
fn upgrade_from_old(t: &T) -> Value {
    *t.ops.on_disk.lock().unwrap() = Some("0.0.1".into());
    crate::sew::store::update(1, |r| r.installed_version = Some("0.0.1".into())).unwrap();
    t.ops.clear();
    let req = crate::sew::supervisor::UpgradeRequest {
        tarball_path: Some(TARBALL.into()),
        media_tarball_path: Some(MEDIA.into()),
        dry_run: false,
        accept_binary: None,
    };
    let env = t.env();
    let (l, from) = crate::sew::supervisor::upgrade_begin(&env, &req).unwrap().unwrap();
    let p = crate::sew::supervisor::upgrade_machine(&env, &req, &from);
    drop(l);
    p
}

fn position(effects: &[String], argv: &str) -> usize {
    effects.iter().position(|e| e == argv).unwrap_or_else(|| panic!("no `{argv}` in {effects:?}"))
}

/// A tunnel box: Sew's order, upgrade → media upgrade → `sew media install
/// --reconfigure --restart` with the tunnel's own keys → `sew doctor`.
#[test]
fn a_tunnel_upgrade_closes_the_ipv6_bind_after_the_engine_upgrade() {
    let t = t();
    t.install().expect("install");
    let p = upgrade_from_old(&t);
    assert_eq!(p["result"], "done", "{p}");
    assert!(p["steps"].get("tunnel-bind").is_some(), "{p}");
    let fx = t.ops.effects();
    let (up, media, reconf, doctor) = (position(&fx, "engine sew upgrade"), position(&fx, MEDIA_UPGRADE), position(&fx, RECONF), position(&fx, DOCTOR));
    assert!(up < media && media < reconf && reconf < doctor, "{fx:?}");
    let body = t.stdin_of(RECONF);
    assert_eq!((body["relay_only"].as_bool(), body["bind_v4"].as_str(), body["turn_tls"].as_str()), (Some(true), Some("127.0.0.1"), Some("passthrough")), "{body}");
    assert_eq!((body["bind_v6"].as_str(), body["clear_node_ip"].as_bool()), (Some(""), Some(true)), "the clears go with every tunnel reconfigure: {body}");
}

/// A direct box skips the step (Sew: it listens publicly on purpose).
#[test]
fn a_direct_upgrade_runs_no_tunnel_bind_step() {
    let t = t();
    t.install().expect("install");
    set_tier_named(&t.env(), TIER_DIRECT).expect("to direct");
    let p = upgrade_from_old(&t);
    assert_eq!(p["result"], "done", "{p}");
    assert!(p["steps"].get("tunnel-bind").is_none(), "{p}");
    let fx = t.ops.effects();
    let media = position(&fx, MEDIA_UPGRADE);
    assert!(!fx[media..].iter().any(|e| e == RECONF || e == DOCTOR), "{fx:?}");
}

/// Sew's exit 78 naming `voice.ops.bind_v6` (a hand-set, non-loopback bind)
/// is its own error with Sew's words; the upgrade itself stands.
#[test]
fn a_hand_set_ipv6_bind_is_named_and_the_upgrade_stands() {
    let t = t();
    t.install().expect("install");
    t.ops.queue(RECONF, Ok(json!({"sewExit": 78, "reason": "voice.ops.bind_v6 = \"2001:db8::1\" is not allowed in tunnel mode (use \"\" or \"::1\")"})));
    let p = upgrade_from_old(&t);
    assert_eq!(p["result"], "tunnel_bind_failed", "{p}");
    let last = p["lastError"].as_str().expect("a reason");
    assert!(last.contains("sew_tunnel_bind_refused") && last.contains("2001:db8::1") && last.contains("k2 hostchat media rebind"), "{last}");
    assert_eq!(crate::sew::store::load().unwrap().installed_version.as_deref(), Some(k2_root_proto::sew::pinned().version));
}

/// Sew's exit 78 is "config refused" (Sew 0.8.2 gives no machine-readable
/// reason): its last stderr line goes into the hint verbatim. Only a line
/// naming `bind_v6` is the bind refusal, only one about relay-only and
/// passthrough the relay-only one.
#[test]
fn exit_78_is_told_apart_by_what_sews_last_line_names() {
    const TAIL: &[&str] = &["media", "reconfigure", "restart-now"];
    let ans = |stderr: &str| json!({"sewExit": 78, "sew": null, "stderrTail": stderr, "reason": stderr});
    let node = "sew-media: config: media.net.node_ip: 135.148.103.182 is a local address but the socket is bound to 127.0.0.1";
    let f = exit_refusal(TAIL, 78, &ans(&format!("sew: checking the new media settings\n{node}\n")), "media_reconfigure_failed");
    assert_eq!((f.code, f.status), ("sew_config_refused", 409), "{}", f.hint);
    assert!(f.hint.ends_with(&format!("Sew said: {node}")), "the line, verbatim: {}", f.hint);
    assert!(f.hint.contains("Nothing changed"), "{}", f.hint);
    assert!(!f.hint.contains("relay-only needs"), "not the relay-only story: {}", f.hint);
    assert_eq!(f.extra.as_ref().unwrap()["sewSaid"], node);
    // The bind refusal: the line names bind_v6.
    let bind = exit_refusal(TAIL, 78, &ans("sew-media: config: voice.ops.bind_v6: \"2001:db8::1\" is not allowed in tunnel mode"), "x");
    assert_eq!((bind.code, bind.status), ("sew_tunnel_bind_refused", 409));
    assert!(
        bind.hint.contains("voice.ops.bind_v6") && bind.hint.contains("k2 hostchat media rebind") && bind.hint.contains("2001:db8::1"),
        "{}",
        bind.hint
    );
    // The relay-only refusal: the line says relay-only and passthrough.
    for line in ["sew: config: relay_only requires turn tier passthrough", "sew-media: config: voice.ops.ice_relay_only: relay-only needs turn_tls passthrough"] {
        assert_eq!(exit_refusal(TAIL, 78, &ans(line), "x").code, "sew_relay_only_refused", "{line}");
    }
    // Relay-only without passthrough in the line is just a config refusal.
    assert_eq!(config_refusal("sew-media: config: voice.ops.ice_relay_only: must be a boolean").code, "sew_config_refused");
    // No stderr: the helper's reason, last line.
    let r = exit_refusal(TAIL, 78, &json!({"sewExit": 78, "reason": "first\nsew: config: turn_domain: not a hostname"}), "x");
    assert!(r.hint.ends_with("Sew said: sew: config: turn_domain: not a hostname"), "{}", r.hint);
    // Any other exit keeps the caller's code.
    let two = exit_refusal(TAIL, 2, &json!({"sewExit": 2, "reason": "bad key"}), "media_reconfigure_failed");
    assert_eq!((two.code, two.status), ("media_reconfigure_failed", 500));
    assert!(two.hint.contains("exit 2: bad key"), "{}", two.hint);
}

/// What a Sew 0.6.0 direct install leaves in Sew's media settings.
fn leftovers() -> serde_json::Map<String, Value> {
    json!({
        "turn_tls": "direct", "turn_domain": "turn-lumon.k2.dev", "relay_only": false, "bind_v4": "0.0.0.0",
        "node_ip": "135.148.103.182", "turn_advertise_port": 5349, "bind_v6": "::",
    })
    .as_object()
    .cloned()
    .unwrap()
}

/// A direct install, then the preview on and Sew's media settings modelled
/// as direct mode left them.
fn direct_with_leftovers() -> T {
    let t = t();
    *t.ops.tunnel_preview.lock().unwrap() = false;
    t.install().expect("direct install");
    *t.ops.tunnel_preview.lock().unwrap() = true;
    *t.ops.media_conf.lock().unwrap() = Some(leftovers());
    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();
    t.ops.media_argv.lock().unwrap().clear();
    t
}

/// A box that ran voice direct (Sew 0.6.0) and was upgraded keeps direct
/// mode's settings: Sew's reconfigure merges. The move to the tunnel clears
/// them: Sew gets `--node-ip ''` on argv, `turn_advertise_port: 0` and
/// `bind_v6: ""` in the JSON, and the move goes through. The way back to
/// direct is as it was.
#[test]
fn a_box_with_direct_modes_leftovers_moves_to_the_tunnel() {
    let t = direct_with_leftovers();
    let v = set_tier_named(&t.env(), "tunnel").unwrap_or_else(|e| panic!("{}: {}", e.code, e.hint));
    assert_eq!((v["turnTier"].as_str(), v["changed"].as_bool()), (Some("tunnel"), Some(true)), "{v}");
    let argv = t.ops.media_argv.lock().unwrap().last().cloned().expect("Sew ran");
    assert_eq!(
        argv,
        ["media", "install", "--reconfigure", "--restart", "--now", "--config-json", "/stage/media.json", "--node-ip", ""],
        "--node-ip '' beside the JSON"
    );
    let r = t.stdin_of(RECONF);
    assert_eq!((r["turn_advertise_port"].as_u64(), r["bind_v6"].as_str()), (Some(0), Some("")), "{r}");
    assert!(r.get("node_ip").is_none(), "never the key with the flag: {r}");
    let conf = t.ops.media_conf.lock().unwrap().clone().unwrap();
    assert!(conf.get("node_ip").is_none(), "the node IP is gone: {conf:?}");
    assert_eq!(
        (conf["turn_advertise_port"].as_u64(), conf["bind_v6"].as_str(), conf["relay_only"].as_bool(), conf["bind_v4"].as_str()),
        (Some(0), Some(""), Some(true), Some("127.0.0.1")),
        "{conf:?}"
    );
    assert_eq!(voice::tier(&load(&t.env())), "tunnel");

    // The way back to direct: node IP, relay-only off, every interface,
    // 5349; no clears.
    t.ops.clear();
    t.ops.media_argv.lock().unwrap().clear();
    set_tier_named(&t.env(), TIER_DIRECT).expect("to direct");
    let back = t.stdin_of(RECONF);
    assert_eq!(
        (back["node_ip"].as_str(), back["relay_only"].as_bool(), back["bind_v4"].as_str(), back["turn_advertise_port"].as_u64()),
        (Some("203.0.113.7"), Some(false), Some("0.0.0.0"), Some(5349)),
        "{back}"
    );
    assert!(back.get("bind_v6").is_none() && back.get("clear_node_ip").is_none(), "{back}");
    assert!(!t.ops.media_argv.lock().unwrap()[0].iter().any(|a| a == "--node-ip"), "no --node-ip on the way back");
    let conf = t.ops.media_conf.lock().unwrap().clone().unwrap();
    assert_eq!(
        (conf["node_ip"].as_str(), conf["turn_advertise_port"].as_u64(), conf["relay_only"].as_bool()),
        (Some("203.0.113.7"), Some(5349), Some(false)),
        "{conf:?}"
    );
}

/// A Sew that predates the clears gets today's tunnel keys, so a box with
/// direct mode's node IP is refused: `sew_config_refused`, Sew's line
/// verbatim, nothing changed, and how to get past it.
#[test]
fn a_node_ip_refusal_is_a_config_refusal_with_sews_line() {
    let t = direct_with_leftovers();
    *t.ops.mgmt_version.lock().unwrap() = Some("0.0.1".into());
    let e = set_tier_named(&t.env(), "tunnel").expect_err("refused");
    assert_eq!((e.code, e.status), ("sew_config_refused", 409), "{}", e.hint);
    let line = "sew-media: config: media.net.node_ip: 135.148.103.182 is a local address but the socket is bound to 127.0.0.1";
    assert!(e.hint.contains(&format!("Sew said: {line}")) && e.hint.contains("Nothing changed"), "{}", e.hint);
    assert!(!e.hint.contains("relay-only needs"), "{}", e.hint);
    assert!(e.hint.contains("k2 hostchat upgrade"), "{}", e.hint);
    assert!(t.stdin_of(RECONF).get("clear_node_ip").is_none(), "this Sew got no clears");
    assert_eq!(*t.ops.media_conf.lock().unwrap(), Some(leftovers()), "Sew kept nothing");
    assert_eq!(voice::tier(&load(&t.env())), "direct");
}

/// A new daemon on a root helper older than protocol 14 (no
/// `sewtunnelclear`): every move to the tunnel stops before anything runs,
/// and says to update the helper. Nothing is half-applied.
#[test]
fn a_helper_without_the_clears_refuses_every_tunnel_move_cleanly() {
    let ran = |t: &T| t.ops.calls.lock().unwrap().iter().any(|c| c.argv.contains("engine sew media"));

    // Direct → tunnel.
    let t = direct_with_leftovers();
    t.helper(13, true);
    let e = set_tier_named(&t.env(), "tunnel").expect_err("old helper");
    assert_eq!((e.code, e.status), ("helper_outdated", 409), "{}", e.hint);
    assert!(e.hint.contains("protocol 14") && e.hint.contains("Nothing changed"), "{}", e.hint);
    assert!(!ran(&t), "nothing reached Sew");
    assert!(t.ops.cp_log().is_empty(), "no cert, no DNS change: {:?}", t.ops.cp_log());
    assert_eq!(*t.ops.media_conf.lock().unwrap(), Some(leftovers()));
    assert_eq!(voice::tier(&load(&t.env())), "direct");

    // A fresh tunnel install: the preflight names it.
    let t2 = self::t();
    t2.helper(13, true);
    let e = t2.install().expect_err("old helper");
    assert_eq!(e.code, "preflight_failed", "{}", e.hint);
    let report = e.extra.unwrap()["report"].clone();
    let tt = report.as_array().unwrap().iter().find(|c| c["id"] == "tunnel_tier").cloned().unwrap();
    assert_eq!(tt["detail"], "helper_outdated", "{report}");
    assert!(!ran(&t2), "nothing reached Sew");

    // The rebind of a tunnel box.
    let t3 = self::t();
    t3.install().expect("tunnel install");
    t3.helper(13, true);
    t3.ops.clear();
    let e = rebind(&t3.env()).expect_err("old helper");
    assert_eq!(e.code, "helper_outdated", "{}", e.hint);
    assert!(!ran(&t3), "nothing reached Sew");

    // Below the clears' Sew a 13 helper still moves voice, with today's keys.
    let t4 = direct_with_leftovers();
    *t4.ops.media_conf.lock().unwrap() = None;
    t4.helper(13, true);
    *t4.ops.mgmt_version.lock().unwrap() = Some("0.0.1".into());
    set_tier_named(&t4.env(), "tunnel").unwrap_or_else(|e| panic!("{}: {}", e.code, e.hint));
    let r = t4.stdin_of(RECONF);
    assert_eq!(keys_of(&r), ["bind_v4", "relay_only", "turn_cert_pem", "turn_domain", "turn_key_pem", "turn_tls"], "{r}");
}

/// `sew doctor`'s `media.tunnel_bind`: ok passes; anything else, or no row
/// at all, fails with Sew's detail and hint.
#[test]
fn the_doctor_check_is_read_and_a_failing_one_is_an_error() {
    let ok = json!({"sewExit": 0, "sew": {"checks": [{"id": "media.tunnel_bind", "status": "ok", "detail": "bind_v6 \"\""}]}});
    assert_eq!(tunnel_bind_check(&ok)["status"], "ok");
    assert_eq!(tunnel_bind_check(&json!({"sewExit": 0, "sew": {"checks": []}}))["status"], "missing");
    let t = t();
    t.install().expect("install");
    t.ops.queue(DOCTOR, Ok(json!({"sewExit": 1, "sew": {"ok": false, "checks": [
        {"id": "media.tunnel_bind", "status": "fail", "detail": "sew-media listens on [::]:7881", "hint": "sew media install --reconfigure --restart"},
    ]}})));
    let e = rebind(&t.env()).expect_err("doctor fails");
    assert_eq!(e.code, "tunnel_bind_not_ok");
    assert!(e.hint.contains("[::]:7881") && e.hint.contains("--reconfigure --restart"), "{}", e.hint);
    assert_eq!(e.extra.as_ref().unwrap()["tunnelBind"]["status"], "fail");
}

/// `k2 hostchat media rebind`: steps 3 and 4 on an upgraded tunnel box,
/// nothing on a direct one, refused on a Sew that keeps "::".
#[test]
fn the_owner_command_rebinds_a_tunnel_box_only() {
    let t = t();
    t.install().expect("install");
    t.ops.clear();
    t.ops.queue(RECONF, Ok(json!({"sewExit": 0, "sew": {"changed": ["media.toml"], "warnings": ["tunnel_bind_v6_rewritten"]}})));
    let v = rebind(&t.env()).expect("rebind");
    assert_eq!((v["changed"].as_bool(), v["rewritten"].as_bool(), v["tunnelBind"]["status"].as_str()), (Some(true), Some(true), Some("ok")), "{v}");
    let fx = t.ops.effects();
    assert!(position(&fx, RECONF) < position(&fx, DOCTOR), "{fx:?}");

    *t.ops.mgmt_version.lock().unwrap() = Some("0.0.1".into());
    t.ops.clear();
    let e = rebind(&t.env()).expect_err("too old");
    assert_eq!(e.code, "sew_upgrade_needed", "{}", e.hint);
    assert!(e.hint.contains("k2 hostchat upgrade"), "{}", e.hint);
    assert!(!t.ops.effects().iter().any(|x| x == RECONF), "nothing ran");
    *t.ops.mgmt_version.lock().unwrap() = None;

    set_tier_named(&t.env(), TIER_DIRECT).expect("to direct");
    t.ops.clear();
    let v = rebind(&t.env()).expect("direct");
    assert_eq!((v["changed"].as_bool(), v["turnTier"].as_str()), (Some(false), Some(TIER_DIRECT)), "{v}");
    assert!(!t.ops.effects().iter().any(|x| x == RECONF), "direct is left alone");
}

#[test]
fn the_real_gates_are_sew_0_7_4_and_0_8_2() {
    assert_eq!(TUNNEL_CLEARS_MIN_SEW, "0.8.2", "Sew confirmed the clears on 0.8.2");
    for (v, yes) in [("0.8.2", true), ("0.8.2+g1", true), ("0.9.0", true), ("0.8.1", false), ("0.7.4", false), ("", false)] {
        assert_eq!(at_least(v, TUNNEL_CLEARS_MIN_SEW), yes, "{v}");
    }
    assert_eq!(TUNNEL_BIND_MIN_SEW, "0.7.4");
    for (v, yes) in [("0.7.4", true), ("0.8.0", true), ("0.8.0+g0507cb9", true), ("0.7.3", false), ("0.6.1", false), ("", false)] {
        assert_eq!(at_least(v, TUNNEL_BIND_MIN_SEW), yes, "{v}");
    }
}
