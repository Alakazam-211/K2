//! Thread parity (K2 0.46.3, ticket 611f8b81; Rosson 2026-10-20: "app
//! gateway and garden gateway both need the attachments and any other
//! systems to match parity with the new thread features"), headless: the
//! real dispatcher in-process on a temp `$HOME`, over loopback and over an
//! App's gateway link.
//!
//! - **Images (S3, thread-images-v1):** `mode=thumb` for PNG, JPEG, GIF,
//!   WebP, BMP, TIFF and SVG (HEIC on macOS through `sips`); the SVG is the
//!   sanitized rewrite on thumb AND preview (no script, handler, outside
//!   reference); a non-image is 415 `no_thumbnail`; `px` is 256 or 512; a
//!   thumb is cached and a changed file gets a new one; guests get a thumb
//!   only for files their own rights cover.
//! - **App guests attach** files their OWN file rights cover (files:read +
//!   folder scope, room-relative); anything else is the uniform
//!   `cap_not_granted` (existing or not); the agent's line carries the
//!   paths.
//! - **Guest read cursors:** `POST /cli/thread/read` and `GET
//!   /cli/thread/unread` for App guests, on their own key, their rooms only;
//!   mute / project chat stay people-only; their own post reads itself.
//! - **Guest file links:** `POST /cli/fs/resolve-links` under the guest's
//!   rights, never a host path.
//! - **The App link** serves every new row: attachment (meta/thumb), the
//!   catch-up delta, mark-read, unread, resolve-links, a post with files.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::Cursor;
use std::path::Path;

use app_fixture::*;
use base64::Engine as _;
use image::{DynamicImage, GenericImageView, ImageFormat};
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

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

async fn guest_login(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": username, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "guest login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// A role with `caps` in `ws` (and, when given, read folders).
async fn role(port: u16, name: &str, ws: &Ws, caps: &[&str], read: Option<&[&str]>) {
    owner_ok(port, "/cli/skin/roles", json!({ "name": name })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": name, "handle": ws.handle, "caps": caps })).await;
    if let Some(r) = read {
        owner_ok(port, "/cli/skin/roles/paths", json!({ "name": name, "handle": ws.handle, "read": r })).await;
    }
}

fn code_of(r: &Resp) -> String {
    r.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", r.body)).to_string()
}

async fn att_get(port: u16, token: &str, addr: &str, id: &str, n: usize, extra: &str) -> Resp {
    get(port, "/cli/thread/attachment", token, &format!("addr={}&id={}&n={n}&{extra}", enc(addr), enc(id))).await
}

fn encode(w: u32, h: u32, f: ImageFormat) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([30, 140, 220])));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), f).unwrap();
    out
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().unwrap_or_else(|| panic!("no base64: {v}"))).unwrap()
}

const EVIL_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" onload="alert(1)"><script>alert(2)</script><rect width="10" height="10" fill="#08f" onclick="alert(3)"/><image href="https://evil.example/t.png"/><foreignObject><b>x</b></foreignObject></svg>"##;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_images_every_type_sanitized_svg_and_guest_rights() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("img{tag}"));
    let _conv = pin_chat(&ws);
    let addr = ws.handle.clone();

    let boot = get(port, "/boot-status", OWNER, "").await.json();
    let features: Vec<String> = boot["features"].as_array().expect("features").iter().map(|f| f.as_str().unwrap().to_string()).collect();
    assert!(features.contains(&"thread-images-v1".to_string()), "{features:?}");
    assert!(features.contains(&"app-thread-parity-v1".to_string()), "{features:?}");

    let types: [(&str, ImageFormat); 6] = [
        ("png", ImageFormat::Png),
        ("jpg", ImageFormat::Jpeg),
        ("gif", ImageFormat::Gif),
        ("webp", ImageFormat::WebP),
        ("bmp", ImageFormat::Bmp),
        ("tiff", ImageFormat::Tiff),
    ];
    let mut files = Vec::new();
    for (ext, f) in types {
        write(&ws.path, &format!("pics/a.{ext}"), &encode(1000, 500, f));
        files.push(json!({ "path": format!("pics/a.{ext}") }));
    }
    write(&ws.path, "pics/logo.svg", EVIL_SVG.as_bytes());
    files.push(json!({ "path": "pics/logo.svg" }));
    write(&ws.path, "docs/plan.md", b"# Plan\n");
    files.push(json!({ "path": "docs/plan.md" }));
    write(&ws.path, "secret/x.png", &encode(40, 40, ImageFormat::Png));
    files.push(json!({ "path": "secret/x.png" }));
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "pictures", "attachments": files })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = r.json()["id"].as_str().expect("id").to_string();
    let atts = r.json()["attachments"].clone();
    let mimes: Vec<&str> = atts.as_array().unwrap().iter().map(|a| a["mime"].as_str().unwrap()).collect();
    assert_eq!(
        mimes,
        ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/tiff", "image/svg+xml", "text/markdown", "image/png"]
    );

    // Every raster type: a thumbnail that fits px, as an image the client
    // decodes (JPEG, or PNG when the source may be transparent).
    for (n, (ext, _)) in types.iter().enumerate() {
        for (px, want) in [("256", (256u32, 128u32)), ("512", (512, 256))] {
            let r = att_get(port, OWNER, &addr, &id, n, &format!("mode=thumb&px={px}")).await;
            assert_eq!(r.status, 200, "{ext} px={px}: {}", r.body);
            let v = r.json();
            assert_eq!((v["kind"].clone(), v["state"].clone()), (json!("image"), json!("ok")), "{ext}: {v}");
            assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(want.0 as u64), Some(want.1 as u64)), "{ext} {px}");
            let img = image::load_from_memory(&b64(&v)).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(img.dimensions(), want, "{ext} px={px}");
            assert!(matches!(v["mime"].as_str(), Some("image/jpeg") | Some("image/png")), "{ext}: {}", v["mime"]);
            assert!(v.get("sanitized").is_none(), "{ext}");
        }
    }
    // The cache answers the same bytes; a changed file gets a new thumb.
    let a = att_get(port, OWNER, &addr, &id, 0, "mode=thumb").await.json();
    let b = att_get(port, OWNER, &addr, &id, 0, "mode=thumb").await.json();
    assert_eq!(a["base64"], b["base64"]);
    assert!(std::fs::read_dir(env.path().join(".k2/cache/thread-thumbs")).expect("the thumb cache").count() >= 12);
    write(&ws.path, "pics/a.png", &encode(300, 600, ImageFormat::Png));
    let c = att_get(port, OWNER, &addr, &id, 0, "mode=thumb").await.json();
    assert_eq!((c["width"].clone(), c["height"].clone(), c["state"].clone()), (json!(128), json!(256), json!("changed")), "{c}");

    // SVG: thumb and preview are the sanitized rewrite, never the file.
    for mode in ["mode=thumb", "mode=preview"] {
        let r = att_get(port, OWNER, &addr, &id, 6, mode).await;
        assert_eq!(r.status, 200, "{mode}: {}", r.body);
        let v = r.json();
        assert_eq!((v["mime"].clone(), v["sanitized"].clone()), (json!("image/svg+xml"), json!(true)), "{mode}: {v}");
        let svg = String::from_utf8(b64(&v)).unwrap();
        for bad in ["<script", "alert(", "onload", "onclick", "evil.example", "foreignObject"] {
            assert!(!svg.contains(bad), "{mode}: {bad} survived: {svg}");
        }
        assert!(svg.contains("<rect") && svg.contains("fill=\"#08f\""), "{mode}: the drawing stays: {svg}");
        assert_eq!((v["width"].clone(), v["height"].clone()), (json!(200), json!(100)), "{mode}");
    }
    // Not an image; bad px; a broken image file.
    let r = att_get(port, OWNER, &addr, &id, 7, "mode=thumb").await;
    assert_eq!((r.status, code_of(&r).as_str()), (415, "no_thumbnail"), "{}", r.body);
    let r = att_get(port, OWNER, &addr, &id, 0, "mode=thumb&px=4096").await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);
    write(&ws.path, "pics/a.webp", b"this is not a webp at all");
    let r = att_get(port, OWNER, &addr, &id, 3, "mode=thumb").await;
    assert_eq!((r.status, code_of(&r).as_str()), (415, "no_thumbnail"), "{}", r.body);

    // HEIC (macOS: the box converts with sips).
    #[cfg(target_os = "macos")]
    {
        let src = env.path().join(format!("heic-src-{tag}.png"));
        std::fs::write(&src, encode(800, 400, ImageFormat::Png)).unwrap();
        let st = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&src)
            .arg("--out")
            .arg(ws.path.join("pics/photo.heic"))
            .output()
            .expect("sips");
        assert!(st.status.success(), "sips made a HEIC: {}", String::from_utf8_lossy(&st.stderr));
        let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "", "attachments": [{ "path": "pics/photo.heic" }] })).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let hid = r.json()["id"].as_str().unwrap().to_string();
        assert_eq!(r.json()["attachments"][0]["mime"], "image/heic");
        let r = att_get(port, OWNER, &addr, &hid, 0, "mode=thumb").await;
        assert_eq!(r.status, 200, "{}", r.body);
        let img = image::load_from_memory(&b64(&r.json())).expect("a JPEG from the HEIC");
        assert_eq!(img.dimensions(), (256, 128));
    }

    // Guests: a thumb only for what their own file rights cover.
    let rn = format!("pics{tag}");
    role(port, &rn, &ws, &["thread:read", "files:read"], Some(&["pics/"])).await;
    let guest = guest_login(port, &format!("pg{tag}"), &rn).await;
    let r = att_get(port, &guest, &addr, &id, 1, "mode=thumb").await;
    assert_eq!(r.status, 200, "pics/ is the guest's: {}", r.body);
    let cap_body = json!({ "ok": false, "error": { "code": "cap_not_granted", "cap": "files:read", "hint": "missing capability files:read" } });
    for n in [7usize, 8] {
        let r = att_get(port, &guest, &addr, &id, n, "mode=thumb").await;
        assert_eq!((r.status, r.json()), (403, cap_body.clone()), "n={n}");
    }
    let rn2 = format!("talk{tag}");
    role(port, &rn2, &ws, &["thread:read"], None).await;
    let no_files = guest_login(port, &format!("pt{tag}"), &rn2).await;
    let r = att_get(port, &no_files, &addr, &id, 1, "mode=thumb").await;
    assert_eq!((r.status, r.json()), (403, cap_body), "no files:read, no thumb");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_guests_attach_read_cursors_and_file_links() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("par{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    let other = seed_ws(&env, &format!("oth{tag}"));
    let _other_conv = pin_chat(&other);
    write(&ws.path, "docs/plan.md", b"# Plan\n");
    write(&ws.path, "docs/photo.png", &encode(64, 64, ImageFormat::Png));
    write(&ws.path, "docs/.env", b"TOKEN=abc\n");
    write(&ws.path, "private/notes.txt", b"not for guests");

    let rn = format!("docs{tag}");
    role(port, &rn, &ws, &["thread:read", "thread:post", "files:read"], Some(&["docs/"])).await;
    let guest = guest_login(port, &format!("pat{tag}"), &rn).await;
    let rn2 = format!("talk{tag}");
    role(port, &rn2, &ws, &["thread:read", "thread:post"], None).await;
    let no_files = guest_login(port, &format!("sam{tag}"), &rn2).await;

    // Outside its folders, missing, absolute, `..`, `~`, a secret: refused
    // and nothing stored; outside-scope refusals are the same body whether
    // the file exists or not (no existence oracle).
    let before = get(port, "/cli/thread", OWNER, &format!("addr={}&limit=0", enc(&addr))).await.json()["items"].as_array().unwrap().len();
    let cap_body = json!({ "ok": false, "error": { "code": "cap_not_granted", "cap": "files:read", "hint": "missing capability files:read" } });
    let abs = ws.path.join("docs/plan.md").to_string_lossy().into_owned();
    for p in ["private/notes.txt", "private/missing.txt", abs.as_str(), "docs/../private/notes.txt", "~/x", "/etc/passwd"] {
        let r = post(port, "/cli/thread/post", &guest, json!({ "addr": addr, "text": "x", "attachments": [{ "relPath": p }] })).await;
        assert_eq!(r.status, 403, "{p}: {}", r.body);
        let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("{p}: {}", r.body));
        assert_eq!(v["error"]["code"], "cap_not_granted", "{p}: {}", r.body);
    }
    let r = post(port, "/cli/thread/post", &guest, json!({ "addr": addr, "text": "x", "attachments": [{ "relPath": "private/notes.txt" }] })).await;
    let r2 = post(port, "/cli/thread/post", &guest, json!({ "addr": addr, "text": "x", "attachments": [{ "relPath": "private/missing.txt" }] })).await;
    assert_eq!((r.status, r.body.clone()), (r2.status, r2.body.clone()), "same refusal, existing or not");
    let r = post(port, "/cli/thread/post", &guest, json!({ "addr": addr, "text": "x", "attachments": [{ "relPath": "docs/.env" }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "attachment_secret"), "{}", r.body);
    let r = post(port, "/cli/thread/post", &no_files, json!({ "addr": addr, "text": "x", "attachments": [{ "relPath": "docs/plan.md" }] })).await;
    assert_eq!((r.status, r.json()), (403, cap_body.clone()), "no files:read, no attaching");
    let after = get(port, "/cli/thread", OWNER, &format!("addr={}&limit=0", enc(&addr))).await.json()["items"].as_array().unwrap().len();
    assert_eq!(before, after, "no refused post was stored");

    // ── Guest read cursors ────────────────────────────────────────────
    let unread = |tok: String| async move {
        let r = get(port, "/cli/thread/unread", &tok, "").await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()
    };
    let u = unread(guest.clone()).await;
    let rows = u["byConversation"].as_array().expect("rows").clone();
    assert_eq!(rows.len(), 1, "only its room's Thread: {u}");
    assert_eq!((rows[0]["id"].clone(), rows[0]["addr"].clone(), rows[0]["unread"].clone()), (json!(conv), json!(addr), json!(0)), "{u}");
    assert!(u.get("person").is_none() && u.get("badgeKey").is_none(), "no person or push key for a guest: {u}");
    let mut seqs = Vec::new();
    for t in ["one", "two", "three"] {
        let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": t })).await;
        assert_eq!(r.status, 200, "{}", r.body);
        seqs.push(r.json()["seq"].as_i64().unwrap());
    }
    assert_eq!(unread(guest.clone()).await["total"], 3);
    let r = post(port, "/cli/thread/read", &guest, json!({ "addr": addr, "seq": seqs[0] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["changed"].clone(), v["unread"].clone(), v["lastReadSeq"].clone()), (json!(true), json!(2), json!(seqs[0])), "{v}");
    let r = post(port, "/cli/thread/read", &guest, json!({ "conversationId": conv, "seq": 0 })).await;
    assert_eq!((r.json()["changed"].clone(), r.json()["unread"].clone()), (json!(false), json!(2)), "forward only: {}", r.body);
    // Its own cursor: another guest and the owner are untouched.
    assert_eq!(unread(no_files.clone()).await["total"], 0, "first look baselines at the head");
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "four" })).await;
    assert_eq!(r.status, 200);
    assert_eq!(unread(no_files.clone()).await["total"], 1);
    assert_eq!(unread(guest.clone()).await["total"], 3);
    let r = post(port, "/cli/thread/read", &guest, json!({ "addr": addr, "seq": seqs[2] })).await;
    assert_eq!(r.json()["unread"], 1, "{}", r.body);
    // No seq = the newest now.
    post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "five" })).await;
    let r = post(port, "/cli/thread/read", &no_files, json!({ "addr": addr })).await;
    assert_eq!(r.json()["unread"], 0, "{}", r.body);
    // Rooms it doesn't hold; people-only routes; GET on the POST route.
    let r = post(port, "/cli/thread/read", &guest, json!({ "addr": other.handle })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);
    let r = post(port, "/cli/thread/mute", &guest, json!({ "addr": addr, "muted": true })).await;
    assert_eq!(r.status, 403, "mute is people-only: {}", r.body);
    let r = post(port, "/cli/project-group/read", &guest, json!({ "group": "x" })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = get(port, "/cli/thread/read", &guest, &format!("addr={}", enc(&addr))).await;
    assert_eq!(r.status, 405, "{}", r.body);
    let r = get(port, "/cli/thread/unread", "k2skn_not-a-live-pass", "").await;
    assert!(r.status == 401 || r.status == 403, "a dead pass: {} {}", r.status, r.body);
    // A pass with no thread:read: no Thread rows, and the read is coded.
    let rn3 = format!("files{tag}");
    role(port, &rn3, &ws, &["files:read"], None).await;
    let files_only = guest_login(port, &format!("fil{tag}"), &rn3).await;
    assert_eq!(unread(files_only.clone()).await["byConversation"], json!([]));
    let r = post(port, "/cli/thread/read", &files_only, json!({ "addr": addr })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "cap_not_granted"), "{}", r.body);

    // ── Guest file links ──────────────────────────────────────────────
    let r = post(
        port,
        "/cli/fs/resolve-links",
        &guest,
        json!({ "workspace": addr, "paths": ["docs/plan.md:3", "docs/plan.md", "docs", "private/notes.txt", "docs/.env", "~/x", abs, "nope.md"] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert!(v.get("root").is_none() && v.get("prefixes").is_none(), "never the host path: {v}");
    let links = v["links"].as_array().unwrap();
    let linkable: Vec<bool> = links.iter().map(|l| l["linkable"].as_bool().unwrap()).collect();
    assert_eq!(linkable, [false, true, true, false, false, false, true, false], "{v}");
    assert_eq!((links[1]["rel"].clone(), links[1]["kind"].clone()), (json!("docs/plan.md"), json!("file")));
    assert_eq!(links[2]["kind"], "directory");
    assert_eq!(links[6]["rel"], "docs/plan.md", "an absolute path inside the room answers rel only");
    assert!(links.iter().all(|l| l.get("path").is_none()), "{v}");
    let r = post(port, "/cli/fs/resolve-links", &no_files, json!({ "workspace": addr, "paths": ["docs/plan.md"] })).await;
    assert_eq!((r.status, r.json()), (403, cap_body), "{}", r.body);
    let r = post(port, "/cli/fs/resolve-links", &guest, json!({ "workspace": other.handle, "paths": ["a"] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);
    // The owner's answer is unchanged (root + absolute path).
    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": addr, "paths": ["docs/plan.md"] })).await;
    assert!(r.json()["root"].is_string() && r.json()["links"][0]["path"].is_string(), "{}", r.body);

    // ── Paging back and the delta, as the guest ───────────────────────
    let page = get(port, "/cli/thread", &guest, &format!("addr={}&limit=2", enc(&addr))).await.json();
    let first = page["items"][0]["seq"].as_i64().unwrap();
    let older = get(port, "/cli/thread", &guest, &format!("addr={}&limit=2&before_seq={first}", enc(&addr))).await.json();
    assert!(older["items"].as_array().unwrap().iter().all(|i| i["seq"].as_i64().unwrap() < first), "{older}");
    let delta = get(port, "/cli/thread", &guest, &format!("addr={}&after_seq={}", enc(&addr), seqs[2])).await.json();
    assert_eq!(delta["after_seq"], seqs[2]);
    assert!(delta.get("rev").is_none(), "no server-wide counter for a guest: {delta}");

    // ── A guest attaches what its own rights cover (last: a guest's post
    // wakes the room's pinned Chat, which in this harness re-pins it to a
    // new conversation, so the post is read back by its conversation id) ──
    let r = post(port, "/cli/thread/post", &guest, json!({ "addr": addr, "text": "my photo", "attachments": [{ "relPath": "docs/photo.png" }, { "path": "docs/plan.md", "line": 1 }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gid = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(r.json()["conversation_id"], conv.as_str());
    assert_eq!(r.json()["attachments"][0]["path"], "docs/photo.png");
    assert_eq!(r.json()["attachments"][1]["line"], 1);
    let page = get(port, "/cli/thread", OWNER, &format!("addr={conv}&limit=0")).await.json();
    let mine = page["items"].as_array().unwrap().iter().find(|i| i["id"] == gid.as_str()).cloned().unwrap_or_else(|| panic!("the guest's post: {page}"));
    assert_eq!(mine["doc"]["author"]["kind"], "guest");
    assert_eq!(mine["doc"]["attachments"][0]["state"], "ok", "the owner sees the guest's files");
}

/// One request on an App's link: the App pass as Bearer, JSON on POST.
fn on_link(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\nAuthorization: Bearer {pass}\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status for {method} {pq}: {text:?}"));
    Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
}

async fn link(port: u16, method: &'static str, pq: String, pass: String, body: Option<Value>) -> Resp {
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || on_link(port, method, &pq, &pass, body.as_deref())).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_link_serves_every_new_thread_row() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let ws = seed_ws(&env, &format!("lnk{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    write(&ws.path, "out/chart.png", &encode(900, 300, ImageFormat::Png));
    write(&ws.path, "out/notes.md", b"notes\n");

    // Every new row is on both doors (the generated table).
    for (m, p) in [
        ("GET", "/cli/thread/attachment"),
        ("POST", "/cli/thread/catchup"),
        ("POST", "/cli/thread/read"),
        ("GET", "/cli/thread/unread"),
        ("POST", "/cli/fs/resolve-links"),
    ] {
        assert!(gateway::route_allowed(m, p, "", false), "{m} {p} on the link");
        assert!(k2_daemon::skin_gateway::allowlisted_http(m, p), "{m} {p} on the helper");
        assert!(k2_core::app_surface::verb_for(m, p).is_some(), "{m} {p} names its verb");
    }
    for (m, p) in [("POST", "/cli/thread/mute"), ("POST", "/cli/project-group/read"), ("POST", "/cli/fs/search-tree")] {
        assert!(!gateway::route_allowed(m, p, "", false), "{m} {p} stays off the link");
    }

    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "the chart", "attachments": [{ "path": "out/chart.png" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = r.json()["id"].as_str().unwrap().to_string();
    let seq = r.json()["seq"].as_i64().unwrap();

    let svc = format!("svc-{tag}");
    let spec = LinkSpec {
        app_id: format!("par{tag}"),
        service_id: svc.clone(),
        workspace_id: ws.id.clone(),
        port: 0,
        caps: vec!["thread:read".into(), "thread:post".into(), "files:read".into()],
        rooms: vec![ws.id.clone()],
    };
    let pass = gateway::open(spec, None).expect("open link");
    let lport = gateway::port_of(&format!("par{tag}")).expect("listening");

    let r = link(lport, "GET", format!("/cli/thread/attachment?addr={}&id={id}&n=0&mode=thumb", enc(&addr)), pass.clone(), None).await;
    assert_eq!(r.status, 200, "thumb on the link: {}", r.body);
    assert_eq!(r.json()["width"], 256, "{}", r.body);
    assert!(matches!(r.json()["height"].as_u64(), Some(85 | 86)), "{}", r.body);
    let r = link(lport, "GET", format!("/cli/thread/attachment?addr={}&id={id}&n=0", enc(&addr)), pass.clone(), None).await;
    assert_eq!((r.status, r.json()["name"].clone()), (200, json!("chart.png")), "{}", r.body);

    let r = link(lport, "POST", "/cli/thread/catchup".into(), pass.clone(), Some(json!({ "threads": [{ "addr": addr, "afterSeq": 0 }] }))).await;
    assert_eq!(r.status, 200, "catch-up on the link: {}", r.body);
    let e = &r.json()["threads"][0];
    assert_eq!(e["conversation_id"], conv.as_str(), "{e}");
    assert!(e["items"].as_array().unwrap().iter().any(|i| i["id"] == id.as_str()), "{e}");
    assert!(r.json().get("rev").is_none(), "{}", r.body);

    let r = link(lport, "GET", "/cli/thread/unread".into(), pass.clone(), None).await;
    assert_eq!(r.status, 200, "unread on the link: {}", r.body);
    assert_eq!(r.json()["byConversation"][0]["id"], conv.as_str());
    let pin_now = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get(&conn, &ws.id).expect("row").and_then(|s| s.session_id)
    };
    assert_eq!(pin_now.as_deref(), Some(conv.as_str()), "the room's pinned Chat moved under the test");
    let r = link(lport, "POST", "/cli/thread/read".into(), pass.clone(), Some(json!({ "addr": addr, "seq": seq }))).await;
    assert_eq!(r.status, 200, "mark-read on the link (conv {conv}): {}", r.body);
    assert_eq!(r.json()["lastReadSeq"], seq);

    let r = link(lport, "POST", "/cli/fs/resolve-links".into(), pass.clone(), Some(json!({ "workspace": addr, "paths": ["out/notes.md"] }))).await;
    assert_eq!(r.status, 200, "links on the link: {}", r.body);
    assert_eq!(r.json()["links"][0]["rel"], "out/notes.md");

    let r = link(
        lport,
        "POST",
        "/cli/thread/post".into(),
        pass.clone(),
        Some(json!({ "addr": addr, "text": "from the App", "attachments": [{ "relPath": "out/notes.md" }] })),
    )
    .await;
    assert_eq!(r.status, 200, "a post with a file on the link: {}", r.body);
    assert_eq!(r.json()["attachments"][0]["path"], "out/notes.md");
    gateway::close(&format!("par{tag}"), None, true);
}
