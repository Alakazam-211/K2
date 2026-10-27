//! Downloading exposed files (prd-app-caps-0464-v1 §4, AC15–AC23,
//! 0.46.4), headless: the real dispatcher in-process on a temp `$HOME`.
//!
//! - `files:download` + a role line's `download` entries (no `files:read`):
//!   the entry downloads, its sibling and every read door are the missing-
//!   cap 403.
//! - A folder entry lists through `files.downloads`, stops at 500 and says
//!   `truncated`; a symlink out of an entry is refused (and never listed).
//! - A `files:download` holder without entries and without `files:read`
//!   gets nothing new; with `files:read` it saves what it could read.
//! - Adding or widening entries is person-only (an agent: 403
//!   `owner_only`), narrowing stays open; an omitted `download` keeps the
//!   stored list; a download-only body leaves the read folders alone.
//! - Each saved chunk leaves an audit line; `.k2` is never an entry.

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

async fn download(port: u16, pass: &str, ws: &Ws, rel: &str, extra: &str) -> Resp {
    get(port, "/cli/fs/download", pass, &format!("workspace={}&path={}{extra}", enc(&ws.handle), enc(rel))).await
}

fn seed(docs: &Ws) {
    write(&docs.path, "reports/2026-10.pdf", b"%PDF october");
    write(&docs.path, "reports/2026-11.pdf", b"%PDF november");
    write(&docs.path, "public/a.txt", b"public a");
    write(&docs.path, "public/sub/b.txt", b"public b");
    write(&docs.path, "public/.env", b"SECRET=1");
    write(&docs.path, "private/secret.txt", b"private");
    write(&docs.path, ".k2/AGENT.md", b"agent config");
    k2_core::test_env::symlink(docs.path.join("private"), docs.path.join("public/to-private")).unwrap();
    // Review A3: never listed or served, by name or by content.
    write(&docs.path, "public/.git/config", b"[core]\n");
    write(&docs.path, "public/.git/HEAD", b"ref: refs/heads/main\n");
    write(&docs.path, "public/.aws/credentials", b"[default]\n");
    write(&docs.path, "public/infra/terraform.tfstate", b"{}");
    write(&docs.path, "public/AuthKey_X.p8", b"key");
    write(&docs.path, "public/notes-with-key.txt", b"token: ghp_0123456789abcdefghijklmnopqrstuvwxyzAB\n");
    // Round 2 A3: a hard link into the entry under a harmless name.
    std::fs::hard_link(docs.path.join("private/secret.txt"), docs.path.join("public/harmless.txt")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_entries_expose_exactly_their_files_and_nothing_else() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("dl{tag}"));
    seed(&docs);

    let role = format!("saver{tag}");
    role_with_rooms(port, &role, &[(&docs, &["thread:read", "files:download"])]).await;
    let set = owner_ok(
        port,
        "/cli/skin/roles/paths",
        json!({ "name": role, "handle": docs.handle, "download": ["reports/2026-10.pdf", "public/"] }),
    )
    .await;
    let line = set["roomAccess"].as_array().unwrap().iter().find(|a| a["handle"] == docs.handle.as_str()).cloned().unwrap();
    assert_eq!(line["folders"], json!({ "download": ["reports/2026-10.pdf", "public"] }));
    let user = format!("sam{tag}");
    let pass = login_with_role(port, &user, &role).await;

    // The file entry downloads, save-shaped.
    let r = download(port, &pass, &docs, "reports/2026-10.pdf", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["name"], "2026-10.pdf");
    assert_eq!(v["mime"], "application/pdf");
    assert_eq!((v["offset"].as_u64(), v["size"].as_u64(), v["len"].as_u64(), v["eof"].as_bool()), (Some(0), Some(12), Some(12), Some(true)));
    let bytes = base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes, b"%PDF october");
    // Chunks: offset and len.
    let r = download(port, &pass, &docs, "reports/2026-10.pdf", "&offset=5&len=3").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["len"].as_u64(), r.json()["eof"].as_bool()), (Some(3), Some(false)));
    let r = download(port, &pass, &docs, "reports/2026-10.pdf", "&len=0").await;
    assert_eq!(r.status, 400, "{}", r.body);
    // A folder entry: everything under it.
    let r = download(port, &pass, &docs, "public/sub/b.txt", "").await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Its sibling, outside entries, secrets, .k2, a link out: the missing-cap
    // 403, existing or not.
    for rel in [
        "reports/2026-11.pdf",
        "reports/2026-12.pdf",
        "private/secret.txt",
        "public/.env",
        ".k2/AGENT.md",
        "public/to-private/secret.txt",
        "reports",
    ] {
        let r = download(port, &pass, &docs, rel, "").await;
        assert_missing_cap(&r, "files:download", rel);
    }
    // Review A6: inside an entry, a folder, a missing file and a link out
    // are all the same missing-cap 403; so are the A3 secrets.
    for rel in [
        "public/sub",
        "public/no-such-file.txt",
        "public/to-private/secret.txt",
        "public/.git/config",
        "public/.aws/credentials",
        "public/infra/terraform.tfstate",
        "public/AuthKey_X.p8",
        "public/notes-with-key.txt",
        "public/harmless.txt",
    ] {
        let r = download(port, &pass, &docs, rel, "").await;
        assert_missing_cap(&r, "files:download", rel);
    }
    // Every read door is the plain files:read 403 (no files:read).
    for route in ["/cli/fs/read-dir", "/cli/fs/read-file", "/cli/fs/read-binary", "/cli/fs/stat"] {
        let r = get(port, route, &pass, &format!("workspace={}&path={}", enc(&docs.handle), enc("public/a.txt"))).await;
        assert_missing_cap(&r, "files:read", route);
    }

    // files.downloads lists the entries, expanded; the link out and the
    // secret file are left out.
    let r = get(port, "/cli/fs/downloads", &pass, &format!("workspace={}", enc(&docs.handle))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["truncated"], false);
    let mut rels: Vec<String> = v["items"].as_array().unwrap().iter().map(|i| i["relPath"].as_str().unwrap().to_string()).collect();
    rels.sort();
    assert_eq!(rels, ["public/a.txt", "public/sub/b.txt", "reports/2026-10.pdf"], ".git, secrets and the content-flagged file are never listed");
    let item = v["items"].as_array().unwrap().iter().find(|i| i["relPath"] == "public/a.txt").cloned().unwrap();
    assert_eq!((item["name"].as_str(), item["size"].as_u64(), item["mime"].as_str()), (Some("a.txt"), Some(8), Some("text/plain")));
    assert!(item["modifiedAt"].as_u64().is_some_and(|t| t > 1_000_000_000_000), "{item}");
    assert!(!r.body.contains(&docs.path.to_string_lossy().to_string()), "never a host path");

    // me.get (AC21): folders.download and can.download.
    let me = get(port, "/cli/skin/me", &pass, "").await.json();
    let agent = me["agents"].as_array().unwrap().iter().find(|a| a["room"] == docs.id.as_str()).cloned().unwrap();
    assert_eq!(agent["folders"], json!({ "download": ["reports/2026-10.pdf", "public"] }));
    assert_eq!(agent["can"]["download"], true);
    assert_eq!(agent["can"]["files"], "none");

    // One audit line per saved chunk.
    let audit = k2_core::auth_audit::tail(200).expect("audit");
    let lines: Vec<&Value> = audit
        .iter()
        .filter(|e| e["event"] == "files.download" && e["user"] == format!("guest:{user}").as_str())
        .collect();
    // Three saved chunks (the full file, its offset-5 chunk, public/sub/b.txt);
    // the refused and 400 calls leave none.
    assert_eq!(lines.len(), 3, "one audit line per saved chunk: {audit:?}");
    // Review A6: refusals and listings are audited too.
    let refusals = audit
        .iter()
        .filter(|e| e["event"] == "files.download_refused" && e["user"] == format!("guest:{user}").as_str())
        .count();
    assert!(refusals >= 15, "every refused save leaves a line: {refusals}");
    assert!(
        audit.iter().any(|e| e["event"] == "files.downloads" && e["user"] == format!("guest:{user}").as_str()),
        "the listing leaves a line"
    );
    let report_lines = lines.iter().filter(|e| e["outcome"].as_str().is_some_and(|o| o.contains("reports/2026-10.pdf"))).count();
    assert_eq!(report_lines, 2, "{lines:?}");
    assert!(lines.iter().all(|e| e["outcome"].as_str().is_some_and(|o| o.contains("via=entry"))), "{lines:?}");

    // The owner token has no download entries: a usage 400, not a read.
    let r = get(port, "/cli/fs/download", OWNER, &format!("workspace={}&path=reports/2026-10.pdf", enc(&docs.handle))).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("App guest"), "{}", r.body);
    // Both App doors serve it (catalog rows, no door = both); no Garden door.
    assert_eq!(k2_core::app_surface::verb_for("GET", "/cli/fs/download"), Some("files.download"));
    assert_eq!(k2_core::app_surface::verb_for("GET", "/cli/fs/downloads"), Some("files.downloads"));
    let row = k2_core::contract::catalog().verb("files.download").unwrap();
    assert_eq!(row.exposure, [k2_core::contract::Exposure::App], "App only: a Garden is the owner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_new_without_entries_and_the_500_cap() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("none{tag}"));
    seed(&docs);

    // files:download without entries and without files:read: nothing.
    let bare = format!("bare{tag}");
    role_with_rooms(port, &bare, &[(&docs, &["thread:read", "files:download"])]).await;
    let pass = login_with_role(port, &format!("bare{tag}"), &bare).await;
    for rel in ["reports/2026-10.pdf", "public/a.txt", "nope.txt"] {
        let r = download(port, &pass, &docs, rel, "").await;
        assert_missing_cap(&r, "files:download", rel);
    }
    let r = get(port, "/cli/fs/downloads", &pass, &format!("workspace={}", enc(&docs.handle))).await;
    assert_eq!(r.json(), json!({ "ok": true, "items": [], "truncated": false }));
    // Without files:download at all: the missing-cap 403 on both doors.
    let reader = format!("reader{tag}");
    role_with_rooms(port, &reader, &[(&docs, &["files:read"])]).await;
    let rpass = login_with_role(port, &format!("reader{tag}"), &reader).await;
    let r = download(port, &rpass, &docs, "public/a.txt", "").await;
    assert_missing_cap(&r, "files:download", "files:read alone");
    let r = get(port, "/cli/fs/downloads", &rpass, &format!("workspace={}", enc(&docs.handle))).await;
    assert_missing_cap(&r, "files:download", "files:read alone lists nothing");
    // files:read + files:download: what it could read, save-shaped; the
    // read scope still walls.
    let both = format!("both{tag}");
    role_with_rooms(port, &both, &[(&docs, &["files:read", "files:download"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": both, "handle": docs.handle, "read": ["public"] })).await;
    let bpass = login_with_role(port, &format!("both{tag}"), &both).await;
    let r = download(port, &bpass, &docs, "public/a.txt", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = download(port, &bpass, &docs, "public/to-private/secret.txt", "").await;
    assert_missing_cap(&r, "files:download", "the read arm's landed check");
    let r = download(port, &bpass, &docs, "private/secret.txt", "").await;
    assert_missing_cap(&r, "files:download", "outside the read folders");

    // The 500 cap: a folder entry with 510 files says truncated.
    for i in 0..510 {
        write(&docs.path, &format!("bulk/f{i:04}.txt"), b"x");
    }
    let bulk = format!("bulk{tag}");
    role_with_rooms(port, &bulk, &[(&docs, &["files:download"])]).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": bulk, "handle": docs.handle, "download": ["bulk"] })).await;
    let kpass = login_with_role(port, &format!("bulk{tag}"), &bulk).await;
    let r = get(port, "/cli/fs/downloads", &kpass, &format!("workspace={}", enc(&docs.handle))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 500);
    assert_eq!(r.json()["truncated"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn widening_entries_is_person_only_and_omitted_entries_are_kept() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let docs = seed_ws(&env, &format!("wid{tag}"));
    seed(&docs);
    allow_manage(&docs, true);
    let agent = passport(&docs);
    let role = format!("wide{tag}");
    role_with_rooms(port, &role, &[(&docs, &["files:read", "files:download"])]).await;
    let line = |v: &Value| v["roomAccess"].as_array().unwrap().iter().find(|a| a["handle"] == docs.handle.as_str()).cloned().unwrap()["folders"].clone();

    // The owner sets entries and read folders.
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": docs.handle, "read": ["public"], "download": ["reports"] })).await;
    // An agent adding or widening an entry: 403 owner_only, nothing changes.
    for download in [json!(["reports", "private/secret.txt"]), json!(["private"]), json!(["."])] {
        let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "download": download })).await;
        if download == json!(["."]) {
            assert_eq!(r.status, 400, "the room root is never an entry: {}", r.body);
            continue;
        }
        assert_eq!(r.status, 403, "{download}: {}", r.body);
        assert_eq!(r.code(), "owner_only", "{}", r.body);
    }
    let r = get(port, "/cli/skin/roles", OWNER, "").await;
    assert!(r.body.contains("reports") && !r.body.contains("private/secret.txt"), "{}", r.body);
    // An agent narrowing: 200 (a file inside the stored folder is covered).
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "download": ["reports/2026-10.pdf"] })).await;
    assert_eq!(r.status, 200, "narrowing: {}", r.body);
    assert_eq!(line(&r.json()), json!({ "read": ["public"], "download": ["reports/2026-10.pdf"] }), "a download-only body keeps the read folders");
    // An agent editing read folders without naming download keeps the entries.
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "read": ["public/sub"] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(line(&r.json()), json!({ "read": ["public/sub"], "download": ["reports/2026-10.pdf"] }), "omitted download = kept");
    // ...and widening back is refused; clearing is narrowing.
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "download": ["reports"] })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "download": [] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(line(&r.json()), json!({ "read": ["public/sub"] }));
    // The owner widens freely.
    let r = owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": docs.handle, "download": ["reports", "public"] })).await;
    assert_eq!(line(&r), json!({ "read": ["public/sub"], "download": ["reports", "public"] }));
    // clear: everything goes, entries included.
    let r = post(port, "/cli/skin/roles/paths", &agent, json!({ "name": role, "handle": docs.handle, "clear": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(line(&r.json()).is_null(), "{}", r.body);
    // .k2 is never an entry, even for the owner.
    let r = post(port, "/cli/skin/roles/paths", OWNER, json!({ "name": role, "handle": docs.handle, "download": [".k2/AGENT.md"] })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains(".k2"), "{}", r.body);
    // files:download itself stays person-only for agents (unchanged).
    let r = post(port, "/cli/skin/roles/room", &agent, json!({ "name": format!("x{tag}"), "handle": docs.handle, "caps": ["files:download"] })).await;
    assert!(r.status == 403 || r.status == 400, "{}", r.body);
}
