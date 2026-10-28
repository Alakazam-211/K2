//! Room-scope R3 (amendment RS-A1..A3, 0.46.1): folder path scope per
//! role per room line, headless. The real dispatcher in-process on a temp
//! `$HOME`, driven over loopback with guest session passes, plus the `k2`
//! CLI against that same daemon.
//!
//! - RS-A1: a login whose home room is `desk` holds `docs: jobs/J1/,
//!   jobs/J3/ (read)` and sees exactly those folders in `docs`.
//! - RS-A2: an agent-marked login (`k2 app user agent`) gets the same walls.
//!   A mark is a tie (agent-app-users, 0.46.4): the login is held to the
//!   marked workspace's room, so here it is marked as `docs`'s agent.
//! - RS-A3: an unlisted folder, existing or not, is the very same 403 as a
//!   missing cap; a symlink inside a granted folder can't reach a sibling.
//! - Writes need a write folder; copies read their sources; uploads with
//!   no folder land in the line's `uploadDir`.
//! - `fs/events` carries only the login's folders.
//! - `GET /cli/fs/stat` (`files.stat`) under the same rules.
//! - `k2 app role paths` round trip.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;
use std::time::Duration;

use app_fixture::*;
use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A guest login on the box (host App) holding `role`; returns its pass.
async fn login_with_role(port: u16, username: &str, role: &str) -> String {
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": username, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

async fn role_with_rooms(port: u16, role: &str, lines: &[(&Ws, &[&str])]) {
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    for (ws, caps) in lines {
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    }
}

async fn guest_get(port: u16, route: &str, pass: &str, ws: &Ws, rel: &str) -> Resp {
    get(port, route, pass, &format!("workspace={}&path={}", enc(&ws.handle), enc(rel))).await
}

/// The exact missing-cap refusal (status and body) for `cap`.
fn assert_missing_cap(r: &Resp, cap: &str, what: &str) {
    assert_eq!(r.status, 403, "{what}: {}", r.body);
    assert_eq!(
        r.json(),
        json!({ "error": format!("missing capability {cap}") }),
        "{what}: the same 403 as a missing cap"
    );
}

fn names(r: &Resp) -> Vec<String> {
    let mut v: Vec<String> = r
        .json()
        .as_array()
        .unwrap_or_else(|| panic!("entries: {}", r.body))
        .iter()
        .map(|e| e["path"].as_str().expect("path").to_string())
        .collect();
    v.sort();
    v
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A client room holding three jobs as folders, plus a file at the top.
fn seed_jobs(docs: &Ws) {
    write(&docs.path, "top.txt", b"company notes");
    write(&docs.path, "jobs/J1/plan.pdf", b"%PDF-1.7 plan set J1");
    write(&docs.path, "jobs/J1/photos/site.jpg", b"jpeg");
    write(&docs.path, "jobs/J2/secret.txt", b"job two only");
    write(&docs.path, "jobs/J3/notes.txt", b"job three notes");
    std::fs::create_dir_all(docs.path.join("inbox")).unwrap();
    // A link inside a granted folder pointing at a sibling job.
    k2_core::test_env::symlink(docs.path.join("jobs/J2"), docs.path.join("jobs/J1/to-j2")).unwrap();
    // And one inside a write folder (photos), relative this time.
    k2_core::test_env::symlink("../../J2", docs.path.join("jobs/J1/photos/esc")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rs_a1_a2_a3_scoped_logins_see_only_their_folders_in_any_room() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let desk = seed_ws(&env, &format!("desk{tag}"));
    let docs = seed_ws(&env, &format!("docs{tag}"));
    seed_jobs(&docs);

    // The Plans Desk role: home room `desk` (Thread), and `docs` read-only
    // on two job folders (RS-A1: a room line that isn't the home room).
    let role = format!("plans-desk{tag}");
    role_with_rooms(port, &role, &[(&desk, &["thread:read", "thread:post"]), (&docs, &["files:read"])]).await;
    let set = owner_ok(
        port,
        "/cli/skin/roles/paths",
        json!({ "name": role, "handle": docs.handle, "read": ["jobs/J1/", "jobs/J3"] }),
    )
    .await;
    let line = set["roomAccess"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == docs.handle.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("docs line in {set}"));
    assert_eq!(line["folders"], json!({ "read": ["jobs/J1", "jobs/J3"] }));

    let person = login_with_role(port, &format!("pat{tag}"), &role).await;
    // RS-A2: an agent login of the same role, marked as the docs room's
    // agent (a mark ties the login to that workspace: its ceiling is docs).
    let agent_user = format!("docsbot{tag}");
    let agent = login_with_role(port, &agent_user, &role).await;
    owner_ok(port, "/cli/skin/users/agent", json!({ "username": agent_user, "workspace": docs.handle })).await;
    let roster = get(port, "/cli/skin/users", OWNER, "").await;
    assert!(roster.body.contains(&docs.id), "the agent mark is stored: {}", roster.body);

    // The refusal a login gets for a room line WITHOUT files:read.
    let no_cap = guest_get(port, "/cli/fs/read-dir", &person, &desk, ".").await;
    assert_missing_cap(&no_cap, "files:read", "desk has no files cap");

    for (who, pass) in [("person", &person), ("agent", &agent)] {
        // Listing shows only the way in to the granted folders.
        let root = guest_get(port, "/cli/fs/read-dir", pass, &docs, ".").await;
        assert_eq!(root.status, 200, "{who}: {}", root.body);
        assert_eq!(names(&root), ["jobs"], "{who}: top.txt and inbox/ are not on the way in");
        let jobs = guest_get(port, "/cli/fs/read-dir", pass, &docs, "jobs").await;
        assert_eq!(names(&jobs), ["jobs/J1", "jobs/J3"], "{who}: never the sibling J2");
        let j1 = guest_get(port, "/cli/fs/read-dir", pass, &docs, "jobs/J1").await;
        assert_eq!(j1.status, 200, "{who}: {}", j1.body);

        // Granted reads work.
        let r = guest_get(port, "/cli/fs/read-file", pass, &docs, "jobs/J3/notes.txt").await;
        assert_eq!(r.status, 200, "{who}: {}", r.body);
        assert_eq!(r.json()["content"], "job three notes");
        let r = guest_get(port, "/cli/fs/read-binary", pass, &docs, "jobs/J1/plan.pdf").await;
        assert_eq!(r.status, 200, "{who}: {}", r.body);

        // RS-A3: unlisted = the missing-cap 403, existing or not.
        for (route, rel) in [
            ("/cli/fs/read-dir", "jobs/J2"),
            ("/cli/fs/read-file", "jobs/J2/secret.txt"),
            ("/cli/fs/read-file", "jobs/J2/no-such-file.txt"),
            ("/cli/fs/read-file", "jobs/J9/nothing-here"),
            ("/cli/fs/read-file", "top.txt"),
            ("/cli/fs/read-binary", "jobs/J2/secret.txt"),
            ("/cli/fs/stat", "jobs/J2/secret.txt"),
            ("/cli/fs/stat", "jobs/J2/missing"),
            ("/cli/fs/read-file", "jobs/J1/to-j2/secret.txt"),
            ("/cli/fs/read-dir", "jobs/J1/to-j2"),
        ] {
            let r = guest_get(port, route, pass, &docs, rel).await;
            assert_missing_cap(&r, "files:read", &format!("{who} {route} {rel}"));
            assert_eq!(r.body, no_cap.body, "{who} {route} {rel}: byte-identical to a missing cap");
            assert!(!r.body.contains("J2") && !r.body.contains(&docs.path.to_string_lossy().to_string()));
        }
        let r = get(
            port,
            "/cli/fs/read-range",
            pass,
            &format!("workspace={}&path={}&offset=0&len=4", enc(&docs.handle), enc("jobs/J2/secret.txt")),
        )
        .await;
        assert_missing_cap(&r, "files:read", &format!("{who} read-range"));

        // A malformed path is the jail's own 400, as for any login.
        let r = guest_get(port, "/cli/fs/read-file", pass, &docs, "jobs/J1/../J2/secret.txt").await;
        assert_eq!(r.status, 400, "{who}: {}", r.body);
        assert!(r.body.contains("'..'"), "{who}: {}", r.body);

        // skin/me names the folders on that line only.
        let me = get(port, "/cli/skin/me", pass, "").await.json();
        let rooms = me["rooms"].as_array().unwrap();
        let docs_room = rooms.iter().find(|r| r["room"] == docs.id.as_str()).expect("docs in me");
        assert_eq!(docs_room["folders"], json!({ "read": ["jobs/J1", "jobs/J3"] }), "{who}");
        let desk_room = rooms.iter().find(|r| r["room"] == desk.id.as_str());
        if who == "agent" {
            assert!(desk_room.is_none(), "the agent login is held to its tie's room: {me}");
        } else {
            assert!(desk_room.expect("desk in me").get("folders").is_none(), "{who}: the desk line is unscoped");
        }
    }

    // Live: widening the line reaches the live sessions at once…
    owner_ok(
        port,
        "/cli/skin/roles/paths",
        json!({ "name": role, "handle": docs.handle, "read": ["jobs/J1", "jobs/J2", "jobs/J3"] }),
    )
    .await;
    let r = guest_get(port, "/cli/fs/read-file", &agent, &docs, "jobs/J2/secret.txt").await;
    assert_eq!(r.status, 200, "widened: {}", r.body);
    // …and clearing puts the whole room back.
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": docs.handle, "clear": true })).await;
    let r = guest_get(port, "/cli/fs/read-file", &person, &docs, "top.txt").await;
    assert_eq!(r.status, 200, "whole room again: {}", r.body);

    // The owner token is never scoped (absolute paths, as before).
    let r = get(port, "/cli/fs/read-file", OWNER, &format!("path={}", enc(&docs.path.join("jobs/J2/secret.txt").to_string_lossy()))).await;
    assert_eq!(r.status, 200, "owner: {}", r.body);

    // Route guards: POST only, and a room the role lacks can't get folders.
    let r = get(port, "/cli/skin/roles/paths", OWNER, "").await;
    assert_eq!(r.status, 405, "{}", r.body);
    let other = seed_ws(&env, &format!("other{tag}"));
    let r = post(port, "/cli/skin/roles/paths", OWNER, json!({ "name": role, "handle": other.handle, "read": ["a"] })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("no line for that room"), "{}", r.body);
    let r = post(port, "/cli/skin/roles/paths", OWNER, json!({ "name": role, "handle": docs.handle, "read": ["../x"] })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = post(port, "/cli/skin/roles/paths", &person, json!({ "name": role, "handle": docs.handle, "clear": true })).await;
    assert!(r.status == 401 || r.status == 403, "a guest can't edit roles: {} {}", r.status, r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_need_a_write_folder_and_uploads_land_in_upload_dir() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("docs{tag}"));
    seed_jobs(&docs);
    let role = format!("crew{tag}");
    role_with_rooms(port, &role, &[(&docs, &["files:read", "files:write"])]).await;
    owner_ok(
        port,
        "/cli/skin/roles/paths",
        json!({ "name": role, "handle": docs.handle, "read": ["jobs/J3"], "write": ["jobs/J1/photos"], "uploadDir": "jobs/J1/photos" }),
    )
    .await;
    let pass = login_with_role(port, &format!("crew{tag}"), &role).await;
    let h = docs.handle.clone();
    let p = |body: Value| {
        let pass = pass.clone();
        async move {
            let route = body["_route"].as_str().unwrap().to_string();
            let mut b = body.clone();
            b.as_object_mut().unwrap().remove("_route");
            post(port, &route, &pass, b).await
        }
    };

    // write-file / create inside the write folder: yes.
    let r = p(json!({ "_route": "/cli/fs/write-file", "workspace": h, "path": "jobs/J1/photos/a.txt", "content": "hi" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = p(json!({ "_route": "/cli/fs/create", "workspace": h, "path": "jobs/J1/photos/sub", "is_directory": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Read-only or unlisted folders: the missing files:write 403.
    for (route, body) in [
        ("write-file", json!({ "workspace": h, "path": "jobs/J3/x.txt", "content": "no" })),
        ("write-file", json!({ "workspace": h, "path": "jobs/J1/plan2.txt", "content": "no" })),
        ("write-file", json!({ "workspace": h, "path": "jobs/J2/new/deep.txt", "content": "no" })),
        ("write-file", json!({ "workspace": h, "path": "jobs/J1/photos/esc/planted.txt", "content": "no" })),
        ("create", json!({ "workspace": h, "path": "jobs/J1/photos/esc/dir", "is_directory": true })),
        ("create", json!({ "workspace": h, "path": "jobs/J2/newdir", "is_directory": true })),
        ("rename", json!({ "workspace": h, "path": "jobs/J3/notes.txt", "new_name": "n2.txt" })),
        ("rename", json!({ "workspace": h, "path": "jobs/J1/photos/a.txt", "new_name": "../a.txt" })),
        ("move", json!({ "workspace": h, "sources": ["jobs/J1/photos/a.txt"], "destination": "jobs/J3" })),
        ("move", json!({ "workspace": h, "sources": ["jobs/J3/notes.txt"], "destination": "jobs/J1/photos" })),
        ("upload-binary", json!({ "workspace": h, "dir": "jobs/J3", "filename": "u.bin", "base64": "AA==" })),
        ("upload-chunk", json!({ "workspace": h, "dir": "jobs/J2", "filename": "c.bin", "upload_id": "u1", "offset": 0, "base64": "AA==", "is_last": true })),
    ] {
        let mut b = body.clone();
        b["_route"] = json!(format!("/cli/fs/{route}"));
        let r = p(b).await;
        if body.get("new_name").and_then(|n| n.as_str()) == Some("../a.txt") {
            assert_eq!(r.status, 400, "a bad new name stays the old 400: {}", r.body);
            continue;
        }
        assert_missing_cap(&r, "files:write", &format!("{route} {body}"));
    }
    // Naming the link itself as the upload folder: the jail's own refusal
    // (a write never goes through an existing leaf link), as for any login.
    let r = p(json!({ "_route": "/cli/fs/upload-binary", "workspace": h, "dir": "jobs/J1/photos/esc", "filename": "u.bin", "base64": "AA==" })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("escapes the workspace"), "{}", r.body);
    assert!(!docs.path.join("jobs/J2/planted.txt").exists(), "nothing planted through the link");
    assert!(!docs.path.join("jobs/J2/dir").exists() && !docs.path.join("jobs/J2/u.bin").exists());
    assert!(!docs.path.join("jobs/J2/new").exists());
    assert!(docs.path.join("jobs/J3/notes.txt").exists());

    // rename inside the write folder; copy reads a read folder into it.
    let r = p(json!({ "_route": "/cli/fs/rename", "workspace": h, "path": "jobs/J1/photos/a.txt", "new_name": "b.txt" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["path"], "jobs/J1/photos/b.txt");
    let r = p(json!({ "_route": "/cli/fs/copy", "workspace": h, "sources": ["jobs/J3/notes.txt"], "destination": "jobs/J1/photos" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(docs.path.join("jobs/J1/photos/notes.txt").exists());
    let r = p(json!({ "_route": "/cli/fs/copy", "workspace": h, "sources": ["jobs/J2/secret.txt"], "destination": "jobs/J1/photos" })).await;
    assert_missing_cap(&r, "files:read", "copy from an unlisted folder");

    // Uploads with no folder land in uploadDir.
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"photo bytes");
    let r = p(json!({ "_route": "/cli/fs/upload-binary", "workspace": h, "filename": "p1.jpg", "base64": b64 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["path"], "jobs/J1/photos/p1.jpg");
    let r = p(json!({ "_route": "/cli/fs/upload-chunk", "workspace": h, "filename": "p2.jpg", "upload_id": "up2", "offset": 0, "base64": b64, "is_last": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["path"], "jobs/J1/photos/p2.jpg");
    assert_eq!(std::fs::read(docs.path.join("jobs/J1/photos/p2.jpg")).unwrap(), b"photo bytes");

    // An unscoped line with only an uploadDir: a default, the room stays open.
    let open_role = format!("open{tag}");
    role_with_rooms(port, &open_role, &[(&docs, &["files:read", "files:write"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": open_role, "handle": docs.handle, "uploadDir": "inbox" })).await;
    let open = login_with_role(port, &format!("open{tag}"), &open_role).await;
    let r = post(port, "/cli/fs/upload-binary", &open, json!({ "workspace": h, "filename": "i.txt", "base64": b64 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["path"], "inbox/i.txt");
    let r = post(port, "/cli/fs/write-file", &open, json!({ "workspace": h, "path": "top2.txt", "content": "open" })).await;
    assert_eq!(r.status, 200, "an uploadDir alone walls nothing: {}", r.body);
    // No uploadDir and no folder: the old 400.
    let plain_role = format!("plain{tag}");
    role_with_rooms(port, &plain_role, &[(&docs, &["files:read", "files:write"])]).await;
    let plain = login_with_role(port, &format!("plain{tag}"), &plain_role).await;
    let r = post(port, "/cli/fs/upload-binary", &plain, json!({ "workspace": h, "filename": "i.txt", "base64": b64 })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("Destination directory is empty"), "{}", r.body);

    // uploadDir must sit in a write folder when folders are listed.
    let r = post(
        port,
        "/cli/skin/roles/paths",
        OWNER,
        json!({ "name": role, "handle": docs.handle, "write": ["jobs/J1/photos"], "uploadDir": "jobs/J3" }),
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("uploadDir"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_stat_answers_kind_size_and_mime_under_the_scope() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("docs{tag}"));
    seed_jobs(&docs);
    let role = format!("stat{tag}");
    role_with_rooms(port, &role, &[(&docs, &["files:read"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": docs.handle, "read": ["jobs/J1"] })).await;
    let pass = login_with_role(port, &format!("stat{tag}"), &role).await;

    let r = guest_get(port, "/cli/fs/stat", &pass, &docs, "jobs/J1/plan.pdf").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "file");
    assert_eq!(v["size"], b"%PDF-1.7 plan set J1".len() as u64);
    assert_eq!(v["mime"], "application/pdf");
    assert_eq!(v["ext"], "pdf");
    assert_eq!(v["name"], "plan.pdf");
    assert_eq!(v["path"], "jobs/J1/plan.pdf", "room-relative, never a host path");
    assert!(v["modifiedAt"].as_f64().is_some_and(|t| t > 1.0e12), "{v}");
    // A folder on the way in stats like it lists.
    let r = guest_get(port, "/cli/fs/stat", &pass, &docs, "jobs").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["kind"], "directory");
    assert_eq!(r.json()["size"], 0);
    // Out of scope: the missing-cap 403; in scope but missing: the jail's 400.
    let r = guest_get(port, "/cli/fs/stat", &pass, &docs, "jobs/J2/secret.txt").await;
    assert_missing_cap(&r, "files:read", "stat outside the scope");
    let r = guest_get(port, "/cli/fs/stat", &pass, &docs, "top.txt").await;
    assert_missing_cap(&r, "files:read", "stat of a top file");
    let r = guest_get(port, "/cli/fs/stat", &pass, &docs, "jobs/J1/nope.pdf").await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(!r.body.contains(&docs.path.to_string_lossy().to_string()), "no host path: {}", r.body);
    // A room the pass doesn't hold is the uniform skin_room; no cap is the cap 403.
    let other = seed_ws(&env, &format!("other{tag}"));
    let r = guest_get(port, "/cli/fs/stat", &pass, &other, ".").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "skin_room");
    // The owner stats an absolute path.
    let abs = docs.path.join("jobs/J2/secret.txt");
    let r = get(port, "/cli/fs/stat", OWNER, &format!("path={}", enc(&abs.to_string_lossy()))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["kind"], "file");
    assert_eq!(r.json()["mime"], "text/plain");
    // Both App doors serve it (catalog row, no door = both).
    assert!(k2_daemon::skin_gateway::allowlisted_http("GET", "/cli/fs/stat"));
    assert!(k2_daemon::apps::gateway::route_allowed("GET", "/cli/fs/stat", "", false));
    assert_eq!(k2_core::app_surface::verb_for("GET", "/cli/fs/stat"), Some("files.stat"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fs_events_carry_only_the_logins_folders() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("docs{tag}"));
    seed_jobs(&docs);
    let role = format!("ev{tag}");
    role_with_rooms(port, &role, &[(&docs, &["files:read"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": docs.handle, "read": ["jobs/J1"] })).await;
    let pass = login_with_role(port, &format!("ev{tag}"), &role).await;

    let url = format!("ws://127.0.0.1:{port}/cli/fs/events?workspace={}&token={}", enc(&docs.handle), enc(&pass));
    let (mut ws, resp) = tokio_tungstenite::connect_async(&url).await.expect("scoped files WS upgrades");
    assert_eq!(resp.status(), 101);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The owner writes in J2 (not this login's) and then in J1.
    let j2 = docs.path.join("jobs/J2/secret.txt").to_string_lossy().to_string();
    let j1 = docs.path.join("jobs/J1/new.txt").to_string_lossy().to_string();
    owner_ok(port, "/cli/fs/write-file", json!({ "path": j2, "content": "changed" })).await;
    owner_ok(port, "/cli/fs/write-file", json!({ "path": j1, "content": "fresh" })).await;
    // Every frame names only J1 paths; the J1 write arrives.
    let mut saw_j1 = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = match tokio::time::timeout(left, ws.next()).await {
            Ok(n) => n,
            Err(_) => break,
        };
        let frame = next.expect("ws open").expect("ws ok");
        if !frame.is_text() {
            continue;
        }
        let v: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        if v["kind"] != "fs_changed" {
            continue;
        }
        let paths: Vec<String> = serde_json::from_value(v["paths"].clone()).unwrap();
        assert!(!paths.is_empty(), "an emptied frame is dropped, never sent: {v}");
        assert!(paths.iter().all(|p| p.starts_with("jobs/J1")), "the J2 change never shows: {v}");
        if paths.iter().any(|p| p == "jobs/J1/new.txt") {
            saw_j1 = true;
            // Give a stray J2 frame a moment to (wrongly) arrive.
            tokio::time::sleep(Duration::from_millis(300)).await;
            break;
        }
    }
    assert!(saw_j1, "the J1 write reached the scoped socket");
    while let Ok(Some(Ok(frame))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
        if let Ok(t) = frame.to_text() {
            assert!(!t.contains("J2"), "late frame leaked J2: {t}");
        }
    }
}

fn k2_cli(home: &Path, args: &[&str]) -> std::process::Output {
    let cli = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    std::process::Command::new(k2_core::test_env::bash_program())
        .arg(&cli)
        .args(args)
        // A clean env: the outer shell may be a K2 agent cell whose
        // K2_*/K2SO_* hook vars would point the CLI at the real daemon.
        .env_clear().envs(k2_core::test_env::cli_base_env())
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .env("HOME", home)
        .current_dir(home)
        .output()
        .expect("run cli/k2")
}

fn ok_out(out: std::process::Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{what}: stdout={stdout} stderr={}", String::from_utf8_lossy(&out.stderr));
    stdout
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_role_paths_round_trip() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let home = env.path().to_path_buf();
    std::fs::create_dir_all(home.join(".k2")).unwrap();
    std::fs::write(home.join(".k2/heartbeat.port"), port.to_string()).unwrap();
    std::fs::write(home.join(".k2/heartbeat.token"), OWNER).unwrap();
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("docs{tag}"));
    seed_jobs(&docs);
    let role = format!("adam{tag}");
    let h = docs.handle.clone();

    let run = |args: Vec<String>| {
        let home = home.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                k2_cli(&home, &a)
            })
            .await
            .unwrap()
        }
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    ok_out(run(s(&["app", "role", "create", &role])).await, "role create");
    ok_out(run(s(&["app", "role", "room", &role, &h, "--caps", "files:read,files:write"])).await, "role room");
    let out = ok_out(
        run(s(&["app", "role", "paths", &role, &h, "--read", "jobs/J1/,jobs/J3", "--write", "jobs/J1/photos/", "--upload-dir", "jobs/J1/photos"])).await,
        "role paths",
    );
    assert!(out.contains("read:   jobs/J1, jobs/J3"), "{out}");
    assert!(out.contains("write:  jobs/J1/photos"), "{out}");
    assert!(out.contains("upload: jobs/J1/photos"), "{out}");
    let listed = ok_out(run(s(&["app", "role", "list"])).await, "role list");
    assert!(listed.contains("read:   jobs/J1, jobs/J3") && listed.contains("write:  jobs/J1/photos"), "{listed}");

    // The daemon holds exactly what the CLI set, and a login is walled by it.
    let roles = get(port, "/cli/skin/roles", OWNER, "").await.json();
    let r = roles["roles"].as_array().unwrap().iter().find(|r| r["name"] == role.as_str()).cloned().unwrap();
    assert_eq!(
        r["roomAccess"][0]["folders"],
        json!({ "read": ["jobs/J1", "jobs/J3"], "write": ["jobs/J1/photos"], "uploadDir": "jobs/J1/photos" })
    );
    let pass = login_with_role(port, &format!("adam{tag}"), &role).await;
    let jobs = guest_get(port, "/cli/fs/read-dir", &pass, &docs, "jobs").await;
    assert_eq!(names(&jobs), ["jobs/J1", "jobs/J3"]);

    // Usage errors fail loudly; a room the role lacks is refused.
    let out = run(s(&["app", "role", "paths", &role, &h])).await;
    assert_eq!(out.status.code(), Some(2), "no folders and no --clear");
    let out = run(s(&["app", "role", "paths", &role, &h, "--clear", "--read", "a"])).await;
    assert_eq!(out.status.code(), Some(2), "--clear with folders");
    let other = seed_ws(&env, &format!("other{tag}"));
    let out = run(s(&["app", "role", "paths", &role, &other.handle, "--read", "a"])).await;
    assert!(!out.status.success(), "a room the role lacks");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no line for that room"), "{}", String::from_utf8_lossy(&out.stderr));

    // --clear: back to the whole room, live.
    let out = ok_out(run(s(&["app", "role", "paths", &role, &h, "--clear"])).await, "paths --clear");
    assert!(out.contains("folders: whole room"), "{out}");
    let root = guest_get(port, "/cli/fs/read-dir", &pass, &docs, ".").await;
    assert!(names(&root).contains(&"top.txt".to_string()), "{}", root.body);
}
