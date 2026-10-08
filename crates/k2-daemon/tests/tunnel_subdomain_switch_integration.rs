//! 0.45.2 — switching this device's K2 Connect subdomain moves the E2E
//! listener and its certificate to the new name WITHOUT a daemon restart.
//!
//! Field bug (Reggie's Mac mini, 0.45.0): the owner released `afs` and
//! connected `k2n1-m4` in Settings. tunnel.json and frpc moved, but the
//! E2E TLS listener kept serving the `afs.k2.dev` certificate (and its
//! renewer kept renewing `afs`), so every direct client failed the TLS
//! handshake with `CertificateUnknown` until the daemon was restarted.
//!
//! Everything here is hermetic: a temp `$HOME`, a loopback cert broker
//! that signs leaves with a test CA (for whatever name it is asked, with
//! the daemon's own key), a control plane that answers 503, no frpc (the
//! tunnel is never started; the listener is brought up through the same
//! ensure hook a tunnel start uses).
//!
//! One shared tokio runtime for the whole file: the daemon's ensure hook
//! captures the runtime handle on first registration (first write wins),
//! so every test must run on the runtime that registered it.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_daemon::test_harness;

const OWNER_TOKEN: &str = "owner-token-subdomain-switch";

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime")
    })
}

static TEST_LOCK: Mutex<()> = Mutex::new(());

// ── A loopback cert broker backed by a test CA ─────────────────────────

struct CaBroker {
    url: String,
    ca_pem: String,
    /// `(subdomain, token)` of every request, in order.
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

fn read_http_request(sock: &mut StdTcpStream) -> String {
    sock.set_read_timeout(Some(Duration::from_secs(5))).expect("read timeout");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if body.len() >= len {
                return body.to_string();
            }
        }
        match sock.read(&mut chunk) {
            Ok(0) | Err(_) => return String::new(),
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
        }
    }
}

impl CaBroker {
    fn start(daemon_key: PathBuf) -> Self {
        let ca_kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca key");
        let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "K2 Switch Test CA");
        let ca_cert = ca_params.self_signed(&ca_kp).expect("ca cert");
        let ca_pem = ca_cert.pem();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind broker");
        let url = format!("http://{}/cert", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_t = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { continue };
                let body = read_http_request(&mut sock);
                let v: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let sub = v["subdomain"].as_str().unwrap_or_default().to_string();
                let token = v["token"].as_str().unwrap_or_default().to_string();
                seen_t.lock().expect("seen").push((sub.clone(), token));
                // The broker signs the daemon's CSR: the leaf carries the
                // daemon's public key and exactly the requested name.
                let key_pem = std::fs::read_to_string(&daemon_key).expect("daemon key on disk");
                let daemon_kp = rcgen::KeyPair::from_pem(&key_pem).expect("parse daemon key");
                let leaf = rcgen::CertificateParams::new(k2_core::tunnel::tls::sans_for(&sub))
                    .expect("leaf params")
                    .signed_by(&daemon_kp, &ca_cert, &ca_kp)
                    .expect("sign leaf");
                let resp_body =
                    serde_json::json!({ "cert": format!("{}{}", leaf.pem(), ca_cert.pem()) })
                        .to_string();
                let _ = sock.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Self { url, ca_pem, seen }
    }

    fn seen(&self) -> Vec<(String, String)> {
        self.seen.lock().expect("seen").clone()
    }
}

/// TLS-handshake 127.0.0.1:`port` with SNI `sni`, trusting only the test
/// CA. `Ok` = the listener presented a valid certificate for `sni`.
async fn handshake(ca_pem: &str, port: u16, sni: &str) -> Result<(), String> {
    use tokio_rustls::TlsConnector;
    let mut roots = rustls::RootCertStore::empty();
    for item in rustls_pemfile::read_all(&mut ca_pem.as_bytes()) {
        if let Ok(rustls_pemfile::Item::X509Certificate(der)) = item {
            roots.add(der).expect("add CA root");
        }
    }
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("SNI");
    tokio::time::timeout(
        Duration::from_secs(10),
        TlsConnector::from(Arc::new(cfg)).connect(name, tcp),
    )
    .await
    .map_err(|_| "handshake timed out".to_string())?
    .map(|_| ())
    .map_err(|e| format!("handshake: {e}"))
}

/// The tunnel start's own path to the listener: the k2-core ensure hook
/// (blocking; the daemon hook drives the async bring-up on the runtime).
async fn ensure(daemon_port: u16) -> u16 {
    tokio::task::spawn_blocking(move || k2_core::tunnel::tls::ensure_https_port(daemon_port))
        .await
        .expect("join")
        .expect("ensure the E2E listener")
}

/// Wait until nothing accepts on `port` any more (the retired accept task
/// is aborted asynchronously). Fails loudly after 5 s.
async fn wait_port_closed(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return;
        }
        assert!(Instant::now() < deadline, "old listener port {port} still accepts after 5 s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status line: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body:?}"))
}

fn seed(subdomain: &str, token: &str) {
    k2_core::tunnel::config::save(&k2_core::tunnel::TunnelConfig {
        token: token.to_string(),
        subdomain: subdomain.to_string(),
        device_id: Some("dev-app".to_string()),
        ..Default::default()
    })
    .expect("seed tunnel.json");
}

fn identity_events(rx: &mut tokio::sync::broadcast::Receiver<k2_daemon::session_events::SessionEvent>) -> Vec<(Option<String>, Option<String>, bool)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let k2_daemon::session_events::SessionEvent::TunnelIdentityChanged {
            previous_subdomain,
            subdomain,
            ok,
            ..
        } = ev
        {
            out.push((previous_subdomain, subdomain, ok));
        }
    }
    out
}

#[test]
fn switching_subdomains_moves_the_e2e_listener_and_cert_without_a_restart() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let _home = k2_core::test_env::TempHome::new();
    let _e2e = k2_core::test_env::EnvVar::remove("K2_E2E"); // E2E on (default)
    let _air = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
    let _ss = k2_core::test_env::EnvVar::remove("K2_E2E_SELF_SIGNED");
    let _lease = k2_core::test_env::EnvVar::set("K2_TUNNEL_LEASE", "off");
    let cp = k2_core::test_env::ErrorHttpServer::start(503);
    let _cp = k2_core::test_env::EnvVar::set(
        k2_core::tunnel::subdomains::CONTROL_PLANE_BASE_ENV,
        cp.url(),
    );
    let broker = CaBroker::start(k2_core::tunnel::tls::key_path());
    let _broker = k2_core::test_env::EnvVar::set(
        k2_core::tunnel::cert_broker::BROKER_URL_ENV,
        broker.url.clone(),
    );

    rt().block_on(async {
        k2_daemon::tunnel_tls_listener::register_ensure_hook();
        let d = test_harness::start(OWNER_TOKEN).await;
        let mut events = k2_daemon::session_events::subscribe();

        // ── 1. Paired as afs: listener + renewer serve afs.k2.dev. ──────
        seed("afs", "tok-afs");
        let p1 = ensure(d.port).await;
        assert_eq!(
            k2_daemon::tunnel_tls_listener::live_listener(),
            Some((p1, "afs".to_string()))
        );
        assert_eq!(
            k2_daemon::tunnel_tls_listener::live_renewer_subdomain().as_deref(),
            Some("afs")
        );
        handshake(&broker.ca_pem, p1, "afs.k2.dev")
            .await
            .expect("afs.k2.dev handshake on the afs listener");

        // ── 2. The 0.45.0 field path: tunnel.json moves to k2n1-m4 under
        //       the listener (no identity hook ran), then the tunnel start
        //       ensures its listener. It must be REPLACED, not reused. ───
        seed("k2n1-m4", "tok-new");
        let p2 = ensure(d.port).await;
        assert_ne!(p2, p1, "a new name must get a new listener");
        assert_eq!(
            k2_daemon::tunnel_tls_listener::live_listener(),
            Some((p2, "k2n1-m4".to_string()))
        );
        assert_eq!(
            k2_daemon::tunnel_tls_listener::live_renewer_subdomain().as_deref(),
            Some("k2n1-m4"),
            "the cert renewer must follow the new name"
        );
        handshake(&broker.ca_pem, p2, "k2n1-m4.k2.dev")
            .await
            .expect("k2n1-m4.k2.dev must handshake after the switch (the field bug)");
        let old = handshake(&broker.ca_pem, p2, "afs.k2.dev")
            .await
            .expect_err("the old name must not validate on the new listener");
        assert!(old.contains("handshake"), "clean TLS failure, got: {old}");
        wait_port_closed(p1).await;
        assert_eq!(
            k2_core::tunnel::tls::read_https_port(),
            Some(p2),
            "tunnel-https.port must name the live listener"
        );
        assert_eq!(
            broker.seen(),
            vec![
                ("afs".to_string(), "tok-afs".to_string()),
                ("k2n1-m4".to_string(), "tok-new".to_string()),
            ],
            "the broker was asked for the new name with the new token"
        );
        let r = http(d.port, "GET", &format!("/cli/tunnel/status?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(
            json(&r.body)["e2eListener"],
            serde_json::json!({ "subdomain": "k2n1-m4", "port": p2 })
        );

        // ── 3. Switch BACK to afs through the route (tunnel off): the
        //       daemon retires the k2n1-m4 listener at once, and the next
        //       start brings afs back with a valid cert. ─────────────────
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/switch?token={OWNER_TOKEN}"),
            Some(r#"{"subdomain":"afs","token":"tok-afs","deviceId":"dev-app","start":false}"#),
        );
        assert_eq!(r.status, 200, "switch: {}", r.body);
        let v = json(&r.body);
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["previousSubdomain"], serde_json::json!("k2n1-m4"), "{v}");
        assert_eq!(v["subdomain"], serde_json::json!("afs"), "{v}");
        let steps: Vec<String> = v["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .map(|s| format!("{}:{}", s["step"].as_str().unwrap_or(""), s["detail"].as_str().unwrap_or("")))
            .collect();
        assert!(
            steps.iter().any(|s| s.starts_with("listener:") && s.contains("k2n1-m4.k2.dev")),
            "the switch must report stopping the k2n1-m4 listener: {steps:?}"
        );
        assert_eq!(k2_daemon::tunnel_tls_listener::live_listener(), None);
        assert_eq!(k2_daemon::tunnel_tls_listener::live_renewer_subdomain(), None);
        wait_port_closed(p2).await;
        assert_eq!(
            identity_events(&mut events),
            vec![(Some("k2n1-m4".to_string()), Some("afs".to_string()), true)],
            "clients are told the tunnel moved"
        );
        let p3 = ensure(d.port).await;
        handshake(&broker.ca_pem, p3, "afs.k2.dev")
            .await
            .expect("afs.k2.dev handshake after switching back");

        // ── 4. A config write with a new subdomain + token (the Settings
        //       picker's bind) moves the daemon too. ─────────────────────
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/config?token={OWNER_TOKEN}"),
            Some(r#"{"subdomain":"k2n2-m1","token":"tok-third","deviceId":"dev-app"}"#),
        );
        assert_eq!(r.status, 200, "config: {}", r.body);
        let v = json(&r.body);
        assert_eq!(v["subdomain"], serde_json::json!("k2n2-m1"), "{v}");
        assert_eq!(v["identityChange"]["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["identityChange"]["previousSubdomain"], serde_json::json!("afs"), "{v}");
        assert_eq!(k2_daemon::tunnel_tls_listener::live_listener(), None);
        wait_port_closed(p3).await;

        // A config write that changes nothing about the identity (the app
        // re-pushing its device id) touches nothing.
        let p4 = ensure(d.port).await;
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/config?token={OWNER_TOKEN}"),
            Some(r#"{"deviceId":"dev-app","deviceLabel":"MacIntel"}"#),
        );
        assert_eq!(r.status, 200, "config: {}", r.body);
        assert!(json(&r.body).get("identityChange").is_none(), "{}", r.body);
        assert_eq!(
            k2_daemon::tunnel_tls_listener::live_listener(),
            Some((p4, "k2n2-m1".to_string()))
        );

        // ── 5. Forget locally: the listener goes, nothing is sent. ───────
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/release?confirm=1&mode=forget&token={OWNER_TOKEN}"),
            Some(""),
        );
        assert_eq!(r.status, 200, "forget: {}", r.body);
        let v = json(&r.body);
        assert_eq!(v["localOnly"], serde_json::json!(true), "{v}");
        assert_eq!(v["upstreamReported"], serde_json::json!(false), "{v}");
        assert_eq!(v["queued"], serde_json::json!(false), "{v}");
        assert_eq!(v["identityChange"]["subdomain"], serde_json::Value::Null, "{v}");
        assert_eq!(k2_daemon::tunnel_tls_listener::live_listener(), None);
        wait_port_closed(p4).await;
        assert!(
            k2_core::tunnel::tls::key_path().exists(),
            "the federation key survives a forget/release"
        );

        // ── 6. Connect a new name on the same device after the forget: the
        //       app re-sends its device id, which must not read as the
        //       released identity. ─────────────────────────────────────────
        let r = http(
            d.port,
            "POST",
            &format!("/cli/tunnel/switch?token={OWNER_TOKEN}"),
            Some(r#"{"subdomain":"k2n1-m4","token":"tok-new","deviceId":"dev-app","start":false}"#),
        );
        assert_eq!(r.status, 200, "switch after forget: {}", r.body);
        assert_eq!(json(&r.body)["ok"], serde_json::json!(true), "{}", r.body);
        let r = http(d.port, "GET", &format!("/cli/tunnel/status?token={OWNER_TOKEN}"), None);
        assert_eq!(json(&r.body)["released"], serde_json::json!(false), "{}", r.body);
        let p5 = ensure(d.port).await;
        handshake(&broker.ca_pem, p5, "k2n1-m4.k2.dev")
            .await
            .expect("k2n1-m4 after release + reconnect on the same device");
    });
}

#[test]
fn switch_input_errors_change_nothing() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _home = k2_core::test_env::TempHome::new();
    let cp = k2_core::test_env::ErrorHttpServer::start(503);
    let _cp = k2_core::test_env::EnvVar::set(
        k2_core::tunnel::subdomains::CONTROL_PLANE_BASE_ENV,
        cp.url(),
    );
    rt().block_on(async {
        let d = test_harness::start(OWNER_TOKEN).await;
        seed("afs", "tok-afs");
        let before = std::fs::read_to_string(k2_core::tunnel::config::config_path()).expect("cfg");
        for (body, needle) in [
            (r#"{"subdomain":"Bad Label!","token":"t"}"#, "invalid subdomain"),
            (r#"{"subdomain":"other"}"#, "needs its tunnel token"),
            ("not json", "invalid JSON"),
        ] {
            let r = http(d.port, "POST", &format!("/cli/tunnel/switch?token={OWNER_TOKEN}"), Some(body));
            assert_eq!(r.status, 400, "{body}: {}", r.body);
            assert!(r.body.contains(needle), "{body}: {}", r.body);
        }
        let r = http(d.port, "GET", &format!("/cli/tunnel/switch?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 405, "GET must never switch: {}", r.body);
        let r = http(
            d.port,
            "POST",
            "/cli/tunnel/switch?token=not-the-owner",
            Some(r#"{"subdomain":"other","token":"t"}"#),
        );
        assert_eq!(r.status, 403, "owner only: {}", r.body);
        assert_eq!(
            std::fs::read_to_string(k2_core::tunnel::config::config_path()).expect("cfg"),
            before,
            "a refused switch must not touch tunnel.json"
        );
    });
}
