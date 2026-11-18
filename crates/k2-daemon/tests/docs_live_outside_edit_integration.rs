//! An agent edits an open HTML page the old way (Rosson 2026-11-18): the
//! page is open in a person's Live tab, and the agent's own write tool
//! rewrites the file on disk, in place and as a temp file renamed over it.
//! Then the tab closes and opens again.
//!
//! Pinned, over the real dispatcher and docs runtime (temp HOME):
//!   - each outside write is a new version on the open socket (a `change`
//!     from `kit:external`), never a `closed` frame;
//!   - the daemon's text is the file's new text after each one;
//!   - a reopen after them is welcomed with the latest text (nothing stored
//!     keeps refusing the file), and so is one after the tab closed first.
//!
//! Every frame is printed (`--no-capture`), so a failure shows the Closed
//! frame's code.
//!
//! ISOLATION: one test (the docs runtime is process-wide).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::StreamExt;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-outside-edit";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_else(|| panic!("no body: {text:?}"));
    (status, body)
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

/// The doc's text as the daemon reads it now.
fn daemon_text(port: u16, path: &str) -> String {
    let (status, body) = http_get(port, &format!("/cli/docs/read?token={OWNER}&path={}", enc(path)));
    assert_eq!(status, 200, "read: {body}");
    let j: J = serde_json::from_str(&body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"));
    j["text"].as_str().unwrap_or_else(|| panic!("text: {j}")).to_string()
}

#[derive(Debug)]
enum In {
    Text(J),
    Ping,
    Close,
}

struct Live {
    _sink: SplitSink<Ws, Message>,
    rx: mpsc::UnboundedReceiver<In>,
}

fn reader(mut read: SplitStream<Ws>) -> mpsc::UnboundedReceiver<In> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(m) = read.next().await {
            let item = match m {
                Ok(Message::Text(t)) => In::Text(serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"))),
                Ok(Message::Ping(_)) => In::Ping,
                Ok(Message::Close(_)) => In::Close,
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

async fn open(port: u16, doc: &str) -> Live {
    let url = format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}&root=source", enc(doc));
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap_or_else(|e| panic!("Live socket must open: {e}"));
    let (sink, read) = ws.split();
    Live { _sink: sink, rx: reader(read) }
}

impl Live {
    /// Every text frame within `ms`, printed; a close ends it (and is a frame of its own here).
    async fn drain(&mut self, what: &str, ms: u64) -> (Vec<J>, bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(left, self.rx.recv()).await {
                Err(_) => return (out, false),
                Ok(None) | Ok(Some(In::Close)) => {
                    println!("[outside-edit] {what}: socket closed");
                    return (out, true);
                }
                Ok(Some(In::Text(j))) => {
                    let mut shown = j.to_string();
                    if shown.len() > 400 {
                        shown.truncate(400);
                        shown.push('…');
                    }
                    println!("[outside-edit] {what}: {shown}");
                    out.push(j);
                }
                Ok(Some(In::Ping)) => {}
            }
        }
    }

    /// The first frame (the welcome, or the refusal).
    async fn first(&mut self, what: &str) -> J {
        loop {
            match timeout(Duration::from_secs(10), self.rx.recv()).await {
                Err(_) => panic!("{what}: no first frame in 10 s"),
                Ok(None) | Ok(Some(In::Close)) => panic!("{what}: closed before any frame"),
                Ok(Some(In::Ping)) => continue,
                Ok(Some(In::Text(j))) => {
                    println!("[outside-edit] {what}: first frame type={} code={} reason={}", j["type"], j["code"], j["reason"]);
                    return j;
                }
            }
        }
    }
}

fn page(edition: u32) -> String {
    let mut p = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Progress</title>\n<style>table {{ border-collapse: collapse; }} td {{ padding: 4px; }}</style></head><body>\n<h1>Progress, edition {edition}</h1>\n<table><tr><th>Slice</th><th>Commit</th><th>Status</th></tr>\n"
    );
    let mut n = 0u64;
    while p.len() < 9_800 {
        n += 1;
        p.push_str(&format!("<tr><td>slice {n}</td><td><code>{:016x}{:016x}</code></td><td>landed</td></tr>\n", n.wrapping_mul(0x9e37_79b9_7f4a_7c15), n.wrapping_mul(0xc2b2_ae3d_27d4_eb4f) ^ u64::from(edition)));
    }
    p.push_str("</table>\n<pre><code>k5 msg send --to builder \"done\"</code></pre>\n</body></html>\n");
    p
}

/// The doc's text reaches `want` within 10 s (the watcher's ingest), else the last text seen.
async fn settles(port: u16, doc: &str, want: &str) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..100 {
        last = daemon_text(port, doc);
        if last == want {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last)
}

fn no_close(frames: &[J], closed: bool, what: &str) {
    let c: Vec<&J> = frames.iter().filter(|f| f["type"] == "closed").collect();
    assert!(c.is_empty() && !closed, "{what}: the socket closed: {c:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outside_edit_to_an_open_html_page_is_a_new_version_and_reopen_works() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-outside-edit-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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
    let after: std::collections::BTreeMap<String, bool> = [("markdown".to_string(), true), ("text".to_string(), true)].into();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token")).await.expect("cores on");

    let ws = tmp.join("ws-agent");
    std::fs::create_dir_all(ws.join(".k2")).expect("ws");
    let ws = ws.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'agentws', ?2, 'agentws')", params![uuid::Uuid::new_v4().to_string(), ws.to_string_lossy()])
            .expect("seed project");
    }
    let file = ws.join("progress.html");
    std::fs::write(&file, page(1)).expect("page 1");
    let doc = file.to_string_lossy().to_string();

    // The tab opens the page live.
    let mut a = open(port, &doc).await;
    let w = a.first("open").await;
    assert_eq!(w["type"], "welcome", "the page opens live: {w}");
    assert_eq!(w["text"].as_str(), Some(page(1).as_str()), "welcome carries the page");

    // 1. The agent's write tool, in place (truncate, then write).
    std::fs::write(&file, page(2)).expect("page 2 in place");
    let settled = settles(port, &doc, &page(2)).await;
    let (frames, closed) = a.drain("after the in-place write", 1_500).await;
    no_close(&frames, closed, "in-place write");
    settled.unwrap_or_else(|last| panic!("in-place write: the doc never took the new text (now {} bytes): {frames:?}", last.len()));
    assert!(frames.iter().any(|f| f["type"] == "change" && f["event"]["pid"] == "kit:external"), "in-place write: a change from the outside save: {frames:?}");

    // 2. A temp file renamed over the page.
    let tmp_file = ws.join(".progress.html.tmp-4821");
    std::fs::write(&tmp_file, page(3)).expect("temp");
    std::fs::rename(&tmp_file, &file).expect("rename over");
    let settled = settles(port, &doc, &page(3)).await;
    let (frames, closed) = a.drain("after the temp-plus-rename", 1_500).await;
    no_close(&frames, closed, "temp-plus-rename");
    settled.unwrap_or_else(|last| panic!("temp-plus-rename: the doc never took the new text (now {} bytes): {frames:?}", last.len()));

    // 3. Another socket (a second tab) opens it now: welcomed, latest text.
    let mut b = open(port, &doc).await;
    let w = b.first("a second tab").await;
    assert_eq!(w["type"], "welcome", "a second tab after the outside writes opens live: {w}");
    assert_eq!(w["text"].as_str(), Some(page(3).as_str()), "the latest text");
    drop(b);

    // 4. The tab closes; the agent writes again with nobody open; the tab reopens.
    drop(a);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    std::fs::write(&tmp_file, page(4)).expect("temp 4");
    std::fs::rename(&tmp_file, &file).expect("rename 4");
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let mut c = open(port, &doc).await;
    let w = c.first("reopen").await;
    assert_eq!(w["type"], "welcome", "the reopened tab opens live: {w}");
    assert_eq!(w["text"].as_str(), Some(page(4).as_str()), "the reopened tab has the file's text");
    let (frames, closed) = c.drain("after reopen", 800).await;
    no_close(&frames, closed, "reopen");
}
