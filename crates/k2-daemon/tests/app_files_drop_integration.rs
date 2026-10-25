//! Upload-only drop folders (prd-app-caps-0464-v1 §3, AC7–AC14, 0.46.4),
//! headless: the real dispatcher in-process on a temp `$HOME`, driven over
//! loopback with guest session passes.
//!
//! - `files:upload` + a role line's `upload` folders: both upload doors
//!   land in `<drop folder>/<username>/`, answer `{path, size}`, and only
//!   ever collide with the guest's own files (Q1).
//! - Every other files door refuses an upload-only holder exactly as it
//!   refuses a missing `files:read` / `files:write` today (AC11).
//! - An unscoped line with no `uploadDir` has nowhere to drop (403 with a
//!   hint); a drop folder that links to a sibling folder is the missing-cap
//!   403 and nothing lands there.
//! - `files:write` keeps today's landing (no subfolder), `size` added.
//! - `me.get` shows `folders.upload` and `can.upload`; an agent with
//!   "manage Apps" may grant `files:upload` and set drop folders.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use base64::Engine as _;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

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

async fn role_with_rooms(port: u16, role: &str, lines: &[(&Ws, &[&str])]) {
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    for (ws, caps) in lines {
        owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    }
}

fn assert_missing_cap(r: &Resp, cap: &str, what: &str) {
    assert_eq!(r.status, 403, "{what}: {}", r.body);
    assert_eq!(r.json(), json!({ "error": format!("missing capability {cap}") }), "{what}: the plain missing-cap 403");
}

fn write(root: &std::path::Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn upload_binary(port: u16, pass: &str, ws: &Ws, dir: Option<&str>, name: &str, bytes: &[u8]) -> Resp {
    let mut b = json!({ "workspace": ws.handle, "filename": name, "base64": b64(bytes) });
    if let Some(d) = dir {
        b["dir"] = json!(d);
    }
    post(port, "/cli/fs/upload-binary", pass, b).await
}

async fn upload_chunked(port: u16, pass: &str, ws: &Ws, name: &str, id: &str, bytes: &[u8]) -> Resp {
    let (a, z) = bytes.split_at(bytes.len() / 2);
    let r = post(
        port,
        "/cli/fs/upload-chunk",
        pass,
        json!({ "workspace": ws.handle, "filename": name, "upload_id": id, "offset": 0, "base64": b64(a), "is_last": false, "total_bytes": bytes.len() }),
    )
    .await;
    assert_eq!(r.status, 200, "first chunk: {}", r.body);
    assert_eq!(r.json(), json!({ "received": a.len(), "done": false }));
    // Resume status sees the part in the guest's own subfolder.
    let s = post(port, "/cli/fs/upload-chunk", pass, json!({ "workspace": ws.handle, "filename": name, "upload_id": id, "status": true })).await;
    assert_eq!(s.status, 200, "status: {}", s.body);
    assert_eq!(s.json()["received"], a.len());
    post(
        port,
        "/cli/fs/upload-chunk",
        pass,
        json!({ "workspace": ws.handle, "filename": name, "upload_id": id, "offset": a.len(), "base64": b64(z), "is_last": true }),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drop_folders_take_uploads_into_each_guests_own_subfolder_and_nothing_else() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("drop{tag}"));
    write(&docs.path, "Inbox/someone-elses.pdf", b"not yours");
    write(&docs.path, "jobs/J2/secret.txt", b"job two only");
    write(&docs.path, "top.txt", b"company notes");

    let role = format!("dropbox{tag}");
    role_with_rooms(port, &role, &[(&docs, &["thread:read", "files:upload"])]).await;
    let set = owner_ok(
        port,
        "/cli/skin/roles/paths",
        json!({ "name": role, "handle": docs.handle, "upload": ["Inbox/"], "uploadDir": "Inbox" }),
    )
    .await;
    let line = set["roomAccess"].as_array().unwrap().iter().find(|a| a["handle"] == docs.handle.as_str()).cloned().unwrap();
    assert_eq!(line["folders"], json!({ "upload": ["Inbox"], "uploadDir": "Inbox" }));

    let dana_user = format!("dana{tag}");
    let erin_user = format!("erin{tag}");
    let dana = login_with_role(port, &dana_user, &role).await;
    let erin = login_with_role(port, &erin_user, &role).await;

    // Two guests drop a.pdf: each lands in its own subfolder, the answer is
    // exactly {path, size}, and neither learns the other's file is there.
    let r = upload_binary(port, &dana, &docs, None, "a.pdf", b"dana's pdf").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "path": format!("Inbox/{dana_user}/a.pdf"), "size": 10 }));
    let r = upload_binary(port, &erin, &docs, Some("Inbox"), "a.pdf", b"erin").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "path": format!("Inbox/{erin_user}/a.pdf"), "size": 4 }));
    // The same name again collides with the guest's own file only.
    let r = upload_binary(port, &dana, &docs, None, "a.pdf", b"again").await;
    assert_eq!(r.json()["path"], format!("Inbox/{dana_user}/a (1).pdf"));
    // A file named like someone else's top-level drop never says it exists.
    let r = upload_binary(port, &dana, &docs, None, "someone-elses.pdf", b"x").await;
    assert_eq!(r.json()["path"], format!("Inbox/{dana_user}/someone-elses.pdf"));
    assert_eq!(std::fs::read(docs.path.join("Inbox/someone-elses.pdf")).unwrap(), b"not yours");
    assert_eq!(std::fs::read(docs.path.join(format!("Inbox/{dana_user}/a.pdf"))).unwrap(), b"dana's pdf");
    assert_eq!(std::fs::read(docs.path.join(format!("Inbox/{erin_user}/a.pdf"))).unwrap(), b"erin");

    // Chunked: the part and the published file sit in the guest's subfolder.
    let r = upload_chunked(port, &erin, &docs, "big.bin", "up-1", b"0123456789abcdef").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "path": format!("Inbox/{erin_user}/big.bin"), "done": true, "size": 16 }));
    assert_eq!(std::fs::read(docs.path.join(format!("Inbox/{erin_user}/big.bin"))).unwrap(), b"0123456789abcdef");

    // Outside the drop folder: the missing files:upload 403, nothing lands.
    for dir in ["jobs/J2", ".", "Inboxes", "top.txt"] {
        let r = upload_binary(port, &dana, &docs, Some(dir), "x.bin", b"x").await;
        assert_missing_cap(&r, "files:upload", &format!("binary into {dir}"));
    }
    let r = post(
        port,
        "/cli/fs/upload-chunk",
        &dana,
        json!({ "workspace": docs.handle, "dir": "jobs/J2", "filename": "c.bin", "upload_id": "u9", "offset": 0, "base64": b64(b"x"), "is_last": true }),
    )
    .await;
    assert_missing_cap(&r, "files:upload", "chunk into jobs/J2");
    assert!(!docs.path.join("jobs/J2/x.bin").exists() && !docs.path.join("jobs/J2/c.bin").exists());

    // AC11: every other files door is the plain missing read/write 403, the
    // drop folder and the guest's own uploads included.
    let h = docs.handle.clone();
    let own = format!("Inbox/{dana_user}/a.pdf");
    for (route, rel) in [
        ("/cli/fs/read-dir", "Inbox".to_string()),
        ("/cli/fs/read-dir", format!("Inbox/{dana_user}")),
        ("/cli/fs/read-dir", ".".to_string()),
        ("/cli/fs/read-file", own.clone()),
        ("/cli/fs/read-binary", own.clone()),
        ("/cli/fs/stat", own.clone()),
        ("/cli/fs/stat", "Inbox".to_string()),
    ] {
        let r = get(port, route, &dana, &format!("workspace={}&path={}", enc(&h), enc(&rel))).await;
        assert_missing_cap(&r, "files:read", &format!("{route} {rel}"));
    }
    let r = get(port, "/cli/fs/read-range", &dana, &format!("workspace={}&path={}&offset=0&len=4", enc(&h), enc(&own))).await;
    assert_missing_cap(&r, "files:read", "read-range");
    let r = get(port, "/cli/workspace/resources", &dana, &format!("workspace={}", enc(&h))).await;
    assert_missing_cap(&r, "files:read", "workspace resources");
    let r = post(port, "/cli/fs/resolve-links", &dana, json!({ "workspace": h, "paths": [own] })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "cap_not_granted");
    for (route, body) in [
        ("write-file", json!({ "workspace": h, "path": own, "content": "overwrite" })),
        ("create", json!({ "workspace": h, "path": format!("Inbox/{dana_user}/new.txt"), "is_directory": false })),
        ("rename", json!({ "workspace": h, "path": own, "new_name": "b.pdf" })),
        ("move", json!({ "workspace": h, "sources": [format!("Inbox/{erin_user}/a.pdf")], "destination": format!("Inbox/{dana_user}") })),
        ("copy", json!({ "workspace": h, "sources": [format!("Inbox/{erin_user}/a.pdf")], "destination": format!("Inbox/{dana_user}") })),
    ] {
        let r = post(port, &format!("/cli/fs/{route}"), &dana, body.clone()).await;
        assert_missing_cap(&r, "files:write", &format!("{route} {body}"));
    }
    assert_eq!(std::fs::read(docs.path.join(&own)).unwrap(), b"dana's pdf", "nothing overwritten");
    assert!(docs.path.join(format!("Inbox/{erin_user}/a.pdf")).exists(), "nothing moved away");
    // The files socket needs files:read too.
    let url = format!("ws://127.0.0.1:{port}/cli/fs/events?workspace={}&token={}", enc(&h), enc(&dana));
    assert!(tokio_tungstenite::connect_async(&url).await.is_err(), "an upload-only guest gets no files socket");

    // me.get (AC12): folders.upload, can.upload; can.files stays none.
    let me = get(port, "/cli/skin/me", &dana, "").await;
    assert_eq!(me.status, 200, "{}", me.body);
    let me = me.json();
    let agent = me["agents"].as_array().unwrap().iter().find(|a| a["room"] == docs.id.as_str()).cloned().unwrap();
    assert_eq!(agent["folders"], json!({ "upload": ["Inbox"], "uploadDir": "Inbox" }));
    assert_eq!((agent["can"]["upload"].as_bool(), agent["can"]["files"].as_str()), (Some(true), Some("none")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nowhere_to_drop_links_out_and_write_holders_keep_todays_landing() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("land{tag}"));
    write(&docs.path, "jobs/J2/secret.txt", b"job two only");

    // An unscoped line with no uploadDir: nowhere to drop (403 + hint).
    let bare = format!("bare{tag}");
    role_with_rooms(port, &bare, &[(&docs, &["files:upload"])]).await;
    let pass = login_with_role(port, &format!("bare{tag}"), &bare).await;
    let r = upload_binary(port, &pass, &docs, Some("anywhere"), "x.bin", b"x").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["error"], "missing capability files:upload");
    assert!(r.json()["hint"].as_str().unwrap().contains("--upload <folder>"), "{}", r.body);
    assert!(!docs.path.join("anywhere").exists());

    // An unscoped line with an uploadDir drops there (and below) only.
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": bare, "handle": docs.handle, "uploadDir": "intake" })).await;
    let user = format!("bare{tag}");
    let r = upload_binary(port, &pass, &docs, None, "x.bin", b"x").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["path"], format!("intake/{user}/x.bin"));
    let r = upload_binary(port, &pass, &docs, Some("intake/batch"), "y.bin", b"y").await;
    assert_eq!(r.json()["path"], format!("intake/batch/{user}/y.bin"));
    let r = upload_binary(port, &pass, &docs, Some("jobs/J2"), "x.bin", b"x").await;
    assert_missing_cap(&r, "files:upload", "unscoped line, outside its uploadDir");
    // ...and an uploadDir alone walls nothing for the read/write doors (it
    // has no files:read anyway: the read 403 as before).
    let r = get(port, "/cli/fs/read-file", &pass, &format!("workspace={}&path=intake/{user}/x.bin", enc(&docs.handle))).await;
    assert_missing_cap(&r, "files:read", "no read-back");

    // A drop folder that links to a sibling folder: the landed check
    // refuses (missing-cap 403) and nothing lands in the sibling.
    let linked = format!("linked{tag}");
    role_with_rooms(port, &linked, &[(&docs, &["files:upload"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": linked, "handle": docs.handle, "upload": ["Drop"] })).await;
    std::os::unix::fs::symlink(docs.path.join("jobs/J2"), docs.path.join("Drop")).unwrap();
    let lpass = login_with_role(port, &format!("linked{tag}"), &linked).await;
    let r = upload_binary(port, &lpass, &docs, Some("Drop"), "x.bin", b"x").await;
    assert_missing_cap(&r, "files:upload", "drop folder links to a sibling");
    let r = post(
        port,
        "/cli/fs/upload-chunk",
        &lpass,
        json!({ "workspace": docs.handle, "dir": "Drop", "filename": "c.bin", "upload_id": "u1", "offset": 0, "base64": b64(b"x"), "is_last": true }),
    )
    .await;
    assert_missing_cap(&r, "files:upload", "chunk: drop folder links to a sibling");
    let leaked: Vec<_> = std::fs::read_dir(docs.path.join("jobs/J2")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leaked, vec![std::ffi::OsString::from("secret.txt")], "nothing landed in the sibling");

    // .k2 is never a drop folder.
    let r = post(port, "/cli/skin/roles/paths", OWNER, json!({ "name": linked, "handle": docs.handle, "upload": [".K2/inbox"] })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains(".k2"), "{}", r.body);

    // files:write (+ files:upload): today's landing, no subfolder; size added.
    let crew = format!("crew{tag}");
    role_with_rooms(port, &crew, &[(&docs, &["files:read", "files:write", "files:upload"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": crew, "handle": docs.handle, "write": ["work"], "upload": ["Drop2"], "uploadDir": "work" })).await;
    let cpass = login_with_role(port, &format!("crew{tag}"), &crew).await;
    let r = upload_binary(port, &cpass, &docs, None, "w.txt", b"work").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json(), json!({ "path": "work/w.txt", "size": 4 }));
    let r = upload_binary(port, &cpass, &docs, Some("Drop2"), "w.txt", b"x").await;
    assert_missing_cap(&r, "files:write", "a write holder writes only in write folders, as today");
    // A platform pass with files:upload has no username to drop under.
    let (_, platform) =
        k2_core::skin::create_token(&format!("bff{tag}"), Some(&["files:upload".to_string()]), &[docs.id.clone()]).expect("platform token");
    let r = upload_binary(port, &platform, &docs, Some("intake"), "p.bin", b"p").await;
    // (Refused at the dispatcher, as before: a platform pass needs
    // files:write at the pass level for the upload doors.)
    assert_missing_cap(&r, "files:write", "a platform pass can't drop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_with_manage_apps_may_grant_upload_and_set_drop_folders() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("agt{tag}"));
    allow_manage(&docs, true);
    let agent = passport(&docs);
    let role = format!("intake{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    // files:upload is strictly narrower than files:write: not person-only.
    let r = post(port, "/cli/skin/roles/room", &agent, json!({ "name": role, "handle": docs.handle, "caps": ["thread:read", "files:upload"] })).await;
    assert_eq!(r.status, 200, "an agent grants files:upload: {}", r.body);
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "upload": ["Inbox"] })).await;
    assert_eq!(r.status, 200, "an agent sets drop folders: {}", r.body);
    // files:download stays person-only (unchanged).
    let r = post(port, "/cli/skin/roles/room", &agent, json!({ "name": role, "handle": docs.handle, "caps": ["files:download"] })).await;
    assert_eq!(r.status, 403, "{}", r.body);
}
