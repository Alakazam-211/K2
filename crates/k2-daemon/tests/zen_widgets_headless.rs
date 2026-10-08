//! Zen v2 custom widgets, headless (prd-zen-user-widgets-v2 TUW1.1,
//! TUW1.4, TUW1.6, TUW2.1, TUWB1, TUWB2, TUWB4 daemon side, TUWB6).
//!
//! 1. The REAL `k2-daemon` binary under a temp HOME, no client: widget/new,
//!    the bundle round trip (stable hash, fresh nonce), a custom placement
//!    in `get`, one `zen_changed` per widget save, the templates list with
//!    the Diary, 405 on GET of every POST row, Connect logins refused.
//! 2. The in-process dispatcher (`test_harness`) under a temp HOME, so the
//!    test can mint an agent passport and edit the grants table: the grant
//!    routes take only the owner token, the daemon signs rows, a hand-edited
//!    row is `invalid`, Sending off/on, the runaway pause and resume,
//!    revoke, `garden/new` with a grant.
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

const POST_ROWS: &[&str] = &[
    "/cli/zen/widget/grant",
    "/cli/zen/widget/new",
    "/cli/zen/widget/resume",
    "/cli/zen/widget/revoke",
    "/cli/zen/widget/sending",
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

    // Templates: the two starts, then the Diary with its grant.
    let (s, v) = call(port, "GET", &format!("/cli/zen/templates?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    let shorts: Vec<&str> = v["templates"].as_array().expect("templates").iter().filter_map(|t| t["short"].as_str()).collect();
    assert_eq!(shorts, vec!["texting", "blank", "diary"]);
    assert_eq!(v["templates"][2]["needsGrant"]["widget"], "k2:diary@1");
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

    // Place it: get shows the custom widget, no grant yet.
    let before = events.load(Ordering::SeqCst);
    std::fs::write(zen.join(format!("gardens/{garden2}.toml")), PAGE).expect("write Garden 2");
    expect_one_event(&events, before, "a Garden save placing the widget");
    let (s, g) = call(port, "GET", &format!("/cli/zen/get?garden={garden2}&token={tok}"), None);
    assert_eq!(s, 200, "{g}");
    assert_eq!(g["errors"], json!([]), "{g}");
    let w = &g["page"]["widgets"][0];
    assert_eq!(w["kind"], "custom");
    assert_eq!(w["source"], "user");
    assert_eq!(w["caps"], json!([]));
    assert_eq!(w["requested"], json!(["agents:read", "thread:read", "thread:post"]));
    assert_eq!(w["grant"], J::Null);
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
    assert_eq!(arcade["placements"][0]["grant"], "none", "{arcade}");

    // Connect logins (every role) never reach Zen; the grant routes say
    // owner_only, the rest zen_local_only.
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
        let (s, v) = call(port, "POST", &format!("/cli/zen/widget/grant?token={session}"), Some("{}"));
        assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "{role}: {v}");
    }
    // 65 KB body → 413.
    let big = format!("{{\"name\":\"{}\"}}", "x".repeat(65 * 1024));
    let (s, _) = Conn::try_open(port).expect("connect").request("POST", &format!("/cli/zen/widget/new?token={tok}"), Some(&big));
    assert_eq!(s, 413);
    task.abort();
}

// ── 2. In-process: grants ───────────────────────────────────────────────

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

/// TUW2.1, TUWB1, TUWB2, TUWB4 (daemon side), TUWB6.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tuwb_grants_are_owner_only_signed_and_fail_closed() {
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
        let (s, v) = owner("POST", "/cli/zen/widget/new", Some(r#"{"name":"agent-arcade","from":"arcade"}"#));
        assert_eq!(s, 200, "{v}");
        std::fs::write(home.join(format!(".k2/zen/gardens/{garden}.toml")), PAGE).expect("place");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        let hash = g["page"]["widgets"][0]["hash"].as_str().expect("hash").to_string();

        let body = |caps: &str, entries: &str, h: &str| {
            format!(
                r#"{{"garden":"{garden}","placement":"arcade","widget":"agent-arcade","caps":{caps},"scope":{{"home":"h-test0001"}},"entries":{entries},"hash":"{h}"}}"#
            )
        };
        let one = r#"[{"server":"alice.example.test","room":"cortana"}]"#;
        let good = body(r#"["agents:read","thread:read"]"#, one, &hash);

        // TUWB1: passport (agent), Connect login, app pass, API key → 403.
        let passport = mint_passport();
        assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
        let session = login("zen-owner-login");
        for (who, cred) in [("passport", passport.as_str()), ("connect login", session.as_str()), ("app pass", "k2skn_notreal"), ("api key", "k2sk_notreal")] {
            for (p, b) in [
                ("/cli/zen/widget/grant", good.clone()),
                ("/cli/zen/widget/resume", format!(r#"{{"garden":"{garden}","placement":"arcade"}}"#)),
                ("/cli/zen/widget/sending", format!(r#"{{"garden":"{garden}","placement":"arcade","on":true}}"#)),
                ("/cli/zen/garden/new", r#"{"name":"D2","template":"diary","grant":{"scope":{"allHomes":true}}}"#.to_string()),
            ] {
                let (s, v) = call(port, "POST", &format!("{p}?token={cred}"), Some(&b));
                assert_eq!(s, 403, "{who} {p}: {v}");
                assert_eq!(v["error"], "owner_only", "{who} {p}: {v}");
            }
        }
        let (_, gs) = owner("GET", "/cli/zen/gardens", None);
        assert!(!gs["gardens"].as_array().expect("gardens").iter().any(|g| g["name"] == "D2"), "a refused create leaves nothing");

        // Bad bodies.
        for (b, status, code) in [
            (body(r#"["agents:read"]"#, one, "stale"), 409, "widget_changed"),
            (body(r#"["gardens:manage"]"#, one, &hash), 400, "bad_request"),
            (body(r#"["presence:read"]"#, one, &hash), 400, "bad_request"),
            (body(r#"["agents:read"]"#, "[]", &hash), 400, "bad_request"),
            (good.replace(r#""scope":{"home":"h-test0001"}"#, r#""scope":{"agent":"cortana::alice.example.test"}"#).replace(one, r#"[{"server":"a.example.test","room":"x"},{"server":"b.example.test","room":"y"}]"#), 400, "bad_request"),
            (good.replace("\"placement\":\"arcade\"", "\"placement\":\"talk\""), 404, "not_found"),
        ] {
            let (s, v) = owner("POST", "/cli/zen/widget/grant", Some(&b));
            assert_eq!((s, v["error"].as_str()), (status, Some(code)), "{b}: {v}");
        }

        // The owner grants: stored, signed, key file 0600.
        let (s, v) = owner("POST", "/cli/zen/widget/grant", Some(&good));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["grant"]["state"], "partial", "thread:post is asked for but not granted: {v}");
        assert_eq!(v["grant"]["caps"], json!(["agents:read", "thread:read"]));
        let key = home.join(".k2/zen-grant.key");
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&key).expect("key").permissions().mode() & 0o777, 0o600);
        }
        let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit log");
        assert!(audit.contains("zen.widget.grant") && audit.contains("zen.widget.refused"), "{audit}");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["caps"], json!(["agents:read", "thread:read"]), "{g}");
        assert_eq!(g["page"]["widgets"][0]["grant"]["sending"], true, "Sending is on by default (R6)");

        // TUWB4: the runaway guard turns sending off and pauses; a passport
        // can't resume or turn it on; the owner resumes.
        let off = format!(r#"{{"garden":"{garden}","placement":"arcade","on":false,"reason":"runaway"}}"#);
        let (s, v) = owner("POST", "/cli/zen/widget/sending", Some(&off));
        assert_eq!(s, 200, "{v}");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        let gr = &g["page"]["widgets"][0]["grant"];
        assert_eq!((gr["sending"].as_bool(), gr["paused"]["reason"].as_str()), (Some(false), Some("runaway")), "{gr}");
        assert_eq!(gr["state"], "partial", "a pause keeps the grant valid");
        let (s, v) = call(port, "POST", &format!("/cli/zen/widget/sending?token={}", mint_passport()), Some(&off.replace("\"reason\":\"runaway\"", "\"reason\":\"user\"")));
        assert_eq!(s, 403, "a passport turning sending off still needs Zen's owner token: {v}");
        assert_eq!(v["error"], "zen_local_only");
        let (s, v) = owner("POST", "/cli/zen/widget/resume", Some(&format!(r#"{{"garden":"{garden}","placement":"arcade"}}"#)));
        assert_eq!(s, 200, "{v}");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["grant"]["paused"], J::Null);
        assert_eq!(g["page"]["widgets"][0]["grant"]["sending"], true);

        // The Settings list.
        let (s, v) = owner("GET", "/cli/zen/widget/grants", None);
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["grants"][0]["placement"], "arcade");
        assert_eq!(v["grants"][0]["state"], "partial");

        // TUWB2: a row edited in SQLite is invalid, caps [].
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE zen_widget_grants SET caps_json = '[\"agents:read\",\"thread:read\",\"thread:post\"]' WHERE garden = ?1 AND revoked_at IS NULL",
                [&garden],
            )
            .expect("hand edit");
        }
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["grant"]["state"], "invalid", "{g}");
        assert_eq!(g["page"]["widgets"][0]["caps"], json!([]));
        let (s, v) = owner("POST", "/cli/zen/widget/resume", Some(&format!(r#"{{"garden":"{garden}","placement":"arcade"}}"#)));
        assert_eq!((s, v["error"].as_str()), (409, Some("widget_changed")), "an invalid row is never re-signed: {v}");

        // Re-grant, then a new key file voids it.
        let (s, v) = owner("POST", "/cli/zen/widget/grant", Some(&good));
        assert_eq!(s, 200, "{v}");
        let saved = std::fs::read(&key).expect("key");
        std::fs::remove_file(&key).expect("rm key");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["grant"]["state"], "invalid", "no key, no grant: {g}");
        std::fs::write(&key, &saved).expect("restore key");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        }

        // The placement's home changes → review.
        std::fs::write(home.join(format!(".k2/zen/gardens/{garden}.toml")), PAGE.replace("Work", "Play")).expect("edit");
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["grant"]["state"], "review", "{g}");
        std::fs::write(home.join(format!(".k2/zen/gardens/{garden}.toml")), PAGE).expect("restore");

        // Revoke → caps [], grant null.
        let (s, v) = owner("POST", "/cli/zen/widget/revoke", Some(&format!(r#"{{"garden":"{garden}","placement":"arcade"}}"#)));
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["revoked"].as_array().map(Vec::len), Some(1));
        let (_, g) = owner("GET", &format!("/cli/zen/get?garden={garden}"), None);
        assert_eq!(g["page"]["widgets"][0]["grant"], J::Null);
        assert_eq!(g["page"]["widgets"][0]["caps"], json!([]));

        // TUWB6: garden/new with a catalog grant, from the owner token.
        let (s, v) = owner("POST", "/cli/zen/garden/new", Some(r#"{"name":"Blank one","template":"blank","grant":{"scope":{"allHomes":true}}}"#));
        assert_eq!((s, v["error"].as_str()), (400, Some("bad_request")), "blank has nothing to allow: {v}");
        let diary = k2_core::zen::builtin_widgets::parse_widget_ref(k2_core::zen::builtin_widgets::DIARY_WIDGET).expect("ref");
        let (s, v) = owner("POST", "/cli/zen/garden/new", Some(r#"{"name":"My diary","template":"diary","grant":{"scope":{"allHomes":true}}}"#));
        if k2_core::zen::builtin_widgets::builtin_widget(&diary).is_some() {
            assert_eq!(s, 200, "{v}");
            assert_eq!(v["grant"]["caps"], json!(["agents:read", "thread:read", "thread:post"]), "{v}");
            assert_eq!(v["grant"]["sending"], true);
        } else {
            // This branch has no k2:diary@1 compiled in yet (B1 ships it).
            assert_eq!((s, v["error"].as_str()), (404, Some("unknown_widget")), "{v}");
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
