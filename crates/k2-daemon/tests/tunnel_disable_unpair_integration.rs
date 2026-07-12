//! PRD `prd-tunnel-disable-unpair-v1.md` — the two-intent split, driven
//! end-to-end through the REAL dispatcher (`k2_daemon::test_harness`,
//! cribbed from `tunnel_config_role_gate_integration.rs`):
//!
//!   1. `POST /cli/tunnel/disable` persists `enabled: false` INTO
//!      `~/.k2/tunnel.json` (the on-disk pause — restarts/reboots/orphans
//!      all read it), and `/enable` symmetrically removes it. The status
//!      route reports the tri-state truthfully either way.
//!   2. `POST /cli/tunnel/release` without `confirm=1` is a 400 no-op
//!      (destructive verbs never fire from a bare POST); with it, the
//!      device identity is DELETED (tunnel.json gone), the
//!      `~/.k2/unpaired.json` tombstone is written (upstream revocation
//!      queued — the control plane is pointed at a dead port), and
//!      status/config report `released: true`.
//!
//! Auth/method tiers (owner-token-only, POST-only) are pinned in
//! `tunnel_config_role_gate_integration.rs` item 6 — not repeated here.
//!
//! ISOLATION: `$HOME` (→ `~/.k2/`) and the `K2_CONNECT_BASE` env are
//! process-wide — every test serializes on `TEST_LOCK` and redirects HOME
//! to a fresh tempdir.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-disable-unpair";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream =
        StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Redirect `$HOME` to a fresh tempdir AND point the control plane at a
/// dead local port (upstream release reports fail fast + offline-queue —
/// a unit test must never dial the real connect.k2.dev). Caller holds
/// `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev_home = std::env::var_os("HOME");
    let cp_var = k2_core::tunnel::subdomains::CONTROL_PLANE_BASE_ENV;
    let prev_cp = std::env::var_os(cp_var);
    std::env::set_var(cp_var, "http://127.0.0.1:9");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir()
        .join(format!("k2-tunnel-unpair-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);

    f();

    match prev_home {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    match prev_cp {
        Some(p) => std::env::set_var(cp_var, p),
        None => std::env::remove_var(cp_var),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

/// Seed a paired identity on disk the way the pairing flow leaves it.
fn seed_identity(subdomain: &str, token: &str) {
    k2_core::tunnel::config::save(&k2_core::tunnel::TunnelConfig {
        token: token.to_string(),
        subdomain: subdomain.to_string(),
        device_id: Some("dev-integration".to_string()),
        e2e: false,
        ..Default::default()
    })
    .expect("seed tunnel.json");
}

fn tunnel_json_raw() -> String {
    std::fs::read_to_string(k2_core::tunnel::config::config_path()).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────
// (1) disable persists ON DISK; enable removes it; status reports the
//     tri-state truthfully at each step.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disable_persists_to_disk_and_enable_is_symmetric() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            seed_identity("unpairtest", "tok-disable-int");

            // Disable → 200, and the flag is IN tunnel.json (the on-disk
            // persistence a restarted/orphaned daemon reads — the whole
            // point of the PRD).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/disable?token={OWNER_TOKEN}"),
                Some(""),
            );
            assert_eq!(r.status, 200, "disable must succeed; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["enabled"], serde_json::json!(false), "{v}");
            assert_eq!(v["running"], serde_json::json!(false), "{v}");
            let raw = tunnel_json_raw();
            assert!(
                raw.contains("\"enabled\": false"),
                "disable must persist in tunnel.json, not daemon memory\n{raw}"
            );
            // Identity untouched: pause is not divorce.
            assert!(raw.contains("tok-disable-int"), "token must survive a disable\n{raw}");

            // Status route reports the tri-state honestly.
            let r = http(
                d.port,
                "GET",
                &format!("/cli/tunnel/status?token={OWNER_TOKEN}"),
                None,
            );
            assert_eq!(r.status, 200, "status; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["enabled"], serde_json::json!(false), "{v}");
            assert_eq!(v["released"], serde_json::json!(false), "{v}");

            // Enable → the persisted flag comes OFF disk again (default
            // shape: the key disappears entirely). The config is made
            // deliberately NOT connectable first (no token) so the route
            // deterministically persists the flag + returns status without
            // ever attempting a real frpc spawn from a test.
            k2_core::tunnel::config::save(&k2_core::tunnel::TunnelConfig {
                subdomain: "unpairtest".to_string(),
                enabled: false,
                e2e: false,
                ..Default::default()
            })
            .expect("reseed token-less disabled config");
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/enable?token={OWNER_TOKEN}"),
                Some(""),
            );
            assert_eq!(r.status, 200, "enable must succeed; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["enabled"], serde_json::json!(true), "{v}");
            let raw = tunnel_json_raw();
            assert!(
                !raw.contains("\"enabled\": false"),
                "enable must clear the persisted disable\n{raw}"
            );
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// (2) release: confirm-gated 400 no-op without confirm=1; with it, the
//     identity is deleted, the tombstone written, and the tri-state flips
//     to released.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_requires_confirm_then_deletes_identity_and_tombstones() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        futures_block(async {
            seed_identity("unpairrel", "tok-release-int");

            // Bare POST (no confirm) → 400, and NOTHING happened.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/release?token={OWNER_TOKEN}"),
                Some(""),
            );
            assert_eq!(r.status, 400, "unconfirmed release must 400; body={}", r.body);
            assert!(
                r.body.contains("confirm"),
                "the 400 must tell the caller HOW to confirm: {}",
                r.body
            );
            assert!(
                tunnel_json_raw().contains("tok-release-int"),
                "an unconfirmed release must not touch the identity"
            );
            assert!(
                !k2_core::tunnel::unpair::tombstone_path().exists(),
                "an unconfirmed release must not tombstone"
            );

            // Confirmed release → identity gone, tombstone written, and
            // the upstream report honestly QUEUED (control plane is a
            // dead port in this test).
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/release?confirm=1&token={OWNER_TOKEN}"),
                Some(""),
            );
            assert_eq!(r.status, 200, "confirmed release must succeed; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["subdomain"], serde_json::json!("unpairrel"), "{v}");
            assert_eq!(v["upstreamReported"], serde_json::json!(false), "{v}");
            assert_eq!(v["alreadyReleased"], serde_json::json!(false), "{v}");

            assert!(
                !k2_core::tunnel::config::config_path().exists(),
                "release must DELETE tunnel.json"
            );
            let tomb = std::fs::read_to_string(k2_core::tunnel::unpair::tombstone_path())
                .expect("tombstone must exist after release");
            assert!(tomb.contains("unpairrel"), "tombstone names the subdomain\n{tomb}");
            assert!(
                tomb.contains("\"upstream_reported\": false"),
                "offline release must queue the revocation\n{tomb}"
            );

            // The tri-state: status + config view both report released.
            let r = http(
                d.port,
                "GET",
                &format!("/cli/tunnel/status?token={OWNER_TOKEN}"),
                None,
            );
            let v = json(&r.body);
            assert_eq!(v["released"], serde_json::json!(true), "{v}");
            assert_eq!(v["running"], serde_json::json!(false), "{v}");
            let r = http(
                d.port,
                "GET",
                &format!("/cli/tunnel/config?token={OWNER_TOKEN}"),
                None,
            );
            let v = json(&r.body);
            assert_eq!(v["released"], serde_json::json!(true), "{v}");
            assert_eq!(v["tokenSet"], serde_json::json!(false), "{v}");

            // Releasing again is idempotent-honest.
            let r = http(
                d.port,
                "POST",
                &format!("/cli/tunnel/release?confirm=1&token={OWNER_TOKEN}"),
                Some(""),
            );
            assert_eq!(r.status, 200, "re-release; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["alreadyReleased"], serde_json::json!(true), "{v}");
        });
    });
}
