//! Home avatar cache routes, headless (prd-home-picker-and-remote-avatars-v1
//! T3.5, vs-live P30).
//!
//! 1. Through the REAL dispatcher (`k2_daemon::test_harness`) under a temp
//!    HOME: GET on `put` is 405 and the same keep-alive socket answers the
//!    next request; a 300 KiB put is 413 and the socket closes; a Connect
//!    login of every role gets 403 `role_required` (NoLogin); an agent
//!    passport and no token get 403 `home_avatars_local_only`; the owner's
//!    put → GET round trip returns the same data URL; 101 addresses is 400
//!    `too_many`; prune removes an unlisted entry.
//! 2. The REAL `k2-daemon` binary under a temp HOME with no client attached:
//!    `/boot-status` lists `home-avatars-v1`, and a put lands in
//!    `$HOME/.k2/cache/agent-avatars/<server>/<workspace>.png`.
//!
//! Never touches the real `~/.k2`.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use k2_core::connect_users;
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::Value as J;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-home-avatars";

/// One HTTP/1.1 keep-alive connection; one Content-Length response per request.
struct Conn {
    reader: BufReader<TcpStream>,
}

impl Conn {
    fn try_open(port: u16) -> std::io::Result<Conn> {
        let s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(15)))?;
        Ok(Conn { reader: BufReader::new(s) })
    }

    fn open(port: u16) -> Conn {
        Conn::try_open(port).expect("connect daemon")
    }

    fn send(&mut self, method: &str, path: &str, body: Option<&str>) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request");
    }

    /// Send an oversize request: the daemon answers 413 after the head and
    /// may close before the whole body is written, so a write error here is
    /// expected and ignored. The response is still read and asserted.
    fn send_oversize(&mut self, method: &str, path: &str, body: &str) {
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request head");
        let _ = self.reader.get_mut().write_all(body.as_bytes());
    }

    fn read_response(&mut self, what: &str) -> (u16, String) {
        let mut status_line = String::new();
        self.reader.read_line(&mut status_line).expect("read status line");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_else(|| panic!("{what}: no status in {status_line:?}"))
            .parse()
            .unwrap_or_else(|_| panic!("{what}: bad status in {status_line:?}"));
        let mut len: Option<usize> = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("read header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = Some(v.trim().parse().expect("content-length is a number"));
                }
            }
        }
        let len = len.unwrap_or_else(|| panic!("{what}: response has no Content-Length"));
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("read body");
        (status, String::from_utf8(buf).expect("utf-8 body"))
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        self.send(method, path, body);
        self.read_response(path)
    }

    /// True when the server closed the socket (EOF) instead of answering.
    fn closed(&mut self) -> bool {
        let mut b = [0u8; 1];
        loop {
            match self.reader.read(&mut b) {
                Ok(0) => return true,
                Ok(_) => return false,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    return true
                }
                Err(e) => panic!("waiting for close: {e:?}"),
            }
        }
    }
}

fn json(body: &str, what: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn call(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, J) {
    let (s, b) = Conn::open(port).request(method, path, body);
    (s, json(&b, path))
}

fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!(
        "k2-home-avatars-headless-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();

    f(&tmp);

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&tmp).expect("remove temp HOME");
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn png_url(seed: u8, len: usize) -> (Vec<u8>, String) {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend(std::iter::repeat(seed).take(len.saturating_sub(8)));
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&b));
    (b, url)
}

fn put_body(address: &str, data_url: Option<&str>) -> String {
    serde_json::json!({ "address": address, "dataUrl": data_url }).to_string()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn mint_passport() -> String {
    let sid = SessionId::new();
    session_token::mint_session_token(
        &sid,
        "pane-home-avatars",
        HookPrincipal {
            workspace_uuid: "ws-home-avatars".to_string(),
            agent_address: "agent-home-avatars".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(username: &str, role: Option<connect_users::Role>) -> String {
    connect_users::add_user(username, "password123").unwrap_or_else(|e| panic!("add_user {username}: {e:?}"));
    if let Some(r) = role {
        connect_users::set_role(username, r).unwrap_or_else(|e| panic!("set_role {username}: {e:?}"));
    }
    connect_users::create_session(username)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_5_routes_gate_cap_keep_alive_and_round_trip() {
    let _g = lock();
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let tok = OWNER_TOKEN;
        let cache_root = home.join(".k2").join("cache").join("agent-avatars");
        let addr = "scout::dtl.k2.dev";
        let (bytes, url) = png_url(5, 500);

        // GET on each POST row → 405, and the same socket answers the next request.
        for p in ["/cli/home/avatars/put", "/cli/home/avatars/prune"] {
            let mut c = Conn::open(port);
            let (s, b) = c.request("GET", &format!("{p}?token={tok}"), None);
            assert_eq!(s, 405, "GET {p} must be 405: {b}");
            let (s, b) = c.request("GET", &format!("/cli/home/avatars?token={tok}&addresses={}", enc(addr)), None);
            assert_eq!(s, 200, "the request after the 405 on {p} must be answered: {b}");
            assert_eq!(json(&b, "get")["avatars"], serde_json::json!({}), "nothing cached yet: {b}");
        }
        // POST on the GET row → 405.
        let (s, b) = Conn::open(port).request("POST", &format!("/cli/home/avatars?token={tok}"), Some("{}"));
        assert_eq!(s, 405, "POST on the GET row: {b}");

        // A 300 KiB put → 413, and the socket closes (the rest of the body is never parsed).
        let big = put_body(addr, Some(&format!("data:image/png;base64,{}", "A".repeat(300 * 1024))));
        let mut c = Conn::open(port);
        c.send_oversize("POST", &format!("/cli/home/avatars/put?token={tok}"), &big);
        let (s, b) = c.read_response("oversize put");
        assert_eq!(s, 413, "oversize put: {b}");
        assert_eq!(json(&b, "413")["error"], "body_too_large", "{b}");
        assert!(c.closed(), "the daemon must close the socket after a 413");
        let big_prune = format!(r#"{{"keep":["{}"]}}"#, "x".repeat(70 * 1024));
        let mut c = Conn::open(port);
        c.send_oversize("POST", &format!("/cli/home/avatars/prune?token={tok}"), &big_prune);
        let (s, b) = c.read_response("oversize prune");
        assert_eq!(s, 413, "oversize prune (64 KiB cap): {b}");
        assert!(!cache_root.exists(), "nothing was written by refused requests");

        // Connect logins of every role → 403 role_required (NoLogin), before the handler.
        for (user, role) in [
            ("hamember", None),
            ("haadmin", Some(connect_users::Role::Admin)),
            ("haowner", Some(connect_users::Role::Owner)),
        ] {
            let session = login(user, role);
            for (m, p, body) in [
                ("GET", format!("/cli/home/avatars?addresses={}", enc(addr)), None),
                ("POST", "/cli/home/avatars/put".to_string(), Some(put_body(addr, Some(&url)))),
                ("POST", "/cli/home/avatars/prune".to_string(), Some(r#"{"keep":[]}"#.to_string())),
            ] {
                let sep = if p.contains('?') { '&' } else { '?' };
                let (s, v) = call(port, m, &format!("{p}{sep}token={session}"), body.as_deref());
                assert_eq!(s, 403, "{user} {m} {p} must be refused: {v}");
                assert_eq!(v["error"], "role_required", "{user} {p}: {v}");
            }
        }
        // An agent passport passes role_gate and is refused by the handler.
        let passport = mint_passport();
        assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
        for (m, p, body) in [
            ("GET", format!("/cli/home/avatars?addresses={}", enc(addr)), None),
            ("POST", "/cli/home/avatars/put".to_string(), Some(put_body(addr, Some(&url)))),
            ("POST", "/cli/home/avatars/prune".to_string(), Some(r#"{"keep":[]}"#.to_string())),
        ] {
            let sep = if p.contains('?') { '&' } else { '?' };
            let (s, v) = call(port, m, &format!("{p}{sep}token={passport}"), body.as_deref());
            assert_eq!(s, 403, "passport {m} {p} must be refused: {v}");
            assert_eq!(v["error"], "home_avatars_local_only", "passport {p}: {v}");
            // No token at all is refused too.
            let (s, v) = call(port, m, &p, body.as_deref());
            assert_eq!(s, 403, "no-token {m} {p} must be refused: {v}");
        }
        assert!(!cache_root.exists(), "refused callers wrote nothing");

        // Owner put → GET round trip, on one keep-alive socket.
        let mut c = Conn::open(port);
        let (s, b) = c.request("POST", &format!("/cli/home/avatars/put?token={tok}"), Some(&put_body(addr, Some(&url))));
        assert_eq!(s, 200, "owner put: {b}");
        assert_eq!(json(&b, "put")["changed"], true, "{b}");
        assert_eq!(
            std::fs::read(cache_root.join("dtl.k2.dev").join("scout.png")).expect("image on disk"),
            bytes,
            "the image lands under $HOME/.k2/cache/agent-avatars/<server>/<workspace>.png"
        );
        let (s, b) = c.request("POST", &format!("/cli/home/avatars/put?token={tok}"), Some(&put_body(addr, Some(&url))));
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "put again")["changed"], false, "same bytes → changed:false: {b}");
        let other = "nora::192.168.1.20:38471";
        let (s, b) = c.request("POST", &format!("/cli/home/avatars/put?token={tok}"), Some(&put_body(other, None)));
        assert_eq!(s, 200, "a miss: {b}");
        let q = enc(&format!("{addr},{other},ghost::dtl.k2.dev,not-an-address"));
        let (s, b) = c.request("GET", &format!("/cli/home/avatars?token={tok}&addresses={q}"), None);
        assert_eq!(s, 200, "{b}");
        let v = json(&b, "get");
        let avatars = v["avatars"].as_object().unwrap_or_else(|| panic!("avatars object: {v}"));
        assert_eq!(avatars.len(), 2, "only cached addresses come back: {v}");
        assert_eq!(avatars[addr]["dataUrl"], url.as_str(), "same data URL back: {v}");
        assert_eq!(avatars[addr]["missing"], false, "{v}");
        assert!(avatars[addr]["fetchedAt"].as_u64().is_some_and(|t| t > 0), "{v}");
        assert_eq!(avatars[addr]["sha256"].as_str().map(str::len), Some(64), "{v}");
        assert_eq!(avatars[other]["missing"], true, "{v}");
        assert_eq!(avatars[other]["dataUrl"], J::Null, "{v}");

        // Refusals over the wire carry their codes.
        for (body, code) in [
            (put_body(addr, Some("https://x/y.png")), "not_data_url"),
            (put_body("no-host", None), "bad_address"),
            (r#"{"address":"a::b","dataUrl":null,"extra":1}"#.to_string(), "bad_request"),
        ] {
            let (s, v) = call(port, "POST", &format!("/cli/home/avatars/put?token={tok}"), Some(&body));
            assert_eq!(s, 400, "{body}: {v}");
            assert_eq!(v["error"], code, "{body}: {v}");
        }

        // 101 addresses → 400 too_many; 100 is fine.
        let many: Vec<String> = (0..101).map(|i| format!("w{i}::dtl.k2.dev")).collect();
        let (s, v) = call(port, "GET", &format!("/cli/home/avatars?token={tok}&addresses={}", enc(&many.join(","))), None);
        assert_eq!(s, 400, "{v}");
        assert_eq!(v["error"], "too_many", "{v}");
        let (s, v) = call(port, "GET", &format!("/cli/home/avatars?token={tok}&addresses={}", enc(&many[..100].join(","))), None);
        assert_eq!(s, 200, "100 addresses: {v}");

        // Prune: both entries are fresh (inside the 120 s grace), so nothing goes yet.
        let (s, v) = call(port, "POST", &format!("/cli/home/avatars/prune?token={tok}"), Some(&format!(r#"{{"keep":["{addr}"]}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!((v["removed"].as_u64(), v["kept"].as_u64()), (Some(0), Some(2)), "fresh entries survive: {v}");
        // Age the unlisted entry past the grace, then prune again.
        let meta = cache_root.join("192.168.1.20_3a38471").join("nora.json");
        let mut m = json(&std::fs::read_to_string(&meta).expect("nora meta"), "meta");
        m["fetchedAt"] = serde_json::json!(1_000);
        std::fs::write(&meta, m.to_string()).expect("age meta");
        let (s, v) = call(port, "POST", &format!("/cli/home/avatars/prune?token={tok}"), Some(&format!(r#"{{"keep":["{addr}"]}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!((v["removed"].as_u64(), v["kept"].as_u64()), (Some(1), Some(1)), "{v}");
        assert!(!cache_root.join("192.168.1.20_3a38471").exists(), "empty server folder removed");
        assert!(cache_root.join("dtl.k2.dev").join("scout.png").is_file(), "kept entry stays");
    });
}

// ── The real binary, no client attached ─────────────────────────────────

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Daemon {
    child: std::process::Child,
    port: u16,
    owner: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn spawn_daemon(home: &Path) -> Daemon {
    let port_file = home.join(".k2").join("daemon.port");
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", home)
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let token_file = home.join(".k2").join("daemon.token");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&token_file)) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child, port, owner };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut conn) = Conn::try_open(d.port) {
            let (status, body) = conn.request("GET", "/boot-status", None);
            if status == 200 && json(&body, "boot-status")["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon never reached phase ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

#[test]
fn t3_5_real_binary_headless_put_lands_under_temp_home() {
    let home = Home(std::env::temp_dir().join(format!(
        "k2-home-avatars-bin-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir_all(home.0.join(".k2")).expect("create temp HOME/.k2");
    std::fs::create_dir_all(home.0.join("agent-shim-empty")).expect("shim dir");
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();

    let (s, boot) = call(d.port, "GET", "/boot-status", None);
    assert_eq!(s, 200);
    let features: Vec<&str> = boot["features"].as_array().expect("features").iter().filter_map(J::as_str).collect();
    assert!(features.contains(&"home-avatars-v1"), "feature keys: {features:?}");
    assert!(
        !home.0.join(".k2").join("cache").join("agent-avatars").exists(),
        "the cache folder is not created at boot"
    );

    let addr = "scout::[fe80::1]:38471";
    let (bytes, url) = png_url(9, 200);
    let (s, v) = call(d.port, "POST", &format!("/cli/home/avatars/put?token={tok}"), Some(&put_body(addr, Some(&url))));
    assert_eq!(s, 200, "{v}");
    let file = home
        .0
        .join(".k2/cache/agent-avatars/_5bfe80_3a_3a1_5d_3a38471/scout.png");
    assert_eq!(std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())), bytes);
    let (s, v) = call(d.port, "GET", &format!("/cli/home/avatars?token={tok}&addresses={}", enc(addr)), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["avatars"][addr]["dataUrl"], url.as_str(), "{v}");
}
