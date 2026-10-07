//! Security patch 0.44.4 — daemon route hardening, end to end.
//!
//! Real dispatcher (`test_harness::start`, in-process, ephemeral port,
//! temp `$HOME`), raw HTTP over loopback. Covers:
//!
//! 1. No state change over GET: every route in
//!    `route_policy::FORMERLY_GET_WRITES` answers GET with 405 for the
//!    OWNER token, and the write did not happen (DB columns / folders
//!    checked). The POST form still works (query params or JSON body).
//! 2. Cookie-only browser credentials (`k2_session`): every request —
//!    GET, POST, WebSocket upgrade — must come from the server's own
//!    origin. A foreign Origin is refused even with `X-K2-Client: web`
//!    (the hosted edge Worker injects that header). Same-origin, the
//!    hosted web client (`https://<sub>.app.k2.dev` for Host
//!    `<sub>.k2.dev`) and the Tauri origin pass. Bearer / `?token=`
//!    callers are unaffected.
//! 3. Agent permission switches need Admin: a Member login can't flip
//!    them through `workspace/set` or the toggle routes; an Admin can.
//! 4. An agent passport can't self-grant database access.
//! 5. The activity routes (prd-daemon-activity-and-thread-working-v1)
//!    sit behind the same gates: a foreign-origin browser cookie is
//!    refused before any handler; token callers ignore Origin.
//!
//! Every test here fails on the pre-0.44.4 tree.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::routes::route_policy::FORMERLY_GET_WRITES;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-sec-0444";
const EVIL: &str = "https://evil.k2.dev";

struct Resp {
    status: u16,
    body: String,
}

/// One raw request. `host` is the Host header; `extra` are extra header
/// lines. Always `Connection: close` and read to EOF.
fn req(
    port: u16,
    method: &str,
    path_and_query: &str,
    host: &str,
    extra: &[&str],
    body: Option<&str>,
) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    let mut headers = String::new();
    for h in extra {
        headers.push_str(h);
        headers.push_str("\r\n");
    }
    let raw_req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None if method == "POST" => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{headers}Content-Length: 0\r\n\r\n"
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{headers}\r\n"
        ),
    };
    stream.write_all(raw_req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                panic!(
                    "{method} {path_and_query}: no EOF within 15s; got {:?}",
                    String::from_utf8_lossy(&raw)
                )
            }
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
    let text = String::from_utf8_lossy(&raw).to_string();
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

/// A WebSocket upgrade handshake; returns only the status line code (101
/// on accept). The socket is dropped right after the head arrives.
fn ws_upgrade(port: u16, path_and_query: &str, host: &str, extra: &[&str]) -> (u16, String) {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    let mut headers = String::new();
    for h in extra {
        headers.push_str(h);
        headers.push_str("\r\n");
    }
    let raw_req = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n"
    );
    stream.write_all(raw_req.as_bytes()).expect("write upgrade");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("upgrade read {path_and_query}: {e:?} after {:?}", String::from_utf8_lossy(&raw)),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in upgrade response: {text:?}"));
    // For a refusal read the (small) JSON body too.
    if status != 101 {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
            }
            if String::from_utf8_lossy(&raw).contains('}') {
                break;
            }
        }
    }
    (status, String::from_utf8_lossy(&raw).to_string())
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .expect("clock after epoch");
    let tmp = std::env::temp_dir().join(format!("k2-sec-0444-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    f(&tmp);
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn provision(port: u16, username: &str, role: &str) -> String {
    let r = req(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        "127.0.0.1",
        &[],
        Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)),
    );
    assert_eq!(r.status, 200, "users/add({username}); {}", r.body);
    if role != "member" {
        let r = req(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            "127.0.0.1",
            &[],
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role; {}", r.body);
    }
    let r = req(
        port,
        "POST",
        "/cli/auth/login",
        "127.0.0.1",
        &[],
        Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)),
    );
    assert_eq!(r.status, 200, "login {username}; {}", r.body);
    json(&r.body)["token"].as_str().expect("login token").to_string()
}

fn seed_ws(handle: &str) -> (String, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/sec-0444-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    (id, path)
}

fn col_i64(id: &str, col: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(&format!("SELECT COALESCE({col}, 0) FROM projects WHERE id = ?1"), params![id], |r| {
        r.get(0)
    })
    .unwrap_or_else(|e| panic!("read {col}: {e}"))
}

fn db_access(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COALESCE(db_agent_access, 'off') FROM projects WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .expect("read db_agent_access")
}

fn mint_passport(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: workspace_uuid.to_string(),
            agent_address: "sec-0444-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn assert_origin_refused(r: &Resp, label: &str) {
    assert_eq!(r.status, 403, "{label}; {}", r.body);
    assert_eq!(json(&r.body)["error"], "origin_refused", "{label}; {}", r.body);
}

// ── 1. no state change over GET ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_to_every_formerly_get_write_is_405_and_changes_nothing() {
    let _g = lock();
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let handle = format!("g{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (id, path) = seed_ws(&handle);
        let p = enc(&path);

        assert!(FORMERLY_GET_WRITES.len() >= 60, "list shrank");
        for route in FORMERLY_GET_WRITES {
            // The owner token — the strongest caller — still gets 405.
            let r = req(
                d.port,
                "GET",
                &format!("{route}?token={OWNER_TOKEN}&project={p}&enable=1&message=x&id=x&name=x"),
                "127.0.0.1",
                &[],
                None,
            );
            assert_eq!(r.status, 405, "GET {route} must be 405; body={}", r.body);
            assert_eq!(json(&r.body)["error"], "POST required", "GET {route}: {}", r.body);
        }

        // Nothing changed: the toggle GETs (enable=1 above) left the columns 0.
        assert_eq!(col_i64(&id, "dns_manage_enabled"), 0, "GET dns-manage wrote");
        assert_eq!(col_i64(&id, "agents_can_create_connections"), 0, "GET agents-create-connections wrote");
        assert_eq!(col_i64(&id, "allow_remote_instruct"), 0, "GET remote-instruct wrote");
        assert_eq!(col_i64(&id, "worktree_mode"), 0, "GET worktree wrote");
        // GET workspace/create did not create the folder.
        let new_ws = home.join("made-by-get");
        let r = req(
            d.port,
            "GET",
            &format!("/cli/workspace/create?token={OWNER_TOKEN}&path={}", enc(&new_ws.to_string_lossy())),
            "127.0.0.1",
            &[],
            None,
        );
        assert_eq!(r.status, 405, "{}", r.body);
        assert!(!new_ws.exists(), "GET workspace/create created {}", new_ws.display());

        // Readable routes refuse only their write form.
        let r = req(
            d.port,
            "GET",
            &format!("/cli/connections?token={OWNER_TOKEN}&project={p}&action=add&target=someone"),
            "127.0.0.1",
            &[],
            None,
        );
        assert_eq!(r.status, 405, "GET connections action=add: {}", r.body);
        let r = req(
            d.port,
            "GET",
            &format!("/cli/connections?token={OWNER_TOKEN}&project={p}&action=list"),
            "127.0.0.1",
            &[],
            None,
        );
        assert_eq!(r.status, 200, "GET connections list stays a read: {}", r.body);
        let r = req(
            d.port,
            "GET",
            &format!("/cli/mode?token={OWNER_TOKEN}&project={p}&set=manager"),
            "127.0.0.1",
            &[],
            None,
        );
        assert_eq!(r.status, 405, "GET mode set=: {}", r.body);

        // The POST form works: query params …
        let r = req(
            d.port,
            "POST",
            &format!("/cli/dns-manage?token={OWNER_TOKEN}&project={p}&enable=1"),
            "127.0.0.1",
            &[],
            None,
        );
        assert_eq!(r.status, 200, "POST dns-manage: {}", r.body);
        assert_eq!(col_i64(&id, "dns_manage_enabled"), 1, "POST dns-manage did not write");
        // … or a JSON body (query wins only on a clash).
        let r = req(
            d.port,
            "POST",
            &format!("/cli/dns-manage?token={OWNER_TOKEN}"),
            "127.0.0.1",
            &[],
            Some(&format!(r#"{{"project":"{path}","enable":"0"}}"#)),
        );
        assert_eq!(r.status, 200, "POST dns-manage JSON body: {}", r.body);
        assert_eq!(col_i64(&id, "dns_manage_enabled"), 0, "JSON-body POST did not write");
        // terminal/write reaches its handler as POST (unknown session → 400,
        // never 404/405).
        let r = req(
            d.port,
            "POST",
            &format!("/cli/terminal/write?token={OWNER_TOKEN}&id=00000000-0000-4000-8000-000000000000&message=hi"),
            "127.0.0.1",
            &[],
            None,
        );
        assert!(r.status != 404 && r.status != 405, "POST terminal/write must reach the handler: {} {}", r.status, r.body);
    });
}

// ── 2. cookie-only browser credentials need the server's own origin ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_only_requests_must_come_from_the_servers_own_origin() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let member = provision(d.port, &format!("cm{}", &uuid::Uuid::new_v4().to_string()[..6]), "member");
        let cookie = format!("Cookie: k2_session={member}");
        let handle = format!("c{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (id, path) = seed_ws(&handle);
        let pin = |v: &str| format!(r#"{{"project":"{path}","fields":{{"pinned":"{v}"}}}}"#);
        let host = "rosson.k2.dev";
        let xfp = "X-Forwarded-Proto: https";

        // a) The pre-0.44.4 header-only gate: still refused without the header.
        let r = req(d.port, "POST", "/cli/workspace/set", host, &[&cookie, xfp], Some(&pin("1")));
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(json(&r.body)["error"], "csrf_required", "{}", r.body);

        // b) Foreign Origin + X-K2-Client: web (what the edge Worker forwards)
        //    → refused, and nothing written.
        let evil = format!("Origin: {EVIL}");
        let r = req(
            d.port,
            "POST",
            "/cli/workspace/set",
            host,
            &[&cookie, xfp, "X-K2-Client: web", &evil],
            Some(&pin("1")),
        );
        assert_origin_refused(&r, "foreign-origin POST with X-K2-Client");
        assert_eq!(col_i64(&id, "pinned"), 0, "foreign-origin POST wrote");
        // Another customer's hosted web client is foreign too.
        let r = req(
            d.port,
            "POST",
            "/cli/workspace/set",
            host,
            &[&cookie, xfp, "X-K2-Client: web", "Origin: https://mallory.app.k2.dev"],
            Some(&pin("1")),
        );
        assert_origin_refused(&r, "other customer's app origin");
        // No Origin, browser says same-site / cross-site → refused.
        for sfs in ["same-site", "cross-site"] {
            let h = format!("Sec-Fetch-Site: {sfs}");
            let r = req(d.port, "POST", "/cli/workspace/set", host, &[&cookie, xfp, "X-K2-Client: web", &h], Some(&pin("1")));
            assert_origin_refused(&r, sfs);
        }
        assert_eq!(col_i64(&id, "pinned"), 0, "refused POSTs wrote");

        // c) The hosted web client's own origin (through the Worker) passes.
        let r = req(
            d.port,
            "POST",
            "/cli/workspace/set",
            host,
            &[&cookie, xfp, "X-K2-Client: web", "Origin: https://rosson.app.k2.dev"],
            Some(&pin("1")),
        );
        assert_eq!(r.status, 200, "own app origin POST: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 1, "own-origin POST did not write");
        // Same origin (loopback dev) and the Tauri webview pass.
        let lo_host = format!("127.0.0.1:{}", d.port);
        let lo_origin = format!("Origin: http://127.0.0.1:{}", d.port);
        let r = req(d.port, "POST", "/cli/workspace/set", &lo_host, &[&cookie, "X-K2-Client: web", &lo_origin], Some(&pin("0")));
        assert_eq!(r.status, 200, "same-origin POST: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 0);
        let r = req(d.port, "POST", "/cli/workspace/set", &lo_host, &[&cookie, "X-K2-Client: web", "Origin: tauri://localhost"], Some(&pin("1")));
        assert_eq!(r.status, 200, "Tauri-origin POST: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 1);

        // d) GET: foreign Origin refused, same-origin fetch (no Origin,
        //    Sec-Fetch-Site: same-origin) and a non-browser pass.
        let r = req(d.port, "GET", "/cli/projects/list", host, &[&cookie, &evil], None);
        assert_origin_refused(&r, "foreign-origin cookie GET");
        let r = req(d.port, "GET", "/cli/projects/list", host, &[&cookie, "Sec-Fetch-Site: cross-site"], None);
        assert_origin_refused(&r, "cross-site cookie GET");
        let r = req(d.port, "GET", "/cli/projects/list", host, &[&cookie, "Sec-Fetch-Site: same-origin"], None);
        assert_eq!(r.status, 200, "same-origin cookie GET: {}", r.body);
        let r = req(d.port, "GET", "/cli/projects/list", host, &[&cookie], None);
        assert_eq!(r.status, 200, "non-browser cookie GET: {}", r.body);
        // A side-effecting GET with a cookie + foreign Origin never runs.
        let r = req(
            d.port,
            "GET",
            "/cli/terminal/write?id=00000000-0000-4000-8000-000000000000&message=x",
            host,
            &[&cookie, &evil],
            None,
        );
        assert!(r.status == 403 || r.status == 405, "cookie GET terminal/write: {} {}", r.status, r.body);

        // e) Bearer / ?token= callers are unaffected by Origin.
        let bearer = format!("Authorization: Bearer {member}");
        let r = req(d.port, "POST", "/cli/workspace/set", host, &[&bearer, &evil], Some(&pin("0")));
        assert_eq!(r.status, 200, "Bearer + foreign Origin: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 0);
        let r = req(
            d.port,
            "POST",
            &format!("/cli/workspace/set?token={member}"),
            host,
            &[&evil, &cookie],
            Some(&pin("1")),
        );
        assert_eq!(r.status, 200, "?token= + foreign Origin: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 1);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_websocket_upgrades_must_come_from_the_servers_own_origin() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let member = provision(d.port, &format!("ws{}", &uuid::Uuid::new_v4().to_string()[..6]), "member");
        let cookie = format!("Cookie: k2_session={member}");
        let evil = format!("Origin: {EVIL}");
        let host = "rosson.k2.dev";

        for path in ["/cli/sessions/events?path=", "/events", "/cli/awareness/subscribe", "/cli/ops/stream"] {
            let (status, raw) = ws_upgrade(d.port, path, host, &[&cookie, &evil]);
            assert_eq!(status, 403, "foreign-origin cookie WS {path}: {raw}");
            assert!(raw.contains("origin_refused"), "{path}: {raw}");
            assert!(!raw.contains("101 Switching"), "{path} upgraded: {raw}");
        }
        // The hosted web client's origin and the Tauri origin upgrade.
        let (status, raw) = ws_upgrade(
            d.port,
            "/cli/sessions/events?path=",
            host,
            &[&cookie, "Origin: https://rosson.app.k2.dev", "X-Forwarded-Proto: https"],
        );
        assert_eq!(status, 101, "own app origin WS: {raw}");
        let lo_host = format!("127.0.0.1:{}", d.port);
        let (status, raw) = ws_upgrade(d.port, "/cli/sessions/events?path=", &lo_host, &[&cookie, "Origin: tauri://localhost"]);
        assert_eq!(status, 101, "Tauri origin WS: {raw}");
        let lo_origin = format!("Origin: http://127.0.0.1:{}", d.port);
        let (status, raw) = ws_upgrade(d.port, "/cli/sessions/events?path=", &lo_host, &[&cookie, &lo_origin]);
        assert_eq!(status, 101, "same-origin WS: {raw}");
        // A token-authenticated socket ignores Origin (CLI, Companion, desktop).
        let (status, raw) = ws_upgrade(d.port, &format!("/cli/sessions/events?path=&token={member}"), host, &[&evil]);
        assert_eq!(status, 101, "token WS with foreign Origin: {raw}");
        let (status, raw) = ws_upgrade(d.port, &format!("/cli/sessions/events?path=&token={OWNER_TOKEN}"), host, &[&evil]);
        assert_eq!(status, 101, "owner-token WS with foreign Origin: {raw}");
    });
}

// ── 3. agent switches need Admin ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_cannot_flip_agent_switches_admin_can() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let member = provision(d.port, &format!("sm{}", &uuid::Uuid::new_v4().to_string()[..6]), "member");
        let admin = provision(d.port, &format!("sa{}", &uuid::Uuid::new_v4().to_string()[..6]), "admin");
        let handle = format!("s{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (id, path) = seed_ws(&handle);
        let p = enc(&path);

        let switches: &[(&str, &str, &str)] = &[
            ("dns_manage_enabled", "1", "dns_manage_enabled"),
            ("agents_can_create_connections", "1", "agents_can_create_connections"),
            ("allow_remote_instruct", "1", "allow_remote_instruct"),
        ];
        for (field, value, col) in switches {
            let body = format!(r#"{{"project":"{path}","fields":{{"{field}":"{value}"}}}}"#);
            let r = req(d.port, "POST", &format!("/cli/workspace/set?token={member}"), "127.0.0.1", &[], Some(&body));
            assert_eq!(r.status, 403, "Member workspace/set {field}: {}", r.body);
            let v = json(&r.body);
            assert_eq!(v["error"], "role_required", "{field}: {}", r.body);
            assert_eq!(v["required"], "admin", "{field}: {}", r.body);
            assert_eq!(v["field"], *field, "{}", r.body);
            assert_eq!(col_i64(&id, col), 0, "Member write landed for {field}");
            let r = req(d.port, "POST", &format!("/cli/workspace/set?token={admin}"), "127.0.0.1", &[], Some(&body));
            assert_eq!(r.status, 200, "Admin workspace/set {field}: {}", r.body);
            assert_eq!(col_i64(&id, col), 1, "Admin write missing for {field}");
        }
        // db_agent_access through workspace/set.
        let body = format!(r#"{{"project":"{path}","fields":{{"db_agent_access":"write"}}}}"#);
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={member}"), "127.0.0.1", &[], Some(&body));
        assert_eq!(r.status, 403, "Member db_agent_access: {}", r.body);
        assert_eq!(db_access(&id), "off");
        // A mixed batch is refused whole (nothing applied).
        let mixed = format!(r#"{{"project":"{path}","fields":{{"pinned":"1","mail_agent_send":"on"}}}}"#);
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={member}"), "127.0.0.1", &[], Some(&mixed));
        assert_eq!(r.status, 403, "Member mixed batch: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 0, "mixed batch half-applied");
        // Ordinary fields stay Member-writable.
        let ok = format!(r#"{{"project":"{path}","fields":{{"pinned":"1"}}}}"#);
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={member}"), "127.0.0.1", &[], Some(&ok));
        assert_eq!(r.status, 200, "Member pinned: {}", r.body);
        assert_eq!(col_i64(&id, "pinned"), 1);

        // The dedicated toggle routes: Member → role_required, Admin → 200.
        for (route, col) in [
            ("/cli/dns-manage", "dns_manage_enabled"),
            ("/cli/agents-create-connections", "agents_can_create_connections"),
            ("/cli/remote-instruct", "allow_remote_instruct"),
        ] {
            let r = req(d.port, "POST", &format!("{route}?token={member}&project={p}&enable=0"), "127.0.0.1", &[], None);
            assert_eq!(r.status, 403, "Member {route}: {}", r.body);
            assert_eq!(json(&r.body)["error"], "role_required", "{route}: {}", r.body);
            assert_eq!(col_i64(&id, col), 1, "Member {route} wrote");
            let r = req(d.port, "POST", &format!("{route}?token={admin}&project={p}&enable=0"), "127.0.0.1", &[], None);
            assert_eq!(r.status, 200, "Admin {route}: {}", r.body);
            assert_eq!(col_i64(&id, col), 0, "Admin {route} did not write");
        }

        // Global agent mail-send default (settings/update) is Admin too.
        let r = req(
            d.port,
            "POST",
            &format!("/cli/settings/update?token={member}"),
            "127.0.0.1",
            &[],
            Some(r#"{"mailAgentSend":"on"}"#),
        );
        assert_eq!(r.status, 403, "Member mailAgentSend: {}", r.body);
    });
}

// ── 4. an agent passport can't self-grant db access ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_passport_cannot_self_grant_db_access() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let admin = provision(d.port, &format!("da{}", &uuid::Uuid::new_v4().to_string()[..6]), "admin");
        let handle = format!("p{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (id, path) = seed_ws(&handle);
        let passport = mint_passport(&id);
        let body = format!(r#"{{"project":"{path}","fields":{{"db_agent_access":"write"}}}}"#);

        // Query token and Bearer passport both refused; nothing written.
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={passport}"), "127.0.0.1", &[], Some(&body));
        assert_eq!(r.status, 403, "passport ?token=: {}", r.body);
        let bearer = format!("Authorization: Bearer {passport}");
        let r = req(d.port, "POST", "/cli/workspace/set", "127.0.0.1", &[&bearer], Some(&body));
        assert_eq!(r.status, 403, "passport Bearer: {}", r.body);
        assert_eq!(db_access(&id), "off", "passport self-grant landed");

        // Owner and Admin still can.
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={admin}"), "127.0.0.1", &[], Some(&body));
        assert_eq!(r.status, 200, "Admin grant: {}", r.body);
        assert_eq!(db_access(&id), "write");
        let off = format!(r#"{{"project":"{path}","fields":{{"db_agent_access":"off"}}}}"#);
        let r = req(d.port, "POST", &format!("/cli/workspace/set?token={OWNER_TOKEN}"), "127.0.0.1", &[], Some(&off));
        assert_eq!(r.status, 200, "Owner revoke: {}", r.body);
        assert_eq!(db_access(&id), "off");
    });
}

// ── 5. the activity routes behind the same gates ────────────────────

/// prd-daemon-activity-and-thread-working-v1 S1: `POST /hook/event` and
/// `POST /cli/hooks/install`. A foreign-origin cookie never reaches either
/// handler; the hook's own credential (owner token or scoped pass, never a
/// cookie) ignores Origin; a same-origin Admin login may run the installer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_hook_routes_refuse_foreign_origin_cookies() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let admin = provision(d.port, &format!("ha{}", &uuid::Uuid::new_v4().to_string()[..6]), "admin");
        let cookie = format!("Cookie: k2_session={admin}");
        let evil = format!("Origin: {EVIL}");
        let host = "rosson.k2.dev";
        let remove = r#"{"remove":true}"#;

        for path in ["/hook/event", "/cli/hooks/install"] {
            for bad in [evil.as_str(), "Origin: https://mallory.app.k2.dev", "Sec-Fetch-Site: cross-site"] {
                let r = req(d.port, "POST", path, host, &[&cookie, "X-K2-Client: web", bad], Some(remove));
                assert_origin_refused(&r, &format!("{path} {bad}"));
            }
        }

        // The hook ingest's credential is a token; Origin does not matter.
        let r = req(d.port, "POST", &format!("/hook/event?token={OWNER_TOKEN}"), host, &[&evil], Some("{}"));
        assert_eq!(r.status, 204, "owner-token hook with foreign Origin: {}", r.body);
        // A same-origin login passes the gate but is not a hook credential.
        let lo_host = format!("127.0.0.1:{}", d.port);
        let lo_origin = format!("Origin: http://127.0.0.1:{}", d.port);
        let r = req(d.port, "POST", "/hook/event", &lo_host, &[&cookie, "X-K2-Client: web", &lo_origin], Some("{}"));
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(json(&r.body)["error"], "Invalid or missing auth token", "{}", r.body);

        // The installer (remove: no `claude --version` probe; temp HOME):
        // same-origin Admin cookie and a Bearer Admin with a foreign Origin.
        let r = req(d.port, "POST", "/cli/hooks/install", &lo_host, &[&cookie, "X-K2-Client: web", &lo_origin], Some(remove));
        assert_eq!(r.status, 200, "same-origin Admin cookie: {}", r.body);
        let bearer = format!("Authorization: Bearer {admin}");
        let r = req(d.port, "POST", "/cli/hooks/install", host, &[&bearer, &evil], Some(remove));
        assert_eq!(r.status, 200, "Bearer Admin with foreign Origin: {}", r.body);
    });
}

/// prd-daemon-activity-and-thread-working-v1 S4: `GET /cli/activity/snapshot`
/// is a read, but a cookie read from a foreign origin is refused like any
/// other (0.44.4); same-origin, the hosted web client, the Tauri webview and
/// token callers get the snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_snapshot_get_refuses_foreign_origin_cookies() {
    let _g = lock();
    with_temp_home(|_| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let member = provision(d.port, &format!("sn{}", &uuid::Uuid::new_v4().to_string()[..6]), "member");
        let cookie = format!("Cookie: k2_session={member}");
        let evil = format!("Origin: {EVIL}");
        let host = "rosson.k2.dev";
        let path = "/cli/activity/snapshot";

        for bad in [
            evil.as_str(),
            "Origin: https://mallory.app.k2.dev",
            "Sec-Fetch-Site: cross-site",
            "Sec-Fetch-Site: same-site",
        ] {
            let r = req(d.port, "GET", path, host, &[&cookie, bad], None);
            assert_origin_refused(&r, &format!("snapshot {bad}"));
        }

        let is_snapshot = |r: &Resp, label: &str| {
            assert_eq!(r.status, 200, "{label}: {}", r.body);
            assert!(json(&r.body)["rows"].is_array(), "{label}: {}", r.body);
        };
        let r = req(d.port, "GET", path, host, &[&cookie, "Sec-Fetch-Site: same-origin"], None);
        is_snapshot(&r, "same-origin cookie");
        let r = req(
            d.port,
            "GET",
            path,
            host,
            &[&cookie, "Origin: https://rosson.app.k2.dev", "X-Forwarded-Proto: https"],
            None,
        );
        is_snapshot(&r, "own app origin cookie");
        let lo_host = format!("127.0.0.1:{}", d.port);
        let r = req(d.port, "GET", path, &lo_host, &[&cookie, "Origin: tauri://localhost"], None);
        is_snapshot(&r, "Tauri origin cookie");
        let bearer = format!("Authorization: Bearer {member}");
        let r = req(d.port, "GET", path, host, &[&bearer, &evil], None);
        is_snapshot(&r, "Bearer login with foreign Origin");
        let r = req(d.port, "GET", &format!("{path}?token={OWNER_TOKEN}"), host, &[&evil], None);
        is_snapshot(&r, "owner token with foreign Origin");
    });
}
