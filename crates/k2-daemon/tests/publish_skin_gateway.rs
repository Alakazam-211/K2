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
use k2_core::published_services::{self as ps, CMD_SKIN_SENTINEL, DESIRED_RUNNING, KIND_SKIN};
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
    text.split_once("\r\n\r\n")
        .map(|(_, b)| b.len())
        .unwrap_or(0)
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
        let Some(colon) = line.find(':') else {
            continue;
        };
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
    let path = format!("/tmp/k2-psg-ws-{}-{}", std::process::id(), &id[..8]);
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
        Some(&format!(
            r#"{{"username":"{username}","rooms":["{handle}"]}}"#
        )),
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
        assert_eq!(
            neither.status, 400,
            "neither cmd nor skin; {}",
            neither.body
        );
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
        let cc = header_value(&login_page.headers, "cache-control").expect("Cache-Control");
        assert!(
            cc.to_ascii_lowercase().contains("no-store"),
            "GET /login Cache-Control: {cc}"
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
        assert_eq!(ov["username"], "guest", "keep username: {}", ok.body);
        assert!(
            ov.get("caps").is_some(),
            "browser JSON must include caps: {}",
            ok.body
        );
        assert!(
            ov.get("role").is_some(),
            "browser JSON must include role: {}",
            ok.body
        );
        assert!(
            ov.get("roomAccess").is_some(),
            "browser JSON must include roomAccess: {}",
            ok.body
        );
        let set_cookie = header_value(&ok.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        assert!(!set_cookie.contains("k2_skin_session"), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
        assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
        assert!(set_cookie.contains("Path=/"), "{set_cookie}");
        assert!(
            !set_cookie.contains("Secure"),
            "http omits Secure: {set_cookie}"
        );
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        assert!(
            !sid.starts_with("k2skn_"),
            "cookie is not the raw pass: {sid}"
        );

        let https_login = http_ex(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
            "X-Forwarded-Proto: https",
        );
        let sc = header_value(&https_login.headers, "set-cookie").unwrap_or_default();
        assert!(
            sc.contains("Secure"),
            "Secure iff X-Forwarded-Proto https: {sc}"
        );

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
            &format!("/cli/fs/read-dir?workspace={handle}&path=."),
            None,
            &cookie,
        );
        assert_eq!(
            fs.status, 403,
            "unassigned thread-only files:read; {}",
            fs.body
        );
        assert!(
            fs.body.contains("missing capability files:read"),
            "must be missing cap, not 404: {}",
            fs.body
        );
        assert!(
            !fs.body.contains("skin_room"),
            "thread-only files must not be skin_room: {}",
            fs.body
        );

        let tickets = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/list?project={handle}"),
            None,
            &cookie,
        );
        assert_eq!(
            tickets.status, 403,
            "unassigned thread-only tickets:read; {}",
            tickets.body
        );
        assert!(
            tickets.body.contains("missing capability tickets:read"),
            "must be missing cap, not 404: {}",
            tickets.body
        );
        assert!(
            !tickets.body.contains("skin_room"),
            "thread-only tickets must not be skin_room: {}",
            tickets.body
        );
        assert_ne!(tickets.status, 404, "unassigned tickets must not 404");

        let grid = http_ex(
            gport,
            "GET",
            "/cli/sessions/grid?session=nope",
            None,
            &cookie,
        );
        assert_eq!(
            grid.status, 403,
            "gateway must not proxy grid; {}",
            grid.body
        );
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

fn mint(port: u16, name: &str, caps: &[&str], rooms: &[&str]) -> String {
    let caps_json = serde_json::to_string(&caps).expect("caps json");
    let rooms_json = serde_json::to_string(&rooms).expect("rooms json");
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"name":"{name}","caps":{caps_json},"rooms":{rooms_json}}}"#
        )),
    );
    assert_eq!(r.status, 200, "mint; {}", r.body);
    json(&r.body)["token"]
        .as_str()
        .expect("token once")
        .to_string()
}

fn write_skin_ui(ws: &str, login_title: Option<&str>) {
    let ui = format!("{ws}/ui");
    std::fs::create_dir_all(&ui).expect("ui dir");
    std::fs::write(
        format!("{ui}/index.html"),
        "<!DOCTYPE html><title>overlay</title><body>ok</body>",
    )
    .expect("index.html");
    if let Some(title) = login_title {
        std::fs::write(
            format!("{ui}/login.html"),
            format!(
                "<!DOCTYPE html><html><head><title>{title}</title></head><body><form method=\"post\" action=\"/login\"><input name=\"username\"><input name=\"password\" type=\"password\"></form></body></html>"
            ),
        )
        .expect("login.html");
    }
}

fn publish_skin(dport: u16, path: &str, gport: u16, skin_root: Option<&str>) {
    let root_json = match skin_root {
        Some(r) => format!(r#","skinRoot":"{r}""#),
        None => String::new(),
    };
    let run = http(
        dport,
        "POST",
        &format!("/cli/publish/run?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"name":"agents","skin":true,"port":{gport},"project":"{path}","noTunnel":true{root_json}}}"#
        )),
    );
    assert_eq!(run.status, 200, "run skin; {}", run.body);
    assert!(
        wait_port(gport, 8000),
        "skin gateway must listen on 127.0.0.1:{gport}"
    );
}

fn stop_skin(dport: u16, path: &str) {
    let _ = http(
        dport,
        "POST",
        &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"agents","project":"{path}"}}"#)),
    );
}

fn seed_files_workspace(handle: &str) -> (String, String, String) {
    let (id, conv, path) = seed_workspace(handle);
    std::fs::write(format!("{path}/README.md"), b"hello files\n").expect("readme");
    (id, conv, path)
}

fn ws_upgrade(port: u16, path_and_query: &str, extra_headers: &str) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("ws connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .expect("read timeout");
    let extra = if extra_headers.is_empty() {
        String::new()
    } else if extra_headers.ends_with("\r\n") {
        extra_headers.to_string()
    } else {
        format!("{extra_headers}\r\n")
    };
    let req = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    );
    stream.write_all(req.as_bytes()).expect("ws write");
    stream.flush().expect("ws flush");
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => raw.extend_from_slice(&buf[..n]),
        Err(_) => {}
    }
    parse_resp(&raw)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_custom_login_html_or_bundled() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psglog{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");

        write_skin_ui(&path, None);
        let gport_missing = free_port();
        publish_skin(dport, &path, gport_missing, Some("ui"));
        let missing = http(gport_missing, "GET", "/login", None);
        assert_eq!(
            missing.status, 200,
            "missing login.html must not 404; {}",
            missing.body
        );
        assert!(
            missing.body.contains("Sign in — K2") || missing.body.contains("password"),
            "dir without login.html stays bundled: {}",
            missing.body
        );
        let cc = header_value(&missing.headers, "cache-control").expect("Cache-Control");
        assert!(
            cc.to_ascii_lowercase().contains("no-store"),
            "missing login.html Cache-Control: {cc}"
        );
        stop_skin(dport, &path);
        std::thread::sleep(Duration::from_millis(200));

        let title = "Custom Skin Login 2.1";
        write_skin_ui(&path, Some(title));
        let gport = free_port();
        publish_skin(dport, &path, gport, Some("ui"));
        let custom = http(gport, "GET", "/login", None);
        assert_eq!(custom.status, 200, "{}", custom.body);
        assert!(
            custom.body.contains(title),
            "custom login.html title: {}",
            custom.body
        );
        assert!(
            !custom.body.contains("Sign in — K2"),
            "must not serve bundled title: {}",
            custom.body
        );
        let cc2 = header_value(&custom.headers, "cache-control").expect("Cache-Control");
        assert!(
            cc2.to_ascii_lowercase().contains("no-store"),
            "custom login Cache-Control: {cc2}"
        );

        let ok = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(
            ok.status, 200,
            "POST /login through custom page plumbing; {}",
            ok.body
        );
        assert!(!ok.body.contains("k2skn_"), "{}", ok.body);
        let ov = json(&ok.body);
        assert!(ov.get("token").is_none(), "no token key: {}", ok.body);
        assert_eq!(ov["username"], "guest", "{}", ok.body);
        let set_cookie = header_value(&ok.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_thread_answer_not_ask_or_fs() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgans{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");

        let gport = free_port();
        publish_skin(dport, &path, gport, None);

        let ask = http(
            dport,
            "POST",
            &format!("/cli/thread/ask?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"addr":"{handle}","prompt":"Ship it?","options":"Go,Stop"}}"#
            )),
        );
        assert_eq!(ask.status, 200, "owner ask; {}", ask.body);
        let card_id = json(&ask.body)["id"].as_str().expect("card id").to_string();
        assert!(!card_id.is_empty(), "ask id: {}", ask.body);

        let login = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "{}", login.body);
        let sid =
            cookie_k2_skin_ui(&header_value(&login.headers, "set-cookie").expect("Set-Cookie"))
                .expect("opaque id");
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let answered = http_ex(
            gport,
            "POST",
            "/cli/thread/answer",
            Some(&format!(
                r#"{{"addr":"{handle}","id":"{card_id}","answer":"Go"}}"#
            )),
            &cookie,
        );
        assert_eq!(answered.status, 200, "gateway answer; {}", answered.body);
        let av = json(&answered.body);
        assert_eq!(av["ok"], true, "{}", answered.body);
        assert_eq!(av["status"], "answered", "{}", answered.body);
        assert_eq!(av["answer"], "Go", "{}", answered.body);

        let ask_gw = http_ex(
            gport,
            "POST",
            "/cli/thread/ask",
            Some(&format!(
                r#"{{"addr":"{handle}","prompt":"nope","options":"A,B"}}"#
            )),
            &cookie,
        );
        assert_eq!(
            ask_gw.status, 404,
            "ask must 404 on gateway; {}",
            ask_gw.body
        );
        assert!(
            ask_gw.body.contains("not found"),
            "ask 404 body: {}",
            ask_gw.body
        );

        let fs = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-dir?workspace={handle}&path=."),
            None,
            &cookie,
        );
        assert_eq!(
            fs.status, 403,
            "unassigned thread-only files:read; {}",
            fs.body
        );
        assert!(
            fs.body.contains("missing capability files:read"),
            "must be missing cap, not 404: {}",
            fs.body
        );
        assert!(
            !fs.body.contains("skin_room"),
            "thread-only files must not be skin_room: {}",
            fs.body
        );
        let info = http_ex(gport, "GET", "/cli/fs/info", None, &cookie);
        assert_eq!(info.status, 404, "info stays 404; {}", info.body);
        assert!(
            info.body.contains("not found"),
            "info 404 body: {}",
            info.body
        );

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_daemon_thread_answer_void_caps_and_rooms() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgdans{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("psgdoth{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        let (_oid, _oconv, opath) = seed_workspace(&other);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");

        let login = http(
            dport,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "{}", login.body);
        let sess = json(&login.body)["token"]
            .as_str()
            .expect("session token")
            .to_string();
        assert!(sess.starts_with("k2skn_"), "{sess}");

        let ask = http(
            dport,
            "POST",
            &format!("/cli/thread/ask?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"addr":"{handle}","prompt":"Ship it?","options":"Go,Stop"}}"#
            )),
        );
        assert_eq!(ask.status, 200, "{}", ask.body);
        let card_id = json(&ask.body)["id"].as_str().expect("id").to_string();

        let answered = http(
            dport,
            "POST",
            &format!("/cli/thread/answer?token={sess}"),
            Some(&format!(
                r#"{{"addr":"{handle}","id":"{card_id}","answer":"Go"}}"#
            )),
        );
        assert_eq!(answered.status, 200, "session answer; {}", answered.body);

        let ask2 = http(
            dport,
            "POST",
            &format!("/cli/thread/ask?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"addr":"{handle}","prompt":"Void?","options":"Go,Stop"}}"#
            )),
        );
        assert_eq!(ask2.status, 200, "{}", ask2.body);
        let id2 = json(&ask2.body)["id"].as_str().expect("id2").to_string();
        let voided = http(
            dport,
            "POST",
            &format!("/cli/thread/void?token={sess}"),
            Some(&format!(r#"{{"addr":"{handle}","id":"{id2}"}}"#)),
        );
        assert_eq!(voided.status, 200, "session void; {}", voided.body);

        let grid = http(
            dport,
            "GET",
            &format!("/cli/sessions/grid?token={sess}&session=nope"),
            None,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert_eq!(grid.body.trim(), TERMINAL_403);

        let read_tok = mint(dport, "guest-read", &["thread:read"], &[&handle]);
        let no_post = http(
            dport,
            "POST",
            &format!("/cli/thread/answer?token={read_tok}"),
            Some(&format!(
                r#"{{"addr":"{handle}","id":"{card_id}","answer":"Go"}}"#
            )),
        );
        assert_eq!(no_post.status, 403, "missing cap; {}", no_post.body);
        assert!(
            no_post.body.contains("missing capability thread:post"),
            "missing-cap body: {}",
            no_post.body
        );

        let other_ask = http(
            dport,
            "POST",
            &format!("/cli/thread/ask?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"addr":"{other}","prompt":"Other?","options":"A,B"}}"#
            )),
        );
        assert_eq!(other_ask.status, 200, "{}", other_ask.body);
        let other_id = json(&other_ask.body)["id"]
            .as_str()
            .expect("other id")
            .to_string();
        let room_deny = http(
            dport,
            "POST",
            &format!("/cli/thread/answer?token={sess}"),
            Some(&format!(
                r#"{{"addr":"{other}","id":"{other_id}","answer":"A"}}"#
            )),
        );
        assert_eq!(room_deny.status, 403, "skin_room; {}", room_deny.body);
        let rv = json(&room_deny.body);
        assert_eq!(rv["error"]["code"], "skin_room", "{}", room_deny.body);

        let secret_val = "s3cr3t-NEVER-IN-DAEMON-ANSWER-xyz";
        let secret = http(
            dport,
            "POST",
            &format!("/cli/thread/secret?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"addr":"{handle}","name":"API_TOKEN","prompt":"Paste"}}"#
            )),
        );
        assert_eq!(secret.status, 200, "{}", secret.body);
        let sid = json(&secret.body)["id"]
            .as_str()
            .expect("secret id")
            .to_string();
        let filled = http(
            dport,
            "POST",
            &format!("/cli/thread/answer?token={sess}"),
            Some(&format!(
                r#"{{"addr":"{handle}","id":"{sid}","secret":"{secret_val}"}}"#
            )),
        );
        assert_eq!(filled.status, 200, "secret fill; {}", filled.body);
        assert!(
            !filled.body.contains(secret_val),
            "must not echo secret: {}",
            filled.body
        );
        let fv = json(&filled.body);
        assert_eq!(fv["status"], "set", "{}", filled.body);

        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_dir_all(&opath);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_files_rooms_jail_and_ws() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let anna = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_anna_id, _anna_conv, anna_path) = seed_files_workspace(&anna);
        let (_docs_id, _docs_conv, docs_path) = seed_files_workspace(&docs);
        add_user(dport, "bob");
        set_password(dport, "bob", "s3cret-horse");

        let created = http(
            dport,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"dentist"}"#),
        );
        assert_eq!(created.status, 200, "role create; {}", created.body);
        let anna_room = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"dentist","handle":"{anna}","caps":["thread:read","thread:post"]}}"#
            )),
        );
        assert_eq!(anna_room.status, 200, "anna room; {}", anna_room.body);
        let docs_room = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"dentist","handle":"{docs}","caps":["thread:read","thread:post","files:read","files:write"]}}"#
            )),
        );
        assert_eq!(docs_room.status, 200, "docs room; {}", docs_room.body);
        let assign = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"bob","role":"dentist"}"#),
        );
        assert_eq!(assign.status, 200, "assign; {}", assign.body);

        let gport = free_port();
        publish_skin(dport, &anna_path, gport, None);

        let public = http(gport, "GET", "/", None);
        assert_eq!(public.status, 200, "static / no cookie; {}", public.body);
        assert!(!public.body.contains("k2skn_"), "{}", public.body);
        assert!(!public.body.contains("?token="), "{}", public.body);

        let no_cookie = http(
            gport,
            "GET",
            &format!("/cli/fs/read-dir?workspace={docs}&path=."),
            None,
        );
        assert_eq!(no_cookie.status, 401, "no cookie; {}", no_cookie.body);
        assert!(
            no_cookie.body.contains("not logged in"),
            "live 401 string; {}",
            no_cookie.body
        );

        let login = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "gateway login; {}", login.body);
        assert!(
            !login.body.contains("k2skn_"),
            "official origin must not put k2skn_ on the wire: {}",
            login.body
        );
        assert!(
            !login.body.contains("?token="),
            "official origin must not put ?token= on the wire: {}",
            login.body
        );
        let lv = json(&login.body);
        assert!(lv.get("token").is_none(), "no token key: {}", login.body);
        assert!(
            lv.get("roomAccess").is_some(),
            "login JSON must include roomAccess: {}",
            login.body
        );
        let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        assert!(
            !sid.starts_with("k2skn_"),
            "cookie is not the raw pass: {sid}"
        );
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let docs_dir = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-dir?workspace={docs}&path=."),
            None,
            &cookie,
        );
        assert_eq!(docs_dir.status, 200, "docs files; {}", docs_dir.body);
        assert!(
            !docs_dir.body.contains("k2skn_"),
            "no pass in files body: {}",
            docs_dir.body
        );

        let anna_dir = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-dir?workspace={anna}&path=."),
            None,
            &cookie,
        );
        assert_eq!(anna_dir.status, 403, "anna files; {}", anna_dir.body);
        assert!(
            anna_dir.body.contains("missing capability files:read"),
            "anna must be missing cap, not skin_room: {}",
            anna_dir.body
        );
        assert!(
            !anna_dir.body.contains("skin_room"),
            "anna files must not be skin_room: {}",
            anna_dir.body
        );

        let thread_anna = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={anna}"),
            None,
            &cookie,
        );
        assert_eq!(thread_anna.status, 200, "thread anna; {}", thread_anna.body);

        let write_docs = http_ex(
            gport,
            "POST",
            "/cli/fs/write-file",
            Some(&format!(
                r#"{{"workspace":"{docs}","path":"hello.md","content":"from-gateway"}}"#
            )),
            &cookie,
        );
        assert_eq!(write_docs.status, 200, "write docs; {}", write_docs.body);
        assert_eq!(
            std::fs::read_to_string(format!("{docs_path}/hello.md")).expect("hello.md"),
            "from-gateway"
        );

        let write_anna = http_ex(
            gport,
            "POST",
            "/cli/fs/write-file",
            Some(&format!(
                r#"{{"workspace":"{anna}","path":"hello.md","content":"nope"}}"#
            )),
            &cookie,
        );
        assert_eq!(write_anna.status, 403, "write anna; {}", write_anna.body);
        assert!(
            write_anna.body.contains("missing capability files:write"),
            "anna write must be missing files:write: {}",
            write_anna.body
        );
        assert!(
            !std::path::Path::new(&format!("{anna_path}/hello.md")).exists(),
            "must not write anna"
        );

        let wrong = http_ex(
            gport,
            "GET",
            "/cli/fs/read-dir?workspace=not-a-handle&path=.",
            None,
            &cookie,
        );
        assert_eq!(wrong.status, 403, "wrong handle; {}", wrong.body);
        assert!(
            wrong.body.contains("skin_room"),
            "wrong handle must be skin_room: {}",
            wrong.body
        );

        let jail_dot = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-dir?workspace={docs}&path=../"),
            None,
            &cookie,
        );
        assert!(
            jail_dot.status == 400 || jail_dot.status == 403,
            "jail ../ status; {}",
            jail_dot.body
        );
        assert_ne!(jail_dot.status, 200);

        let jail_abs = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-file?workspace={docs}&path=/etc/passwd"),
            None,
            &cookie,
        );
        assert!(
            jail_abs.status == 400 || jail_abs.status == 403,
            "jail abs; {}",
            jail_abs.body
        );
        assert!(
            !jail_abs.body.contains("root:"),
            "must never leak /etc/passwd: {}",
            jail_abs.body
        );

        let outside = std::env::temp_dir().join(format!(
            "k2-psg-outside-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, b"untouched\n").expect("outside");
        std::os::unix::fs::symlink(&outside, format!("{docs_path}/notes.md")).expect("symlink");
        let write_link = http_ex(
            gport,
            "POST",
            "/cli/fs/write-file",
            Some(&format!(
                r#"{{"workspace":"{docs}","path":"notes.md","content":"pwned"}}"#
            )),
            &cookie,
        );
        assert_eq!(
            write_link.status, 400,
            "F23 symlink write; {}",
            write_link.body
        );
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside intact"),
            "untouched\n"
        );

        let write_fresh = http_ex(
            gport,
            "POST",
            "/cli/fs/write-file",
            Some(&format!(
                r#"{{"workspace":"{docs}","path":"fresh.md","content":"new-leaf"}}"#
            )),
            &cookie,
        );
        assert_eq!(
            write_fresh.status, 200,
            "create-new-leaf; {}",
            write_fresh.body
        );
        assert_eq!(
            std::fs::read_to_string(format!("{docs_path}/fresh.md")).expect("fresh.md"),
            "new-leaf"
        );

        let info = http_ex(gport, "GET", "/cli/fs/info", None, &cookie);
        assert_eq!(info.status, 404, "info; {}", info.body);
        assert!(info.body.contains("not found"), "{}", info.body);
        let ask = http_ex(
            gport,
            "POST",
            "/cli/thread/ask",
            Some(&format!(
                r#"{{"addr":"{anna}","prompt":"nope","options":"A,B"}}"#
            )),
            &cookie,
        );
        assert_eq!(ask.status, 404, "ask; {}", ask.body);

        let grid = http_ex(
            gport,
            "GET",
            "/cli/sessions/grid?session=nope",
            None,
            &cookie,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert_ne!(
            grid.body.trim(),
            TERMINAL_403,
            "gateway grid body must not be the teaching string: {}",
            grid.body
        );
        assert!(
            grid.body.contains("not allowed"),
            "grid 403 not allowed; {}",
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

        let files_ws = ws_upgrade(gport, &format!("/cli/fs/events?workspace={docs}"), &cookie);
        assert_eq!(files_ws.status, 101, "files WS cookie; {}", files_ws.body);

        let missing_ws = ws_upgrade(gport, "/cli/fs/events", &cookie);
        assert_eq!(
            missing_ws.status, 400,
            "missing workspace; {}",
            missing_ws.body
        );
        assert!(
            missing_ws
                .body
                .contains("missing workspace query parameter"),
            "files missing workspace string; {}",
            missing_ws.body
        );
        assert!(
            !missing_ws.body.contains("conversation"),
            "must not use overlay conversation error: {}",
            missing_ws.body
        );

        let conv_only = ws_upgrade(gport, "/cli/fs/events?conversation=abc", &cookie);
        assert_eq!(
            conv_only.status, 400,
            "conversation without workspace; {}",
            conv_only.body
        );
        assert!(
            conv_only.body.contains("missing workspace query parameter"),
            "{}",
            conv_only.body
        );

        let overlay_missing = ws_upgrade(gport, "/cli/overlay/events", &cookie);
        assert_eq!(
            overlay_missing.status, 400,
            "overlay missing conversation; {}",
            overlay_missing.body
        );
        assert!(
            overlay_missing
                .body
                .contains("missing conversation query parameter"),
            "{}",
            overlay_missing.body
        );

        let ws_wrong = ws_upgrade(gport, "/cli/fs/events?workspace=not-a-handle", &cookie);
        assert_eq!(ws_wrong.status, 403, "wrong room WS; {}", ws_wrong.body);
        assert_ne!(ws_wrong.status, 101);
        assert!(
            ws_wrong.body.contains("skin_room"),
            "wrong room WS must be skin_room: {}",
            ws_wrong.body
        );

        let sessions_ev = ws_upgrade(gport, "/cli/sessions/events", &cookie);
        assert_eq!(
            sessions_ev.status, 403,
            "sessions/events; {}",
            sessions_ev.body
        );
        assert!(
            sessions_ev.body.contains("not allowed"),
            "sessions/events must stay G8: {}",
            sessions_ev.body
        );
        assert_ne!(sessions_ev.status, 101);

        stop_skin(dport, &anna_path);
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&anna_path);
        let _ = std::fs::remove_dir_all(&docs_path);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_tickets_per_room() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let anna = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("othr{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_anna_id, _anna_conv, anna_path) = seed_workspace(&anna);
        let (_docs_id, _docs_conv, docs_path) = seed_workspace(&docs);
        let (_other_id, _other_conv, other_path) = seed_workspace(&other);
        add_user(dport, "bob");
        set_password(dport, "bob", "s3cret-horse");

        let created = http(
            dport,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"dentist"}"#),
        );
        assert_eq!(created.status, 200, "role create; {}", created.body);
        let anna_room = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"dentist","handle":"{anna}","caps":["thread:read","thread:post"]}}"#
            )),
        );
        assert_eq!(anna_room.status, 200, "anna room; {}", anna_room.body);
        let docs_room = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"dentist","handle":"{docs}","caps":["thread:read","thread:post","tickets:read","tickets:post"]}}"#
            )),
        );
        assert_eq!(docs_room.status, 200, "docs room; {}", docs_room.body);
        let assign = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"bob","role":"dentist"}"#),
        );
        assert_eq!(assign.status, 200, "assign; {}", assign.body);

        let anna_ticket = http(
            dport,
            "POST",
            &format!("/cli/feedback/create?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"project":"{anna}","title":"Anna ask"}}"#)),
        );
        assert_eq!(
            anna_ticket.status, 200,
            "owner anna ticket; {}",
            anna_ticket.body
        );
        let anna_id = json(&anna_ticket.body)["id"]
            .as_str()
            .expect("anna id")
            .to_string();
        let other_ticket = http(
            dport,
            "POST",
            &format!("/cli/feedback/create?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"project":"{other}","title":"Other ask"}}"#)),
        );
        assert_eq!(
            other_ticket.status, 200,
            "owner other ticket; {}",
            other_ticket.body
        );
        let other_id = json(&other_ticket.body)["id"]
            .as_str()
            .expect("other id")
            .to_string();

        let gport = free_port();
        publish_skin(dport, &anna_path, gport, None);

        let login = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "gateway login; {}", login.body);
        assert!(!login.body.contains("k2skn_"), "{}", login.body);
        assert!(!login.body.contains("?token="), "{}", login.body);
        let lv = json(&login.body);
        assert!(lv.get("token").is_none(), "no token key: {}", login.body);
        assert!(
            lv.get("roomAccess").is_some(),
            "login JSON must include roomAccess: {}",
            login.body
        );
        let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        assert!(
            !sid.starts_with("k2skn_"),
            "cookie is not the raw pass: {sid}"
        );
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let docs_list = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/list?project={docs}"),
            None,
            &cookie,
        );
        assert_eq!(docs_list.status, 200, "docs tickets; {}", docs_list.body);
        assert!(!docs_list.body.contains("k2skn_"), "{}", docs_list.body);

        let anna_list = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/list?project={anna}"),
            None,
            &cookie,
        );
        assert_eq!(
            anna_list.status, 403,
            "anna tickets list; {}",
            anna_list.body
        );
        assert!(
            anna_list.body.contains("missing capability tickets:read"),
            "anna list must be missing cap, not skin_room: {}",
            anna_list.body
        );
        assert!(
            !anna_list.body.contains("skin_room"),
            "anna list must not be skin_room: {}",
            anna_list.body
        );

        let thread_anna = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={anna}"),
            None,
            &cookie,
        );
        assert_eq!(thread_anna.status, 200, "thread anna; {}", thread_anna.body);

        let show_anna = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/show?id={anna_id}"),
            None,
            &cookie,
        );
        assert_eq!(
            show_anna.status, 403,
            "show anna ticket; {}",
            show_anna.body
        );
        assert!(
            show_anna.body.contains("missing capability tickets:read"),
            "show anna (room on map, no tickets) is missing cap: {}",
            show_anna.body
        );

        let show_other = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/show?id={other_id}"),
            None,
            &cookie,
        );
        assert_eq!(
            show_other.status, 403,
            "show other ticket; {}",
            show_other.body
        );
        assert!(
            show_other.body.contains("skin_room"),
            "other room's id must be skin_room, not 404: {}",
            show_other.body
        );
        assert!(
            !show_other.body.contains("not_found"),
            "must not leak live 404 not_found: {}",
            show_other.body
        );

        let show_missing = http_ex(
            gport,
            "GET",
            "/cli/feedback/show?id=00000000-0000-0000-0000-000000000000",
            None,
            &cookie,
        );
        assert_eq!(
            show_missing.status, 403,
            "missing id; {}",
            show_missing.body
        );
        assert!(
            show_missing.body.contains("skin_room"),
            "unknown id must be skin_room (no 404 oracle): {}",
            show_missing.body
        );

        let abs_list = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/list?project={docs_path}"),
            None,
            &cookie,
        );
        assert_eq!(abs_list.status, 403, "abs project=; {}", abs_list.body);
        assert!(
            abs_list.body.contains("skin_room"),
            "abs path project= must be skin_room: {}",
            abs_list.body
        );

        let base = std::path::Path::new(&docs_path)
            .file_name()
            .and_then(|s| s.to_str())
            .expect("basename");
        let base_list = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/list?project={base}"),
            None,
            &cookie,
        );
        assert_eq!(
            base_list.status, 403,
            "basename project=; {}",
            base_list.body
        );
        assert!(
            base_list.body.contains("skin_room"),
            "folder-basename project= must be skin_room: {}",
            base_list.body
        );

        let create_docs = http_ex(
            gport,
            "POST",
            "/cli/feedback/create",
            Some(&format!(
                r#"{{"project":"{docs}","title":"Docs ask","body":"from-skin"}}"#
            )),
            &cookie,
        );
        assert_eq!(create_docs.status, 200, "create docs; {}", create_docs.body);
        let docs_id = json(&create_docs.body)["id"]
            .as_str()
            .expect("docs id")
            .to_string();

        let comment_docs = http_ex(
            gport,
            "POST",
            "/cli/feedback/comment",
            Some(&format!(
                r#"{{"id":"{docs_id}","body":"ship it","author":"owner"}}"#
            )),
            &cookie,
        );
        assert_eq!(
            comment_docs.status, 200,
            "comment docs; {}",
            comment_docs.body
        );
        let cv = json(&comment_docs.body);
        assert_eq!(
            cv["author"], "bob",
            "session_author is pass.username, never owner: {}",
            comment_docs.body
        );
        assert_ne!(cv["author"], "owner");

        let create_anna = http_ex(
            gport,
            "POST",
            "/cli/feedback/create",
            Some(&format!(r#"{{"project":"{anna}","title":"nope"}}"#)),
            &cookie,
        );
        assert_eq!(create_anna.status, 403, "create anna; {}", create_anna.body);
        assert!(
            create_anna.body.contains("missing capability tickets:post"),
            "anna create must be missing tickets:post: {}",
            create_anna.body
        );

        let comment_anna = http_ex(
            gport,
            "POST",
            "/cli/feedback/comment",
            Some(&format!(r#"{{"id":"{anna_id}","body":"nope"}}"#)),
            &cookie,
        );
        assert_eq!(
            comment_anna.status, 403,
            "comment anna; {}",
            comment_anna.body
        );
        assert!(
            comment_anna
                .body
                .contains("missing capability tickets:post"),
            "anna comment must be missing tickets:post: {}",
            comment_anna.body
        );

        let waiting = http_ex(gport, "GET", "/cli/feedback/waiting-count", None, &cookie);
        assert_eq!(waiting.status, 404, "waiting-count; {}", waiting.body);
        assert!(waiting.body.contains("not found"), "{}", waiting.body);

        let assign_gw = http_ex(
            gport,
            "POST",
            "/cli/feedback/assign",
            Some(&format!(r#"{{"id":"{docs_id}","usernames":["bob"]}}"#)),
            &cookie,
        );
        assert_eq!(assign_gw.status, 404, "assign; {}", assign_gw.body);
        assert!(assign_gw.body.contains("not found"), "{}", assign_gw.body);

        let foo = http_ex(gport, "GET", "/cli/feedback/foo", None, &cookie);
        assert_eq!(foo.status, 404, "foo; {}", foo.body);
        assert!(foo.body.contains("not found"), "{}", foo.body);

        let grid = http_ex(
            gport,
            "GET",
            "/cli/sessions/grid?session=nope",
            None,
            &cookie,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert!(
            grid.body.contains("not allowed"),
            "grid 403 not allowed; {}",
            grid.body
        );
        assert_ne!(grid.body.trim(), TERMINAL_403, "{}", grid.body);

        stop_skin(dport, &anna_path);
        let _ = std::fs::remove_dir_all(&anna_path);
        let _ = std::fs::remove_dir_all(&docs_path);
        let _ = std::fs::remove_dir_all(&other_path);
    });
}
