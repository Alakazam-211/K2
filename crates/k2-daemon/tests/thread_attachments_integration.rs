//! File objects on Thread messages (0.46.1, `thread-attachments-v1`),
//! headless: the real dispatcher in-process on a temp `$HOME`, over
//! loopback, plus the `k2` CLI against the same daemon.
//!
//! - Send-time checks: inside the workspace (realpath), a file, not a
//!   secret (path or content); outside / symlink escape / `..` / folder /
//!   missing / >10 refused, all or nothing; App guests attach only what
//!   their own file rights cover (0.46.3).
//! - The live state on read: ok → changed → gone; a `touch` stays ok on
//!   the route's re-hash.
//! - Guests: a pointer, never a grant. Files inside the guest's folder
//!   scope show; the rest are `{kind, state:"hidden"}` on GET and on the
//!   overlay WS; the route answers the uniform missing-cap 403.
//! - `GET /cli/thread/attachment`: meta / preview / range, its auth
//!   (owner, guest, passport 403, POST 405), gone 404, ids from another
//!   Thread 404, the range cap.
//! - The push preview says `[file]`, never a name; `thread/latest` counts.
//! - `k2 thread <addr> "…" --file path[:line]` and the read listing.

#![cfg(unix)]

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

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Pin a Chat on `ws` so its handle is a Thread address. Returns the
/// conversation id.
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

async fn login_with_role(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": username, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn thread_post(port: u16, token: &str, addr: &str, text: &str, files: Value) -> Resp {
    post(port, "/cli/thread/post", token, json!({ "addr": addr, "text": text, "attachments": files })).await
}

fn code_of(r: &Resp) -> String {
    r.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", r.body)).to_string()
}

/// The newest Thread item for `addr` as `token` sees it.
async fn newest(port: u16, token: &str, addr: &str) -> Value {
    let r = get(port, "/cli/thread", token, &format!("addr={}", enc(addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let items = r.json()["items"].as_array().expect("items").clone();
    items.last().cloned().expect("an item")
}

type Sock = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The first overlay frame for doc `id` on `sock` (parsed and raw).
async fn frame_for(sock: &mut Sock, id: &str) -> (Value, String) {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), sock.next())
            .await
            .expect("a frame")
            .expect("open")
            .expect("ok");
        let Ok(t) = msg.to_text() else { continue };
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        if v["id"] == id {
            return (v, t.to_string());
        }
    }
}

async fn att_get(port: u16, token: &str, addr: &str, id: &str, n: usize, extra: &str) -> Resp {
    get(port, "/cli/thread/attachment", token, &format!("addr={}&id={}&n={n}&{extra}", enc(addr), enc(id))).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_attachments_send_view_open_and_scope() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("att{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    let other = seed_ws(&env, &format!("oth{tag}"));
    let _other_conv = pin_chat(&other);

    write(&ws.path, "docs/report.pdf", b"%PDF-1.7 the report");
    write(&ws.path, "docs/plan.md", b"# Plan\n\nline 3\nline 4\n");
    write(&ws.path, "src/session.rs", "fn main() {}\n".repeat(40).as_bytes());
    write(&ws.path, "private/notes.txt", b"not for guests");
    write(&ws.path, ".env", b"PORT=3000\n");
    write(&ws.path, "certs/server.pem", b"cert");
    write(&ws.path, "config/app.yaml", b"db:\n  password: hunter2hunter2\n");
    write(&ws.path, "big.bin", &vec![7u8; 300_000]);
    let outside = env.path().join(format!("outside-{tag}"));
    write(&outside, "elsewhere.txt", b"outside the workspace");
    std::os::unix::fs::symlink(outside.join("elsewhere.txt"), ws.path.join("leak.txt")).unwrap();

    // Feature key.
    let boot = get(port, "/boot-status", OWNER, "").await;
    assert!(boot.json()["features"].as_array().expect("features").iter().any(|f| f == "thread-attachments-v1"), "{}", boot.body);

    // ── Refusals: all or nothing, nothing posted ─────────────────────
    let before = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await.json()["items"].as_array().unwrap().len();
    let abs_outside = outside.join("elsewhere.txt").to_string_lossy().into_owned();
    for (files, code) in [
        (json!([{ "path": abs_outside }]), "attachment_outside_workspace"),
        (json!([{ "path": format!("../outside-{tag}/elsewhere.txt") }]), "attachment_outside_workspace"),
        (json!([{ "path": "leak.txt" }]), "attachment_outside_workspace"),
        (json!([{ "path": "docs" }]), "attachment_not_a_file"),
        (json!([{ "path": "docs/missing.md" }]), "attachment_not_a_file"),
        (json!([{ "path": ".env" }]), "attachment_secret"),
        (json!([{ "path": "certs/server.pem" }]), "attachment_secret"),
        (json!([{ "path": "config/app.yaml" }]), "attachment_secret"),
        (json!([{ "path": "docs/report.pdf" }, { "path": ".env" }]), "attachment_secret"),
        (json!([{ "path": "docs/plan.md", "line": 0 }]), "usage"),
        (json!((0..11).map(|_| json!({ "path": "docs/plan.md" })).collect::<Vec<_>>()), "usage"),
    ] {
        let r = thread_post(port, OWNER, &addr, "see attached", files.clone()).await;
        assert_eq!(r.status, 400, "{files}: {}", r.body);
        assert_eq!(code_of(&r), code, "{files}: {}", r.body);
        assert!(!r.body.contains("hunter2"), "a content refusal never quotes the secret: {}", r.body);
    }
    let r = thread_post(port, OWNER, &addr, "x", json!((0..11).map(|_| json!({ "path": "docs/plan.md" })).collect::<Vec<_>>())).await;
    assert!(r.body.contains("at most 10 files per message"), "{}", r.body);
    let after = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await.json()["items"].as_array().unwrap().len();
    assert_eq!(before, after, "no refused post was stored");
    // No text and no files is still a usage error.
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "" })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);

    // ── A good post ───────────────────────────────────────────────────
    let abs_plan = ws.path.join("docs/plan.md").to_string_lossy().into_owned();
    let r = thread_post(
        port,
        OWNER,
        &addr,
        "The report is ready",
        json!([
            { "path": "docs/report.pdf" },
            { "path": abs_plan, "line": 3, "lineEnd": 4 },
            { "path": "src/session.rs" },
            { "path": "private/notes.txt" },
        ]),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let posted = r.json();
    let id = posted["id"].as_str().expect("id").to_string();
    let atts = posted["attachments"].as_array().expect("attachments in the reply");
    assert_eq!(atts.len(), 4);
    let sha = |rel: &str| k2_core::overlay::attachments::sha256_file(&ws.path.join(rel)).unwrap();
    assert_eq!(atts[0]["kind"], "file");
    assert_eq!(atts[0]["path"], "docs/report.pdf");
    assert_eq!(atts[0]["name"], "report.pdf");
    assert_eq!(atts[0]["size"], 19);
    assert_eq!(atts[0]["mime"], "application/pdf");
    assert_eq!(atts[0]["sha256"], json!(sha("docs/report.pdf")));
    assert_eq!(atts[0]["state"], "ok");
    assert!(atts[0]["modifiedAt"].as_i64().is_some_and(|t| t > 0), "{}", atts[0]);
    assert_eq!(atts[1]["path"], "docs/plan.md", "absolute paths are stored relative");
    assert_eq!((atts[1]["line"].clone(), atts[1]["lineEnd"].clone()), (json!(3), json!(4)));
    assert_eq!(atts[2]["mime"], "text/plain");
    assert!(!r.body.contains(&ws.path.to_string_lossy().to_string()), "the reply never carries the absolute root: {}", r.body);

    // The push preview counts the files and never names one.
    let pv = k2_daemon::thread_push::last_preview(&conv).expect("a push preview for the agent's post");
    assert_eq!(pv, "The report is ready [4 files]");
    let r = thread_post(port, OWNER, &addr, "", json!([{ "path": "docs/report.pdf" }])).await;
    assert_eq!(r.status, 200, "text is optional with a file: {}", r.body);
    let only_file_id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(k2_daemon::thread_push::last_preview(&conv).as_deref(), Some("[file]"));
    let latest = get(port, "/cli/thread/latest", OWNER, &format!("addrs={}", enc(&addr))).await.json();
    assert_eq!(latest["items"][0]["preview"], "[file]", "{latest}");
    assert_eq!(latest["items"][0]["attachmentCount"], 1, "{latest}");
    assert!(!latest.to_string().contains("report.pdf"), "{latest}");

    // ── Live state on read ────────────────────────────────────────────
    let item = newest(port, OWNER, &addr).await;
    assert_eq!(item["id"], only_file_id.as_str());
    let first = {
        let r = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await;
        r.json()["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).cloned().expect("the post")
    };
    let states = |it: &Value| -> Vec<String> {
        it["doc"]["attachments"].as_array().unwrap().iter().map(|a| a["state"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(states(&first), ["ok", "ok", "ok", "ok"]);
    // A touch (same bytes): changed by stat on GET, ok on the route's re-hash.
    let f = std::fs::File::options().write(true).open(ws.path.join("docs/report.pdf")).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(30)).unwrap();
    drop(f);
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=meta").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["state"], "ok", "same bytes re-hash to ok: {}", r.body);
    // Edited, then deleted.
    write(&ws.path, "docs/plan.md", b"# Plan v2\n");
    std::fs::remove_file(ws.path.join("src/session.rs")).unwrap();
    let r = get(port, "/cli/thread", OWNER, &format!("addr={}", enc(&addr))).await;
    let it = r.json()["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).cloned().unwrap();
    assert_eq!(states(&it)[1..3], ["changed".to_string(), "gone".to_string()]);
    assert_eq!(it["doc"]["attachments"][2]["name"], "session.rs", "gone keeps its snapshot for the chip");
    // A file swapped for a link that escapes is gone, and never opens.
    std::fs::remove_file(ws.path.join("private/notes.txt")).unwrap();
    std::os::unix::fs::symlink(outside.join("elsewhere.txt"), ws.path.join("private/notes.txt")).unwrap();
    let r = att_get(port, OWNER, &addr, &id, 3, "mode=preview").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "attachment_gone"), "{}", r.body);
    assert!(!r.body.contains("outside the workspace"), "{}", r.body);
    std::fs::remove_file(ws.path.join("private/notes.txt")).unwrap();
    write(&ws.path, "private/notes.txt", b"not for guests");

    // ── The route (owner) ─────────────────────────────────────────────
    let r = att_get(port, OWNER, &addr, &id, 1, "mode=meta").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m = r.json();
    assert_eq!((m["name"].clone(), m["state"].clone(), m["line"].clone(), m["lineEnd"].clone()), (json!("plan.md"), json!("changed"), json!(3), json!(4)));
    assert_eq!(m["size"], 10, "meta is a fresh stat");
    let r = att_get(port, OWNER, &addr, &id, 1, "mode=preview").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["kind"].clone(), r.json()["text"].clone(), r.json()["truncated"].clone()), (json!("text"), json!("# Plan v2\n"), json!(false)));
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=preview").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["kind"], "pdf");
    let bytes = base64::engine::general_purpose::STANDARD.decode(r.json()["base64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes, b"%PDF-1.7 the report");
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=range&offset=9&len=4").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["len"].clone(), v["size"].clone(), v["eof"].clone(), v["offset"].clone()), (json!(4), json!(19), json!(false), json!(9)));
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().unwrap()).unwrap(), b"the ");
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=range&offset=15").await;
    assert_eq!((r.json()["len"].clone(), r.json()["eof"].clone()), (json!(4), json!(true)), "{}", r.body);
    let r = att_get(port, OWNER, &addr, &id, 0, &format!("mode=range&len={}", 16 * 1024 * 1024 + 1)).await;
    assert_eq!(r.status, 400, "range len is capped at 16 MB: {}", r.body);
    // A binary blob has no preview; it downloads.
    let r = thread_post(port, OWNER, &addr, "blob", json!([{ "path": "big.bin" }])).await;
    let blob_id = r.json()["id"].as_str().unwrap().to_string();
    let r = att_get(port, OWNER, &addr, &blob_id, 0, "mode=preview").await;
    assert_eq!((r.status, code_of(&r).as_str()), (415, "no_preview"), "{}", r.body);
    assert_eq!(r.json()["ok"], false, "{}", r.body);
    let r = att_get(port, OWNER, &addr, &blob_id, 0, "mode=range&offset=0&len=1000000").await;
    assert_eq!((r.json()["len"].clone(), r.json()["eof"].clone()), (json!(300_000), json!(true)));
    // Gone, bad index, unknown id, an id from another Thread.
    let r = att_get(port, OWNER, &addr, &id, 2, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "attachment_gone"), "{}", r.body);
    let r = att_get(port, OWNER, &addr, &id, 9, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = att_get(port, OWNER, &other.handle, &id, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "an id from another Thread: {}", r.body);
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=nope").await;
    assert_eq!(r.status, 400, "{}", r.body);
    // conversationId works in place of addr.
    let r = get(port, "/cli/thread/attachment", OWNER, &format!("conversationId={conv}&id={id}&n=0")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // GET only; an agent passport is refused; no token, no answer.
    let r = post(port, "/cli/thread/attachment", OWNER, json!({ "addr": addr, "id": id, "n": 0 })).await;
    assert_eq!(r.status, 405, "{}", r.body);
    let agent = passport(&ws);
    let r = att_get(port, &agent, &addr, &id, 0, "mode=meta").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = att_get(port, "not-a-token", &addr, &id, 0, "mode=meta").await;
    assert_ne!(r.status, 200, "{}", r.body);
    // An agent still sees the field on its own Thread read.
    let r = get(port, "/cli/thread", &agent, &format!("addr={}", enc(&addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("\"attachments\""), "{}", r.body);

    // ── Guests: a pointer, never a grant ──────────────────────────────
    let role = format!("docs{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read", "thread:post", "files:read"] })).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "read": ["docs/"] })).await;
    let scoped = login_with_role(port, &format!("pat{tag}"), &role).await;
    let role2 = format!("talk{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role2 })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role2, "handle": ws.handle, "caps": ["thread:read", "thread:post"] })).await;
    let no_files = login_with_role(port, &format!("sam{tag}"), &role2).await;

    let guest_view = |token: String| {
        let addr = addr.clone();
        let id = id.clone();
        async move {
            let r = get(port, "/cli/thread", &token, &format!("addr={}", enc(&addr))).await;
            assert_eq!(r.status, 200, "{}", r.body);
            let it = r.json()["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).cloned().unwrap();
            (it["doc"]["attachments"].clone(), r.body)
        }
    };
    let (seen, body) = guest_view(scoped.clone()).await;
    assert_eq!(seen[0]["path"], "docs/report.pdf");
    assert_eq!(seen[1]["path"], "docs/plan.md");
    assert_eq!(seen[2], json!({ "kind": "file", "state": "hidden" }), "src/ is outside the guest's folders");
    assert_eq!(seen[3], json!({ "kind": "file", "state": "hidden" }));
    assert!(!body.contains("private/notes.txt") && !body.contains("session.rs"), "{body}");
    let (seen, body) = guest_view(no_files.clone()).await;
    for a in seen.as_array().unwrap() {
        assert_eq!(a, &json!({ "kind": "file", "state": "hidden" }), "no files:read, nothing shows");
    }
    assert!(!body.contains("report.pdf"), "{body}");

    // The route: the guest opens what it can see; the rest is the
    // uniform missing-cap 403, existing or not.
    let r = att_get(port, &scoped, &addr, &id, 0, "mode=preview").await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Coded refusals (no string matching on the phone), the same body
    // whether the file exists (private/notes.txt) or is gone (src/session.rs).
    let cap_body = json!({ "ok": false, "error": { "code": "cap_not_granted", "cap": "files:read", "hint": "missing capability files:read" } });
    for (who, tok, n) in [("scoped src/", &scoped, 2usize), ("scoped private/", &scoped, 3), ("no files", &no_files, 0)] {
        for mode in ["mode=preview", "mode=range&offset=0&len=4"] {
            let r = att_get(port, tok, &addr, &id, n, mode).await;
            assert_eq!(r.status, 403, "{who} {mode}: {}", r.body);
            assert_eq!(r.json(), cap_body, "{who} {mode}");
        }
        // meta answers what the Thread shows.
        let r = att_get(port, tok, &addr, &id, n, "mode=meta").await;
        assert_eq!(r.status, 200, "{who}: {}", r.body);
        assert_eq!(r.json(), json!({ "ok": true, "kind": "file", "state": "hidden" }), "{who}");
    }
    // A pass with no thread:read anywhere: coded too.
    let role3 = format!("files{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role3 })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role3, "handle": ws.handle, "caps": ["files:read"] })).await;
    let files_only = login_with_role(port, &format!("fil{tag}"), &role3).await;
    let r = att_get(port, &files_only, &addr, &id, 0, "mode=meta").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["error"]["code"], "cap_not_granted", "{}", r.body);
    assert_eq!(r.json()["error"]["cap"], "thread:read", "{}", r.body);
    // A guest attaches only what its own file rights cover (App parity,
    // 0.46.3; was "can't attach" in 0.46.1): private/ no, and a guest with
    // no files:read nothing (the coded missing-cap 403). The allowed case
    // is in thread_parity_integration.rs (a guest post wakes the pinned
    // Chat, which in this harness moves the conversation the WS below uses).
    let r = thread_post(port, &scoped, &addr, "here", json!([{ "path": "private/notes.txt" }])).await;
    assert_eq!((r.status, r.json()), (403, cap_body.clone()), "{}", r.body);
    let r = thread_post(port, &no_files, &addr, "here", json!([{ "path": "docs/plan.md" }])).await;
    assert_eq!((r.status, r.json()), (403, cap_body.clone()), "{}", r.body);
    // A guest's thread rights stop at its rooms.
    let r = att_get(port, &scoped, &other.handle, &id, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);

    // The overlay WS strips per guest.
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(&scoped));
    let (mut sock, _) = tokio_tungstenite::connect_async(&url).await.expect("guest overlay WS");
    let owner_url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={}", enc(OWNER));
    let (mut owner_sock, _) = tokio_tungstenite::connect_async(&owner_url).await.expect("owner overlay WS");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = thread_post(port, OWNER, &addr, "two more", json!([{ "path": "docs/report.pdf" }, { "path": "src/main.rs" }])).await;
    assert_eq!(r.status, 400, "src/main.rs doesn't exist: {}", r.body);
    write(&ws.path, "src/main.rs", b"fn main() {}\n");
    let r = thread_post(port, OWNER, &addr, "two more", json!([{ "path": "docs/report.pdf" }, { "path": "src/main.rs" }])).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let ws_id = r.json()["id"].as_str().unwrap().to_string();
    let (gv, graw) = frame_for(&mut sock, &ws_id).await;
    assert_eq!(gv["doc"]["attachments"][0]["path"], "docs/report.pdf");
    assert_eq!(gv["doc"]["attachments"][0]["state"], "ok");
    assert_eq!(gv["doc"]["attachments"][1], json!({ "kind": "file", "state": "hidden" }));
    assert!(!graw.contains("main.rs"), "{graw}");
    let (ov, _) = frame_for(&mut owner_sock, &ws_id).await;
    assert_eq!(ov["doc"]["attachments"][1]["path"], "src/main.rs", "the owner sees both");
    assert_eq!(ov["doc"]["attachments"][1]["state"], "ok");

    // ── The CLI ───────────────────────────────────────────────────────
    let home = env.path().to_path_buf();
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    std::fs::write(home.join(".k2/heartbeat.port"), port.to_string()).unwrap();
    std::fs::write(home.join(".k2/heartbeat.token"), OWNER).unwrap();
    let cli_dir = ws.path.clone();
    let run = move |args: Vec<String>| {
        let cli = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        std::process::Command::new("bash")
            .arg(&cli)
            .args(&args)
            .env_clear()
            .env("PATH", std::env::var("PATH").expect("PATH"))
            .env("HOME", &home)
            .current_dir(&cli_dir)
            .output()
            .expect("run cli/k2")
    };
    let (a, run1) = (addr.clone(), run.clone());
    let out = tokio::task::spawn_blocking(move || {
        run1(vec!["thread".into(), a, "See the plan".into(), "--file".into(), "docs/plan.md:1-2".into(), "--file".into(), "docs/report.pdf".into()])
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "cli post: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("  [file] docs/plan.md:1-2 (10 B)"), "{stdout}");
    assert!(stdout.contains("  [file] docs/report.pdf (19 B)"), "{stdout}");
    let item = newest(port, OWNER, &addr).await;
    assert_eq!(item["doc"]["body"], "See the plan");
    assert_eq!(item["doc"]["attachments"][0]["line"], 1);
    assert_eq!(item["doc"]["attachments"][0]["lineEnd"], 2);
    // A refused file fails the command loudly.
    let (a, run2) = (addr.clone(), run.clone());
    let out = tokio::task::spawn_blocking(move || run2(vec!["thread".into(), a, "secret".into(), "--file".into(), ".env".into()])).await.unwrap();
    assert!(!out.status.success(), "a refused attachment is a failed command");
    assert!(String::from_utf8_lossy(&out.stderr).contains("attachment_secret"), "{}", String::from_utf8_lossy(&out.stderr));
    // Files only, then the read listing shows each file and its state.
    let (a, run3) = (addr.clone(), run.clone());
    let out = tokio::task::spawn_blocking(move || run3(vec!["thread".into(), a, "--file".into(), "src/main.rs".into()])).await.unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_file(ws.path.join("src/main.rs")).unwrap();
    let (a, run4) = (addr.clone(), run);
    let out = tokio::task::spawn_blocking(move || run4(vec!["thread".into(), a, "--limit".into(), "3".into()])).await.unwrap();
    let listing = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{listing}");
    assert!(listing.contains("  [file] docs/plan.md:1-2 (10 B)"), "{listing}");
    assert!(listing.contains("  [file] src/main.rs (13 B) (gone)"), "{listing}");
}
