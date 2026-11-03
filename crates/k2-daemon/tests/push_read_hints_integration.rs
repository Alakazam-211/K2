//! push-read-hints-v1 end to end through the REAL dispatcher and the REAL
//! off-thread wiring (`test_harness`: in-process, ephemeral port, temp `$HOME`,
//! a paired tunnel on disk). The coalescer's rules are unit tested in
//! `badge_push`; this proves a read route reaches a relay call.
//!
//! No push leaves the machine: the gateway URL is a local fake, the tunnel
//! points at nothing and doesn't start.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::db::schema::WorkspaceSession;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};

const OWNER_TOKEN: &str = "owner-token-read-hints";

/// One silent push's window plus some room.
const WINDOW_WAIT: Duration = Duration::from_secs(16);

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("{method} {path}: read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn ok(port: u16, method: &str, path: &str, body: Option<J>) -> J {
    let b = body.map(|b| b.to_string());
    let r = http(port, method, path, b.as_deref());
    assert_eq!(r.status, 200, "{method} {path}: {}", r.body);
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", r.body))
}

fn gateway_calls() -> &'static Mutex<Vec<J>> {
    static CALLS: OnceLock<Mutex<Vec<J>>> = OnceLock::new();
    CALLS.get_or_init(|| Mutex::new(Vec::new()))
}

fn start_fake_gateway() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake gateway");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || serve_gateway(stream));
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn serve_gateway(stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut len = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).expect("gateway body");
    gateway_calls().lock().expect("calls").push(serde_json::from_slice(&body).expect("gateway got JSON"));
    let reply = r#"{"results":[]}"#;
    let mut out = stream;
    let _ = out.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{}: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            k2_core::push::S0_MARKER_HEADER,
            k2_core::push::S0_MARKER_VALUE,
            reply.len()
        )
        .as_bytes(),
    );
}

fn mark() -> usize {
    gateway_calls().lock().expect("calls").len()
}

fn payloads_since(from: usize) -> Vec<J> {
    gateway_calls().lock().expect("calls")[from..].iter().map(|c| c["payload"].clone()).collect()
}

/// The first push since `from` matching `pred`, within `wait`. Panics loudly
/// with everything seen when none comes.
fn wait_push(from: usize, wait: Duration, what: &str, pred: impl Fn(&J) -> bool) -> J {
    let deadline = Instant::now() + wait;
    loop {
        let got = payloads_since(from);
        if let Some(p) = got.iter().find(|p| pred(p)) {
            return p.clone();
        }
        assert!(Instant::now() < deadline, "{what}: no such push in {wait:?}; saw {got:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn is_read_push(p: &J) -> bool {
    p["data"]["kind"] == "badge" && p["data"].get("read").is_some()
}

fn seed_thread(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/read-hints-{handle}-{id}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    (handle, conv)
}

fn agent_post(port: u16, addr: &str, text: &str) -> i64 {
    let v = ok(port, "POST", &format!("/cli/thread/post?token={OWNER_TOKEN}"), Some(json!({ "addr": addr, "text": text })));
    assert!(v["author"].is_null(), "an agent-shaped post has no person author: {v}");
    v["seq"].as_i64().expect("seq")
}

fn login(port: u16, username: &str) -> String {
    ok(port, "POST", &format!("/cli/users/add?token={OWNER_TOKEN}"), Some(json!({ "username": username, "password": "password123", "mustChangePassword": false })));
    let r = ok(port, "POST", "/cli/auth/login", Some(json!({ "username": username, "password": "password123" })));
    r["token"].as_str().expect("token").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_reaches_the_relay_as_a_read_entry() {
    let tmp = std::env::temp_dir().join(format!("k2-read-hints-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    std::fs::create_dir_all(tmp.join(".k2")).expect(".k2");
    k2_core::test_env::write_executable(&shim_dir.join("claude"), "#!/bin/sh\nexec cat\n");
    // A paired tunnel, which is what the per-daemon relay path needs. It points
    // at nothing and never starts.
    std::fs::write(
        tmp.join(".k2/tunnel.json"),
        json!({ "server_addr": "127.0.0.1", "server_port": 1, "token": "tunnel-tok", "subdomain": "readhints", "local_port": 1, "auto_start": false }).to_string(),
    )
    .expect("tunnel.json");
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    std::env::set_var(k2_core::push::ENV_GATEWAY_URL, start_fake_gateway());

    let d = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(test_harness::start(OWNER_TOKEN)));
    let port = d.port;

    let boot = ok(port, "GET", "/boot-status", None);
    assert!(boot["features"].as_array().expect("features").iter().any(|f| f == "push-read-hints-v1"), "{boot}");

    ok(
        port,
        "POST",
        &format!("/cli/push/register-device?token={OWNER_TOKEN}"),
        Some(json!({ "deviceId": "rh-phone", "platform": "apns", "token": "tok-rh-phone" })),
    );

    // Alert, mute, read: the totals don't move at the read, and it still goes
    // (contract test 2). This is the case that sent nothing before.
    let (h1, c1) = seed_thread("rh1");
    let from = mark();
    let seq1 = agent_post(port, &h1, "first");
    let alert = wait_push(from, WINDOW_WAIT, "the Thread alert", |p| p["data"]["conversationId"] == c1.as_str());
    assert_eq!(alert["data"]["unreadTotal"], "1", "{alert}");
    ok(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(json!({ "conversationId": c1, "muted": true })));
    wait_push(from, WINDOW_WAIT, "the mute's badge correction", |p| p["data"]["kind"] == "badge" && p["data"]["unreadTotal"] == "0");
    let reads = |since: usize| payloads_since(since).into_iter().filter(is_read_push).collect::<Vec<_>>();
    assert_eq!(reads(from), Vec::<J>::new(), "a mute is not a read");
    let from = mark();
    let v = ok(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(json!({ "conversationId": c1, "seq": seq1 })));
    assert_eq!(v["changed"], true, "{v}");
    assert_eq!(v["total"], 0, "the read leaves the total where the mute put it: {v}");
    let hint = wait_push(from, WINDOW_WAIT, "the read entry at equal totals", is_read_push);
    assert_eq!(hint["data"]["read"], format!("t:{c1}:{seq1}"), "{hint}");
    assert_eq!(hint["data"]["unreadTotal"], "0", "{hint}");
    assert!(hint.get("title").is_none() && hint.get("body").is_none(), "silent: {hint}");
    assert!(hint["data"].get("readMore").is_none(), "{hint}");

    // A muted post makes no alert, so there's nothing to clear on its read.
    let seq2 = agent_post(port, &h1, "second, while muted");
    let from = mark();
    ok(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(json!({ "conversationId": c1, "seq": seq2 })));
    std::thread::sleep(WINDOW_WAIT);
    assert_eq!(reads(from), Vec::<J>::new(), "no alert since the last hint: no entry");

    // A person's own post moves their cursor: the alert before it clears.
    let (h2, c2) = seed_thread("rh2");
    let from = mark();
    agent_post(port, &h2, "needs an answer");
    wait_push(from, WINDOW_WAIT, "the second Thread's alert", |p| p["data"]["conversationId"] == c2.as_str());
    let own = ok(port, "POST", &format!("/cli/thread/post?token={OWNER_TOKEN}"), Some(json!({ "addr": h2, "text": "on it", "via": "compose" })));
    assert!(own["author"].is_object(), "a compose post carries its person: {own}");
    let own_seq = own["seq"].as_i64().expect("seq");
    let hint = wait_push(from, WINDOW_WAIT, "the own post's entry", is_read_push);
    assert_eq!(hint["data"]["read"], format!("t:{c2}:{own_seq}"), "{hint}");

    // A project chat: its alert has the post's seq, its read a `p:` entry.
    let gina = login(port, "rhgina");
    let g = ok(port, "POST", &format!("/cli/project-group/create?token={OWNER_TOKEN}"), Some(json!({ "name": "Read hints" })));
    let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).unwrap_or_else(|| panic!("group id: {g}")).to_string();
    let from = mark();
    let m = ok(port, "POST", &format!("/cli/project-group/msg?token={gina}"), Some(json!({ "group": gid, "body": "hi all" })));
    let alert = wait_push(from, WINDOW_WAIT, "the project alert", |p| p["data"]["kind"] == "project" && p["data"]["groupId"] == gid.as_str());
    let seq = alert["data"]["seq"].as_str().unwrap_or_else(|| panic!("a project alert carries seq: {alert}")).to_string();
    assert_eq!(seq, m["seq"].as_i64().map(|s| s.to_string()).unwrap_or_else(|| m["message"]["seq"].to_string()), "{alert} vs {m}");
    let from = mark();
    ok(port, "POST", &format!("/cli/project-group/read?token={OWNER_TOKEN}"), Some(json!({ "group": gid, "seq": seq.parse::<i64>().expect("a seq") })));
    let hint = wait_push(from, WINDOW_WAIT, "the project read entry", is_read_push);
    assert_eq!(hint["data"]["read"], format!("p:{gid}:{seq}"), "{hint}");

    // `k2 push log` shows the attempts, and never the chat.
    let log = ok(port, "GET", &format!("/cli/push/log?token={OWNER_TOKEN}"), None);
    let attempts = log["attempts"].as_array().expect("attempts");
    assert!(attempts.iter().any(|a| a["kind"] == "badge" && a["lane"] == "legacy"), "{log}");
    for id in [&c1, &c2, &gid] {
        assert!(!log.to_string().contains(id.as_str()), "the log names no chat: {log}");
    }

    drop(d);
    let _ = std::fs::remove_dir_all(&tmp);
}
