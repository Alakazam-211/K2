//! The timeline, fab and blend cores, headless: the real dispatcher
//! (`test_harness`), the real docs runtime (the vendored kit and the
//! timeline, fab and BLEND packs, a temp `~/.k2`), driven over HTTP and the
//! docs socket with no client.
//!
//! What it pins:
//!   1. Off (the default), each file is the any-file core's
//!      (`docs/cores?path`); on, its own core's, with no live editor for
//!      people (`liveView: false`).
//!   2. A live timeline (`.otio`, the timeline core on): the docs socket's
//!      welcome (`root: tracks`, `family`/`mode: timeline`, `features`:
//!      keyed watch, view address and time position), with `root=source`
//!      in the URL as the text views send it; its playhead reaches another
//!      socket; the pack's verbs (`timeline info`, `track list`) answer.
//!   3. A timeline the pack declines at open: `closed {reason: declined}`,
//!      the kit's report with the pack's code in `error.details.code`.
//!   4. Say something on a timeline with `time: {at, in}`: the frame carries
//!      the time and the track it is in.
//!   5. The index verbs on files that stay tier 0 while their core is on:
//!      `clip list` on an `.fcpxml`, `feature list` on a photoplot file,
//!      `hole list` on a drill file.
//!   6. A live scene (`.blend`, the blend core on): `mode: scene`, `root:
//!      colls`, `features` keyed watch, view address and presence, `rights:
//!      read` with `read_only_reason`, and `previews` listing the scene's
//!      view, its cameras and its objects by their live keys; `scene list`
//!      and `object show` answer; a selection by address and a pointer reach
//!      another socket as roster events; say works in the scene's room.
//!   6b. Nothing writes a `.blend` for now: the pack's edit verbs and the
//!      write routes are refused `409 blend_writer_disabled` (exit 3, the
//!      fixed words), the file untouched; no claims.
//!   6c. A person's `edit`, `undo` and `redo` on the scene's socket are
//!      `refused` with `blend_writer_disabled` in `error.details.code`.
//!   6d. After every refusal the scene reads as before (the watch snapshot,
//!      `object show`, `scene list`, the previews, an `as-is` export of the
//!      original bytes), at the same version, its disk state `clean`, also
//!      after disk ticks: no refused change sits in the doc.
//!   7. Turning the fab core off closes its files and they are any-file
//!      docs again (no index verbs).
//!
//! The scene worker isn't built here: a render may fail (`preview_ready`
//! with a failure), which this test ignores.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-pack-cores";

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

/// The next frame of `ty`, skipping others.
async fn next_of(ws: &mut Ws, ty: &str) -> J {
    loop {
        let f = next_frame(ws, ty).await;
        if f["type"] == ty {
            return f;
        }
    }
}

fn file_core(port: u16, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/cores?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "cores {path}: {body}");
    let v: J = serde_json::from_str(&body).expect("cores JSON");
    v["file"].clone()
}

/// `/cli/docs/status` on `path` (200).
fn doc_status(port: u16, path: &str) -> J {
    let (st, body) = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(path)), None);
    assert_eq!(st, 200, "status {path}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("status JSON ({e}): {body}"))
}

/// `/cli/docs/tool` on `path`: (status, the answer).
fn tool(port: u16, path: &str, argv: &[&str]) -> (u16, J) {
    let (st, body) = http(port, "POST", &format!("/cli/docs/tool?token={OWNER}&path={}", enc(path)), Some(&json!({ "argv": argv }).to_string()));
    (st, serde_json::from_str(&body).unwrap_or_else(|e| panic!("tool JSON ({e}): {body}")))
}

/// An index verb on a tier-0 doc: retried while its index is being built
/// (`index_pending`, exit 1), at most 60 s. The answer, exit 0.
fn index_verb(port: u16, path: &str, argv: &[&str]) -> J {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (st, v) = tool(port, path, argv);
        if st == 200 && v["exit"] == 0 {
            return v;
        }
        let pending = v.to_string().contains("index_pending");
        assert!(pending, "{argv:?} on {path}: {st} {v}");
        assert!(std::time::Instant::now() < deadline, "{argv:?} on {path}: still pending: {v}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn rt(value: &str) -> J {
    json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": value.parse::<f64>().expect("a number")})
}

fn tr(start: &str, dur: &str) -> J {
    json!({"OTIO_SCHEMA": "TimeRange.1", "duration": rt(dur), "start_time": rt(start)})
}

fn otio_clip(name: &str, src: &str, dur: &str) -> J {
    json!({
        "OTIO_SCHEMA": "Clip.1",
        "metadata": {},
        "name": name,
        "source_range": tr(src, dur),
        "effects": [],
        "markers": [],
        "enabled": true,
        "media_reference": {
            "OTIO_SCHEMA": "ExternalReference.1",
            "metadata": {},
            "name": "",
            "available_range": tr("0.0", "100000.0"),
            "available_image_bounds": null,
            "target_url": format!("file:///media/{name}.mov"),
        },
    })
}

/// A one-track OTIO timeline of two clips.
fn otio() -> Vec<u8> {
    let track = json!({
        "OTIO_SCHEMA": "Track.1",
        "metadata": {},
        "name": "V1",
        "source_range": null,
        "effects": [],
        "markers": [],
        "enabled": true,
        "children": [otio_clip("Shot A", "100.0", "48.0"), otio_clip("Shot B", "0.0", "24.0")],
        "kind": "Video",
    });
    let t = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "metadata": {},
        "name": "cut",
        "global_start_time": rt("86400.0"),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "metadata": {},
            "name": "tracks",
            "source_range": null,
            "effects": [],
            "markers": [],
            "enabled": true,
            "children": [track],
        },
    });
    let mut b = serde_json::to_vec_pretty(&t).expect("OTIO JSON");
    b.push(b'\n');
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeline_fab_and_blend_cores_headless() {
    let tmp = std::env::temp_dir().join(format!("k2-docs-pack-cores-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
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

    let ws_dir = tmp.join("ws-packs");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'packws', ?2, 'packws')",
            params![uuid::Uuid::new_v4().to_string(), ws_dir.to_string_lossy()],
        )
        .expect("seed project");
    }
    let put = |name: &str, bytes: &[u8]| -> String {
        let p = ws_dir.join(name);
        std::fs::write(&p, bytes).expect("write a file");
        p.to_string_lossy().to_string()
    };
    let cut = put("cut.otio", &otio());
    let junk = put("broken.otio", b"{ this is not a timeline\n");
    let fcpxml = put("edit.fcpxml", include_bytes!("../../../vendor/k2-mpc-timeline/crates/timeline/tests/data/seed.fcpxml"));
    let gbr = put("board.gbr", include_bytes!("../../../vendor/k2-mpc-fab/crates/fab/tests/data/board.gbr"));
    let xnc = put("board.xnc", include_bytes!("../../../vendor/k2-mpc-fab/crates/fab/tests/data/board.xnc"));
    let room = put("room.blend", &k2_mpc_blend::testgen::scenes::living_room());

    // ── 1. Off: the any-file core's; on: each its own, no live editor ──
    for p in [&cut, &fcpxml, &gbr, &xnc, &room] {
        assert_eq!(file_core(port, p), json!({"core": "any-file", "on": false, "liveView": false}), "{p}");
    }
    let on = flip(&[("any-file", true), ("timeline", true), ("fab", true), ("blend", true)]).await;
    for core in ["timeline", "fab", "blend"] {
        assert!(on.changed.contains(&(core.to_string(), true)), "{core}: {on:?}");
    }
    for (p, core) in [(&cut, "timeline"), (&fcpxml, "timeline"), (&gbr, "fab"), (&xnc, "fab"), (&room, "blend")] {
        assert_eq!(file_core(port, p), json!({"core": core, "on": true, "liveView": false}), "{p}");
    }

    // ── 2. A live timeline, watched; root=source as the text views send ──
    let url = |doc: &str, root: Option<&str>| {
        let r = root.map(|r| format!("&root={r}")).unwrap_or_default();
        format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}{r}", enc(doc))
    };
    let (mut a, _) = tokio_tungstenite::connect_async(&url(&cut, Some("source"))).await.expect("the docs socket opens on a timeline");
    let welcome = next_frame(&mut a, "welcome").await;
    assert_eq!(welcome["type"], "welcome", "{welcome}");
    assert_eq!(welcome["wire"], 3, "{welcome}");
    assert_eq!((&welcome["root"], &welcome["family"], &welcome["mode"], &welcome["core"]), (&json!("tracks"), &json!("timeline"), &json!("timeline"), &json!("timeline")), "{welcome}");
    assert_eq!(welcome["features"], json!(["keyed_watch", "view_address", "time_position"]), "{welcome}");
    assert_eq!(welcome["previews"], J::Null, "no lane tiles on any host yet: {welcome}");
    let (mut b, _) = tokio_tungstenite::connect_async(&url(&cut, Some("tracks"))).await.expect("its own root is taken too");
    let wb = next_frame(&mut b, "the second welcome").await;
    assert_eq!(wb["type"], "welcome", "{wb}");
    // A root it doesn't have: declined, before anything opens.
    let (mut bad, _) = tokio_tungstenite::connect_async(&url(&cut, Some("slides"))).await.expect("a socket");
    let f = next_frame(&mut bad, "closed").await;
    assert_eq!((&f["type"], &f["reason"], &f["code"]), (&json!("closed"), &json!("declined"), &json!("bad_request")), "{f}");
    // A's playhead, in its track, reaches B.
    a.send(Message::Text(json!({"type": "time_position", "req": 1, "time_position": {"at": "254016000000", "state": "paused", "in": "@trk:1"}}).to_string()))
        .await
        .expect("send time_position");
    let mut time = None;
    while time.is_none() {
        let f = next_frame(&mut b, "a's time position").await;
        if f["type"] == "roster_event" && f["event"]["kind"] == "time_position" {
            time = Some(f["event"].clone());
        }
        assert_ne!(f["type"], "presence_refused", "{f}");
    }
    let time = time.expect("time");
    assert_eq!((&time["time_position"]["at"], &time["time_position"]["in"]), (&json!("254016000000"), &json!("@trk:1")), "{time}");
    // The pack's own verbs, as an agent runs them.
    let (st, info) = tool(port, &cut, &["timeline", "info"]);
    assert_eq!((st, &info["exit"]), (200, &json!(0)), "timeline info: {info}");
    assert!(info["stdout"].as_str().is_some_and(|s| !s.is_empty()), "{info}");
    let (st, tracks) = tool(port, &cut, &["track", "list"]);
    assert_eq!((st, &tracks["exit"]), (200, &json!(0)), "track list: {tracks}");
    assert!(tracks["stdout"].as_str().is_some_and(|s| s.contains("@trk:")), "{tracks}");

    // ── 3. A timeline the pack declines: closed {declined}, the pack's code ──
    let (mut d, _) = tokio_tungstenite::connect_async(&url(&junk, Some("source"))).await.expect("a socket");
    let f = next_frame(&mut d, "the decline").await;
    assert_eq!((&f["type"], &f["reason"]), (&json!("closed"), &json!("declined")), "{f}");
    let code = f["error"]["details"]["code"].as_str().unwrap_or_else(|| panic!("the pack's code in error.details.code: {f}"));
    assert!(!code.is_empty(), "{f}");
    assert!(f["error"]["id"].as_str().is_some_and(|id| id.starts_with("MPC-E")), "the kit's report: {f}");

    // ── 4. Say something with the playhead and its track ──
    let (st, body) = http(
        port,
        "POST",
        &format!("/cli/docs/say?token={OWNER}"),
        Some(&json!({"path": cut, "text": "This cut?", "at": "@item:1", "time": {"at": "254016000000", "in": "@trk:1"}}).to_string()),
    );
    assert_eq!(st, 200, "say: {body}");
    let said = next_of(&mut b, "say").await;
    assert_eq!((&said["at"], &said["time"], &said["time_in"]), (&json!("@item:1"), &json!("254016000000"), &json!("@trk:1")), "{said}");
    let (st, body) = http(port, "POST", &format!("/cli/docs/say?token={OWNER}"), Some(&json!({"path": cut, "text": "x", "time": {"at": "1.5"}}).to_string()));
    assert_eq!(st, 400, "a time the kit won't take: {body}");
    drop(a);
    drop(b);

    // ── 5. Index verbs on the files that stay tier 0 ──
    let clips = index_verb(port, &fcpxml, &["clip", "list"]);
    assert!(clips["stdout"].as_str().is_some_and(|s| !s.is_empty()), "{clips}");
    let feats = index_verb(port, &gbr, &["feature", "list"]);
    assert!(feats["stdout"].as_str().is_some_and(|s| !s.is_empty()), "{feats}");
    let holes = index_verb(port, &xnc, &["hole", "list"]);
    assert!(holes["stdout"].as_str().is_some_and(|s| !s.is_empty()), "{holes}");

    // ── 6. A live scene: previews of its view, cameras and objects ──
    let (mut s, _) = tokio_tungstenite::connect_async(&url(&room, None)).await.expect("the docs socket opens on a scene");
    let w = next_frame(&mut s, "the scene's welcome").await;
    assert_eq!(w["type"], "welcome", "{w}");
    assert_eq!((&w["root"], &w["family"], &w["mode"], &w["core"]), (&json!("colls"), &json!("scene"), &json!("scene"), &json!("blend")), "{w}");
    assert_eq!(w["features"], json!(["keyed_watch", "view_address", "presence"]), "a read-only scene: presence, no claims, no time: {w}");
    assert_eq!(w["read_only_reason"], "K2 can't save .blend edits yet; this file opens read-only.", "{w}");
    assert_eq!(w["previews"]["kind"], "scene", "the scene renderer lists it: {w}");
    let keys: Vec<&str> = w["previews"]["containers"].as_array().unwrap_or_else(|| panic!("containers: {w}")).iter().map(|c| c["key"].as_str().expect("key")).collect();
    assert_eq!(keys.first(), Some(&"@scene:main"), "{keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("@cam:")) && keys.iter().any(|k| k.starts_with("@obj:")), "{keys:?}");
    let (st, scene) = tool(port, &room, &["scene", "list"]);
    assert_eq!((st, &scene["exit"]), (200, &json!(0)), "scene list: {scene}");
    // The listing's object addresses are the live view's (the read is
    // paired with the head's carrier).
    for obj in keys.iter().filter(|k| k.starts_with("@obj:")) {
        let (st, shown) = tool(port, &room, &["object", "show", obj]);
        assert_eq!((st, &shown["exit"]), (200, &json!(0)), "{obj} is a live object: {shown}");
    }
    // Read-only for now: no claims listed, so a claim frame is `error`
    // MPC-E113; the person's rights say no edit.
    assert_eq!(w["rights"], "read", "nobody edits a .blend for now, the owner included: {w}");
    let obj = keys.iter().find(|k| k.starts_with("@obj:")).expect("an object").to_string();
    s.send(Message::Text(json!({"type": "claim", "req": 1, "keys": [obj], "note": "moving it"}).to_string())).await.expect("send claim");
    let e = next_of(&mut s, "error").await;
    assert_eq!(e["error"]["id"], "MPC-E113", "{e}");
    // Presence: a selection by address reaches another socket as the kit's
    // `selection` roster event; a pointer goes too.
    let (mut s2, _) = tokio_tungstenite::connect_async(&url(&room, None)).await.expect("a second socket on the scene");
    let w2 = next_frame(&mut s2, "the second welcome").await;
    assert_eq!(w2["type"], "welcome", "{w2}");
    s.send(Message::Text(json!({"type": "select", "req": 2, "selection": {"keys": [obj]}}).to_string())).await.expect("send select");
    s.send(Message::Text(json!({"type": "pointer", "req": 3, "point": {"x": 0.25, "y": 0.5}}).to_string())).await.expect("send pointer");
    let (mut sel, mut ptr) = (None, None);
    while sel.is_none() || ptr.is_none() {
        let f = next_frame(&mut s2, "the selection and pointer").await;
        assert_ne!(f["type"], "presence_refused", "{f}");
        if f["type"] == "roster_event" {
            match f["event"]["kind"].as_str() {
                Some("selection") => sel = Some(f["event"].clone()),
                Some("pointer") => ptr = Some(f["event"].clone()),
                _ => {}
            }
        }
    }
    let sel = sel.expect("selection");
    assert!(sel.to_string().contains(&obj), "the selection names {obj}: {sel}");
    // Say something in the scene's room.
    let (st, body) = http(port, "POST", &format!("/cli/docs/say?token={OWNER}"), Some(&json!({"path": room, "text": "This lamp?", "at": obj}).to_string()));
    assert_eq!(st, 200, "say on a .blend: {body}");
    let said = next_of(&mut s2, "say").await;
    assert_eq!(said["at"], json!(obj), "{said}");
    drop(s2);
    drop(s);

    // ── 6b. Nothing writes a .blend: the pack's edit verbs and the write
    //        routes are refused with the typed reason; the file is untouched ──
    let before = std::fs::read(&room).expect("the scene's bytes");
    let (st, shown_before) = tool(port, &room, &["object", "show", &obj]);
    assert_eq!((st, &shown_before["exit"]), (200, &json!(0)), "object show before the refusals: {shown_before}");
    let status_before = doc_status(port, &room);
    assert_eq!(status_before["disk"]["state"], "clean", "{status_before}");
    let (st, placed) = tool(port, &room, &["object", "place", &obj, "--loc", "0,0,1"]);
    assert_eq!(st, 409, "object place: {placed}");
    assert_eq!((&placed["error"]["code"], &placed["error"]["exit"]), (&json!("blend_writer_disabled"), &json!(3)), "{placed}");
    assert_eq!(placed["error"]["hint"], "K2 can't save .blend edits yet; this file opens read-only.", "{placed}");
    for (route, body) in [
        ("/cli/docs/restore", json!({"path": room, "version": 1})),
        ("/cli/docs/undo", json!({"path": room})),
        ("/cli/docs/write", json!({"path": room, "base": "1", "bytesB64": "AAAA"})),
    ] {
        let (st, out) = http(port, "POST", &format!("{route}?token={OWNER}"), Some(&body.to_string()));
        let v: J = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{route} JSON ({e}): {out}"));
        assert_eq!((st, &v["error"]["code"]), (409, &json!("blend_writer_disabled")), "{route}: {out}");
    }
    assert_eq!(std::fs::read(&room).expect("the scene's bytes"), before, "the file is untouched");

    // ── 6c. A person's edit, undo and redo on the scene's socket (as a
    //        person's client would send them) are refused the same way,
    //        before the kit; nothing is stored ──
    let (mut p, _) = tokio_tungstenite::connect_async(&url(&room, None)).await.expect("a person's socket on the scene");
    let wp = next_frame(&mut p, "the person's welcome").await;
    assert_eq!((&wp["type"], &wp["rights"]), (&json!("welcome"), &json!("read")), "{wp}");
    let base = wp["version"].as_u64().unwrap_or_else(|| panic!("the welcome's version: {wp}"));
    for (op, frame) in [
        (7, json!({"type": "edit", "op": 7, "root": "colls", "base": base, "hunks": [{"start": 0, "end": 0, "insert": "x"}]})),
        (8, json!({"type": "undo", "op": 8})),
        (9, json!({"type": "redo", "op": 9})),
    ] {
        p.send(Message::Text(frame.to_string())).await.expect("send the person's frame");
        let r = next_of(&mut p, "refused").await;
        assert_eq!(r["op"], op, "{r}");
        assert_eq!(
            (&r["error"]["details"]["code"], &r["error"]["details"]["hint"]),
            (&json!("blend_writer_disabled"), &json!("K2 can't save .blend edits yet; this file opens read-only.")),
            "{r}"
        );
    }

    // ── 6d. After every refusal the scene reads as before: the person's
    //        watch snapshot, object show and scene list, the previews (the
    //        head read), an export of the whole file; the version is the
    //        same and nothing waits to be written, also after disk ticks ──
    p.send(Message::Text(json!({"type": "watch"}).to_string())).await.expect("send watch");
    let head = next_of(&mut p, "keyed_snapshot").await;
    assert!(head["pages"].as_u64().is_some_and(|n| n >= 1), "the watch snapshot after the refusals: {head}");
    drop(p);
    let (st, shown_after) = tool(port, &room, &["object", "show", &obj]);
    assert_eq!((st, &shown_after["exit"]), (200, &json!(0)), "object show after the refusals: {shown_after}");
    assert_eq!(shown_after["stdout"], shown_before["stdout"], "object show is as before");
    let (st, listed) = tool(port, &room, &["scene", "list"]);
    assert_eq!((st, &listed["stdout"]), (200, &scene["stdout"]), "scene list is as before: {listed}");
    let (st, body) = http(port, "GET", &format!("/cli/docs/preview?token={OWNER}&path={}", enc(&room)), None);
    assert_eq!(st, 200, "the previews after the refusals: {body}");
    let pv: J = serde_json::from_str(&body).expect("preview JSON");
    assert!(pv["containers"].as_array().is_some_and(|c| c.iter().any(|c| c["key"] == json!(obj))), "{pv}");
    let (st, body) = http(port, "POST", &format!("/cli/docs/export?token={OWNER}"), Some(&json!({"path": room, "target": "as-is", "fetch": true}).to_string()));
    assert_eq!(st, 200, "an export after the refusals: {body}");
    let mut job: J = serde_json::from_str(&body).expect("export JSON");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while job["state"] != "done" {
        assert!(matches!(job["state"].as_str(), Some("queued" | "running")), "the export: {job}");
        assert!(std::time::Instant::now() < deadline, "the export never finished: {job}");
        std::thread::sleep(Duration::from_millis(250));
        let id = job["job"].as_str().unwrap_or_else(|| panic!("a job id: {job}")).to_string();
        let (st, body) = http(port, "GET", &format!("/cli/docs/export/status?token={OWNER}&job={id}"), None);
        assert_eq!(st, 200, "export status: {body}");
        job = serde_json::from_str(&body).expect("export status JSON");
    }
    assert_eq!(job["size"], json!(before.len()), "{job}");
    let id = job["job"].as_str().unwrap_or_else(|| panic!("a job id: {job}")).to_string();
    let (st, body) = http(port, "GET", &format!("/cli/docs/export/fetch?token={OWNER}&job={id}"), None);
    assert_eq!(st, 200, "export fetch: {body}");
    let fetched: J = serde_json::from_str(&body).expect("fetch JSON");
    assert_eq!(fetched["eof"], true, "one chunk: {fetched}");
    let b64 = fetched["bytesB64"].as_str().unwrap_or_else(|| panic!("bytesB64: {fetched}"));
    let got = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).expect("base64");
    assert_eq!(got, before, "the export is the original scene");
    for when in ["after the refusals", "after the disk ticks"] {
        let st = doc_status(port, &room);
        assert_eq!((&st["version"], &st["disk"]["state"]), (&status_before["version"], &json!("clean")), "{when}: {st}");
        std::thread::sleep(Duration::from_millis(1500));
    }
    assert_eq!(std::fs::read(&room).expect("the scene's bytes"), before, "the file is still untouched");

    // ── 7. Fab off: its files are any-file docs again ──
    let off = flip(&[("any-file", true), ("timeline", true), ("fab", false), ("blend", true)]).await;
    assert!(off.changed.contains(&("fab".to_string(), false)), "{off:?}");
    assert_eq!(file_core(port, &gbr), json!({"core": "any-file", "on": true, "liveView": false}));
    let (st, v) = tool(port, &gbr, &["feature", "list"]);
    assert!(st != 200 || v["exit"] != 0, "an any-file doc has no feature verbs: {st} {v}");

    flip(&[]).await;
    drop(daemon);
}
