//! Live `.pptx` previews (V3), headless: the real dispatcher
//! (`test_harness`), the real docs runtime (the vendored kit and the pptx
//! pack, a temp `~/.k2`), and the real `k2-deckrender` worker, driven over
//! HTTP and the docs socket with no client.
//!
//! What it pins:
//!   1. `GET /cli/docs/preview` lists a live deck's slides with keyed render
//!      hashes; an image is `202 rendering`, then `ready` (WebP at 320 px,
//!      the contract's metadata) with `want=thumb`; no token is 403.
//!   2. The docs socket on a deck: `welcome` says wire 3, `mode: deck`, the
//!      previews and `turn_idle_ms`; a text frame is refused.
//!   3. Only dirty slides: a title edit on slide 2 changes slide 2's hash
//!      alone; slide 2's thumb is re-drawn on its own and `preview_ready`
//!      reaches the socket for it, never for the untouched slides, whose
//!      thumbs stay `ready` at the same hash. The old hash is still served
//!      (`hash=`). The change frame carries the verb (wire 3).
//!   4. A rename moves the previews (no render needed at the new path).
//!   5. Slides off: the deck opened live is `stored_as_live_deck` (409, the
//!      history kept), not `pack_mismatch`; previews say `no_preview`. On
//!      again, it is the same live doc.
//!   6. A removed workspace takes its previews with it (the sweep).
//!
//! The test builds the worker binary itself (`cargo build -p k2-deckrender`
//! into its own target dir) and fails loudly when that build fails.
//!
//! ISOLATION: one test (the docs runtime is process-wide), temp HOME.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use k2_daemon::test_harness;
use k2_mpc_pptx::testgen::{deck, DeckSpec, Slide};
use rusqlite::params;
use serde_json::{json, Value as J};
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "owner-token-docs-preview";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
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

fn preview(port: u16, path: &str, extra: &str) -> Resp {
    http(port, "GET", &format!("/cli/docs/preview?token={OWNER}&path={}{extra}", enc(path)), None)
}

fn listing(port: u16, path: &str) -> J {
    let r = preview(port, path, "");
    assert_eq!(r.status, 200, "listing {path}: {}", r.body);
    json_of(&r)
}

/// `(key, render_hash, thumb state)` per container.
fn hashes(l: &J) -> Vec<(String, String, String)> {
    l["containers"]
        .as_array()
        .unwrap_or_else(|| panic!("containers: {l}"))
        .iter()
        .map(|c| {
            (
                c["key"].as_str().expect("key").to_string(),
                c["render_hash"].as_str().expect("hash").to_string(),
                c["previews"]["thumb"].as_str().expect("thumb state").to_string(),
            )
        })
        .collect()
}

/// Ask for one image until it is `ready` (up to 90 s: a cold worker process
/// per render).
fn ready_image(port: u16, path: &str, container: &str, width: &str) -> J {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let r = preview(port, path, &format!("&container={}&width={width}", enc(container)));
        let v = json_of(&r);
        match (r.status, v["state"].as_str()) {
            (200, Some("ready")) => return v,
            (202, Some("rendering" | "stale")) => {}
            other => panic!("{container} {width}: unexpected {other:?}: {}", r.body),
        }
        assert!(Instant::now() < deadline, "{container} at {width} never got ready: {}", r.body);
        std::thread::sleep(Duration::from_millis(200));
    }
}

async fn flip(cores: &[(&str, bool)]) -> k2_daemon::docs::cores::Report {
    let after: std::collections::BTreeMap<String, bool> = cores.iter().map(|(c, on)| (c.to_string(), *on)).collect();
    tokio::task::spawn_blocking(move || k2_daemon::docs::cores::sync_multiplayer_cores(&after, "owner-token"))
        .await
        .expect("flip")
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next text frame (Pings skipped), within 60 s.
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

/// The worker binary, built for this test (`cargo build -p k2-deckrender`).
fn deckrender_bin() -> std::path::PathBuf {
    k2_core::test_env::built_workspace_bin("k2-deckrender", k2_deckrender::proto::BINARY_NAME)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_deck_previews_only_its_dirty_slides_headless() {
    // Before HOME moves: a build here needs cargo's real homes.
    let worker = deckrender_bin();
    let tmp = std::env::temp_dir().join(format!("k2-docs-preview-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("home");
    let tmp = tmp.canonicalize().expect("canonical home");
    std::env::set_var("HOME", &tmp);
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    std::env::set_var(k2_daemon::docs::preview::deck::ENV_BIN, worker);

    let daemon = test_harness::start(OWNER).await;
    let port = daemon.port;
    tokio::task::spawn_blocking(k2_daemon::docs::start).await.expect("docs start");
    assert!(k2_daemon::docs::live_ready(), "the docs runtime is up: {}", k2_daemon::docs::pool_json());

    let ws_dir = tmp.join("ws-deck");
    std::fs::create_dir_all(ws_dir.join(".k2")).expect("ws");
    let ws_dir = ws_dir.canonicalize().expect("canonical ws");
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'deckws', ?2, 'deckws')",
            params![project_id, ws_dir.to_string_lossy()],
        )
        .expect("seed project");
    }
    let bytes = deck(&DeckSpec::new(vec![
        Slide::titled(256, "Quarterly review", ""),
        Slide::titled(257, "Goals", "Grow the pilot"),
        Slide::titled(258, "Next steps", "Hire"),
    ]));
    let talk_abs = ws_dir.join("talk.pptx");
    std::fs::write(&talk_abs, &bytes).expect("deck");
    let talk = talk_abs.to_string_lossy().to_string();

    let on = flip(&[("any-file", true), ("slides", true)]).await;
    assert!(on.changed.contains(&("slides".to_string(), true)), "{on:?}");

    // ── 1. The listing, rights, and one image ──
    let r = http(port, "GET", &format!("/cli/docs/preview?path={}", enc(&talk)), None);
    assert_eq!(r.status, 403, "no token: {}", r.body);
    let l0 = listing(port, &talk);
    assert_eq!(l0["kind"], "deck", "{l0}");
    assert_eq!(l0["page"]["unit"], "emu", "{l0}");
    let h0 = hashes(&l0);
    let keys: Vec<&str> = h0.iter().map(|h| h.0.as_str()).collect();
    assert_eq!(keys, ["@sld:256", "@sld:257", "@sld:258"]);
    for (k, h, state) in &h0 {
        assert_eq!(h.len(), 32, "{k}: a keyed hash, 32 hex: {h}");
        assert!(h.bytes().all(|b| b.is_ascii_hexdigit()), "{h}");
        assert_eq!(state, "none", "{k}: nothing drawn yet");
    }
    assert_eq!(l0["containers"][1]["title"], "Goals");
    // An image never asked for: 202, rendering.
    let r = preview(port, &talk, "&container=%40sld%3A256&width=thumb");
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(json_of(&r)["state"], "rendering", "{}", r.body);
    let r = preview(port, &talk, "&container=%40sld%3A256&width=huge");
    assert_eq!(r.status, 400, "a width that isn't a bucket: {}", r.body);
    // `want=thumb` queues the rest.
    let r = preview(port, &talk, "&want=thumb");
    assert_eq!(r.status, 200, "{}", r.body);
    let mut thumbs = std::collections::BTreeMap::new();
    for (k, h, _) in &h0 {
        let img = ready_image(port, &talk, k, "thumb");
        assert_eq!(img["render_hash"], json!(h), "{k}");
        assert_eq!(img["width"], "thumb");
        assert_eq!(img["size"]["w"], 320, "{k}");
        assert_eq!(img["format"], "webp-lossless");
        assert_eq!(img["mime"], "image/webp");
        assert_eq!(img["page"]["unit"], "emu");
        assert!(img["rev"].as_u64().is_some() && img["rendered_at_ms"].as_u64().is_some() && img["fonts_substituted"].is_array(), "{img}");
        let raw = base64::engine::general_purpose::STANDARD.decode(img["base64"].as_str().expect("base64")).expect("base64 decodes");
        assert!(raw.len() > 12 && &raw[..4] == b"RIFF" && &raw[8..12] == b"WEBP", "a WebP image for {k}");
        thumbs.insert(k.clone(), raw);
    }
    let l1 = listing(port, &talk);
    assert!(hashes(&l1).iter().all(|(_, _, s)| s == "ready"), "{l1}");

    // ── 2. The docs socket on a live deck ──
    let url = format!("ws://127.0.0.1:{port}/cli/docs/live?token={OWNER}&doc={}", enc(&talk));
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("the docs socket opens on a deck");
    let welcome = next_frame(&mut ws, "welcome").await;
    assert_eq!(welcome["type"], "welcome", "{welcome}");
    assert_eq!(welcome["wire"], 3, "{welcome}");
    assert_eq!(welcome["mode"], "deck", "{welcome}");
    assert_eq!(welcome["root"], "slides", "{welcome}");
    assert_eq!(welcome["family"], "deck", "{welcome}");
    assert_eq!(welcome["turn_idle_ms"], 120_000, "{welcome}");
    assert!(welcome.get("text").is_none(), "a deck has no text: {welcome}");
    assert_eq!(welcome["comments"], json!({"threads": [], "next": null}), "{welcome}");
    let wkeys: Vec<(&str, &str)> = welcome["previews"]["containers"]
        .as_array()
        .expect("previews")
        .iter()
        .map(|c| (c["key"].as_str().expect("key"), c["render_hash"].as_str().expect("hash")))
        .collect();
    let want: Vec<(&str, &str)> = h0.iter().map(|(k, h, _)| (k.as_str(), h.as_str())).collect();
    assert_eq!(wkeys, want, "the welcome's previews are the listing's");
    ws.send(Message::Text(json!({"type": "edit", "op": 9, "root": "source", "base": 1, "hunks": []}).to_string())).await.expect("send");
    let mut refused = None;
    while refused.is_none() {
        let f = next_frame(&mut ws, "the edit's refusal").await;
        if f["type"] == "refused" {
            refused = Some(f);
        }
    }
    let refused = refused.expect("refused");
    assert_eq!(refused["op"], 9, "{refused}");
    assert_eq!(refused["error"]["id"], "MPC-E113", "{refused}");

    // ── 3. Only dirty slides re-render ──
    let r = http(
        port,
        "POST",
        &format!("/cli/docs/tool?token={OWNER}&path={}", enc(&talk)),
        Some(&json!({"argv": ["shape", "text", "@sld:257/sp:2", "--text", "Goals, revised"]}).to_string()),
    );
    assert_eq!(r.status, 200, "shape text: {}", r.body);
    assert_eq!(json_of(&r)["exit"], 0, "{}", r.body);
    let mut change = None;
    let mut ready = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    while ready.is_empty() {
        assert!(Instant::now() < deadline, "no preview_ready after the edit");
        let f = next_frame(&mut ws, "the change and preview_ready").await;
        match f["type"].as_str() {
            Some("change") => change = Some(f),
            Some("preview_ready") => ready.push(f),
            _ => {}
        }
    }
    let change = change.expect("the change came before its preview");
    assert_eq!(change["event"]["verb"], "shape text", "wire 3: the verb rides the change: {change}");
    assert_eq!(change["event"]["turn"], J::Null, "a person's change has no turn: {change}");
    let l2 = listing(port, &talk);
    let h2 = hashes(&l2);
    assert_eq!(h2[0].1, h0[0].1, "slide 1's hash stays");
    assert_ne!(h2[1].1, h0[1].1, "slide 2's hash moved");
    assert_eq!(h2[2].1, h0[2].1, "slide 3's hash stays");
    assert_eq!(ready[0]["container"], "@sld:257", "{:?}", ready);
    assert_eq!(ready[0]["render_hash"], json!(h2[1].1));
    assert_eq!((ready[0]["width"].as_str(), ready[0]["state"].as_str()), (Some("thumb"), Some("ready")), "{:?}", ready);
    assert_eq!(h2[0].2, "ready", "slide 1's thumb is still ready, not re-drawn");
    assert_eq!(h2[2].2, "ready");
    let new2 = ready_image(port, &talk, "@sld:257", "thumb");
    assert_ne!(base64::engine::general_purpose::STANDARD.decode(new2["base64"].as_str().expect("b64")).expect("b64"), thumbs["@sld:257"], "a new image");
    // No other preview_ready arrives for the untouched slides.
    let quiet = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let f = next_frame(&mut ws, "more frames").await;
            if f["type"] == "preview_ready" {
                return f;
            }
        }
    })
    .await;
    assert!(quiet.is_err(), "only the dirty slide was drawn: {quiet:?}");
    // The old hash is still served: the hash names the bytes drawn.
    let old = preview(port, &talk, &format!("&container=%40sld%3A257&width=thumb&hash={}", h0[1].1));
    assert_eq!((old.status, json_of(&old)["state"].clone()), (200, json!("ready")), "{}", old.body);
    // The stage's width: 1,920 px.
    let full = ready_image(port, &talk, "@sld:256", "full");
    assert_eq!(full["size"]["w"], 1920);
    drop(ws);

    // ── 4. A rename moves the previews ──
    let r = http(
        port,
        "POST",
        &format!("/cli/docs/mv?token={OWNER}&path={}", enc(&talk)),
        Some(&json!({"to": "renamed.pptx"}).to_string()),
    );
    assert_eq!(r.status, 200, "mv: {}", r.body);
    let renamed = ws_dir.join("renamed.pptx").to_string_lossy().to_string();
    let r = preview(port, &renamed, "&container=%40sld%3A256&width=thumb");
    assert_eq!((r.status, json_of(&r)["state"].clone()), (200, json!("ready")), "the previews followed the rename: {}", r.body);

    // ── 5. Slides off: the live deck isn't silently turned into an any-file doc ──
    let off = flip(&[("any-file", true), ("slides", false)]).await;
    assert!(off.changed.contains(&("slides".to_string(), false)), "{off:?}");
    let r = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(&renamed)), None);
    assert_eq!(r.status, 409, "{}", r.body);
    let v = json_of(&r);
    assert_eq!(v["error"]["code"], "stored_as_live_deck", "not pack_mismatch: {v}");
    assert!(v["error"]["hint"].as_str().expect("hint").contains("k2 doc cores on slides"), "{v}");
    let r = preview(port, &renamed, "");
    assert_eq!((r.status, json_of(&r)["error"]["code"].clone()), (409, json!("no_preview")), "{}", r.body);
    flip(&[("any-file", true), ("slides", true)]).await;
    let r = http(port, "GET", &format!("/cli/docs/status?token={OWNER}&path={}", enc(&renamed)), None);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json_of(&r)["tier"], 1, "the same live doc: {}", r.body);

    // ── 6. A removed workspace takes its previews ──
    let space_dir = tmp.join(".k2/cache/doc-previews").join(&project_id);
    assert!(space_dir.is_dir(), "previews cached under the space: {}", space_dir.display());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id]).expect("remove the workspace");
    }
    tokio::task::spawn_blocking(k2_daemon::docs::sweep_orphans_now).await.expect("sweep");
    assert!(!space_dir.exists(), "the removed workspace's previews are gone");
}
