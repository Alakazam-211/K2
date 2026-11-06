//! The kit lane against a fake Sew on a loopback port (HTTP and the gateway):
//! what crosses, whose session it runs on, and what never reaches anyone.
//!
//! The fake is in process, like `sew_lane_tests`' (the door is async and its
//! tests drive it over a duplex pipe). People are named by their login
//! session id's first word: `alice-…` sees channels 10, 20 and 30 (READY lists
//! 10 and 20), `bob-…` sees 99, `carol-…`'s first Sew session is already gone
//! (401). A session token is `sews_<sid>_<generation>`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::*;
use crate::voice::kit_feed::{self, Gate, Seen as FeedSeen};
use crate::voice::sew_lane;

// ── the fake Sew ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String,
    /// The head, lower-cased.
    head: String,
    body: String,
}

impl Req {
    fn bearer(&self) -> String {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix("authorization: bearer "))
            .map(str::to_string)
            .unwrap_or_default()
    }
    /// `(sid, generation)` from `sews_<sid>_<gen>`.
    fn who(&self) -> (String, String) {
        let b = self.bearer();
        let rest = b.strip_prefix("sews_").unwrap_or("");
        let (sid, gen) = rest.rsplit_once('_').unwrap_or(("", ""));
        (sid.to_string(), gen.to_string())
    }
}

enum Cmd {
    Event(String, Value),
    Close,
}

#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<Req>>,
    tickets: Mutex<HashMap<String, String>>,
    identifies: Mutex<HashMap<String, usize>>,
    pushers: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Cmd>>>>,
    n: AtomicUsize,
}

fn role(sid: &str) -> &str {
    sid.split('-').next().unwrap_or("")
}

fn visible(sid: &str, ch: &str) -> bool {
    match role(sid) {
        "alice" | "carol" => ["10", "20", "30"].contains(&ch),
        "bob" => ch == "99",
        _ => false,
    }
}

fn member_id(sid: &str) -> &'static str {
    match role(sid) {
        "alice" => "1001",
        "bob" => "1002",
        _ => "1003",
    }
}

fn masked_404() -> (u16, Vec<(&'static str, String)>, String) {
    (404, vec![], json!({"ok": false, "error": {"code": "not_found", "hint": "no such channel", "details": null}}).to_string())
}

impl Fake {
    fn requests(&self) -> Vec<Req> {
        self.seen.lock().expect("seen").clone()
    }

    fn targets_of(&self, sid: &str) -> Vec<String> {
        self.requests().into_iter().filter(|r| r.who().0 == sid).map(|r| format!("{} {}", r.method, r.target)).collect()
    }

    /// Send `cmd` to every live gateway socket of `sid` (at least one).
    fn to_live(&self, sid: &str, cmd: impl Fn() -> Cmd) {
        let mut g = self.pushers.lock().expect("pushers");
        let senders = g.get_mut(sid).expect("a gateway socket for this person");
        senders.retain(|s| s.send(cmd()).is_ok());
        assert!(!senders.is_empty(), "no live gateway socket for {sid}");
    }

    fn push(&self, sid: &str, t: &str, d: Value) {
        self.to_live(sid, || Cmd::Event(t.to_string(), d.clone()));
    }

    fn close(&self, sid: &str) {
        self.to_live(sid, || Cmd::Close);
    }

    fn reply(&self, r: &Req) -> (u16, Vec<(&'static str, String)>, String) {
        let (sid, gen) = r.who();
        if role(&sid) == "carol" && gen == "1" {
            return (401, vec![], json!({"ok": false, "error": {"code": "unauthenticated", "hint": "session gone", "details": null}}).to_string());
        }
        let path = r.target.split('?').next().unwrap_or("");
        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (r.method.as_str(), segs.as_slice()) {
            ("POST", ["api", "v1", "gateway", "ticket"]) => {
                let n = self.n.fetch_add(1, Ordering::SeqCst);
                let ticket = format!("sewtk_{n}");
                self.tickets.lock().expect("tickets").insert(ticket.clone(), sid);
                // An address K2 must never dial: it dials its own loopback.
                (200, vec![], json!({"ticket": ticket, "url": "ws://198.51.100.1/api/v1/gateway", "expires_in_ms": 30000}).to_string())
            }
            ("GET", ["api", "v1", "members"]) => (
                200,
                vec![("x-ratelimit-remaining", "9".into()), ("set-cookie", "sew=1".into()), ("x-internal", "1".into())],
                json!({"items": [{"id": "1002", "username": "bob"}], "has_more": false}).to_string(),
            ),
            ("GET", ["api", "v1", "channels", id]) => {
                if visible(&sid, id) {
                    (200, vec![("etag", "\"c1\"".into())], json!({"id": id, "name": "secret-name", "topic": "t", "recording_audio_days": 30}).to_string())
                } else {
                    masked_404()
                }
            }
            ("POST", ["api", "v1", "channels", id, "recording", "start"]) => {
                if visible(&sid, id) {
                    (200, vec![], json!({"id": "77", "channel_id": id, "state": "active"}).to_string())
                } else {
                    masked_404()
                }
            }
            ("POST", ["api", "v1", "recordings", "429", "stop"]) => (
                429,
                vec![("retry-after", "7".into())],
                json!({"ok": false, "error": {"code": "rate_limited", "hint": "slow down", "details": {"retry_after_ms": 7000}}}).to_string(),
            ),
            ("GET", ["api", "v1", "me", "devices"]) => {
                let items = if role(&sid) == "alice" {
                    json!([{"device_id": "501", "platform": "ios", "label": "phone", "created_at": "2026-01-01T00:00:00Z", "serial": "9"}])
                } else {
                    json!([])
                };
                (200, vec![], json!({"items": items, "has_more": false}).to_string())
            }
            ("POST", ["api", "v1", "devices", _, "revoke"]) => (200, vec![], json!({"ok": true, "serials": ["9"]}).to_string()),
            _ => (200, vec![], json!({"ok": true}).to_string()),
        }
    }
}

async fn read_req(s: &mut TcpStream) -> Req {
    let mut got = Vec::new();
    let mut b = [0u8; 8192];
    loop {
        let n = s.read(&mut b).await.expect("read");
        assert!(n > 0, "K2 closed before a whole request");
        got.extend_from_slice(&b[..n]);
        let text = String::from_utf8_lossy(&got).to_string();
        if let Some(i) = text.find("\r\n\r\n") {
            let head = text[..i].to_ascii_lowercase();
            let len = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().expect("len"))).unwrap_or(0);
            if got.len() >= i + 4 + len {
                let mut first = text.lines().next().unwrap_or("").split_whitespace();
                return Req {
                    method: first.next().unwrap_or("").to_string(),
                    target: first.next().unwrap_or("").to_string(),
                    head,
                    body: text[i + 4..i + 4 + len].to_string(),
                };
            }
        }
    }
}

fn ready_for(sid: &str, n: usize) -> Value {
    json!({
        "session_id": format!("gs_{n}"),
        "resume_url": "ws://127.0.0.1:1/api/v1/gateway",
        "protocol": 1,
        "member": {"id": member_id(sid), "username": role(sid)},
        "install": {"policy_digest": {}},
        "spaces": [], "categories": [], "read_states": [], "threads_inbox": {}, "dm_members": [],
        "channels": [{"id": if role(sid) == "bob" { "99" } else { "10" }}],
        "voice_channels": [{"id": "20"}],
        "voice_states": [], "voice_states_partial": false, "calls": [], "recordings": [],
        "member_seq": 0,
    })
}

async fn gateway(fake: Arc<Fake>, s: TcpStream) {
    let mut ws = tokio_tungstenite::accept_async(s).await.expect("upgrade");
    ws.send(Message::Text(json!({"op": 8, "d": {"heartbeat_interval_ms": 45000, "session_limits": {}}}).to_string())).await.expect("hello");
    let identify: Value = match ws.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str(&t).expect("identify json"),
        other => panic!("no IDENTIFY: {other:?}"),
    };
    assert_eq!(identify["op"], 2, "{identify}");
    assert_eq!(identify["d"]["capabilities"], 0, "{identify}");
    assert!(identify["d"].get("token").is_none(), "K2 identifies with a ticket, never its session token: {identify}");
    let ticket = identify["d"]["ticket"].as_str().expect("ticket").to_string();
    // Single use.
    let Some(sid) = fake.tickets.lock().expect("tickets").remove(&ticket) else {
        let _ = ws.close(None).await;
        return;
    };
    let n = {
        let mut g = fake.identifies.lock().expect("identifies");
        let c = g.entry(sid.clone()).or_insert(0);
        *c += 1;
        *c
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    fake.pushers.lock().expect("pushers").entry(sid.clone()).or_default().push(tx);
    let mut s = 1u64;
    ws.send(Message::Text(json!({"op": 0, "t": "ready", "s": s, "d": ready_for(&sid, n)}).to_string())).await.expect("ready");
    loop {
        tokio::select! {
            c = rx.recv() => match c {
                Some(Cmd::Event(t, d)) => {
                    s += 1;
                    if ws.send(Message::Text(json!({"op": 0, "t": t, "s": s, "d": d}).to_string())).await.is_err() { return; }
                }
                Some(Cmd::Close) | None => { let _ = ws.close(None).await; return; }
            },
            m = ws.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let f: Value = serde_json::from_str(&t).expect("json");
                    if f["op"] == 1 {
                        let _ = ws.send(Message::Text(json!({"op": 9, "d": null}).to_string())).await;
                    }
                }
                Some(Ok(_)) => {}
                _ => return,
            }
        }
    }
}

async fn fake_sew() -> (String, Arc<Fake>) {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("addr").to_string();
    let fake = Arc::new(Fake::default());
    let f = fake.clone();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.expect("accept");
            let f = f.clone();
            tokio::spawn(async move {
                let mut peek = [0u8; 32];
                let n = s.peek(&mut peek).await.expect("peek");
                if peek[..n].starts_with(b"GET /api/v1/gateway?") {
                    return gateway(f, s).await;
                }
                let r = read_req(&mut s).await;
                f.seen.lock().expect("seen").push(r.clone());
                let (status, hs, body) = f.reply(&r);
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (k, v) in hs {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                let _ = s.write_all(format!("{head}\r\n{body}").as_bytes()).await;
            });
        }
    });
    (addr, fake)
}

// ── K2's side: who's who, without a box ──────────────────────────────

struct TestWorld {
    gens: Mutex<HashMap<String, usize>>,
    ended: AtomicBool,
}

fn ctx_for(door: Door, recheck: Duration) -> (KitCtx, Arc<TestWorld>) {
    let w = Arc::new(TestWorld { gens: Mutex::new(HashMap::new()), ended: AtomicBool::new(false) });
    let wr = w.clone();
    let ws = w.clone();
    let resolve = move |door: &Door, bearer: &str| -> Result<KitPerson, Refusal> {
        let (kind, sid) = bearer.split_once(':').ok_or_else(|| Refusal::new(401, "unauthenticated", "unknown"))?;
        if wr.ended.load(Ordering::SeqCst) {
            return Err(Refusal::new(401, "unauthenticated", "that login session is unknown, expired or revoked"));
        }
        match (kind, door) {
            ("login", Door::Box { .. }) => Ok(KitPerson::stand_in(sid, false)),
            ("app", Door::App { .. }) => Ok(KitPerson::stand_in(sid, true)),
            ("login", Door::App { .. }) => Err(Refusal::new(401, "unauthenticated", "on an App's address this door takes the person's App login session")),
            ("app", Door::Box { .. }) => Err(Refusal::new(403, "app_pass_not_allowed", "voice rooms are for this server's own logins")),
            ("agent", _) => Err(Refusal::new(403, "agents_not_in_voice", "agents don't join voice rooms")),
            _ => Err(Refusal::new(401, "unauthenticated", "unknown")),
        }
    };
    let session = move |p: &KitPerson, fresh: bool| -> Result<String, Refusal> {
        let mut g = ws.gens.lock().expect("gens");
        let gen = g.entry(p.sid.clone()).or_insert(0);
        if fresh || *gen == 0 {
            *gen += 1;
        }
        Ok(format!("sews_{}_{}", p.sid, gen))
    };
    let deps = Arc::new(KitDeps { resolve: Arc::new(resolve), session: Arc::new(session), recheck });
    (KitCtx { door, deps }, w)
}

fn box_ctx() -> KitCtx {
    ctx_for(Door::Box { owner_token: "owner-token".into() }, Duration::from_secs(60)).0
}

fn app_ctx() -> KitCtx {
    ctx_for(Door::App { app_id: "acme".into() }, Duration::from_secs(60)).0
}

/// One request through the door; what the client read.
async fn through(addr: &str, kit: &KitCtx, req: &str) -> String {
    let head_len = req.find("\r\n\r\n").expect("head") + 4;
    let head = req[..head_len].to_string();
    let (mut client, mut lane) = tokio::io::duplex(256 * 1024);
    client.write_all(req.as_bytes()).await.expect("write");
    let (addr, kit) = (addr.to_string(), kit.clone());
    let task = tokio::spawn(async move {
        match &kit.door {
            Door::Box { .. } => sew_lane::serve_box(&mut lane, &head, head_len, true, &addr, Some(&kit)).await,
            Door::App { .. } => sew_lane::serve_app(&mut lane, &head, head_len, true, &addr, Some(&kit)).await,
        }
        lane.shutdown().await.expect("shutdown");
    });
    let mut out = Vec::new();
    client.read_to_end(&mut out).await.expect("read");
    task.await.expect("door task");
    String::from_utf8_lossy(&out).to_string()
}

fn req(method: &str, target: &str, bearer: &str, body: Option<&str>) -> String {
    let extra = "Cookie: k2_session=abc\r\nX-Forwarded-For: 1.2.3.4\r\nOrigin: tauri://localhost\r\nAccept: application/json\r\n";
    match body {
        Some(b) => format!(
            "{method} /_k2/voice/sew{target} HTTP/1.1\r\nHost: lab.k2.dev\r\nAuthorization: Bearer {bearer}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} /_k2/voice/sew{target} HTTP/1.1\r\nHost: lab.k2.dev\r\nAuthorization: Bearer {bearer}\r\n{extra}\r\n"),
    }
}

fn status(out: &str) -> u16 {
    out.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("no status line: {out}"))
}

fn body_json(out: &str) -> Value {
    let i = out.find("\r\n\r\n").expect("head end");
    serde_json::from_str(&out[i + 4..]).unwrap_or_else(|e| panic!("body isn't JSON ({e}): {out}"))
}

// ── the allowlist ────────────────────────────────────────────────────

#[test]
fn the_allowlist_is_exact() {
    for (m, p, q, want) in [
        ("GET", "/api/v1/members", Some("ids=1002"), KitRoute::Members),
        ("GET", "/api/v1/members", Some("ids=1,2,3"), KitRoute::Members),
        ("POST", "/api/v1/channels/10/call/decline", None, KitRoute::CallDecline),
        ("GET", "/api/v1/channels/10", None, KitRoute::Channel),
        ("POST", "/api/v1/channels/10/recording/start", None, KitRoute::RecordingStart),
        ("POST", "/api/v1/recordings/77/stop", None, KitRoute::RecordingStop),
        ("POST", "/api/v1/spaces/3/members/1002/voice", None, KitRoute::MemberVoice),
        ("POST", "/api/v1/spaces/3/members/1002/voice/move", None, KitRoute::MemberMove),
        ("GET", "/api/v1/me/devices", None, KitRoute::MyDevices),
        ("POST", "/api/v1/devices/501/revoke", None, KitRoute::DeviceRevoke),
        ("GET", "/feed", None, KitRoute::Feed),
    ] {
        assert_eq!(kit_route(m, p, q), Some(want), "{m} {p} {q:?}");
    }
    let hundred = (1..=100).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    assert_eq!(kit_route("GET", "/api/v1/members", Some(&format!("ids={hundred}"))), Some(KitRoute::Members));
    let too_many = format!("ids={hundred},101");
    let twenty = "1".repeat(20);
    let ids_twenty = format!("ids={twenty}");
    let ch_twenty = format!("/api/v1/channels/{twenty}");
    for (m, p, q) in [
        // Not the kit's.
        ("POST", "/api/v1/auth/k2", None),
        ("POST", "/api/v1/gateway/ticket", None),
        ("GET", "/api/v1/gateway", None),
        ("GET", "/api/v1/channels/10/messages", None),
        ("POST", "/api/v1/channels/10/messages", None),
        ("POST", "/api/v1/channels/10/voice/join", None),
        ("GET", "/api/v1/spaces", None),
        ("GET", "/mgmt/v1/local/users", None),
        // The wrong method.
        ("POST", "/api/v1/members", Some("ids=1")),
        ("GET", "/api/v1/channels/10/call/decline", None),
        ("POST", "/api/v1/channels/10", None),
        ("GET", "/api/v1/devices/501/revoke", None),
        ("POST", "/api/v1/me/devices", None),
        ("POST", "/feed", None),
        // R1's query and nothing else.
        ("GET", "/api/v1/members", None),
        ("GET", "/api/v1/members", Some("")),
        ("GET", "/api/v1/members", Some("ids=")),
        ("GET", "/api/v1/members", Some("username=bob")),
        ("GET", "/api/v1/members", Some("ids=1&username=bob")),
        ("GET", "/api/v1/members", Some("ids=1,,2")),
        ("GET", "/api/v1/members", Some("ids=1%2C2")),
        ("GET", "/api/v1/members", Some("IDS=1")),
        ("GET", "/api/v1/members", Some(too_many.as_str())),
        ("GET", "/api/v1/members", Some(ids_twenty.as_str())),
        ("GET", "/api/v1/channels/10", Some("x=1")),
        ("GET", "/api/v1/me/devices", Some("limit=5")),
        ("GET", "/feed", Some("v=1")),
        // Ids are plain decimals of 1–19 digits, one segment each.
        ("GET", ch_twenty.as_str(), None),
        ("GET", "/api/v1/channels/abc", None),
        ("GET", "/api/v1/channels/-1", None),
        ("GET", "/api/v1/channels/1e3", None),
        ("GET", "/api/v1/channels/%31", None),
        ("GET", "/api/v1/channels/10%2Fmessages", None),
        ("GET", "/api/v1/channels/../me/devices", None),
        ("GET", "/api/v1/channels/", None),
        ("GET", "/api/v1/channels//", None),
        ("GET", "/api/v1/channels/10/", None),
        ("GET", "/api/v1//channels/10", None),
        ("POST", "/api/v1/spaces/3/members/me/voice", None),
        ("POST", "/api/v1/spaces/3/members/1002/voice/move/", None),
        ("POST", "/api/v1/devices/501/revoke/x", None),
        ("GET", "/feed/", None),
        ("GET", "", None),
        ("GET", "/", None),
    ] {
        assert_eq!(kit_route(m, p, q), None, "{m} {p} {q:?}");
    }
}

#[test]
fn app_sessions_get_the_narrow_set() {
    for r in [KitRoute::Members, KitRoute::CallDecline, KitRoute::Channel, KitRoute::MyDevices, KitRoute::DeviceRevoke, KitRoute::Feed] {
        assert!(r.for_app_sessions(), "{r:?}");
    }
    for r in [KitRoute::RecordingStart, KitRoute::RecordingStop, KitRoute::MemberVoice, KitRoute::MemberMove] {
        assert!(!r.for_app_sessions(), "{r:?}");
    }
}

// ── routes ───────────────────────────────────────────────────────────

#[tokio::test]
async fn each_route_runs_as_the_person_and_only_the_listed_headers_cross() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let me = "login:alice-each";
    let cases: [(&str, &str, Option<&str>); 9] = [
        ("GET", "/api/v1/members?ids=1002,1003", None),
        ("POST", "/api/v1/channels/10/call/decline", Some(r#"{"call_id": "5"}"#)),
        ("GET", "/api/v1/channels/10", None),
        ("POST", "/api/v1/channels/20/recording/start", Some("{}")),
        ("POST", "/api/v1/recordings/77/stop", Some("{}")),
        ("POST", "/api/v1/spaces/3/members/1002/voice", Some(r#"{"server_mute": true}"#)),
        ("POST", "/api/v1/spaces/3/members/1002/voice/move", Some(r#"{"channel_id": null}"#)),
        ("GET", "/api/v1/me/devices", None),
        ("POST", "/api/v1/devices/501/revoke", Some(r#"{"reason": "retired"}"#)),
    ];
    for (m, target, body) in cases {
        let out = through(&addr, &kit, &req(m, target, me, body)).await;
        assert_eq!(status(&out), 200, "{m} {target}: {out}");
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains("set-cookie") && !lower.contains("x-internal"), "an unlisted header came back: {out}");
        let r = fake.requests().into_iter().rev().find(|r| r.method == m && r.target == target).unwrap_or_else(|| panic!("Sew never saw {m} {target}"));
        assert_eq!(r.bearer(), "sews_alice-each_1", "{m} {target}: not the person's own session");
        assert_eq!(r.head.matches("authorization:").count(), 1, "{}", r.head);
        for gone in ["cookie", "x-forwarded-for", "origin", "lab.k2.dev", "login:alice"] {
            assert!(!r.head.contains(gone), "{gone} crossed on {m} {target}: {}", r.head);
        }
        assert!(r.head.contains("accept: application/json"), "{}", r.head);
        if let Some(b) = body {
            let sent: Value = serde_json::from_str(b).expect("json");
            assert_eq!(serde_json::from_str::<Value>(&r.body).expect("Sew got JSON"), sent, "{m} {target}");
        } else {
            assert!(r.body.is_empty(), "a GET carried a body: {}", r.body);
        }
    }
    // R1: the query byte for byte, the rate-limit headers back.
    let out = through(&addr, &kit, &req("GET", "/api/v1/members?ids=1002,1003", me, None)).await;
    assert!(out.to_ascii_lowercase().contains("x-ratelimit-remaining: 9"), "{out}");
    assert_eq!(body_json(&out)["items"][0]["username"], "bob", "{out}");
    // R3: trimmed to what the kit reads.
    let out = through(&addr, &kit, &req("GET", "/api/v1/channels/10", me, None)).await;
    assert_eq!(body_json(&out), json!({"id": "10", "recording_audio_days": 30}), "{out}");
    assert!(out.to_ascii_lowercase().contains("etag: \"c1\""), "{out}");
    // R9 read the person's own list first.
    let mine = fake.targets_of("alice-each");
    let revoke = mine.iter().position(|t| t == "POST /api/v1/devices/501/revoke").expect("revoke");
    assert_eq!(mine[revoke - 1], "GET /api/v1/me/devices", "{mine:?}");
}

#[tokio::test]
async fn an_idempotency_key_crosses_when_sent() {
    let (addr, fake) = fake_sew().await;
    let r = req("POST", "/api/v1/channels/10/call/decline", "login:alice-idem", Some(r#"{"call_id":"5"}"#))
        .replacen("Content-Type", "Idempotency-Key: k-123\r\nContent-Type", 1);
    assert_eq!(status(&through(&addr, &box_ctx(), &r).await), 200);
    let seen = fake.requests().into_iter().find(|r| r.who().0 == "alice-idem").expect("Sew saw it");
    assert!(seen.head.contains("idempotency-key: k-123"), "{}", seen.head);
}

#[tokio::test]
async fn unlisted_and_malformed_requests_never_reach_sew() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let me = "login:alice-unlisted";
    for (m, target, body) in [
        ("POST", "/api/v1/auth/k2", Some("{}")),
        ("POST", "/api/v1/gateway/ticket", Some("{}")),
        ("GET", "/api/v1/gateway/ticket", None),
        ("GET", "/api/v1/channels/10/messages", None),
        ("GET", "/api/v1/members?username=bob", None),
        ("GET", "/api/v1/members?ids=1&x=2", None),
        ("GET", "/api/v1/channels/10?x=1", None),
        ("GET", "/api/v1/channels/10%2F..%2Fmessages", None),
        ("GET", "/api/v1/channels/..%2F..%2Fmgmt", None),
        ("POST", "/api/v1/devices/abc/revoke", Some("{}")),
        ("GET", "/api/v1/spaces", None),
    ] {
        let out = through(&addr, &kit, &req(m, target, me, body)).await;
        assert_eq!(status(&out), 404, "{m} {target}: {out}");
    }
    // Malformed: Sew's error shape, never forwarded.
    let post = |extra: &str, body: &str| {
        format!(
            "POST /_k2/voice/sew/api/v1/channels/10/call/decline HTTP/1.1\r\nAuthorization: Bearer {me}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let cases = [
        (post("", "[1,2]"), 400),
        (post("", "not json"), 400),
        (post("Transfer-Encoding: chunked\r\n", "{}"), 400),
        (post("Content-Length: 2\r\n", "{}"), 400),
        (post("Authorization: Bearer login:bob-unlisted\r\n", "{}"), 400),
        (post("Idempotency-Key: a\r\nIdempotency-Key: b\r\n", "{}"), 400),
        (format!("POST /_k2/voice/sew/api/v1/channels/10/call/decline HTTP/1.1\r\nAuthorization: Bearer {me}\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1), 413),
        (format!("GET /_k2/voice/sew/api/v1/me/devices HTTP/1.1\r\nAuthorization: Bearer {me}\r\nContent-Length: 2\r\n\r\n{{}}"), 400),
    ];
    for (r, want) in cases {
        let out = through(&addr, &kit, &r).await;
        assert_eq!(status(&out), want, "{r}\n→ {out}");
        let b = body_json(&out);
        assert_eq!(b["ok"], false, "{out}");
        assert!(b["error"]["code"].is_string(), "Sew's ErrorBody shape: {out}");
    }
    assert!(fake.targets_of("alice-unlisted").is_empty(), "something reached Sew: {:?}", fake.targets_of("alice-unlisted"));
    assert!(fake.targets_of("bob-unlisted").is_empty());
}

#[tokio::test]
async fn bearers_pick_the_lane_and_bad_ones_are_refused_here() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    // No bearer on a kit route: 401 in Sew's shape.
    let out = through(&addr, &kit, "GET /_k2/voice/sew/api/v1/channels/10 HTTP/1.1\r\nCookie: k2_session=abc\r\n\r\n").await;
    assert_eq!(status(&out), 401, "{out}");
    assert_eq!(body_json(&out)["error"]["code"], "unauthenticated");
    // A K2 login on an engine-only route is still the engine's 401.
    let out = through(&addr, &kit, &req("POST", "/api/v1/me/voice/state", "login:alice-lanes", Some("{}"))).await;
    assert!(out.starts_with("HTTP/1.1 401") && out.contains("call_credential_required"), "{out}");
    // Who K2 refuses, as join refuses them.
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "agent:x", None)).await;
    assert_eq!(status(&out), 403, "{out}");
    assert_eq!(body_json(&out)["error"]["code"], "agents_not_in_voice");
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "app:alice-lanes", None)).await;
    assert_eq!(status(&out), 403, "an App session on the box address: {out}");
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "nonsense", None)).await;
    assert_eq!(status(&out), 401, "{out}");
    assert!(!out.contains("nonsense"), "the token came back: {out}");
    assert!(fake.requests().is_empty(), "something reached Sew: {:?}", fake.requests());
}

#[tokio::test]
async fn someone_elses_room_and_device_are_refused() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    // Bob's room: Sew answers alice's own session with its masked 404, and
    // K2 passes it through unchanged.
    let out = through(&addr, &kit, &req("POST", "/api/v1/channels/99/recording/start", "login:alice-idor", Some("{}"))).await;
    assert_eq!(status(&out), 404, "{out}");
    assert_eq!(body_json(&out), json!({"ok": false, "error": {"code": "not_found", "hint": "no such channel", "details": null}}));
    let r = fake.requests().into_iter().find(|r| r.target == "/api/v1/channels/99/recording/start").expect("Sew saw it");
    assert_eq!(r.bearer(), "sews_alice-idor_1", "it ran on alice's own session");
    // The same room is bob's: his own session, his answer.
    let out = through(&addr, &kit, &req("POST", "/api/v1/channels/99/recording/start", "login:bob-idor", Some("{}"))).await;
    assert_eq!(status(&out), 200, "{out}");
    // Someone else's device: K2 refuses before Sew's revoke.
    let out = through(&addr, &kit, &req("POST", "/api/v1/devices/777/revoke", "login:alice-idor", Some(r#"{"reason":"retired"}"#))).await;
    assert_eq!(status(&out), 404, "{out}");
    assert_eq!(body_json(&out)["error"]["details"]["reason"], "not_your_device", "{out}");
    let revokes: Vec<String> = fake.requests().into_iter().filter(|r| r.target.ends_with("/revoke")).map(|r| r.target).collect();
    assert!(revokes.is_empty(), "a revoke reached Sew: {revokes:?}");
    // R3 for bob's room: Sew's masked 404, untrimmed.
    let out = through(&addr, &kit, &req("GET", "/api/v1/channels/99", "login:alice-idor", None)).await;
    assert_eq!(status(&out), 404, "{out}");
}

#[tokio::test]
async fn an_app_session_gets_the_narrow_set() {
    let (addr, fake) = fake_sew().await;
    let kit = app_ctx();
    let me = "app:alice-guest";
    for (m, target, body) in [
        ("GET", "/api/v1/members?ids=1002", None),
        ("POST", "/api/v1/channels/10/call/decline", Some(r#"{"call_id":"5"}"#)),
        ("GET", "/api/v1/channels/10", None),
        ("GET", "/api/v1/me/devices", None),
        ("POST", "/api/v1/devices/501/revoke", Some("{}")),
    ] {
        let out = through(&addr, &kit, &req(m, target, me, body)).await;
        assert_eq!(status(&out), 200, "{m} {target}: {out}");
    }
    for (m, target) in [
        ("POST", "/api/v1/channels/10/recording/start"),
        ("POST", "/api/v1/recordings/77/stop"),
        ("POST", "/api/v1/spaces/3/members/1002/voice"),
        ("POST", "/api/v1/spaces/3/members/1002/voice/move"),
    ] {
        let out = through(&addr, &kit, &req(m, target, me, Some("{}"))).await;
        assert_eq!(status(&out), 403, "{m} {target}: {out}");
        let b = body_json(&out);
        assert_eq!(b["error"]["code"], "missing_permission", "{out}");
        assert_eq!(b["error"]["details"]["reason"], "app_session_route", "{out}");
        assert!(!fake.targets_of("alice-guest").iter().any(|t| t.ends_with(target)), "{target} reached Sew");
    }
    // A box login on an App's address is not this App's person.
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "login:alice-guestbox", None)).await;
    assert_eq!(status(&out), 401, "{out}");
    // The feed is in the narrow set.
    let (mut ws, _t) = open_feed(&addr, &kit, "app:alice-guestfeed").await.expect("the feed opens");
    assert_eq!(next_json(&mut ws).await["t"], "ready");
}

#[tokio::test]
async fn k2s_rate_limit_and_sews_own_429_both_reach_the_kit() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let now = Instant::now();
    while take_token("alice-rate", now) {}
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "login:alice-rate", None)).await;
    assert_eq!(status(&out), 429, "{out}");
    assert!(out.contains(&format!("Retry-After: {RETRY_SECS}\r\n")), "{out}");
    let b = body_json(&out);
    assert_eq!(b["error"]["code"], "rate_limited", "{out}");
    assert_eq!(b["error"]["details"]["retry_after_ms"], RETRY_SECS * 1000, "{out}");
    assert!(fake.targets_of("alice-rate").is_empty(), "past the limit something reached Sew");
    // Another login has its own budget.
    assert!(take_token("bob-rate", now));
    // Sew's own 429: status, body and Retry-After unchanged.
    let out = through(&addr, &kit, &req("POST", "/api/v1/recordings/429/stop", "login:alice-rate429", Some("{}"))).await;
    assert_eq!(status(&out), 429, "{out}");
    assert!(out.to_ascii_lowercase().contains("retry-after: 7\r\n"), "{out}");
    assert_eq!(body_json(&out), json!({"ok": false, "error": {"code": "rate_limited", "hint": "slow down", "details": {"retry_after_ms": 7000}}}));
    // The flood guard refills.
    assert!(take_token("alice-rate", now + Duration::from_secs(1)));
}

#[tokio::test]
async fn a_401_renews_the_session_a_get_retries_once_a_post_never() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "login:carol-get", None)).await;
    assert_eq!(status(&out), 200, "the GET retried on a fresh session: {out}");
    let gens: Vec<String> = fake.requests().into_iter().filter(|r| r.who().0 == "carol-get").map(|r| r.who().1).collect();
    assert_eq!(gens, ["1", "2"], "one retry, on the renewed session");
    let out = through(&addr, &kit, &req("POST", "/api/v1/channels/10/call/decline", "login:carol-post", Some("{}"))).await;
    assert_eq!(status(&out), 401, "a POST is never retried: {out}");
    assert_eq!(fake.targets_of("carol-post").len(), 1, "{:?}", fake.targets_of("carol-post"));
    let out = through(&addr, &kit, &req("GET", "/api/v1/me/devices", "login:carol-post", None)).await;
    assert_eq!(status(&out), 200, "the next request has the renewed session: {out}");
    let last = fake.requests().into_iter().rev().find(|r| r.who().0 == "carol-post").expect("seen");
    assert_eq!(last.who().1, "2");
}

// ── the feed ─────────────────────────────────────────────────────────

const WS_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// Open the feed through the door: the socket, or the HTTP answer.
async fn open_feed(addr: &str, kit: &KitCtx, bearer: &str) -> Result<(WebSocketStream<DuplexStream>, tokio::task::JoinHandle<()>), String> {
    let r = format!(
        "GET /_k2/voice/sew/feed HTTP/1.1\r\nHost: lab.k2.dev\r\nAuthorization: Bearer {bearer}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {WS_KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    open_feed_raw(addr, kit, &r).await
}

async fn open_feed_raw(addr: &str, kit: &KitCtx, r: &str) -> Result<(WebSocketStream<DuplexStream>, tokio::task::JoinHandle<()>), String> {
    let (mut client, mut lane) = tokio::io::duplex(1024 * 1024);
    client.write_all(r.as_bytes()).await.expect("write");
    let (addr, kit, head) = (addr.to_string(), kit.clone(), r.to_string());
    let len = head.len();
    let task = tokio::spawn(async move {
        match &kit.door {
            Door::Box { .. } => sew_lane::serve_box(&mut lane, &head, len, true, &addr, Some(&kit)).await,
            Door::App { .. } => sew_lane::serve_app(&mut lane, &head, len, true, &addr, Some(&kit)).await,
        }
        let _ = lane.shutdown().await;
    });
    // Byte by byte, so no frame after the 101 is read here.
    let mut got = Vec::new();
    let mut b = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), client.read(&mut b)).await.expect("handshake in time").expect("read");
        assert!(n > 0, "closed in the handshake: {}", String::from_utf8_lossy(&got));
        got.push(b[0]);
    }
    let head = String::from_utf8_lossy(&got).to_string();
    if !head.starts_with("HTTP/1.1 101") {
        let mut rest = Vec::new();
        let _ = client.read_to_end(&mut rest).await;
        return Err(format!("{head}{}", String::from_utf8_lossy(&rest)));
    }
    assert!(head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"), "{head}");
    Ok((WebSocketStream::from_raw_socket(client, Role::Client, None).await, task))
}

async fn next_msg(ws: &mut WebSocketStream<DuplexStream>) -> Message {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("a frame in time") {
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            Some(Ok(m)) => return m,
            other => panic!("the feed ended: {other:?}"),
        }
    }
}

async fn next_json(ws: &mut WebSocketStream<DuplexStream>) -> Value {
    match next_msg(ws).await {
        Message::Text(t) => serde_json::from_str(&t).expect("json frame"),
        other => panic!("not a text frame: {other:?}"),
    }
}

async fn next_close(ws: &mut WebSocketStream<DuplexStream>) -> u16 {
    match next_msg(ws).await {
        Message::Close(Some(f)) => u16::from(f.code),
        other => panic!("not a close: {other:?}"),
    }
}

async fn quiet(ws: &mut WebSocketStream<DuplexStream>) {
    if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
        panic!("the feed sent something it shouldn't have: {m:?}");
    }
}

#[test]
fn the_gate_follows_sews_visibility_table() {
    let me = "1001";
    assert_eq!(gate_of("message_created", json!({"channel_id": "10"})), Gate::Drop);
    assert_eq!(gate_of("typing_started", json!({})), Gate::Drop);
    assert_eq!(gate_of("call_keys_delivered", json!({})), Gate::Drop);
    assert_eq!(kit_feed::gate("voice_state_updated", &json!({"member_id": me, "channel_id": "99"}), me), Gate::Pass);
    assert_eq!(
        kit_feed::gate("voice_state_updated", &json!({"member_id": "1002", "channel_id": "20", "previous_channel_id": "99"}), me),
        Gate::AnyOf(vec!["20".into(), "99".into()])
    );
    assert_eq!(kit_feed::gate("voice_state_updated", &json!({"member_id": "1002"}), me), Gate::Drop);
    assert_eq!(gate_of("recording_updated", json!({"recording": {"channel_id": 99}})), Gate::AnyOf(vec!["99".into()]));
    assert_eq!(gate_of("call_updated", json!({"call": {"channel_id": "5"}})), Gate::AnyOf(vec!["5".into()]));
    assert_eq!(gate_of("channel_updated", json!({"id": "5"})), Gate::AnyOf(vec!["5".into()]));
    for t in ["call_ring", "voice_server_updated", "media_policy_updated", "perms_changed", "install_updated", "member_updated", "space_updated"] {
        assert_eq!(gate_of(t, json!({})), Gate::Pass, "{t}");
    }
    let mut seen = FeedSeen::from_ready(&ready_for("alice-x", 1));
    assert_eq!(seen.me, "1001");
    assert_eq!((seen.known("10"), seen.known("20"), seen.known("99")), (Some(true), Some(true), None));
    seen.after("perms_changed");
    assert_eq!(seen.known("10"), None, "perms change: asked again");
}

fn gate_of(t: &str, d: Value) -> Gate {
    kit_feed::gate(t, &d, "1001")
}

#[tokio::test]
async fn the_feed_sends_ready_first_filters_and_renumbers() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let (mut ws, _task) = open_feed(&addr, &kit, "login:alice-feed").await.expect("the feed opens");
    let ready = next_json(&mut ws).await;
    assert_eq!((ready["op"].clone(), ready["t"].clone(), ready["s"].clone()), (json!(0), json!("ready"), json!(1)), "{ready}");
    assert_eq!(ready["d"]["member"]["id"], "1001", "READY whole");
    assert_eq!(ready["d"]["resume_url"], "", "K2 never resumes: the loopback URL stays here");
    assert_eq!(ready["d"]["voice_channels"][0]["id"], "20");
    let ticket = fake.requests().into_iter().find(|r| r.target == "/api/v1/gateway/ticket").expect("ticket");
    assert_eq!(ticket.bearer(), "sews_alice-feed_1", "the person's own session's ticket");

    let other = |ch: &str| json!({"member_id": "1002", "channel_id": ch, "state": null});
    fake.push("alice-feed", "message_created", json!({"channel_id": "10", "content": "hi"}));
    fake.push("alice-feed", "voice_state_updated", other("20"));
    fake.push("alice-feed", "voice_state_updated", other("99"));
    fake.push("alice-feed", "voice_state_updated", json!({"member_id": "1001", "channel_id": "99", "reason": "moved"}));
    fake.push("alice-feed", "recording_updated", json!({"recording": {"id": "7", "channel_id": "99"}}));
    fake.push("alice-feed", "voice_state_updated", other("30"));
    fake.push("alice-feed", "call_ring", json!({"call_id": "4", "channel_id": "50", "started_by": "1002", "ringing": true, "silent": false}));
    fake.push("alice-feed", "voice_server_updated", json!({"channel_id": "20", "token": "media-secret"}));
    fake.push("alice-feed", "voice_state_updated", other("99"));

    let want = [
        ("voice_state_updated", json!("20")),
        ("voice_state_updated", json!("99")),
        ("voice_state_updated", json!("30")),
        ("call_ring", json!("50")),
        ("voice_server_updated", json!("20")),
    ];
    for (i, (t, ch)) in want.iter().enumerate() {
        let f = next_json(&mut ws).await;
        assert_eq!(f["t"], *t, "frame {i}: {f}");
        assert_eq!(f["s"], json!(i as u64 + 2), "s stays contiguous: {f}");
        assert_eq!(f["d"]["channel_id"], *ch, "frame {i}: {f}");
        if i == 1 {
            assert_eq!(f["d"]["member_id"], "1001", "a hidden room passes only for the person's own state");
        }
    }
    quiet(&mut ws).await;
    // Each unknown channel was asked once, as alice; 99's no stayed known.
    let asks: Vec<String> = fake.targets_of("alice-feed").into_iter().filter(|t| t.starts_with("GET /api/v1/channels/")).collect();
    assert_eq!(asks, ["GET /api/v1/channels/99", "GET /api/v1/channels/30"], "{asks:?}");
    // A perms change forgets what K2 learned: 99 is asked again.
    fake.push("alice-feed", "perms_changed", json!({"channels": []}));
    assert_eq!(next_json(&mut ws).await["t"], "perms_changed");
    fake.push("alice-feed", "voice_state_updated", other("99"));
    quiet(&mut ws).await;
    let asks = fake.targets_of("alice-feed").into_iter().filter(|t| t == "GET /api/v1/channels/99").count();
    assert_eq!(asks, 2, "asked again after perms_changed");
}

#[tokio::test]
async fn one_upstream_per_person_so_a_hidden_rooms_events_never_cross() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let (mut alice, _a) = open_feed(&addr, &kit, "login:alice-two").await.expect("alice's feed");
    let (mut bob, _b) = open_feed(&addr, &kit, "login:bob-two").await.expect("bob's feed");
    assert_eq!(next_json(&mut alice).await["d"]["member"]["id"], "1001");
    assert_eq!(next_json(&mut bob).await["d"]["member"]["id"], "1002");
    fake.push("bob-two", "voice_state_updated", json!({"member_id": "1003", "channel_id": "99"}));
    fake.push("bob-two", "recording_updated", json!({"recording": {"id": "8", "channel_id": "99", "my_consent": "granted"}}));
    let f = next_json(&mut bob).await;
    assert_eq!((f["t"].clone(), f["s"].clone()), (json!("voice_state_updated"), json!(2)), "{f}");
    let f = next_json(&mut bob).await;
    assert_eq!(f["d"]["recording"]["my_consent"], "granted", "bob's own copy: {f}");
    quiet(&mut alice).await;
    assert_eq!(fake.identifies.lock().expect("ids").get("alice-two"), Some(&1));
    assert_eq!(fake.identifies.lock().expect("ids").get("bob-two"), Some(&1));
}

#[tokio::test]
async fn after_a_reset_the_next_feed_starts_with_a_fresh_ready() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let (mut ws, _t) = open_feed(&addr, &kit, "login:alice-again").await.expect("the feed opens");
    assert_eq!(next_json(&mut ws).await["t"], "ready");
    fake.push("alice-again", "voice_state_updated", json!({"member_id": "1001", "channel_id": "20"}));
    assert_eq!(next_json(&mut ws).await["s"], 2);
    fake.close("alice-again");
    assert_eq!(next_close(&mut ws).await, kit_feed::CLOSE_RESET, "Sew's socket ended: reconnect");
    let (mut ws, _t) = open_feed(&addr, &kit, "login:alice-again").await.expect("the feed opens again");
    let ready = next_json(&mut ws).await;
    assert_eq!((ready["t"].clone(), ready["s"].clone()), (json!("ready"), json!(1)), "a fresh READY seeds it: {ready}");
    assert_eq!(fake.identifies.lock().expect("ids").get("alice-again"), Some(&2), "a new upstream session");
    let tickets = fake.targets_of("alice-again").into_iter().filter(|t| t == "POST /api/v1/gateway/ticket").count();
    assert_eq!(tickets, 2, "one single-use ticket each");
    fake.push("alice-again", "voice_state_updated", json!({"member_id": "1001", "channel_id": "20"}));
    assert_eq!(next_json(&mut ws).await["s"], 2);
}

#[tokio::test]
async fn the_feed_closes_when_the_login_ends() {
    let (addr, _fake) = fake_sew().await;
    let (kit, world) = ctx_for(Door::Box { owner_token: "owner-token".into() }, Duration::from_millis(100));
    let (mut ws, _t) = open_feed(&addr, &kit, "login:alice-ends").await.expect("the feed opens");
    assert_eq!(next_json(&mut ws).await["t"], "ready");
    world.ended.store(true, Ordering::SeqCst);
    assert_eq!(next_close(&mut ws).await, kit_feed::CLOSE_LOGIN_ENDED);
}

#[tokio::test]
async fn feed_refusals_are_http_answers() {
    let (addr, fake) = fake_sew().await;
    let kit = box_ctx();
    let no_upgrade = "GET /_k2/voice/sew/feed HTTP/1.1\r\nAuthorization: Bearer login:alice-nofeed\r\n\r\n";
    let out = open_feed_raw(&addr, &kit, no_upgrade).await.expect_err("no socket without an upgrade");
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    let out = open_feed(&addr, &kit, "agent:x").await.expect_err("agents get no feed");
    assert!(out.starts_with("HTTP/1.1 403") && out.contains("agents_not_in_voice"), "{out}");
    let out = open_feed(&addr, &kit, "nonsense").await.expect_err("an unknown login");
    assert!(out.starts_with("HTTP/1.1 401"), "{out}");
    assert!(fake.targets_of("alice-nofeed").is_empty());
    // At most FEEDS_PER_LOGIN at once on one login.
    let mut open = Vec::new();
    for _ in 0..kit_feed::FEEDS_PER_LOGIN {
        let (mut ws, t) = open_feed(&addr, &kit, "login:alice-many").await.expect("under the cap");
        assert_eq!(next_json(&mut ws).await["t"], "ready");
        open.push((ws, t));
    }
    let out = open_feed(&addr, &kit, "login:alice-many").await.expect_err("past the cap");
    assert!(out.starts_with("HTTP/1.1 429"), "{out}");
    // One closes, another opens.
    let (ws, t) = open.pop().expect("one");
    drop(ws);
    tokio::time::timeout(Duration::from_secs(5), t).await.expect("closed in time").expect("task");
    let (mut ws, _t) = open_feed(&addr, &kit, "login:alice-many").await.expect("a slot came back");
    assert_eq!(next_json(&mut ws).await["t"], "ready");
}
