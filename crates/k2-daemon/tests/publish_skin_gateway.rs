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
/// A JSON-safe ticket brief for owner-door creates (prd-ticket-html-brief
/// H36: these keep passing when the brief policy flips to Require).
const OWNER_BRIEF: &str = "<p>Owner ask.</p><section class='k2-need'><p>Yes or no.</p></section>";
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
    http_host(port, "127.0.0.1", method, path_and_query, body, extra_headers)
}

/// [`http_ex`] with an explicit `Host` (the published app's host name).
fn http_host(
    port: u16,
    host: &str,
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
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n"),
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

/// The sid from `k2_skin_ui=` (loopback/http) or `__Host-k2_skin_ui=`
/// (Secure, S24-14).
fn cookie_k2_skin_ui(set_cookie: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(v) = part
            .strip_prefix("k2_skin_ui=")
            .or_else(|| part.strip_prefix("__Host-k2_skin_ui="))
        {
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

/// Temp `HOME` + agent shim + helper-orphan hygiene for the duration of
/// `f`. Everything below runs on the way out INCLUDING on panic (Drop
/// guard): env restore, `publish_runtime::kill_all_live_children()` (the
/// `k2-daemon --skin-gateway` helpers are `setsid` session leaders — two
/// outlived the 2026-09-12 run and were killed by hand), and dir removal.
///
/// Spawn guard (2026-09-12): a skin Thread post may wake the pinned Chat,
/// which spawns the workspace's `claude`; `K2_TEST_AGENT_SHIM_DIR` makes
/// the daemon resolve it ONLY to `<tmp>/shim/claude` (`exec cat`) instead
/// of the real `~/.local/bin/claude` that opened a browser OAuth login.
/// `K2_PUBLISH_HELPER_EXIT_WITH_PARENT=1` is inherited by every helper
/// so a SIGKILLed / timed-out test binary cannot leave one behind either.
fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            k2_daemon::publish_runtime::kill_all_live_children();
            for (name, val) in self.prev.drain(..) {
                match val {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    const NAMES: [&str; 4] = [
        "HOME",
        "K2_TEST_AGENT_SHIM_DIR",
        "K2_PUBLISH_HELPER_EXIT_WITH_PARENT",
        "K2_PUBLISH_PROBE_MS",
    ];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-psg-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("write claude shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .expect("chmod claude shim");
    }
    let _restore = Restore {
        prev: NAMES.iter().map(|n| (*n, std::env::var_os(n))).collect(),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    std::env::set_var("K2_PUBLISH_HELPER_EXIT_WITH_PARENT", "1");
    std::env::set_var("K2_PUBLISH_PROBE_MS", "8000");
    f();
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

/// `meet` / `chat` are reserved for Sew box doors: a NEW publish under
/// either name is a loud 400. A `chat` row made before the reservation
/// keeps working: it lists (flagged `reservedLabel`), stops, re-runs
/// without a reservation error, and can be removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_reserved_meet_chat_new_refused_old_row_keeps_working() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgrsv{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (id, _conv, path) = seed_workspace(&handle);

        for name in ["meet", "chat", "Meet", "CHAT"] {
            let r = http(
                dport,
                "POST",
                &format!("/cli/publish/run?token={OWNER_TOKEN}"),
                Some(&format!(
                    r#"{{"name":"{name}","cmd":"echo hi","port":9,"project":"{path}","noTunnel":true}}"#
                )),
            );
            assert_eq!(r.status, 400, "new {name}; {}", r.body);
            assert!(r.body.contains("reserved_label"), "new {name}: {}", r.body);
            assert!(r.body.contains("Pick another nested label"), "{}", r.body);
        }
        let skin_run = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"skin","cmd":"echo hi","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(skin_run.status, 400, "{}", skin_run.body);
        assert!(
            skin_run.body.contains(
                "reserved_label: 'skin' is reserved. Pick another nested label for your UI"
            ),
            "skin text unchanged: {}",
            skin_run.body
        );

        // A `chat` row from before the reservation. Its port is held by a
        // listener so a re-run stops at the bind check, never spawning.
        let held = TcpListener::bind("127.0.0.1:0").expect("bind");
        let held_port = held.local_addr().expect("addr").port();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ps::insert(
                &conn,
                &id,
                "chat",
                "echo old",
                &path,
                held_port,
                ps::EXPOSE_LOCAL,
                ps::DESIRED_STOPPED,
                ps::KIND_CMD,
                "",
            )
            .expect("old chat row");
        }

        let list = http(
            dport,
            "GET",
            &format!("/cli/publish/list?token={OWNER_TOKEN}&project={path}"),
            None,
        );
        assert_eq!(list.status, 200, "{}", list.body);
        let v = json(&list.body);
        let services = v["services"].as_array().expect("services array");
        assert_eq!(services.len(), 1, "only the old chat row: {}", list.body);
        assert_eq!(services[0]["name"], "chat");
        assert_eq!(services[0]["reservedLabel"], true, "{}", list.body);

        let stop = http(
            dport,
            "POST",
            &format!("/cli/publish/stop?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"chat","project":"{path}"}}"#)),
        );
        assert_eq!(stop.status, 200, "stop old chat; {}", stop.body);

        for (verb, body) in [
            ("start", format!(r#"{{"name":"chat","project":"{path}"}}"#)),
            (
                "run",
                format!(
                    r#"{{"name":"chat","cmd":"echo again","port":{held_port},"project":"{path}","noTunnel":true}}"#
                ),
            ),
        ] {
            let r = http(
                dport,
                "POST",
                &format!("/cli/publish/{verb}?token={OWNER_TOKEN}"),
                Some(&body),
            );
            assert!(
                !r.body.contains("reserved_label"),
                "{verb} of the old chat row must not hit the reservation: {}",
                r.body
            );
            assert!(
                r.body.contains("bind clash"),
                "{verb} reached the bind check (row still addressable): {} {}",
                r.status,
                r.body
            );
        }

        let rm = http(
            dport,
            "POST",
            &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"chat","project":"{path}"}}"#)),
        );
        assert_eq!(rm.status, 200, "rm old chat; {}", rm.body);
        let after = http(
            dport,
            "GET",
            &format!("/cli/publish/list?token={OWNER_TOKEN}&project={path}"),
            None,
        );
        assert_eq!(after.status, 200, "{}", after.body);
        let v = json(&after.body);
        assert!(
            v["services"].as_array().expect("services").is_empty(),
            "rm removed the old row: {}",
            after.body
        );

        // Gone now, so `chat` is a new claim again: refused.
        let again = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"chat","cmd":"echo hi","port":9,"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(again.status, 400, "{}", again.body);
        assert!(again.body.contains("reserved_label"), "{}", again.body);

        let get_run = http(
            dport,
            "GET",
            &format!("/cli/publish/run?token={OWNER_TOKEN}&name=chat&project={path}"),
            None,
        );
        assert_eq!(get_run.status, 405, "{}", get_run.body);
        drop(held);
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

        // S24 (Q6): a new App starts with nobody. The right password is the
        // same generic 401 until the owner grants this App.
        let closed = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(closed.status, 401, "fail closed: {}", closed.body);
        assert_eq!(closed.body, bad.body, "no difference from a bad password");
        let copied = copy_host(dport, &format!("{handle}/agents"));
        assert_eq!(copied["added"], serde_json::json!(["guest"]), "{copied}");

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
        let pid = ov["principalId"].as_str().unwrap_or("");
        assert!(
            !pid.is_empty() && uuid::Uuid::parse_str(pid).is_ok(),
            "session login JSON may include principalId UUID: {}",
            ok.body
        );
        assert!(!pid.starts_with("k2skn_"), "{}", ok.body);
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
        // S24-14: a Secure cookie carries the __Host- prefix, and a plain
        // name on an https request is ignored (planted-cookie defence).
        assert!(sc.starts_with("__Host-k2_skin_ui="), "{sc}");
        let https_sid = sc
            .strip_prefix("__Host-k2_skin_ui=")
            .and_then(|v| v.split(';').next())
            .expect("__Host- sid")
            .to_string();
        let planted = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={handle}"),
            None,
            &format!("X-Forwarded-Proto: https\r\nCookie: k2_skin_ui={https_sid}"),
        );
        assert_eq!(planted.status, 401, "unprefixed cookie on https: {}", planted.body);
        let prefixed = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={handle}"),
            None,
            &format!("X-Forwarded-Proto: https\r\nCookie: __Host-k2_skin_ui={https_sid}"),
        );
        assert_eq!(prefixed.status, 200, "__Host- cookie on https: {}", prefixed.body);

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

        let wiki = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/index?project={handle}"),
            None,
            &cookie,
        );
        assert_eq!(
            wiki.status, 403,
            "unassigned thread-only wiki:read; {}",
            wiki.body
        );
        assert!(
            wiki.body.contains("missing capability wiki:read"),
            "must be missing cap, not 404: {}",
            wiki.body
        );
        assert!(
            !wiki.body.contains("skin_room"),
            "thread-only wiki must not be skin_room: {}",
            wiki.body
        );
        assert_ne!(wiki.status, 404, "unassigned wiki must not 404");

        let store = http_ex(
            gport,
            "GET",
            &format!("/cli/store/list?workspace={handle}"),
            None,
            &cookie,
        );
        assert_eq!(
            store.status, 403,
            "unassigned thread-only store:read; {}",
            store.body
        );
        assert!(
            store.body.contains("missing capability store:read"),
            "must be missing cap, not 404: {}",
            store.body
        );
        assert!(
            !store.body.contains("skin_room"),
            "thread-only store must not be skin_room: {}",
            store.body
        );
        assert_ne!(store.status, 404, "unassigned store must not 404");

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

fn publish_skin(dport: u16, path: &str, gport: u16, skin_root: Option<&str>) -> String {
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
    // S24: an App published after 0.45.5 starts with nobody (Q6). These
    // tests set their guests up on the box login first, so copy them on —
    // exactly what an owner does with `k2 app grant copy-host`.
    let id = json(&run.body)["id"]
        .as_str()
        .expect("published service id")
        .to_string();
    copy_host(dport, &id);
    id
}

/// `POST /cli/skin/grants/copy-host` (owner): everyone with the box login
/// gets the same access on App `app` (a service id or `<workspace>/<name>`).
fn copy_host(dport: u16, app: &str) -> serde_json::Value {
    let r = http(
        dport,
        "POST",
        &format!("/cli/skin/grants/copy-host?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"app":"{app}"}}"#)),
    );
    assert_eq!(r.status, 200, "copy-host; {}", r.body);
    json(&r.body)
}

/// One text frame, or `None` when nothing arrives within `wait`.
fn try_read_ws_text(stream: &mut StdTcpStream, wait: Duration) -> Option<String> {
    stream.set_read_timeout(Some(wait)).expect("short timeout");
    let mut first = [0u8; 1];
    let got = match stream.peek(&mut first) {
        Ok(0) => panic!("ws closed while waiting for a frame"),
        Ok(_) => true,
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            false
        }
        Err(e) => panic!("ws peek: {e:?}"),
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("restore timeout");
    got.then(|| read_ws_text(stream))
}

/// Open the room socket through the helper; returns the stream after 101.
fn open_room_socket(gport: u16, workspace: &str, cookie: &str) -> StdTcpStream {
    let mut sock = StdTcpStream::connect(("127.0.0.1", gport)).expect("ws connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let req = format!(
        "GET /cli/activity/events?workspace={workspace} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{cookie}\r\n\r\n"
    );
    sock.write_all(req.as_bytes()).expect("ws write");
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        sock.read_exact(&mut byte).expect("upgrade head");
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_string();
    assert!(head.starts_with("HTTP/1.1 101"), "room socket {workspace}: {head}");
    sock
}

/// prd-app-tickets-websocket-v1 T6–T9 through the helper: tickets:read
/// opens the room socket; each mutation made through the gateway sends one
/// `ticket_changed` with ids + metadata only; two rooms stay apart; a pass
/// without tickets:read in a room gets no socket and 403 reads; assign,
/// option pick vs free text and status set work for apps; the brief comes
/// back cleaned and over-cap briefs are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_tickets_socket_and_parity() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let anna = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("othr{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_docs_id, _c1, docs_path) = seed_workspace(&docs);
        let (_anna_id, _c2, anna_path) = seed_workspace(&anna);
        let (_other_id, _c3, other_path) = seed_workspace(&other);
        for (user, role, rooms) in [
            (
                "tkbob",
                "tkdocs",
                vec![
                    (docs.as_str(), r#"["thread:read","tickets:read","tickets:post"]"#),
                    (anna.as_str(), r#"["thread:read","thread:post"]"#),
                ],
            ),
            ("tkcarl", "tkanna", vec![(anna.as_str(), r#"["tickets:read"]"#)]),
        ] {
            add_user(dport, user);
            set_password(dport, user, "s3cret-horse");
            let r = http(
                dport,
                "POST",
                &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"name":"{role}"}}"#)),
            );
            assert_eq!(r.status, 200, "role {role}; {}", r.body);
            for (handle, caps) in rooms {
                let r = http(
                    dport,
                    "POST",
                    &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
                    Some(&format!(r#"{{"name":"{role}","handle":"{handle}","caps":{caps}}}"#)),
                );
                assert_eq!(r.status, 200, "room {handle}; {}", r.body);
            }
            let r = http(
                dport,
                "POST",
                &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"username":"{user}","role":"{role}"}}"#)),
            );
            assert_eq!(r.status, 200, "assign {user}; {}", r.body);
        }

        let gport = free_port();
        publish_skin(dport, &docs_path, gport, None);
        let bob = gateway_login_cookie(gport, "tkbob", "s3cret-horse");
        let carl = gateway_login_cookie(gport, "tkcarl", "s3cret-horse");

        // No tickets:read in anna → no socket (thread caps only).
        let refused = ws_upgrade(gport, &format!("/cli/activity/events?workspace={anna}"), &bob);
        assert_eq!(refused.status, 403, "bob anna socket; {}", refused.body);
        assert!(refused.body.contains("tickets:read"), "{}", refused.body);

        let mut bob_docs = open_room_socket(gport, &docs, &bob);
        let mut carl_anna = open_room_socket(gport, &anna, &carl);
        std::thread::sleep(Duration::from_millis(200));

        // Owner tickets in a room nobody holds, then in anna.
        let owner_create = |project: &str, extra: &str| {
            let r = http(
                dport,
                "POST",
                &format!("/cli/feedback/create?token={OWNER_TOKEN}"),
                Some(&format!(
                    r#"{{"project":"{project}","title":"Owner ask","briefHtml":"{OWNER_BRIEF}"{extra}}}"#
                )),
            );
            assert_eq!(r.status, 200, "owner create {project}; {}", r.body);
            json(&r.body)["id"].as_str().expect("id").to_string()
        };
        let _other_ticket = owner_create(&other, "");
        let anna_ticket = owner_create(&anna, r#","options":["Yes","No"]"#);

        let f = try_read_ws_text(&mut carl_anna, Duration::from_secs(5)).expect("carl hears anna");
        assert_eq!(
            json(&f),
            serde_json::json!({"kind": "ticket_changed", "workspace": anna, "id": anna_ticket,
                               "change": "created", "status": "waiting", "via": null,
                               "hasBrief": true}),
            "{f}"
        );
        assert!(!f.contains("Owner ask"), "no title on the wire: {f}");
        assert!(!f.contains(&anna_path), "no path on the wire: {f}");

        // Carl (tickets:read only) cannot write; reads in anna work.
        let r = http_ex(
            gport,
            "POST",
            "/cli/feedback/comment",
            Some(&format!(r#"{{"id":"{anna_ticket}","body":"Yes","optionPick":true}}"#)),
            &carl,
        );
        assert_eq!(r.status, 403, "{}", r.body);
        assert!(r.body.contains("missing capability tickets:post"), "{}", r.body);
        // Bob has no tickets:read in anna: 403 on reads.
        let r = http_ex(gport, "GET", &format!("/cli/feedback/show?id={anna_ticket}"), None, &bob);
        assert_eq!(r.status, 403, "{}", r.body);
        assert!(r.body.contains("missing capability tickets:read"), "{}", r.body);

        // Bob works in docs through the gateway.
        let post = |path: &str, body: serde_json::Value| {
            http_ex(gport, "POST", path, Some(&body.to_string()), &bob)
        };
        let r = post(
            "/cli/feedback/create",
            serde_json::json!({"project": docs, "title": "Which colour?",
                               "options": ["Navy", "Teal"],
                               "briefHtml": "<p>Pick<script>alert(1)</script></p>"}),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let t = json(&r.body)["id"].as_str().expect("id").to_string();
        let r = post("/cli/feedback/comment", serde_json::json!({"id": t, "body": "why?"}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r.body)["status"], "needs_discussion", "{}", r.body);
        let r = post(
            "/cli/feedback/comment",
            serde_json::json!({"id": t, "body": "Navy", "optionPick": true}),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r.body)["answered"], true, "{}", r.body);
        let r = post("/cli/feedback/resolve", serde_json::json!({"id": t, "status": "planned"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = post(
            "/cli/feedback/assign",
            serde_json::json!({"id": t, "usernames": ["owner", "nobody-here-zz"]}),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let av = json(&r.body);
        assert_eq!(av["assignees"], serde_json::json!(["nobody-here-zz", "owner"]), "{av}");
        assert_eq!(av["warnings"][0]["code"], "assignee_unknown", "{av}");
        let r = post("/cli/feedback/resolve", serde_json::json!({"id": t}));
        assert_eq!(r.status, 200, "{}", r.body);

        let want = [
            ("created", serde_json::Value::Null, "waiting"),
            ("commented", serde_json::json!("free_text"), "needs_discussion"),
            ("answered", serde_json::json!("option_pick"), "answered"),
            ("status_changed", serde_json::Value::Null, "planned"),
            ("assigned", serde_json::Value::Null, "planned"),
            ("resolved", serde_json::Value::Null, "resolved"),
        ];
        for (change, via, status) in want {
            let f = try_read_ws_text(&mut bob_docs, Duration::from_secs(5))
                .unwrap_or_else(|| panic!("bob: no {change} frame"));
            assert_eq!(
                json(&f),
                serde_json::json!({"kind": "ticket_changed", "workspace": docs, "id": t,
                                   "change": change, "status": status, "via": via,
                                   "hasBrief": true}),
                "{f}"
            );
            assert!(!f.contains("Navy") && !f.contains("nobody-here-zz"), "no bodies or names: {f}");
        }
        assert!(
            try_read_ws_text(&mut bob_docs, Duration::from_millis(600)).is_none(),
            "each mutation sends one frame; no other room's ticket"
        );
        assert!(
            try_read_ws_text(&mut carl_anna, Duration::from_millis(600)).is_none(),
            "room anna never hears docs or the unheld room"
        );

        // Brief: fetch on demand, cleaned, no host internals.
        let r = http_ex(gport, "GET", &format!("/cli/feedback/show?id={t}&brief=1"), None, &bob);
        assert_eq!(r.status, 200, "{}", r.body);
        let sv = json(&r.body);
        assert_eq!(sv["brief"]["html"], "<p>Pick</p>", "{sv}");
        assert_eq!(sv["assignees"], serde_json::json!(["nobody-here-zz", "owner"]), "{sv}");
        assert!(sv.get("projectPath").is_none(), "{sv}");
        assert!(sv.get("canonicalSessionId").is_none(), "{sv}");
        assert!(!r.body.contains(&docs_path), "{}", r.body);
        // Over the 1 MiB cap → refused, nothing stored, no frame.
        let big = format!("<p>{}</p>", "x".repeat(k2_core::feedback_brief::MAX_BRIEF_BYTES));
        let r = post(
            "/cli/feedback/create",
            serde_json::json!({"project": docs, "title": "Too big", "briefHtml": big}),
        );
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(json(&r.body)["error"]["code"], "brief_too_large", "{}", r.body);
        assert!(
            try_read_ws_text(&mut bob_docs, Duration::from_millis(600)).is_none(),
            "a refused create sends nothing"
        );

        drop(bob_docs);
        drop(carl_anna);
        stop_skin(dport, &docs_path);
        for p in [&docs_path, &anna_path, &other_path] {
            let _ = std::fs::remove_dir_all(p);
        }
    });
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
    // Read until the head + the advertised Content-Length body are in (or
    // EOF / a 101 with no body). The gateway writes head and body as two
    // segments, so a single read can legitimately see only the head.
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
        let text = String::from_utf8_lossy(&raw);
        let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
        let clen = head.lines().find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        });
        match clen {
            Some(c) if body.len() < c => continue,
            _ => break,
        }
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

        // Secret cards answer into the K2 vault (0.45.4). The in-process
        // harness doesn't run daemon_main, so open this temp home's vault
        // the way boot does (a file key store: never the Keychain).
        k2_daemon::vault::boot(&k2_core::paths::k2_home());
        let secret_val = "s3cr3t-NEVER-IN-DAEMON-ANSWER-xyz";
        let secret = http(
            dport,
            "POST",
            &format!("/cli/thread/secret?token={OWNER_TOKEN}"),
            Some(&format!(
                // A guest may fill a vault card only for a named site.
                r#"{{"addr":"{handle}","name":"API_TOKEN","prompt":"Paste","for":"api.example.com"}}"#
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

        std::fs::write(format!("{docs_path}/doc.pdf"), b"%PDF-1.4 gateway\n").expect("pdf");
        let bin_docs = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-binary?workspace={docs}&path=doc.pdf"),
            None,
            &cookie,
        );
        assert_eq!(bin_docs.status, 200, "binary docs; {}", bin_docs.body);
        assert!(
            json(&bin_docs.body)["base64"].as_str().is_some(),
            "{}",
            bin_docs.body
        );
        let bin_anna = http_ex(
            gport,
            "GET",
            &format!("/cli/fs/read-binary?workspace={anna}&path=README.md"),
            None,
            &cookie,
        );
        assert_eq!(bin_anna.status, 403, "binary anna; {}", bin_anna.body);
        assert!(
            bin_anna.body.contains("missing capability files:read"),
            "anna binary must be missing cap: {}",
            bin_anna.body
        );

        let upload = http_ex(
            gport,
            "POST",
            "/cli/fs/upload-binary",
            Some(&format!(
                r#"{{"workspace":"{docs}","dir":".","filename":"up.pdf","base64":"{}"}}"#,
                json(&bin_docs.body)["base64"].as_str().unwrap()
            )),
            &cookie,
        );
        assert_eq!(upload.status, 200, "upload docs; {}", upload.body);

        let create = http_ex(
            gport,
            "POST",
            "/cli/fs/create",
            Some(&format!(
                r#"{{"workspace":"{docs}","path":"inbox","is_directory":true}}"#
            )),
            &cookie,
        );
        assert_eq!(create.status, 200, "create docs; {}", create.body);

        let pin_none = http_ex(
            gport,
            "POST",
            "/cli/workspace/ensure-pinned-chat",
            Some(&format!(r#"{{"workspace":"{docs}"}}"#)),
            &cookie,
        );
        assert_eq!(
            pin_none.status, 404,
            "pin find-only none; {}",
            pin_none.body
        );

        let info = http_ex(gport, "GET", "/cli/fs/info", None, &cookie);
        assert_eq!(info.status, 404, "info; {}", info.body);
        assert!(info.body.contains("not found"), "{}", info.body);
        let delete = http_ex(
            gport,
            "POST",
            "/cli/fs/delete",
            Some(r#"{"paths":["doc.pdf"]}"#),
            &cookie,
        );
        assert_eq!(delete.status, 404, "delete; {}", delete.body);
        assert!(delete.body.contains("not found"), "{}", delete.body);
        // Resumable guest upload through the gateway: jailed to the room,
        // never a host-wide absolute dir.
        let chunk_abs = http_ex(
            gport,
            "POST",
            "/cli/fs/upload-chunk",
            Some(&format!(
                r#"{{"workspace":"{docs}","upload_id":"x","dir":"/tmp","filename":"a","offset":0,"base64":"YQ==","is_last":true}}"#
            )),
            &cookie,
        );
        assert_eq!(chunk_abs.status, 400, "upload-chunk abs dir; {}", chunk_abs.body);
        let chunk0 = http_ex(
            gport,
            "POST",
            "/cli/fs/upload-chunk",
            Some(&format!(
                r#"{{"workspace":"{docs}","upload_id":"gw1","dir":"up","filename":"big.bin","offset":0,"base64":"QUJD","is_last":false,"total_bytes":6}}"#
            )),
            &cookie,
        );
        assert_eq!(chunk0.status, 200, "upload-chunk 0; {}", chunk0.body);
        assert_eq!(json(&chunk0.body)["received"].as_u64(), Some(3), "{}", chunk0.body);
        let chunk1 = http_ex(
            gport,
            "POST",
            "/cli/fs/upload-chunk",
            Some(&format!(
                r#"{{"workspace":"{docs}","upload_id":"gw1","dir":"up","filename":"big.bin","offset":3,"base64":"REVG","is_last":true}}"#
            )),
            &cookie,
        );
        assert_eq!(chunk1.status, 200, "upload-chunk 1; {}", chunk1.body);
        assert_eq!(json(&chunk1.body)["path"].as_str(), Some("up/big.bin"), "{}", chunk1.body);
        assert_eq!(json(&chunk1.body)["done"].as_bool(), Some(true), "{}", chunk1.body);
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
            Some(&format!(
                r#"{{"project":"{anna}","title":"Anna ask","briefHtml":"{OWNER_BRIEF}"}}"#
            )),
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
            Some(&format!(
                r#"{{"project":"{other}","title":"Other ask","briefHtml":"{OWNER_BRIEF}"}}"#
            )),
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
        // Ticket brief H1: an app guest is a person. No brief needed,
        // and no `brief_missing` warning either.
        let cd = json(&create_docs.body);
        assert_eq!(cd["hasBrief"], false, "{}", create_docs.body);
        assert_eq!(cd["warnings"], serde_json::json!([]), "{}", create_docs.body);
        let docs_id = cd["id"].as_str().expect("docs id").to_string();

        // Ticket brief T6: a guest MAY attach one; it is cleaned and stored.
        let guest_brief = serde_json::json!({
            "project": docs,
            "title": "Guest brief",
            "briefHtml": "<p>Hi<script>alert(1)</script></p><a href=\"javascript:x()\">x</a>",
        });
        let create_brief = http_ex(
            gport,
            "POST",
            "/cli/feedback/create",
            Some(&guest_brief.to_string()),
            &cookie,
        );
        assert_eq!(create_brief.status, 200, "guest brief; {}", create_brief.body);
        let cb = json(&create_brief.body);
        assert_eq!(cb["hasBrief"], true, "{}", create_brief.body);
        let brief_id = cb["id"].as_str().expect("brief id").to_string();
        let show_brief = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/show?id={brief_id}&brief=1"),
            None,
            &cookie,
        );
        assert_eq!(show_brief.status, 200, "guest show brief; {}", show_brief.body);
        let sb = json(&show_brief.body);
        assert_eq!(
            sb["brief"]["html"], "<p>Hi</p><a rel=\"noopener noreferrer\">x</a>",
            "{}",
            show_brief.body
        );
        assert!(sb["brief"].get("projectPath").is_none(), "H40: {}", show_brief.body);
        // Room check unchanged: another room's brief is a 403, no oracle.
        let show_other_brief = http_ex(
            gport,
            "GET",
            &format!("/cli/feedback/show?id={other_id}&brief=1"),
            None,
            &cookie,
        );
        assert_eq!(show_other_brief.status, 403, "{}", show_other_brief.body);
        assert!(show_other_brief.body.contains("skin_room"), "{}", show_other_brief.body);
        assert!(!show_other_brief.body.contains("Owner ask"), "{}", show_other_brief.body);

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

        // The host-wide Tickets list is never an app door.
        let list_all = http_ex(gport, "GET", "/cli/feedback/list-all?all=1", None, &cookie);
        assert_eq!(list_all.status, 404, "list-all; {}", list_all.body);
        assert!(list_all.body.contains("not found"), "{}", list_all.body);

        // prd-app-tickets-websocket-v1 (0.43.3): assign is an app door
        // with tickets:post in the ticket's room; anna's is a cap miss.
        let assign_gw = http_ex(
            gport,
            "POST",
            "/cli/feedback/assign",
            Some(&format!(r#"{{"id":"{docs_id}","usernames":["owner"]}}"#)),
            &cookie,
        );
        assert_eq!(assign_gw.status, 200, "assign; {}", assign_gw.body);
        assert_eq!(json(&assign_gw.body)["assignees"], serde_json::json!(["owner"]));
        let assign_anna = http_ex(
            gport,
            "POST",
            "/cli/feedback/assign",
            Some(&format!(r#"{{"id":"{anna_id}","usernames":["owner"]}}"#)),
            &cookie,
        );
        assert_eq!(assign_anna.status, 403, "assign anna; {}", assign_anna.body);
        assert!(
            assign_anna.body.contains("missing capability tickets:post"),
            "{}",
            assign_anna.body
        );
        let assign_get = http_ex(gport, "GET", "/cli/feedback/assign", None, &cookie);
        assert_eq!(assign_get.status, 404, "GET assign is no door; {}", assign_get.body);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_wiki_per_room() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let anna = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (anna_id, _anna_conv, anna_path) = seed_workspace(&anna);
        let (docs_id, _docs_conv, docs_path) = seed_workspace(&docs);
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
                r#"{{"name":"dentist","handle":"{docs}","caps":["thread:read","thread:post","wiki:read"]}}"#
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
        let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_ui="), "{set_cookie}");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        assert!(
            !sid.starts_with("k2skn_"),
            "cookie is not the raw pass: {sid}"
        );
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let docs_index = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/index?project={docs}"),
            None,
            &cookie,
        );
        assert_eq!(
            docs_index.status, 200,
            "empty docs wiki index; {}",
            docs_index.body
        );
        assert!(!docs_index.body.contains("k2skn_"), "{}", docs_index.body);
        assert!(
            docs_index.body.contains("\"noteCount\":0") || docs_index.body.contains("noteCount"),
            "empty vault must not auto-seed: {}",
            docs_index.body
        );

        k2_core::wiki::seed_wiki(std::path::Path::new(&docs_path)).expect("seed docs wiki");

        let docs_note = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/note?project={docs}&id=Home.md"),
            None,
            &cookie,
        );
        assert_eq!(docs_note.status, 200, "docs note; {}", docs_note.body);
        assert!(!docs_note.body.contains("k2skn_"), "{}", docs_note.body);
        assert!(
            docs_note.body.contains("Knowledge Base") || docs_note.body.contains("Home"),
            "{}",
            docs_note.body
        );

        let anna_index = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/index?project={anna}"),
            None,
            &cookie,
        );
        assert_eq!(
            anna_index.status, 403,
            "anna wiki index; {}",
            anna_index.body
        );
        assert!(
            anna_index.body.contains("missing capability wiki:read"),
            "anna index must be missing cap, not skin_room: {}",
            anna_index.body
        );
        assert!(
            !anna_index.body.contains("skin_room"),
            "anna index must not be skin_room: {}",
            anna_index.body
        );

        let thread_anna = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={anna}"),
            None,
            &cookie,
        );
        assert_eq!(thread_anna.status, 200, "thread anna; {}", thread_anna.body);

        let abs_index = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/index?project={docs_path}"),
            None,
            &cookie,
        );
        assert_eq!(abs_index.status, 403, "abs project=; {}", abs_index.body);
        assert!(
            abs_index.body.contains("skin_room"),
            "abs path project= must be skin_room: {}",
            abs_index.body
        );

        let base = std::path::Path::new(&docs_path)
            .file_name()
            .and_then(|s| s.to_str())
            .expect("basename");
        let base_index = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/index?project={base}"),
            None,
            &cookie,
        );
        assert_eq!(
            base_index.status, 403,
            "basename project=; {}",
            base_index.body
        );
        assert!(
            base_index.body.contains("skin_room"),
            "folder-basename project= must be skin_room: {}",
            base_index.body
        );

        for scope in ["k2", "host", "fleet"] {
            let scoped = http_ex(
                gport,
                "GET",
                &format!("/cli/wiki/index?scope={scope}&project={docs}"),
                None,
                &cookie,
            );
            assert_eq!(
                scoped.status, 403,
                "scope={scope} with project; {}",
                scoped.body
            );
            assert!(
                scoped.body.contains("skin_room"),
                "scope={scope} must be skin_room: {}",
                scoped.body
            );
            assert!(
                !scoped.body.contains("\"scope\":\"k2\""),
                "must not return a fleet map: {}",
                scoped.body
            );
            let scoped_only = http_ex(
                gport,
                "GET",
                &format!("/cli/wiki/index?scope={scope}"),
                None,
                &cookie,
            );
            assert_eq!(
                scoped_only.status, 403,
                "scope={scope} alone; {}",
                scoped_only.body
            );
            assert!(
                scoped_only.body.contains("skin_room"),
                "scope={scope} alone must be skin_room: {}",
                scoped_only.body
            );
        }

        let fleet = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/note?project={docs}&id={docs_id}::Home.md"),
            None,
            &cookie,
        );
        assert_eq!(fleet.status, 403, "fleet id; {}", fleet.body);
        assert!(
            fleet.body.contains("skin_room"),
            "fleet id must be skin_room: {}",
            fleet.body
        );

        let hub = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/note?project={docs}&id={docs_id}::__workspace__"),
            None,
            &cookie,
        );
        assert_eq!(hub.status, 403, "workspace hub; {}", hub.body);
        assert!(hub.body.contains("skin_room"), "{}", hub.body);

        let fg = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/note?project={docs}&id=__focusgroup__::abc"),
            None,
            &cookie,
        );
        assert_eq!(fg.status, 403, "focus group hub; {}", fg.body);
        assert!(fg.body.contains("skin_room"), "{}", fg.body);

        let proj = http_ex(
            gport,
            "GET",
            &format!("/cli/wiki/note?project={docs}&id=__project__::abc"),
            None,
            &cookie,
        );
        assert_eq!(proj.status, 403, "project hub; {}", proj.body);
        assert!(proj.body.contains("skin_room"), "{}", proj.body);

        for path in [
            "/cli/wiki/seed",
            "/cli/wiki/serve",
            "/cli/wiki/chat",
            "/cli/wiki/serve/status",
            "/cli/wiki/status",
            "/cli/wiki/foo",
            "/cli/wiki/read",
        ] {
            let r = http_ex(gport, "GET", path, None, &cookie);
            assert_eq!(r.status, 404, "{path}; {}", r.body);
            assert!(r.body.contains("not found"), "{path} {}", r.body);
            let post = http_ex(gport, "POST", path, Some("{}"), &cookie);
            assert_eq!(post.status, 404, "POST {path}; {}", post.body);
        }

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
        let _ = anna_id;
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_store_per_room() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let anna = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_anna_id, _anna_conv, anna_path) = seed_workspace(&anna);
        let (_docs_id, _docs_conv, docs_path) = seed_workspace(&docs);
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
                r#"{{"name":"dentist","handle":"{docs}","caps":["thread:read","thread:post","store:read"]}}"#
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

        let login = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "gateway login; {}", login.body);
        assert!(!login.body.contains("k2skn_"), "{}", login.body);
        let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let docs_list = http_ex(
            gport,
            "GET",
            &format!("/cli/store/list?workspace={docs}"),
            None,
            &cookie,
        );
        assert_ne!(
            docs_list.status, 401,
            "allowlisted store; {}",
            docs_list.body
        );
        assert!(
            !docs_list.body.contains("missing capability store:read"),
            "docs with store:read must not be missing cap: {}",
            docs_list.body
        );
        assert!(!docs_list.body.contains("k2skn_"), "{}", docs_list.body);

        let anna_list = http_ex(
            gport,
            "GET",
            &format!("/cli/store/list?workspace={anna}"),
            None,
            &cookie,
        );
        assert_eq!(anna_list.status, 403, "anna store list; {}", anna_list.body);
        assert!(
            anna_list.body.contains("missing capability store:read"),
            "anna list must be missing cap, not skin_room: {}",
            anna_list.body
        );
        assert!(
            !anna_list.body.contains("skin_room"),
            "anna list must not be skin_room: {}",
            anna_list.body
        );

        let abs_list = http_ex(
            gport,
            "GET",
            &format!("/cli/store/list?workspace={docs_path}"),
            None,
            &cookie,
        );
        assert_eq!(abs_list.status, 403, "abs workspace=; {}", abs_list.body);
        assert!(
            abs_list.body.contains("skin_room"),
            "abs path workspace= must be skin_room: {}",
            abs_list.body
        );

        let put = http_ex(
            gport,
            "POST",
            "/cli/store/put",
            Some(&format!(
                r#"{{"workspace":"{docs}","name":"items","id":"a","json":{{}}}}"#
            )),
            &cookie,
        );
        assert_eq!(put.status, 404, "put 404 on gateway; {}", put.body);
        assert!(put.body.contains("not found"), "{}", put.body);

        let foo = http_ex(gport, "GET", "/cli/store/foo", None, &cookie);
        assert_eq!(foo.status, 404, "store foo; {}", foo.body);

        let tables = http_ex(
            gport,
            "GET",
            &format!("/cli/db/tables?workspace={docs}"),
            None,
            &cookie,
        );
        assert_ne!(
            tables.status, 401,
            "allowlisted dump tables; {}",
            tables.body
        );
        assert!(
            !tables.body.contains("missing capability store:read"),
            "docs store:read must list dump tables: {}",
            tables.body
        );

        let anna_tables = http_ex(
            gport,
            "GET",
            &format!("/cli/db/tables?workspace={anna}"),
            None,
            &cookie,
        );
        assert_eq!(
            anna_tables.status, 403,
            "anna dump tables; {}",
            anna_tables.body
        );
        assert!(
            anna_tables.body.contains("missing capability store:read"),
            "anna dump tables must be missing cap: {}",
            anna_tables.body
        );

        let dump_ins = http_ex(
            gport,
            "POST",
            "/cli/db/rows",
            Some(&format!(
                r#"{{"workspace":"{docs}","table":"example","row":{{"n":1}}}}"#
            )),
            &cookie,
        );
        assert_eq!(
            dump_ins.status, 403,
            "store:read does not grant dump write; {}",
            dump_ins.body
        );
        assert!(
            dump_ins.body.contains("missing capability store:write"),
            "{}",
            dump_ins.body
        );

        let db_foo = http_ex(gport, "GET", "/cli/db/foo", None, &cookie);
        assert_eq!(db_foo.status, 404, "db foo; {}", db_foo.body);

        let db_list = http_ex(gport, "GET", "/cli/db/list", None, &cookie);
        assert_eq!(
            db_list.status, 404,
            "--skin /cli/db/list 404; {}",
            db_list.body
        );

        let gw_dsn = http_ex(gport, "GET", "/cli/db/dsn", None, &cookie);
        assert_eq!(gw_dsn.status, 404, "gateway dsn 404; {}", gw_dsn.body);

        let daemon_login = http(
            dport,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        assert_eq!(daemon_login.status, 200, "{}", daemon_login.body);
        let session_tok = json(&daemon_login.body)["token"]
            .as_str()
            .expect("session token")
            .to_string();
        assert!(session_tok.starts_with("k2skn_"), "{session_tok}");
        let dsn = http(
            dport,
            "GET",
            &format!("/cli/db/dsn?token={session_tok}"),
            None,
        );
        assert_eq!(dsn.status, 403, "k2skn_ dsn; {}", dsn.body);
        assert!(
            dsn.body
                .contains("skin tokens cannot use database credentials"),
            "teaching 403: {}",
            dsn.body
        );
        assert!(
            !dsn.body.contains("postgres://"),
            "must not invent DSN: {}",
            dsn.body
        );

        let plat = mint(
            dport,
            "vercel-store",
            &["store:read", "thread:read"],
            &[&docs],
        );
        let plat_store = http(
            dport,
            "GET",
            &format!("/cli/store/list?token={plat}&workspace={docs}"),
            None,
        );
        assert_eq!(
            plat_store.status, 403,
            "platform mint cannot store; {}",
            plat_store.body
        );
        assert!(
            plat_store.body.contains("platform tokens cannot use store"),
            "{}",
            plat_store.body
        );
        let plat_dsn = http(dport, "GET", &format!("/cli/db/dsn?token={plat}"), None);
        assert_eq!(plat_dsn.status, 403, "platform dsn; {}", plat_dsn.body);
        assert!(
            plat_dsn
                .body
                .contains("skin tokens cannot use database credentials"),
            "{}",
            plat_dsn.body
        );

        stop_skin(dport, &anna_path);
        let _ = std::fs::remove_dir_all(&anna_path);
        let _ = std::fs::remove_dir_all(&docs_path);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_password_reset_and_change() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("pwgw{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");
        // d19889de (prd-skin-guest-email-v1, LIVE 0.40.139): forgot mints
        // a reset token only for a guest with a password AND an email;
        // otherwise it answers the uniform `{"ok":true}` miss.
        let email = http(
            dport,
            "POST",
            &format!("/cli/skin/users/email?token={OWNER_TOKEN}"),
            Some(r#"{"username":"guest","email":"guest@clinic.example"}"#),
        );
        assert_eq!(email.status, 200, "set email; {}", email.body);

        let gport = free_port();
        publish_skin(dport, &path, gport, None);

        let reset_page = http(gport, "GET", "/reset", None);
        assert_eq!(reset_page.status, 200, "{}", reset_page.body);
        assert!(
            reset_page.body.contains("Reset password") || reset_page.body.contains("password"),
            "bundled /reset never 404: {}",
            reset_page.body
        );
        let cc = header_value(&reset_page.headers, "cache-control").expect("Cache-Control");
        assert!(
            cc.to_ascii_lowercase().contains("no-store"),
            "GET /reset Cache-Control: {cc}"
        );

        let forgot_gw = http(
            gport,
            "POST",
            "/cli/skin/password/forgot",
            Some(r#"{"username":"guest"}"#),
        );
        assert_eq!(
            forgot_gw.status, 404,
            "forgot 404 on --skin; {}",
            forgot_gw.body
        );

        let mint = http(
            dport,
            "POST",
            &format!("/cli/skin/password/forgot?token={OWNER_TOKEN}"),
            Some(r#"{"username":"guest"}"#),
        );
        assert_eq!(mint.status, 200, "{}", mint.body);
        let raw = json(&mint.body)["token"]
            .as_str()
            .expect("reset token")
            .to_string();
        assert!(!raw.starts_with("k2skn_"), "{raw}");

        let consume = http(
            gport,
            "POST",
            "/cli/skin/password/reset",
            Some(&format!(
                r#"{{"token":"{raw}","password":"new-pass-word"}}"#
            )),
        );
        assert_eq!(
            consume.status, 200,
            "public consume no cookie; {}",
            consume.body
        );
        assert!(!consume.body.contains("k2skn_"), "{}", consume.body);
        assert_eq!(json(&consume.body)["ok"], true);
        assert!(
            header_value(&consume.headers, "set-cookie")
                .map(|c| !c.contains("k2_skin_ui="))
                .unwrap_or(true),
            "consume must not mint a gateway session; {}",
            consume.headers
        );

        let login = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"guest","password":"new-pass-word"}"#),
        );
        assert_eq!(login.status, 200, "{}", login.body);
        let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
        let cookie = format!("Cookie: k2_skin_ui={sid}");

        let grid = http_ex(
            gport,
            "GET",
            "/cli/sessions/grid?session=nope",
            None,
            &cookie,
        );
        assert_eq!(grid.status, 403, "grid still 403; {}", grid.body);

        let change = http_ex(
            gport,
            "POST",
            "/cli/skin/password/change",
            Some(r#"{"oldPassword":"new-pass-word","password":"third-pass"}"#),
            &cookie,
        );
        assert_eq!(change.status, 200, "{}", change.body);
        let clear = header_value(&change.headers, "set-cookie").expect("clear cookie");
        assert!(clear.contains("Max-Age=0"), "{clear}");
        assert!(clear.contains("k2_skin_ui="), "{clear}");

        let after = http_ex(
            gport,
            "GET",
            &format!("/cli/thread?addr={handle}"),
            None,
            &cookie,
        );
        assert_eq!(
            after.status, 401,
            "dead cookie after change; {}",
            after.body
        );

        let change_nocookie = http(
            gport,
            "POST",
            "/cli/skin/password/change",
            Some(r#"{"oldPassword":"third-pass","password":"fourth-pw"}"#),
        );
        assert_eq!(
            change_nocookie.status, 401,
            "change requires cookie; {}",
            change_nocookie.body
        );

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

/// Gateway login through the helper; returns the `Cookie:` header line.
fn gateway_login_cookie(gport: u16, username: &str, password: &str) -> String {
    let login = http(
        gport,
        "POST",
        "/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(login.status, 200, "gateway login; {}", login.body);
    let set_cookie = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
    let sid = cookie_k2_skin_ui(&set_cookie).expect("opaque id");
    format!("Cookie: k2_skin_ui={sid}")
}

/// GA3/GA4 (prd-gateway-activity-socket-workspace-v1): the helper forwards
/// `/cli/activity/events?workspace=` to the daemon. Before the fix it wrote
/// `400 missing conversation query parameter` itself and never dialled up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_activity_ws_per_room() {
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
            Some(r#"{"name":"watcher"}"#),
        );
        assert_eq!(created.status, 200, "role create; {}", created.body);
        for (handle, caps) in [
            (&anna, r#"["thread:read","thread:post"]"#),
            (&docs, r#"["thread:read","thread:post","activity:read"]"#),
        ] {
            let r = http(
                dport,
                "POST",
                &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
                Some(&format!(
                    r#"{{"name":"watcher","handle":"{handle}","caps":{caps}}}"#
                )),
            );
            assert_eq!(r.status, 200, "room {handle}; {}", r.body);
        }
        let assign = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"bob","role":"watcher"}"#),
        );
        assert_eq!(assign.status, 200, "assign; {}", assign.body);

        let gport = free_port();
        publish_skin(dport, &anna_path, gport, None);
        let cookie = gateway_login_cookie(gport, "bob", "s3cret-horse");

        // GA3: the regression. Exactly 101.
        let docs_ws = ws_upgrade(
            gport,
            &format!("/cli/activity/events?workspace={docs}"),
            &cookie,
        );
        assert_eq!(docs_ws.status, 101, "activity WS docs; {}", docs_ws.body);

        // GA4: the refusals stay.
        let anna_ws = ws_upgrade(
            gport,
            &format!("/cli/activity/events?workspace={anna}"),
            &cookie,
        );
        assert_eq!(anna_ws.status, 403, "activity WS anna; {}", anna_ws.body);
        assert!(
            anna_ws.body.contains("activity:read"),
            "anna must name activity:read: {}",
            anna_ws.body
        );

        let wrong = ws_upgrade(
            gport,
            "/cli/activity/events?workspace=not-a-handle",
            &cookie,
        );
        assert_eq!(wrong.status, 403, "wrong room; {}", wrong.body);
        assert!(wrong.body.contains("skin_room"), "{}", wrong.body);

        let missing = ws_upgrade(gport, "/cli/activity/events", &cookie);
        assert_eq!(missing.status, 400, "no workspace; {}", missing.body);
        assert!(
            missing.body.contains("missing workspace query parameter"),
            "{}",
            missing.body
        );
        assert!(
            !missing.body.contains("conversation"),
            "must not ask for conversation: {}",
            missing.body
        );

        let empty = ws_upgrade(gport, "/cli/activity/events?workspace=", &cookie);
        assert_eq!(empty.status, 400, "empty workspace; {}", empty.body);
        let prefixed = ws_upgrade(
            gport,
            &format!("/cli/activity/events?xworkspace={docs}"),
            &cookie,
        );
        assert_eq!(prefixed.status, 400, "xworkspace; {}", prefixed.body);

        let sub = ws_upgrade(
            gport,
            &format!("/cli/activity/events/foo?workspace={docs}"),
            &cookie,
        );
        assert_eq!(sub.status, 403, "sub path; {}", sub.body);
        assert!(sub.body.contains("not allowed"), "{}", sub.body);

        let sessions_ev = ws_upgrade(gport, "/cli/sessions/events", &cookie);
        assert_eq!(sessions_ev.status, 403, "sessions; {}", sessions_ev.body);
        assert!(
            sessions_ev.body.contains("not allowed"),
            "{}",
            sessions_ev.body
        );

        stop_skin(dport, &anna_path);
        let _ = std::fs::remove_dir_all(&anna_path);
        let _ = std::fs::remove_dir_all(&docs_path);
    });
}

/// Read one unmasked server WebSocket text frame (test-only parser).
fn read_ws_text(stream: &mut StdTcpStream) -> String {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).expect("ws frame head");
    assert_eq!(head[0] & 0x0f, 0x1, "expected a text frame, opcode {:#x}", head[0]);
    let len = match head[1] & 0x7f {
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).expect("ws len16");
            u16::from_be_bytes(ext) as usize
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).expect("ws len64");
            u64::from_be_bytes(ext) as usize
        }
        n => n as usize,
    };
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("ws payload");
    String::from_utf8(payload).expect("utf8 frame")
}

/// AH20–AH22 through the helper: an app with `heartbeats:read` (no
/// `activity:read`) opens the room socket through the gateway and hears
/// `heartbeat_changed` after a heartbeat add made through the gateway.
/// The heartbeat routes are allowlisted; host-wide ones are not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_heartbeats_and_socket() {
    let _g = lock();
    struct NoSelfHeal(Option<std::ffi::OsString>);
    impl Drop for NoSelfHeal {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => std::env::set_var("K2_HEARTBEAT_NO_SELF_HEAL", v),
                None => std::env::remove_var("K2_HEARTBEAT_NO_SELF_HEAL"),
            }
        }
    }
    let _no_self_heal = NoSelfHeal(std::env::var_os("K2_HEARTBEAT_NO_SELF_HEAL"));
    std::env::set_var("K2_HEARTBEAT_NO_SELF_HEAL", "1");
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let docs = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_docs_id, _conv, docs_path) = seed_files_workspace(&docs);
        add_user(dport, "hbguest");
        set_password(dport, "hbguest", "s3cret-horse");
        let created = http(
            dport,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"hbrole"}"#),
        );
        assert_eq!(created.status, 200, "role; {}", created.body);
        let room = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"hbrole","handle":"{docs}","caps":["thread:read","heartbeats:read","heartbeats:write"]}}"#
            )),
        );
        assert_eq!(room.status, 200, "room; {}", room.body);
        let assign = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"hbguest","role":"hbrole"}"#),
        );
        assert_eq!(assign.status, 200, "assign; {}", assign.body);

        let gport = free_port();
        publish_skin(dport, &docs_path, gport, None);
        let cookie = gateway_login_cookie(gport, "hbguest", "s3cret-horse");

        let list = http_ex(
            gport,
            "GET",
            &format!("/cli/heartbeat/list?workspace={docs}"),
            None,
            &cookie,
        );
        assert_eq!(list.status, 200, "list through the helper; {}", list.body);
        assert_eq!(json(&list.body), serde_json::json!([]));
        let host_wide = http_ex(gport, "GET", "/cli/heartbeat/scheduler-status", None, &cookie);
        assert_eq!(host_wide.status, 404, "never allowlisted; {}", host_wide.body);
        let get_write = http_ex(
            gport,
            "GET",
            &format!("/cli/heartbeat/add?workspace={docs}"),
            None,
            &cookie,
        );
        assert_eq!(get_write.status, 404, "GET on a write path; {}", get_write.body);

        // Open the room socket through the helper and keep it.
        let mut sock = StdTcpStream::connect(("127.0.0.1", gport)).expect("ws connect");
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let req = format!(
            "GET /cli/activity/events?workspace={docs} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{cookie}\r\n\r\n"
        );
        sock.write_all(req.as_bytes()).expect("ws write");
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            sock.read_exact(&mut byte).expect("upgrade head");
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).to_string();
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "heartbeats:read opens the room socket through the helper: {head}"
        );
        std::thread::sleep(Duration::from_millis(200));

        let add = http_ex(
            gport,
            "POST",
            "/cli/heartbeat/add",
            Some(&format!(
                r#"{{"workspace":"{docs}","name":"inbox-sweep","frequency":"hourly","spec":{{"every_seconds":900}},"instructions":"Check the inbox."}}"#
            )),
            &cookie,
        );
        assert_eq!(add.status, 200, "add through the helper; {}", add.body);
        let v = json(&add.body);
        assert_eq!(v["name"], "inbox-sweep", "{v}");
        assert!(v.get("wakeupAbs").is_none(), "{v}");

        let frame = read_ws_text(&mut sock);
        let fv = json(&frame);
        assert_eq!(fv["kind"], "heartbeat_changed", "{frame}");
        assert_eq!(fv["workspace"], docs.as_str(), "{frame}");
        assert!(!frame.contains(&docs_path), "no path on the wire: {frame}");

        drop(sock);
        stop_skin(dport, &docs_path);
        let _ = std::fs::remove_dir_all(&docs_path);
    });
}

/// security-review-app-cookie-csrf-v1 §7 against a real temp daemon: a
/// state-changing request or socket from another origin is refused by the
/// helper and the side effect does not happen; the same request from the
/// app's own origin (or from a non-browser caller with no Origin and no
/// Sec-Fetch-Site) lands; daemon CORS headers never reach the browser; the
/// cookie is Secure with the pass TTL on a non-loopback Host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_refuses_cross_origin() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgxo{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, conv, path) = seed_workspace(&handle);
        add_user(dport, "guest");
        set_rooms(dport, "guest", &handle);
        set_password(dport, "guest", "s3cret-horse");
        let gport = free_port();
        publish_skin(dport, &path, gport, None);

        let host = "agents.acme.k2.dev";
        let same = "Origin: https://agents.acme.k2.dev";
        let creds = r#"{"username":"guest","password":"s3cret-horse"}"#;

        // Login CSRF: refused, no cookie.
        let evil_login = http_host(
            gport,
            host,
            "POST",
            "/login",
            Some(creds),
            "Origin: https://evil.k2.dev",
        );
        assert_eq!(evil_login.status, 403, "{}", evil_login.body);
        assert!(
            evil_login.body.contains("cross_origin_refused"),
            "{}",
            evil_login.body
        );
        assert!(
            header_value(&evil_login.headers, "set-cookie").is_none(),
            "{}",
            evil_login.headers
        );

        // Same-origin login on the tunnel host: Secure + Max-Age = pass TTL.
        let login = http_host(gport, host, "POST", "/login", Some(creds), same);
        assert_eq!(login.status, 200, "{}", login.body);
        let sc = header_value(&login.headers, "set-cookie").expect("Set-Cookie");
        let sid = cookie_k2_skin_ui(&sc).expect("opaque id");
        let max_age = k2_core::connect_users::session_ttl_days() * 86_400;
        assert_eq!(
            sc,
            format!(
                "__Host-k2_skin_ui={sid}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}; Secure"
            ),
            "cookie on a non-loopback Host with no X-Forwarded-Proto (S24-14 __Host- prefix)"
        );
        let cookie = format!("Cookie: __Host-k2_skin_ui={sid}");

        let read_thread = || {
            let r = http_host(
                gport,
                host,
                "GET",
                &format!("/cli/thread?addr={handle}"),
                None,
                &cookie,
            );
            assert_eq!(r.status, 200, "thread read; {}", r.body);
            r
        };

        // Foreign Origin and foreign Sec-Fetch-Site: 403, nothing posted.
        let marker = format!("csrf-{}", uuid::Uuid::new_v4());
        let post_body = format!(r#"{{"addr":"{handle}","text":"{marker}"}}"#);
        for bad in [
            format!("{cookie}\r\nOrigin: https://evil.k2.dev"),
            format!("{cookie}\r\nOrigin: null"),
            format!("{cookie}\r\nSec-Fetch-Site: cross-site"),
            format!("{cookie}\r\nSec-Fetch-Site: same-site"),
        ] {
            let r = http_host(gport, host, "POST", "/cli/thread/post", Some(&post_body), &bad);
            assert_eq!(r.status, 403, "{bad:?}: {}", r.body);
            assert!(r.body.contains("cross_origin_refused"), "{}", r.body);
        }
        // Read the store directly (same process as the temp daemon): a
        // skin post may respawn the pinned Chat and move the address to a
        // new conversation, so check every conversation seen.
        let bodies = |c: &str| -> Vec<String> {
            k2_core::overlay::read_thread_page(c, None, None, 500)
                .expect("read thread store")
                .items
                .into_iter()
                .filter_map(|i| i.doc.body)
                .collect()
        };
        let before = read_thread();
        let before_conv = json(&before.body)["conversation_id"]
            .as_str()
            .expect("conversation_id")
            .to_string();
        for c in [conv.as_str(), before_conv.as_str()] {
            assert!(
                !bodies(c).iter().any(|b| b.contains(&marker)),
                "a refused post must not land in {c}"
            );
        }

        // Refused logout leaves the session alone.
        let evil_logout = http_host(
            gport,
            host,
            "POST",
            "/logout",
            None,
            &format!("{cookie}\r\nOrigin: https://evil.k2.dev"),
        );
        assert_eq!(evil_logout.status, 403, "{}", evil_logout.body);
        let _still = read_thread();

        // Same origin: lands, and the daemon's ACAO: * does not come back.
        let direct = http(
            dport,
            "GET",
            &format!("/cli/skin/agents?token={OWNER_TOKEN}"),
            None,
        );
        assert!(
            direct
                .headers
                .to_ascii_lowercase()
                .contains("access-control-allow-origin"),
            "precondition: the daemon sends CORS headers: {}",
            direct.headers
        );
        let ok = http_host(
            gport,
            host,
            "POST",
            "/cli/thread/post",
            Some(&post_body),
            &format!("{cookie}\r\n{same}\r\nSec-Fetch-Site: same-origin"),
        );
        assert_eq!(ok.status, 200, "same-origin post; {}", ok.body);
        assert!(
            !ok.headers.to_ascii_lowercase().contains("access-control-"),
            "CORS headers stripped: {}",
            ok.headers
        );
        let posted_conv = json(&ok.body)["conversation_id"]
            .as_str()
            .expect("post conversation_id")
            .to_string();
        assert!(
            bodies(&posted_conv).iter().any(|b| b == &marker),
            "same-origin post lands in {posted_conv}"
        );
        let after = read_thread();
        assert!(
            !after.headers.to_ascii_lowercase().contains("access-control-"),
            "{}",
            after.headers
        );

        // Server-caller rule: no Origin and no Sec-Fetch-Site → allowed.
        let marker2 = format!("server-{}", uuid::Uuid::new_v4());
        let server = http_host(
            gport,
            host,
            "POST",
            "/cli/thread/post",
            Some(&format!(r#"{{"addr":"{handle}","text":"{marker2}"}}"#)),
            &cookie,
        );
        assert_eq!(server.status, 200, "server caller; {}", server.body);
        let server_conv = json(&server.body)["conversation_id"]
            .as_str()
            .expect("server conversation_id")
            .to_string();
        assert!(
            bodies(&server_conv).iter().any(|b| b == &marker2),
            "server-caller post lands in {server_conv}"
        );

        // Sockets: foreign Origin refused by the helper; own origin 101.
        // The address's current conversation (a skin post may have moved it).
        let ws_path = format!("/cli/overlay/events?conversation={server_conv}");
        let evil_ws = ws_upgrade(
            gport,
            &ws_path,
            &format!("{cookie}\r\nOrigin: https://evil.k2.dev"),
        );
        assert_eq!(evil_ws.status, 403, "{}", evil_ws.body);
        assert!(
            evil_ws.body.contains("cross_origin_refused"),
            "{}",
            evil_ws.body
        );
        let own_ws = ws_upgrade(
            gport,
            &ws_path,
            // `ws_upgrade` sends `Host: 127.0.0.1` (no port), so the
            // page's own origin is exactly `http://127.0.0.1`. That is a
            // plain-http request, so the sid travels under the unprefixed
            // name (S24-14: `__Host-` is only read when Secure).
            &format!("Cookie: k2_skin_ui={sid}\r\nOrigin: http://127.0.0.1"),
        );
        assert_eq!(
            own_ws.status, 101,
            "{}\n{}",
            own_ws.headers, own_ws.body
        );
        assert!(
            !own_ws.headers.to_ascii_lowercase().contains("access-control-"),
            "{}",
            own_ws.headers
        );

        // Same-origin logout clears with Secure and ends the session.
        let out = http_host(
            gport,
            host,
            "POST",
            "/logout",
            None,
            &format!("{cookie}\r\n{same}"),
        );
        assert_eq!(out.status, 200, "{}", out.body);
        assert_eq!(
            header_value(&out.headers, "set-cookie").expect("clear"),
            "__Host-k2_skin_ui=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        );
        let gone = http_host(
            gport,
            host,
            "GET",
            &format!("/cli/thread?addr={handle}"),
            None,
            &cookie,
        );
        assert_eq!(gone.status, 401, "{}", gone.body);

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

// ── prd-daemon-activity-and-thread-working-v1 S8: app guests ────────────

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A live session under `cwd` whose PTY runs `cat` (never an agent CLI).
/// Registering it in the v2 map creates its activity row (DA2), so the
/// 10 s liveness sweep keeps it. The spawn guard resolves every program
/// from `K2_TEST_AGENT_SHIM_DIR` only, so `cat` gets a shim there too.
fn spawn_cat(cwd: &str, key: &str) -> std::sync::Arc<k2_core::terminal::DaemonPtySession> {
    let shim_dir = std::env::var_os("K2_TEST_AGENT_SHIM_DIR").expect("with_temp_home sets the shim dir");
    let shim = std::path::Path::new(&shim_dir).join("cat");
    if !shim.exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&shim, "#!/bin/sh\nexec /bin/cat\n").expect("write cat shim");
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod cat shim");
    }
    let cfg = k2_core::terminal::DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(std::path::PathBuf::from(cwd)),
        program: Some("cat".to_string()),
        ..k2_core::terminal::DaemonPtyConfig::default()
    };
    let session = k2_core::terminal::DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    k2_daemon::v2_session_map::register(key.to_string(), std::sync::Arc::clone(&session));
    session
}

/// Apply a synthetic Claude hook payload to a session's row at `at`.
fn activity_hook(sid: &str, body: serde_json::Value, at: i64) {
    use k2_core::agent_hooks::envelope::{self, HookHeaders, HookSource};
    let env = envelope::parse(
        &HookHeaders {
            pane: sid.to_string(),
            agent_pid: Some(4242),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".into()),
            truncated: false,
            event_hint: None,
        },
        body.to_string().as_bytes(),
    )
    .expect("synthetic hook parses");
    k2_daemon::activity_store::apply_at(sid, k2_core::activity::Evidence::Hook(&env), at);
}

fn sorted_keys(v: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = v.as_object().unwrap_or_else(|| panic!("object: {v}")).keys().cloned().collect();
    keys.sort();
    keys
}

/// T-S8b: exactly the documented keys (a field added later fails here).
fn assert_guest_frame(v: &serde_json::Value, room: &str) {
    assert_eq!(
        sorted_keys(v),
        ["agentName", "counts", "kind", "paneGroupId", "serverNow", "since", "status", "workspace"],
        "guest frame keys: {v}"
    );
    assert_eq!(sorted_keys(&v["counts"]), ["commands", "subagents", "tools"], "guest counts keys: {v}");
    assert_eq!(v["kind"], "session_activity_changed", "{v}");
    assert_eq!(v["workspace"], room, "{v}");
}

/// Agent frames on a room socket until one for `agent` satisfies `pred`.
/// Every frame on the way must be a well-formed guest frame for `room`.
fn guest_frame_until(
    sock: &mut StdTcpStream,
    room: &str,
    agent: &str,
    what: &str,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline, "no guest frame: {what}");
        let Some(text) = try_read_ws_text(sock, Duration::from_millis(500)) else { continue };
        let v = json(&text);
        assert_guest_frame(&v, room);
        if v["agentName"] == agent && pred(&v) {
            return v;
        }
    }
}

/// Open any WebSocket on `port`; returns the stream after 101.
fn open_socket(port: u16, path_and_query: &str, extra_headers: &str) -> StdTcpStream {
    let mut sock = StdTcpStream::connect(("127.0.0.1", port)).expect("ws connect");
    sock.set_read_timeout(Some(Duration::from_secs(10))).expect("read timeout");
    let extra = if extra_headers.is_empty() { String::new() } else { format!("{extra_headers}\r\n") };
    let req = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    );
    sock.write_all(req.as_bytes()).expect("ws write");
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        sock.read_exact(&mut byte).unwrap_or_else(|e| panic!("upgrade head {path_and_query}: {e}; got {:?}", String::from_utf8_lossy(&head)));
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_string();
    assert!(head.starts_with("HTTP/1.1 101"), "{path_and_query}: {head}");
    sock
}

/// S8 through the official helper (AP1–AP4):
/// - T-S8a: an app guest never gets an `activity` overlay frame (the owner
///   on the same conversation does); `/cli/thread/activity` is not an app
///   door (helper 404, daemon 403); `/cli/chat/transcript` stays refused.
/// - T-S8b: the guest snapshot and the guest frames carry exactly the
///   documented keys and nothing from the row (no path, id, reason, tool,
///   count); `permission`, `unknown`, `since`, change-only sends, and a
///   removed working session going `idle`.
/// - T-S8c: `activity:read` in one room never reads another room
///   (`skin_room`), and a room without the cap is `missing capability`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_activity_guest_projection() {
    let _g = lock();
    k2_core::test_isolation::assert_no_prod_env();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let docs = format!("docs{tag}");
        let anna = format!("anna{tag}");
        let other = format!("othr{tag}");
        let (docs_id, _c1, docs_path) = seed_files_workspace(&docs);
        let (_anna_id, _c2, anna_path) = seed_files_workspace(&anna);
        let (_other_id, _c3, other_path) = seed_files_workspace(&other);
        add_user(dport, "watch");
        set_password(dport, "watch", "s3cret-horse");
        let created = http(dport, "POST", &format!("/cli/skin/roles?token={OWNER_TOKEN}"), Some(r#"{"name":"watcher"}"#));
        assert_eq!(created.status, 200, "role; {}", created.body);
        for (handle, caps) in [
            (&docs, r#"["thread:read","thread:post","activity:read"]"#),
            (&anna, r#"["thread:read"]"#),
        ] {
            let r = http(
                dport,
                "POST",
                &format!("/cli/skin/roles/room?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"name":"watcher","handle":"{handle}","caps":{caps}}}"#)),
            );
            assert_eq!(r.status, 200, "room {handle}; {}", r.body);
        }
        let assign = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"watch","role":"watcher"}"#),
        );
        assert_eq!(assign.status, 200, "assign; {}", assign.body);
        let gport = free_port();
        publish_skin(dport, &docs_path, gport, None);
        let cookie = gateway_login_cookie(gport, "watch", "s3cret-horse");

        // Live sessions (PTYs running `cat`): two in docs, one in anna.
        let work = spawn_cat(&docs_path, "tab-docswork");
        let quiet = spawn_cat(&docs_path, "tab-docsquiet");
        let elsewhere = spawn_cat(&anna_path, "tab-annawork");
        let sid = |s: &std::sync::Arc<k2_core::terminal::DaemonPtySession>| s.session_id.to_string();
        let (work_sid, quiet_sid, anna_sid) = (sid(&work), sid(&quiet), sid(&elsewhere));
        let started = now_ms();
        let prompt = |s: &str, at: i64| {
            activity_hook(s, serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt_id": "p1"}), at)
        };
        prompt(&work_sid, started);
        prompt(&anna_sid, started);

        // ── T-S8b: the guest snapshot ──
        let snap = http_ex(gport, "GET", &format!("/cli/activity/snapshot?workspace={docs}"), None, &cookie);
        assert_eq!(snap.status, 200, "guest snapshot through the helper; {}", snap.body);
        let v = json(&snap.body);
        assert_eq!(sorted_keys(&v), ["serverNow", "sessions", "since", "status", "workspace"], "{v}");
        assert_eq!((v["workspace"].as_str(), v["status"].as_str(), v["since"].as_i64()), (Some(docs.as_str()), Some("working"), Some(started)), "{v}");
        assert!(v["serverNow"].as_i64().is_some_and(|t| t >= started), "{v}");
        let sessions = v["sessions"].as_array().expect("sessions");
        assert_eq!(sessions.len(), 2, "docs sessions only: {v}");
        for s in sessions {
            assert_eq!(sorted_keys(s), ["agentName", "counts", "paneGroupId", "since", "status"], "{s}");
        }
        let zero = serde_json::json!({"subagents": 0, "tools": 0, "commands": 0});
        assert_eq!(
            sessions,
            &vec![
                serde_json::json!({"agentName": "tab-docsquiet", "paneGroupId": "docsquiet", "status": "idle", "since": null, "counts": zero}),
                serde_json::json!({"agentName": "tab-docswork", "paneGroupId": "docswork", "status": "working", "since": started, "counts": zero}),
            ],
            "{v}"
        );
        for secret in [docs_path.as_str(), docs_id.as_str(), work_sid.as_str(), "tab-annawork", "reason", "turnStartedAt", "children", "evidence"] {
            assert!(!snap.body.contains(secret), "{secret} must not reach an app: {}", snap.body);
        }

        // ── T-S8c: room isolation and the cap ──
        let no_cap = http_ex(gport, "GET", &format!("/cli/activity/snapshot?workspace={anna}"), None, &cookie);
        assert_eq!(no_cap.status, 403, "anna has no activity:read; {}", no_cap.body);
        assert!(no_cap.body.contains("missing capability activity:read"), "{}", no_cap.body);
        let not_mine = http_ex(gport, "GET", &format!("/cli/activity/snapshot?workspace={other}"), None, &cookie);
        assert_eq!(not_mine.status, 403, "room not on the pass; {}", not_mine.body);
        assert!(not_mine.body.contains("skin_room"), "{}", not_mine.body);
        assert!(!not_mine.body.contains("tab-"), "{}", not_mine.body);
        let no_ws = http_ex(gport, "GET", "/cli/activity/snapshot", None, &cookie);
        assert_eq!(no_ws.status, 400, "an app pass needs workspace=; {}", no_ws.body);
        let post = http_ex(gport, "POST", &format!("/cli/activity/snapshot?workspace={docs}"), Some("{}"), &cookie);
        assert_eq!(post.status, 404, "GET only on the helper; {}", post.body);
        // A platform pass with activity:read in anna only (daemon door).
        let anna_pass = mint(dport, &format!("act{tag}"), &["activity:read"], &[&anna]);
        let cross = http(dport, "GET", &format!("/cli/activity/snapshot?workspace={docs}&token={anna_pass}"), None);
        assert_eq!(cross.status, 403, "activity:read in anna never reads docs; {}", cross.body);
        assert!(cross.body.contains("skin_room"), "{}", cross.body);
        let own = http(dport, "GET", &format!("/cli/activity/snapshot?workspace={anna}&token={anna_pass}"), None);
        assert_eq!(own.status, 200, "{}", own.body);
        let ov = json(&own.body);
        assert_eq!(ov["sessions"].as_array().map(Vec::len), Some(1), "{ov}");
        assert_eq!(ov["sessions"][0]["agentName"], "tab-annawork", "{ov}");
        let all = http(dport, "GET", &format!("/cli/activity/snapshot?token={anna_pass}"), None);
        assert_eq!(all.status, 400, "never the host-wide snapshot; {}", all.body);
        let thread_pass = mint(dport, &format!("thr{tag}"), &["thread:read"], &[&anna]);
        let cap = http(dport, "GET", &format!("/cli/activity/snapshot?workspace={anna}&token={thread_pass}"), None);
        assert_eq!(cap.status, 403, "{}", cap.body);
        assert!(cap.body.contains("missing capability activity:read"), "{}", cap.body);

        // ── T-S8b: guest frames on the room socket ──
        let mut sock = open_room_socket(gport, &docs, &cookie);
        std::thread::sleep(Duration::from_millis(200));
        // A tool step: one counts frame (numbers only, no tool name or
        // command). Anna's turn is another room's: nothing.
        activity_hook(&work_sid, serde_json::json!({
            "hook_event_name": "PreToolUse", "prompt_id": "p1", "tool_name": "Bash",
            "tool_use_id": "toolu_s8", "tool_input": {"command": "cargo test"}
        }), now_ms());
        activity_hook(&anna_sid, serde_json::json!({"hook_event_name": "PreToolUse", "prompt_id": "p1", "tool_name": "Read", "tool_use_id": "toolu_a"}), now_ms());
        let text = try_read_ws_text(&mut sock, Duration::from_secs(5)).expect("a counts frame for the tool step");
        let f = json(&text);
        assert_guest_frame(&f, &docs);
        assert_eq!((f["agentName"].as_str(), f["status"].as_str(), f["since"].as_i64()), (Some("tab-docswork"), Some("working"), Some(started)), "{f}");
        assert_eq!(f["counts"], serde_json::json!({"subagents": 0, "tools": 1, "commands": 1}), "{f}");
        for secret in ["Bash", "cargo test", "toolu_s8", "tab-annawork"] {
            assert!(!text.contains(secret), "{secret} must not reach an app: {text}");
        }
        assert!(
            try_read_ws_text(&mut sock, Duration::from_millis(600)).is_none(),
            "another room's change sends nothing to the app"
        );
        // Waiting → permission (Q17), no since.
        activity_hook(&work_sid, serde_json::json!({"hook_event_name": "PermissionRequest", "prompt_id": "p1", "tool_name": "Bash"}), now_ms());
        let f = guest_frame_until(&mut sock, &docs, "tab-docswork", "permission", |v| v["status"] == "permission");
        assert!(f["since"].is_null(), "{f}");
        assert_eq!(f["paneGroupId"], "docswork");
        // Approved → working again, since = the same turn start.
        activity_hook(&work_sid, serde_json::json!({
            "hook_event_name": "PostToolUse", "prompt_id": "p1", "tool_name": "Bash",
            "tool_use_id": "toolu_s8", "tool_response": {"stdout": "synthetic"}
        }), now_ms());
        let f = guest_frame_until(&mut sock, &docs, "tab-docswork", "working again", |v| v["status"] == "working");
        assert_eq!(f["since"], started, "{f}");
        // Turn done with a background shell → monitoring: still `working`
        // for apps, but no turn running (since null).
        activity_hook(&work_sid, serde_json::json!({
            "hook_event_name": "Stop", "prompt_id": "p1", "session_crons": [],
            "background_tasks": [{"id": "bash_1", "type": "shell", "status": "running"}]
        }), now_ms());
        let f = guest_frame_until(&mut sock, &docs, "tab-docswork", "monitoring", |v| v["since"].is_null());
        assert_eq!(f["status"], "working", "monitoring is working for apps: {f}");
        // No evidence for 40 minutes → unverifiable → `unknown` (Q9).
        prompt(&quiet_sid, now_ms() - 40 * 60 * 1000);
        k2_daemon::activity_store::tick_all(now_ms());
        let f = guest_frame_until(&mut sock, &docs, "tab-docsquiet", "unknown", |v| v["status"] == "unknown");
        assert!(f["since"].is_null(), "{f}");
        // The working session goes away → one `idle`.
        k2_daemon::v2_session_map::unregister("tab-docswork");
        work.kill();
        let f = guest_frame_until(&mut sock, &docs, "tab-docswork", "removal", |v| v["status"] == "idle");
        assert!(f["since"].is_null(), "{f}");
        drop(sock);

        // ── T-S8a (1): activity overlay frames never reach an app guest ──
        let thread = http_ex(gport, "GET", &format!("/cli/thread?addr={docs}"), None, &cookie);
        assert_eq!(thread.status, 200, "{}", thread.body);
        let conv = json(&thread.body)["conversation_id"].as_str().expect("conversation_id").to_string();
        let mut guest_ov = open_socket(gport, &format!("/cli/overlay/events?conversation={conv}"), &cookie);
        let mut owner_ov = open_socket(dport, &format!("/cli/overlay/events?conversation={conv}&token={OWNER_TOKEN}"), "");
        std::thread::sleep(Duration::from_millis(200));
        // A strip frame on this conversation, through the tracker's own
        // broadcast (a Thread message alone no longer opens a turn, and
        // this room has no agent working).
        let turn = uuid::Uuid::new_v4().to_string();
        k2_daemon::overlay_ws::publish_activity(k2_daemon::overlay_ws::OverlayFrame {
            collection: "activity".into(),
            seq: 0,
            id: turn.clone(),
            doc: None,
            activity: Some(serde_json::json!({ "turnId": turn, "state": "working", "phase": "working", "startedAt": now_ms() })),
            conversation_id: Some(conv.clone()),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(std::time::Instant::now() < deadline, "the owner must see the activity frame");
            let Some(text) = try_read_ws_text(&mut owner_ov, Duration::from_millis(500)) else { continue };
            let v = json(&text);
            if v["collection"] == "activity" {
                assert_eq!(v["id"], turn.as_str(), "{v}");
                break;
            }
        }
        let ask = http(
            dport,
            "POST",
            &format!("/cli/thread/ask?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"addr":"{docs}","prompt":"Ship it?","options":"Go,Stop"}}"#)),
        );
        assert_eq!(ask.status, 200, "owner ask; {}", ask.body);
        let card = json(&ask.body)["id"].as_str().expect("card id").to_string();
        let mut saw_card = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let Some(text) = try_read_ws_text(&mut guest_ov, Duration::from_millis(500)) else {
                if saw_card {
                    break;
                }
                continue;
            };
            let v = json(&text);
            assert_ne!(v["collection"], "activity", "an app guest must never get an activity frame: {v}");
            assert!(v.get("activity").is_none(), "{v}");
            saw_card |= v["collection"] == "thread" && v["id"] == card.as_str();
        }
        assert!(saw_card, "the guest overlay socket was live on this conversation");
        k2_daemon::thread_activity::clear_for_tests();
        drop(guest_ov);
        drop(owner_ov);

        // ── T-S8a (2): the Thread strip catch-up is not an app door ──
        let gw = http_ex(gport, "GET", &format!("/cli/thread/activity?addr={docs}"), None, &cookie);
        assert_eq!(gw.status, 404, "never allowlisted; {}", gw.body);
        let gw_ws = ws_upgrade(gport, &format!("/cli/thread/activity?addr={docs}"), &cookie);
        assert_eq!(gw_ws.status, 403, "{}", gw_ws.body);
        assert!(gw_ws.body.contains("not allowed"), "{}", gw_ws.body);
        let docs_pass = mint(dport, &format!("dcs{tag}"), &["thread:read", "activity:read"], &[&docs]);
        let direct = http(dport, "GET", &format!("/cli/thread/activity?addr={docs}&token={docs_pass}"), None);
        assert_eq!(direct.status, 403, "app pass at the daemon; {}", direct.body);
        assert!(direct.body.contains("app passes cannot use thread/activity"), "{}", direct.body);

        // ── T-S8a (3): the chat transcript stays refused ──
        let gw = http_ex(gport, "GET", "/cli/chat/transcript?session=x", None, &cookie);
        assert_eq!(gw.status, 403, "{}", gw.body);
        assert!(gw.body.contains("not allowed"), "{}", gw.body);
        let gw_ws = ws_upgrade(gport, "/cli/chat/transcript?session=x", &cookie);
        assert_eq!(gw_ws.status, 403, "{}", gw_ws.body);
        assert!(gw_ws.body.contains("not allowed"), "{}", gw_ws.body);
        let direct = http(dport, "GET", &format!("/cli/chat/transcript?session=x&token={docs_pass}"), None);
        assert_eq!(direct.status, 403, "{}", direct.body);
        assert!(direct.body.contains("skin token cannot read host session logs"), "{}", direct.body);

        for key in ["tab-docsquiet", "tab-annawork"] {
            if let Some(s) = k2_daemon::v2_session_map::unregister(key) {
                s.kill();
            }
        }
        stop_skin(dport, &docs_path);
        for p in [&docs_path, &anna_path, &other_path] {
            let _ = std::fs::remove_dir_all(p);
        }
    });
}

// ── prd-app-frame-ancestors-v1: framing headers end to end ─────────────

/// Every `Content-Security-Policy` and `X-Frame-Options` value in a head.
fn frame_headers(headers: &str) -> (Vec<String>, Vec<String>) {
    let mut csp = Vec::new();
    let mut xfo = Vec::new();
    for line in headers.lines().skip(1) {
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.trim().eq_ignore_ascii_case("content-security-policy") {
            csp.push(v.trim().to_string());
        } else if k.trim().eq_ignore_ascii_case("x-frame-options") {
            xfo.push(v.trim().to_string());
        }
    }
    (csp, xfo)
}

fn assert_frame(r: &Resp, csp: &str, xfo: &str, what: &str) {
    let (c, x) = frame_headers(&r.headers);
    assert_eq!(c, vec![csp.to_string()], "{what}: exactly one CSP\n{}", r.headers);
    assert_eq!(x, vec![xfo.to_string()], "{what}: exactly one XFO\n{}", r.headers);
}

fn frame_post(dport: u16, path: &str, body_extra: &str) -> Resp {
    http(
        dport,
        "POST",
        &format!("/cli/publish/frame?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"agents","project":"{path}"{body_extra}}}"#)),
    )
}

fn first_service(dport: u16, path: &str) -> serde_json::Value {
    let listed = http(
        dport,
        "GET",
        &format!("/cli/publish/list?token={OWNER_TOKEN}&project={path}"),
        None,
    );
    assert_eq!(listed.status, 200, "{}", listed.body);
    json(&listed.body)["services"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .expect("one service")
}

/// §5 tests 2, 3 and 8 against a real daemon + real helper child:
/// default headers on the App and the daemon, `k2 publish frame` add /
/// `none` / reset with a live helper restart, bad origins, GET 405, and
/// the list JSON (FA4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_frame_policy_end_to_end() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgfa{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        const APP: &str = "frame-ancestors 'self' tauri://localhost http://tauri.localhost";

        // The daemon itself (Connect surfaces): 'self' on every response.
        let boot = http(dport, "GET", "/boot-status", None);
        assert_frame(&boot, "frame-ancestors 'self'", "SAMEORIGIN", "daemon /boot-status");
        let missing = http(dport, "GET", &format!("/cli/nope?token={OWNER_TOKEN}"), None);
        assert_frame(&missing, "frame-ancestors 'self'", "SAMEORIGIN", "daemon 404");

        let gport = free_port();
        publish_skin(dport, &path, gport, None);
        let svc = first_service(dport, &path);
        assert_eq!(svc["frameAncestors"], "", "{svc}");
        assert_eq!(svc["framePolicy"], "'self' tauri://localhost http://tauri.localhost", "{svc}");
        assert!(svc["frameWarning"].is_null(), "{svc}");

        for p in ["/", "/login", "/reset", "/nope"] {
            let r = http(gport, "GET", p, None);
            assert_frame(&r, APP, "SAMEORIGIN", &format!("helper GET {p}"));
        }
        let r = http(gport, "GET", "/cli/thread?addr=x", None);
        assert_eq!(r.status, 401, "{}", r.body);
        assert_frame(&r, APP, "SAMEORIGIN", "helper 401");
        let bad_login = http(gport, "POST", "/login", Some(r#"{"username":"x","password":"y"}"#));
        assert_eq!(bad_login.status, 401, "{}", bad_login.body);
        assert_frame(&bad_login, APP, "SAMEORIGIN", "helper login 401 (daemon reply re-framed)");

        // GET twin is 405; a GET never changes the policy.
        let get = http(
            dport,
            "GET",
            &format!("/cli/publish/frame?token={OWNER_TOKEN}&name=agents&project={path}&none=true"),
            None,
        );
        assert_eq!(get.status, 405, "{}", get.body);

        // Bad input: named codes, nothing stored, helper untouched.
        for (extra, code) in [
            (r#","allow":"https://*.k2.dev""#, "bad_frame_origin"),
            (r#","allow":"http://partner.example""#, "bad_frame_origin"),
            (r#","allow":"https://a.example/path""#, "bad_frame_origin"),
            (r#","allow":"https://a.example;x""#, "bad_frame_origin"),
            (r#","none":true,"reset":true"#, "usage"),
            ("", "usage"),
        ] {
            let r = frame_post(dport, &path, extra);
            assert_eq!(r.status, 400, "{extra}: {}", r.body);
            assert_eq!(json(&r.body)["error"]["code"], code, "{extra}: {}", r.body);
        }
        let nine: Vec<String> = (1..=9).map(|i| format!("https://o{i}.example")).collect();
        let r = frame_post(dport, &path, &format!(r#","allow":"{}""#, nine.join(" ")));
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(json(&r.body)["error"]["code"], "frame_too_many", "{}", r.body);
        assert_eq!(first_service(dport, &path)["frameAncestors"], "", "refusals store nothing");

        // Allow a partner: stored, helper restarted, new header live.
        let r = frame_post(dport, &path, r#","allow":"HTTPS://Partner.Example/""#);
        assert_eq!(r.status, 200, "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["restarted"], true, "{}", r.body);
        assert_eq!(v["service"]["frameAncestors"], "https://partner.example", "{}", r.body);
        assert!(wait_port(gport, 8000), "helper back on {gport}");
        let r = http(gport, "GET", "/login", None);
        assert_frame(
            &r,
            &format!("{APP} https://partner.example"),
            "SAMEORIGIN",
            "after --allow",
        );
        // Additive + dedup.
        let r = frame_post(dport, &path, r#","allow":"https://partner.example https://b.example""#);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(
            json(&r.body)["service"]["frameAncestors"],
            "https://partner.example https://b.example",
            "{}",
            r.body
        );
        assert!(wait_port(gport, 8000));

        // --none: DENY, K2's windows too.
        let r = frame_post(dport, &path, r#","none":true"#);
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(wait_port(gport, 8000));
        let r = http(gport, "GET", "/login", None);
        assert_frame(&r, "frame-ancestors 'none'", "DENY", "after --none");
        assert_eq!(first_service(dport, &path)["framePolicy"], "'none'");

        // Stopped App: stored, no restart, applied at the next start.
        stop_skin_only(dport, &path);
        let r = frame_post(dport, &path, r#","reset":true"#);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r.body)["restarted"], false, "{}", r.body);
        let start = http(
            dport,
            "POST",
            &format!("/cli/publish/start?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"agents","project":"{path}"}}"#)),
        );
        assert_eq!(start.status, 200, "{}", start.body);
        assert!(wait_port(gport, 8000));
        let r = http(gport, "GET", "/login", None);
        assert_frame(&r, APP, "SAMEORIGIN", "after --reset + start");

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

fn stop_skin_only(dport: u16, path: &str) {
    let r = http(
        dport,
        "POST",
        &format!("/cli/publish/stop?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"agents","project":"{path}"}}"#)),
    );
    assert_eq!(r.status, 200, "stop; {}", r.body);
}

/// FA4 / FA21: a `--cmd` App whose own `GET /` is HTML with no framing
/// header gets a `frameWarning` in the list; `k2 publish frame` refuses
/// it (K2 does not touch its responses). Needs `python3` (the same
/// stand-in server the other publish tests use).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_cmd_app_without_frame_header_is_warned() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgfw{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        std::fs::write(format!("{path}/index.html"), "<!doctype html><title>own</title>")
            .expect("index.html");
        let port = free_port();
        let run = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"agents","cmd":"python3 -m http.server {port} --bind 127.0.0.1","port":{port},"project":"{path}","noTunnel":true}}"#
            )),
        );
        assert_eq!(run.status, 200, "run cmd; {}", run.body);
        assert!(wait_port(port, 8000), "python http.server must listen on {port}");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let svc = loop {
            let svc = first_service(dport, &path);
            if !svc["frameWarning"].is_null() || std::time::Instant::now() > deadline {
                break svc;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(svc["kind"], "cmd", "{svc}");
        assert!(svc["framePolicy"].is_null(), "{svc}");
        let warn = svc["frameWarning"].as_str().expect("frameWarning set within 10 s");
        assert!(warn.contains("frame-ancestors"), "{warn}");
        assert!(warn.contains("k2 study publish-framing"), "{warn}");

        let r = frame_post(dport, &path, r#","none":true"#);
        assert_eq!(r.status, 400, "{}", r.body);
        assert!(r.body.contains("--skin Apps"), "{}", r.body);

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

/// Upgrade path: `--skin` helpers survive a daemon restart (setsid) and are
/// reattached at boot. One left over from a build without framing headers
/// must be restarted, or the fix would not reach a box until someone
/// restarts the App by hand. A stand-in "old helper" (python, no headers)
/// sits on the App's port with the row pointing at its pid; boot replaces
/// it with a real helper sending the policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_reattach_restarts_a_helper_without_frame_headers() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("psgfb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (project_id, _conv, path) = seed_workspace(&handle);
        let old_root = format!("{path}/old-helper");
        std::fs::create_dir_all(&old_root).expect("old root");
        std::fs::write(format!("{old_root}/login"), "<!doctype html><title>old</title>")
            .expect("old login");
        let gport = free_port();
        let mut old = std::process::Command::new("python3")
            .args(["-m", "http.server", &gport.to_string(), "--bind", "127.0.0.1", "--directory", &old_root])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("python3 stand-in old helper");
        assert!(wait_port(gport, 8000), "stand-in must listen on {gport}");
        let before = http(gport, "HEAD", "/login", None);
        assert_eq!(before.status, 200, "{}", before.headers);
        assert_eq!(frame_headers(&before.headers), (vec![], vec![]), "old helper sends no framing");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ps::insert(
                &conn,
                &project_id,
                "agents",
                CMD_SKIN_SENTINEL,
                &path,
                gport,
                ps::EXPOSE_LOCAL,
                DESIRED_RUNNING,
                KIND_SKIN,
                "",
            )
            .expect("row");
            ps::set_runtime(&conn, &project_id, "agents", Some(old.id() as i64), None, None)
                .expect("pid");
        }

        k2_daemon::publish_runtime::boot_desired_running();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while old.try_wait().expect("try_wait").is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(old.try_wait().expect("try_wait").is_some(), "old helper must be stopped");
        assert!(wait_port(gport, 8000), "new helper on {gport}");
        let after = http(gport, "GET", "/login", None);
        assert_eq!(after.status, 200, "{}", after.body);
        assert_frame(
            &after,
            "frame-ancestors 'self' tauri://localhost http://tauri.localhost",
            "SAMEORIGIN",
            "restarted helper",
        );
        let svc = first_service(dport, &path);
        assert_ne!(svc["pid"], serde_json::json!(old.id()), "{svc}");
        assert_eq!(svc["status"], "running", "{svc}");

        // A helper that already sends its policy is left alone.
        let pid_before = svc["pid"].clone();
        assert!(pid_before.is_i64(), "{svc}");
        k2_daemon::publish_runtime::boot_desired_running();
        let svc = first_service(dport, &path);
        assert_eq!(svc["pid"], pid_before, "no restart when the policy matches: {svc}");
        let again = http(gport, "GET", "/login", None);
        assert_eq!(again.status, 200, "{}", again.body);
        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}

fn thread_status(gport: u16, handle: &str, cookie: &str) -> Resp {
    http_ex(
        gport,
        "GET",
        &format!("/cli/thread?addr={handle}"),
        None,
        cookie,
    )
}

fn owner_post(dport: u16, route: &str, body: &str) -> Resp {
    let r = http(
        dport,
        "POST",
        &format!("{route}?token={OWNER_TOKEN}"),
        Some(body),
    );
    assert_eq!(r.status, 200, "{route} {body}; {}", r.body);
    r
}

fn host_grant_id(dport: u16, username: &str) -> String {
    let users = http(
        dport,
        "GET",
        &format!("/cli/skin/users?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(users.status, 200, "{}", users.body);
    let uid = json(&users.body)["users"]
        .as_array()
        .and_then(|rows| rows.iter().find(|u| u["username"] == username))
        .and_then(|u| u["id"].as_str())
        .unwrap_or_else(|| panic!("{username} id in {}", users.body))
        .to_string();
    let grants = http(
        dport,
        "GET",
        &format!("/cli/skin/grants?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(grants.status, 200, "{}", grants.body);
    json(&grants.body)["grants"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|g| {
                g["subjectId"] == uid.as_str()
                    && g["kind"] == "app"
                    && g["targetId"] == k2_core::skin::HOST_APP_ID
            })
        })
        .and_then(|g| g["id"].as_str())
        .unwrap_or_else(|| panic!("{username} host grant in {}", grants.body))
        .to_string()
}

fn principal_id(dport: u16, username: &str) -> String {
    let users = http(
        dport,
        "GET",
        &format!("/cli/skin/users?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(users.status, 200, "{}", users.body);
    json(&users.body)["users"]
        .as_array()
        .and_then(|rows| rows.iter().find(|u| u["username"] == username))
        .and_then(|u| u["id"].as_str())
        .unwrap_or_else(|| panic!("{username} id in {}", users.body))
        .to_string()
}

/// The grant id of `username`'s `kind=app` grant for App `app`.
fn app_grant_id(dport: u16, username: &str, app: &str) -> String {
    let uid = principal_id(dport, username);
    let grants = http(
        dport,
        "GET",
        &format!("/cli/skin/grants?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(grants.status, 200, "{}", grants.body);
    json(&grants.body)["grants"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|g| g["subjectId"] == uid.as_str() && g["kind"] == "app" && g["targetId"] == app)
        })
        .and_then(|g| g["id"].as_str())
        .unwrap_or_else(|| panic!("{username} grant for {app} in {}", grants.body))
        .to_string()
}

fn gateway_login_status(gport: u16, username: &str, password: &str) -> u16 {
    http(
        gport,
        "POST",
        "/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    )
    .status
}

/// Access-model D3: a grant, principal, role or password change must end
/// (or narrow) the guest's live gateway session right away, not after the
/// 7-day session TTL. Another guest's session is never touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_gateway_access_changes_end_live_sessions() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("revgw{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("revgx{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        let (_id2, _conv2, other_path) = seed_workspace(&other);
        for u in ["ada", "bob", "cara"] {
            add_user(dport, u);
            set_rooms(dport, u, &handle);
            set_password(dport, u, "s3cret-horse");
        }
        let gport = free_port();
        let app = publish_skin(dport, &path, gport, None);

        let bob = gateway_login_cookie(gport, "bob", "s3cret-horse");
        let bob_ok = |label: &str| {
            let r = thread_status(gport, &handle, &bob);
            assert_eq!(
                r.status, 200,
                "unrelated guest bob after {label}; {}",
                r.body
            );
        };
        bob_ok("login");

        // 1. App grant disabled: refused at the gateway and at re-login.
        //    S24-6: disabling the BOX login grant leaves the App session.
        let ada = gateway_login_cookie(gport, "ada", "s3cret-horse");
        assert_eq!(thread_status(gport, &handle, &ada).status, 200);
        let host_id = host_grant_id(dport, "ada");
        owner_post(
            dport,
            "/cli/skin/grants/enabled",
            &format!(r#"{{"id":"{host_id}","enabled":false}}"#),
        );
        assert_eq!(
            thread_status(gport, &handle, &ada).status,
            200,
            "box-login grant off must not end the App session"
        );
        owner_post(
            dport,
            "/cli/skin/grants/enabled",
            &format!(r#"{{"id":"{host_id}","enabled":true}}"#),
        );
        let app_id = app_grant_id(dport, "ada", &app);
        owner_post(
            dport,
            "/cli/skin/grants/enabled",
            &format!(r#"{{"id":"{app_id}","enabled":false}}"#),
        );
        let r = thread_status(gport, &handle, &ada);
        assert_eq!(
            r.status, 401,
            "disabled App grant must end the session; {}",
            r.body
        );
        assert_eq!(
            gateway_login_status(gport, "ada", "s3cret-horse"),
            401,
            "disabled App grant must refuse a fresh login"
        );
        bob_ok("grant disable");
        owner_post(
            dport,
            "/cli/skin/grants/enabled",
            &format!(r#"{{"id":"{app_id}","enabled":true}}"#),
        );
        let r = thread_status(gport, &handle, &ada);
        assert_eq!(
            r.status, 401,
            "re-enable must not revive the old pass; {}",
            r.body
        );

        // 2. Box login switched off (the per-guest off switch) ends App
        //    sessions too (Q8 default).
        let ada = gateway_login_cookie(gport, "ada", "s3cret-horse");
        assert_eq!(thread_status(gport, &handle, &ada).status, 200);
        owner_post(
            dport,
            "/cli/skin/grants/host",
            r#"{"username":"ada","present":false}"#,
        );
        let r = thread_status(gport, &handle, &ada);
        assert_eq!(
            r.status, 401,
            "host switch off must end the session; {}",
            r.body
        );
        bob_ok("host switch off");
        owner_post(
            dport,
            "/cli/skin/grants/host",
            r#"{"username":"ada","present":true}"#,
        );

        // 3. Role change on the App grant: a removed room leaves the live
        //    session (the rewrite follows the grant's role, S24-6).
        owner_post(
            dport,
            "/cli/skin/roles",
            &format!(
                r#"{{"name":"front","roomAccess":[{{"handle":"{other}","caps":["thread:read"]}}]}}"#
            ),
        );
        let ada_uid = principal_id(dport, "ada");
        owner_post(
            dport,
            "/cli/skin/grants/delete",
            &format!(r#"{{"id":"{}"}}"#, app_grant_id(dport, "ada", &app)),
        );
        owner_post(
            dport,
            "/cli/skin/grants",
            &format!(
                r#"{{"subjectKind":"principal","subjectId":"{ada_uid}","kind":"app","targetId":"{app}","roleId":"front"}}"#
            ),
        );
        let ada = gateway_login_cookie(gport, "ada", "s3cret-horse");
        let r = thread_status(gport, &other, &ada);
        assert_eq!(r.status, 200, "role room before removal; {}", r.body);
        assert_eq!(
            thread_status(gport, &handle, &ada).status,
            403,
            "the App role, not the box-login rooms"
        );
        owner_post(
            dport,
            "/cli/skin/roles/room",
            &format!(r#"{{"role":"front","handle":"{other}","clear":true}}"#),
        );
        let r = thread_status(gport, &other, &ada);
        assert_eq!(
            r.status, 403,
            "removed role room must leave the session; {}",
            r.body
        );
        assert!(r.body.contains("skin_room"), "{}", r.body);
        bob_ok("role room removal");

        // 4. Role delete: refused while a grant uses it; after the App grant
        //    goes to plain rooms the role can go.
        owner_post(
            dport,
            "/cli/skin/roles/room",
            &format!(r#"{{"role":"front","handle":"{other}","caps":["thread:read"]}}"#),
        );
        assert_eq!(
            thread_status(gport, &other, &ada).status,
            200,
            "room back on the role"
        );
        let refused = http(
            dport,
            "POST",
            &format!("/cli/skin/roles/remove?token={OWNER_TOKEN}"),
            Some(r#"{"name":"front"}"#),
        );
        assert_eq!(
            refused.status, 400,
            "a role a grant uses cannot go; {}",
            refused.body
        );
        owner_post(
            dport,
            "/cli/skin/grants/delete",
            &format!(r#"{{"id":"{}"}}"#, app_grant_id(dport, "ada", &app)),
        );
        assert_eq!(
            thread_status(gport, &other, &ada).status,
            401,
            "deleting the App grant ends that App's session"
        );
        owner_post(
            dport,
            "/cli/skin/users/rooms",
            &format!(r#"{{"username":"ada","app":"{app}","rooms":["{handle}"]}}"#),
        );
        owner_post(dport, "/cli/skin/roles/remove", r#"{"name":"front"}"#);
        let ada = gateway_login_cookie(gport, "ada", "s3cret-horse");
        assert_eq!(
            thread_status(gport, &other, &ada).status,
            403,
            "deleted role's room must be gone"
        );
        assert_eq!(
            thread_status(gport, &handle, &ada).status,
            200,
            "the App grant's own rooms apply"
        );
        bob_ok("role delete");

        // 5. Owner password set (`k2 app user password`) ends the sessions.
        let cara = gateway_login_cookie(gport, "cara", "s3cret-horse");
        assert_eq!(thread_status(gport, &handle, &cara).status, 200);
        set_password(dport, "cara", "fresh-horse-pw");
        let r = thread_status(gport, &handle, &cara);
        assert_eq!(
            r.status, 401,
            "password set must end the session; {}",
            r.body
        );
        bob_ok("password set");

        // 6. Principal removed.
        let r = thread_status(gport, &handle, &ada);
        assert_eq!(r.status, 200, "ada live before remove; {}", r.body);
        owner_post(dport, "/cli/skin/users/remove", r#"{"username":"ada"}"#);
        let r = thread_status(gport, &handle, &ada);
        assert_eq!(
            r.status, 401,
            "removed principal must end the session; {}",
            r.body
        );
        assert_eq!(gateway_login_status(gport, "ada", "s3cret-horse"), 401);
        bob_ok("principal remove");

        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_dir_all(&other_path);
    });
}

/// Publish a second `--skin` App on another name; returns its service id.
fn publish_named_skin(dport: u16, path: &str, name: &str, gport: u16) -> String {
    let run = http(
        dport,
        "POST",
        &format!("/cli/publish/run?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"name":"{name}","skin":true,"port":{gport},"project":"{path}","noTunnel":true}}"#
        )),
    );
    assert_eq!(run.status, 200, "run skin {name}; {}", run.body);
    assert!(wait_port(gport, 8000), "{name} helper must listen on {gport}");
    json(&run.body)["id"]
        .as_str()
        .expect("published service id")
        .to_string()
}

fn daemon_login(dport: u16, body: &str) -> Resp {
    http(dport, "POST", "/cli/skin/login", Some(body))
}

/// S24-17 daemon integration: per-App logins on one daemon.
/// - a guest granted only App A gets the generic 401 at B's `/login`, and
///   B's helper never forwards a browser-supplied `app`;
/// - BFF path: `POST /cli/skin/login` with no `app` is the box login exactly
///   as before; with `app` it needs that App's grant; an unknown App is the
///   same 401 body as a bad password;
/// - `users add --app` creates no box login;
/// - `publish rm` ends the sessions bound to that App (S24-20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_run_skin_per_app_logins_fail_closed() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("s24a{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        let gport_a = free_port();
        let gport_b = free_port();
        let app_a = publish_named_skin(dport, &path, "alpha", gport_a);
        let app_b = publish_named_skin(dport, &path, "beta", gport_b);
        assert_ne!(app_a, app_b);

        // An App-only guest for A (owner): no box login.
        let r = http(
            dport,
            "POST",
            &format!("/cli/skin/users?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"aonly","app":"{handle}/alpha"}}"#)),
        );
        assert_eq!(r.status, 200, "users add --app; {}", r.body);
        set_password(dport, "aonly", "s3cret-horse");
        owner_post(
            dport,
            "/cli/skin/users/rooms",
            &format!(r#"{{"username":"aonly","app":"{app_a}","rooms":["{handle}"]}}"#),
        );
        let grants = json(
            &http(dport, "GET", &format!("/cli/skin/grants?token={OWNER_TOKEN}"), None).body,
        );
        let uid = principal_id(dport, "aonly");
        let mine: Vec<&serde_json::Value> = grants["grants"]
            .as_array()
            .expect("grants")
            .iter()
            .filter(|g| g["subjectId"] == uid.as_str())
            .collect();
        assert_eq!(mine.len(), 1, "only the App A grant: {mine:?}");
        assert_eq!(mine[0]["targetId"], app_a.as_str());
        assert_eq!(mine[0]["createdBy"], "owner-token", "{mine:?}");

        // Gateway A: ok. Gateway B: generic 401, even naming A in the body.
        let a_cookie = gateway_login_cookie(gport_a, "aonly", "s3cret-horse");
        assert_eq!(thread_status(gport_a, &handle, &a_cookie).status, 200);
        let bad_pw = http(
            gport_b,
            "POST",
            "/login",
            Some(r#"{"username":"aonly","password":"wrong-horse"}"#),
        );
        assert_eq!(bad_pw.status, 401);
        for body in [
            r#"{"username":"aonly","password":"s3cret-horse"}"#.to_string(),
            format!(r#"{{"username":"aonly","password":"s3cret-horse","app":"{app_a}"}}"#),
        ] {
            let r = http(gport_b, "POST", "/login", Some(&body));
            assert_eq!(r.status, 401, "cross-App at B: {body} → {}", r.body);
            assert_eq!(r.body, bad_pw.body, "same body as a bad password");
            assert!(header_value(&r.headers, "set-cookie").is_none());
        }
        // A's cookie is per helper: it opens nothing at B.
        assert_eq!(thread_status(gport_b, &handle, &a_cookie).status, 401);

        // Daemon door (BFF): no app = box login → refused (no host grant).
        let generic = daemon_login(dport, r#"{"username":"aonly","password":"wrong-horse"}"#);
        assert_eq!(generic.status, 401);
        let host = daemon_login(dport, r#"{"username":"aonly","password":"s3cret-horse"}"#);
        assert_eq!(host.status, 401, "no box login: {}", host.body);
        assert_eq!(host.body, generic.body);
        for app in [app_b.as_str(), "no-such-app", "host"] {
            let r = daemon_login(
                dport,
                &format!(r#"{{"username":"aonly","password":"s3cret-horse","app":"{app}"}}"#),
            );
            assert_eq!(r.status, 401, "{app}: {}", r.body);
            assert_eq!(r.body, generic.body, "{app}: no App enumeration");
        }
        let ok = daemon_login(
            dport,
            &format!(r#"{{"username":"aonly","password":"s3cret-horse","app":"{app_a}"}}"#),
        );
        assert_eq!(ok.status, 200, "{}", ok.body);
        let ov = json(&ok.body);
        assert_eq!(ov["app"], app_a.as_str(), "{}", ok.body);
        let a_tok = ov["token"].as_str().expect("token").to_string();
        // A box-login guest: the daemon door without app is unchanged.
        add_user(dport, "boxer");
        set_rooms(dport, "boxer", &handle);
        set_password(dport, "boxer", "s3cret-horse");
        let r = daemon_login(dport, r#"{"username":"boxer","password":"s3cret-horse"}"#);
        assert_eq!(r.status, 200, "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["app"], "host");
        assert_eq!(v["roomHandles"], serde_json::json!([handle.clone()]), "{v}");
        let boxer_tok = v["token"].as_str().expect("token").to_string();
        // ...and can't reach App B until the owner grants it.
        let r = daemon_login(
            dport,
            &format!(r#"{{"username":"boxer","password":"s3cret-horse","app":"{app_b}"}}"#),
        );
        assert_eq!(r.status, 401, "{}", r.body);
        let copied = copy_host(dport, &format!("{handle}/beta"));
        assert_eq!(copied["added"], serde_json::json!(["boxer"]), "{copied}");
        assert_eq!(gateway_login_status(gport_b, "boxer", "s3cret-horse"), 200);

        // Agents keep their 0.45.4 powers (Q5): no `app` from a passport is
        // covered by unit tests; grants/copy-host is owner-only here too.
        let r = http(
            dport,
            "POST",
            "/cli/skin/grants/copy-host",
            Some(&format!(r#"{{"app":"{app_b}"}}"#)),
        );
        assert!(r.status == 401 || r.status == 403, "copy-host without owner: {}", r.status);
        let r = http(
            dport,
            "GET",
            &format!("/cli/skin/grants/copy-host?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 405, "copy-host is POST-only; {}", r.body);

        // S24-20: removing App A ends its sessions; the box session stays.
        let thread_tok = |tok: &str| {
            http(
                dport,
                "GET",
                &format!("/cli/thread?addr={handle}&token={tok}"),
                None,
            )
            .status
        };
        assert_eq!(thread_tok(&a_tok), 200);
        assert_eq!(thread_tok(&boxer_tok), 200);
        let rm = http(
            dport,
            "POST",
            &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"alpha","project":"{path}"}}"#)),
        );
        assert_eq!(rm.status, 200, "{}", rm.body);
        assert_eq!(thread_tok(&a_tok), 401, "App A's pass ends with the App");
        assert_eq!(thread_tok(&boxer_tok), 200, "box pass unaffected");
        let r = daemon_login(
            dport,
            &format!(r#"{{"username":"aonly","password":"s3cret-horse","app":"{app_a}"}}"#),
        );
        assert_eq!(r.status, 401, "a removed App's leftover grant mints nothing");
        assert!(
            json(&http(dport, "GET", &format!("/cli/skin/grants?token={OWNER_TOKEN}"), None).body)
                ["grants"]
                .as_array()
                .expect("grants")
                .iter()
                .any(|g| g["targetId"] == app_a.as_str()),
            "grants stay after publish rm"
        );

        let _ = http(
            dport,
            "POST",
            &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"beta","project":"{path}"}}"#)),
        );
        let _ = std::fs::remove_dir_all(&path);
    });
}

/// S24-9: a published `--cmd` service learns its App id and the daemon's
/// loopback address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_cmd_service_gets_app_id_env() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("s24c{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        let port = free_port();
        let cmd = format!(
            "printf '%s|%s|%s' \"$K2_APP_ID\" \"$K2_HOST\" \"$K2_PORT\" > app_env.txt; exec python3 -m http.server {port} --bind 127.0.0.1"
        );
        let body = serde_json::json!({
            "name": "bff",
            "cmd": cmd,
            "port": port,
            "project": path,
            "noTunnel": true,
        })
        .to_string();
        let run = http(
            dport,
            "POST",
            &format!("/cli/publish/run?token={OWNER_TOKEN}"),
            Some(&body),
        );
        assert_eq!(run.status, 200, "run cmd; {}", run.body);
        let id = json(&run.body)["id"].as_str().expect("id").to_string();
        assert!(wait_port(port, 8000), "cmd service must listen");
        let got = std::fs::read_to_string(format!("{path}/app_env.txt")).expect("env file");
        assert_eq!(got, format!("{id}|127.0.0.1|{dport}"));
        let _ = http(
            dport,
            "POST",
            &format!("/cli/publish/rm?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"name":"bff","project":"{path}"}}"#)),
        );
        let _ = std::fs::remove_dir_all(&path);
    });
}

/// nsi BFF asks (2026-10-13), so a BFF never reads skin.db:
/// (a) `POST /cli/skin/login` takes an email in place of the username,
///     with and without `app`; an unknown email is the generic 401;
/// (b) a locked login is `423 {"error":"locked","retryAfterSecs":N}` with
///     `Retry-After`, for a known OR unknown name alike (the lock counts
///     per presented string, so it is no username oracle), on the daemon
///     door and through a `--skin` helper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_login_email_and_locked_reply() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let dport = daemon.port;
        let handle = format!("s24l{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_id, _conv, path) = seed_workspace(&handle);
        add_user(dport, "jane");
        set_rooms(dport, "jane", &handle);
        set_password(dport, "jane", "s3cret-horse");
        owner_post(
            dport,
            "/cli/skin/users/email",
            r#"{"username":"jane","email":"Jane@Clinic.example"}"#,
        );
        let gport = free_port();
        let app = publish_skin(dport, &path, gport, None);

        // (a) email in place of the username, case-insensitive.
        let r = daemon_login(dport, r#"{"username":"jane@clinic.EXAMPLE","password":"s3cret-horse"}"#);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r.body)["username"], "jane");
        assert_eq!(json(&r.body)["app"], "host");
        let r = daemon_login(
            dport,
            &format!(r#"{{"username":"jane@clinic.example","password":"s3cret-horse","app":"{app}"}}"#),
        );
        assert_eq!(r.status, 200, "email + app; {}", r.body);
        assert_eq!(json(&r.body)["app"], app.as_str());
        let generic = daemon_login(dport, r#"{"username":"jane","password":"wrong-horse"}"#);
        assert_eq!(generic.status, 401);
        let unknown = daemon_login(dport, r#"{"username":"who@nowhere.example","password":"s3cret-horse"}"#);
        assert_eq!(unknown.status, 401);
        assert_eq!(unknown.body, generic.body, "unknown email = generic 401");

        // (b) lockout: two more bad tries for jane (3 in all), then 423.
        for _ in 0..2 {
            assert_eq!(
                daemon_login(dport, r#"{"username":"jane","password":"wrong-horse"}"#).status,
                401
            );
        }
        let locked = daemon_login(dport, r#"{"username":"jane","password":"s3cret-horse"}"#);
        assert_eq!(locked.status, 423, "{}", locked.body);
        let lv = json(&locked.body);
        assert_eq!(lv["error"], "locked", "{}", locked.body);
        let secs = lv["retryAfterSecs"].as_i64().expect("retryAfterSecs");
        assert!((1..=900).contains(&secs), "{secs}");
        let ra: i64 = header_value(&locked.headers, "retry-after")
            .expect("Retry-After")
            .parse()
            .expect("Retry-After seconds");
        assert_eq!(ra, secs);
        assert!(!locked.body.contains("k2skn_"));
        // With an App named, still locked (the lock is on the person).
        let r = daemon_login(
            dport,
            &format!(r#"{{"username":"jane","password":"s3cret-horse","app":"{app}"}}"#),
        );
        assert_eq!(r.status, 423, "{}", r.body);
        // Through the --skin helper the browser sees the same 423.
        let r = http(
            gport,
            "POST",
            "/login",
            Some(r#"{"username":"jane","password":"s3cret-horse"}"#),
        );
        assert_eq!(r.status, 423, "gateway; {}", r.body);
        assert_eq!(json(&r.body)["error"], "locked");
        assert!(header_value(&r.headers, "set-cookie").is_none());

        // An unknown name locks the same way: "locked" says nothing about
        // whether a username exists.
        for _ in 0..3 {
            assert_eq!(
                daemon_login(dport, r#"{"username":"ghost","password":"x-wrong-1"}"#).status,
                401
            );
        }
        let ghost = daemon_login(dport, r#"{"username":"ghost","password":"x-wrong-1"}"#);
        assert_eq!(ghost.status, 423, "{}", ghost.body);
        assert_eq!(json(&ghost.body)["error"], "locked");

        // Owner unlock clears it.
        owner_post(dport, "/cli/skin/users/unlock", r#"{"username":"jane"}"#);
        assert_eq!(
            daemon_login(dport, r#"{"username":"jane","password":"s3cret-horse"}"#).status,
            200
        );
        stop_skin(dport, &path);
        let _ = std::fs::remove_dir_all(&path);
    });
}
