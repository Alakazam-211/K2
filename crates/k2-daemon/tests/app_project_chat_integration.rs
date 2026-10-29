//! Project chat as an App surface (0.46.3), headless: the real dispatcher
//! in process on a temp `$HOME`, driven over loopback with guest passes
//! made through the real login, and on an App's own gateway link.
//!
//! - `project.list`: a project shows only when every member agent is a
//!   room the guest holds with `thread:read`; one unreachable agent hides
//!   it, and its name and id never appear.
//! - R3: a hidden project answers every verb byte for byte what an unknown
//!   one answers (403 `skin_room`), the socket included.
//! - `project.read` pages back with `before_seq` and catches up with
//!   `after_seq`, byte-identical to the owner's `/cli/project-group/messages`.
//! - `project.post`: 403 `cap_not_granted` without `project:write`, 200 with
//!   it; stored as `guest:<username>`.
//! - `project.events`: frames for owner and guest lines, shaped like
//!   Thread frames with `collection: "project"`; a new unreachable member
//!   closes the socket before the next line.
//! - The doors: the helper and the link forward the rows; the owner token
//!   and GETs on the POST rows are refused; an App's own pass has no read
//!   state (`not_exposed`).

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A box-login guest whose role holds `caps` in each room; returns its pass.
async fn guest(port: u16, username: &str, rooms: &[(&Ws, &[&str])]) -> String {
    owner_post(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(&username, "s3cret-horse");
    let role = format!("r-{username}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role })).await;
    for (ws, caps) in rooms {
        owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    }
    owner_post(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn project(port: u16, name: &str, members: &[&Ws]) -> String {
    let g = owner_post(port, "/cli/project-group/create", json!({ "name": name })).await;
    let id = g["id"].as_str().expect("group id").to_string();
    for m in members {
        owner_post(port, "/cli/project-group/add-member", json!({ "group": id, "workspace": m.id })).await;
    }
    id
}

fn ids(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["projects"].as_array().unwrap_or_else(|| panic!("projects in {v}")).iter().map(|p| p["groupId"].as_str().unwrap().to_string()).collect();
    out.sort();
    out
}

fn seqs(v: &Value) -> Vec<i64> {
    v["messages"].as_array().unwrap_or_else(|| panic!("messages in {v}")).iter().map(|m| m["seq"].as_i64().unwrap()).collect()
}

/// One request on an App's link with `pass` as Bearer.
fn on_link(port: u16, method: &str, pq: &str, pass: &str, body: Option<&str>) -> Resp {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect link");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
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

async fn link(port: u16, method: &'static str, pq: String, pass: String, body: Option<String>) -> Resp {
    tokio::task::spawn_blocking(move || on_link(port, method, &pq, &pass, body.as_deref())).await.unwrap()
}

/// The next text frame on a socket, within 10 s.
async fn next_frame<S>(sock: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), sock.next())
            .await
            .expect("a frame in time")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(t) = m {
            return serde_json::from_str(&t).expect("JSON frame");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_chat_for_app_guests_end_to_end() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    let hr = seed_ws(&env, &format!("hr{tag}"));
    let launch = project(port, &format!("launch-{tag}"), &[&sales, &ops]).await;
    let payroll_name = format!("payroll-{tag}");
    let payroll = project(port, &payroll_name, &[&sales, &hr]).await;
    let solo = project(port, &format!("solo-{tag}"), &[&sales]).await;
    owner_post(port, "/cli/project-group/msg", json!({ "group": payroll, "body": "payroll secret line" })).await;

    let rw: &[&str] = &["thread:read", "project:write"];
    let ro: &[&str] = &["thread:read"];
    let writer = guest(port, &format!("wes{tag}"), &[(&sales, rw), (&ops, rw)]).await;
    let reader = guest(port, &format!("rae{tag}"), &[(&sales, ro), (&ops, ro)]).await;

    // ── visibility (the all-members rule) ──────────────────────────
    let r = get(port, "/cli/skin/projects", &reader, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mut want = vec![launch.clone(), solo.clone()];
    want.sort();
    assert_eq!(ids(&r.json()), want, "payroll has hr, which rae can't reach: {}", r.body);
    assert!(!r.body.contains(&payroll) && !r.body.contains(&payroll_name) && !r.body.contains(&hr.id), "{}", r.body);
    assert!(!r.body.contains(&reader), "the pass is never echoed");

    // ── R3: hidden = unknown, byte for byte ─────────────────────────
    let pairs = [
        ("GET", "/cli/skin/projects/messages", "before_seq=5"),
        ("GET", "/cli/skin/projects/messages", ""),
    ];
    for (_, path, extra) in pairs {
        let hidden = get(port, path, &reader, &format!("group={payroll}&{extra}")).await;
        let by_name = get(port, path, &reader, &format!("group={}&{extra}", enc(&payroll_name))).await;
        let unknown = get(port, path, &reader, &format!("group=no-such-{tag}&{extra}")).await;
        assert_eq!((hidden.status, hidden.code()), (403, "skin_room".to_string()), "{}", hidden.body);
        assert_eq!((hidden.status, &hidden.body), (unknown.status, &unknown.body));
        assert_eq!((by_name.status, &by_name.body), (unknown.status, &unknown.body));
    }
    for (path, extra) in [("/cli/skin/projects/post", json!({ "body": "x" })), ("/cli/skin/projects/read", json!({ "seq": 1 }))] {
        let with = |group: &str| {
            let mut b = extra.clone();
            b["group"] = json!(group);
            b
        };
        let hidden = post(port, path, &writer, with(&payroll)).await;
        let unknown = post(port, path, &writer, with(&format!("no-such-{tag}"))).await;
        assert_eq!((hidden.status, hidden.code()), (403, "skin_room".to_string()), "{path}: {}", hidden.body);
        assert_eq!((hidden.status, &hidden.body), (unknown.status, &unknown.body), "{path}");
    }
    let url = |g: &str, pass: &str| format!("ws://127.0.0.1:{port}/cli/skin/projects/events?group={g}&token={}", enc(pass));
    let refused_status = |e: tokio_tungstenite::tungstenite::Error| match e {
        tokio_tungstenite::tungstenite::Error::Http(resp) => resp.status().as_u16(),
        other => panic!("expected an HTTP refusal, got {other:?}"),
    };
    let hidden_ws = tokio_tungstenite::connect_async(&url(&payroll, &reader)).await.map(|_| ()).expect_err("hidden: no socket");
    let unknown_ws = tokio_tungstenite::connect_async(&url("nope", &reader)).await.map(|_| ()).expect_err("unknown: no socket");
    assert_eq!((refused_status(hidden_ws), refused_status(unknown_ws)), (403, 403));

    // ── paging, byte-identical to the owner's read ─────────────────
    for i in 1..=60 {
        owner_post(port, "/cli/project-group/msg", json!({ "group": launch, "body": format!("pc{i}") })).await;
    }
    for q in ["before_seq=61&limit=25", "before_seq=36&limit=25", "before_seq=11&limit=25", "before_seq=1", "after_seq=50", ""] {
        let g = get(port, "/cli/skin/projects/messages", &reader, &format!("group={launch}&{q}")).await;
        let o = get(port, "/cli/project-group/messages", OWNER, &format!("group={launch}&{q}")).await;
        assert_eq!(g.status, 200, "{q}: {}", g.body);
        assert_eq!(g.body, o.body, "{q}: the guest's page is the owner's");
    }
    let v = get(port, "/cli/skin/projects/messages", &reader, &format!("group={launch}&before_seq=61&limit=25")).await.json();
    assert_eq!((seqs(&v).first().copied(), seqs(&v).last().copied(), v["has_more"].as_bool()), (Some(36), Some(60), Some(true)));
    let v = get(port, "/cli/skin/projects/messages", &reader, &format!("group={launch}&before_seq=11&limit=25")).await.json();
    assert_eq!((seqs(&v), v["has_more"].as_bool()), ((1..=10).collect::<Vec<_>>(), Some(false)));
    let r = get(port, "/cli/skin/projects/messages", &reader, &format!("group={launch}&before_seq=0")).await;
    assert_eq!((r.status, r.code()), (400, "usage".to_string()));

    // ── the socket, then posting with and without the cap ──────────
    let (mut sock, _) = tokio_tungstenite::connect_async(&url(&launch, &reader)).await.expect("rae reads launch");
    let r = post(port, "/cli/skin/projects/post", &reader, json!({ "group": launch, "body": "can I?" })).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "{}", r.body);
    assert_eq!(r.json()["error"]["cap"], "project:write");
    let r = post(port, "/cli/skin/projects/post", &writer, json!({ "group": launch, "body": "Ship Friday?" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let posted = r.json();
    assert_eq!((posted["author"].as_str(), posted["authorKind"].as_str(), posted["seq"].as_i64()), (Some(format!("guest:wes{tag}").as_str()), Some("guest"), Some(61)));
    assert!(posted.get("deliveredSessionId").is_none(), "{posted}");
    let f = next_frame(&mut sock).await;
    assert_eq!((f["collection"].as_str(), f["seq"].as_i64(), f["groupId"].as_str()), (Some("project"), Some(61), Some(launch.as_str())), "{f}");
    assert_eq!((f["doc"]["author"].as_str(), f["doc"]["body"].as_str()), (Some(format!("guest:wes{tag}").as_str()), Some("Ship Friday?")));
    owner_post(port, "/cli/project-group/msg", json!({ "group": launch, "body": "Yes" })).await;
    let f = next_frame(&mut sock).await;
    assert_eq!((f["seq"].as_i64(), f["doc"]["author"].as_str(), f["doc"]["authorKind"].as_str()), (Some(62), Some("owner"), Some("person")));
    // A line in another project never reaches this socket; the owner's
    // view shows the guest's line as a guest's.
    owner_post(port, "/cli/project-group/msg", json!({ "group": solo, "body": "elsewhere" })).await;
    let o = get(port, "/cli/project-group/messages", OWNER, &format!("group={launch}&after_seq=60")).await.json();
    assert_eq!(o["messages"][0]["author"], format!("guest:wes{tag}"));

    // A new member rae can't reach hides the project: the socket closes
    // instead of sending the next line, and the list drops it.
    owner_post(port, "/cli/project-group/add-member", json!({ "group": launch, "workspace": hr.id })).await;
    owner_post(port, "/cli/project-group/msg", json!({ "group": launch, "body": "after hr joined" })).await;
    let end = tokio::time::timeout(Duration::from_secs(10), sock.next()).await.expect("the socket ends in time");
    match end {
        None | Some(Ok(Message::Close(_))) | Some(Err(_)) => {}
        Some(Ok(m)) => panic!("no frame after the project became hidden, got {m:?}"),
    }
    assert_eq!(ids(&get(port, "/cli/skin/projects", &reader, "").await.json()), vec![solo.clone()]);
    owner_post(port, "/cli/project-group/remove-member", json!({ "group": launch, "workspace": hr.id })).await;

    // ── read state: the guest's own, shared across its surfaces ────
    let row = |v: Value, g: &str| v["projects"].as_array().unwrap().iter().find(|p| p["groupId"] == g).cloned().expect("listed");
    let wes = row(get(port, "/cli/skin/projects", &writer, "").await.json(), &launch);
    assert_eq!((wes["canPost"].as_bool(), wes["lastSeq"].as_i64()), (Some(true), Some(63)));
    // Surface B: rae signs in again (another device, another pass) and
    // opens the project socket there; surface A marks the chat read.
    let rae_user = format!("rae{tag}");
    let body = json!({ "username": rae_user, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "second login: {}", r.body);
    let reader_b = r.json()["token"].as_str().expect("token").to_string();
    assert_ne!(reader_b, reader, "two sessions");
    let before = row(get(port, "/cli/skin/projects", &reader_b, "").await.json(), &launch);
    assert!(before["unread"].as_i64().unwrap() > 0, "{before}");
    let (mut sock_b, _) = tokio_tungstenite::connect_async(&url(&launch, &reader_b)).await.expect("B's socket");
    let r = post(port, "/cli/skin/projects/read", &reader, json!({ "group": launch })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["unread"].as_i64(), r.json()["lastReadSeq"].as_i64()), (Some(0), Some(63)));
    let f = next_frame(&mut sock_b).await;
    assert_eq!(
        (f["collection"].as_str(), f["id"].as_str(), f["unread"].as_i64(), f["lastReadSeq"].as_i64()),
        (Some("read"), Some(launch.as_str()), Some(0), Some(63)),
        "B hears A's read: {f}"
    );
    let rae = row(get(port, "/cli/skin/projects", &reader_b, "").await.json(), &launch);
    assert_eq!((rae["unread"].as_i64(), rae["lastReadSeq"].as_i64(), rae["canPost"].as_bool()), (Some(0), Some(63), Some(false)), "same person, same key");
    // The key is the guest's person key, the one its Thread read state uses.
    {
        let me = get(port, "/cli/skin/me", &reader_b, "").await;
        assert_eq!(me.status, 200, "{}", me.body);
        let pid = me.json()["principalId"].as_str().expect("a guest has a principal").to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM read_cursors WHERE person = ?1 AND kind = 'project' AND conv_id = ?2 AND last_read_seq = 63",
                rusqlite::params![format!("guest:{pid}"), launch],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
    drop(sock_b);

    // ── the doors ──────────────────────────────────────────────────
    let r = get(port, "/cli/skin/projects", OWNER, "").await;
    assert_eq!((r.status, r.code()), (403, "skin_token".to_string()), "the owner uses /cli/project-group: {}", r.body);
    let r = get(port, "/cli/skin/projects/post", &writer, &format!("group={launch}&body=x")).await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);
    for (m, p) in [("GET", "/cli/skin/projects"), ("GET", "/cli/skin/projects/messages"), ("POST", "/cli/skin/projects/post"), ("POST", "/cli/skin/projects/read")] {
        assert!(k2_daemon::skin_gateway::allowlisted_http(m, p), "helper {m} {p}");
        assert!(gateway::route_allowed(m, p, "", false), "link {m} {p}");
    }
    assert!(k2_daemon::skin_gateway::allowlisted_ws("/cli/skin/projects/events"));
    assert!(gateway::route_allowed("GET", "/cli/skin/projects/events", &format!("group={launch}"), true));
    // The project chat routes for people are still not for passes.
    let r = get(port, "/cli/project-group/messages", &reader, &format!("group={launch}")).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // An App's own pass on its link: its room only, no read state.
    let jobs = format!("jobs{tag}");
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let dir = sales.path.join(&jobs);
        std::fs::create_dir_all(&dir).unwrap();
        let svc = k2_core::published_services::insert(&conn, &sales.id, &jobs, "node server.mjs", &dir.to_string_lossy(), 47_101, "local", "stopped", "cmd", "")
            .expect("insert service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: &jobs,
                service_id: &svc,
                seed_id: None,
                owner_workspace: &sales.id,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .expect("insert app");
        k2_daemon::apps::grants::record(&conn, &jobs, &["thread:read".to_string()]).expect("approved set");
        svc
    };
    let pass = gateway::open(
        LinkSpec { app_id: jobs.clone(), service_id: svc, workspace_id: sales.id.clone(), port: 0, caps: vec!["thread:read".into()], rooms: vec![sales.id.clone()] },
        None,
    )
    .expect("open link");
    let lport = gateway::port_of(&jobs).expect("listening");
    let r = link(lport, "GET", "/cli/skin/projects".into(), pass.clone(), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(ids(&v), vec![solo.clone()], "the App's own room only: {v}");
    assert_eq!(v["projects"][0]["unread"], Value::Null);
    let r = link(lport, "GET", format!("/cli/skin/projects/messages?group={solo}"), pass.clone(), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = link(lport, "POST", "/cli/skin/projects/read".into(), pass.clone(), Some(json!({ "group": solo }).to_string())).await;
    assert_eq!((r.status, r.code()), (403, "not_exposed".to_string()), "{}", r.body);
    let r = link(lport, "POST", "/cli/skin/projects/post".into(), pass.clone(), Some(json!({ "group": solo, "body": "x" }).to_string())).await;
    assert_eq!((r.status, r.code()), (403, "cap_not_granted".to_string()), "{}", r.body);
    let r = link(lport, "GET", format!("/cli/skin/projects/messages?group={launch}"), pass.clone(), None).await;
    assert_eq!((r.status, r.code()), (403, "skin_room".to_string()), "launch has ops: {}", r.body);

    gateway::close(&jobs, None, true);
    drop(env);
}
