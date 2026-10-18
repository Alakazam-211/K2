//! Voice (K13), standalone identity and users (K14 demo asks) over a fake
//! box and a fake K2 Connect control plane. Nothing touches the network,
//! systemd, the vault on disk or Sew.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::sew::ops::fake::{FakeOps, INSTALL_ID};
use crate::sew::ops::{CpError, MgmtError, SewOps};
use crate::sew::secrets::{MemSecrets, SewSecrets};
use crate::sew::store;
use crate::sew::supervisor::{enable_begin, enable_machine, EnableRequest};

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

fn t() -> T {
    let ops = FakeOps::new();
    ops.files.lock().unwrap().insert(TARBALL.into(), fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    ops.files.lock().unwrap().insert(MEDIA.into(), fixture("sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    T { ops, secrets: MemSecrets::default(), latch: AtomicBool::new(false), token: Mutex::new(None), _db: k2_core::db::scoped_for_test() }
}

impl T {
    fn env(&self) -> Env<'_> {
        Env { ops: &self.ops, secrets: &self.secrets, latch: &self.latch, token: &self.token }
    }
    fn enable_with(&self, provider: Option<&str>) -> Result<(), String> {
        let env = self.env();
        let req = EnableRequest { tarball_path: Some(TARBALL.into()), provider: provider.map(str::to_string), ..Default::default() };
        let latch = match enable_begin(&env, &req) {
            Ok(Ok(l)) => l,
            Ok(Err(v)) => return Err(format!("no machine: {v}")),
            Err(f) => return Err(format!("{}: {}", f.code, f.hint)),
        };
        let r = enable_machine(&env, &req);
        drop(latch);
        r
    }
    fn chat(&self) {
        self.enable_with(None).expect("enable");
        self.ops.clear();
    }
    fn install(&self, req: InstallRequest) -> Result<Value, Fail> {
        let env = self.env();
        let latch = install_begin(&env, &req)?;
        let r = install_machine(&env, &req);
        drop(latch);
        r
    }
    fn voice(&self) -> Value {
        self.chat();
        let v = self.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).expect("voice install");
        self.ops.clear();
        self.ops.cp_calls.lock().unwrap().clear();
        v
    }
    fn state(&self) -> VoiceState {
        load(&self.env())
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
    fn advance(&self, secs: i64) {
        *self.ops.clock.lock().unwrap() += secs;
    }
}

fn csr_names(csr_pem: &str) -> (Vec<String>, Vec<String>, Vec<u8>) {
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::FromDer;
    let mut reader = csr_pem.as_bytes();
    let (pem, _) = x509_parser::pem::Pem::read(std::io::Cursor::new(&mut reader)).unwrap();
    let (_, csr) = x509_parser::certification_request::X509CertificationRequest::from_der(&pem.contents).unwrap();
    let mut sans = vec![];
    for ext in csr.requested_extensions().into_iter().flatten() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext {
            for n in &san.general_names {
                match n {
                    GeneralName::DNSName(d) => sans.push(d.to_string()),
                    other => sans.push(format!("{other:?}")),
                }
            }
        }
    }
    let cns = csr.certification_request_info.subject.iter_common_name().map(|c| c.as_str().unwrap().to_string()).collect();
    (sans, cns, csr.certification_request_info.subject_pki.raw.to_vec())
}

// ── install ──────────────────────────────────────────────────────────────

#[test]
fn install_gets_dns_then_a_single_san_cert_for_its_own_key_and_hands_sew_turn_tls() {
    let t = t();
    t.chat();
    let out = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap();
    assert_eq!(out["state"], "installed");
    assert_eq!(out["turnHost"], "turn-lztek.k2.dev");
    assert_eq!(out["publicUrl"], "wss://voice.lztek.k2.dev");
    assert_eq!(out["nodeIp"], "203.0.113.7");
    assert_eq!(out["turnTls"]["served"], "ok");
    assert_eq!(out["warnings"], json!([]), "the box carries its public IP, served leaf is ours");

    // Order of the K2 Connect calls (docs/turn.md): DNS first, then the cert.
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek", "POST /cert"]);
    assert_eq!(t.cp_body("PUT /turn/dns/lztek"), json!({}), "the control plane observes the IP");
    let cert_body = t.cp_body("POST /cert");
    assert_eq!(cert_body["mode"], "turn");
    assert_eq!(cert_body["subdomain"], "lztek");
    assert_eq!(cert_body["token"], "", "the tunnel token rides the Authorization header");
    let (sans, cns, spki) = csr_names(cert_body["csr"].as_str().unwrap());
    assert_eq!(sans, vec!["turn-lztek.k2.dev"], "exactly one SAN, no wildcard, no base name");
    assert_eq!(cns, vec!["turn-lztek.k2.dev"], "CN is the host (rcgen's default CN is refused)");

    // The key: P-256, in the vault, never the tunnel key, never on disk.
    let st = t.state();
    let key_ref = st.turn_key_ref.clone().expect("vault ref");
    assert!(key_ref.starts_with("sewsec_turn-key_"), "{key_ref}");
    let key_pem = t.secrets.resolve(&key_ref).unwrap().unwrap();
    let key = rcgen::KeyPair::from_pem(&key_pem).unwrap();
    assert_eq!(key.public_key_der(), spki, "the CSR is for the vault key");
    assert!(key.is_compatible(&rcgen::PKCS_ECDSA_P256_SHA256));
    let state_text = serde_json::to_string(&t.ops.voice_load().unwrap()).unwrap();
    assert!(!state_text.contains("PRIVATE KEY"), "the voice record holds no key");

    // Sew gets the TURN TLS as PEM text on the helper's stdin only.
    let effects = t.ops.effects();
    assert_eq!(effects, vec!["engine sew stage-media", "engine sew media install"]);
    let body = t.stdin_of("engine sew media install");
    assert_eq!(body["turn_tls"], "direct");
    assert_eq!(body["turn_domain"], "turn-lztek.k2.dev");
    assert_eq!(body["turn_advertise_port"], 5349);
    assert_eq!(body["udp_port"], 7882);
    assert_eq!(body["tcp_port"], 7881);
    assert_eq!(body["node_ip"], "203.0.113.7");
    assert_eq!(body["sysctl"], "apply");
    assert_eq!(body["turn_key_pem"], key_pem);
    assert_eq!(body["turn_cert_pem"].as_str(), st.cert_pem.as_deref());
    // The helper's own parser accepts exactly this body.
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(body).unwrap();
    req.validate().unwrap();
    for c in t.ops.calls.lock().unwrap().iter() {
        assert!(!c.argv.contains("PRIVATE"), "no key in argv");
        let in_stdin = c.stdin.as_ref().is_some_and(|s| String::from_utf8_lossy(s).contains("PRIVATE KEY"));
        assert_eq!(in_stdin, c.argv == "engine sew media install", "{}", c.argv);
    }
    // The default URL needs the nested voice label at sew-media's loopback.
    assert_eq!(*t.ops.nested.lock().unwrap(), vec![("voice".to_string(), Some("127.0.0.1:7880".to_string()))]);
    assert!(st.installed && st.voice_label);
    assert_eq!(st.cert_not_after.unwrap() - t.ops.now() > 80 * 86_400, true);
}

#[test]
fn an_app_origin_is_the_voice_url_and_needs_no_nested_label() {
    let t = t();
    t.chat();
    let out = t
        .install(InstallRequest { public_url: Some("wss://hearth.lztek.k2.dev".into()), tarball_path: Some(MEDIA.into()), ..Default::default() })
        .unwrap();
    assert_eq!(out["publicUrl"], "wss://hearth.lztek.k2.dev");
    assert_eq!(t.stdin_of("engine sew media install")["public_url"], "wss://hearth.lztek.k2.dev");
    assert!(t.ops.nested.lock().unwrap().is_empty(), "Hearth's BFF proxies /rtc same-origin");
    // A custom domain bound to this box works too; anything else is refused
    // before any effect.
    assert_eq!(check_voice_url(&t.env(), "wss://chat.acme.dev").unwrap(), "wss://chat.acme.dev");
    assert_eq!(check_voice_url(&t.env(), "wss://acme.dev").unwrap(), "wss://acme.dev");
    for bad in [
        "wss://evil.example",
        "wss://lztek.k2.dev.evil.example",
        "wss://otherbox.k2.dev",
        "wss://notacme.dev",
        "https://hearth.lztek.k2.dev",
        "wss://hearth.lztek.k2.dev/rtc",
        "wss://hearth.lztek.k2.dev:8443",
        "wss://Hearth.lztek.k2.dev",
    ] {
        assert!(check_voice_url(&t.env(), bad).is_err(), "{bad}");
    }
    t.ops.clear();
    let f = t.install(InstallRequest { public_url: Some("wss://evil.example".into()), ..Default::default() }).unwrap_err();
    assert_eq!((f.code, f.status), ("url_not_on_this_box", 400));
    assert!(t.ops.effects().is_empty());
}

#[test]
fn ip_unverified_stops_before_sew_and_says_k2_retries() {
    let t = t();
    t.chat();
    t.ops.queue_cp(
        "PUT /turn/dns/lztek",
        Err(CpError { status: 403, code: "ip_unverified".into(), detail: "caller 198.51.100.9; relay none".into() }),
    );
    let f = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap_err();
    assert_eq!((f.code, f.status), ("turn_ip_unverified", 409));
    assert!(f.hint.contains("next tunnel login"), "{}", f.hint);
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek"], "no cert without the record");
    assert!(!t.ops.effects().contains(&"engine sew media install".to_string()));
    let st = t.state();
    assert!(!st.installed);
    assert_eq!(st.progress.as_ref().unwrap()["result"], "error");
    // The re-run resumes: staged media is reused, the record goes up.
    t.ops.clear();
    t.install(InstallRequest::default()).unwrap();
    assert_eq!(t.ops.effects(), vec!["engine sew media install"], "the staged pin is reused");
}

#[test]
fn an_install_re_run_reuses_a_valid_cert_and_spends_no_turn_issuance() {
    let t = t();
    t.voice();
    let before = t.state();
    t.install(InstallRequest::default()).unwrap();
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek"], "DNS again, no new cert");
    assert_eq!(t.state().cert_leaf_sha256, before.cert_leaf_sha256);
    assert_eq!(t.state().turn_key_ref, before.turn_key_ref);
    // Inside the renewal window an install re-run issues a fresh one.
    t.ops.cp_calls.lock().unwrap().clear();
    t.advance(70 * 86_400);
    t.install(InstallRequest::default()).unwrap();
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek", "POST /cert"]);
}

#[test]
fn a_cert_for_another_name_or_key_is_never_handed_to_sew() {
    let t = t();
    t.chat();
    // The broker answers a cert for another host (a broker bug or a MITM).
    let other_key = rcgen::KeyPair::generate().unwrap();
    let foreign = rcgen::CertificateParams::new(vec!["turn-other.k2.dev".to_string()]).unwrap().self_signed(&other_key).unwrap();
    t.ops.queue_cp("POST /cert", Ok(json!({"cert": foreign.pem()})));
    let f = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap_err();
    assert_eq!(f.code, "turn_failed");
    assert!(f.hint.contains("expected exactly [turn-lztek.k2.dev]"), "{}", f.hint);
    let key_ref = t.state().turn_key_ref.expect("the vault key is recorded even when issuance fails");
    // Right name, wrong key.
    let wrong = rcgen::CertificateParams::new(vec!["turn-lztek.k2.dev".to_string()]).unwrap().self_signed(&other_key).unwrap();
    t.ops.queue_cp("POST /cert", Ok(json!({"cert": wrong.pem()})));
    let f = t.install(InstallRequest::default()).unwrap_err();
    assert!(f.hint.contains("not for K2's TURN key"), "{}", f.hint);
    assert_eq!(t.state().turn_key_ref.as_deref(), Some(key_ref.as_str()), "the retry reuses the key, no orphan");
    assert_eq!(t.secrets.map.lock().unwrap().keys().filter(|k| k.starts_with("sewsec_turn-key_")).count(), 1);
    assert!(!t.ops.effects().contains(&"engine sew media install".to_string()));
}

#[test]
fn preflight_refuses_taken_ports_no_label_and_no_running_chat() {
    let t = t();
    let ids = |c: &[Value]| c.iter().filter(|x| x["status"] == "fail").map(|x| x["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(ids(&preflight(&t.env())).contains(&"chat".to_string()), "no Sew yet");
    t.chat();
    assert!(ids(&preflight(&t.env())).is_empty(), "{:?}", preflight(&t.env()));
    t.ops.taken.lock().unwrap().extend([(7882, true), (5349, false)]);
    *t.ops.label.lock().unwrap() = None;
    let c = preflight(&t.env());
    assert_eq!(ids(&c), vec!["label", "ports"]);
    let ports = c.iter().find(|x| x["id"] == "ports").unwrap();
    assert!(ports["detail"].as_str().unwrap().contains("UDP 7882") && ports["detail"].as_str().unwrap().contains("TCP 5349"));
    let f = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap_err();
    assert_eq!(f.code, "preflight_failed");
    assert!(t.ops.effects().is_empty() && t.ops.cp_log().is_empty());
}

#[test]
fn nat_is_warned_and_a_foreign_cert_on_5349_is_reported() {
    let t = t();
    t.chat();
    *t.ops.ips.lock().unwrap() = vec!["10.0.0.5".into()];
    *t.ops.serves_ours.lock().unwrap() = Some(false);
    let out = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap();
    let w = out["warnings"].to_string();
    assert!(w.contains("1:1 NAT"), "{w}");
    assert!(w.contains("served mismatch"), "{w}");
    assert_eq!(out["turnTls"]["served"], "mismatch");
}

// ── voice URL later ──────────────────────────────────────────────────────

#[test]
fn the_voice_url_changes_later_through_reconfigure_and_a_sew_restart() {
    let t = t();
    t.voice();
    let out = set_url(&t.env(), "wss://hearth.lztek.k2.dev").unwrap();
    assert_eq!(out["changed"], true);
    assert_eq!(t.ops.effects(), vec!["engine sew media reconfigure", "engine sew systemctl restart"]);
    assert_eq!(t.stdin_of("engine sew media reconfigure"), json!({"public_url": "wss://hearth.lztek.k2.dev"}));
    // Moving off the default drops the nested voice label.
    assert_eq!(t.ops.nested.lock().unwrap().last().unwrap(), &("voice".to_string(), None));
    assert_eq!(t.state().public_url.as_deref(), Some("wss://hearth.lztek.k2.dev"));
    assert!(!t.state().voice_label);
    t.ops.clear();
    assert_eq!(set_url(&t.env(), "wss://hearth.lztek.k2.dev").unwrap()["changed"], false);
    assert!(t.ops.effects().is_empty());
    assert_eq!(set_url(&t.env(), "wss://elsewhere.example").unwrap_err().code, "url_not_on_this_box");
}

#[test]
fn a_url_set_before_voice_exists_is_kept_for_the_install() {
    let t = t();
    t.chat();
    let out = set_url(&t.env(), "wss://hearth.lztek.k2.dev").unwrap();
    assert_eq!(out["applied"], false);
    assert!(t.ops.effects().is_empty());
    t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap();
    assert_eq!(t.stdin_of("engine sew media install")["public_url"], "wss://hearth.lztek.k2.dev");
}

// ── renewal ──────────────────────────────────────────────────────────────

#[test]
fn renewal_waits_for_the_window_then_reuses_the_key_without_an_engine_restart() {
    let t = t();
    t.voice();
    let st0 = t.state();
    tick(&t.env());
    assert!(t.ops.cp_log().is_empty(), "90-day cert: nothing due");
    t.advance(61 * 86_400);
    tick(&t.env());
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek", "POST /cert"]);
    assert_eq!(t.ops.effects(), vec!["engine sew media turn-cert"], "same IP: no reconfigure, no restart");
    let st = t.state();
    assert_eq!(st.turn_key_ref, st0.turn_key_ref, "the key is reused");
    let (_, _, spki) = csr_names(t.cp_body("POST /cert")["csr"].as_str().unwrap());
    let key = rcgen::KeyPair::from_pem(&t.secrets.resolve(st.turn_key_ref.as_deref().unwrap()).unwrap().unwrap()).unwrap();
    assert_eq!(spki, key.public_key_der());
    assert!(st.cert_not_after > st0.cert_not_after);
    let body = t.stdin_of("engine sew media turn-cert");
    let req: k2_root_proto::sew::TurnCertRequest = serde_json::from_value(body).unwrap();
    req.validate().unwrap();
}

#[test]
fn a_new_box_ip_moves_the_record_and_sews_node_ip() {
    let t = t();
    t.voice();
    *t.ops.cp_ip.lock().unwrap() = "203.0.113.99".into();
    let out = renew(&t.env(), true).unwrap();
    assert_eq!(out["nodeIpMoved"], "203.0.113.99");
    assert_eq!(t.ops.effects(), vec!["engine sew media reconfigure restart-now", "engine sew media turn-cert"]);
    assert_eq!(t.stdin_of("engine sew media reconfigure restart-now"), json!({"node_ip": "203.0.113.99"}));
    assert_eq!(t.state().node_ip.as_deref(), Some("203.0.113.99"));
}

#[test]
fn ip_unverified_on_renewal_backs_off_until_the_next_frpc_login() {
    let t = t();
    t.voice();
    t.advance(61 * 86_400);
    t.ops.queue_cp("PUT /turn/dns/lztek", Err(CpError { status: 403, code: "ip_unverified".into(), detail: "x".into() }));
    tick(&t.env());
    let st = t.state();
    assert_eq!(st.failures, 1);
    assert!(st.last_error.as_deref().unwrap().contains("turn_ip_unverified"));
    assert_eq!(st.logins_at_failure, Some(1));
    t.ops.cp_calls.lock().unwrap().clear();
    tick(&t.env());
    assert!(t.ops.cp_log().is_empty(), "backing off");
    *t.ops.logins.lock().unwrap() = 2; // frpc logged in again
    tick(&t.env());
    assert_eq!(t.ops.cp_log(), vec!["PUT /turn/dns/lztek", "POST /cert"]);
    let st = t.state();
    assert_eq!((st.failures, st.logins_at_failure, st.last_error.clone()), (0, None, None));
}

#[test]
fn other_renewal_failures_back_off_and_never_fight_a_lifecycle_step() {
    let t = t();
    t.voice();
    t.advance(61 * 86_400);
    t.ops.queue_cp("POST /cert", Err(CpError { status: 502, code: "issuance_failed".into(), detail: "acme".into() }));
    tick(&t.env());
    let st = t.state();
    assert_eq!(st.next_try_at, Some(t.ops.now() + RETRY_MIN_SECS));
    t.ops.cp_calls.lock().unwrap().clear();
    t.advance(RETRY_MIN_SECS);
    t.latch.store(true, std::sync::atomic::Ordering::SeqCst);
    tick(&t.env());
    assert!(t.ops.cp_log().is_empty(), "latched: no renewal");
    t.latch.store(false, std::sync::atomic::Ordering::SeqCst);
    tick(&t.env());
    assert_eq!(t.ops.cp_log().len(), 2);
    assert_eq!(backoff(1), 120);
    assert_eq!(backoff(3), 480);
    assert_eq!(backoff(30), RETRY_MAX_SECS);
}

// ── status ───────────────────────────────────────────────────────────────

#[test]
fn status_maps_voice_into_the_server_verdict() {
    let t = t();
    t.voice();
    // Clear the chat-side K2 alarms (no off-box backups, recovery key).
    store::update(1, |r| {
        r.ship_kind = "dir".into();
        r.recovery_shown_at = Some(1);
    })
    .unwrap();
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert_eq!(s["voice"]["state"], "running", "{}", s["voice"]);
    assert_eq!(s["state"], "running", "port_443_unavailable is a setup warning, not degraded: {}", s["alarms"]);
    let w = s["warnings"].to_string();
    assert!(w.contains("port_443_unavailable") && w.contains("voice_unprobed"), "{w}");
    assert_eq!(s["voice"]["turnHost"], "turn-lztek.k2.dev");
    assert!(s["voice"]["cert"]["daysLeft"].as_i64().unwrap() >= 89);

    // An engine alarm degrades the server; a probe clears the warning.
    *t.ops.media_state.lock().unwrap() = json!({"state": "running", "alarms": ["engine_unhealthy"]});
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert_eq!(s["state"], "degraded");
    assert!(s["alarms"].as_array().unwrap().contains(&json!("voice_engine_unhealthy")));
    *t.ops.media_state.lock().unwrap() = json!({"state": "running", "alarms": []});
    probe_record(&t.env(), true, true, true).unwrap();
    assert!(t.ops.effects().contains(&"engine sew media probe-record ok ok ok".to_string()));
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert_eq!(s["warnings"], json!([]));

    // A live check that finds another cert on 5349.
    *t.ops.serves_ours.lock().unwrap() = Some(false);
    let s = crate::sew::supervisor::status(&t.env(), true);
    assert!(s["alarms"].as_array().unwrap().contains(&json!("k2_turn_tls_mismatch")));
    assert_eq!(s["state"], "degraded");
    *t.ops.serves_ours.lock().unwrap() = Some(true);
    crate::sew::supervisor::status(&t.env(), true);

    // Expiring, then a stopped engine.
    t.advance(80 * 86_400);
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert!(s["alarms"].as_array().unwrap().contains(&json!("k2_turn_cert_expiring")), "{}", s["alarms"]);
    t.ops.set_unit(k2_root_proto::sew::UNIT_MEDIA, "failed", 1);
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert_eq!(s["voice"]["state"], "error");
    assert!(s["alarms"].as_array().unwrap().contains(&json!("voice_engine_down")));
}

#[test]
fn uninstall_removes_the_engine_record_label_and_key_and_purge_forgets_voice() {
    let t = t();
    t.voice();
    let key_ref = t.state().turn_key_ref.unwrap();
    let out = uninstall(&t.env()).unwrap();
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(t.ops.effects(), vec!["engine sew media uninstall"]);
    assert_eq!(t.ops.cp_log(), vec!["DELETE /turn/dns/lztek"]);
    assert_eq!(t.ops.nested.lock().unwrap().last().unwrap(), &("voice".to_string(), None));
    assert_eq!(t.secrets.resolve(&key_ref).unwrap(), None);
    assert!(t.ops.voice_load().is_none());
    assert_eq!(uninstall(&t.env()).unwrap()["alreadyRemoved"], true);
}

// ── standalone identity ──────────────────────────────────────────────────

#[test]
fn standalone_install_has_no_jwks_and_the_identity_is_fixed_until_a_fresh_install() {
    let t = t();
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    t.enable_with(Some("standalone")).unwrap();
    let calls = t.ops.calls.lock().unwrap().clone();
    let install = calls.iter().find(|c| c.argv == "engine sew install").unwrap();
    let body: Value = serde_json::from_slice(install.stdin.as_ref().unwrap()).unwrap();
    assert_eq!(body["provider"], "standalone");
    assert!(body.get("jwksUrl").is_none());
    let req: k2_root_proto::sew::InstallRequest = serde_json::from_value(body).unwrap();
    req.validate().unwrap();
    let row = store::load().unwrap();
    assert_eq!(crate::sew::supervisor::row_provider(&row), "standalone");
    let s = crate::sew::supervisor::status(&t.env(), false);
    assert_eq!(s["identityProvider"], "standalone");
    // Re-enabling as k2 is refused before any effect.
    store::update(1, |r| r.status = "disabled".into()).unwrap();
    t.ops.clear();
    let e = t.enable_with(None).unwrap_err();
    assert!(e.starts_with("provider_fixed"), "{e}");
    assert!(t.ops.effects().is_empty());
}

#[test]
fn a_k2_install_refuses_to_link_a_standalone_sew() {
    let t = t();
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    let e = t.enable_with(None).unwrap_err();
    assert!(e.contains("will not adopt"), "{e}");
}

// ── config ───────────────────────────────────────────────────────────────

#[test]
fn server_config_sets_only_the_public_url_and_setup_links_follow_it() {
    let t = t();
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    t.enable_with(Some("standalone")).unwrap();
    t.ops.clear();
    let out = crate::sew::supervisor::server_config(&t.env(), "https://hearth.lztek.k2.dev").unwrap();
    assert_eq!(out["restarted"], true);
    assert_eq!(out["setupLinks"], "https://hearth.lztek.k2.dev/setup");
    assert_eq!(t.ops.effects(), vec!["engine sew config", "engine sew systemctl restart"]);
    let body = t.stdin_of("engine sew config");
    assert_eq!(body, json!({"server.public_url": "https://hearth.lztek.k2.dev"}), "never identity.standalone.setup_url");
    let req: k2_root_proto::sew::ConfigRequest = serde_json::from_value(body).unwrap();
    req.validate().unwrap();
    t.ops.clear();
    for bad in ["https://evil.example", "https://hearth.lztek.k2.dev/setup", "http://hearth.lztek.k2.dev", ""] {
        assert!(crate::sew::supervisor::server_config(&t.env(), bad).is_err(), "{bad}");
    }
    assert!(t.ops.effects().is_empty());
}

#[test]
fn the_voice_url_never_rides_config_set() {
    let t = t();
    t.voice();
    set_url(&t.env(), "wss://hearth.lztek.k2.dev").unwrap();
    assert!(!t.ops.effects().iter().any(|e| e == "engine sew config"), "{:?}", t.ops.effects());
    // `media reconfigure` is `sew media install --reconfigure` with the
    // URL in its JSON input (the helper's own test pins that argv).
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(t.stdin_of("engine sew media reconfigure")).unwrap();
    req.validate_reconfigure().unwrap();
    assert_eq!(req.public_url.as_deref(), Some("wss://hearth.lztek.k2.dev"));
}

// ── users (standalone) ───────────────────────────────────────────────────

fn standalone(t: &T) {
    t.ops.mgmt_status.lock().unwrap()["provider"] = json!("standalone");
    t.enable_with(Some("standalone")).unwrap();
    t.ops.clear();
    t.ops.mgmt_bodies.lock().unwrap().clear();
}

fn link_answer(user: &str, role: &str) -> Value {
    json!({"member_id": "48831765147549703", "username": user, "install_role": role, "deactivated": false,
           "created": true, "reactivated": false, "changed": true,
           "setup_link": {"url": format!("https://hearth.lztek.k2.dev/setup#token=sewpw_{}", "z".repeat(43)), "expires_at": "2026-10-17T12:00:00.000Z"}})
}

fn queue_mgmt(t: &T, key: &str, a: Result<Value, MgmtError>) {
    t.ops.mgmt_answers.lock().unwrap().entry(key.into()).or_default().push_back(a);
}

#[test]
fn users_owner_invite_reset_role_revoke_go_to_sews_local_routes() {
    let t = t();
    standalone(&t);
    let env = t.env();
    queue_mgmt(&t, "POST /mgmt/v1/local/setup-link", Ok(link_answer("rosson", "owner")));
    let o = crate::sew::users::owner(&env, "rosson", Some("Rosson Long")).unwrap();
    assert!(o["setupLink"]["url"].as_str().unwrap().contains("#token=sewpw_"));
    queue_mgmt(&t, "POST /mgmt/v1/local/users", Ok(link_answer("ana-maria", "admin")));
    let i = crate::sew::users::invite(&env, "Ana.Maria@Acme.dev", None, "admin", None).unwrap();
    assert_eq!(i["installRole"], "admin");
    queue_mgmt(&t, "POST /mgmt/v1/local/setup-link", Ok(link_answer("bob", "member")));
    crate::sew::users::reset(&env, "bob").unwrap();
    queue_mgmt(&t, "POST /mgmt/v1/local/users/role", Ok(link_answer("bob", "admin")));
    crate::sew::users::role(&env, "bob", "admin").unwrap();
    queue_mgmt(&t, "POST /mgmt/v1/principals/deprovision", Ok(json!({"member_id": "48831765147549704", "sessions_revoked": 2})));
    let r = crate::sew::users::revoke(&env, "bob").unwrap();
    assert_eq!(r["sessionsRevoked"], 2);
    queue_mgmt(&t, "POST /mgmt/v1/local/users/reactivate", Ok(link_answer("bob", "member")));
    crate::sew::users::reactivate(&env, "bob").unwrap();
    let calls: Vec<String> = t.ops.mgmt_calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            "POST /mgmt/v1/local/setup-link",
            "POST /mgmt/v1/local/users",
            "POST /mgmt/v1/local/setup-link",
            "POST /mgmt/v1/local/users/role",
            "POST /mgmt/v1/principals/deprovision",
            "POST /mgmt/v1/local/users/reactivate",
        ]
    );
    let bodies: Vec<Value> = t.ops.mgmt_bodies.lock().unwrap().iter().map(|b| b.clone().unwrap()).collect();
    assert_eq!(bodies[0], json!({"username": "rosson", "owner": true, "display_name": "Rosson Long"}));
    assert_eq!(bodies[1], json!({"username": "ana-maria", "display_name": "Ana.Maria", "install_role": "admin", "email": "ana.maria@acme.dev"}));
    assert_eq!(bodies[2], json!({"username": "bob"}));
    assert_eq!(bodies[3], json!({"username": "bob", "install_role": "admin"}));
    assert_eq!(bodies[4], json!({"external_id": "local:bob"}));
    assert_eq!(bodies[5], json!({"username": "bob"}));
    // Nothing of the link reaches the voice record, the row or an effect.
    let row = format!("{:?}", store::load().unwrap());
    assert!(!row.contains("sewpw_"));
    assert!(t.ops.effects().is_empty(), "users never go through the root helper");
}

#[test]
fn users_refusals_map_and_bad_input_never_reaches_sew() {
    let t = t();
    standalone(&t);
    let env = t.env();
    // Sew's real error bodies (docs/mgmt.md; the 0.1.2 VM cycle answered
    // exactly this for a second `users owner`): the reason is in
    // error.details.reason.
    queue_mgmt(
        &t,
        "POST /mgmt/v1/local/users",
        Err(MgmtError::Status(
            409,
            r#"{"ok":false,"error":{"code":"conflict","hint":"the username is held by another active member","details":{"reason":"username_taken","field":"username"}}}"#.into(),
        )),
    );
    assert_eq!(crate::sew::users::invite(&env, "bob", None, "member", None).unwrap_err().code, "username_taken");
    queue_mgmt(
        &t,
        "POST /mgmt/v1/local/setup-link",
        Err(MgmtError::Status(409, r#"{"ok":false,"error":{"code":"conflict","hint":"x","details":{"reason":"owner_exists"}}}"#.into())),
    );
    let f = crate::sew::users::owner(&env, "rosson", None).unwrap_err();
    assert_eq!(f.code, "owner_exists");
    assert!(f.hint.contains("already has an owner"), "{}", f.hint);
    queue_mgmt(
        &t,
        "POST /mgmt/v1/local/users",
        Err(MgmtError::Status(403, r#"{"ok":false,"error":{"code":"forbidden","hint":"x","details":{"reason":"provider_is_k2"}}}"#.into())),
    );
    let f = crate::sew::users::invite(&env, "bob", None, "member", None).unwrap_err();
    assert_eq!(f.code, "provider_is_k2");
    assert!(f.hint.contains("--identity standalone"));
    t.ops.mgmt_calls.lock().unwrap().clear();
    assert_eq!(crate::sew::users::invite(&env, "bob", None, "owner", None).unwrap_err().code, "bad_request");
    assert_eq!(crate::sew::users::invite(&env, "!", None, "member", None).unwrap_err().code, "bad_request");
    assert_eq!(crate::sew::users::role(&env, "bob", "owner").unwrap_err().code, "bad_request");
    assert!(t.ops.mgmt_calls.lock().unwrap().is_empty());
}

#[test]
fn users_list_follows_every_page() {
    let t = t();
    standalone(&t);
    let item = |id: &str, u: &str| json!({"member_id": id, "external_id": format!("local:{u}"), "kind": "person", "install_role": "member", "username": u, "display_name": u, "deactivated": false});
    queue_mgmt(&t, "GET /mgmt/v1/principals?limit=500", Ok(json!({"items": [item("1", "a"), item("2", "b")], "has_more": true})));
    queue_mgmt(&t, "GET /mgmt/v1/principals?limit=500&after=2", Ok(json!({"items": [item("3", "c")], "has_more": false})));
    let l = crate::sew::users::list(&t.env()).unwrap();
    let names: Vec<&str> = l["users"].as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn users_need_a_running_server() {
    let t = t();
    assert_eq!(crate::sew::users::invite(&t.env(), "bob", None, "member", None).unwrap_err().code, "not_installed");
    standalone(&t);
    store::update(1, |r| r.status = "disabled".into()).unwrap();
    assert_eq!(crate::sew::users::list(&t.env()).unwrap_err().code, "not_running");
    let _ = INSTALL_ID;
}

// ── front443: the passthrough TURN tier ──────────────────────────────────

/// Behind the :443 front (S3): sew-media gets `turn_tls: "passthrough"`
/// with the same staged cert and key as direct, no advertise port (Sew's
/// passthrough default is 443, the only ICE URL), and K2 checks the served
/// leaf on 443.
#[test]
fn behind_the_front_voice_installs_on_the_passthrough_tier() {
    let t = t();
    *t.ops.front.lock().unwrap() = true;
    t.chat();
    let out = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap();
    assert_eq!(out["turnTier"], "passthrough");
    assert_eq!(out["turnTls"]["port"], 443);
    assert_eq!(out["ports"]["turnTls"], 443);
    let body = t.stdin_of("engine sew media install");
    assert_eq!(body["turn_tls"], "passthrough");
    assert_eq!(body["turn_domain"], "turn-lztek.k2.dev");
    assert!(body.get("turn_advertise_port").is_none(), "passthrough advertises 443 by Sew's default, never 5349");
    assert!(body["turn_cert_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(body["turn_key_pem"].as_str().unwrap().contains("PRIVATE KEY"));
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(body).unwrap();
    req.validate().expect("the helper accepts passthrough");
    assert_eq!(*t.ops.served_ports.lock().unwrap(), vec![443], "the served check probes :443");
    let st = t.state();
    assert_eq!(st.turn_tier.as_deref(), Some("passthrough"));
    assert_eq!(front_route_host(&t.env()).as_deref(), Some("turn-lztek.k2.dev"), "the front routes the TURN host");
    let (s, _, _) = status(&t.env(), true);
    assert_eq!(s["turnTier"], "passthrough");
    assert_eq!(s["turnTls"]["port"], 443);
}

/// `set_tier` moves an installed engine: one `media reconfigure
/// restart-now` with the tier, host and the SAME cert and key (no new
/// issuance), recorded; a second call is a no-op; back to direct restores
/// the 5349 advertise port.
#[test]
fn set_tier_moves_an_installed_engine_both_ways_without_reissuing() {
    let t = t();
    assert_eq!(set_tier(&t.env(), true).unwrap()["changed"], false, "nothing installed: nothing to move");
    t.voice();
    assert_eq!(t.state().turn_tier.as_deref(), Some("direct"));
    assert_eq!(front_route_host(&t.env()), None, "direct: no front route");
    let chain = t.state().cert_pem.unwrap();

    let v = set_tier(&t.env(), true).unwrap();
    assert_eq!(v["changed"], true);
    assert_eq!(v["turnTlsPort"], 443);
    assert_eq!(t.ops.effects(), vec!["engine sew media reconfigure restart-now"]);
    let body = t.stdin_of("engine sew media reconfigure restart-now");
    assert_eq!(body["turn_tls"], "passthrough");
    assert_eq!(body["turn_domain"], "turn-lztek.k2.dev");
    assert_eq!(body["turn_cert_pem"].as_str(), Some(chain.as_str()), "the cert K2 already holds");
    assert!(body.get("turn_advertise_port").is_none());
    let req: k2_root_proto::sew::MediaRequest = serde_json::from_value(body).unwrap();
    req.validate_reconfigure().expect("the helper accepts the reconfigure");
    assert!(t.ops.cp_log().is_empty(), "no TURN DNS or cert call: no issuance spent");
    assert_eq!(t.state().turn_tier.as_deref(), Some("passthrough"));

    t.ops.clear();
    assert_eq!(set_tier(&t.env(), true).unwrap()["changed"], false);
    assert!(t.ops.effects().is_empty(), "already there: no helper call");

    let v = set_tier(&t.env(), false).unwrap();
    assert_eq!(v["changed"], true);
    let body = t.stdin_of("engine sew media reconfigure restart-now");
    assert_eq!(body["turn_tls"], "direct");
    assert_eq!(body["turn_advertise_port"], 5349);
    assert_eq!(t.state().turn_tier.as_deref(), Some("direct"));
}

/// Sew 0.2.0 has no passthrough tier (it arrives in 0.2.1): behind the
/// front, voice installs on `direct` with a warning, and a move to
/// passthrough is refused with a clear code, never a half-moved engine.
#[test]
fn a_sew_without_passthrough_keeps_turn_on_5349() {
    let t = t();
    *t.ops.front.lock().unwrap() = true;
    *t.ops.passthrough.lock().unwrap() = false;
    t.chat();
    let out = t.install(InstallRequest { tarball_path: Some(MEDIA.into()), ..Default::default() }).unwrap();
    assert_eq!(out["turnTier"], "direct");
    assert_eq!(t.stdin_of("engine sew media install")["turn_tls"], "direct");
    let w = out["warnings"].as_array().unwrap();
    assert!(w.iter().any(|x| x.as_str().unwrap().contains("no passthrough TURN tier")), "{w:?}");
    t.ops.clear();
    let f = set_tier(&t.env(), true).unwrap_err();
    assert_eq!(f.code, "sew_passthrough_unsupported");
    assert!(f.hint.contains("0.2.1"), "{}", f.hint);
    assert!(t.ops.effects().is_empty(), "no helper call");
    assert_eq!(t.state().turn_tier.as_deref(), Some("direct"));
}
