//! `POST /cli/thread/catchup` (inbox-catchup-v1, K2 0.46.3), headless: the
//! real dispatcher in-process on a temp `$HOME`, over loopback.
//!
//! - Every entry equals the plain read (`GET /cli/thread?addr=…&after_seq=…
//!   &limit=L`, or `&limit=L` for `newest`) minus `ok`, for the owner, a
//!   Connect login, an agent passport (own and other workspace) and an App
//!   guest with scoped files (hidden attachments, own rooms only).
//! - A failing address is an error on that entry only (404 / 403 as on
//!   the plain read).
//! - 405 on GET; 400 on an exactly-one violation and over the caps.
//! - `rev` + `instanceId` on plain reads and the catch-up for people and
//!   agents, never for App guests (C4); `rev` is monotonic.
//! - The 4 MiB budget: an entry cut item by item keeps `has_more:true`
//!   with fewer than `limit` items (C1), later entries are `deferred`, and
//!   top-level `has_more` asks for another call.
//! - Project chat groups equal the plain `after_seq` read.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";
const LIMIT: i64 = 50;

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

async fn connect_login(port: u16, username: &str) -> String {
    owner_ok(port, "/cli/users/add", json!({ "username": username, "password": "password123" })).await;
    let body = json!({ "username": username, "password": "password123" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/auth/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "connect login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn post_text(port: u16, addr: &str, text: &str, files: Option<Value>) -> String {
    let mut body = json!({ "addr": addr, "text": text });
    if let Some(f) = files {
        body["attachments"] = f;
    }
    let r = post(port, "/cli/thread/post", OWNER, body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

async fn catchup(port: u16, token: &str, body: Value) -> Resp {
    post(port, "/cli/thread/catchup", token, body).await
}

async fn catchup_ok(port: u16, token: &str, body: Value) -> Value {
    let r = catchup(port, token, body).await;
    assert_eq!(r.status, 200, "catchup: {}", r.body);
    let v = r.json();
    assert_eq!(v["ok"], true, "{v}");
    v
}

/// What one catch-up entry asks for.
#[derive(Clone)]
enum Want {
    After(String, i64),
    Newest(String),
}

impl Want {
    fn addr(&self) -> &str {
        match self {
            Want::After(a, _) | Want::Newest(a) => a,
        }
    }
    fn entry(&self) -> Value {
        match self {
            Want::After(a, n) => json!({ "addr": a, "afterSeq": n }),
            Want::Newest(a) => json!({ "addr": a, "newest": true }),
        }
    }
    fn query(&self) -> String {
        match self {
            Want::After(a, n) => format!("addr={}&after_seq={n}&limit={LIMIT}", enc(a)),
            Want::Newest(a) => format!("addr={}&limit={LIMIT}", enc(a)),
        }
    }
}

fn strip(mut v: Value, keys: &[&str]) -> Value {
    if let Value::Object(m) = &mut v {
        for k in keys {
            m.remove(*k);
        }
    }
    v
}

/// Each entry equals the plain read for the same caller (`rev` value
/// ignored; its presence is checked: on for people and agents, off for
/// guests). An error entry carries the plain read's status and error.
async fn assert_entries_equal_plain(port: u16, token: &str, wants: &[Want], guest: bool, who: &str) -> Value {
    let body = json!({ "threads": wants.iter().map(Want::entry).collect::<Vec<_>>(), "limit": LIMIT });
    let v = catchup_ok(port, token, body).await;
    assert_eq!(v["has_more"], false, "{who}: {v}");
    if guest {
        assert!(v.get("rev").is_none() && v.get("instanceId").is_none(), "{who}: no rev/instanceId for a guest: {v}");
    } else {
        assert!(v["rev"].is_u64(), "{who}: top-level rev: {v}");
        assert_eq!(v["instanceId"].as_str().map(str::len), Some(16), "{who}: {v}");
    }
    let entries = v["threads"].as_array().unwrap_or_else(|| panic!("{who}: threads: {v}"));
    assert_eq!(entries.len(), wants.len(), "{who}: one entry per request, in order: {v}");
    for (w, e) in wants.iter().zip(entries) {
        let plain = get(port, "/cli/thread", token, &w.query()).await;
        let pv = plain.json();
        if plain.status == 200 {
            if guest {
                assert!(pv.get("rev").is_none() && pv.get("instanceId").is_none(), "{who}: plain guest read has no rev: {}", plain.body);
                assert!(e.get("rev").is_none() && e.get("instanceId").is_none(), "{who}: guest entry has no rev: {e}");
            } else {
                assert!(pv["rev"].is_u64() && e["rev"].is_u64(), "{who}: rev on both: {} / {e}", plain.body);
                assert_eq!(pv["instanceId"], e["instanceId"], "{who}");
                assert_eq!(pv["instanceId"], v["instanceId"], "{who}");
            }
            assert_eq!(
                strip(e.clone(), &["rev"]),
                strip(pv.clone(), &["ok", "rev"]),
                "{who}: entry for {} differs from the plain read",
                w.addr()
            );
        } else {
            assert_eq!(e["addr"], w.addr(), "{who}: {e}");
            assert_eq!(e["status"].as_u64(), Some(plain.status as u64), "{who}: status for {}: {e} vs {}", w.addr(), plain.body);
            let err = &pv["error"];
            if err.is_object() {
                assert_eq!(&e["error"], err, "{who}: error for {}", w.addr());
            } else {
                assert!(e["error"]["code"].is_string() && e["error"]["hint"].is_string(), "{who}: {e}");
            }
            assert!(e.get("items").is_none(), "{who}: an error entry carries no items: {e}");
        }
    }
    v
}

struct World {
    port: u16,
    a: Ws,
    a_conv: String,
    b: Ws,
    s1: i64,
    with_files: String,
}

async fn world(env: &Env, port: u16, tag: &str) -> World {
    let a = seed_ws(env, &format!("cua{tag}"));
    let a_conv = pin_chat(&a);
    let b = seed_ws(env, &format!("cub{tag}"));
    pin_chat(&b);
    write(&a.path, "docs/a.md", b"# A\n");
    write(&a.path, "private/c.txt", b"not for guests");
    post_text(port, &a.handle, "hello", None).await;
    let with_files = post_text(port, &a.handle, "files", Some(json!([{ "path": "docs/a.md" }, { "path": "private/c.txt" }]))).await;
    post_text(port, &a.handle, "third", None).await;
    post_text(port, &b.handle, "b one", None).await;
    let all = get(port, "/cli/thread", OWNER, &format!("addr={}&limit=0", enc(&a.handle))).await.json();
    let s1 = all["items"][0]["seq"].as_i64().expect("seq");
    World { port, a, a_conv, b, s1, with_files }
}

fn wants(w: &World) -> Vec<Want> {
    vec![
        Want::After(w.a.handle.clone(), w.s1),
        Want::Newest(w.a_conv.clone()),
        Want::After(w.b.handle.clone(), 0),
        Want::Newest(w.b.handle.clone() + "-nope"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catchup_entries_equal_the_plain_read_for_every_caller() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let w = world(&env, d.port, tag).await;
    let port = w.port;

    // Owner: every real address serves; the unknown one is a 404 entry.
    let v = assert_entries_equal_plain(port, OWNER, &wants(&w), false, "owner").await;
    let e = &v["threads"];
    assert_eq!(e[0]["after_seq"], w.s1, "{v}");
    assert_eq!(e[0]["items"].as_array().map(Vec::len), Some(2), "after s1: files + third: {v}");
    assert!(e[1].get("after_seq").is_none() && e[1].get("changed").is_none(), "newest is the page shape: {v}");
    assert_eq!(e[1]["items"].as_array().map(Vec::len), Some(3), "{v}");
    assert_eq!(e[3]["status"], 404, "{v}");
    assert_eq!(e[3]["error"]["code"], "not_found", "{v}");
    assert!(v.to_string().contains("private/c.txt"), "the owner sees every file");

    // A Connect login: the same reads as the owner.
    let login = connect_login(port, &format!("cul{tag}")).await;
    assert_entries_equal_plain(port, &login, &wants(&w), false, "connect login").await;

    // An agent passport on A: its own workspace serves, B is a 403 entry.
    let pass = passport(&w.a);
    let v = assert_entries_equal_plain(port, &pass, &wants(&w), false, "agent passport").await;
    assert!(v["threads"][0]["items"].is_array(), "own workspace: {v}");
    assert_eq!((v["threads"][2]["status"].clone(), v["threads"][2]["error"]["code"].clone()), (json!(403), json!("forbidden")), "{v}");

    // An App guest on A with files:read on docs/ only.
    let role = format!("cur{tag}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": w.a.handle, "caps": ["thread:read", "files:read"] })).await;
    owner_ok(port, "/cli/skin/roles/paths", json!({ "name": role, "handle": w.a.handle, "read": ["docs/"] })).await;
    let guest = guest_login(port, &format!("cug{tag}"), &role).await;
    let v = assert_entries_equal_plain(port, &guest, &wants(&w), true, "app guest").await;
    let body = v.to_string();
    assert!(!body.contains("c.txt") && !body.contains("private/"), "the guest's catch-up leaks a hidden file: {body}");
    let files = v["threads"][0]["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == w.with_files.as_str())
        .unwrap_or_else(|| panic!("files post: {v}"))["doc"]["attachments"]
        .clone();
    assert_eq!(files[0]["path"], "docs/a.md", "{files}");
    assert_eq!(files[1], json!({ "kind": "file", "state": "hidden" }), "{files}");
    assert_eq!(v["threads"][2]["error"]["code"], "skin_room", "another room: {v}");
    assert_eq!(v["threads"][2]["status"], 403, "{v}");

    // Groups: an App guest gets an error per entry, never the chat.
    let v = catchup_ok(port, &guest, json!({ "groups": [{ "group": "anything", "afterSeq": 0 }] })).await;
    assert_eq!((v["groups"][0]["status"].clone(), v["groups"][0]["error"]["code"].clone()), (json!(403), json!("forbidden")), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catchup_405_on_get_and_400_on_bad_requests() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let w = world(&env, port, tag).await;

    let r = get(port, "/cli/thread/catchup", OWNER, "").await;
    assert_eq!(r.status, 405, "GET is 405: {}", r.body);

    for bad in [
        json!({ "threads": [{ "addr": w.a.handle, "afterSeq": 1, "newest": true }] }),
        json!({ "threads": [{ "addr": w.a.handle }] }),
        json!({ "threads": [{ "addr": w.a.handle, "afterSeq": -1 }] }),
        json!({ "threads": [{ "addr": w.a.handle, "afterSeq": 0 }], "limit": 0 }),
        json!({ "threads": [{ "addr": w.a.handle, "afterSeq": 0 }], "limit": 501 }),
        json!({ "threads": (0..201).map(|i| json!({ "addr": format!("x{i}"), "afterSeq": 0 })).collect::<Vec<_>>() }),
        json!({ "groups": (0..51).map(|i| json!({ "group": format!("g{i}"), "afterSeq": 0 })).collect::<Vec<_>>() }),
    ] {
        let r = catchup(port, OWNER, bad.clone()).await;
        assert_eq!((r.status, r.code()), (400, "usage".to_string()), "{bad}: {}", r.body);
    }

    // 200 entries are allowed; an unknown address is an error on its entry only.
    let mut many: Vec<Value> = (0..199).map(|i| json!({ "addr": format!("nope{tag}{i}"), "afterSeq": 0 })).collect();
    many.push(json!({ "addr": w.a.handle, "afterSeq": 0 }));
    let v = catchup_ok(port, OWNER, json!({ "threads": many })).await;
    let t = v["threads"].as_array().expect("threads");
    assert_eq!(t.len(), 200);
    assert!(t[..199].iter().all(|e| e["status"] == 404), "{v}");
    assert_eq!(t[199]["items"].as_array().map(Vec::len), Some(3), "{v}");

    // limit defaults to 50 and caps the items per conversation.
    let v = catchup_ok(port, OWNER, json!({ "threads": [{ "addr": w.a.handle, "afterSeq": 0 }], "limit": 2 })).await;
    let e = &v["threads"][0];
    assert_eq!((e["items"].as_array().map(Vec::len), e["has_more"].clone()), (Some(2), json!(true)), "{v}");
    let plain = get(port, "/cli/thread", OWNER, &format!("addr={}&after_seq=0&limit=2", enc(&w.a.handle))).await.json();
    assert_eq!(strip(e.clone(), &["rev"]), strip(plain, &["ok", "rev"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rev_is_on_plain_reads_monotonic_and_matches_the_instance() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let w = world(&env, port, tag).await;
    let boot = get(port, "/boot-status", OWNER, "").await.json();
    assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "inbox-catchup-v1"), "{boot}");
    let instance = boot["instanceId"].as_str().expect("instanceId").to_string();

    let addr = enc(&w.a.handle);
    let mut last = 0u64;
    for (i, q) in [
        format!("addr={addr}"),
        format!("addr={addr}&after_seq=0"),
        format!("addr={addr}&since_seq=1"),
        format!("addr={addr}&before_seq=3"),
    ]
    .iter()
    .enumerate()
    {
        let v = get(port, "/cli/thread", OWNER, q).await.json();
        let rev = v["rev"].as_u64().unwrap_or_else(|| panic!("rev on {q}: {v}"));
        assert_eq!(v["instanceId"], instance.as_str(), "{q}: {v}");
        assert!(rev >= last, "rev never goes back: {rev} < {last}");
        post_text(port, &w.a.handle, &format!("tick {i}"), None).await;
        let after = get(port, "/cli/thread", OWNER, q).await.json()["rev"].as_u64().expect("rev");
        assert!(after > rev, "a published write raises rev: {rev} -> {after}");
        last = after;
    }
    let v = catchup_ok(port, OWNER, json!({ "threads": [{ "addr": w.a.handle, "newest": true }] })).await;
    assert!(v["rev"].as_u64().expect("rev") >= last && v["threads"][0]["rev"].as_u64().expect("rev") >= v["rev"].as_u64().unwrap());
    assert_eq!(v["instanceId"], instance.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catchup_budget_cuts_an_entry_and_defers_the_rest() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let big = "y".repeat(1_500_000);
    let mut handles = Vec::new();
    for i in 0..3 {
        let ws = seed_ws(&env, &format!("cbg{i}{tag}"));
        pin_chat(&ws);
        handles.push(ws.handle.clone());
    }
    // Thread 0: three 1.5 MB posts (4.5 MB > the 4 MiB budget).
    for _ in 0..3 {
        post_text(port, &handles[0], &big, None).await;
    }
    post_text(port, &handles[1], "small", None).await;
    post_text(port, &handles[2], "small", None).await;

    let body = json!({ "threads": handles.iter().map(|h| json!({ "addr": h, "afterSeq": 0 })).collect::<Vec<_>>() });
    let r = catchup(port, OWNER, body).await;
    assert_eq!(r.status, 200, "{}", &r.body[..r.body.len().min(400)]);
    assert!(r.body.len() <= 4 * 1024 * 1024, "the response stays in the budget: {}", r.body.len());
    let v = r.json();
    let t = v["threads"].as_array().expect("threads");
    // C1: the first entry is cut item by item: 2 of 3 items, has_more.
    let first = &t[0];
    assert_eq!(first["items"].as_array().map(Vec::len), Some(2), "cut inside the entry");
    assert_eq!(first["has_more"], true, "has_more with fewer than limit items");
    let seqs: Vec<i64> = first["items"].as_array().unwrap().iter().map(|i| i["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, vec![1, 2], "oldest first");
    for (i, e) in t.iter().enumerate().skip(1) {
        assert_eq!(e, &json!({ "addr": handles[i], "deferred": true }), "later entries are deferred");
    }
    assert_eq!(v["has_more"], true, "call again with the deferred ones");

    // The next call carries on from the cut and serves the deferred ones.
    let body = json!({ "threads": [
        { "addr": handles[0], "afterSeq": 2 },
        { "addr": handles[1], "afterSeq": 0 },
        { "addr": handles[2], "afterSeq": 0 },
    ] });
    let v = catchup_ok(port, OWNER, body).await;
    assert_eq!(v["has_more"], false, "{}", v["has_more"]);
    let t = v["threads"].as_array().expect("threads");
    assert_eq!(t[0]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(t[0]["has_more"], false);
    for (i, e) in t.iter().enumerate().skip(1) {
        assert_eq!(e["items"][0]["doc"]["body"], "small", "entry {i}");
        let plain = get(port, "/cli/thread", OWNER, &format!("addr={}&after_seq=0&limit=50", enc(&handles[i]))).await.json();
        assert_eq!(strip(e.clone(), &["rev"]), strip(plain, &["ok", "rev"]), "uncut entries equal the plain read");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catchup_groups_equal_the_plain_project_chat_delta() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let member = seed_ws(&env, &format!("cgm{tag}"));
    pin_chat(&member);
    let name = format!("Catch {tag}");
    let g = owner_ok(port, "/cli/project-group/create", json!({ "name": name })).await;
    let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).unwrap_or_else(|| panic!("group id: {g}")).to_string();
    owner_ok(port, "/cli/project-group/add-member", json!({ "group": gid, "workspace": member.id })).await;
    for i in 1..=3 {
        owner_ok(port, "/cli/project-group/msg", json!({ "group": gid, "body": format!("p{i}") })).await;
    }
    let body = json!({ "groups": [
        { "group": name, "afterSeq": 1 },
        { "group": format!("Nope {tag}"), "afterSeq": 0 },
    ], "limit": LIMIT });
    // An agent outside the project gets the project chat read's own 403
    // (0.46.2 member rule), on its entry only.
    let outsider = seed_ws(&env, &format!("cgo{tag}"));
    let v = catchup_ok(port, &passport(&outsider), body.clone()).await;
    let plain = get(port, "/cli/project-group/messages", &passport(&outsider), &format!("group={}&after_seq=1&limit={LIMIT}", enc(&name))).await;
    assert_eq!(plain.status, 403, "{}", plain.body);
    assert_eq!(v["groups"][0]["status"], 403, "{v}");
    assert_eq!(v["groups"][0]["error"], plain.json()["error"], "{v}");
    assert!(v["groups"][0].get("messages").is_none(), "{v}");

    for token in [OWNER.to_string(), passport(&member)] {
        let v = catchup_ok(port, &token, body.clone()).await;
        let e = &v["groups"][0];
        let plain = get(port, "/cli/project-group/messages", &token, &format!("group={}&after_seq=1&limit={LIMIT}", enc(&name))).await;
        assert_eq!(plain.status, 200, "{}", plain.body);
        let mut want = strip(plain.json(), &["ok"]);
        want["group"] = json!(name);
        assert_eq!(e, &want, "group entry equals the plain read");
        let bodies: Vec<&str> = e["messages"].as_array().expect("messages").iter().map(|m| m["body"].as_str().unwrap()).collect();
        assert_eq!(bodies, vec!["p2", "p3"]);
        let missing = get(port, "/cli/project-group/messages", &token, &format!("group={}&after_seq=0", enc(&format!("Nope {tag}")))).await;
        let m = &v["groups"][1];
        assert_eq!(m["status"].as_u64(), Some(missing.status as u64), "{m}");
        assert_eq!(m["error"], missing.json()["error"], "{m}");
        assert!(v["threads"].as_array().expect("threads").is_empty());
    }
}
