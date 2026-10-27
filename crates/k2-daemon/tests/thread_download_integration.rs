//! Thread file downloads (0.46.3, Rosson 2026-10-22): "if they have threads
//! available that doesn't mean they can download files unless they have
//! file downloads turned on." Headless: the real dispatcher in-process on a
//! temp `$HOME`, over loopback.
//!
//! - The daemon says per viewer whether a file may be saved: `canDownload`
//!   on every visible attachment (the page, the post reply, the overlay
//!   WS). People: true. An App guest: only with `files:download` in the
//!   room. A hidden attachment stays exactly `{kind, state:"hidden"}`.
//! - The daemon enforces it on the route: `GET /cli/thread/attachment
//!   ?mode=download` is 403 `cap_not_granted` (`cap: files:download`) for
//!   a guest without the cap, and the file (chunks + name + mime) with it
//!   and for people.
//! - No regression: a guest without the cap still opens what it opened
//!   before (meta, preview, range), byte for byte.
//! - `files:download` is person-only to grant: an agent with "manage Apps"
//!   gets 403 `owner_only`; the owner may grant it.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;
use std::time::Duration;

use app_fixture::*;
use base64::Engine as _;
use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";
const REPORT: &[u8] = b"%PDF-1.7 the quarterly report";

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    conv
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A guest with its own role holding `caps` in `ws`. Returns its pass.
async fn guest(port: u16, ws: &Ws, name: &str, caps: &[&str]) -> String {
    let role = format!("r-{name}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": name })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": name, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": name, "role": role })).await;
    let body = json!({ "username": name, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {name}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn att_get(port: u16, token: &str, addr: &str, id: &str, n: usize, extra: &str) -> Resp {
    get(port, "/cli/thread/attachment", token, &format!("addr={}&id={}&n={n}&{extra}", enc(addr), enc(id))).await
}

/// The attachments of doc `id` as `token` sees them on `GET /cli/thread`.
async fn seen(port: u16, token: &str, addr: &str, id: &str) -> Vec<Value> {
    let r = get(port, "/cli/thread", token, &format!("addr={}", enc(addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let it = r.json()["items"].as_array().expect("items").iter().find(|i| i["id"] == id).cloned().expect("the post");
    it["doc"]["attachments"].as_array().expect("attachments").clone()
}

/// Every `mode=download` chunk, joined, plus the first chunk's answer.
async fn download_all(port: u16, token: &str, addr: &str, id: &str, n: usize, len: usize) -> (Vec<u8>, Value) {
    let mut out = Vec::new();
    let mut first: Option<Value> = None;
    let mut offset = 0usize;
    loop {
        let r = att_get(port, token, addr, id, n, &format!("mode=download&offset={offset}&len={len}")).await;
        assert_eq!(r.status, 200, "download chunk at {offset}: {}", r.body);
        let v = r.json();
        let bytes = base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().expect("base64")).expect("b64");
        out.extend_from_slice(&bytes);
        offset += v["len"].as_u64().expect("len") as usize;
        let eof = v["eof"].as_bool().expect("eof");
        if first.is_none() {
            first = Some(v);
        }
        if eof {
            break;
        }
    }
    (out, first.expect("one chunk"))
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn frame_for(sock: &mut Sock, id: &str) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), sock.next())
            .await
            .expect("a frame")
            .expect("open")
            .expect("ok");
        let Ok(t) = msg.to_text() else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        if v["id"] == id {
            return v;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_downloads_are_the_daemons_call_per_viewer() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("dl{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    write(&ws.path, "docs/report.pdf", REPORT);
    write(&ws.path, "private/notes.txt", b"not for guests");

    // ── People: true, and the file ───────────────────────────────────
    let r = post(
        port,
        "/cli/thread/post",
        OWNER,
        json!({ "addr": addr, "text": "the report", "attachments": [{ "path": "docs/report.pdf" }, { "path": "private/notes.txt" }] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = r.json()["id"].as_str().expect("id").to_string();
    for a in r.json()["attachments"].as_array().expect("reply attachments") {
        assert_eq!(a["canDownload"], json!(true), "the poster (a person) may save: {a}");
    }
    let owner_view = seen(port, OWNER, &addr, &id).await;
    assert_eq!(owner_view.len(), 2);
    for a in &owner_view {
        assert_eq!(a["canDownload"], json!(true), "people may save what they may open: {a}");
    }
    let (bytes, first) = download_all(port, OWNER, &addr, &id, 0, 8).await;
    assert_eq!(bytes, REPORT, "the owner downloads the whole file in chunks");
    assert_eq!(first["name"], "report.pdf", "{first}");
    assert_eq!(first["mime"], "application/pdf", "{first}");
    assert_eq!(first["size"], json!(REPORT.len()), "{first}");
    assert_eq!(first["offset"], 0, "{first}");

    // ── Guests ───────────────────────────────────────────────────────
    let viewer = guest(port, &ws, &format!("view{tag}"), &["thread:read", "thread:post", "files:read"]).await;
    let saver = guest(port, &ws, &format!("save{tag}"), &["thread:read", "thread:post", "files:read", "files:download"]).await;
    // files:download is a save of what you may open, never a file grant.
    let blind = guest(port, &ws, &format!("blind{tag}"), &["thread:read", "files:download"]).await;

    // The flag.
    let v = seen(port, &viewer, &addr, &id).await;
    assert_eq!(v[0]["path"], "docs/report.pdf");
    assert_eq!(v[0]["canDownload"], json!(false), "thread:read + files:read is not a download: {}", v[0]);
    let v = seen(port, &saver, &addr, &id).await;
    assert_eq!(v[0]["canDownload"], json!(true), "files:download turns it on: {}", v[0]);
    let v = seen(port, &blind, &addr, &id).await;
    for a in &v {
        assert_eq!(a, &json!({ "kind": "file", "state": "hidden" }), "no files:read: hidden, unchanged shape");
    }

    // The route refuses a guest without the cap, with a clear code.
    let dl_refused = json!({ "ok": false, "error": { "code": "cap_not_granted", "cap": "files:download", "hint": "missing capability files:download" } });
    let r = att_get(port, &viewer, &addr, &id, 0, "mode=download&offset=0&len=8").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json(), dl_refused, "{}", r.body);
    // ...the same whether or not the file is still there (no oracle).
    std::fs::rename(ws.path.join("docs/report.pdf"), ws.path.join("docs/moved.pdf")).unwrap();
    let r = att_get(port, &viewer, &addr, &id, 0, "mode=download").await;
    assert_eq!((r.status, r.json()), (403, dl_refused.clone()), "{}", r.body);
    std::fs::rename(ws.path.join("docs/moved.pdf"), ws.path.join("docs/report.pdf")).unwrap();
    // A file outside the guest's own file rights stays the files:read 403.
    let r = att_get(port, &blind, &addr, &id, 0, "mode=download").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["error"]["cap"], "files:read", "a pointer, never a grant: {}", r.body);

    // No regression: the guest without the cap opens exactly as before.
    let r = att_get(port, &viewer, &addr, &id, 0, "mode=meta").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["name"], "report.pdf");
    let r = att_get(port, &viewer, &addr, &id, 0, "mode=preview").await;
    assert_eq!(r.status, 200, "preview still opens: {}", r.body);
    let pdf = base64::engine::general_purpose::STANDARD.decode(r.json()["base64"].as_str().expect("base64")).unwrap();
    assert_eq!(pdf, REPORT);
    let r = att_get(port, &viewer, &addr, &id, 0, "mode=range&offset=0&len=8").await;
    assert_eq!(r.status, 200, "range (the phone's open) is unchanged: {}", r.body);
    assert_eq!(r.json()["len"], 8);
    assert!(r.json().get("name").is_none(), "range keeps its shape: {}", r.body);

    // With the cap: the file.
    let (bytes, first) = download_all(port, &saver, &addr, &id, 0, 5).await;
    assert_eq!(bytes, REPORT, "a guest with files:download saves the file");
    assert_eq!(first["name"], "report.pdf");

    // The guest's overlay WS frames carry its own flag.
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(&viewer));
    let (mut vsock, _) = tokio_tungstenite::connect_async(&url).await.expect("viewer overlay WS");
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(&saver));
    let (mut ssock, _) = tokio_tungstenite::connect_async(&url).await.expect("saver overlay WS");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "again", "attachments": [{ "path": "docs/report.pdf" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id2 = r.json()["id"].as_str().unwrap().to_string();
    let f = frame_for(&mut vsock, &id2).await;
    assert_eq!(f["doc"]["attachments"][0]["canDownload"], json!(false), "{f}");
    let f = frame_for(&mut ssock, &id2).await;
    assert_eq!(f["doc"]["attachments"][0]["canDownload"], json!(true), "{f}");

    // Usage: an unknown mode names download.
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=save").await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("download"), "{}", r.body);

    drop(d);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_download_is_person_only_to_grant() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("pg{tag}"));
    allow_manage(&ws, true);
    let hook = passport(&ws);
    let role = format!("agent-r{tag}");

    // Sanity: this agent may manage roles without the cap.
    let r = post(port, "/cli/skin/roles", &hook, json!({ "name": role, "roomAccess": [{ "handle": ws.handle, "caps": ["thread:read", "files:read"] }] })).await;
    assert_eq!(r.status, 200, "an agent with manage Apps makes a plain role: {}", r.body);

    let owner_only = |r: &Resp, label: &str| {
        assert_eq!(r.status, 403, "{label}: {}", r.body);
        assert_eq!(r.json()["error"]["code"], "owner_only", "{label}: {}", r.body);
        assert!(r.body.contains("files:download"), "{label}: {}", r.body);
    };
    let flagged = json!(["thread:read", "files:read", "files:download"]);
    let r = post(port, "/cli/skin/roles", &hook, json!({ "name": format!("x{tag}"), "roomAccess": [{ "handle": ws.handle, "caps": flagged }] })).await;
    owner_only(&r, "agent creates a role with files:download");
    let r = post(port, "/cli/skin/roles/room", &hook, json!({ "name": role, "handle": ws.handle, "caps": flagged })).await;
    owner_only(&r, "agent adds files:download to a room");
    let r = post(port, "/cli/skin/roles/update", &hook, json!({ "name": role, "roomAccess": [{ "handle": ws.handle, "caps": flagged }] })).await;
    owner_only(&r, "agent updates a role to carry files:download");
    let r = post(port, "/cli/skin-tokens", &hook, json!({ "name": format!("tok{tag}"), "caps": flagged, "rooms": [ws.handle] })).await;
    owner_only(&r, "agent mints a platform pass with files:download");

    // Nothing stuck; the owner may grant it.
    let roles = get(port, "/cli/skin/roles", OWNER, "").await.json();
    let mine = roles["roles"].as_array().expect("roles").iter().find(|r| r["name"] == role.as_str()).cloned().expect("role");
    assert!(!mine.to_string().contains("files:download"), "{mine}");
    let r = post(port, "/cli/skin/roles/room", OWNER, json!({ "name": role, "handle": ws.handle, "caps": flagged })).await;
    assert_eq!(r.status, 200, "the owner grants files:download: {}", r.body);
    // An agent handing that role to a guest would spread it.
    let app = {
        let dir = ws.path.join("dlapp");
        std::fs::create_dir_all(&dir).unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, "dlapp", "node s.mjs", &dir.to_string_lossy(), 47_402, "local", "stopped", "cmd", "").expect("service").id
    };
    let r = post(port, "/cli/skin/users", &hook, json!({ "username": format!("g{tag}"), "app": app })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/roles/assign", &hook, json!({ "username": format!("g{tag}"), "role": role })).await;
    owner_only(&r, "agent hands out a role holding files:download");
    // Tightening stays open to the agent.
    let r = post(port, "/cli/skin/roles/room", &hook, json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    assert_eq!(r.status, 200, "agent removes files:download: {}", r.body);

    drop(d);
}
