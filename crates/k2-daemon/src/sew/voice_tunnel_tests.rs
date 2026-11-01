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

/// A box with chat running, root helper protocol 12 (sewtunnel), Sew
/// listing `turn_relay_only` and `voice_media_path`, the preview on.
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
    t.helper(12, true);
    t.features(&[FEATURE_TURN_RELAY_ONLY, media_path::FEATURE_VOICE_MEDIA_PATH]);
    *t.ops.tunnel_preview.lock().unwrap() = true;
    t.ops.clear();
    t.ops.cp_calls.lock().unwrap().clear();
    t
}

impl T {
    fn env(&self) -> Env<'_> {
        Env { ops: &self.ops, secrets: &self.secrets, latch: &self.latch, token: &self.token }
    }
    fn helper(&self, protocol: u32, sewtunnel: bool) {
        let mut h = helper_ok(protocol, true, k2_root_proto::sew::pinned().version);
        h.version_json.as_mut().unwrap()["namespaces"]["sewtunnel"] = json!(sewtunnel);
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

/// S9a end to end: no TURN DNS, a `turn-tunnel` cert for `turn.<label>`
/// with the bare label, Sew's M5/M7/M9 keys and no node IP, the voice lane
/// armed, the served check through the relay.
#[test]
fn a_tunnel_install_publishes_no_ip_and_hands_sew_relay_only_passthrough() {
    let t = t();
    let out = t.install().expect("tunnel install");
    assert_eq!(out["turnTier"], "tunnel");
    assert_eq!(out["turnHost"], "turn.lztek.k2.dev");
    assert_eq!(out["nodeIp"], Value::Null);
    assert_eq!(out["relayOnly"], true);
    assert_eq!(out["iceServers"], json!(["turns:turn.lztek.k2.dev:443?transport=tcp"]));
    assert_eq!(out["turnTls"]["served"], "ok", "{out}");

    let cp = t.ops.cp_log();
    assert!(!cp.iter().any(|c| c.contains("/turn/dns")), "no DNS in the tunnel tier: {cp:?}");
    let cert = t.cp_body("POST /cert");
    assert_eq!(cert["mode"], "turn-tunnel");
    assert_eq!(cert["subdomain"], "lztek", "the bare label");
    assert_eq!(csr_sans(cert["csr"].as_str().unwrap()), vec!["turn.lztek.k2.dev".to_string()]);

    let m = t.stdin_of(TUNNEL_INSTALL);
    assert_eq!(m["turn_tls"], "passthrough");
    assert_eq!(m["turn_domain"], "turn.lztek.k2.dev");
    assert_eq!(m["turn_trusted_cidrs"], json!(["127.0.0.1/32", "::1/128"]));
    assert_eq!(m["relay_only"], true);
    assert_eq!(m["bind_v4"], "127.0.0.1", "media sockets off the network");
    assert!(m.get("node_ip").is_none(), "no node IP: Sew writes 127.0.0.1 itself: {m}");
    assert!(m.get("turn_advertise_port").is_none(), "Sew's passthrough default (443): {m}");
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(m).expect("the helper's MediaRequest parses it");
    req.validate().expect("valid for protocol 12");

    assert_eq!(*t.ops.lane_calls.lock().unwrap(), vec![true], "the voice lane is armed");
    assert_eq!(t.ops.served_connects.lock().unwrap().last().map(String::as_str), Some("turn.lztek.k2.dev"));
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
    assert_eq!(out["turnHost"], "turn-lztek.k2.dev");
    assert!(t.ops.lane_calls.lock().unwrap().is_empty(), "no voice lane without the preview");
    assert!(t.stdin_of(TUNNEL_INSTALL).get("relay_only").is_none(), "protocol-11 clean");

    let refused = |t: &T, code: &str| {
        let e = t.install().expect_err("refused");
        assert_eq!(e.code, "preflight_failed", "{}", e.hint);
        let report = e.extra.unwrap()["report"].clone();
        let tt = report.as_array().unwrap().iter().find(|c| c["id"] == "tunnel_tier").cloned().unwrap();
        assert_eq!((tt["status"].as_str(), tt["detail"].as_str()), (Some("fail"), Some(code)), "{report}");
    };
    let t2 = t();
    t2.helper(11, false);
    refused(&t2, "helper_outdated");
    let t3 = t();
    t3.features(&[media_path::FEATURE_VOICE_MEDIA_PATH]);
    refused(&t3, "sew_too_old");
    assert!(t3.ops.cp_log().is_empty(), "nothing issued");
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
    assert_eq!((r["turn_tls"].as_str(), r["turn_domain"].as_str(), r["relay_only"].as_bool()), (Some("passthrough"), Some("turn.lztek.k2.dev"), Some(true)));
    assert!(r.get("node_ip").is_none());
    assert!(t.ops.cp_log().contains(&"DELETE /turn/dns/lztek".to_string()), "the box IP leaves DNS: {:?}", t.ops.cp_log());
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
    assert_eq!((r["turn_tls"].as_str(), r["turn_domain"].as_str(), r["relay_only"].as_bool()), (Some("direct"), Some("turn-lztek.k2.dev"), Some(false)));
    assert_eq!(r["node_ip"], "203.0.113.7");
    assert_eq!(r["bind_v4"], "0.0.0.0", "media sockets back on the network");
    assert!(t.ops.cp_log().contains(&"PUT /turn/dns/lztek".to_string()));
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
    *t.ops.connect_tier.lock().unwrap() = Ok(Some("single".into()));
    let c = voice::preflight(&t.env());
    let get = |id: &str| c.iter().find(|x| x["id"] == id).cloned().unwrap_or_else(|| panic!("no {id}: {c:?}"));
    assert_eq!(get("tunnel_tier")["status"], "ok");
    assert_eq!(get("e2e")["status"], "fail");
    assert_eq!(get("pro")["status"], "fail");
    assert_eq!(get("tunnel_label")["detail"], "TURN host turn.lztek.k2.dev");
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
