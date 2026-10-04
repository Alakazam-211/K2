//! Zen Mode v1 headless (prd-zen-mode-v1 T1.1–T1.4): the REAL `k2-daemon`
//! binary under a temp HOME with no client attached.
//!
//! - T1.1: `page/ensure` sets Zen up; a hand edit with an error at line 7 is
//!   reported by `validate` with file:line:col while `get` keeps the previous
//!   version; the fix goes live; a `/cli/sessions/events` socket sees exactly
//!   one `zen_changed` per effective change and none for a no-op save or a
//!   write to `grants.json`/`homes.json`.
//! - T1.2: restarted with a broken file on disk, `get` serves the newest
//!   clean `.history/` snapshot and reports the error.
//! - T1.3: GET on a POST row is 405; Connect logins of every role, no token
//!   and an oversize body are refused; the owner token works.
//! - T1.4: a keep-alive socket answers a run of zen requests in order.
//! - Agents can't grant: no zen route names `grants.json`/`homes.json`.
//!
//! Never touches the real `~/.k2`: HOME is a temp dir removed on drop.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value as J;
use tokio_tungstenite::tungstenite::Message;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_home() -> Home {
    let home = std::env::temp_dir().join(format!(
        "k2-zen-headless-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
    std::fs::create_dir_all(home.join("agent-shim-empty")).expect("shim dir");
    Home(home)
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
    let _ = std::fs::remove_file(&port_file);
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

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request");
        let mut status_line = String::new();
        self.reader.read_line(&mut status_line).expect("read status line");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_else(|| panic!("{path}: no status in {status_line:?}"))
            .parse()
            .unwrap_or_else(|_| panic!("{path}: bad status in {status_line:?}"));
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
        let len = len.unwrap_or_else(|| panic!("{path}: response has no Content-Length"));
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("read body");
        (status, String::from_utf8(buf).expect("utf-8 body"))
    }
}

fn json(body: &str, what: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn call(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, J) {
    let (s, b) = Conn::open(port).request(method, path, body);
    (s, json(&b, path))
}

fn zen_dir(home: &Path) -> PathBuf {
    home.join(".k2").join("zen")
}

/// `zen.toml` with `acent` (a typo) at line 7, column 3.
const BROKEN: &str = "schema = 1\n\
[theme]\n\
scheme = \"auto\"\n\
\n\
[colors.light]\n\
canvas = \"#faf7f2\"\n  acent = \"#fff\"\n";

const FIXED: &str = "schema = 1\n\
[theme]\n\
scheme = \"auto\"\n\
\n\
[colors.light]\n\
canvas = \"#faf7f2\"\n  accent = \"#9a3412\"\n";

fn wait_for<T>(what: &str, within: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + within;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out after {within:?} waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn login_as(port: u16, owner: &str, username: &str, role: &str) -> String {
    let (s, b) = call(
        port,
        "POST",
        &format!("/cli/users/add?token={owner}"),
        Some(&format!(r#"{{"username":"{username}","password":"correct-horse-battery-9"}}"#)),
    );
    assert_eq!(s, 200, "users/add {username}: {b}");
    if role != "member" {
        let (s, b) = call(
            port,
            "POST",
            &format!("/cli/users/set-role?token={owner}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(s, 200, "users/set-role {username}: {b}");
    }
    let (s, b) = call(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(r#"{{"username":"{username}","password":"correct-horse-battery-9"}}"#)),
    );
    assert_eq!(s, 200, "auth/login {username}: {b}");
    b["token"].as_str().unwrap_or_else(|| panic!("login has no token: {b}")).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_1_t1_2_headless_round_trip_last_good_and_events() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();

    let (s, boot) = call(d.port, "GET", "/boot-status", None);
    assert_eq!(s, 200);
    let features: Vec<&str> = boot["features"].as_array().expect("features").iter().filter_map(J::as_str).collect();
    assert!(features.contains(&"zen-v1") && features.contains(&"thread-latest"), "feature keys: {features:?}");

    // Not set up: the folder is never created at boot.
    assert!(!zen_dir(&home.0).exists(), "~/.k2/zen must not exist before page/ensure");
    let (s, v) = call(d.port, "GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"], "zen_not_set_up");
    let (s, v) = call(d.port, "GET", &format!("/cli/zen/status?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["setUp"], false);

    let (s, v) = call(
        d.port,
        "POST",
        &format!("/cli/zen/page/ensure?token={tok}"),
        Some(r#"{"homeId":"home-1","name":"Work"}"#),
    );
    assert_eq!(s, 200, "page/ensure: {v}");
    assert_eq!(v["createdFolder"], true, "{v}");
    assert_eq!(v["file"], "pages/home-1.toml");
    assert!(zen_dir(&home.0).join("zen.toml").is_file(), "ensure writes zen.toml");
    assert!(zen_dir(&home.0).join("pages/home-1.toml").is_file(), "ensure writes the page stub");
    let homes = std::fs::read_to_string(zen_dir(&home.0).join("homes.json")).expect("homes.json");
    assert!(homes.contains("\"Work\""), "{homes}");

    let (s, v0) = call(d.port, "GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
    assert_eq!(s, 200, "{v0}");
    for k in ["schema", "version", "page", "theme", "chrome", "motion", "errors", "warnings", "lastGoodAt"] {
        assert!(v0.get(k).is_some(), "get must carry {k}: {v0}");
    }
    assert_eq!(v0["schema"], 1);
    assert_eq!(v0["page"]["template"], "k2.texting@1");
    // docs/zen-contract.md (the S4 renderer's reading of the page).
    assert_eq!(v0["page"]["layout"]["split"], serde_json::json!([34, 66]), "{v0}");
    assert_eq!(v0["page"]["layout"]["minWidths"], serde_json::json!([240, 360]), "{v0}");
    assert_eq!(v0["page"]["widgets"][0]["kind"], "agents");
    assert_eq!(v0["page"]["widgets"][1]["column"], 1);
    let kinds: Vec<&str> = v0["page"]["controls"].as_array().expect("controls").iter().filter_map(|c| c["kind"].as_str()).collect();
    assert_eq!(kinds.len(), 3, "{v0}");
    assert_eq!(v0["errors"], serde_json::json!([]));
    let version0 = v0["version"].as_str().expect("version").to_string();

    // Events socket, no client otherwise attached.
    let url = format!("ws://127.0.0.1:{}/cli/sessions/events?token={tok}", d.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("hello in time");
    match hello {
        Some(Ok(Message::Text(t))) => assert_eq!(json(&t, "hello")["kind"], "hello"),
        other => panic!("first frame must be hello, got {other:?}"),
    }
    let zen_events = Arc::new(AtomicUsize::new(0));
    let collector = {
        let n = zen_events.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(t) = msg {
                    let v: J = serde_json::from_str(&t).expect("frame JSON");
                    if v["kind"] == "zen_changed" {
                        assert_eq!(v.as_object().map(|o| o.len()), Some(1), "zen_changed is payload-free: {v}");
                        n.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    let port = d.port;
    let events = || zen_events.load(Ordering::SeqCst);

    // Hand edit with a mistake at line 7.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), BROKEN).expect("write broken");
    let v = tokio::task::spawn_blocking({
        let tok = tok.clone();
        move || {
            wait_for("validate to report the error", Duration::from_secs(2), || {
                let (s, v) = call(port, "GET", &format!("/cli/zen/validate?token={tok}"), None);
                assert_eq!(s, 200, "{v}");
                (v["ok"] == false).then_some(v)
            })
        }
    })
    .await
    .expect("join");
    let e0 = &v["errors"][0];
    assert_eq!(e0["file"], "zen.toml", "{v}");
    assert_eq!(e0["line"], 7, "{v}");
    assert_eq!(e0["col"], 3, "{v}");
    assert!(e0["message"].as_str().is_some_and(|m| m.starts_with("unknown key 'acent'")), "{v}");
    let (s, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
    assert_eq!(s, 200, "errors still answer 200 with the last good page: {g}");
    assert_eq!(g["page"], v0["page"], "the last good page rides with the errors: {g}");
    assert_eq!(g["version"], version0.as_str(), "a broken file keeps the previous version: {g}");
    assert_eq!(g["errors"][0], *e0, "get reports the same error: {g}");
    wait_for("one zen_changed for the new error", Duration::from_secs(2), || (events() >= 1).then_some(()));

    // Fix it: the new version goes live within 2 s.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), FIXED).expect("write fixed");
    let g1 = tokio::task::spawn_blocking({
        let tok = tok.clone();
        let version0 = version0.clone();
        move || {
            wait_for("the fixed version to go live", Duration::from_secs(2), || {
                let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
                (g["version"] != version0.as_str() && g["errors"] == serde_json::json!([])).then_some(g)
            })
        }
    })
    .await
    .expect("join");
    assert_eq!(g1["theme"]["colors"]["light"]["accent"], "#9a3412", "{g1}");
    wait_for("the second zen_changed", Duration::from_secs(2), || (events() >= 2).then_some(()));
    let version1 = g1["version"].as_str().expect("v1").to_string();

    // No-op save, daemon-owned files and grants.json: no event.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), FIXED).expect("same bytes");
    std::fs::write(zen_dir(&home.0).join("grants.json"), r#"{"widgets":{}}"#).expect("grants");
    let (s, r) = call(port, "POST", &format!("/cli/zen/reload?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["changed"], false, "a no-op save is not a change: {r}");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(events(), 2, "exactly one zen_changed per effective change");

    // A history exists; the page is listed.
    let (_, h) = call(port, "GET", &format!("/cli/zen/history?token={tok}"), None);
    let zen_hist = h["files"].as_array().expect("files").iter().find(|f| f["file"] == "zen.toml").cloned().expect("zen.toml history");
    assert!(zen_hist["snapshots"].as_array().is_some_and(|s| !s.is_empty()), "{h}");
    let (_, st) = call(port, "GET", &format!("/cli/zen/status?token={tok}"), None);
    assert_eq!(st["setUp"], true);
    assert_eq!(st["watching"], true, "{st}");
    assert_eq!(st["pages"][0]["id"], "home-1", "{st}");
    assert_eq!(st["pages"][0]["name"], "Work", "{st}");

    collector.abort();
    drop(d);

    // T1.2: restart with a broken file on disk.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), BROKEN).expect("break before restart");
    let d2 = spawn_daemon(&home.0);
    let (s, g2) = call(d2.port, "GET", &format!("/cli/zen/get?token={}&home=home-1", d2.owner), None);
    assert_eq!(s, 200, "{g2}");
    assert_eq!(g2["version"], version1.as_str(), "restart serves the newest clean snapshot: {g2}");
    assert_eq!(g2["errors"][0]["line"], 7, "{g2}");
    assert!(g2["sources"]["zen.toml"].as_str().is_some_and(|s| s.starts_with("snapshot:")), "{g2}");
    let (_, st) = call(d2.port, "GET", &format!("/cli/zen/status?token={}", d2.owner), None);
    assert_eq!(st["watching"], true, "the watcher starts at boot when the folder exists: {st}");

    // Reset to default restores a clean zen.toml.
    let (s, r) = call(d2.port, "POST", &format!("/cli/zen/reset?token={}", d2.owner), Some("{}"));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["restored"], "default");
    let (_, v) = call(d2.port, "GET", &format!("/cli/zen/validate?token={}", d2.owner), None);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        std::fs::read_to_string(zen_dir(&home.0).join("grants.json")).expect("grants"),
        r#"{"widgets":{}}"#,
        "no zen route writes grants.json"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_3_t1_4_policy_keep_alive_and_no_grant_route() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        let (s, v) = call(port, "POST", &format!("/cli/zen/page/ensure?token={tok}"), Some(r#"{"homeId":"home-1","name":"Work"}"#));
        assert_eq!(s, 200, "{v}");

        // GET on every POST row → 405.
        for p in ["/cli/zen/page/ensure", "/cli/zen/homes/sync", "/cli/zen/reload", "/cli/zen/reset"] {
            let (s, b) = Conn::open(port).request("GET", &format!("{p}?token={tok}"), None);
            assert_eq!(s, 405, "GET {p} must be 405: {b}");
        }
        // POST on a GET row → 405 (not on the POST allowlist).
        for p in ["/cli/zen/get", "/cli/zen/validate", "/cli/zen/history", "/cli/zen/status", "/cli/zen/doctor"] {
            let (s, b) = Conn::open(port).request("POST", &format!("{p}?token={tok}"), Some("{}"));
            assert_eq!(s, 405, "POST {p} must be 405: {b}");
        }

        // No token, and Connect logins of every role → refused.
        let (s, b) = Conn::open(port).request("GET", "/cli/zen/get?home=home-1", None);
        assert_eq!(s, 403, "no token: {b}");
        for (user, role) in [("zmember", "member"), ("zadmin", "admin"), ("zowner", "owner")] {
            let session = login_as(port, &tok, user, role);
            for (m, p, body) in [
                ("GET", "/cli/zen/get?home=home-1", None),
                ("GET", "/cli/zen/validate", None),
                ("GET", "/cli/zen/status", None),
                ("POST", "/cli/zen/page/ensure", Some(r#"{"homeId":"evil","name":"Evil"}"#)),
                ("POST", "/cli/zen/reset", Some("{}")),
            ] {
                let sep = if p.contains('?') { '&' } else { '?' };
                let (s, b) = Conn::open(port).request(m, &format!("{p}{sep}token={session}"), body);
                assert_eq!(s, 403, "{role} login {m} {p} must be refused: {b}");
                assert_eq!(json(&b, p)["error"], "zen_local_only", "{role} {p}: {b}");
            }
        }
        assert!(!zen_dir(&home.0).join("pages/evil.toml").exists(), "a refused ensure wrote nothing");

        // Oversize body → 413.
        let big = format!(r#"{{"homes":[],"pad":"{}"}}"#, "x".repeat(70 * 1024));
        let (s, b) = Conn::open(port).request("POST", &format!("/cli/zen/homes/sync?token={tok}"), Some(&big));
        assert_eq!(s, 413, "{b}");

        // Agents can't grant: no zen route can name a daemon-owned file.
        for (m, p, body) in [
            ("POST", "/cli/zen/reset", Some(r#"{"file":"grants.json"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"file":"homes.json"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"file":"pages/../grants.json"}"#)),
            ("GET", "/cli/zen/validate?file=grants.json", None),
            ("GET", "/cli/zen/history?file=.history/zen.toml", None),
            ("POST", "/cli/zen/page/ensure", Some(r#"{"homeId":"../grants","name":"x"}"#)),
            ("POST", "/cli/zen/homes/sync", Some(r#"{"homes":[{"id":"../../grants","name":"x"}]}"#)),
        ] {
            let sep = if p.contains('?') { '&' } else { '?' };
            let (s, b) = Conn::open(port).request(m, &format!("{p}{sep}token={tok}"), body);
            assert_eq!(s, 400, "{m} {p} {body:?} must be refused: {b}");
        }
        for p in ["/cli/zen/grant", "/cli/zen/grants", "/cli/zen/grants/set"] {
            let (s, b) = Conn::open(port).request("POST", &format!("{p}?token={tok}"), Some("{}"));
            assert_eq!(s, 405, "there is no grant route ({p}): {b}");
            let (s, b) = Conn::open(port).request("GET", &format!("{p}?token={tok}"), None);
            assert_eq!(s, 404, "there is no grant route ({p}): {b}");
        }
        assert!(!zen_dir(&home.0).join("grants.json").exists(), "nothing created grants.json");

        // T1.4: one keep-alive socket, a run of zen requests, each answered once.
        let mut c = Conn::open(port);
        let (s, b) = c.request("POST", &format!("/cli/zen/reload?token={tok}"), Some("{}"));
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "reload").get("changed").is_some(), "reload body: {b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "get")["version"].is_string(), "the request after reload got the wrong body: {b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/status?token={tok}"), None);
        assert_eq!(s, 200);
        assert!(json(&b, "status")["setUp"].is_boolean(), "status body: {b}");
        let (s, b) = c.request("POST", &format!("/cli/zen/homes/sync?token={tok}"), Some(r#"{"homes":[{"id":"home-1","name":"Work"}]}"#));
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "sync")["written"], true, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/validate?token={tok}"), None);
        assert_eq!(s, 200);
        assert_eq!(json(&b, "validate")["ok"], true, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/doctor?token={tok}"), None);
        assert_eq!(s, 200);
        let doc = json(&b, "doctor");
        assert!(doc["checks"].as_array().is_some_and(|c| c.iter().any(|x| x["name"] == "watcher")), "{doc}");
        let (s, b) = c.request("GET", &format!("/cli/feedback/waiting-count?token={tok}"), None);
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "waiting-count")["count"].is_number(), "a non-zen route after zen ones: {b}");
    })
    .await
    .expect("blocking body");
}
