//! Zen headless (prd-zen-mode-v1 T1.1–T1.4; prd-zen-gardens-v1 TG1.1–TG1.3):
//! the REAL `k2-daemon` binary under a temp HOME with no client attached.
//!
//! - TG1.1: before setup, `gardens` is 200 `setUp:false` and `garden/new` is
//!   404 `zen_not_set_up`; `setup` makes Garden 1 on `k2.texting@1` (Garden
//!   switcher, never the Home switcher) and Garden 2 on `k2.blank@1`, once; a Garden made over plain HTTP gets
//!   the empty `k2.blank@1` page; create, rename, reorder, delete and a page
//!   save each emit exactly ONE `zen_changed`; a no-op rename emits none;
//!   the last Garden can't go; a deleted Garden is 404 `unknown_garden`.
//! - T1.1/T1.2: a hand edit with an error at line 7 is reported with
//!   file:line:col while `get` keeps the previous version; a restart with a
//!   broken file serves the newest clean snapshot.
//! - TG1.2/TG1.3: removed routes are 404; GET on a POST row is 405; Connect
//!   logins of every role, no token and an oversize body are refused; the
//!   owner token works; a keep-alive socket answers a run of requests.
//! - Agents can't grant: no zen route names `grants.json`/`gardens.json`.
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


/// POST `setup` as the app does when Zen is first turned on. Returns the
/// answer and Garden 1's id.
fn setup(port: u16, tok: &str) -> (J, String) {
    let (s, v) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "setup: {v}");
    let id = v["gardens"][0]["id"].as_str().unwrap_or_else(|| panic!("setup lists no Garden: {v}")).to_string();
    (v, id)
}

/// Count payload-free `zen_changed` frames on one `/cli/sessions/events`
/// socket (no client otherwise attached).
async fn zen_event_counter(port: u16, tok: &str) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={tok}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("hello in time");
    match hello {
        Some(Ok(Message::Text(t))) => assert_eq!(json(&t, "hello")["kind"], "hello"),
        other => panic!("first frame must be hello, got {other:?}"),
    }
    let n = Arc::new(AtomicUsize::new(0));
    let counter = n.clone();
    let task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                let v: J = serde_json::from_str(&t).expect("frame JSON");
                if v["kind"] == "zen_changed" {
                    assert_eq!(v.as_object().map(|o| o.len()), Some(1), "zen_changed is payload-free: {v}");
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    (n, task)
}

/// Exactly one more `zen_changed` than `before`, held for a 1 s quiet window.
fn expect_one_event(events: &AtomicUsize, before: usize, what: &str) {
    wait_for(&format!("one zen_changed for {what}"), Duration::from_secs(3), || {
        (events.load(Ordering::SeqCst) > before).then_some(())
    });
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(events.load(Ordering::SeqCst), before + 1, "{what} must emit exactly one zen_changed");
}

fn control_kinds(g: &J) -> Vec<String> {
    g["page"]["controls"]
        .as_array()
        .unwrap_or_else(|| panic!("controls: {g}"))
        .iter()
        .map(|c| c["kind"].as_str().unwrap_or_else(|| panic!("control kind: {c}")).to_string())
        .collect()
}

/// TG1.1: Gardens over plain HTTP with no client running, one event each.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tg1_1_headless_gardens_round_trip_and_one_event_each() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let port = d.port;

    let (s, boot) = call(port, "GET", "/boot-status", None);
    assert_eq!(s, 200);
    let features: Vec<&str> = boot["features"].as_array().expect("features").iter().filter_map(J::as_str).collect();
    assert!(features.contains(&"zen-gardens-v1") && features.contains(&"zen-v1"), "G19 feature keys: {features:?}");

    // Not set up: 200 setUp:false (never 404), and agents can't set it up.
    let (s, v) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["setUp"], false, "{v}");
    assert_eq!(v["gardens"], serde_json::json!([]), "{v}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Notes"}"#));
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"], "zen_not_set_up", "{v}");
    assert_eq!(
        v["message"], "Zen isn't set up on this computer. Turn it on with the Zen toggle in the K2 app's top bar.",
        "G46 sentence: {v}"
    );
    let (s, v) = call(port, "GET", &format!("/cli/zen/get?token={tok}"), None);
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"], "zen_not_set_up");
    assert!(!zen_dir(&home.0).exists(), "nothing but setup creates ~/.k2/zen");

    // setup: folder, gardens.json with Garden 1 on k2.texting@1 and
    // Garden 2 on k2.blank@1 (Rosson 2026-10-04).
    let (v, default_id) = setup(port, &tok);
    assert_eq!(v["createdFolder"], true, "{v}");
    assert_eq!(v["createdDefault"], true, "{v}");
    assert!(v["migrated"].is_null(), "per-Home Zen never shipped: nothing migrates: {v}");
    assert_eq!(v["gardens"].as_array().map(Vec::len), Some(2), "{v}");
    assert_eq!(v["gardens"][0]["name"], "Garden 1", "{v}");
    assert_eq!(v["gardens"][0]["template"], "k2.texting@1", "{v}");
    assert_eq!(v["gardens"][0]["index"], 1, "{v}");
    assert_eq!(v["gardens"][1]["name"], "Garden 2", "{v}");
    assert_eq!(v["gardens"][1]["template"], "k2.blank@1", "{v}");
    assert_eq!(v["gardens"][1]["index"], 2, "{v}");
    let two_id = v["gardens"][1]["id"].as_str().expect("Garden 2 id").to_string();
    let (s, g2) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={two_id}"), None);
    assert_eq!(s, 200, "{g2}");
    let g2_widgets: Vec<&str> = g2["page"]["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["kind"].as_str()).collect();
    assert_eq!(g2_widgets, vec!["garden-empty"], "Garden 2 holds only the empty-Garden widget: {g2}");
    let list: J = serde_json::from_str(&std::fs::read_to_string(zen_dir(&home.0).join("gardens.json")).expect("gardens.json"))
        .expect("gardens.json JSON");
    assert_eq!(list["gardens"][0]["id"], default_id.as_str(), "{list}");
    assert!(zen_dir(&home.0).join(format!("gardens/{default_id}.toml")).is_file(), "Garden 1's stub");
    assert!(zen_dir(&home.0).join(format!("gardens/{two_id}.toml")).is_file(), "Garden 2's stub");
    let (s, again) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["createdFolder"], false, "setup is idempotent: {again}");
    assert_eq!(again["createdDefault"], false, "setup is idempotent: {again}");
    assert_eq!(again["gardens"].as_array().map(Vec::len), Some(2), "{again}");
    let (_, g0) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={default_id}"), None);
    assert_eq!(g0["page"]["template"], "k2.texting@1", "{g0}");
    let kinds = control_kinds(&g0);
    assert!(kinds.contains(&"garden-switcher".to_string()), "{kinds:?}");
    assert!(!kinds.contains(&"home-switcher".to_string()), "never the Home switcher: {kinds:?}");
    assert_eq!(g0["garden"]["id"], default_id.as_str(), "{g0}");
    assert!(g0.get("home").is_none(), "{g0}");
    let (_, first) = call(port, "GET", &format!("/cli/zen/get?token={tok}"), None);
    assert_eq!(first["garden"]["id"], default_id.as_str(), "no garden= is the first Garden");
    let (s, v) = call(port, "GET", &format!("/cli/zen/get?token={tok}&home=home-1"), None);
    assert_eq!(s, 400, "a pre-Gardens home= is refused, not ignored: {v}");

    let (events, collector) = zen_event_counter(port, &tok).await;
    let home_dir = home.0.clone();
    let tok2 = tok.clone();
    tokio::task::spawn_blocking(move || {
        let tok = tok2;
        let n = || events.load(Ordering::SeqCst);

        // Create over plain HTTP: g- + 8 hex, empty template, a stub on disk.
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Notes"}"#));
        assert_eq!(s, 200, "{v}");
        let notes = v["garden"]["id"].as_str().expect("id").to_string();
        assert!(notes.starts_with("g-") && notes.len() == 10 && notes[2..].chars().all(|c| c.is_ascii_hexdigit()), "{notes}");
        assert_eq!(v["garden"]["template"], "k2.blank@1", "{v}");
        assert_eq!(v["garden"]["index"], 3, "{v}");
        assert_eq!(v["file"], format!("gardens/{notes}.toml"), "{v}");
        assert!(zen_dir(&home_dir).join(format!("gardens/{notes}.toml")).is_file(), "stub on disk");
        expect_one_event(&events, before, "a create");
        // The headless proof: get returns the empty template.
        let (s, e) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={notes}"), None);
        assert_eq!(s, 200, "{e}");
        assert_eq!(e["page"]["template"], "k2.blank@1", "{e}");
        let widgets = e["page"]["widgets"].as_array().expect("widgets");
        assert_eq!(widgets.len(), 1, "{e}");
        assert_eq!(widgets[0]["kind"], "garden-empty", "{e}");
        assert_eq!(widgets[0]["caps"], serde_json::json!(["agents:read", "thread:read", "thread:post", "gardens:template"]), "{e}");
        assert_eq!(control_kinds(&e), vec!["garden-switcher", "drag-region", "zen-toggle"], "{e}");
        let toggle = e["page"]["controls"].as_array().expect("controls").iter().find(|c| c["kind"] == "zen-toggle").expect("zen-toggle");
        assert_eq!(toggle["placement"], "top-right", "the Zen toggle sits top right: {e}");
        assert_eq!(e["garden"], serde_json::json!({"id": notes, "name": "Notes", "index": 3}), "{e}");
        assert_eq!(e["errors"], serde_json::json!([]), "{e}");
        let (_, by_name) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden=notes"), None);
        assert_eq!(by_name["garden"]["id"], notes.as_str(), "a name selects, case aside");

        // A duplicate name in another case → 409 garden_exists; no event.
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"NOTES"}"#));
        assert_eq!(s, 409, "{v}");
        assert_eq!(v["error"], "garden_exists", "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"x","home":"h"}"#));
        assert_eq!(s, 400, "unknown body fields are refused: {v}");

        // Rename: once; the same name again: none.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/rename?token={tok}"), Some(&format!(r#"{{"garden":"{notes}","name":"Launch room"}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["garden"]["name"], "Launch room", "{v}");
        expect_one_event(&events, before, "a rename");
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/rename?token={tok}"), Some(r#"{"garden":"launch room","name":"Launch room"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["changed"], false, "{v}");
        std::thread::sleep(Duration::from_millis(1000));
        assert_eq!(n(), before, "a no-op rename emits nothing");

        // Reorder: once; out of range 400.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/reorder?token={tok}"), Some(&format!(r#"{{"garden":"{notes}","to":1}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["gardens"][0]["id"], notes.as_str(), "{v}");
        expect_one_event(&events, before, "a reorder");
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/reorder?token={tok}"), Some(&format!(r#"{{"garden":"{notes}","to":4}}"#)));
        assert_eq!(s, 400, "{v}");
        let (_, l) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
        assert_eq!(l["setUp"], true);
        let names: Vec<&str> = l["gardens"].as_array().expect("gardens").iter().filter_map(|g| g["name"].as_str()).collect();
        assert_eq!(names, vec!["Launch room", "Garden 1", "Garden 2"], "{l}");

        // A page save (the watcher): once. A broken one keeps the last good page.
        let page = zen_dir(&home_dir).join(format!("gardens/{notes}.toml"));
        let before = n();
        std::fs::write(
            &page,
            "schema = 1\n[[widget]]\nid = \"work\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n",
        )
        .expect("save page");
        expect_one_event(&events, before, "a Garden page save");
        let (_, w) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={notes}"), None);
        assert_eq!(w["page"]["widgets"][0]["kind"], "agents", "the page's widgets replace the empty one: {w}");
        assert_eq!(w["page"]["widgets"][0]["props"]["mode"], "home", "{w}");
        let good = w["version"].clone();
        std::fs::write(
            &page,
            "schema = 1\n[[widget]]\nid = \"work\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nhom = \"Work\"\n",
        )
        .expect("break page");
        let broken = wait_for("get to report the line 7 error", Duration::from_secs(3), || {
            let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={notes}"), None);
            g["errors"].as_array().is_some_and(|e| !e.is_empty()).then_some(g)
        });
        assert_eq!(broken["version"], good, "a broken page keeps the last good version: {broken}");
        assert_eq!(broken["errors"][0]["file"], format!("gardens/{notes}.toml"), "{broken}");
        assert_eq!(broken["errors"][0]["line"], 7, "{broken}");
        let (_, v) = call(port, "GET", &format!("/cli/zen/validate?token={tok}&garden={notes}"), None);
        assert_eq!(v["ok"], false, "{v}");

        // Delete: once; the page moves into history; 404 after.
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/delete?token={tok}"), Some(&format!(r#"{{"garden":"{notes}"}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["deleted"], notes.as_str(), "{v}");
        let snap = v["snapshot"].as_str().expect("snapshot").to_string();
        expect_one_event(&events, before, "a delete");
        assert!(!page.exists(), "the page left gardens/");
        assert!(zen_dir(&home_dir).join(format!(".history/gardens/{notes}.toml/{snap}")).is_file(), "the page is in history");
        let (s, v) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={notes}"), None);
        assert_eq!(s, 404, "{v}");
        assert_eq!(v["error"], "unknown_garden", "{v}");
        assert_eq!(v["gardens"], serde_json::json!([default_id, two_id]), "{v}");
        let (s, h) = call(port, "GET", &format!("/cli/zen/history?token={tok}&garden={notes}"), None);
        assert_eq!(s, 200, "history still lists a deleted Garden by id: {h}");
        assert_eq!(h["files"][0]["deleted"], true, "{h}");

        // Garden 2 can go, and setup never brings it back; the last Garden stays.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/delete?token={tok}"), Some(r#"{"garden":"Garden 2"}"#));
        assert_eq!(s, 200, "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["createdDefault"], false, "{v}");
        assert_eq!(v["gardens"].as_array().map(Vec::len), Some(1), "a deleted Garden 2 stays deleted: {v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/delete?token={tok}"), Some(r#"{"garden":"Garden 1"}"#));
        assert_eq!(s, 409, "{v}");
        assert_eq!(v["error"], "last_garden", "{v}");
        assert_eq!(v["message"], "That's your last Garden.", "{v}");

        // Per-Garden theme: the pick shows on that Garden only.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Mornings","template":"texting","seedHome":"home-2","at":1}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["garden"]["seedHome"], "home-2", "{v}");
        assert_eq!(v["garden"]["template"], "k2.texting@1", "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"midnight","garden":"Mornings"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["scope"], "garden", "{v}");
        let (_, m) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden=Mornings"), None);
        assert_eq!(m["theme"]["name"], "midnight", "{m}");
        assert_eq!(m["theme"]["scope"], "garden", "{m}");
        let (_, dflt) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={default_id}"), None);
        assert_eq!(dflt["theme"]["name"], "basic", "other Gardens follow the global theme: {dflt}");
        let (_, l) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
        assert_eq!(l["gardens"][0]["theme"], "midnight", "{l}");
    })
    .await
    .expect("blocking body");
    collector.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_1_t1_2_headless_zen_toml_last_good_and_restart() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let (_, g) = setup(d.port, &tok);
    let port = d.port;

    let (s, v0) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={g}"), None);
    assert_eq!(s, 200, "{v0}");
    for k in ["schema", "version", "garden", "page", "theme", "chrome", "motion", "errors", "warnings", "lastGoodAt"] {
        assert!(v0.get(k).is_some(), "get must carry {k}: {v0}");
    }
    assert_eq!(v0["schema"], 1);
    assert_eq!(v0["page"]["layout"]["split"], serde_json::json!([34, 66]), "{v0}");
    assert_eq!(v0["page"]["layout"]["minWidths"], serde_json::json!([240, 360]), "{v0}");
    assert_eq!(v0["page"]["widgets"][0]["kind"], "agents");
    assert_eq!(v0["page"]["widgets"][1]["column"], 1);
    assert_eq!(v0["errors"], serde_json::json!([]));
    let version0 = v0["version"].as_str().expect("version").to_string();

    let (events, collector) = zen_event_counter(port, &tok).await;
    let n = {
        let e = events.clone();
        move || e.load(Ordering::SeqCst)
    };

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
    let (s, gg) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={g}"), None);
    assert_eq!(s, 200, "errors still answer 200 with the last good page: {gg}");
    assert_eq!(gg["page"], v0["page"], "the last good page rides with the errors: {gg}");
    assert_eq!(gg["version"], version0.as_str(), "a broken file keeps the previous version: {gg}");
    assert_eq!(gg["errors"][0], *e0, "get reports the same error: {gg}");
    {
        let n = n.clone();
        wait_for("one zen_changed for the new error", Duration::from_secs(2), move || (n() >= 1).then_some(()));
    }

    // Fix it: the new version goes live within 2 s.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), FIXED).expect("write fixed");
    let g1 = tokio::task::spawn_blocking({
        let (tok, g, version0) = (tok.clone(), g.clone(), version0.clone());
        move || {
            wait_for("the fixed version to go live", Duration::from_secs(2), || {
                let (_, x) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={g}"), None);
                (x["version"] != version0.as_str() && x["errors"] == serde_json::json!([])).then_some(x)
            })
        }
    })
    .await
    .expect("join");
    assert_eq!(g1["theme"]["tokens"]["colors"]["light"]["accent"], "#9a3412", "{g1}");
    {
        let n = n.clone();
        wait_for("the second zen_changed", Duration::from_secs(2), move || (n() >= 2).then_some(()));
    }
    let version1 = g1["version"].as_str().expect("v1").to_string();

    // No-op save and grants.json: no event.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), FIXED).expect("same bytes");
    std::fs::write(zen_dir(&home.0).join("grants.json"), r#"{"widgets":{}}"#).expect("grants");
    let (s, r) = call(port, "POST", &format!("/cli/zen/reload?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["changed"], false, "a no-op save is not a change: {r}");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(n(), 2, "exactly one zen_changed per effective change");

    let (_, h) = call(port, "GET", &format!("/cli/zen/history?token={tok}"), None);
    let zen_hist = h["files"].as_array().expect("files").iter().find(|f| f["file"] == "zen.toml").cloned().expect("zen.toml history");
    assert!(zen_hist["snapshots"].as_array().is_some_and(|s| !s.is_empty()), "{h}");
    let (_, st) = call(port, "GET", &format!("/cli/zen/status?token={tok}"), None);
    assert_eq!(st["setUp"], true);
    assert_eq!(st["watching"], true, "{st}");
    assert_eq!(st["gardens"][0]["id"], g.as_str(), "{st}");
    assert_eq!(st["gardens"][0]["name"], "Garden 1", "{st}");
    assert!(st.get("pages").is_none(), "status lists gardens, not pages: {st}");

    collector.abort();
    drop(d);

    // T1.2: restart with a broken file on disk.
    std::fs::write(zen_dir(&home.0).join("zen.toml"), BROKEN).expect("break before restart");
    let d2 = spawn_daemon(&home.0);
    let (s, g2) = call(d2.port, "GET", &format!("/cli/zen/get?token={}&garden={g}", d2.owner), None);
    assert_eq!(s, 200, "{g2}");
    assert_eq!(g2["version"], version1.as_str(), "restart serves the newest clean snapshot: {g2}");
    assert_eq!(g2["errors"][0]["line"], 7, "{g2}");
    assert!(g2["sources"]["zen.toml"].as_str().is_some_and(|s| s.starts_with("snapshot:")), "{g2}");
    let (_, st) = call(d2.port, "GET", &format!("/cli/zen/status?token={}", d2.owner), None);
    assert_eq!(st["watching"], true, "the watcher starts at boot when Zen is set up: {st}");

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
async fn tg1_2_tg1_3_policy_removed_routes_keep_alive_and_no_grant_route() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        let (_, g) = setup(port, &tok);

        // TG1.2: the per-Home routes are gone. A POST is refused by the
        // top-level guard (no longer a POST row, like any unknown path); a
        // GET reaches the zen arm, which doesn't know them.
        for (p, body) in [
            ("/cli/zen/page/ensure", r#"{"homeId":"home-1","name":"Work"}"#),
            ("/cli/zen/homes/sync", r#"{"homes":[]}"#),
        ] {
            let (s, b) = Conn::open(port).request("POST", &format!("{p}?token={tok}"), Some(body));
            assert_eq!(s, 405, "POST {p} must be gone (not a POST row): {b}");
            let (s, b) = Conn::open(port).request("GET", &format!("{p}?token={tok}"), None);
            assert_eq!(s, 404, "GET {p} must be gone: {b}");
            assert_eq!(json(&b, p)["error"], "unknown zen route", "{p}: {b}");
        }
        assert!(!zen_dir(&home.0).join("pages").exists(), "nothing wrote a per-Home page");

        // GET on every POST row → 405.
        for p in [
            "/cli/zen/setup",
            "/cli/zen/garden/new",
            "/cli/zen/garden/rename",
            "/cli/zen/garden/delete",
            "/cli/zen/garden/reorder",
            "/cli/zen/garden/template",
            "/cli/zen/reload",
            "/cli/zen/reset",
            "/cli/zen/theme/set",
            "/cli/zen/theme/next",
            "/cli/zen/theme/prev",
            "/cli/zen/theme/new",
        ] {
            let (s, b) = Conn::open(port).request("GET", &format!("{p}?token={tok}"), None);
            assert_eq!(s, 405, "GET {p} must be 405: {b}");
        }
        // POST on a GET row → 405 (not on the POST allowlist).
        for p in ["/cli/zen/get", "/cli/zen/gardens", "/cli/zen/validate", "/cli/zen/history", "/cli/zen/status", "/cli/zen/doctor", "/cli/zen/theme/list"] {
            let (s, b) = Conn::open(port).request("POST", &format!("{p}?token={tok}"), Some("{}"));
            assert_eq!(s, 405, "POST {p} must be 405: {b}");
        }

        // No token, and Connect logins of every role → refused.
        let (s, b) = Conn::open(port).request("GET", "/cli/zen/gardens", None);
        assert_eq!(s, 403, "no token: {b}");
        for (user, role) in [("zmember", "member"), ("zadmin", "admin"), ("zowner", "owner")] {
            let session = login_as(port, &tok, user, role);
            for (m, p, body) in [
                ("GET", "/cli/zen/get", None),
                ("GET", "/cli/zen/gardens", None),
                ("GET", "/cli/zen/validate", None),
                ("GET", "/cli/zen/status", None),
                ("POST", "/cli/zen/setup", Some("{}")),
                ("POST", "/cli/zen/garden/new", Some(r#"{"name":"Evil"}"#)),
                ("POST", "/cli/zen/garden/rename", Some(r#"{"garden":"Garden 1","name":"Evil"}"#)),
                ("POST", "/cli/zen/garden/reorder", Some(r#"{"garden":"Garden 1","to":1}"#)),
                ("POST", "/cli/zen/garden/delete", Some(r#"{"garden":"Garden 1"}"#)),
                ("POST", "/cli/zen/garden/template", Some(r#"{"garden":"Garden 2","template":"texting"}"#)),
                ("POST", "/cli/zen/reset", Some("{}")),
                ("GET", "/cli/zen/theme/list", None),
                ("POST", "/cli/zen/theme/set", Some(r#"{"name":"paper"}"#)),
                ("POST", "/cli/zen/theme/next", Some("{}")),
                ("POST", "/cli/zen/theme/new", Some(r#"{"name":"evil"}"#)),
            ] {
                let sep = if p.contains('?') { '&' } else { '?' };
                let (s, b) = Conn::open(port).request(m, &format!("{p}{sep}token={session}"), body);
                assert_eq!(s, 403, "{role} login {m} {p} must be refused: {b}");
                assert_eq!(json(&b, p)["error"], "zen_local_only", "{role} {p}: {b}");
            }
        }
        let (_, l) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
        let names: Vec<&str> = l["gardens"].as_array().expect("gardens").iter().filter_map(|x| x["name"].as_str()).collect();
        assert_eq!(names, vec!["Garden 1", "Garden 2"], "refused requests wrote nothing: {l}");
        assert!(!zen_dir(&home.0).join("themes/evil").exists(), "a refused theme/new wrote nothing");
        assert!(!zen_dir(&home.0).join("active.json").exists(), "a refused theme switch wrote nothing");

        // Oversize body → 413.
        let big = format!(r#"{{"name":"x","pad":"{}"}}"#, "x".repeat(70 * 1024));
        let (s, b) = Conn::open(port).request("POST", &format!("/cli/zen/garden/new?token={tok}"), Some(&big));
        assert_eq!(s, 413, "{b}");

        // Agents can't grant: no zen route can name a daemon-owned file.
        for (m, p, body) in [
            ("POST", "/cli/zen/reset", Some(r#"{"file":"grants.json"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"file":"gardens.json"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"file":"gardens/../grants.json"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"file":"pages/home-1.toml"}"#)),
            ("POST", "/cli/zen/reset", Some(r#"{"home":"home-1"}"#)),
            ("GET", "/cli/zen/validate?file=grants.json", None),
            ("GET", "/cli/zen/validate?home=home-1", None),
            ("GET", "/cli/zen/history?file=.history/zen.toml", None),
            ("POST", "/cli/zen/garden/new", Some(r#"{"name":"x","seedHome":"../grants"}"#)),
            ("POST", "/cli/zen/setup", Some(r#"{"homeId":"x"}"#)),
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
        let (s, b) = c.request("GET", &format!("/cli/zen/get?token={tok}&garden={g}"), None);
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "get")["version"].is_string(), "the request after reload got the wrong body: {b}");
        let (s, b) = c.request("POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Kept alive"}"#));
        assert_eq!(s, 200, "{b}");
        let kept = json(&b, "garden/new")["garden"]["id"].as_str().expect("id").to_string();
        let (s, b) = c.request("POST", &format!("/cli/zen/garden/rename?token={tok}"), Some(&format!(r#"{{"garden":"{kept}","name":"Renamed"}}"#)));
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "rename")["garden"]["name"], "Renamed", "the request after garden/new got the wrong body: {b}");
        let (s, b) = c.request("POST", &format!("/cli/zen/garden/reorder?token={tok}"), Some(&format!(r#"{{"garden":"{kept}","to":1}}"#)));
        assert_eq!(s, 200, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/gardens?token={tok}"), None);
        assert_eq!(s, 200);
        assert_eq!(json(&b, "gardens")["gardens"][0]["id"], kept.as_str(), "{b}");
        let (s, b) = c.request("POST", &format!("/cli/zen/garden/delete?token={tok}"), Some(&format!(r#"{{"garden":"{kept}"}}"#)));
        assert_eq!(s, 200, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/status?token={tok}"), None);
        assert_eq!(s, 200);
        assert!(json(&b, "status")["setUp"].is_boolean(), "status body: {b}");
        let (s, b) = c.request("POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
        assert_eq!(s, 200, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/validate?token={tok}"), None);
        assert_eq!(s, 200);
        assert_eq!(json(&b, "validate")["ok"], true, "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/doctor?token={tok}"), None);
        assert_eq!(s, 200);
        let doc = json(&b, "doctor");
        assert!(doc["checks"].as_array().is_some_and(|c| c.iter().any(|x| x["name"] == "watcher")), "{doc}");
        assert!(doc["checks"].as_array().is_some_and(|c| c.iter().any(|x| x["name"] == "gardens.json" && x["ok"] == true)), "{doc}");
        let (s, b) = c.request("POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper"}"#));
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "theme/set")["theme"], "paper", "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/theme/list?token={tok}"), None);
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "theme/list")["active"], "paper", "the request after theme/set got the wrong body: {b}");
        let (s, b) = c.request("GET", &format!("/cli/feedback/waiting-count?token={tok}"), None);
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "waiting-count")["count"].is_number(), "a non-zen route after zen ones: {b}");
    })
    .await
    .expect("blocking body");
}

/// Omarchy additions 1–3, headless (no client): a theme switch through the
/// routes alone shows in `get` and emits exactly one `zen_changed`; an
/// unknown name is a clear 404 that changes nothing; next/prev cycle in
/// order; a theme bundle made by hand (with a background image) is picked
/// up by the watcher and served as a `data:` URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omarchy_theme_switch_headless_cycle_unknown_and_bundle() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let port = d.port;

    // Before setup every theme route says so.
    let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper"}"#));
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"], "zen_not_set_up", "{v}");
    assert!(!zen_dir(&home.0).exists(), "a theme route must never create ~/.k2/zen");

    let (_, gid) = setup(port, &tok);
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Other"}"#));
    assert_eq!(s, 200, "{v}");
    let other = v["garden"]["id"].as_str().expect("other id").to_string();
    let zen_toml = std::fs::read_to_string(zen_dir(&home.0).join("zen.toml")).expect("zen.toml");
    assert!(
        !zen_toml.lines().any(|l| l.trim_start().starts_with('[')),
        "zen.toml holds only the user's changes (no tables), not the defaults: {zen_toml}"
    );

    let (s, g0) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={gid}"), None);
    assert_eq!(s, 200, "{g0}");
    let t0 = &g0["theme"];
    assert_eq!(t0["name"], "basic", "{g0}");
    assert_eq!(t0["label"], "Basic", "people see the capitalized name: {g0}");
    assert_eq!(t0["builtin"], true, "{g0}");
    for k in ["tokens", "font", "terminal"] {
        assert!(t0[k].is_object(), "theme.{k} must be an object: {t0}");
    }
    assert!(t0.get("background").is_none(), "the basic theme has no background: {t0}");
    assert_eq!(t0["tokens"]["colors"]["light"]["idle"], "#a39b90", "{t0}");
    assert!(t0["tokens"]["colors"]["light"].get("unread").is_none(), "decision 9: no unread token: {t0}");
    assert_eq!(t0["font"]["family"], "system", "{t0}");
    assert_eq!(t0["font"]["terminal"]["family"], "meslo", "a proportional font pairs with meslo in terminals: {t0}");
    assert_eq!(t0["terminal"]["palette"]["dark"]["blue"], "#7aa7d8", "{t0}");
    let names: Vec<&str> = g0["themes"].as_array().expect("themes").iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, vec!["basic", "paper", "midnight"], "{g0}");
    let labels: Vec<&str> = g0["themes"].as_array().expect("themes").iter().filter_map(|t| t["label"].as_str()).collect();
    assert_eq!(labels, vec!["Basic", "Paper", "Midnight"], "{g0}");
    assert!(g0["themes"].as_array().expect("themes").iter().all(|t| t["builtin"].is_boolean()), "{g0}");

    // Events socket.
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={tok}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("hello in time");
    assert!(matches!(hello, Some(Ok(Message::Text(_)))), "hello frame: {hello:?}");
    let zen_events = Arc::new(AtomicUsize::new(0));
    let collector = {
        let n = zen_events.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(t) = msg {
                    let v: J = serde_json::from_str(&t).expect("frame JSON");
                    if v["kind"] == "zen_changed" {
                        n.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    let events = move || zen_events.load(Ordering::SeqCst);

    let home_dir = home.0.clone();
    let tok2 = tok.clone();
    tokio::task::spawn_blocking(move || {
        let tok = tok2;
        let _ = &other;
        // Switch globally: get shows it, one event.
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["theme"], "paper", "{v}");
        assert_eq!(v["scope"], "global", "{v}");
        assert_eq!(v["changed"], true, "{v}");
        wait_for("one zen_changed for the switch", Duration::from_secs(3), || (events() >= 1).then_some(()));
        let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={gid}"), None);
        assert_eq!(g["theme"]["name"], "paper", "{g}");
        assert_eq!(g["theme"]["tokens"]["scheme"], "light", "{g}");
        assert_eq!(g["theme"]["font"]["family"], "serif", "{g}");
        assert_ne!(g["version"], g0["version"], "a switch changes the version");
        let active: J = serde_json::from_str(&std::fs::read_to_string(zen_dir(&home_dir).join("active.json")).expect("active.json")).expect("active JSON");
        assert_eq!(active["theme"], "paper", "{active}");

        // The same switch again is not a change.
        let (_, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper"}"#));
        assert_eq!(v["changed"], false, "{v}");

        // Unknown name: clear 404, nothing changes, no event.
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"neon"}"#));
        assert_eq!(s, 404, "{v}");
        assert_eq!(v["error"], "unknown_theme", "{v}");
        assert_eq!(v["theme"], "neon", "{v}");
        assert!(v["message"].as_str().is_some_and(|m| m.contains("no theme 'neon'") && m.contains("paper")), "{v}");
        assert!(v["themes"].as_array().is_some_and(|t| t.iter().any(|x| x == "basic")), "{v}");
        assert!(v["themes"].as_array().is_some_and(|t| !t.iter().any(|x| x == "default")), "no theme is called default: {v}");
        let (_, l) = call(port, "GET", &format!("/cli/zen/theme/list?token={tok}"), None);
        assert_eq!(l["active"], "paper", "an unknown name changes nothing: {l}");
        // Bad bodies.
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some("{}"));
        assert_eq!(s, 400, "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"nmae":"paper"}"#));
        assert_eq!(s, 400, "unknown body fields are refused: {v}");

        // Cycle: paper -> midnight -> basic (wraps) -> prev -> midnight.
        let (_, v) = call(port, "POST", &format!("/cli/zen/theme/next?token={tok}"), Some("{}"));
        assert_eq!(v["theme"], "midnight", "{v}");
        let (_, v) = call(port, "POST", &format!("/cli/zen/theme/next?token={tok}"), Some(""));
        assert_eq!(v["theme"], "basic", "next wraps: {v}");
        let (_, v) = call(port, "POST", &format!("/cli/zen/theme/prev?token={tok}"), Some("{}"));
        assert_eq!(v["theme"], "midnight", "prev wraps back: {v}");

        // Per Garden: the Garden's pick beats the global one; clear drops it.
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper","garden":"Garden 1"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["scope"], "garden", "{v}");
        assert_eq!(v["garden"], gid.as_str(), "{v}");
        let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={gid}"), None);
        assert_eq!(g["theme"]["name"], "paper", "{g}");
        assert_eq!(g["theme"]["scope"], "garden", "{g}");
        let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={other}"), None);
        assert_eq!(g["theme"]["name"], "midnight", "global stays for the other Garden: {g}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"garden":"Garden 1","clear":true}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["theme"], "midnight", "{v}");
        assert_eq!(v["scope"], "global", "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper","garden":"Nowhere"}"#));
        assert_eq!(s, 404, "{v}");
        assert_eq!(v["error"], "unknown_garden", "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"paper","home":"Work"}"#));
        assert_eq!(s, 400, "a pre-Gardens home key is refused: {v}");

        // A bundle made by hand: theme.toml + background.png. The watcher
        // picks it up; get serves the image as a data: URL.
        let before = events();
        let dir = zen_dir(&home_dir).join("themes/sunset");
        std::fs::create_dir_all(&dir).expect("mkdir sunset");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0u8; 64]);
        std::fs::write(dir.join("background.png"), &png).expect("png");
        std::fs::write(
            dir.join("theme.toml"),
            "schema = 1\n[colors.dark]\naccent = \"#ff9e64\"\n[background]\nimage = \"background.png\"\nfit = \"tile\"\nopacity = 0.5\n",
        )
        .expect("theme.toml");
        let l = wait_for("the watcher to list sunset", Duration::from_secs(5), || {
            let (_, l) = call(port, "GET", &format!("/cli/zen/theme/list?token={tok}"), None);
            l["themes"].as_array().is_some_and(|t| t.iter().any(|x| x["name"] == "sunset")).then_some(l)
        });
        let names: Vec<&str> = l["themes"].as_array().expect("themes").iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["basic", "paper", "midnight", "sunset"], "user themes follow the built-ins: {l}");
        // theme/list doesn't re-read; only the watcher can announce this.
        wait_for("the watcher to announce the new bundle", Duration::from_secs(5), || {
            (events() > before).then_some(())
        });
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/set?token={tok}"), Some(r#"{"name":"sunset"}"#));
        assert_eq!(s, 200, "{v}");
        let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={gid}"), None);
        let bg = &g["theme"]["background"];
        assert!(bg["dataUrl"].as_str().is_some_and(|u| u.starts_with("data:image/png;base64,")), "{bg}");
        assert_eq!(bg["mime"], "image/png", "{bg}");
        assert_eq!(bg["fit"], "tile", "{bg}");
        assert_eq!(bg["opacity"], 0.5, "{bg}");
        assert_eq!(bg["lastGood"], false, "{bg}");
        assert_eq!(g["theme"]["builtin"], false, "{g}");
        assert_eq!(g["theme"]["tokens"]["colors"]["dark"]["accent"], "#ff9e64", "{g}");
        assert_eq!(g["errors"], serde_json::json!([]), "{g}");

        // An oversize image: error with file:line, the last good image stays.
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(2 * 1024 * 1024 + 1, 0);
        std::fs::write(dir.join("background.png"), &big).expect("big png");
        let g2 = wait_for("the oversize image to be reported", Duration::from_secs(5), || {
            let (_, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={gid}"), None);
            (g["errors"].as_array().is_some_and(|e| !e.is_empty())).then_some(g)
        });
        let e = &g2["errors"][0];
        assert_eq!(e["file"], "themes/sunset/theme.toml", "{g2}");
        assert_eq!(e["line"], 5, "the error sits on the image line: {g2}");
        assert!(e["message"].as_str().is_some_and(|m| m.contains("the limit is 2097152 bytes")), "{g2}");
        assert_eq!(g2["theme"]["background"]["lastGood"], true, "{g2}");
        assert_eq!(g2["theme"]["background"]["dataUrl"], bg["dataUrl"], "the last good image stays: {g2}");
        let (_, v) = call(port, "GET", &format!("/cli/zen/validate?token={tok}&theme=sunset"), None);
        assert_eq!(v["ok"], false, "validate reports the image: {v}");

        // theme/new: a copy of basic, never overwrites.
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/new?token={tok}"), Some(r#"{"name":"mine"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["from"], "basic", "{v}");
        assert!(zen_dir(&home_dir).join("themes/mine/theme.toml").is_file());
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/new?token={tok}"), Some(r#"{"name":"mine"}"#));
        assert_eq!(s, 409, "{v}");
        assert_eq!(v["error"], "theme_exists", "{v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/theme/new?token={tok}"), Some(r#"{"name":"Bad Name"}"#));
        assert_eq!(s, 400, "{v}");
    })
    .await
    .expect("blocking body");
    collector.abort();
}

/// Rosson 2026-10-04, "Start with the default", headless: POST
/// `garden/template` turns Garden 2 into Garden 1's texting page with the
/// same id, name and place, keeps the old file in `.history/`, emits ONE
/// `zen_changed`, and is idempotent (no event the second time). GET is
/// 405, an unknown Garden 404, a file with its own widgets 409 without
/// `force`; Connect logins and no token are refused; keep-alive holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garden_template_route_starts_a_garden_with_the_default_headless() {
    let home = new_home();
    let d = spawn_daemon(&home.0);
    let tok = d.owner.clone();
    let port = d.port;

    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Garden 2","template":"texting"}"#));
    assert_eq!(s, 404, "not set up: {v}");
    assert_eq!(v["error"], "zen_not_set_up", "{v}");
    assert!(!zen_dir(&home.0).exists(), "garden/template never creates ~/.k2/zen");

    let (v, one) = setup(port, &tok);
    let two = v["gardens"][1]["id"].as_str().expect("Garden 2 id").to_string();
    let (events, collector) = zen_event_counter(port, &tok).await;
    let home_dir = home.0.clone();
    let tok2 = tok.clone();
    tokio::task::spawn_blocking(move || {
        let tok = tok2;
        let n = || events.load(Ordering::SeqCst);
        let page_file = zen_dir(&home_dir).join(format!("gardens/{two}.toml"));
        let stub = std::fs::read_to_string(&page_file).expect("Garden 2 stub");

        // GET is 405 and changes nothing.
        let (s, b) = Conn::open(port).request("GET", &format!("/cli/zen/garden/template?token={tok}&garden={two}&template=texting"), None);
        assert_eq!(s, 405, "{b}");
        assert_eq!(std::fs::read_to_string(&page_file).expect("file"), stub, "a GET wrote nothing");

        // Start with the default: one event, same id/name/index, texting page.
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Garden 2","template":"texting"}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["changed"], true, "{v}");
        assert_eq!(v["template"], "k2.texting@1", "{v}");
        assert_eq!(v["garden"]["id"], two.as_str(), "{v}");
        assert_eq!(v["garden"]["name"], "Garden 2", "{v}");
        assert_eq!(v["garden"]["index"], 2, "{v}");
        assert_eq!(v["garden"]["template"], "k2.texting@1", "{v}");
        assert_eq!(v["file"], format!("gardens/{two}.toml"), "{v}");
        assert_eq!(v["replaced"], serde_json::json!([]), "{v}");
        expect_one_event(&events, before, "a template switch");
        let snap = v["snapshot"].as_str().unwrap_or_else(|| panic!("no snapshot: {v}")).to_string();
        let kept = zen_dir(&home_dir).join(format!(".history/gardens/{two}.toml/{snap}"));
        assert_eq!(std::fs::read_to_string(&kept).expect("snapshot"), stub, "history holds the old file");
        let (s, g) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={two}"), None);
        assert_eq!(s, 200, "{g}");
        assert_eq!(g["page"]["template"], "k2.texting@1", "{g}");
        let kinds: Vec<&str> = g["page"]["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["kind"].as_str()).collect();
        assert_eq!(kinds, vec!["agents", "conversation", "nav-rail"], "Garden 1's layout: {g}");
        assert_eq!(g["garden"], serde_json::json!({"id": two, "name": "Garden 2", "index": 2}), "{g}");
        assert_eq!(g["errors"], serde_json::json!([]), "{g}");
        assert_eq!(g["warnings"], serde_json::json!([]), "{g}");
        let (_, g1) = call(port, "GET", &format!("/cli/zen/get?token={tok}&garden={one}"), None);
        assert_eq!(g["page"], g1["page"], "the same page as Garden 1");
        let (_, l) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
        let ids: Vec<&str> = l["gardens"].as_array().expect("gardens").iter().filter_map(|x| x["id"].as_str()).collect();
        assert_eq!(ids, vec![one.as_str(), two.as_str()], "the list keeps its order: {l}");
        let (_, h) = call(port, "GET", &format!("/cli/zen/history?token={tok}&garden={two}"), None);
        assert!(h["files"][0]["snapshots"].as_array().is_some_and(|s| s.iter().any(|x| x["name"] == snap.as_str())), "{h}");

        // Idempotent: 200, changed:false, no event.
        let before = n();
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(&format!(r#"{{"garden":"{two}","template":"texting"}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["changed"], false, "{v}");
        assert!(v["snapshot"].is_null(), "{v}");
        std::thread::sleep(Duration::from_millis(1000));
        assert_eq!(n(), before, "a no-op emits nothing");

        // Bad input.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Nowhere","template":"texting"}"#));
        assert_eq!(s, 404, "{v}");
        assert_eq!(v["error"], "unknown_garden", "{v}");
        for body in [
            r#"{"garden":"Garden 2","template":"dashboard"}"#,
            r#"{"garden":"Garden 2"}"#,
            r#"{"garden":"Garden 2","template":"texting","home":"x"}"#,
            "{}",
        ] {
            let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(body));
            assert_eq!(s, 400, "{body}: {v}");
        }

        // A Garden with its own widgets: 409 without force, then force.
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/new?token={tok}"), Some(r#"{"name":"Built"}"#));
        assert_eq!(s, 200, "{v}");
        let built = v["garden"]["id"].as_str().expect("id").to_string();
        let built_file = zen_dir(&home_dir).join(format!("gardens/{built}.toml"));
        let mine = "schema = 1\n[[widget]]\nid = \"work\"\nkind = \"agents\"\ncolumn = 0\n";
        std::fs::write(&built_file, mine).expect("save page");
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Built","template":"texting"}"#));
        assert_eq!(s, 409, "{v}");
        assert_eq!(v["error"], "garden_has_changes", "{v}");
        assert_eq!(v["keys"], serde_json::json!(["widget"]), "{v}");
        assert_eq!(std::fs::read_to_string(&built_file).expect("file"), mine, "a 409 writes nothing");
        let (s, v) = call(port, "POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Built","template":"texting","force":true}"#));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["replaced"], serde_json::json!(["widget"]), "{v}");
        let kept = zen_dir(&home_dir).join(format!(".history/gardens/{built}.toml/{}", v["snapshot"].as_str().expect("snapshot")));
        assert_eq!(std::fs::read_to_string(kept).expect("snapshot"), mine, "the replaced page is in history");

        // Refused: no token, and Connect logins of every role.
        let body = r#"{"garden":"Built","template":"blank"}"#;
        let (s, b) = Conn::open(port).request("POST", "/cli/zen/garden/template", Some(body));
        assert_eq!(s, 403, "no token: {b}");
        for (user, role) in [("tmember", "member"), ("tadmin", "admin"), ("towner", "owner")] {
            let session = login_as(port, &tok, user, role);
            let (s, b) = Conn::open(port).request("POST", &format!("/cli/zen/garden/template?token={session}"), Some(body));
            assert_eq!(s, 403, "{role} login must be refused: {b}");
            assert_eq!(json(&b, "garden/template")["error"], "zen_local_only", "{role}: {b}");
        }
        let (_, l) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
        let built_t = l["gardens"].as_array().expect("gardens").iter().find(|g| g["id"] == built.as_str()).expect("Built")["template"].clone();
        assert_eq!(built_t, "k2.texting@1", "refused requests wrote nothing: {l}");

        // Keep-alive: the request after garden/template gets its own body.
        let mut c = Conn::open(port);
        let (s, b) = c.request("POST", &format!("/cli/zen/garden/template?token={tok}"), Some(r#"{"garden":"Built","template":"blank"}"#));
        assert_eq!(s, 200, "{b}");
        assert_eq!(json(&b, "garden/template")["template"], "k2.blank@1", "{b}");
        let (s, b) = c.request("GET", &format!("/cli/zen/gardens?token={tok}"), None);
        assert_eq!(s, 200, "{b}");
        assert!(json(&b, "gardens")["gardens"].is_array(), "the request after garden/template got the wrong body: {b}");
    })
    .await
    .expect("blocking body");
    collector.abort();
}
