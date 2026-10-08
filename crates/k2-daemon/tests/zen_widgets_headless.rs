//! Zen v2 custom widgets, headless (prd-zen-user-widgets-v2 TUW1.1,
//! TUW1.4, TUW1.6, TUW2.1, TUWB1, TUWB2, TUWB4 daemon side, TUWB6).
//!
//! 1. The REAL `k2-daemon` binary under a temp HOME, no client: widget/new,
//!    the bundle round trip (stable hash, fresh nonce), a custom placement
//!    in `get`, one `zen_changed` per widget save, the templates list with
//!    the Diary, 405 on GET of every POST row, Connect logins refused.
//! 2. The in-process dispatcher (`test_harness`) under a temp HOME, so the
//!    test can mint an agent passport: widgets need no permissions (Rosson
//!    2026-10-08), so a new widget and a new Diary run with every
//!    Garden-safe cap at once; the old grant routes are gone; the runaway
//!    pause shows on the widget and only a person's Resume clears it.
//!
//! Never touches the real `~/.k2`. Fixtures use made-up names only.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use k2_core::connect_users;
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value as J};
use tokio_tungstenite::tungstenite::Message;

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn new_home(tag: &str) -> Home {
    let home = std::env::temp_dir().join(format!("k2-zen-widgets-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
    std::fs::create_dir_all(home.join("agent-shim-empty")).expect("shim dir");
    Home(home)
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
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{path}: bad status line {status_line:?}"));
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

fn json_of(body: &str, what: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn call(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, J) {
    let (s, b) = Conn::try_open(port).expect("connect daemon").request(method, path, body);
    (s, json_of(&b, path))
}

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

const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 60\nmin-width = 200\n[[layout.column]]\nsize = 40\nmin-width = 200\n\n[[widget]]\nid = \"arcade\"\nkind = \"custom\"\nwidget = \"agent-arcade\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n[widget.props]\nagents = \"arcade\"\n";

const POST_ROWS: &[&str] = &["/cli/zen/widget/new", "/cli/zen/widget/pause", "/cli/zen/widget/resume"];

/// The grant routes of the first Zen v2 build: gone (Rosson 2026-10-08).
/// A POST to a path with no policy row is 405 at the method guard; a GET is
/// 404 (no route).
const GONE_ROUTES: &[(&str, &str, u16)] = &[
    ("POST", "/cli/zen/widget/grant", 405),
    ("POST", "/cli/zen/widget/revoke", 405),
    ("POST", "/cli/zen/widget/sending", 405),
    ("GET", "/cli/zen/widget/grants", 404),
];

// ── 1. The real binary ──────────────────────────────────────────────────

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
        .env_remove("K2_HOOK_SOCK")
        .env_remove("K2_HOOK_TOKEN")
        .env_remove("K2SO_HOOK_SOCK")
        .env_remove("K2SO_HOOK_TOKEN")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let token_file = home.join(".k2").join("daemon.token");
    let (port, owner) = wait_for("daemon.port + daemon.token", Duration::from_secs(20), || {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok())?;
        Some((port, read_trimmed(&token_file)?))
    });
    let d = Daemon { child, port, owner };
    wait_for("boot-status ready", Duration::from_secs(30), || {
        let mut c = Conn::try_open(d.port).ok()?;
        let (s, b) = c.request("GET", "/boot-status", None);
        (s == 200 && json_of(&b, "boot-status")["phase"] == "ready").then_some(())
    });
    d
}

async fn zen_event_counter(port: u16, tok: &str) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={tok}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("hello in time");
    assert!(matches!(hello, Some(Ok(Message::Text(_)))), "first frame must be hello: {hello:?}");
    let n = Arc::new(AtomicUsize::new(0));
    let counter = n.clone();
    let task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                let v: J = serde_json::from_str(&t).expect("frame JSON");
                if v["kind"] == "zen_changed" {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    (n, task)
}

/// Exactly `before + 1` events, held for a 1 s quiet window.
fn expect_one_event(events: &AtomicUsize, before: usize, what: &str) {
    wait_for(&format!("one zen_changed for {what}"), Duration::from_secs(4), || {
        (events.load(Ordering::SeqCst) > before).then_some(())
    });
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(events.load(Ordering::SeqCst), before + 1, "{what} must emit exactly one zen_changed");
}

/// TUW1.1, TUW1.4, TUW1.6, TUWB6 over the real binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tuw1_widgets_round_trip_on_the_real_daemon() {
    // Serialized with the in-process tests: one of them sets K2_AIRGAP,
    // which a spawned daemon would inherit.
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = new_home("bin");
    let d = spawn_daemon(&home.0);
    let (port, tok) = (d.port, d.owner.clone());
    let zen = home.0.join(".k2").join("zen");

    let (_, boot) = call(port, "GET", "/boot-status", None);
    let features: Vec<&str> = boot["features"].as_array().expect("features").iter().filter_map(J::as_str).collect();
    let chrome = features.iter().position(|f| *f == "zen-chrome-v1").expect("zen-chrome-v1");
    assert_eq!(features.get(chrome + 1), Some(&"zen-widgets-v1"), "zen-widgets-v1 right after zen-chrome-v1: {features:?}");

    // Before setup: 404 zen_not_set_up.
    let (s, v) = call(port, "POST", &format!("/cli/zen/widget/new?token={tok}"), Some(r#"{"name":"w"}"#));
    assert_eq!((s, v["error"].as_str()), (404, Some("zen_not_set_up")), "{v}");

    let (s, v) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "{v}");
    let names: Vec<&str> = v["gardens"].as_array().expect("gardens").iter().filter_map(|g| g["name"].as_str()).collect();
    assert_eq!(names, vec!["Garden 1", "Garden 2", "Diary"], "the Diary is preinstalled for a new user");
    let garden2 = v["gardens"][1]["id"].as_str().expect("Garden 2 id").to_string();

    // GET on every POST row is 405, before any auth.
    for p in POST_ROWS {
        let (s, _) = Conn::try_open(port).expect("connect").request("GET", &format!("{p}?token={tok}"), None);
        assert_eq!(s, 405, "GET {p}");
    }

    // Templates: the two starts, then the Diary, with nothing to allow.
    let (s, v) = call(port, "GET", &format!("/cli/zen/templates?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    let shorts: Vec<&str> = v["templates"].as_array().expect("templates").iter().filter_map(|t| t["short"].as_str()).collect();
    assert_eq!(shorts, vec!["texting", "blank", "diary"]);
    assert!(v["templates"][2].get("needsGrant").is_none(), "no permissions: {v}");
    assert_eq!(v["templates"][2]["newUsers"], true);

    let (events, task) = zen_event_counter(port, &tok).await;
    std::thread::sleep(Duration::from_millis(500));

    // widget/new: one event; again → 409.
    let before = events.load(Ordering::SeqCst);
    let (s, v) = call(port, "POST", &format!("/cli/zen/widget/new?token={tok}"), Some(r#"{"name":"agent-arcade","from":"arcade"}"#));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["files"], json!(["arcade.css", "arcade.js", "index.html", "manifest.json"]));
    assert!(zen.join("widgets/agent-arcade/arcade.js").is_file());
    expect_one_event(&events, before, "widget/new");
    let (s, v) = call(port, "POST", &format!("/cli/zen/widget/new?token={tok}"), Some(r#"{"name":"agent-arcade"}"#));
    assert_eq!((s, v["error"].as_str()), (409, Some("widget_exists")), "{v}");

    // Bundle: nonce on every script, no src left, assets, stable hash.
    let bundle = |port: u16| call(port, "GET", &format!("/cli/zen/widget/bundle?widget=agent-arcade&token={tok}"), None);
    let (s, a) = bundle(port);
    assert_eq!(s, 200, "{a}");
    let (_, b) = bundle(port);
    let html = a["html"].as_str().expect("html");
    let nonce = a["nonce"].as_str().expect("nonce");
    assert_eq!(html.matches("<script").count(), html.matches(&format!("nonce=\"{nonce}\"")).count(), "{html}");
    assert!(!html.contains(" src=\"arcade.js\"") && !html.contains("href=\"arcade.css\""), "everything inlined");
    assert!(html.contains("window.K2_ASSETS"));
    assert_eq!(a["hash"], b["hash"], "the hash is stable");
    assert_ne!(a["nonce"], b["nonce"], "the nonce is fresh per response");
    assert_eq!(a["bytes"].as_u64(), Some(html.len() as u64));
    let (s, v) = call(port, "GET", &format!("/cli/zen/widget/bundle?widget=nope&token={tok}"), None);
    assert_eq!((s, v["error"].as_str()), (404, Some("unknown_widget")), "{v}");

    // Place it: get shows the custom widget, working at once (no grant).
    let before = events.load(Ordering::SeqCst);
    std::fs::write(zen.join(format!("gardens/{garden2}.toml")), PAGE).expect("write Garden 2");
    expect_one_event(&events, before, "a Garden save placing the widget");
    let (s, g) = call(port, "GET", &format!("/cli/zen/get?garden={garden2}&token={tok}"), None);
    assert_eq!(s, 200, "{g}");
    assert_eq!(g["errors"], json!([]), "{g}");
    let w = &g["page"]["widgets"][0];
    assert_eq!(w["kind"], "custom");
    assert_eq!(w["source"], "user");
    assert_eq!(w["requested"], json!(["agents:read", "thread:read", "thread:post"]));
    assert_eq!(w["caps"], json!(["agents:read", "thread:read", "thread:post"]), "zero clicks: {w}");
    assert_eq!(w["origin"], "local");
    assert_eq!(w["paused"], J::Null);
    assert!(w.get("grant").is_none(), "{w}");
    assert_eq!(w["hash"], a["hash"]);

    // TUW1.4: a JS save → one event; the same bytes → none; a manifest
    // save → one; an asset add → one; 500 writes → one.
    let js = zen.join("widgets/agent-arcade/arcade.js");
    let mut text = std::fs::read_to_string(&js).expect("read js");
    for (i, what) in ["a JS save", "a burst of 500 writes"].iter().enumerate() {
        let before = events.load(Ordering::SeqCst);
        if i == 0 {
            text.push_str("\n// save 1\n");
            std::fs::write(&js, &text).expect("save js");
        } else {
            for n in 0..500 {
                std::fs::write(&js, format!("{text}\n// burst {n}\n")).expect("burst");
            }
        }
        expect_one_event(&events, before, what);
    }
    let before = events.load(Ordering::SeqCst);
    let same = std::fs::read(&js).expect("read");
    std::fs::write(&js, same).expect("same bytes");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(events.load(Ordering::SeqCst), before, "the same bytes emit nothing");
    let before = events.load(Ordering::SeqCst);
    let m = zen.join("widgets/agent-arcade/manifest.json");
    let mt = std::fs::read_to_string(&m).expect("manifest").replace("Agent Arcade", "Agent Arcade 2");
    std::fs::write(&m, mt).expect("manifest save");
    expect_one_event(&events, before, "a manifest save");
    let before = events.load(Ordering::SeqCst);
    std::fs::write(zen.join("widgets/agent-arcade/cat.png"), b"\x89PNG\r\n\x1a\n0000").expect("asset");
    expect_one_event(&events, before, "an asset add");

    // validate / history for one widget.
    let (s, v) = call(port, "GET", &format!("/cli/zen/validate?widget=agent-arcade&token={tok}"), None);
    assert_eq!((s, v["ok"].as_bool()), (200, Some(true)), "{v}");
    let (s, v) = call(port, "GET", &format!("/cli/zen/history?widget=agent-arcade&token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert!(v["files"][0]["snapshots"].as_array().is_some_and(|a| a.len() >= 3), "each good save is kept: {v}");
    let (s, v) = call(port, "GET", &format!("/cli/zen/widgets?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    let arcade = v["widgets"].as_array().expect("widgets").iter().find(|w| w["name"] == "agent-arcade").cloned().expect("listed");
    assert_eq!(arcade["placements"][0]["placement"], "arcade", "{arcade}");
    assert_eq!(arcade["placements"][0]["paused"], J::Null, "{arcade}");
    assert!(arcade["placements"][0].get("grant").is_none(), "{arcade}");

    // Connect logins (every role) never reach Zen; Resume (a person's
    // click) says owner_only, the rest zen_local_only.
    for role in ["member", "admin", "owner"] {
        let user = format!("u-{role}");
        let (s, v) = call(port, "POST", &format!("/cli/users/add?token={tok}"), Some(&format!(r#"{{"username":"{user}","password":"correct-horse-battery-9"}}"#)));
        assert_eq!(s, 200, "{v}");
        if role != "member" {
            let (s, v) = call(port, "POST", &format!("/cli/users/set-role?token={tok}"), Some(&format!(r#"{{"username":"{user}","role":"{role}"}}"#)));
            assert_eq!(s, 200, "{v}");
        }
        let (s, v) = call(port, "POST", "/cli/auth/login", Some(&format!(r#"{{"username":"{user}","password":"correct-horse-battery-9"}}"#)));
        assert_eq!(s, 200, "{v}");
        let session = v["token"].as_str().expect("session").to_string();
        let (s, v) = call(port, "GET", &format!("/cli/zen/widgets?token={session}"), None);
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "{role}: {v}");
        let (s, v) = call(port, "POST", &format!("/cli/zen/widget/resume?token={session}"), Some("{}"));
        assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "{role}: {v}");
    }
    // 65 KB body, sent in full → 413. The daemon answers from the declared
    // length, then drains the rest of the body before closing, so the whole
    // write succeeds and the 413 is read (no reset).
    let big = format!("{{\"name\":\"{}\"}}", "x".repeat(65 * 1024));
    let (s, _) = Conn::try_open(port).expect("connect").request("POST", &format!("/cli/zen/widget/new?token={tok}"), Some(&big));
    assert_eq!(s, 413);
    task.abort();
}

// ── 2. In-process: no permissions, the runaway pause ────────────────────

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER_TOKEN: &str = "zen-widgets-owner-token-0001";

fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!("k2-zen-widgets-inproc-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    let zen = k2_core::zen::zen_root();
    assert!(zen.starts_with(&tmp), "the Zen root must be under the temp HOME: {}", zen.display());
    f(&tmp);
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn mint_passport() -> String {
    session_token::mint_session_token(
        &SessionId::new(),
        "pane-zen-widgets",
        HookPrincipal { workspace_uuid: "ws-zen-widgets".into(), agent_address: "agent-zen-widgets".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(username: &str) -> String {
    connect_users::add_user(username, "password123").unwrap_or_else(|e| panic!("add_user {username}: {e:?}"));
    connect_users::set_role(username, connect_users::Role::Owner).unwrap_or_else(|e| panic!("set_role: {e:?}"));
    connect_users::create_session(username)
}

/// Rosson 2026-10-08: no permissions. A new widget and a new Diary run with
/// every Garden-safe cap at once; the grant routes are gone and no key is
/// made; the runaway pause shows on the widget, anything Zen accepts may
/// set it, and only a person's Resume (the owner token) clears it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn widgets_work_with_zero_clicks_and_only_a_person_resumes_a_pause() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let tok = OWNER_TOKEN;
        let owner = |m: &str, p: &str, b: Option<&str>| {
            let sep = if p.contains('?') { '&' } else { '?' };
            call(port, m, &format!("{p}{sep}token={tok}"), b)
        };
        let (s, v) = owner("POST", "/cli/zen/setup", Some("{}"));
        assert_eq!(s, 200, "{v}");
        let garden = v["gardens"][1]["id"].as_str().expect("Garden 2").to_string();
        let diary = v["gardens"][2]["id"].as_str().expect("the preinstalled Diary").to_string();
        let (s, v) = owner("POST", "/cli/zen/widget/new", Some(r#"{"name":"agent-arcade","from":"arcade"}"#));
        assert_eq!(s, 200, "{v}");
        std::fs::write(home.join(format!(".k2/zen/gardens/{garden}.toml")), PAGE).expect("place");

        // Zero clicks: the new widget and the preinstalled Diary.
        let all = json!(["agents:read", "thread:read", "thread:post"]);
        for (id, what) in [(&garden, "a new widget"), (&diary, "the Diary")] {
            let (s, g) = owner("GET", &format!("/cli/zen/get?garden={id}"), None);
            assert_eq!(s, 200, "{g}");
            let w = &g["page"]["widgets"][0];
            assert_eq!(w["caps"], all, "{what} posts with zero clicks: {w}");
            assert_eq!((w["origin"].as_str(), w["paused"].is_null()), (Some("local"), true), "{what}: {w}");
            assert!(w.get("grant").is_none(), "{what}: {w}");
        }
        // A Diary from New Garden: no grant to send, works at once.
        let (s, v) = owner("POST", "/cli/zen/garden/new", Some(r#"{"name":"My diary","template":"diary"}"#));
        assert_eq!(s, 200, "{v}");
        assert!(v.get("grant").is_none(), "{v}");
        let mine = v["garden"]["id"].as_str().expect("id").to_string();
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={mine}"), None);
        assert_eq!(g["page"]["widgets"][0]["caps"], all, "{g}");
        // The old grant field is refused, not silently honoured.
        let (s, v) = owner("POST", "/cli/zen/garden/new", Some(r#"{"name":"D2","template":"diary","grant":{"scope":{"server":"local"}}}"#));
        assert_eq!((s, v["error"].as_str()), (400, Some("bad_request")), "{v}");

        // The grant routes are gone; nothing makes a key.
        for (m, p, status) in GONE_ROUTES {
            let (s, v) = owner(m, p, if *m == "POST" { Some("{}") } else { None });
            assert_eq!(s, *status, "{m} {p} is gone: {v}");
        }
        assert!(!home.join(".k2/zen-grant.key").exists(), "no grant key any more");

        // The runaway pause: bad bodies first.
        let pause = format!(r#"{{"garden":"{garden}","placement":"arcade","reason":"runaway"}}"#);
        let resume = format!(r#"{{"garden":"{garden}","placement":"arcade"}}"#);
        for (b, status, code) in [
            (pause.replace("runaway", "bored"), 400, "bad_request"),
            (pause.replace("\"placement\":\"arcade\"", "\"placement\":\"talk\""), 404, "not_found"),
            (r#"{"garden":"g-nope0001","placement":"arcade","reason":"runaway"}"#.to_string(), 404, "unknown_garden"),
        ] {
            let (s, v) = owner("POST", "/cli/zen/widget/pause", Some(&b));
            assert_eq!((s, v["error"].as_str()), (status, Some(code)), "{b}: {v}");
        }
        // The guard trips (the renderer posts it with Zen's owner token).
        let (s, v) = owner("POST", "/cli/zen/widget/pause", Some(&pause));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["paused"]["reason"], "runaway", "{v}");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        let w = &g["page"]["widgets"][0];
        assert_eq!(w["paused"]["reason"], "runaway", "the pause shows on the widget: {w}");
        assert_eq!(w["caps"], all, "a pause never takes caps away: {w}");
        let (_, v) = owner("GET", "/cli/zen/widgets", None);
        let arcade = v["widgets"].as_array().expect("widgets").iter().find(|w| w["name"] == "agent-arcade").cloned().expect("listed");
        assert_eq!(arcade["placements"][0]["paused"]["reason"], "runaway", "{arcade}");

        // Only a person resumes: passport, Connect login, app pass, API key → 403.
        let passport = mint_passport();
        assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
        let session = login("zen-owner-login");
        for (who, cred) in [("passport", passport.as_str()), ("connect login", session.as_str()), ("app pass", "k2skn_notreal"), ("api key", "k2sk_notreal")] {
            let (s, v) = call(port, "POST", &format!("/cli/zen/widget/resume?token={cred}"), Some(&resume));
            assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "{who}: {v}");
        }
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["paused"]["reason"], "runaway", "still paused after the refusals");
        // An agent can't reach Zen at all, pause included.
        let (s, v) = call(port, "POST", &format!("/cli/zen/widget/pause?token={passport}"), Some(&pause));
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "{v}");

        // The person's one click.
        let (s, v) = owner("POST", "/cli/zen/widget/resume", Some(&resume));
        assert_eq!(s, 200, "{v}");
        assert_eq!((v["resumed"].as_bool(), v["changed"].as_bool()), (Some(true), Some(true)), "{v}");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["paused"], J::Null, "{g}");
        let (s, v) = owner("POST", "/cli/zen/widget/resume", Some(&resume));
        assert_eq!((s, v["resumed"].as_bool(), v["changed"].as_bool()), (200, Some(false), Some(false)), "resume again is a no-op: {v}");

        let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit log");
        for line in ["zen.widget.pause", "zen.widget.resume", "zen.widget.refused"] {
            assert!(audit.contains(line), "{line} missing: {audit}");
        }
    });
}

// ── 3. In-process: the widget library routes (B3's, hooked here) ───────

fn query_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// UWB15 through `zen_routes::handle`: owner only, GET on fetch is 405,
/// and under `K2_AIRGAP` nothing is downloaded but a cached file is still
/// served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uwb15_lib_routes_refuse_downloads_air_gapped_and_serve_the_cache() {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let tok = OWNER_TOKEN;
        let bytes: &[u8] = b"window.zenLibTest = 1;\n";
        let digest = Sha256::digest(bytes);
        let integrity = format!("sha256-{}", base64::engine::general_purpose::STANDARD.encode(digest));
        let url = "https://cdn.jsdelivr.net/npm/zen-lib-test@1.0.0/dist/test.js";
        let fetch = json!({ "url": url, "integrity": integrity }).to_string();
        let file_q = format!("/cli/zen/lib/file?integrity={}", query_encode(&integrity));

        // GET on the POST row → 405; an agent passport → zen_local_only.
        let (s, _) = Conn::try_open(port).expect("connect").request("GET", &format!("/cli/zen/lib/fetch?token={tok}"), None);
        assert_eq!(s, 405);
        let (s, v) = call(port, "POST", &format!("/cli/zen/lib/fetch?token={}", mint_passport()), Some(&fetch));
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "{v}");

        std::env::set_var("K2_AIRGAP", "1");
        let (s, v) = call(port, "POST", &format!("/cli/zen/lib/fetch?token={tok}"), Some(&fetch));
        assert_eq!((s, v["error"].as_str()), (403, Some("airgapped")), "{v}");
        let (s, v) = call(port, "GET", &format!("{file_q}&token={tok}"), None);
        assert_eq!((s, v["error"].as_str()), (404, Some("not_cached")), "{v}");

        // A file already in the cache is still served, and fetch says cached.
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let cached = home.join(".k2/cache/zen-lib/cdn").join(format!("sha256-{hex}.js"));
        std::fs::create_dir_all(cached.parent().expect("parent")).expect("mkdir cache");
        std::fs::write(&cached, bytes).expect("seed cache");
        let (s, body) = Conn::try_open(port).expect("connect").request("GET", &format!("{file_q}&token={tok}"), None);
        assert_eq!(s, 200, "{body}");
        assert_eq!(body.as_bytes(), bytes, "the cached bytes, as checked");
        let (s, v) = call(port, "POST", &format!("/cli/zen/lib/fetch?token={tok}"), Some(&fetch));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["cached"], true, "{v}");

        // A tampered cache file is refused and deleted, air-gapped or not.
        std::fs::write(&cached, b"window.evil = 1;\n").expect("tamper");
        let (s, v) = call(port, "GET", &format!("{file_q}&token={tok}"), None);
        assert_eq!((s, v["error"].as_str()), (409, Some("hash_mismatch")), "{v}");
        assert!(!cached.exists(), "a file that no longer matches its pin is deleted");
        std::env::remove_var("K2_AIRGAP");
    });
}

/// A client that sends an oversized body IN FULL reads the 413, never a
/// connection reset: the daemon answers from the declared length, then
/// reads and discards the rest of the body before it closes. 20 rounds per
/// route, a fresh connection each, so a race shows up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_full_body_reads_413_not_a_reset() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|_home| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let zen = format!("{{\"name\":\"{}\"}}", "x".repeat(65 * 1024));
        let brief = format!("{{\"title\":\"t\",\"briefHtml\":\"{}\"}}", "y".repeat(3 * 1024 * 1024));
        for (path, body) in [
            (format!("/cli/zen/widget/new?token={OWNER_TOKEN}"), &zen),
            (format!("/cli/feedback/create?token={OWNER_TOKEN}"), &brief),
        ] {
            for round in 1..=20 {
                let mut c = Conn::try_open(d.port).expect("connect");
                let head = format!(
                    "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let stream = c.reader.get_mut();
                stream.write_all(head.as_bytes()).unwrap_or_else(|e| panic!("{path} round {round}: head: {e:?}"));
                stream
                    .write_all(body.as_bytes())
                    .unwrap_or_else(|e| panic!("{path} round {round}: the full body must be accepted, got {e:?}"));
                let mut status_line = String::new();
                c.reader
                    .read_line(&mut status_line)
                    .unwrap_or_else(|e| panic!("{path} round {round}: read the answer: {e:?}"));
                assert!(
                    status_line.starts_with("HTTP/1.1 413"),
                    "{path} round {round}: {} byte body: {status_line:?}",
                    body.len()
                );
                // The rest of the answer, then a clean EOF (not a reset).
                let mut rest = Vec::new();
                c.reader
                    .read_to_end(&mut rest)
                    .unwrap_or_else(|e| panic!("{path} round {round}: after the 413: {e:?}"));
                let rest = String::from_utf8_lossy(&rest);
                assert!(rest.contains("too_large"), "{path} round {round}: 413 body: {rest}");
            }
        }
    });
}
