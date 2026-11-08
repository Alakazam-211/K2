//! CI on K2 Compute ships dark in 0.46.10: its own preview switch,
//! `ciPreview` (`k2 settings set ciPreview true`, or `K2_CI=1`), off by
//! default and on top of compute's (`computePreview`). The real dispatcher
//! in-process (temp `$HOME`, loopback, the vault on a file key there), with
//! compute ON and the CI preview OFF:
//!
//! - every route CI on compute added (`k2 ci`: `ci_routes::ROUTES`; node
//!   rosters, lanes, seats, owners, trust requests, signer pairing:
//!   `compute_trust::ROUTES`) answers 409 `ci_preview_off`, naming how the
//!   owner turns it on, for the owner and for an agent;
//! - the federation leg (`/cli/federation/compute-seat`) refuses a seat
//!   request, offer or roster the same way, before reading the envelope;
//! - `receiptV: 2` on `/cli/compute/run` and a lane grant
//!   (`compute:<ws>/lane:<owner>/<lane>`) are refused the same way, while a
//!   plain (v1) compute run is untouched;
//! - no CI table gets a row.
//!
//! Then the owner turns `ciPreview` on (no restart): the same routes answer
//! as their handlers do (the GET views 200), and off again: dark again.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-ci-preview-gate";
const HOW: &str = "k2 settings set ciPreview true";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        k2_core::compute::set_ci_enabled(false);
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cig-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    // Compute ON (0.46.9's switch), the CI preview OFF (0.46.10's default).
    std::env::set_var("K2_COMPUTE", "1");
    std::env::remove_var("K2_CI");
    k2_core::compute::set_ci_enabled(false);
    env
}

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len()),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// `token` empty: no credential (the federation leg's own envelope is it).
async fn call(port: u16, method: &str, path: &str, token: &str, query: &str, body: Option<Value>) -> Resp {
    let mut pq = path.to_string();
    let mut q: Vec<String> = Vec::new();
    if !token.is_empty() {
        q.push(format!("token={}", enc(token)));
    }
    if !query.is_empty() {
        q.push(query.to_string());
    }
    if !q.is_empty() {
        pq = format!("{pq}?{}", q.join("&"));
    }
    let method = method.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, &method, &pq, body.map(|b| b.to_string()))).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn err_code(r: &Resp) -> Option<String> {
    serde_json::from_str::<Value>(&r.body).ok().and_then(|v| v.pointer("/error/code").and_then(Value::as_str).map(str::to_string))
}

/// The preview-off refusal, exactly: 409, `ci_preview_off`, and the hint
/// says how the owner turns it on.
fn assert_dark(r: &Resp, what: &str) {
    assert_eq!((r.status, err_code(r).as_deref()), (409, Some(k2_core::compute::CI_PREVIEW_OFF)), "{what}: {}", r.body);
    let hint = j(r).pointer("/error/hint").and_then(Value::as_str).unwrap_or_default().to_string();
    assert!(hint.contains(HOW), "{what}: the hint names the switch: {hint}");
}

fn assert_not_dark(r: &Resp, what: &str) {
    assert_ne!(err_code(r).as_deref(), Some(k2_core::compute::CI_PREVIEW_OFF), "{what}: {} {}", r.status, r.body);
}

/// Every table CI on compute writes (migrations 0273–0275).
const CI_TABLES: &[&str] = &[
    "ci_workspaces",
    "ci_seen",
    "ci_runs",
    "ci_checks",
    "ci_signers",
    "compute_node_statements",
    "compute_roster_entries",
    "compute_node_rosters",
    "compute_lanes",
    "compute_trust_requests",
    "compute_seats",
    "compute_owner_trust",
    "compute_key_revocations",
];

fn ci_rows() -> Vec<(String, i64)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    CI_TABLES
        .iter()
        .map(|t| (t.to_string(), conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get::<_, i64>(0)).unwrap_or_else(|e| panic!("{t}: {e}"))))
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// Every route CI on compute put on the compute router, with its method.
fn ci_routes() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for p in k2_daemon::ci_routes::ROUTES.iter().chain(k2_daemon::compute_trust::ROUTES.iter()) {
        assert!(k2_daemon::compute_routes::is_ci_route(p), "{p} is a CI route");
        out.push((*p, if k2_daemon::compute_routes::is_post_route(p) { "POST" } else { "GET" }));
    }
    assert_eq!(out.len(), 25, "5 `k2 ci` routes + 20 roster/lane/seat/trust/owner/signer routes: {out:?}");
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_ci_route_is_dark_until_the_owner_turns_the_ci_preview_on() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    assert!(k2_core::compute::enabled() && !k2_core::compute::ci_enabled(), "compute on, CI off");

    // A workspace whose agent may use compute.
    let ws_id = uuid::Uuid::new_v4().to_string();
    let ws_path = env.home.join("ws-gate");
    std::fs::create_dir_all(&ws_path).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'gate', ?2, 'gate')", params![ws_id, ws_path.to_string_lossy()]).unwrap();
        conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![ws_id]).unwrap();
    }
    let sid = SessionId::new();
    let agent = k2_daemon::session_token::mint_session_token(&sid, &sid.to_string(), HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "agent".into() }, CredMode::ApiKey, Provider::Anthropic);
    let secret_id = {
        let v = k2_daemon::vault::get().expect("vault open");
        let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws_id.clone()), "GATE_BUILD_TOKEN", Kind::ApiKey);
        r.hosts = vec!["example.com".into()];
        v.put(r, SecretBytes::from_slice(b"k2test-gate-value"), &Actor::Owner).expect("put").meta.id
    };
    let lane_grant = |secret_id: &str| {
        let confirm = k2_daemon::vault::get().unwrap().confirm.mint("grant-read", Some(secret_id), "owner", "local-auth");
        json!({"id": secret_id, "to": "compute:gate/lane:owner.k2.dev/ci-standard", "as": "env", "envName": "GATE_BUILD_TOKEN", "confirmToken": confirm})
    };
    let run_v2 = json!({"plan": true, "argv": ["true"], "receiptV": 2, "noSource": true});
    let run_v1 = json!({"plan": true, "argv": ["true"], "noSource": true});
    let routes = ci_routes();

    // ── preview OFF ─────────────────────────────────────────────────────
    for (p, m) in &routes {
        let body = (*m == "POST").then(|| json!({}));
        let r = call(d.port, m, p, OWNER, "", body.clone()).await;
        assert_dark(&r, &format!("owner {m} {p}"));
        // An agent: on its verbs it reaches the gate too; elsewhere the
        // route floor refuses it first (never the handler).
        let r = call(d.port, m, p, &agent, "", body).await;
        if r.status != 401 && r.status != 403 {
            assert_dark(&r, &format!("agent {m} {p}"));
        }
    }
    // The federation leg: dark before any envelope is read.
    for kind in ["compute-seat-request/1", "compute-seat-offer/1", "compute-node-roster/1"] {
        let r = call(d.port, "POST", "/cli/federation/compute-seat", "", "", Some(json!({"kind": kind}))).await;
        assert_dark(&r, &format!("federation {kind}"));
    }
    // Receipt v2 on a run; a v1 run is 0.46.9's.
    let r = call(d.port, "POST", "/cli/compute/run", &agent, "", Some(run_v2.clone())).await;
    assert_dark(&r, "receiptV 2");
    let r = call(d.port, "POST", "/cli/compute/run", &agent, "", Some(run_v1.clone())).await;
    assert_not_dark(&r, "a v1 run");
    // A lane grant in the vault.
    let r = call(d.port, "POST", "/cli/vault/grant", OWNER, "", Some(lane_grant(&secret_id))).await;
    assert_dark(&r, "lane grant");
    // Nothing written.
    assert_eq!(ci_rows(), Vec::<(String, i64)>::new(), "no CI table got a row while the preview is off");

    // ── the owner turns it ON (no restart) ──────────────────────────────
    let r = call(d.port, "POST", "/cli/settings/update", OWNER, "", Some(json!({"ciPreview": true}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["ciPreview"], json!(true), "{}", r.body);
    assert!(k2_core::compute::ci_enabled(), "synced into the process");
    // An agent can't flip it (owner-only key).
    let r = call(d.port, "POST", "/cli/settings/update", &agent, "", Some(json!({"ciPreview": false}))).await;
    assert!(r.status == 401 || r.status == 403, "an agent never flips ciPreview: {} {}", r.status, r.body);
    assert!(k2_core::compute::ci_enabled());

    for (p, m) in &routes {
        let (query, body) = match (*p, *m) {
            ("/cli/ci/status", _) => ("workspace=gate", None),
            (_, "POST") => ("", Some(json!({}))),
            _ => ("", None),
        };
        let r = call(d.port, m, p, OWNER, query, body).await;
        assert_not_dark(&r, &format!("owner {m} {p}"));
        if *m == "GET" {
            // The views answer for the owner (signers: unix only).
            if *p != k2_daemon::compute_trust::SIGNERS || cfg!(unix) {
                assert_eq!(r.status, 200, "owner GET {p}: {}", r.body);
            }
        }
    }
    let r = call(d.port, "POST", "/cli/federation/compute-seat", "", "", Some(json!({"kind": "compute-seat-request/1"}))).await;
    assert_not_dark(&r, "federation, on (the envelope is checked now)");
    let r = call(d.port, "POST", "/cli/compute/run", &agent, "", Some(run_v2.clone())).await;
    assert_not_dark(&r, "receiptV 2, on");
    let r = call(d.port, "POST", "/cli/vault/grant", OWNER, "", Some(lane_grant(&secret_id))).await;
    assert_not_dark(&r, "lane grant, on (no seat here: refused by the vault's own rules)");

    // ── and OFF again: dark at once ─────────────────────────────────────
    let r = call(d.port, "POST", "/cli/settings/update", OWNER, "", Some(json!({"ciPreview": false}))).await;
    assert_eq!((r.status, j(&r)["ciPreview"].clone()), (200, json!(false)), "{}", r.body);
    for (p, m) in &routes {
        let r = call(d.port, m, p, OWNER, "", (*m == "POST").then(|| json!({}))).await;
        assert_dark(&r, &format!("owner {m} {p}, off again"));
    }
    let r = call(d.port, "POST", "/cli/federation/compute-seat", "", "", Some(json!({}))).await;
    assert_dark(&r, "federation, off again");
}

/// `K2_CI=1` without compute is still dark: the compute router's own gate
/// (404 `compute_disabled`) answers first, and the federation leg too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ci_preview_without_compute_stays_dark() {
    let _env = setup();
    std::env::remove_var("K2_COMPUTE");
    std::env::set_var("K2_CI", "1");
    let d = test_harness::start(OWNER).await;
    assert!(!k2_core::compute::ci_enabled());
    for (p, m) in ci_routes() {
        let r = call(d.port, m, p, OWNER, "", (m == "POST").then(|| json!({}))).await;
        assert_eq!((r.status, err_code(&r).as_deref()), (404, Some("compute_disabled")), "{m} {p}: {}", r.body);
    }
    let r = call(d.port, "POST", "/cli/federation/compute-seat", "", "", Some(json!({}))).await;
    assert_eq!(r.status, 404, "{}", r.body);
    std::env::remove_var("K2_CI");
}
