//! The keyed watch on the docs socket (kit v0.16), headless: the real
//! dispatcher (`test_harness`), the real docs runtime (the vendored kit and
//! the pptx pack, a temp `~/.k2`), driven over HTTP and the docs socket with
//! no client.
//!
//! What it pins:
//!   1. A live deck's welcome lists `keyed_watch` in `features` (and keeps
//!      its `previews` for clients that never send `watch`).
//!   2. `watch`: the first numbered frames are `keyed_snapshot` (seq 1),
//!      then its `keyed_page`s in order, the slides by address.
//!   3. One edit (an agent verb through `/cli/docs/tool`) is one
//!      `ops_change`, the next `seq`, at the doc's new version; the watching
//!      socket gets no bare `change` frame for it.
//!   4. A socket that never sends `watch` keeps the old path: a `change`
//!      frame for the same edit, no watch frames.
//!   4b. The deck's welcome lists `view_address` and `time_position` (kit
//!      v0.18.0); one socket's view and playhead reach the other as roster
//!      events (`at_ms` stamped by the kit), a bad playhead is
//!      `presence_refused {req}` with `MPC-E114`, and a cleared view is null.
//!   5. A Markdown doc's welcome lists `view_address` only (no
//!      `keyed_watch`): its view reaches the roster, its `watch` and a
//!      `time_position` are refused (`MPC-E113`).
//!   6. Say something (`POST /cli/docs/say`): the deck's socket gets one
//!      `say` frame (to a person, trimmed, with its address); to the deck's
//!      agent it is also an inbox note; a reply names what it answers; GET
//!      is 405, an over-long text 400, an unknown recipient 404, an
//!      any-file doc 409 `no_live_room`; past a sender's burst, 429.
//!
//! The preview worker isn't built here: a slide's re-render may fail
//! (`preview_ready` with a failure), which this test ignores.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_daemon::test_harness;
use k2_mpc_pptx::testgen::{deck, DeckSpec, Slide};
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-keyed-watch";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
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

async fn flip(cores: &[(&str, bool)]) -> k2_daemon::docs::cores::Report {
    let after: std::collections::BTreeMap<String, bool> = cores.iter().map(|(c, on)| (c.to_string(), *on)).collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token"))
        .await
        .expect("flip")
}

/// The next text frame (Pings answered), within 60 s.
async fn next_frame(ws: &mut Ws, what: &str) -> J {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => panic!("timed out waiting for {what}"),
            Ok(None) => panic!("socket ended waiting for {what}"),
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}")),
            Ok(Some(Ok(Message::Ping(p)))) => ws.send(Message::Pong(p)).await.expect("pong"),
            Ok(Some(Ok(Message::Close(c)))) => panic!("socket closed ({c:?}) waiting for {what}"),
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("socket error {e} waiting for {what}"),
        }
    }
}

/// Every text frame for `ms` milliseconds.
async fn frames_for(ws: &mut Ws, ms: u64) -> Vec<J> {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(ms), async {
        loop {
            out.push(next_frame(ws, "frames").await);
        }
    })
    .await;
    out
}

/// The keyed watch's frames (and `watch_refused`) among others: roster,
/// host events and presence are never numbered.
fn is_watch(f: &J) -> bool {
    matches!(f["type"].as_str(), Some("keyed_snapshot" | "keyed_page" | "ops_change" | "watch_refused"))
}

async fn next_watch(ws: &mut Ws, what: &str) -> J {
    loop {
        let f = next_frame(ws, what).await;
        if is_watch(&f) {
            return f;
        }
    }
}

fn version(port: u16, path: &str) -> u64 {
    let (st, body) = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "status {path}: {body}");
    let v: J = serde_json::from_str(&body).expect("status JSON");
    v["version"].as_u64().unwrap_or_else(|| panic!("a version in {v}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_deck_is_watched_by_key_headless() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-keyed-watch-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

    let ws_dir = tmp.join("ws-deck");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'deckws', ?2, 'deckws')",
            params![uuid::Uuid::new_v4().to_string(), ws_dir.to_string_lossy()],
        )
        .expect("seed project");
    }
    let talk = ws_dir.join("talk.pptx");
    std::fs::write(
        &talk,
        deck(&DeckSpec::new(vec![
            Slide::titled(256, "Quarterly review", ""),
            Slide::titled(257, "Goals", "Grow the pilot"),
            Slide::titled(258, "Next steps", "Hire"),
        ])),
    )
    .expect("deck");
    let talk = talk.to_string_lossy().to_string();
    let notes = ws_dir.join("notes.md");
    std::fs::write(&notes, "# Notes\n\none\n").expect("notes");
    let notes = notes.to_string_lossy().to_string();

    let on = flip(&[("any-file", true), ("slides", true), ("markdown", true)]).await;
    assert!(on.changed.contains(&("slides".to_string(), true)), "{on:?}");

    // ── 1. The welcome lists the keyed watch ──
    let url = |doc: &str| format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}", enc(doc));
    let (mut watcher, _) = tokio_tungstenite::connect_async(&url(&talk)).await.expect("the docs socket opens on a deck");
    let welcome = next_frame(&mut watcher, "welcome").await;
    assert_eq!(welcome["type"], "welcome", "{welcome}");
    assert_eq!(welcome["wire"], 3, "the wire stays 3: {welcome}");
    assert_eq!(welcome["mode"], "deck", "{welcome}");
    assert_eq!(welcome["features"], json!(["keyed_watch", "view_address", "time_position"]), "{welcome}");
    assert!(welcome.get("previews").is_some(), "the previews stay for clients that never watch: {welcome}");
    // A second socket on the deck that never sends `watch` (an older client).
    let (mut old, _) = tokio_tungstenite::connect_async(&url(&talk)).await.expect("a second socket");
    let w_old = next_frame(&mut old, "the old client's welcome").await;
    assert_eq!(w_old["features"], json!(["keyed_watch", "view_address", "time_position"]), "{w_old}");

    // ── 2. watch: the snapshot head, then its pages ──
    watcher.send(Message::Text(json!({"type": "watch"}).to_string())).await.expect("send watch");
    let head = next_watch(&mut watcher, "keyed_snapshot").await;
    assert_eq!(head["type"], "keyed_snapshot", "the first watch frame is the head: {head}");
    assert_eq!(head["seq"], 1, "{head}");
    let upto = head["upto"].as_u64().expect("upto");
    let pages = head["pages"].as_u64().expect("pages");
    assert!(pages >= 1, "{head}");
    let roots: Vec<&str> = head["schema"]["roots"].as_array().expect("roots").iter().map(|r| r["name"].as_str().expect("name")).collect();
    assert!(roots.contains(&"slides"), "the pptx pack's roots: {head}");
    let mut seq = 1;
    let mut slides = Vec::new();
    for page in 1..=pages {
        let p = next_watch(&mut watcher, "keyed_page").await;
        seq += 1;
        assert_eq!(p["type"], "keyed_page", "{p}");
        assert_eq!(p["seq"], seq, "numbered in order: {p}");
        assert_eq!(p["page"], page, "{p}");
        assert_eq!((p["pages"].as_u64(), p["upto"].as_u64()), (Some(pages), Some(upto)), "{p}");
        if p["root"] == "slides" {
            if let Some(live) = p["live"].as_array() {
                slides.extend(live.iter().map(|e| e[0].as_str().expect("address").to_string()));
            }
        }
    }
    assert_eq!(slides, ["@sld:256", "@sld:257", "@sld:258"], "slides by address");
    // The old socket sees none of it.
    assert!(frames_for(&mut old, 500).await.iter().all(|f| !is_watch(f)), "no watch frames without `watch`");

    // ── 3. One edit, one ops_change, the next seq ──
    let (st, body) = http(
        port,
        "POST",
        &format!("/cli/docs/tool?token={OWNER}&path={}", enc(&talk)),
        Some(&json!({"argv": ["shape", "text", "@sld:257/sp:2", "--text", "Goals, revised"]}).to_string()),
    );
    assert_eq!(st, 200, "shape text: {body}");
    let out: J = serde_json::from_str(&body).expect("tool JSON");
    assert_eq!(out["exit"], 0, "{out}");
    let rev = version(port, &talk);
    let ch = next_watch(&mut watcher, "ops_change").await;
    assert_eq!(ch["type"], "ops_change", "{ch}");
    assert_eq!(ch["seq"], seq + 1, "the next seq: {ch}");
    assert_eq!(ch["rev"], rev, "{ch}");
    assert_eq!(ch["verb"], "shape text", "{ch}");
    assert!(!ch["ops"].as_array().expect("ops").is_empty(), "{ch}");
    let rest = frames_for(&mut watcher, 1500).await;
    assert!(rest.iter().all(|f| !is_watch(f)), "one frame for one change: {rest:?}");
    assert!(rest.iter().all(|f| f["type"] != "change"), "a watching socket gets ops_change, not change: {rest:?}");

    // ── 4. The old client keeps the old path ──
    let mut change = None;
    while change.is_none() {
        let f = next_frame(&mut old, "the old client's change").await;
        assert!(!is_watch(&f), "{f}");
        if f["type"] == "change" {
            change = Some(f);
        }
    }
    let change = change.expect("change");
    assert_eq!(change["event"]["verb"], "shape text", "{change}");
    assert_eq!(change["event"]["version"], rev, "{change}");

    // ── 4b. Presence: a view and a playhead reach the other socket ──
    let me = welcome["self"].as_str().expect("self").to_string();
    let my_client = welcome["client"].as_u64().expect("client");
    watcher.send(Message::Text(json!({"type": "view_address", "req": 1, "address": "@sld:257"}).to_string())).await.expect("send view");
    watcher
        .send(Message::Text(
            json!({"type": "time_position", "req": 2, "time_position": {"at": "48000", "state": "playing", "in": "@sld:257", "rate": "1/2"}})
                .to_string(),
        ))
        .await
        .expect("send time");
    let (mut view, mut time) = (None, None);
    while view.is_none() || time.is_none() {
        let f = next_frame(&mut old, "the roster's view and time").await;
        if f["type"] == "roster_event" && f["event"]["client"] == my_client {
            match f["event"]["kind"].as_str() {
                Some("view_address") => view = Some(f["event"].clone()),
                Some("time_position") => time = Some(f["event"].clone()),
                _ => {}
            }
        }
    }
    let (view, time) = (view.expect("view"), time.expect("time"));
    assert_eq!(view, json!({"kind": "view_address", "who": me, "view_address": "@sld:257", "client": my_client}), "{view}");
    assert_eq!(time["who"], me.as_str(), "{time}");
    assert_eq!(
        (&time["time_position"]["at"], &time["time_position"]["state"], &time["time_position"]["in"], &time["time_position"]["rate"]),
        (&json!("48000"), &json!("playing"), &json!("@sld:257"), &json!("1/2")),
        "{time}"
    );
    assert!(time["time_position"]["at_ms"].as_u64().is_some_and(|ms| ms > 0), "the kit stamps at_ms: {time}");
    // A value the kit won't take: `presence_refused` with the frame's req, nothing changes.
    watcher
        .send(Message::Text(json!({"type": "time_position", "req": 3, "time_position": {"at": "1.5", "state": "paused"}}).to_string()))
        .await
        .expect("send a bad time");
    let mut refused = None;
    while refused.is_none() {
        let f = next_frame(&mut watcher, "presence_refused").await;
        if f["type"] == "presence_refused" {
            refused = Some(f);
        }
    }
    let refused = refused.expect("refused");
    assert_eq!(refused["req"], 3, "{refused}");
    assert_eq!(refused["error"]["id"], "MPC-E114", "{refused}");
    // Clearing the view goes out as null.
    watcher.send(Message::Text(json!({"type": "view_address", "req": 4, "address": null}).to_string())).await.expect("send clear");
    let mut cleared = None;
    while cleared.is_none() {
        let f = next_frame(&mut old, "the cleared view").await;
        if f["type"] == "roster_event" && f["event"]["kind"] == "view_address" && f["event"]["client"] == my_client {
            cleared = Some(f["event"].clone());
        }
    }
    assert_eq!(cleared.expect("cleared")["view_address"], J::Null);
    drop(old);
    drop(watcher);

    // ── 5. A text doc lists no keyed watch and refuses `watch` ──
    let (mut md, _) = tokio_tungstenite::connect_async(&url(&notes)).await.expect("a socket on a Markdown doc");
    let w = next_frame(&mut md, "the Markdown welcome").await;
    assert_eq!(w["type"], "welcome", "{w}");
    assert_eq!(w["root"], "source", "{w}");
    assert_eq!(w["features"], json!(["view_address"]), "a text doc: the view address, no keyed watch, no playhead: {w}");
    // Its view address is taken (no reply) and reaches the roster.
    let md_client = w["client"].as_u64().expect("client");
    md.send(Message::Text(json!({"type": "view_address", "req": 1, "address": "@k:intro"}).to_string())).await.expect("send view");
    let mut seen = None;
    while seen.is_none() {
        let f = next_frame(&mut md, "the text doc's view on the roster").await;
        assert_ne!(f["type"], "presence_refused", "{f}");
        assert_ne!(f["type"], "error", "{f}");
        if f["type"] == "roster_event" && f["event"]["kind"] == "view_address" && (f["event"]["client"] == md_client || f["event"]["client"].is_null()) {
            seen = Some(f["event"]["view_address"].clone());
        }
    }
    assert_eq!(seen.expect("view"), json!("@k:intro"));
    // A presence frame it doesn't list is refused as unreadable (no op: `error`).
    md.send(Message::Text(json!({"type": "time_position", "req": 2, "time_position": {"at": "0", "state": "paused"}}).to_string()))
        .await
        .expect("send time");
    let mut err = None;
    while err.is_none() {
        let f = next_frame(&mut md, "the playhead refusal").await;
        if f["type"] == "error" {
            err = Some(f);
        }
    }
    let err = err.expect("error");
    assert_eq!(err["error"]["id"], "MPC-E113", "{err}");
    assert!(err["error"]["message"].as_str().is_some_and(|m| m.contains("time_position")), "{err}");
    md.send(Message::Text(json!({"type": "watch"}).to_string())).await.expect("send watch");
    let mut err = None;
    while err.is_none() {
        let f = next_frame(&mut md, "the refusal").await;
        assert!(!is_watch(&f), "no watch frames on a text doc: {f}");
        if f["type"] == "error" {
            err = Some(f);
        }
    }
    let err = err.expect("error");
    assert_eq!(err["error"]["id"], "MPC-E113", "{err}");
    drop(md);

    // ── 6. Say something: a POST into the deck's room reaches its sockets ──
    let (mut room, _) = tokio_tungstenite::connect_async(&url(&talk)).await.expect("a socket in the deck's room");
    let w_room = next_frame(&mut room, "the room's welcome").await;
    assert_eq!(w_room["type"], "welcome", "{w_room}");
    let say = |body: J| http(port, "POST", &format!("/cli/docs/say?token={OWNER}"), Some(&body.to_string()));
    let (st, body) = say(json!({"path": talk, "text": "  Can you lighten slide 2?  ", "to": "owner", "at": "@sld:257"}));
    assert_eq!(st, 200, "say: {body}");
    let out: J = serde_json::from_str(&body).expect("say JSON");
    let id = out["say"]["id"].as_str().expect("an id").to_string();
    assert!(id.starts_with("say_"), "{out}");
    assert_eq!(out["say"]["to"], "person:owner", "{out}");
    assert!(out["sockets"].as_u64().is_some_and(|n| n >= 1), "the room's socket took it: {out}");
    assert_eq!(out["inbox"], false, "a person watches live; no inbox note: {out}");
    let mut frame = None;
    while frame.is_none() {
        let f = next_frame(&mut room, "the say frame").await;
        if f["type"] == "say" {
            frame = Some(f);
        }
    }
    let frame = frame.expect("say");
    assert_eq!(frame["id"], id.as_str(), "{frame}");
    assert_eq!(frame["text"], "Can you lighten slide 2?", "trimmed: {frame}");
    assert_eq!((&frame["at"], &frame["time"], &frame["reply_to"]), (&json!("@sld:257"), &J::Null, &J::Null), "{frame}");
    assert_eq!((&frame["from"]["id"], &frame["from"]["kind"]), (&json!("person:owner"), &json!("person")), "{frame}");
    assert_eq!(frame["to"]["id"], "person:owner", "{frame}");
    assert!(frame["at_ms"].as_u64().is_some_and(|ms| ms > 0), "{frame}");

    // To the deck's agent (by its workspace): an inbox note too, and its reply.
    let (st, body) = say(json!({"path": talk, "text": "Lamp a bit left?", "to": "deckws", "time": "48000"}));
    assert_eq!(st, 200, "say to an agent: {body}");
    let to_agent: J = serde_json::from_str(&body).expect("JSON");
    assert!(to_agent["say"]["to"].as_str().is_some_and(|p| p.starts_with("agent:")), "{to_agent}");
    assert_eq!(to_agent["inbox"], true, "the agent holds no socket: an inbox note: {to_agent}");
    let (st, body) = say(json!({"path": talk, "text": "Done.", "reply_to": id}));
    assert_eq!(st, 200, "a reply to the room: {body}");
    let mut reply = None;
    while reply.is_none() {
        let f = next_frame(&mut room, "the reply").await;
        if f["type"] == "say" && f["reply_to"] == id.as_str() {
            reply = Some(f);
        }
    }
    assert_eq!(reply.expect("reply")["to"], J::Null, "to the room");

    // Refusals: GET (405), a message over the cap (400), someone unknown (404),
    // an any-file doc (no live room, 409).
    let (st, _) = http(port, "GET", &format!("/cli/docs/say?token={OWNER}&path={}&text=hi", enc(&talk)), None);
    assert_eq!(st, 405, "POST only");
    let (st, body) = say(json!({"path": talk, "text": "x".repeat(1025)}));
    assert_eq!(st, 400, "{body}");
    let (st, body) = say(json!({"path": talk, "text": "hi", "to": "nobody-here"}));
    assert_eq!(st, 404, "{body}");
    let photo = ws_dir.join("photo.png");
    std::fs::write(&photo, [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3]).expect("png");
    let (st, body) = say(json!({"path": photo.to_string_lossy(), "text": "hi"}));
    assert_eq!(st, 409, "{body}");
    assert!(body.contains("no_live_room"), "{body}");

    // The rate: a burst of 5 a sender (3 sent so far, refusals don't count),
    // then 429 with when to try again. Sent back to back, so at most a token
    // or two refills meanwhile.
    let mut more = 0;
    let body = loop {
        let (st, body) = say(json!({"path": talk, "text": format!("more {more}")}));
        if st == 429 {
            break body;
        }
        assert_eq!(st, 200, "within the burst: {body}");
        more += 1;
        assert!(more <= 4, "no 429 after {more} more messages: the burst is 5");
    };
    assert!(more >= 2, "the 3 refused calls didn't use the burst: 2 more went first, not {more}");
    let limited: J = serde_json::from_str(&body).expect("JSON");
    assert_eq!(limited["error"]["code"], "rate_limited", "{limited}");
    assert!(limited["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 3000), "{limited}");
}
