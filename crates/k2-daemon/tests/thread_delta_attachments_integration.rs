//! Thread attachments on the thread-delta-v1 read (`GET /cli/thread?
//! after_seq=N`), headless: the real dispatcher in-process on a temp
//! `$HOME`, over loopback.
//!
//! The delta's `items` and `changed` carry the same per-caller attachment
//! view as the plain read (`thread-attachments-v1`):
//!
//! - (a) a live file is `state: "ok"`;
//! - (b) edited → `changed`, moved → `gone`, in `items` and in `changed`;
//! - (c) an App guest gets `{kind, state:"hidden"}` (no name, path or
//!   size) for files outside its file rights, exactly as its plain read;
//! - (d) the owner's delta matches the plain read's attachment JSON.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;

use app_fixture::*;
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

async fn post_files(port: u16, addr: &str, text: &str, files: Value) -> String {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": text, "attachments": files })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

/// The plain read (`limit=0`: every item) as `token` sees it.
async fn plain(port: u16, token: &str, addr: &str) -> Resp {
    let r = get(port, "/cli/thread", token, &format!("addr={}&limit=0", enc(addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r
}

/// The delta read after `after` as `token` sees it.
async fn delta(port: u16, token: &str, addr: &str, after: i64) -> Resp {
    let r = get(port, "/cli/thread", token, &format!("addr={}&after_seq={after}&limit=0", enc(addr))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["after_seq"], after, "a delta answer: {}", r.body);
    r
}

/// The item with doc id `id` in `list` (a JSON array of `{seq, id, doc}`).
fn item<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no item {id} in {list}"))
}

fn atts(it: &Value) -> Value {
    let a = it["doc"]["attachments"].clone();
    assert!(a.as_array().is_some_and(|a| !a.is_empty()), "attachments: {it}");
    a
}

fn states(it: &Value) -> Vec<String> {
    atts(it)
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["state"].as_str().unwrap_or_else(|| panic!("no state on {a}")).to_string())
        .collect()
}

/// Mark doc `id` (Thread seq `seq`) changed in place, as an answered card
/// is: the delta lists it in `changed` for a reader that holds it.
fn mark_changed(conv: &str, id: &str, seq: i64) {
    let doc = k2_core::overlay::get_thread_doc(conv, id).expect("store").expect("the doc");
    k2_core::overlay::store::put_card_doc(conv, seq, &doc).expect("mark changed");
}

struct Seeded {
    port: u16,
    ws: Ws,
    conv: String,
    addr: String,
    /// `[docs/a.md, private/c.txt]`
    p1: String,
    s1: i64,
    /// `[docs/b.md, private/c.txt]`
    p2: String,
    s2: i64,
}

async fn seed(env: &Env, port: u16, tag: &str) -> Seeded {
    let ws = seed_ws(env, &format!("dat{tag}"));
    let conv = pin_chat(&ws);
    let addr = ws.handle.clone();
    write(&ws.path, "docs/a.md", b"# A\n");
    write(&ws.path, "docs/b.md", b"# B\n");
    write(&ws.path, "private/c.txt", b"not for guests");
    post(port, "/cli/thread/post", OWNER, json!({ "addr": addr, "text": "hello" })).await;
    let p1 = post_files(port, &addr, "first", json!([{ "path": "docs/a.md" }, { "path": "private/c.txt" }])).await;
    let p2 = post_files(port, &addr, "second", json!([{ "path": "docs/b.md" }, { "path": "private/c.txt" }])).await;
    let all = plain(port, OWNER, &addr).await.json()["items"].clone();
    let s1 = item(&all, &p1)["seq"].as_i64().expect("seq");
    let s2 = item(&all, &p2)["seq"].as_i64().expect("seq");
    assert!(s1 > 0 && s2 > s1, "seqs {s1} {s2}");
    Seeded { port, ws, conv, addr, p1, s1, p2, s2 }
}

/// For each id, the delta item's attachments equal the plain read's.
fn assert_same_view(plain_items: &Value, delta_list: &Value, ids: &[&str], who: &str) {
    for id in ids {
        assert_eq!(atts(item(delta_list, id)), atts(item(plain_items, id)), "{who}: delta vs plain attachments for {id}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_delta_attachments_owner_live_state_matches_plain() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let s = seed(&env, d.port, tag).await;
    let (port, addr) = (s.port, s.addr.as_str());

    // (a) live files are `ok` on the delta.
    let dl = delta(port, OWNER, addr, 0).await.json();
    assert_eq!(states(item(&dl["items"], &s.p1)), ["ok", "ok"]);
    assert_eq!(states(item(&dl["items"], &s.p2)), ["ok", "ok"]);
    // (d) the owner's delta matches the plain read.
    let pl = plain(port, OWNER, addr).await.json();
    assert_eq!(atts(item(&pl["items"], &s.p1))[1]["path"], "private/c.txt", "the owner sees every file");
    assert_same_view(&pl["items"], &dl["items"], &[&s.p1, &s.p2], "owner");

    // (b) edited → `changed`; moved → `gone`, in `items`.
    write(&s.ws.path, "docs/a.md", b"# A, edited and longer\n");
    std::fs::rename(s.ws.path.join("docs/b.md"), s.ws.path.join("docs/b-moved.md")).unwrap();
    let dl = delta(port, OWNER, addr, 0).await.json();
    assert_eq!(states(item(&dl["items"], &s.p1)), ["changed", "ok"]);
    assert_eq!(states(item(&dl["items"], &s.p2)), ["gone", "ok"]);
    assert_eq!(atts(item(&dl["items"], &s.p2))[0]["name"], "b.md", "gone keeps its snapshot for the chip");
    let pl = plain(port, OWNER, addr).await.json();
    assert_same_view(&pl["items"], &dl["items"], &[&s.p1, &s.p2], "owner after edit/move");

    // (b) …and in `changed` for a reader that already holds both.
    mark_changed(&s.conv, &s.p1, s.s1);
    mark_changed(&s.conv, &s.p2, s.s2);
    let dl = delta(port, OWNER, addr, s.s2).await.json();
    assert!(dl["items"].as_array().expect("items").is_empty(), "nothing newer than s2: {dl}");
    assert_eq!(states(item(&dl["changed"], &s.p1)), ["changed", "ok"]);
    assert_eq!(states(item(&dl["changed"], &s.p2)), ["gone", "ok"]);
    assert_same_view(&pl["items"], &dl["changed"], &[&s.p1, &s.p2], "owner changed");
    // `removed` is `{id, seq}` only.
    assert_eq!(dl["removed"], json!([]), "{dl}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_delta_attachments_app_guest_sees_hidden() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let s = seed(&env, d.port, tag).await;
    let (port, addr) = (s.port, s.addr.as_str());
    mark_changed(&s.conv, &s.p1, s.s1);
    mark_changed(&s.conv, &s.p2, s.s2);

    // A guest whose files:read covers docs/ only, and one with no files:read.
    let role = format!("docs{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": s.ws.handle, "caps": ["thread:read", "files:read"] })).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": s.ws.handle, "read": ["docs/"] })).await;
    let scoped = login_with_role(port, &format!("pat{tag}"), &role).await;
    let role2 = format!("talk{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role2 })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role2, "handle": s.ws.handle, "caps": ["thread:read"] })).await;
    let no_files = login_with_role(port, &format!("sam{tag}"), &role2).await;

    let hidden = json!({ "kind": "file", "state": "hidden" });

    // Scoped guest: docs/ shows (with live state), private/ is hidden.
    let pl = plain(port, &scoped, addr).await.json();
    let items = delta(port, &scoped, addr, 0).await;
    let changed = delta(port, &scoped, addr, s.s2).await;
    for (what, r, list) in [("items", &items, "items"), ("changed", &changed, "changed")] {
        let v = r.json();
        let a1 = atts(item(&v[list], &s.p1));
        let a2 = atts(item(&v[list], &s.p2));
        assert_eq!(a1[1], hidden, "scoped {what}: private/ is outside the guest's folders: {}", r.body);
        assert_eq!(a2[1], hidden, "scoped {what}: {}", r.body);
        assert!(!r.body.contains("c.txt") && !r.body.contains("private/"), "scoped {what} leaks a hidden file: {}", r.body);
        assert_eq!(a1[0]["path"], "docs/a.md", "scoped {what}: {}", r.body);
        assert_eq!(a1[0]["state"], "ok", "scoped {what}: {}", r.body);
        assert_eq!(a2[0]["path"], "docs/b.md", "scoped {what}: {}", r.body);
        assert_same_view(&pl["items"], &v[list], &[&s.p1, &s.p2], &format!("scoped {what}"));
    }

    // No files:read: every file hidden, nothing about any of them.
    let pl = plain(port, &no_files, addr).await.json();
    let items = delta(port, &no_files, addr, 0).await;
    let changed = delta(port, &no_files, addr, s.s2).await;
    for (what, r, list) in [("items", &items, "items"), ("changed", &changed, "changed")] {
        let v = r.json();
        for id in [&s.p1, &s.p2] {
            for a in atts(item(&v[list], id)).as_array().unwrap() {
                assert_eq!(a, &hidden, "no-files {what}: {}", r.body);
            }
        }
        for leak in ["a.md", "b.md", "c.txt", "\"size\"", "\"path\"", "\"name\""] {
            assert!(!r.body.contains(leak), "no-files {what} leaks {leak}: {}", r.body);
        }
        assert_same_view(&pl["items"], &v[list], &[&s.p1, &s.p2], &format!("no-files {what}"));
    }
}
