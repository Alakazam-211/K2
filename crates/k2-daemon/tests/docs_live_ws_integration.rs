//! The Live view's socket, `WS /cli/docs/live` (plan L2), headless: the
//! real dispatcher (`test_harness`) and the real docs runtime (the vendored
//! kit, a temp `~/.k2/docs.redb`), driven by WebSocket clients.
//!
//! What it pins (the L2 transport spec v0 and its addendum):
//!   1. Auth before the upgrade: no token and an App pass are 403 (and the
//!      `GET /cli/docs/live/check` probe says so over HTTP: 204 for a person); the owner
//!      token and a Connect login get `welcome` first (text, version, a
//!      client id per socket, self, rights).
//!   2. Two sockets on one doc: edits converge (each side's text equals the
//!      daemon's), the other socket gets `change`, and a socket never gets
//!      its own edit back: it is filtered by version, and the `ack` comes
//!      after every change before it.
//!   3. `undo` comes back as `change` (not filtered) then `undone {op, steps,
//!      version}`; nothing to undo is `steps: 0`.
//!   4. A caret at the current version reaches the other socket's roster; a
//!      caret at an old version is dropped.
//!   5. Reconnect: a new client id, `welcome` without text on a resume the
//!      window covers, and `since` gives the missed changes. Past the window
//!      (`MPC-E112`): `welcome` carries the text, and `snapshot` reloads it.
//!   6. Keepalive: Pings arrive; a socket that never answers is closed and
//!      leaves the doc.
//!   7. Frame cap: a frame over 4 MiB is refused in order (`MPC-E113`) and
//!      the socket stays; one over 8 MiB closes it (`cut_off`, 1009).
//!   8. Never-live files and files that aren't text are refused with one
//!      `closed` frame, before any join.
//!   9. A core that's off refuses the socket (`closed`, `core_off`, no
//!      retry); switching it off ends open sockets with `closed` after the
//!      write-back (the file holds every acknowledged edit), code 4010; the
//!      gate alone (no `close_core`) ends them within a second or two.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME, a
//! short keepalive (`K2_DOCS_LIVE_PING_MS=300`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-live";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ── HTTP ─────────────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_else(|| panic!("no body: {text:?}"));
    Resp { status, body }
}

fn json_of(r: &Resp) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", r.body))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn login_member(port: u16, username: &str) -> String {
    let password = "correct-horse-battery-staple-9";
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER}"),
        Some(&format!(r#"{{"username":"{username}","password":"{password}"}}"#)),
    );
    assert_eq!(r.status, 200, "users/add: {}", r.body);
    let r = http(port, "POST", "/cli/auth/login", Some(&format!(r#"{{"username":"{username}","password":"{password}"}}"#)));
    assert_eq!(r.status, 200, "login: {}", r.body);
    json_of(&r)["token"].as_str().expect("login token").to_string()
}

/// The doc's text as the daemon reads it now.
fn daemon_text(port: u16, path: &str) -> (u64, String) {
    let r = http(port, "GET", &format!("/cli/docs/read?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(r.status, 200, "read: {}", r.body);
    let j = json_of(&r);
    (j["version"].as_u64().expect("version"), j["text"].as_str().expect("text").to_string())
}

/// Roster pids from `who` (the owner's own CLI connection included).
fn who_pids(port: u16, path: &str) -> Vec<String> {
    let r = http(port, "GET", &format!("/cli/docs/who?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(r.status, 200, "who: {}", r.body);
    json_of(&r)["roster"].as_array().expect("roster").iter().map(|e| e["pid"].as_str().expect("pid").to_string()).collect()
}

// ── A Live client ────────────────────────────────────────────────────

#[derive(Debug)]
enum In {
    Text(J),
    Ping,
    Close(Option<CloseFrame<'static>>),
}

/// A socket whose reads run on their own task (so Pings are answered while
/// the test does other things).
struct Live {
    sink: SplitSink<Ws, Message>,
    rx: mpsc::UnboundedReceiver<In>,
}

fn live_url(port: u16, token: &str, doc: &str, extra: &str) -> String {
    format!("ws://127.0.0.1:{port}/cli/docs/live?token={}&doc={}&root=source{extra}", enc(token), enc(doc))
}

async fn connect_raw(url: &str) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap_or_else(|e| panic!("Live socket must open: {e}"));
    ws
}

fn reader(mut read: SplitStream<Ws>) -> mpsc::UnboundedReceiver<In> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(m) = read.next().await {
            let item = match m {
                Ok(Message::Text(t)) => In::Text(serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"))),
                Ok(Message::Ping(_)) => In::Ping,
                Ok(Message::Close(c)) => In::Close(c.map(|c| c.into_owned())),
                Ok(_) => continue,
                Err(_) => break,
            };
            if tx.send(item).is_err() {
                break;
            }
        }
    });
    rx
}

async fn open(url: &str) -> Live {
    let (sink, read) = connect_raw(url).await.split();
    Live { sink, rx: reader(read) }
}

impl Live {
    async fn send(&mut self, v: J) {
        self.sink.send(Message::Text(v.to_string())).await.expect("send");
    }

    /// The next text frame (Pings skipped); fails after 10 s or on close.
    async fn next(&mut self, what: &str) -> J {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(left, self.rx.recv()).await {
                Err(_) => panic!("timed out waiting for {what}"),
                Ok(None) => panic!("socket ended waiting for {what}"),
                Ok(Some(In::Text(j))) => return j,
                Ok(Some(In::Ping)) => continue,
                Ok(Some(In::Close(c))) => panic!("socket closed ({c:?}) waiting for {what}"),
            }
        }
    }

    /// Frames until one matches; returns it and every frame before it.
    async fn until(&mut self, what: &str, pred: impl Fn(&J) -> bool) -> (J, Vec<J>) {
        let mut before = Vec::new();
        loop {
            let f = self.next(what).await;
            if pred(&f) {
                return (f, before);
            }
            before.push(f);
        }
    }

    /// Every text frame within `ms` (Pings skipped; a close ends it).
    async fn drain(&mut self, ms: u64) -> Vec<J> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(left, self.rx.recv()).await {
                Err(_) | Ok(None) | Ok(Some(In::Close(_))) => return out,
                Ok(Some(In::Text(j))) => out.push(j),
                Ok(Some(In::Ping)) => {}
            }
        }
    }

    /// The `closed` frame and the close that follows.
    async fn closed(&mut self, what: &str) -> (J, Option<CloseFrame<'static>>) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut frame: Option<J> = None;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(left, self.rx.recv()).await {
                Err(_) => panic!("timed out waiting for the close: {what} (frame {frame:?})"),
                Ok(None) => return (checked(frame.unwrap_or_else(|| panic!("{what}: no closed frame"))), None),
                Ok(Some(In::Text(j))) if j["type"] == "closed" => frame = Some(j),
                Ok(Some(In::Text(_))) | Ok(Some(In::Ping)) => {}
                Ok(Some(In::Close(c))) => {
                    return (checked(frame.unwrap_or_else(|| panic!("{what}: close without a closed frame"))), c)
                }
            }
        }
    }
}

/// Doc text that must never reach a `closed` frame (its message, its error,
/// its extra fields), whichever refusal path sent it.
const MARKER: &str = "ZQX-DOC-MARKER-7741";

/// The safety rules every `closed` frame keeps: its `retry` is the wire
/// table's (only `cut_off` and `network` retry), and nothing in it is doc
/// text (the docs below all hold [`MARKER`]).
fn checked(c: J) -> J {
    let reason = c["reason"].as_str().unwrap_or_else(|| panic!("a closed frame without a reason: {c}"));
    let want = matches!(reason, "cut_off" | "network");
    assert_eq!(c["retry"].as_bool(), Some(want), "retry must follow the reason table for {reason}: {c}");
    assert!(!c.to_string().contains(MARKER), "a closed frame leaked doc text: {c}");
    c
}

// ── Text model (the client's side, UTF-8 bytes) ──────────────────────

fn apply(text: &str, hunks: &J) -> String {
    let mut out = String::new();
    let mut at = 0usize;
    for h in hunks.as_array().expect("hunks") {
        let (s, e) = (h["start"].as_u64().expect("start") as usize, h["end"].as_u64().expect("end") as usize);
        assert!(s >= at && e >= s && e <= text.len(), "hunk {h} in a {}-byte text", text.len());
        out.push_str(&text[at..s]);
        out.push_str(h["insert"].as_str().expect("insert"));
        at = e;
    }
    out.push_str(&text[at..]);
    out
}

fn ty(f: &J) -> &str {
    f["type"].as_str().unwrap_or("")
}

static MARKDOWN_ON: AtomicBool = AtomicBool::new(true);

fn test_gate(core: &str) -> bool {
    core != "markdown" || MARKDOWN_ON.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_socket_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-live-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("home");
    let tmp = tmp.canonicalize().expect("canonical home");
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_DOCS_LIVE_PING_MS", "300");
    for (k, _) in std::env::vars() {
        if (k.starts_with("K2_") && k != "K2_DOCS_LIVE_PING_MS") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }

    let daemon = test_harness::start(OWNER).await;
    let port = daemon.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "the docs runtime is up: {}", k2_daemon::docs::pool_json());
    k2_daemon::docs::live::set_core_gate(Some(test_gate));

    // A workspace with a Markdown file, a picture and a secret.
    let ws = tmp.join("ws-live");
    std::fs::create_dir_all(ws.join(".k2")).expect("ws");
    let ws = ws.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'livews', ?2, 'livews')",
            params![uuid::Uuid::new_v4().to_string(), ws.to_string_lossy()],
        )
        .expect("seed project");
    }
    let doc = ws.join("notes.md");
    std::fs::write(&doc, "hello\n").expect("notes");
    let mut png = vec![0x89u8, b'P', b'N', b'G', 0, 1, 2, 3];
    png.extend_from_slice(MARKER.as_bytes());
    std::fs::write(ws.join("photo.png"), png).expect("png");
    std::fs::write(ws.join(".env"), format!("API_KEY=sk-live-0123456789abcdef\nNOTE={MARKER}\n")).expect("env");
    let doc = doc.to_string_lossy().to_string();
    let member = login_member(port, "pat");

    // ── 1. Auth before the upgrade ──
    for (what, url) in [
        ("no token", format!("ws://127.0.0.1:{port}/cli/docs/live?doc={}", enc(&doc))),
        ("a wrong token", live_url(port, "nope", &doc, "")),
        ("an App pass", live_url(port, "k2skn_notarealpass", &doc, "")),
    ] {
        match tokio_tungstenite::connect_async(&url).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
                assert!(matches!(r.status().as_u16(), 401 | 403), "{what}: {}", r.status())
            }
            Err(e) => panic!("{what}: expected a 403 before the upgrade, got {e}"),
            Ok(_) => panic!("{what}: must not upgrade"),
        }
    }

    // The transport's probe after a failed dial: 204 for a person, the
    // socket's 403 otherwise (auth, not network: no retry). GET only.
    let check = |tok: &str| http(port, "GET", &format!("/cli/docs/live/check?token={}&doc={}", enc(tok), enc(&doc)), None);
    for (who, tok) in [("the owner", OWNER), ("a login", member.as_str())] {
        let r = check(tok);
        assert_eq!(r.status, 204, "{who}: {}", r.body);
        assert!(r.body.is_empty(), "{who}: {}", r.body);
    }
    for (who, tok) in [("a wrong token", "nope"), ("an App pass", "k2skn_notarealpass")] {
        let r = check(tok);
        assert!(matches!(r.status, 401 | 403), "{who}: {} {}", r.status, r.body);
        assert!(!r.body.contains(MARKER), "{who}: the probe never reads the doc");
    }
    let r = http(port, "GET", &format!("/cli/docs/live/check?doc={}", enc(&doc)), None);
    assert!(matches!(r.status, 401 | 403), "no token: {} {}", r.status, r.body);
    let r = http(port, "POST", &format!("/cli/docs/live/check?token={OWNER}"), Some("{}"));
    assert_eq!(r.status, 405, "GET only: {}", r.body);

    let mut a = open(&live_url(port, OWNER, &doc, "")).await;
    let mut b = open(&live_url(port, &member, &doc, "")).await;
    let wa = a.next("welcome A").await;
    let wb = b.next("welcome B").await;
    for (w, who) in [(&wa, "A"), (&wb, "B")] {
        assert_eq!(ty(w), "welcome", "{who}: the first frame is welcome: {w}");
        assert_eq!(w["root"], "source", "{w}");
        assert_eq!(w["text"], "hello\n", "{who}: first connect carries the text: {w}");
        assert_eq!(w["rights"], "edit", "{w}");
        assert_eq!(w["core"], "markdown", "{w}");
    }
    assert_eq!(wa["self"], "person:owner", "{wa}");
    assert!(wb["self"].as_str().is_some_and(|s| s.starts_with("person:") && s != "person:owner"), "{wb}");
    assert_ne!(wa["client"], wb["client"], "a client id per socket");
    let v0 = wa["version"].as_u64().expect("version");
    assert_eq!(wb["version"].as_u64(), Some(v0));
    // Presence: both people are in the doc.
    let pids = who_pids(port, &doc);
    assert!(pids.contains(&"person:owner".to_string()) && pids.contains(&wb["self"].as_str().expect("self").to_string()), "{pids:?}");

    // ── 2. Two sockets, one doc: converge, and never your own edit back ──
    let mut text_a = "hello\n".to_string();
    let mut text_b = "hello\n".to_string();
    a.send(json!({"type":"edit","op":1,"root":"source","base":v0,"hunks":[{"start":5,"end":5,"insert":" world"}]})).await;
    let (ack1, before) = a.until("ack 1", |f| ty(f) == "ack").await;
    assert_eq!(ack1["op"], 1, "{ack1}");
    let v1 = ack1["version"].as_u64().expect("v1");
    assert_eq!(v1, v0 + 1, "{ack1}");
    assert!(before.iter().all(|f| ty(f) != "change"), "nothing changed before A's edit: {before:?}");
    text_a = apply(&text_a, &ack1["hunks"]);
    // B, still at v0, edits at base v0 (the kit rebases it).
    b.send(json!({"type":"edit","op":1,"root":"source","base":v0,"hunks":[{"start":0,"end":0,"insert":"X"}]})).await;
    let (ack_b, before_b) = b.until("ack B", |f| ty(f) == "ack").await;
    let changes_b: Vec<&J> = before_b.iter().filter(|f| ty(f) == "change").collect();
    assert_eq!(changes_b.len(), 1, "B gets A's change, before its own ack: {before_b:?}");
    assert_eq!(changes_b[0]["event"]["version"].as_u64(), Some(v1));
    assert_eq!(changes_b[0]["event"]["pid"], "person:owner");
    assert_eq!(changes_b[0]["event"]["via"], "tab");
    text_b = apply(&text_b, &changes_b[0]["event"]["hunks"]);
    let v2 = ack_b["version"].as_u64().expect("v2");
    assert_eq!(v2, v1 + 1, "{ack_b}");
    text_b = apply(&text_b, &ack_b["hunks"]);
    // A gets B's change and never its own (by version).
    let (ch2, before_a) = a.until("B's change at A", |f| ty(f) == "change").await;
    assert!(before_a.iter().all(|f| ty(f) != "change"), "{before_a:?}");
    assert_eq!(ch2["event"]["version"].as_u64(), Some(v2), "A's own change at {v1} is never forwarded: {ch2}");
    text_a = apply(&text_a, &ch2["event"]["hunks"]);
    let late: Vec<J> = a.drain(400).await.into_iter().filter(|f| ty(f) == "change").collect();
    assert!(late.is_empty(), "no late echo of A's own edit: {late:?}");
    let (dv, dtext) = daemon_text(port, &doc);
    assert_eq!(dv, v2);
    assert_eq!(text_a, dtext, "A converged");
    assert_eq!(text_b, dtext, "B converged");
    assert_eq!(dtext, "Xhello world\n");

    // ── 3. Undo comes back as change, then undone ──
    a.send(json!({"type":"undo","op":2})).await;
    let (ur, before_u) = a.until("undone", |f| ty(f) == "undone").await;
    assert_eq!(ur["op"], 2);
    assert_eq!(ur["steps"], 1, "{ur}");
    assert!(ur.get("notice").is_none(), "{ur}");
    let v3 = ur["version"].as_u64().expect("v3");
    assert_eq!(v3, v2 + 1);
    let undo_change: Vec<&J> = before_u.iter().filter(|f| ty(f) == "change").collect();
    assert_eq!(undo_change.len(), 1, "the undo arrives as change before its reply: {before_u:?}");
    assert_eq!(undo_change[0]["event"]["version"].as_u64(), Some(v3));
    text_a = apply(&text_a, &undo_change[0]["event"]["hunks"]);
    let (ub, _) = b.until("undo at B", |f| ty(f) == "change").await;
    assert_eq!(ub["event"]["version"].as_u64(), Some(v3));
    text_b = apply(&text_b, &ub["event"]["hunks"]);
    let (_, dtext) = daemon_text(port, &doc);
    assert_eq!(dtext, "Xhello\n", "the owner's ' world' was undone");
    assert_eq!((text_a.as_str(), text_b.as_str()), (dtext.as_str(), dtext.as_str()));
    // Nothing more to undo for B? B still has its X: undo it, then redo.
    b.send(json!({"type":"undo","op":2})).await;
    let (ubr, _) = b.until("B undone", |f| ty(f) == "undone").await;
    assert_eq!(ubr["steps"], 1, "{ubr}");
    b.send(json!({"type":"redo","op":3})).await;
    let (rbr, _) = b.until("B redone", |f| ty(f) == "undone").await;
    assert_eq!(rbr["steps"], 1, "{rbr}");
    b.send(json!({"type":"undo","op":4})).await;
    b.send(json!({"type":"undo","op":5})).await;
    let (_, _) = b.until("B undo 4", |f| ty(f) == "undone" && f["op"] == 4).await;
    let (nothing, before_n) = b.until("B undo 5", |f| ty(f) == "undone" && f["op"] == 5).await;
    assert_eq!(nothing["steps"], 0, "nothing to undo still answers, steps 0: {nothing}");
    assert!(before_n.iter().all(|f| ty(f) != "change"), "nothing to undo makes no change: {before_n:?}");
    assert_eq!(nothing["version"].as_u64(), Some(daemon_text(port, &doc).0), "the version unchanged");

    // ── 4. Carets: current version in, old version dropped ──
    let _ = a.drain(300).await;
    let (vnow, tnow) = daemon_text(port, &doc);
    let _ = b.drain(300).await;
    assert!(tnow.len() >= 3, "{tnow:?}");
    a.send(json!({"type":"cursor","root":"source","version":vnow,"anchor":1,"head":3})).await;
    let (caret, _) = b
        .until("A's caret at B", |f| ty(f) == "roster_event" && f["event"]["kind"] == "cursor" && f["event"]["who"] == "person:owner")
        .await;
    assert_eq!(caret["event"]["cursor"]["anchor"], 1, "{caret}");
    assert_eq!(caret["event"]["cursor"]["head"], 3, "{caret}");
    assert_eq!(caret["event"]["cursor"]["version"].as_u64(), Some(vnow), "{caret}");
    a.send(json!({"type":"cursor","root":"source","version":v0,"anchor":2,"head":2})).await;
    let stale: Vec<J> = b
        .drain(1200)
        .await
        .into_iter()
        .filter(|f| ty(f) == "roster_event" && f["event"]["kind"] == "cursor" && f["event"]["cursor"]["anchor"] == 2)
        .collect();
    assert!(stale.is_empty(), "a caret at an old version is dropped (G6): {stale:?}");
    // Viewing and view frames are taken quietly.
    a.send(json!({"t":"viewing","visible":true})).await;
    a.send(json!({"type":"view","first":1,"last":1})).await;
    a.send(json!({"type":"since","op":6,"version":vnow})).await;
    let (ev, _) = a.until("since after viewing", |f| ty(f) == "events").await;
    assert_eq!(ev["op"], 6);
    assert_eq!(ev["events"].as_array().map(Vec::len), Some(0), "{ev}");

    // ── 5. Reconnect: new client id, resume, since ──
    let a_client = wa["client"].clone();
    let resume_at = vnow;
    let _ = a.sink.send(Message::Close(None)).await;
    drop(a);
    b.send(json!({"type":"edit","op":10,"root":"source","base":vnow,"hunks":[{"start":0,"end":0,"insert":"> "}]})).await;
    let (ack10, _) = b.until("ack 10", |f| ty(f) == "ack").await;
    let v_missed = ack10["version"].as_u64().expect("missed version");
    let mut a = open(&live_url(port, OWNER, &doc, &format!("&resume={resume_at}"))).await;
    let wa2 = a.next("welcome after reconnect").await;
    assert_eq!(ty(&wa2), "welcome", "{wa2}");
    assert!(wa2.get("text").is_none(), "a resume the window covers carries no text: {wa2}");
    assert_ne!(wa2["client"], a_client, "a new client id on every reconnect");
    assert_eq!(wa2["version"].as_u64(), Some(v_missed));
    a.send(json!({"type":"since","op":11,"version":resume_at})).await;
    let (missed, _) = a.until("since after reconnect", |f| ty(f) == "events").await;
    let evs = missed["events"].as_array().expect("events");
    assert_eq!(evs.len(), 1, "{missed}");
    assert_eq!(evs[0]["version"].as_u64(), Some(v_missed));
    assert_eq!(apply(&tnow, &evs[0]["hunks"]), daemon_text(port, &doc).1, "since replays onto the old text");

    // ── 7. Frame cap (before the big edits below) ──
    let big = "a".repeat((4 << 20) + 100);
    a.send(json!({"type":"edit","op":20,"root":"source","base":v_missed,"hunks":[{"start":0,"end":0,"insert":big}]})).await;
    let (r20, _) = a.until("refused 20", |f| f["op"] == 20).await;
    assert_eq!(ty(&r20), "refused", "{}", ty(&r20));
    assert_eq!(r20["error"]["id"], "MPC-E113", "{}", r20["error"]);
    a.send(json!({"type":"since","op":21,"version":v_missed})).await;
    let (s21, _) = a.until("since 21", |f| f["op"] == 21).await;
    assert_eq!(ty(&s21), "events", "the socket stays open after a refused big frame");
    assert_eq!(daemon_text(port, &doc).0, v_missed, "nothing applied");
    {
        let mut huge = open(&live_url(port, OWNER, &doc, "")).await;
        let _ = huge.next("welcome huge").await;
        let blob = "b".repeat(9 << 20);
        let _ = huge.sink.send(Message::Text(format!(r#"{{"type":"edit","op":1,"root":"source","base":0,"hunks":[{{"start":0,"end":0,"insert":"{blob}"}}]}}"#))).await;
        let (c, frame) = huge.closed("over the hard cap").await;
        assert_eq!(c["reason"], "cut_off", "{c}");
        assert_eq!(c["error"]["id"], "MPC-E113", "{c}");
        assert_eq!(frame.map(|f| u16::from(f.code)), Some(1009));
    }

    // ── 5b. Past the window: welcome with text, snapshot reload ──
    let old = daemon_text(port, &doc).0;
    let mut cur = daemon_text(port, &doc);
    for (i, ch) in ["p", "q", "r"].iter().enumerate() {
        let body = ch.repeat(3 << 19); // 1.5 MiB, replacing the whole text
        let op = 30 + i as u64;
        a.send(json!({"type":"edit","op":op,"root":"source","base":cur.0,"hunks":[{"start":0,"end":cur.1.len(),"insert":body}]})).await;
        let (ack, _) = a.until("big ack", |f| f["op"] == op).await;
        assert_eq!(ty(&ack), "ack", "{}", ty(&ack));
        cur = daemon_text(port, &doc);
        assert_eq!(cur.0, ack["version"].as_u64().expect("v"));
    }
    a.send(json!({"type":"since","op":40,"version":old})).await;
    let (exp, _) = a.until("since past the window", |f| f["op"] == 40).await;
    assert_eq!(ty(&exp), "refused", "{exp}");
    assert_eq!(exp["error"]["id"], "MPC-E112", "{}", exp["error"]);
    a.send(json!({"type":"snapshot","op":41})).await;
    let (snap, _) = a.until("snapshot", |f| f["op"] == 41).await;
    assert_eq!(ty(&snap), "snapshot");
    assert_eq!(snap["version"].as_u64(), Some(cur.0));
    assert_eq!(snap["text"].as_str().map(str::len), Some(cur.1.len()));
    let mut c = open(&live_url(port, OWNER, &doc, &format!("&resume={old}"))).await;
    let wc = c.next("welcome past the window").await;
    assert_eq!(wc["text"].as_str().map(str::len), Some(cur.1.len()), "resume past the window: the whole text");
    drop(c);
    // A resume ahead of the doc (another store): the whole text too.
    let mut c = open(&live_url(port, OWNER, &doc, &format!("&resume={}", cur.0 + 1000))).await;
    let wc = c.next("welcome ahead").await;
    assert!(wc.get("text").is_some(), "{}", ty(&wc));
    drop(c);
    // Back to a small text for the rest.
    a.send(json!({"type":"edit","op":42,"root":"source","base":cur.0,"hunks":[{"start":0,"end":cur.1.len(),"insert":"small\n"}]})).await;
    let (ack42, _) = a.until("ack 42", |f| f["op"] == 42).await;
    assert_eq!(ty(&ack42), "ack");
    let _ = b.drain(500).await;

    // ── 6. Keepalive ──
    let pings = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut n = 0;
        while tokio::time::Instant::now() < deadline && n < 2 {
            if let Ok(Some(In::Ping)) = timeout(Duration::from_millis(500), a.rx.recv()).await {
                n += 1;
            }
        }
        n
    };
    assert!(pings >= 2, "the daemon pings (every 300 ms here)");
    let quiet_tok = login_member(port, "quiet");
    let quiet = connect_raw(&live_url(port, &quiet_tok, &doc, "")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(who_pids(port, &doc).iter().any(|p| p.ends_with("quiet")), "the quiet socket joined");
    // It never reads, so it never answers a Ping: closed after ~750 ms.
    let mut gone = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !who_pids(port, &doc).iter().any(|p| p.ends_with("quiet")) {
            gone = true;
            break;
        }
    }
    assert!(gone, "a socket that misses two pongs is closed and leaves the doc");
    drop(quiet);

    // ── 8. Never live, not text ──
    let mut s = open(&live_url(port, OWNER, &ws.join(".env").to_string_lossy(), "")).await;
    let (c, _) = s.closed("secret").await;
    assert_eq!(c["reason"], "declined", "{c}");
    assert_eq!(c["retry"], false);
    assert_eq!(c["code"], "secret", "{c}");
    let mut s = open(&live_url(port, OWNER, &ws.join("photo.png").to_string_lossy(), "")).await;
    let (c, _) = s.closed("png").await;
    assert_eq!(c["code"], "not_a_text_doc", "{c}");
    let mut s = open(&live_url(port, OWNER, &ws.join("nope.md").to_string_lossy(), "")).await;
    let (c, _) = s.closed("missing").await;
    assert_eq!(c["reason"], "not_found", "{c}");

    // ── 8b. A text that doesn't fit one frame: closed {declined}, MPC-E401 ──
    let ctrl = ws.join("ctrl.md");
    std::fs::write(&ctrl, format!("{MARKER}\n{}", "\u{1}".repeat(1 << 20))).expect("ctrl"); // 1 MiB, 6 MiB as JSON
    let mut s = open(&live_url(port, OWNER, &ctrl.to_string_lossy(), "")).await;
    let (c, _) = s.closed("too large for one frame").await;
    assert_eq!(c["reason"], "declined", "{c}");
    assert_eq!(c["retry"], false, "{c}");
    assert_eq!(c["error"]["id"], "MPC-E401", "{c}");
    assert_eq!(c["error"]["details"]["limit"], "frame", "{c}");
    assert_eq!(c["error"]["details"]["frame_max"], 4_194_304, "{c}");

    // ── 10. Renamed (k2 doc mv): moved, with the new path ──
    let other = ws.join("other.md");
    std::fs::write(&other, format!("o {MARKER}\n")).expect("other");
    let other = other.to_string_lossy().to_string();
    let mut m = open(&live_url(port, OWNER, &other, "")).await;
    assert_eq!(ty(&m.next("welcome other").await), "welcome");
    let r = http(
        port,
        "POST",
        &format!("/cli/docs/mv?token={OWNER}"),
        Some(&json!({"path": other, "to": "renamed.md"}).to_string()),
    );
    assert_eq!(r.status, 200, "mv: {}", r.body);
    let (c, _) = m.closed("moved").await;
    assert_eq!(c["reason"], "moved", "{c}");
    assert_eq!(c["retry"], false, "{c}");
    assert_eq!(c["path"], "renamed.md", "{c}");

    // ── 11. prev: a reconnect retires its old connection, and only its own ──
    let mut p1 = open(&live_url(port, OWNER, &doc, "")).await;
    let w1 = p1.next("welcome p1").await;
    let c1 = w1["client"].as_u64().expect("client");
    // Another person naming the owner's connection changes nothing.
    let mut foreign = open(&live_url(port, &member, &doc, &format!("&prev={c1}"))).await;
    assert_eq!(ty(&foreign.next("welcome foreign").await), "welcome");
    // An unknown id (and a malformed one) changes nothing either.
    let mut unknown = open(&live_url(port, OWNER, &doc, "&prev=123456789")).await;
    assert_eq!(ty(&unknown.next("welcome unknown").await), "welcome");
    let mut junk = open(&live_url(port, OWNER, &doc, "&prev=x")).await;
    assert_eq!(ty(&junk.next("welcome junk").await), "welcome");
    for (sock, who) in [(&mut foreign, "foreign"), (&mut unknown, "unknown"), (&mut junk, "junk")] {
        let _ = sock.sink.send(Message::Close(None)).await;
        let _ = who;
    }
    drop((foreign, unknown, junk));
    let v_p1 = w1["version"].as_u64().expect("v");
    p1.send(json!({"type":"since","op":1,"version":v_p1})).await;
    let (still, _) = p1.until("p1 still live", |f| ty(f) == "events" && f["op"] == 1).await;
    assert_eq!(ty(&still), "events", "a foreign or unknown prev left p1 alone");
    // The race: p1's tab reconnects as p2 (prev = p1's client); an edit that
    // p1's socket still carries after p2's `since` never applies.
    let (v_now, t_now) = daemon_text(port, &doc);
    let mut p2 = open(&live_url(port, OWNER, &doc, &format!("&resume={v_now}&prev={c1}"))).await;
    let w2 = p2.next("welcome p2").await;
    assert_eq!(ty(&w2), "welcome");
    assert_ne!(w2["client"], w1["client"]);
    p2.send(json!({"type":"since","op":1,"version":v_now})).await;
    let (s2, _) = p2.until("p2 since", |f| ty(f) == "events").await;
    assert_eq!(s2["events"].as_array().map(Vec::len), Some(0), "{s2}");
    let _ = p1
        .sink
        .send(Message::Text(json!({"type":"edit","op":2,"root":"source","base":v_now,"hunks":[{"start":0,"end":0,"insert":"RACE "}]}).to_string()))
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (v_after, t_after) = daemon_text(port, &doc);
    assert_eq!((v_after, t_after.as_str()), (v_now, t_now.as_str()), "the retired socket's edit never applied");
    p2.send(json!({"type":"since","op":2,"version":v_now})).await;
    let (s3, _) = p2.until("p2 since again", |f| ty(f) == "events" && f["op"] == 2).await;
    assert_eq!(s3["events"].as_array().map(Vec::len), Some(0), "nothing from p1 after p2's since: {s3}");
    let (c, _) = p1.closed("p1 retired").await;
    assert_eq!(c["reason"], "left", "{c}");
    let _ = p2.sink.send(Message::Close(None)).await;
    drop(p2);
    // The owner's other tab (A) was never touched: two tabs stay live.
    a.send(json!({"type":"since","op":60,"version":v_now})).await;
    let (sa, _) = a.until("A still live", |f| ty(f) == "events" && f["op"] == 60).await;
    assert_eq!(ty(&sa), "events");
    let _ = b.drain(300).await;

    // ── 9. A core that's off ──
    MARKDOWN_ON.store(false, Ordering::SeqCst);
    let mut off = open(&live_url(port, OWNER, &doc, "")).await;
    let (c, frame) = off.closed("core off at open").await;
    assert_eq!(c["reason"], "closed", "{c}");
    assert_eq!(c["retry"], false, "{c}");
    assert_eq!(c["code"], "core_off", "{c}");
    assert_eq!(c["core"], "markdown", "{c}");
    assert_eq!(frame.map(|f| u16::from(f.code)), Some(1000), "refused before any join");
    MARKDOWN_ON.store(true, Ordering::SeqCst);

    // Switching it off ends open sockets (close_core), after the write-back.
    let (vs, ts) = daemon_text(port, &doc);
    a.send(json!({"type":"edit","op":50,"root":"source","base":vs,"hunks":[{"start":ts.len(),"end":ts.len(),"insert":format!("last line {MARKER}\n")}]})).await;
    let (ack50, _) = a.until("ack 50", |f| f["op"] == 50).await;
    assert_eq!(ty(&ack50), "ack");
    let v50 = ack50["version"].as_u64().expect("v50");
    MARKDOWN_ON.store(false, Ordering::SeqCst);
    let told = k2_daemon::docs::live::close_core("markdown", Some("Anna"));
    assert!(told >= 2, "both sockets told: {told}");
    for (sock, who) in [(&mut a, "A"), (&mut b, "B")] {
        let (c, frame) = sock.closed(who).await;
        assert_eq!(c["reason"], "closed", "{who}: {c}");
        assert_eq!(c["retry"], false, "{who}: {c}");
        assert_eq!(c["code"], "core_off", "{who}: {c}");
        assert_eq!(c["core"], "markdown");
        assert_eq!(c["version"].as_u64(), Some(v50), "{who}: {c}");
        assert_eq!(c["written"], true, "{who}: {c}");
        assert!(c["message"].as_str().is_some_and(|m| m.contains("Anna")), "{c}");
        assert_eq!(frame.map(|f| u16::from(f.code)), Some(4010), "{who}");
    }
    let on_disk = std::fs::read_to_string(&doc).expect("file");
    assert_eq!(on_disk, format!("{ts}last line {MARKER}\n"), "the file holds every acknowledged edit when the sockets close");
    let mut left = k2_daemon::docs::live::open_sockets();
    for _ in 0..40 {
        if left.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        left = k2_daemon::docs::live::open_sockets();
    }
    assert!(left.is_empty(), "no Live socket left: {left:?}");

    // The gate alone (no close_core) ends a socket within a second or two.
    MARKDOWN_ON.store(true, Ordering::SeqCst);
    let mut late = open(&live_url(port, OWNER, &doc, "")).await;
    assert_eq!(ty(&late.next("welcome late").await), "welcome");
    MARKDOWN_ON.store(false, Ordering::SeqCst);
    let (c, frame) = late.closed("gate backstop").await;
    assert_eq!(c["code"], "core_off", "{c}");
    assert_eq!(frame.map(|f| u16::from(f.code)), Some(4010));
    MARKDOWN_ON.store(true, Ordering::SeqCst);

    k2_daemon::docs::live::set_core_gate(None);
    drop(daemon);
    let _ = std::fs::remove_dir_all(&tmp);
}
