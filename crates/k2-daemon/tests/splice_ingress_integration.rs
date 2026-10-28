//! 0.46.3 — a tunnel splice that lands on the daemon's OWN main listener
//! is served as tunnel ingress, never as this machine (`Loopback`).
//!
//! The E2E TLS listener dials a nested label's target and records the
//! upstream socket (`splice_guard::mark`) before it forwards a byte. These
//! tests do exactly that against REAL in-process daemons (one on
//! `127.0.0.1`, one on `[::1]`) under each loopback alias, and check the
//! three protections a loopback peer would have skipped:
//!
//!   1. the edge-only signed-login rule (unsigned login → 404, audited as
//!      `tunnel`, no lockout);
//!   2. the login throttle (`any` mode: the shared tunnel bucket, not the
//!      uncapped local path);
//!   3. passports work only from the box (`remote_passport` on the
//!      identity routes).
//!
//! The same requests on an unmarked loopback socket keep today's
//! behaviour (control).

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-splice-ingress-7a1";
const LOGIN: &str = "/cli/auth/login";

/// The loopback aliases a nested target can use to reach the daemon
/// port; `true` = served by the IPv6 daemon, `false` = the IPv4 one.
/// Windows refuses to dial 0.0.0.0 at all (WSAEADDRNOTAVAIL), so there it
/// is not an alias that reaches the daemon.
#[cfg(not(windows))]
const ALIASES: [(&str, bool); 4] = [
    ("127.0.0.1", false),
    ("localhost", false),
    ("0.0.0.0", false),
    ("::1", true),
];
#[cfg(windows)]
const ALIASES: [(&str, bool); 3] = [("127.0.0.1", false), ("localhost", false), ("::1", true)];

/// The plain local sockets (control).
const LOCAL: [(&str, bool); 2] = [("127.0.0.1", false), ("::1", true)];

/// An IPv4 and an IPv6 loopback daemon.
struct Pair {
    v4: test_harness::TestDaemon,
    v6: test_harness::TestDaemon,
}

impl Pair {
    fn start() -> Self {
        let v4 = run(test_harness::start_on(OWNER_TOKEN, "127.0.0.1:0"));
        let v6 = run(test_harness::start_on(OWNER_TOKEN, "[::1]:0"));
        Pair { v4, v6 }
    }

    fn port(&self, v6: bool) -> u16 {
        if v6 {
            self.v6.port
        } else {
            self.v4.port
        }
    }
}

struct Resp {
    status: u16,
    body: String,
}

/// One request on a fresh connection to `host:port`. `spliced` marks the
/// socket the way the TLS listener marks its upstream (before any byte).
async fn request(host: &str, port: u16, spliced: bool, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect((host, port))
        .await
        .unwrap_or_else(|e| panic!("connect {host}:{port}: {e}"));
    let _mark = spliced.then(|| k2_daemon::splice_guard::mark(&s));
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(req.as_bytes()).await.expect("write request");
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), s.read_to_end(&mut raw))
        .await
        .expect("response within 20s")
        .expect("read response");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head: {text:?}"));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line: {text:?}"));
    Resp { status, body: body.to_string() }
}

fn run<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn setup() -> (std::sync::MutexGuard<'static, ()>, k2_core::test_env::TempHome) {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    (g, home)
}

fn seed_tunnel() {
    k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
        token: "tok".to_string(),
        subdomain: "rosson".to_string(),
        ..Default::default()
    })
    .expect("seed tunnel config");
}

fn logins() -> Vec<serde_json::Value> {
    k2_core::auth_audit::tail(1000)
        .expect("read audit log")
        .into_iter()
        .filter(|e| e["event"] == "login")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spliced_login_on_main_port_needs_edge_signature_under_every_alias() {
    let (_g, _home) = setup();
    seed_tunnel();
    connect_users::add_user("spliced", "password123").expect("add_user");
    connect_users::set_role("spliced", Role::Member).expect("set_role");
    let d = Pair::start();

    let good = r#"{"username":"spliced","password":"password123"}"#;
    for (host, v6) in ALIASES {
        let r = run(request(host, d.port(v6), true, "POST", LOGIN, Some(good)));
        assert_eq!(
            r.status, 404,
            "unsigned login spliced via {host} must hit the edge rule (404), not log in; body={}",
            r.body
        );
        assert_eq!(r.body, r#"{"error":"not_found"}"#, "via {host}");
    }
    let blocked = logins();
    assert_eq!(blocked.len(), ALIASES.len(), "every spliced attempt audited: {blocked:?}");
    for e in &blocked {
        assert_eq!(e["outcome"], "blocked_ingress", "{e}");
        assert_eq!(e["ingress"], "tunnel", "{e}");
    }
    assert_eq!(connect_users::lockout_failed_count("spliced"), 0, "argon2 never ran");

    // Control: the same login on a plain local socket still works.
    for (host, v6) in LOCAL {
        let r = run(request(host, d.port(v6), false, "POST", LOGIN, Some(good)));
        assert_eq!(r.status, 200, "local login via {host} unchanged; body={}", r.body);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spliced_login_is_throttled_as_tunnel_not_uncapped_local() {
    let (_g, _home) = setup();
    seed_tunnel();
    k2_core::app_settings::update(serde_json::json!({ "connectLoginIngress": "any" }))
        .expect("set connectLoginIngress");
    let d = Pair::start();

    // Local failed logins are uncapped: nothing is tracked.
    for (host, v6) in LOCAL {
        let body = format!(r#"{{"username":"local-{host}","password":"WRONG"}}"#);
        let r = run(request(host, d.port(v6), false, "POST", LOGIN, Some(&body)));
        assert_eq!(r.status, 401, "local attempt via {host}; body={}", r.body);
    }
    assert_eq!(d.v4.login_limiter.tracked_keys(), (0, 0), "local logins are not throttled");
    assert_eq!(d.v6.login_limiter.tracked_keys(), (0, 0), "local IPv6 logins are not throttled");

    // Spliced ones land in the shared tunnel bucket, whatever the alias.
    for (host, v6) in ALIASES {
        let body = format!(r#"{{"username":"via-{host}","password":"WRONG"}}"#);
        let r = run(request(host, d.port(v6), true, "POST", LOGIN, Some(&body)));
        assert_eq!(r.status, 401, "any-mode spliced attempt via {host}; body={}", r.body);
    }
    for (what, dd) in [("v4", &d.v4), ("v6", &d.v6)] {
        assert_eq!(
            dd.login_limiter.tracked_keys(),
            (1, 0),
            "{what}: spliced logins count in the shared tunnel bucket"
        );
    }
    for e in logins().iter().filter(|e| e["user"].as_str().is_some_and(|u| u.starts_with("via-"))) {
        assert_eq!(e["ingress"], "tunnel", "{e}");
    }

    // The bucket caps them: fill it, the next spliced login is 429.
    let now = k2_core::edge_attest::now_unix();
    while d.v6.login_limiter.check_and_record_with_limit(
        k2_daemon::login_throttle::TUNNEL_SHARED_KEY,
        k2_daemon::login_throttle::TUNNEL_SHARED_LIMIT,
        now,
    ) == k2_daemon::login_throttle::Verdict::Allow
    {}
    let r = run(request("::1", d.v6.port, true, "POST", LOGIN, Some(r#"{"username":"last","password":"WRONG"}"#)));
    assert_eq!(r.status, 429, "spliced login over the tunnel bucket; body={}", r.body);
    // A local login is still not throttled.
    let r = run(request("::1", d.v6.port, false, "POST", LOGIN, Some(r#"{"username":"local9","password":"WRONG"}"#)));
    assert_eq!(r.status, 401, "local login is never 429; body={}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spliced_passport_is_refused_on_identity_routes() {
    let (_g, home) = setup();
    let d = Pair::start();

    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("splice{}", &id[..8]);
    let dir = home.path().join("ws").join(&handle);
    std::fs::create_dir_all(&dir).expect("room dir");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle, agent_enabled) VALUES (?1, ?2, ?3, ?2, 1)",
            rusqlite::params![id, handle, dir.to_string_lossy()],
        )
        .expect("project");
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: id.clone(), agent_address: "splice-agent".to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let path = format!("/cli/agent/identity?token={passport}");

    for (host, v6) in LOCAL {
        let r = run(request(host, d.port(v6), false, "GET", &path, None));
        assert_eq!(r.status, 200, "the agent's own session on the box reads via {host}; body={}", r.body);
    }

    for (host, v6) in ALIASES {
        let r = run(request(host, d.port(v6), true, "GET", &path, None));
        assert_eq!(r.status, 403, "passport spliced via {host}; body={}", r.body);
        assert!(r.body.contains("remote_passport"), "via {host}: {}", r.body);
        let r = run(request(host, d.port(v6), true, "POST", &path, Some(r##"{"color":"#123456"}"##)));
        assert_eq!(r.status, 403, "passport write spliced via {host}; body={}", r.body);
        assert!(r.body.contains("remote_passport"), "via {host}: {}", r.body);
    }
}
