//! Grouped saves (kit v0.23.0, Rosson's decision for 0.46.17): the docs
//! store waits for the disk once per group of changes, and K2 puts every
//! change on disk at its barriers.
//!
//! Pinned, over the real dispatcher and docs runtime (temp HOME):
//!   - the store opens grouped (32 changes or 100 ms) by default;
//!   - the barriers: an explicit save (`POST /cli/docs/flush`, which says
//!     `durable: true`), a tab going out of view (`visible: false`), a tab's
//!     close, and the docs going idle (every change on disk soon after);
//!   - the owner's "Save every change immediately" (`docsSaveImmediately`):
//!     a GET on the settings write is 405 and changes nothing; the owner's
//!     POST lands, and the docs say a restart is needed for it.
//!
//! ISOLATION: one test (the docs runtime is process-wide).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-grouped-saves";

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(req.as_bytes()).expect("write");
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

fn json_of(body: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
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

/// The pool's `saves` block, now.
fn saves() -> J {
    let p = k2_daemon::docs::pool_json();
    p.get("saves").cloned().unwrap_or_else(|| panic!("the pool has no saves block: {p}"))
}

fn flushes(why: &str) -> u64 {
    let s = saves();
    s["flushes"][why].as_u64().unwrap_or_else(|| panic!("no {why} count: {s}"))
}

/// `cond` holds within 5 s, else the last `saves` block.
async fn within_5s(what: &str, cond: impl Fn(&J) -> bool) {
    let mut last = J::Null;
    for _ in 0..100 {
        last = saves();
        if cond(&last) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what}: not within 5 s; saves now {last}");
}

/// An owner's edit through the docs routes (an agent's `k2 doc append`).
fn append(port: u16, doc: &str, line: &str) {
    let body = json!({ "path": doc, "argv": ["append"], "stdin": line }).to_string();
    let (status, body) = http(port, "POST", &format!("/cli/docs/tool?token={OWNER}"), Some(&body));
    assert_eq!(status, 200, "append: {body}");
    assert_eq!(json_of(&body)["exit"], 0, "append applied: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grouped_saves_reach_the_disk_at_every_barrier_and_the_owner_can_turn_them_off() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-grouped-saves-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("home");
    let tmp = tmp.canonicalize().expect("canonical home");
    std::env::set_var("HOME", &tmp);
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }

    let daemon = test_harness::start(OWNER).await;
    let port = daemon.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "the docs runtime is up: {}", k2_daemon::docs::pool_json());
    let after: std::collections::BTreeMap<String, bool> = [("markdown".to_string(), true)].into();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token")).await.expect("cores on");

    // The default: grouped, 32 changes or 100 ms.
    let s = saves();
    assert_eq!(s["commits"], "grouped", "grouped saves by default: {s}");
    assert_eq!((s["groupMaxChanges"].as_u64(), s["groupMaxMs"].as_u64()), (Some(32), Some(100)), "{s}");
    assert_eq!(s["saveEveryChangeImmediately"], false, "{s}");
    assert_eq!(s["restartNeeded"], false, "{s}");

    let ws = tmp.join("ws-saves");
    std::fs::create_dir_all(ws.join(".k2")).expect("ws");
    let ws = ws.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'savesws', ?2, 'savesws')", params![uuid::Uuid::new_v4().to_string(), ws.to_string_lossy()])
            .expect("seed project");
    }
    let file = ws.join("notes.md");
    std::fs::write(&file, "# Notes\n\none\n").expect("notes");
    let doc = file.to_string_lossy().to_string();

    // A tab opens the doc live.
    let url = format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}&root=source", enc(&doc));
    let (live, _) = tokio_tungstenite::connect_async(&url).await.unwrap_or_else(|e| panic!("Live socket must open: {e}"));
    let (mut sink, mut read) = live.split();
    let welcome = loop {
        match timeout(Duration::from_secs(10), read.next()).await {
            Err(_) => panic!("no welcome in 10 s"),
            Ok(None) => panic!("closed before the welcome"),
            Ok(Some(Err(e))) => panic!("socket error before the welcome: {e}"),
            Ok(Some(Ok(Message::Text(t)))) => break json_of(&t),
            Ok(Some(Ok(_))) => continue,
        }
    };
    assert_eq!(welcome["type"], "welcome", "the doc opens live: {welcome}");
    // Keep reading (pings, changes) so the socket stays healthy.
    tokio::spawn(async move { while let Some(Ok(_)) = read.next().await {} });

    // 1. An explicit save.
    append(port, &doc, "two\n");
    let before = flushes("save");
    let (status, body) = http(port, "POST", &format!("/cli/docs/flush?token={OWNER}"), Some(&json!({ "path": doc }).to_string()));
    assert_eq!(status, 200, "flush: {body}");
    assert_eq!(json_of(&body)["durable"], true, "an explicit save is on disk: {body}");
    assert_eq!(flushes("save"), before + 1, "the save flushed the store");
    let s = saves();
    assert_eq!(s["commitsOnDisk"], s["commitsMade"], "every change on disk after a save: {s}");

    // 2. The tab goes out of view.
    append(port, &doc, "three\n");
    let before = flushes("switch_away");
    sink.send(Message::Text(json!({ "type": "visible", "visible": false }).to_string().into())).await.expect("send visible false");
    within_5s("switch-away flushes", |s| s["flushes"]["switch_away"].as_u64() == Some(before + 1)).await;
    sink.send(Message::Text(json!({ "type": "visible", "visible": true }).to_string().into())).await.expect("send visible true");

    // 3. Idle: a change with no barrier reaches the disk soon after.
    append(port, &doc, "four\n");
    within_5s("idle puts every change on disk", |s| s["commitsMade"].as_u64().is_some() && s["commitsOnDisk"] == s["commitsMade"]).await;

    // 4. The tab closes.
    let before = flushes("close");
    sink.send(Message::Close(None)).await.expect("close the socket");
    drop(sink);
    within_5s("a tab's close flushes", |s| s["flushes"]["close"].as_u64().is_some_and(|n| n > before)).await;

    // The owner's setting: a GET on the settings write is 405 and changes nothing.
    let (status, body) = http(port, "GET", &format!("/cli/settings/update?token={OWNER}&docsSaveImmediately=true"), None);
    assert_eq!(status, 405, "GET on the settings write: {body}");
    assert!(!k2_core::app_settings::load().docs_save_immediately, "a GET wrote nothing");
    // The owner turns it on: it lands, and takes effect after a restart.
    let (status, body) = http(port, "POST", &format!("/cli/settings/update?token={OWNER}"), Some(r#"{"docsSaveImmediately":true}"#));
    assert_eq!(status, 200, "the owner's write: {body}");
    assert!(k2_core::app_settings::load().docs_save_immediately, "the setting is on");
    let (status, body) = http(port, "GET", &format!("/cli/docs/cores?token={OWNER}"), None);
    assert_eq!(status, 200, "cores: {body}");
    let c = json_of(&body);
    assert_eq!(
        c["saves"],
        json!({ "saveEveryChangeImmediately": true, "canChange": true, "running": "grouped", "restartNeeded": true }),
        "Settings shows the owner's setting and that a restart is needed: {c}"
    );
    let s = saves();
    assert_eq!((s["commits"].as_str(), s["restartNeeded"].as_bool()), (Some("grouped"), Some(true)), "still grouped until a restart: {s}");
}
