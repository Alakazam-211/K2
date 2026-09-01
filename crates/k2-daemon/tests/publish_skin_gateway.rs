//! Official `k2 publish run --skin` gateway (prd-publish-skin-gateway-v1 §9).
//!
//! Headless. No Caddy. Teaching string only on FULL `/cli/sessions/grid`
//! and `/cli/sessions/bytes` at the daemon. Gateway must not proxy those.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream as StdTcpStream};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::db::schema::WorkspaceSession;
use k2_core::published_services::{
    self as ps, CMD_SKIN_SENTINEL, DESIRED_RUNNING, KIND_SKIN,
};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-publish-skin-gw";
const TERMINAL_403: &str = r#"{"error":"skin tokens cannot use the terminal"}"#;

struct Resp {
    status: u16,
    body: String,
    headers: String,
}

fn http_ex(
    port: u16,
    method: &str,
    path_and_query: &str,
    body: Option<&str>,
    extra_headers: &str,
) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");
    let extra = if extra_headers.is_empty() {
        String::new()
    } else if extra_headers.ends_with("\r\n") {
        extra_headers.to_string()
    } else {
        format!("{extra_headers}\r\n")
    };
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
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
                break
            }
            Err(_) => break,
        }
        if let Some(clen) = content_len(&raw) {
            if body_len(&raw) >= clen {
                break;
            }
        }
    }
    parse_resp(&raw)
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    http_ex(port, method, path, body, "")
}

fn content_len(raw: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(raw);
    let (h, _) = text.split_once("\r\n\r\n")?;
    h.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse().ok())
    })
}

fn body_len(raw: &[u8]) -> usize {
    let text = String::from_utf8_lossy(raw);
    text.split_once("\r\n\r\n").map(|(_, b)| b.len()).unwrap_or(0)
}

fn parse_resp(raw: &[u8]) -> Resp {
    let text = String::from_utf8_lossy(raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = text.split("\r\n\r\n").next().unwrap_or("").to_string();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Resp {
        status,
        body,
        headers,
    }
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    for line in headers.lines() {
        let Some(colon) = line.find(':') else { continue };
        if line[..colon].eq_ignore_ascii_case(name) {
            return Some(line[colon + 1..].trim().to_string());
        }
    }
    None
}

fn cookie_k2_skin_ui(set_cookie: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("k2_skin_ui=") {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("json ({e}): {body:?}"))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-psg-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_PUBLISH_PROBE_MS", "8000");
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn port_up(port: u16) -> bool {
    StdTcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok()
}

fn wait_port(port: u16, ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if port_up(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    port_up(port)
}

fn seed_workspace(handle: &str) -> (String, String, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!(
        "/tmp/k2-psg-ws-{}-{}",
        std::process::id(),
        &id[..8]
    );
    std::fs::create_dir_all(&path).expect("ws dir");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(
        &conn,
        &format!("ws-{conv}"),
        &id,
        None,
        Some(&conv),
        "claude",
        "system",
        "running",
    )
    .expect("pin");
    (id, conv, path)
}

fn add_user(port: u16, username: &str) {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}"}}"#)),
    );
    assert_eq!(r.status, 200, "user add; {}", r.body);
}

fn set_password(port: u16, username: &str, password: &str) {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users/password?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "password; {}", r.body);
}

fn set_rooms(port: u16, username: &str, handle: &str) {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}","rooms":["{handle}"]}}"#)),
    );
    assert_eq!(r.status, 200, "rooms; {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_xor_reserved_skin_root() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgxor{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);

        let neither = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(neither.status, 400, "neither cmd nor skin; {}", neither.body);
        assert!(
            neither.body.contains("Missing cmd"),
            "neither must keep Missing cmd: {}",
            neither.body
        );

        let xor = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","cmd":"echo hi","skin":true,"port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(xor.status, 400, "cmd+skin; {}", xor.body);
        assert!(
            xor.body.contains("--cmd and --skin are mutually exclusive"),
            "{}",
            xor.body
        );

        let cwd = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","skin":true,"cwd":"{path}","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(cwd.status, 400, "cwd+skin; {}", cwd.body);
        assert!(
            cwd.body.contains("--cwd and --skin are mutually exclusive"),
            "{}",
            cwd.body
        );

        let reserved = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"skin","skin":true,"port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(reserved.status, 400, "{}", reserved.body);
        assert!(
            reserved.body.contains("reserved_label"),
            "{}",
            reserved.body
        );
        assert!(
            !reserved.body.contains("38472"),
            "must not mention 38472 as a publish target: {}",
            reserved.body
        );

        let escape = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","skin":true,"skinRoot":"../etc","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(escape.status, 400, "escape; {}", escape.body);

        let missing = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","skin":true,"skinRoot":"no-such-ui","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(missing.status, 400, "missing dir; {}", missing.body);
        assert!(
            missing.body.contains("not a directory") || missing.body.contains("skinRoot"),
            "{}",
            missing.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_login_proxy_stop_boot() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psg{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");

        let gport = free_port();
        let run = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","skin":true,"port":{gport},"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(run.status, 200, "run skin; {}", run.body);
        let v = json(&run.body);
        assert_eq!(v["kind"], "skin", "ps JSON kind==skin: {}", run.body);
        assert_eq!(v["cmd"], CMD_SKIN_SENTINEL, "{}", run.body);
        assert!(
            wait_port(gport, 8000),
            "skin gateway must listen on 127.0.0.1:{gport}"
        );

        let listed = http(
            dport,
            "GET",
            &format!("/cli/publish/list?token={OWNER_TOKEN}&project={path}"),
            None,
        );
        assert_eq!(listed.status, 200, "{}", listed.body);
        let lv = json(&listed.body);
        let svc = lv["services"]
            .as_array()
            .and_then(|a| a.first())
            .expect("services");
        assert_eq!(svc["kind"], KIND_SKIN, "{}", listed.body);

        let login_page = http(gport, "GET", "/login", None);
        assert_eq!(login_page.status, 200, "{}", login_page.body);
        assert!(
            login_page.body.to_ascii_lowercase().contains("sign in")
                || login_page.body.contains("password"),
            "bundled login: {}",
            login_page.body
        );

        let bad = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"nope"}"#),
        );
        assert_eq!(bad.status, 401, "{}", bad.body);
        assert!(!bad.body.contains("k2skn_"), "{}", bad.body);

        let ok = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(ok.status, 200, "{}", ok.body);
        assert!(
            !ok.body.contains("k2skn_"),
            "browser JSON must not contain the pass: {}",
            ok.body
        );
        let ov = json(&ok.body);
        assert_eq!(ov["ok"], true, "{}", ok.body);
        assert!(ov.get("token").is_none(), "no token key: {}", ok.body);
        assert!(ov.get("caps").is_some(), "browser JSON must include caps: {}", ok.body);
        assert!(ov.get("role").is_some(), "browser JSON must include role: {}", ok.body);
        let set_cookie = header_value(&ok.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        assert!(!set_cookie.contains("k2_skin_session"), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
        assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
        assert!(set_cookie.contains("Path=/"), "{set_cookie}");
        assert!(!set_cookie.contains("Secure"), "http omits Secure: {set_cookie}");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        assert!(!sid.starts_with("k2skn_"), "cookie is not the raw pass: {sid}");

        let https_login = http_ex(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
            "X-Forwarded-Proto: https",
        );
        let sc = header_value(&https_login.headers, "set-cookie").unwrap_or_default();
        assert!(sc.contains("Secure"), "Secure iff X-Forwarded-Proto https: {sc}");

        let cookie = format!("Cookie: k2_skin_ui={sid}");
        let thread = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={handle}"),
            None,
            &cookie,
        );
        assert_eq!(thread.status, 200, "thread with cookie; {}", thread.body);
        let tv = json(&thread.body);
        assert_eq!(tv["ok"], true, "{}", thread.body);
        assert_eq!(tv["collection"], "thread");

        let fs = http_ex(
            gport,
            "GET",
            "/cli/fs/read-dir?workspace=sales&path=.",
            None,
            &cookie,
        );
        assert_eq!(fs.status, 404, "gateway files still 404; {}", fs.body);
        assert!(
            fs.body.contains("not found"),
            "gateway files 404 body; {}",
            fs.body
        );

        let grid = http_ex(
            gport,
            "GET",
            "/cli/sessions/grid?session=nope",
            None,
            &cookie,
        );
        assert_eq!(grid.status, 403, "gateway must not proxy grid; {}", grid.body);
        assert_ne!(
            grid.body.trim(),
            TERMINAL_403,
            "gateway must not forward to the daemon teaching string: {}",
            grid.body
        );
        assert!(
            grid.body.contains("not allowed") || grid.body.contains("not found"),
            "{}",
            grid.body
        );

        let bytes = http_ex(
            gport,
            "GET",
            "/cli/sessions/bytes?session=nope",
            None,
            &cookie,
        );
        assert_eq!(bytes.status, 403, "bytes; {}", bytes.body);
        assert_ne!(bytes.body.trim(), TERMINAL_403, "{}", bytes.body);

        let mut ws = StdTcpStream::connect(("127.0.0.1", gport)).expect("ws connect");
        ws.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let up = format!(
            "GET /cli/sessions/grid HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{cookie}\r\n\r\n"
        );
        ws.write_all(up.as_bytes()).unwrap();
        ws.flush().ok();
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        if let Ok(n) = ws.read(&mut buf) {
            raw.extend_from_slice(&buf[..n]);
        }
        let head = String::from_utf8_lossy(&raw);
        assert!(
            !head.contains("101"),
            "must not WS-upgrade never-proxy grid: {head}"
        );

        // Short paths are catchall only — not the capability oracle.
        let short_grid = http_ex(gport, "GET", "/cli/grid", None, &cookie);
        assert_ne!(
            short_grid.body.trim(),
            TERMINAL_403,
            "/cli/grid must not be the teaching-string oracle: {}",
            short_grid.body
        );
        let short_pty = http_ex(gport, "GET", "/cli/pty", None, &cookie);
        assert_ne!(
            short_pty.body.trim(),
            TERMINAL_403,
            "/cli/pty must not be the teaching-string oracle: {}",
            short_pty.body
        );

        let daemon_login = http(
            dport,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(daemon_login.status, 200, "{}", daemon_login.body);
        let raw_tok = json(&daemon_login.body)["token"]
            .as_str()
            .expect("daemon token")
            .to_string();
        assert!(raw_tok.starts_with("k2skn_"), "{raw_tok}");
        let dgrid = http(
            dport,
            "GET",
            &format!("/cli/sessions/grid?token={raw_tok}&session=nope"),
            None,
        );
        assert_eq!(dgrid.status, 403, "direct skin token grid; {}", dgrid.body);
        assert_eq!(dgrid.body.trim(), TERMINAL_403);
        let dbytes = http(
            dport,
            "GET",
            &format!("/cli/sessions/bytes?token={raw_tok}&session=nope"),
            None,
        );
        assert_eq!(dbytes.status, 403, "{}", dbytes.body);
        assert_eq!(dbytes.body.trim(), TERMINAL_403);

        let stop = http(
            dport,
            "POST",
            &format!("/cli/publish/stop?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"agents","project":"{path}"}}"#)),
        );
        assert_eq!(stop.status, 200, "{}", stop.body);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!port_up(gport), "stop must close the gateway port");

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ps::set_desired(&conn, &_id, "agents", DESIRED_RUNNING).expect("desired");
        }
        k2_daemon::publish_runtime::boot_desired_running();
        assert!(
            wait_port(gport, 8000),
            "boot with desired=running must listen again"
        );

        let _ = http(
            dport,
            "POST",
            &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"agents","project":"{path}"}}"#)),
        );
        let _ = std::fs::remove_dir_all(&path);
    });
}
