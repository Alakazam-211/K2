//! Files on ticket messages (0.46.6, `ticket-attachments-v1`;
//! prd-ticket-attachments-v1 + vs-live TA1–TA29), headless: the real
//! dispatcher in-process on a temp `$HOME`, over loopback.
//!
//! - Files on create, comment, answer and settle: stored on the right row;
//!   `show` has each comment's `id` and its files with live `state`
//!   (`changed` after an edit, `ok` after a same-bytes touch, `gone` after
//!   a delete or a workspace removal); every write echoes `attachments`.
//! - Refusals refuse the whole write, nothing stored: outside the
//!   workspace (a symlink escape too), a folder, `.env`, a credential in
//!   text, 11 files, a wrong-typed `attachments`, files on a plain
//!   `resolve`, files on an unlinked ticket.
//! - A file-only person comment moves the ticket to `needs_discussion`; an
//!   option pick with files is the answer with the files.
//! - The agent gets `Attached: <abs path>` lines; the stored body doesn't.
//! - App guests: scoped `files:read` sees its folder and `hidden` for the
//!   rest; no `files:download` → download 403; attaching outside its scope
//!   or by absolute path → the uniform 403; another room's ticket →
//!   `skin_room`; never a host path anywhere in `show`.
//! - The door `GET /cli/feedback/attachment`: meta / preview / thumb /
//!   range / download; comment ownership; an owner notice is `not_found`
//!   to a Member; an agent passport is 403; POST is 405.
//! - Old rows (NULL) read as none; corrupt JSON drops the files, `show`
//!   still answers.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;
use std::time::Duration;

use app_fixture::*;
use base64::Engine as _;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 90]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
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

fn code_of(r: &Resp) -> String {
    r.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", r.body)).to_string()
}

async fn show(port: u16, token: &str, id: &str) -> Value {
    let r = get(port, "/cli/feedback/show", token, &format!("id={}", enc(id))).await;
    assert_eq!(r.status, 200, "show: {}", r.body);
    r.json()
}

async fn door(port: u16, token: &str, id: &str, comment: &str, n: usize, extra: &str) -> Resp {
    get(port, "/cli/feedback/attachment", token, &format!("id={}&commentId={}&n={n}&{extra}", enc(id), enc(comment))).await
}

fn comment_count(id: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM feedback_comments WHERE feedback_id = ?1", rusqlite::params![id], |r| r.get(0))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_files_store_show_open_and_refuse() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tkt{tag}"));
    let other = seed_ws(&env, &format!("tko{tag}"));

    write(&ws.path, "out/a.png", &png(64, 32));
    write(&ws.path, "out/b.png", &png(40, 40));
    write(&ws.path, "docs/plan.md", b"# Plan\nline 2\n");
    write(&ws.path, "private/notes.txt", b"not for guests");
    write(&ws.path, ".env", b"PORT=3000\n");
    write(&ws.path, "config/app.yaml", b"db:\n  password: hunter2hunter2\n");
    write(&ws.path, "pics/fake.heic", b"text pretending to be a photo\n");
    let outside = env.path().join(format!("outside-{tag}"));
    write(&outside, "elsewhere.txt", b"outside the workspace");
    std::os::unix::fs::symlink(outside.join("elsewhere.txt"), ws.path.join("leak.txt")).unwrap();
    write(&other.path, "theirs.md", b"another workspace's file");

    // Feature key (TK10).
    let boot = get(port, "/boot-status", OWNER, "").await;
    assert!(
        boot.json()["features"].as_array().expect("features").iter().any(|f| f == "ticket-attachments-v1"),
        "{}",
        boot.body
    );

    // ── create with files (the agent's ask) ───────────────────────────
    let r = post(
        port,
        "/cli/feedback/create",
        OWNER,
        json!({
            "project": ws.id,
            "title": "Which logo?",
            "options": ["A", "B"],
            "agentName": "scout",
            "assignees": ["owner"],
            "briefHtml": "<p>Pick one</p>",
            "attachments": [{ "path": "out/a.png" }, { "relPath": "out/b.png" }],
        }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let created = r.json();
    let id = created["id"].as_str().expect("id").to_string();
    let echo = created["attachments"].as_array().unwrap_or_else(|| panic!("create echoes attachments: {created}"));
    assert_eq!(echo.len(), 2);
    assert_eq!((echo[0]["path"].clone(), echo[0]["state"].clone()), (json!("out/a.png"), json!("ok")));
    assert_eq!(echo[0]["canDownload"], true, "a person may save: {}", echo[0]);
    assert_eq!(created["fileCount"], 2, "{created}");
    let shown = show(port, OWNER, &id).await;
    let seed = &shown["comments"][0];
    let seed_id = seed["id"].as_str().unwrap_or_else(|| panic!("comment ids on show (TA1): {shown}")).to_string();
    assert_eq!(seed["attachments"][0]["name"], "a.png");
    assert_eq!(seed["attachments"][1]["mime"], "image/png");
    assert_eq!(shown["fileCount"], 2);
    // A create with no files echoes an empty list (TA3).
    let r = post(
        port,
        "/cli/feedback/create",
        OWNER,
        json!({ "project": ws.id, "title": "plain", "kind": "fyi", "agentName": "scout", "assignees": ["owner"] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["attachments"], json!([]), "{}", r.body);

    // ── refusals: the whole write, nothing stored ─────────────────────
    let before = comment_count(&id);
    let abs_outside = outside.join("elsewhere.txt").to_string_lossy().into_owned();
    for (files, code) in [
        (json!([{ "path": abs_outside }]), "attachment_outside_workspace"),
        (json!([{ "path": "leak.txt" }]), "attachment_outside_workspace"),
        (json!([{ "path": other.path.join("theirs.md").to_string_lossy() }]), "attachment_outside_workspace"),
        (json!([{ "path": "docs" }]), "attachment_not_a_file"),
        (json!([{ "path": ".env" }]), "attachment_secret"),
        (json!([{ "path": "config/app.yaml" }]), "attachment_secret"),
        (json!([{ "path": "docs/plan.md" }, { "path": ".env" }]), "attachment_secret"),
        (json!((0..11).map(|_| json!({ "path": "docs/plan.md" })).collect::<Vec<_>>()), "usage"),
        (json!({ "path": "docs/plan.md" }), "usage"),
        (json!("[{\"path\":\"docs/plan.md\"}]"), "usage"),
        (json!(3), "usage"),
    ] {
        let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "see", "author": "scout", "attachments": files })).await;
        assert_eq!(r.status, 400, "{files}: {}", r.body);
        assert_eq!(code_of(&r), code, "{files}: {}", r.body);
        assert!(!r.body.contains("hunter2"), "a content refusal never quotes the secret: {}", r.body);
        if !files.is_array() {
            assert!(r.body.contains("attachments must be a JSON array"), "the hint names the field (TA18): {}", r.body);
        }
    }
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "x", "author": "scout", "attachments": [{ "path": ".env" }] })).await;
    assert!(r.body.contains("can't ride a ticket") && r.body.contains("k2 vault"), "{}", r.body);
    // Files on a plain resolve: a usage error (TK7); nothing changes.
    let r = post(port, "/cli/feedback/resolve", OWNER, json!({ "id": id, "attachments": [{ "path": "docs/plan.md" }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);
    assert!(r.body.contains("files ride a message"), "{}", r.body);
    assert_eq!(comment_count(&id), before, "no refused write was stored");
    assert_eq!(show(port, OWNER, &id).await["status"], "waiting", "the refused resolve changed nothing");
    // Files on a create are checked before the ticket is stored.
    let r = post(
        port,
        "/cli/feedback/create",
        OWNER,
        json!({ "project": ws.id, "title": "leaky", "kind": "fyi", "agentName": "scout", "assignees": ["owner"], "attachments": [{ "path": ".env" }] }),
    )
    .await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "attachment_secret"), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM feedback WHERE project_id = ?1 AND title = 'leaky'", rusqlite::params![ws.id], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "a refused create stores no ticket");
    }

    // ── an agent comment with files (no inject) ───────────────────────
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "", "author": "scout", "attachments": [{ "path": "docs/plan.md", "line": 2 }] })).await;
    assert_eq!(r.status, 200, "an agent's file-only comment: {}", r.body);
    let agent_comment = r.json()["commentId"].as_str().unwrap().to_string();
    assert_eq!(r.json()["attachments"][0]["line"], 2, "{}", r.body);
    assert_eq!(show(port, OWNER, &id).await["status"], "waiting", "an agent comment changes no status");

    // ── a person's file-only comment: needs_discussion + inject ────────
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "", "attachments": [{ "path": "out/b.png" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["status"].clone(), v["answered"].clone()), (json!("needs_discussion"), json!(false)), "{v}");
    let person_comment = v["commentId"].as_str().unwrap().to_string();
    let inject = k2_daemon::ticket_attachments::last_inject(&id).expect("the person's message was delivered");
    assert!(
        inject.contains(&format!("Attached: {}", ws.path.join("out/b.png").display())),
        "the agent gets the absolute path: {inject}"
    );
    let shown = show(port, OWNER, &id).await;
    let row = shown["comments"].as_array().unwrap().iter().find(|c| c["id"] == person_comment.as_str()).cloned().unwrap();
    assert_eq!(row["body"], "", "the stored body is the person's text, never the Attached: lines");
    assert!(!shown.to_string().contains("Attached:"), "{shown}");
    // An option pick needs the option's text even with files.
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "", "optionPick": true, "attachments": [{ "path": "out/b.png" }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);

    // ── an option pick with files is the answer ───────────────────────
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "B", "optionPick": true, "attachments": [{ "path": "out/b.png" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["status"].clone(), v["answered"].clone()), (json!("answered"), json!(true)), "{v}");
    assert_eq!(v["attachments"][0]["path"], "out/b.png");
    let inject = k2_daemon::ticket_attachments::last_inject(&id).unwrap();
    assert!(inject.contains("] B\nAttached: "), "{inject}");

    // ── /answer and the settled outcome carry files ───────────────────
    let r = post(port, "/cli/feedback/answer", OWNER, json!({ "id": id, "answer": "A after all", "attachments": [{ "path": "out/a.png" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["attachments"][0]["path"], "out/a.png", "{}", r.body);
    let r = post(
        port,
        "/cli/feedback/resolve",
        OWNER,
        json!({ "id": id, "status": "answered", "answer": "Shipped A", "author": "scout", "attachments": [{ "path": "docs/plan.md" }] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let settled = r.json();
    assert_eq!((settled["status"].clone(), settled["attachments"][0]["path"].clone()), (json!("answered"), json!("docs/plan.md")), "{settled}");
    let shown = show(port, OWNER, &id).await;
    let last = shown["comments"].as_array().unwrap().last().cloned().unwrap();
    assert_eq!((last["body"].clone(), last["author"].clone()), (json!("Shipped A"), json!("scout")));
    assert_eq!(last["attachments"][0]["name"], "plan.md");
    assert_eq!(shown["fileCount"], 7, "2 on the ask, 1 agent, 1 person, 1 pick, 1 answer, 1 settle: {shown}");

    // ── the door (owner) ──────────────────────────────────────────────
    let r = door(port, OWNER, &id, &seed_id, 0, "mode=meta").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["name"].clone(), r.json()["state"].clone()), (json!("a.png"), json!("ok")));
    let r = door(port, OWNER, &id[..8], &seed_id, 0, "mode=thumb&px=256").await;
    assert_eq!(r.status, 200, "a short id works: {}", r.body);
    assert_eq!((r.json()["width"].clone(), r.json()["kind"].clone()), (json!(64), json!("image")), "{}", r.body);
    let r = door(port, OWNER, &id, &agent_comment, 0, "mode=preview").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["text"], "# Plan\nline 2\n");
    let r = door(port, OWNER, &id, &seed_id, 1, "mode=download&offset=0&len=8").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["name"].clone(), v["mime"].clone(), v["len"].clone()), (json!("b.png"), json!("image/png"), json!(8)));
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().unwrap()).unwrap(), b"\x89PNG\r\n\x1a\n");
    // Comment ownership: another ticket's comment id is not this ticket's.
    let r = post(port, "/cli/feedback/create", OWNER, json!({ "project": other.id, "title": "other", "kind": "fyi", "agentName": "o", "assignees": ["owner"] })).await;
    let other_id = r.json()["id"].as_str().unwrap().to_string();
    let r = door(port, OWNER, &other_id, &seed_id, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = door(port, OWNER, &id, &seed_id, 9, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    let r = door(port, OWNER, &id, &seed_id, 0, "mode=nope").await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);
    let r = get(port, "/cli/feedback/attachment", OWNER, &format!("id={id}&n=0")).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "commentId is required: {}", r.body);
    // GET only; an agent passport is refused; no token, no answer.
    let r = post(port, "/cli/feedback/attachment", OWNER, json!({ "id": id, "commentId": seed_id, "n": 0 })).await;
    assert_eq!(r.status, 405, "{}", r.body);
    let agent = passport(&ws);
    let r = door(port, &agent, &id, &seed_id, 0, "mode=meta").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = door(port, "not-a-token", &id, &seed_id, 0, "mode=meta").await;
    assert_ne!(r.status, 200, "{}", r.body);

    // ── live states ───────────────────────────────────────────────────
    let f = std::fs::File::options().write(true).open(ws.path.join("out/a.png")).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(30)).unwrap();
    drop(f);
    let shown = show(port, OWNER, &id).await;
    assert_eq!(shown["comments"][0]["attachments"][0]["state"], "changed", "show is a stat, as the Thread list: {shown}");
    let r = door(port, OWNER, &id, &seed_id, 0, "mode=meta").await;
    assert_eq!(r.json()["state"], "ok", "the door re-hashes: a same-bytes touch stays ok: {}", r.body);
    write(&ws.path, "out/a.png", &png(10, 10));
    std::fs::remove_file(ws.path.join("out/b.png")).unwrap();
    let shown = show(port, OWNER, &id).await;
    assert_eq!(shown["comments"][0]["attachments"][0]["state"], "changed");
    assert_eq!(shown["comments"][0]["attachments"][1]["state"], "gone");
    assert_eq!(shown["comments"][0]["attachments"][1]["name"], "b.png", "gone keeps its snapshot");
    let r = door(port, OWNER, &id, &seed_id, 1, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "attachment_gone"), "{}", r.body);

    // ── HEIC by name only never reaches a converter ───────────────────
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": id, "body": "photo", "author": "scout", "attachments": [{ "path": "pics/fake.heic" }] })).await;
    let heic_comment = r.json()["commentId"].as_str().unwrap().to_string();
    let r = door(port, OWNER, &id, &heic_comment, 0, "mode=thumb").await;
    assert_eq!((r.status, code_of(&r).as_str()), (415, "no_thumbnail"), "{}", r.body);

    // ── old and corrupt rows ──────────────────────────────────────────
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE feedback_comments SET attachments_json = '[{\"kind\":3' WHERE id = ?1",
            rusqlite::params![agent_comment],
        )
        .unwrap();
    }
    let shown = show(port, OWNER, &id).await;
    let row = shown["comments"].as_array().unwrap().iter().find(|c| c["id"] == agent_comment.as_str()).cloned().unwrap();
    assert!(row.get("attachments").is_none(), "corrupt JSON drops the files: {row}");
    let r = get(port, "/cli/feedback/list", OWNER, &format!("project={}&all=1", enc(&ws.path.to_string_lossy()))).await;
    assert_eq!(r.status, 200, "the list still reads: {}", r.body);

    // ── a removed workspace: gone, and attaching is refused ───────────
    let gone_ws = seed_ws(&env, &format!("tkg{tag}"));
    write(&gone_ws.path, "a.md", b"hello");
    let r = post(
        port,
        "/cli/feedback/create",
        OWNER,
        json!({ "project": gone_ws.id, "title": "soon gone", "kind": "fyi", "agentName": "g", "assignees": ["owner"], "attachments": [{ "path": "a.md" }] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let gone_id = r.json()["id"].as_str().unwrap().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![gone_ws.id]).unwrap();
    }
    let shown = show(port, OWNER, &gone_id).await;
    assert_eq!(shown["comments"][0]["attachments"][0]["state"], "gone", "{shown}");
    let r = post(port, "/cli/feedback/comment", OWNER, json!({ "id": gone_id, "body": "x", "author": "g", "attachments": [{ "path": "a.md" }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    assert!(r.body.contains("workspace is not registered"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticket_files_for_app_guests_and_members() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("tga{tag}"));
    let other = seed_ws(&env, &format!("tgo{tag}"));
    write(&ws.path, "docs/report.md", b"# Report\n");
    write(&ws.path, "docs/pic.png", &png(30, 20));
    write(&ws.path, "src/main.rs", b"fn main() {}\n");

    // The agent's ask carries a path in its text too (TA17).
    let root_s = ws.path.to_string_lossy().to_string();
    let r = post(
        port,
        "/cli/feedback/create",
        OWNER,
        json!({
            "project": ws.id,
            "title": "Review",
            "body": format!("see {root_s}/docs/report.md"),
            "options": ["ok"],
            "agentName": "scout",
            "assignees": ["owner"],
            "briefHtml": "<p>Look</p>",
            "attachments": [{ "path": "docs/report.md" }, { "path": "src/main.rs" }, { "path": "docs/pic.png" }],
        }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let id = r.json()["id"].as_str().unwrap().to_string();
    let seed_id = show(port, OWNER, &id).await["comments"][0]["id"].as_str().unwrap().to_string();
    let r = post(port, "/cli/feedback/create", OWNER, json!({ "project": other.id, "title": "theirs", "kind": "fyi", "agentName": "o", "assignees": ["owner"] })).await;
    let other_id = r.json()["id"].as_str().unwrap().to_string();

    // Guests: scoped to docs/ with download; tickets only (no files).
    let role = format!("tdocs{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(
        port,
        "/cli/skin/roles/room",
        json!({ "name": role, "handle": ws.handle, "caps": ["tickets:read", "tickets:post", "files:read", "files:download"] }),
    )
    .await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": ws.handle, "read": ["docs/"] })).await;
    let scoped = login_with_role(port, &format!("tpat{tag}"), &role).await;
    let role2 = format!("tonly{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role2 })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role2, "handle": ws.handle, "caps": ["tickets:read", "tickets:post", "files:read"] })).await;
    let no_download = login_with_role(port, &format!("tsam{tag}"), &role2).await;
    let role3 = format!("tbare{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role3 })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role3, "handle": ws.handle, "caps": ["tickets:read", "tickets:post"] })).await;
    let no_files = login_with_role(port, &format!("tbob{tag}"), &role3).await;

    // show: files the scope covers, hidden for the rest, never a host path.
    let r = get(port, "/cli/feedback/show", &scoped, &format!("id={id}")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let atts = v["comments"][0]["attachments"].as_array().unwrap_or_else(|| panic!("{v}")).clone();
    assert_eq!((atts[0]["path"].clone(), atts[0]["canDownload"].clone()), (json!("docs/report.md"), json!(true)));
    assert_eq!(atts[1], json!({ "kind": "file", "state": "hidden" }), "src/ is outside the guest's folder");
    assert_eq!(atts[2]["name"], "pic.png");
    assert!(v["comments"][0]["id"].is_string(), "{v}");
    assert!(!r.body.contains(&root_s), "no host path anywhere in a guest show: {}", r.body);
    assert!(!r.body.contains("main.rs") && v.get("projectPath").is_none(), "{}", r.body);
    let r = get(port, "/cli/feedback/show", &no_download, &format!("id={id}")).await;
    assert_eq!(r.json()["comments"][0]["attachments"][0]["canDownload"], false, "{}", r.body);
    let r = get(port, "/cli/feedback/show", &no_files, &format!("id={id}")).await;
    for a in r.json()["comments"][0]["attachments"].as_array().unwrap() {
        assert_eq!(a, &json!({ "kind": "file", "state": "hidden" }), "no files:read, nothing shows");
    }

    // The door: what the guest may open; the uniform 403 for the rest.
    let cap_body = |cap: &str| json!({ "ok": false, "error": { "code": "cap_not_granted", "cap": cap, "hint": format!("missing capability {cap}") } });
    let r = door(port, &scoped, &id, &seed_id, 0, "mode=preview").await;
    assert_eq!((r.status, r.json()["text"].clone()), (200, json!("# Report\n")), "{}", r.body);
    let r = door(port, &scoped, &id, &seed_id, 2, "mode=thumb").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = door(port, &scoped, &id, &seed_id, 0, "mode=download").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = door(port, &scoped, &id, &seed_id, 1, "mode=preview").await;
    assert_eq!((r.status, r.json()), (403, cap_body("files:read")), "{}", r.body);
    let r = door(port, &scoped, &id, &seed_id, 1, "mode=meta").await;
    assert_eq!((r.status, r.json()), (200, json!({ "ok": true, "kind": "file", "state": "hidden" })), "{}", r.body);
    let r = door(port, &no_download, &id, &seed_id, 0, "mode=download").await;
    assert_eq!((r.status, r.json()), (403, cap_body("files:download")), "{}", r.body);
    let r = door(port, &no_files, &id, &seed_id, 0, "mode=preview").await;
    assert_eq!((r.status, r.json()), (403, cap_body("files:read")), "{}", r.body);
    // Another room's ticket: skin_room, no oracle.
    let r = door(port, &scoped, &other_id, &seed_id, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);
    let r = door(port, &scoped, "not-a-ticket", &seed_id, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (403, "skin_room"), "{}", r.body);

    // Guests attach only what their own scope covers, room-relative.
    let r = post(port, "/cli/feedback/comment", &scoped, json!({ "id": id, "body": "mine", "attachments": [{ "relPath": "docs/pic.png" }] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let guest_comment = r.json()["commentId"].as_str().unwrap().to_string();
    assert_eq!(r.json()["attachments"][0]["path"], "docs/pic.png", "{}", r.body);
    let inject = k2_daemon::ticket_attachments::last_inject(&id).expect("a guest's message reaches the agent");
    assert!(inject.contains(&format!("Attached: {}", ws.path.join("docs/pic.png").display())), "{inject}");
    let r = get(port, "/cli/feedback/show", &scoped, &format!("id={id}")).await;
    assert!(!r.body.contains("Attached:") && !r.body.contains(&root_s), "{}", r.body);
    assert!(r.body.contains(&guest_comment), "{}", r.body);
    let abs = ws.path.join("docs/pic.png").to_string_lossy().into_owned();
    for files in [json!([{ "path": "src/main.rs" }]), json!([{ "path": abs }]), json!([{ "path": "../x" }]), json!([{ "path": "docs/missing.md" }])] {
        let r = post(port, "/cli/feedback/comment", &scoped, json!({ "id": id, "body": "x", "attachments": files })).await;
        if files[0]["path"] == "docs/missing.md" {
            assert_eq!((r.status, code_of(&r).as_str()), (400, "attachment_not_a_file"), "{files}: {}", r.body);
        } else {
            assert_eq!((r.status, r.json()), (403, cap_body("files:read")), "{files}: {}", r.body);
        }
    }
    let r = post(port, "/cli/feedback/comment", &no_files, json!({ "id": id, "body": "x", "attachments": [{ "path": "docs/report.md" }] })).await;
    assert_eq!((r.status, r.json()), (403, cap_body("files:read")), "{}", r.body);
    // Apps can't settle (unchanged), with or without files.
    let r = post(port, "/cli/feedback/resolve", &scoped, json!({ "id": id, "status": "answered", "answer": "x", "attachments": [{ "path": "docs/report.md" }] })).await;
    assert_eq!((r.status, code_of(&r).as_str()), (400, "usage"), "{}", r.body);

    // Owner notices: a Member gets not_found on the door; the owner passes.
    let sentinel = k2_core::db::AUDIT_SENTINEL_IDS[0];
    let notice = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: sentinel.to_string(),
        agent_name: "K2".into(),
        kind: "fyi".into(),
        title: "Disk".into(),
        ..Default::default()
    })
    .expect("notice");
    let notice_comment = k2_core::feedback::get_with_comments(&notice.id).unwrap().1[0].id.clone();
    let member = format!("tmem{tag}");
    k2_core::connect_users::add_user(&member, "password123").expect("add_user");
    let member_tok = k2_core::connect_users::create_session(&member);
    let r = door(port, &member_tok, &notice.id, &notice_comment, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "{}", r.body);
    assert!(!r.body.contains("attachment"), "the same answer as a wrong id: {}", r.body);
    let r = door(port, OWNER, &notice.id, &notice_comment, 0, "mode=meta").await;
    assert_eq!((r.status, code_of(&r).as_str()), (404, "not_found"), "the owner reaches the message (no files on it): {}", r.body);
    assert!(r.body.contains("no such attachment"), "{}", r.body);
    // A Member reads an ordinary ticket's files.
    let r = door(port, &member_tok, &id, &seed_id, 0, "mode=meta").await;
    assert_eq!(r.status, 200, "{}", r.body);
}
