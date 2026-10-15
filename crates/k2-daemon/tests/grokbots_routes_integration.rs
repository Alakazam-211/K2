//! Local Grok Bots (0.45.7) through the REAL dispatcher under a temp HOME
//! with a fake `~/sand-data` and a fake Grok Bot gateway:
//!
//! - off by default: list is empty, nothing under sand-data opened;
//! - GET on `send` is 405 and the keep-alive socket answers the next one;
//! - a Member login can read but `send` / settings writes are 403
//!   `role_required` at the policy gate;
//! - an agent passport and an App pass are 403 `grokbots_humans_only`;
//! - the owner turns Show + Send on, lists, reads the thread and sends; the
//!   gateway sees the bearer, the body never carries the token;
//! - a send reaches Grok Bot as `[from <login>]` / `[from <owner name>]`;
//! - a send emits nothing on its own, but the setting change emits
//!   `grokbots_changed` on the session-event bus;
//! - each bot's chat is its collected K2 Thread: a start line, then what
//!   K2 sees after it (never the older `store.db` history); `/cli/thread`
//!   reads it at `gb:<name>` for people, 403 for a passport or an App pass;
//! - a stale bot (live conversation elsewhere) takes sends; the line K2
//!   sent lands in its Thread, and the send announces `grokbots_changed`.
//!
//! Never touches the real `~/.k2` or a real `~/sand-data`.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use k2_core::connect_users;
use k2_core::external_agents::grokbot::fixture::{self, FakeGateway, FakeSandData, FAKE_TOKEN};
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value as J};

const OWNER: &str = "owner-token-grokbots";
const A: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2346";
/// A second agent: the send limiter is per agent.
const B: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2347";
/// A stale agent (its live conversation runs elsewhere).
const C: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2348";

struct Conn {
    reader: BufReader<TcpStream>,
}

impl Conn {
    fn open(port: u16) -> Conn {
        let s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
        s.set_read_timeout(Some(Duration::from_secs(15)))
            .expect("timeout");
        Conn {
            reader: BufReader::new(s),
        }
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader
            .get_mut()
            .write_all(head.as_bytes())
            .expect("write request");
        let mut status_line = String::new();
        self.reader
            .read_line(&mut status_line)
            .expect("status line");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .expect("status")
            .parse()
            .expect("num");
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().expect("content-length");
                }
            }
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("body");
        (status, String::from_utf8(buf).expect("utf-8"))
    }
}

fn call(port: u16, method: &str, path: &str, body: Option<J>) -> (u16, J, String) {
    let b = body.map(|v| v.to_string());
    let (s, raw) = Conn::open(port).request(method, path, b.as_deref());
    let v = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {raw}"));
    (s, v, raw)
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn mint_passport() -> String {
    session_token::mint_session_token(
        &SessionId::new(),
        "pane-grokbots",
        HookPrincipal {
            workspace_uuid: "ws-grokbots".into(),
            agent_address: "agent-grokbots".into(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(username: &str, role: Option<connect_users::Role>) -> String {
    connect_users::add_user(username, "password123")
        .unwrap_or_else(|e| panic!("add_user {username}: {e:?}"));
    if let Some(r) = role {
        connect_users::set_role(username, r)
            .unwrap_or_else(|e| panic!("set_role {username}: {e:?}"));
    }
    connect_users::create_session(username)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grokbots_routes_gate_consent_and_round_trip() {
    let now = chrono::Utc::now().timestamp_millis();
    let mut stale = fixture::list_item(C, "Grok");
    stale["lastActivityAt"] = json!(now - 5_000);
    stale["localActivityAt"] = json!(now - 47 * 86_400_000_i64);
    stale["lastMessageId"] = json!("r1");
    stale["lastEntry"] = json!({"kind": "send-message", "text": "The October report is out."});
    stale["lastMessagePreview"] = json!("The October report is out.");
    let gw = FakeGateway::start(json!([
        fixture::list_item(A, "Cortana"),
        fixture::list_item(B, "Friday"),
        stale
    ]));
    let home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    let sand = FakeSandData::create(home.path(), gw.port);
    sand.add_agent(A, "Cortana", "Chief of staff");
    sand.add_agent(B, "Friday", "Ops");
    sand.add_agent(C, "Grok", "Assistant");
    sand.add_sample_chat(A);
    sand.add_sample_chat(C);
    let mut events = k2_daemon::session_events::subscribe();

    let d = futures_block(test_harness::start(OWNER));
    let port = d.port;
    let q = |p: &str, tok: &str| format!("{p}?token={tok}");

    // Off by default: nothing read.
    let (s, b, _) = call(port, "GET", &q("/cli/grokbots/list", OWNER), None);
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["enabled"], json!(false));
    assert_eq!(b["agents"], json!([]));
    assert!(
        fixture::opened_under(&sand.root).is_empty(),
        "off: sand-data untouched"
    );
    assert!(gw.seen().is_empty(), "off: gateway untouched");

    // GET on the POST-only send row → 405, and the socket keeps serving.
    let mut c = Conn::open(port);
    let (s, raw) = c.request("GET", &q("/cli/grokbots/send", OWNER), None);
    assert_eq!(s, 405, "{raw}");
    let (s, raw) = c.request("GET", &q("/cli/grokbots/settings", OWNER), None);
    assert_eq!(s, 200, "the next request on the same socket: {raw}");

    // Agent passport and App pass: refused by the handler.
    let passport = mint_passport();
    for tok in [passport.as_str(), "k2skn_0123456789abcdef0123456789abcdef"] {
        for (m, p, body) in [
            ("GET", "/cli/grokbots/list", None),
            ("GET", "/cli/grokbots/thread", None),
            (
                "POST",
                "/cli/grokbots/send",
                Some(json!({"agent": A, "text": "hi"})),
            ),
            (
                "POST",
                "/cli/grokbots/settings",
                Some(json!({"show": true, "send": true})),
            ),
        ] {
            let path = if p.ends_with("thread") {
                format!("{p}?agent={A}&token={tok}")
            } else {
                q(p, tok)
            };
            let (s, b, _) = call(port, m, &path, body);
            assert_eq!(s, 403, "{m} {p} with {}: {b}", &tok[..6]);
        }
    }
    let (_, b, _) = call(port, "GET", &q("/cli/grokbots/list", &passport), None);
    assert_eq!(b["error"], json!("grokbots_humans_only"), "{b}");

    // Member login: settings write and send stop at the policy gate.
    let member = login("gbmember", None);
    let (s, b, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/settings", &member),
        Some(json!({"show": true})),
    );
    assert_eq!(s, 403, "{b}");
    assert_eq!(b["error"], json!("role_required"), "{b}");
    let (s, b, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/send", &member),
        Some(json!({"agent": A, "text": "x"})),
    );
    assert_eq!(
        (s, b["error"].clone()),
        (403, json!("role_required")),
        "{b}"
    );

    // Owner turns Show on → event on the bus.
    let (s, b, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/settings", OWNER),
        Some(json!({"show": true})),
    );
    assert_eq!(s, 200, "{b}");
    assert_eq!(
        (b["show"].clone(), b["send"].clone()),
        (json!(true), json!(false))
    );
    let mut saw = false;
    for _ in 0..50 {
        match events.try_recv() {
            Ok(ev) => {
                let v = serde_json::to_value(&ev).expect("event json");
                if v["kind"] == "grokbots_changed" {
                    assert_eq!(v["roster"], json!(true));
                    saw = true;
                    break;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(saw, "settings change emits grokbots_changed");

    // Member can read now.
    let (s, b, raw) = call(port, "GET", &q("/cli/grokbots/list", &member), None);
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["agents"][0]["kind"], json!("grokbot"));
    assert_eq!(b["agents"][0]["name"], json!("Cortana"));
    assert!(!raw.contains(FAKE_TOKEN));
    let (s, b, raw) = call(
        port,
        "GET",
        &format!("/cli/grokbots/thread?agent={A}&token={member}"),
        None,
    );
    assert_eq!(s, 200, "{b}");
    // The collected Thread starts now: no older store.db history.
    assert_eq!(b["messages"][0]["system"], json!("start"), "{b}");
    assert!(!raw.contains("Build is green."), "{b}");
    assert!(!raw.contains(FAKE_TOKEN));

    // `k2 thread gb:<name>`: the same Thread through /cli/thread, for
    // people only.
    let (s, b, _) = call(
        port,
        "GET",
        &format!("/cli/thread?addr=gb%3ACortana&token={member}"),
        None,
    );
    assert_eq!(s, 200, "{b}");
    assert_eq!(
        (b["collection"].clone(), b["addr"].clone()),
        (json!("thread"), json!("gb:Cortana")),
        "{b}"
    );
    assert_eq!(b["items"][0]["doc"]["external"]["entryId"], json!("start"));
    for tok in [passport.as_str(), "k2skn_0123456789abcdef0123456789abcdef"] {
        let (s, b, _) = call(
            port,
            "GET",
            &format!("/cli/thread?addr=gb%3ACortana&token={tok}"),
            None,
        );
        assert_eq!(s, 403, "{b}");
        assert_eq!(b["error"], json!("grokbots_humans_only"), "{b}");
    }

    // Send is still off.
    let (s, b, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/send", OWNER),
        Some(json!({"agent": A, "text": "x"})),
    );
    assert_eq!(
        (s, b["error"].clone()),
        (403, json!("grokbots_send_off")),
        "{b}"
    );

    // Admin login turns Send on and sends.
    let admin = login("gbadmin", Some(connect_users::Role::Admin));
    let (s, _, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/settings", &admin),
        Some(json!({"send": true})),
    );
    assert_eq!(s, 200);
    let (s, b, raw) = call(
        port,
        "POST",
        &q("/cli/grokbots/send", &admin),
        Some(json!({"agent": A, "text": "ping"})),
    );
    assert_eq!(s, 200, "{b}");
    assert!(!raw.contains(FAKE_TOKEN));
    let sent: Vec<_> = gw
        .seen()
        .into_iter()
        .filter(|r| r.path == "/api/sendPrompt")
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body["prompt"], json!("[from gbadmin] ping"));
    assert_eq!(sent[0].body["clientNonce"], b["clientNonce"]);

    // The owner token: the owner's display name from Settings.
    k2_core::app_settings::update(json!({"ownerDisplayName": "Rosson Test"}))
        .expect("owner display name");
    let (s, b, _) = call(
        port,
        "POST",
        &q("/cli/grokbots/send", OWNER),
        Some(json!({"agent": B, "text": "hello"})),
    );
    assert_eq!(s, 200, "{b}");
    let sent: Vec<_> = gw
        .seen()
        .into_iter()
        .filter(|r| r.path == "/api/sendPrompt")
        .collect();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].body["prompt"], json!("[from Rosson Test] hello"));
    assert_eq!(sent[1].body["agentId"], json!(B));

    // A stale bot: the send goes through and shows in its live tail.
    while events.try_recv().is_ok() {}
    let (s, b, raw) = call(
        port,
        "POST",
        &q("/cli/grokbots/send", OWNER),
        Some(json!({"agent": C, "text": "send it to the board"})),
    );
    assert_eq!(s, 200, "{b}");
    assert!(!raw.contains(FAKE_TOKEN));
    let nonce = b["clientNonce"].as_str().expect("clientNonce").to_string();
    let sent: Vec<_> = gw
        .seen()
        .into_iter()
        .filter(|r| r.path == "/api/sendPrompt")
        .collect();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].body["agentId"], json!(C));
    let mut saw = false;
    for _ in 0..50 {
        match events.try_recv() {
            Ok(ev) => {
                let v = serde_json::to_value(&ev).expect("event json");
                if v["kind"] == "grokbots_changed" && v["agents"] == json!([C]) {
                    saw = true;
                    break;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(saw, "a stale send announces the agent's chat changed");
    let (s, t, raw) = call(
        port,
        "GET",
        &format!("/cli/grokbots/thread?agent={C}&token={member}"),
        None,
    );
    assert_eq!(s, 200, "{t}");
    assert!(!raw.contains(FAKE_TOKEN));
    assert_eq!(t["stale"], json!(true));
    let msgs = t["messages"].as_array().expect("messages");
    let got: Vec<(J, J, J)> = msgs
        .iter()
        .map(|e| (e["entryId"].clone(), e["author"].clone(), e["text"].clone()))
        .collect();
    assert_eq!(got.len(), 2, "the start line, then our send: {t}");
    assert_eq!(got[0].0, json!("start"));
    assert_eq!(
        got[1],
        (
            json!(format!("k2-{nonce}")),
            json!("human"),
            json!("[from Rosson Test] send it to the board")
        )
    );
    assert_eq!(msgs[1]["viaK2"], json!(true));
    assert!(!raw.contains("Build is green."), "no older history: {t}");
    assert!(
        !home
            .path()
            .join(".k2/external-agents/grokbots-live-tail.json")
            .exists(),
        "no JSON tail any more"
    );
    assert!(
        !k2_core::overlay::store::debug_docs_contain(FAKE_TOKEN).expect("scan"),
        "no token in the Thread store"
    );

    drop(d);
    drop(home);
}
