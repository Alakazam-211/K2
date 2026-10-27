//! TLS-ALPN-01 through the App TLS door against a real ACME server,
//! Pebble (Let's Encrypt's test CA), never Let's Encrypt itself
//! (0.46.6, vs-live CD-A10; PRD test 3's CA half).
//!
//! Run on request (it starts two Go binaries):
//!
//! ```text
//! PEBBLE_BIN=…/pebble PEBBLE_CHALLTESTSRV_BIN=…/pebble-challtestsrv \
//! PEBBLE_CERTS_DIR=…/pebble/v2@v2.6.0/test/certs \
//!   cargo test -p k2-daemon --lib pebble -- --ignored
//! ```
//!
//! Build them with `go install github.com/letsencrypt/pebble/v2/cmd/pebble@v2.6.0`
//! (and `…/cmd/pebble-challtestsrv@v2.6.0`). A missing variable fails the
//! test loudly: it never passes by skipping.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::*;
use crate::apps::custom_domains::test_util::runtime_app;

const DOOR_PORT: u16 = 15_001;
const DNS_PORT: u16 = 18_053;
const DIR_PORT: u16 = 14_000;

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn need(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("{var} must point at the Pebble build (see this file's header)"))
}

#[tokio::test]
#[ignore = "needs Pebble; run with --ignored and PEBBLE_* set (see the file header)"]
async fn pebble_issues_a_custom_app_name_through_the_door_by_tls_alpn_01() {
    // The daemon installs aws-lc-rs at boot; the ACME client needs it here.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let _db = k2_core::db::scoped_for_test();
    let _home = crate::test_support::TempHome::new();
    let pebble = need("PEBBLE_BIN");
    let chall = need("PEBBLE_CHALLTESTSRV_BIN");
    let certs = std::path::PathBuf::from(need("PEBBLE_CERTS_DIR"));

    // The App and its name, proven: `issuing`, routed to the door.
    let host = "shop.pebble-cd.test";
    runtime_app("ws-a", "jobs");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "pebble-cd.test", None, false).unwrap();
        k2_core::domains::upsert_name(&conn, host, "pebble-cd.test", "app:jobs").unwrap();
        k2_core::domains::set_app_state(&conn, host, k2_core::domains::STATE_ISSUING, None).unwrap();
    }
    assert_eq!(crate::apps::tls_door::reload().routed, vec![host.to_string()]);

    // The door where Pebble's validator dials (tlsPort).
    let l = tokio::net::TcpListener::bind(("127.0.0.1", DOOR_PORT)).await.expect("bind the door's test port");
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let _ = crate::apps::tls_door::serve_conn(tcp, crate::apps::tls_door::server_config()).await;
            });
        }
    });

    // Mock DNS: every name → 127.0.0.1. Pebble with that resolver.
    let _dns = Kill(
        Command::new(&chall)
            .args(["-defaultIPv4", "127.0.0.1", "-defaultIPv6", "", "-dns01", &format!("127.0.0.1:{DNS_PORT}")])
            .args(["-http01", "", "-https01", "", "-tlsalpn01", "", "-doh", ""])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pebble-challtestsrv"),
    );
    let home = std::env::var("HOME").unwrap();
    let cfg_path = std::path::Path::new(&home).join("pebble.json");
    let cfg = serde_json::json!({
        "pebble": {
            "listenAddress": format!("127.0.0.1:{DIR_PORT}"),
            "managementListenAddress": "127.0.0.1:15000",
            "certificate": certs.join("localhost/cert.pem"),
            "privateKey": certs.join("localhost/key.pem"),
            "httpPort": 15_002,
            "tlsPort": DOOR_PORT,
            "ocspResponderURL": "",
            "externalAccountBindingRequired": false,
        }
    });
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let _ca = Kill(
        Command::new(&pebble)
            .args(["-config", cfg_path.to_str().unwrap(), "-dnsserver", &format!("127.0.0.1:{DNS_PORT}")])
            .env("PEBBLE_VA_NOSLEEP", "1")
            .env("PEBBLE_WFE_NONCEREJECT", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pebble"),
    );
    let mut up = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", DIR_PORT)).await.is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "pebble never listened on {DIR_PORT}");

    let binding = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::get_binding(&conn, "pebble-cd.test").unwrap().unwrap()
    };
    let env = AcmeEnv {
        directory: Some(format!("https://localhost:{DIR_PORT}/dir")),
        root_pem: Some(certs.join("pebble.minica.pem")),
    };
    let pem = tokio::time::timeout(Duration::from_secs(120), live_issue_async(host, &binding, ChallengeKind::TlsAlpn01, &env))
        .await
        .expect("issued within 2 minutes")
        .expect("Pebble validated TLS-ALPN-01 through the door and issued");
    assert!(!crate::apps::tls_door::challenge_pending(host), "the challenge cert is gone after the order");
    let der = rustls_pemfile::certs(&mut pem.chain_pem.as_bytes()).next().unwrap().unwrap();
    let (_, leaf) = x509_parser::parse_x509_certificate(der.as_ref()).unwrap();
    let sans: Vec<String> = leaf
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names
        .iter()
        .filter_map(|n| match n {
            x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(sans, vec![host.to_string()], "one cert per name, no SAN bundling");
    assert!(leaf.issuer().to_string().to_ascii_lowercase().contains("pebble"), "{}", leaf.issuer());

    // Served: the new cert goes live at the door.
    crate::domains::store::install(host, &pem.chain_pem, &pem.key_pem).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::set_app_state(&conn, host, k2_core::domains::STATE_LIVE, None).unwrap();
    }
    let r = crate::apps::tls_door::reload();
    assert!(r.without_cert.is_empty(), "{r:?}");
    assert_eq!(crate::apps::tls_door::app_for_host(host), Some("jobs".to_string()));
}
