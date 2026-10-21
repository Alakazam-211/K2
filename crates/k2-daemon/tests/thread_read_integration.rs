//! thread-read-v1 (0.46.1, unread counts) end to end through the REAL
//! dispatcher (`test_harness`: in-process, ephemeral port, temp `$HOME`,
//! agent shim). Raw HTTP and WebSockets over loopback. Covers:
//!
//! 1. Routes: per-person cursors (the owner and a Connect user never see
//!    each other's), forward-only acks, own posts advance own cursor,
//!    405 on GET of the POST routes, 403 for agent passports / no token,
//!    404 / 400, the `thread-read-v1` feature key, and the person-only
//!    `unread` fields on `thread/latest` rows.
//! 2. Waiting cards (only an answer clears them, reading never), mute
//!    (keeps the count, leaves the badge, waiting still counts), and the
//!    project chat equivalents (`project-group/read`, list rows, `seq`).
//! 3. `thread_read` frames reach EVERY socket of that person (the owner
//!    token and a linked login alike) and no other person's socket.
//! 4. Push: Thread pushes carry the person's `unreadTotal`/`waitingTotal`;
//!    a muted Thread pushes nobody who muted it; a Connect user's own
//!    project chat post never pushes their own phone (the self-push bug).
//!    A fake push gateway on loopback records every dispatch.
//!
//! No push leaves the machine: the gateway URL is a local fake.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;

const OWNER_TOKEN: &str = "owner-token-thread-read-0461";

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
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
    r.json()
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

// ── A fake push gateway (loopback) ────────────────────────────────────

/// Every dispatch body the daemon POSTed to `/push/dispatch`.
fn gateway_calls() -> &'static Mutex<Vec<J>> {
    static CALLS: OnceLock<Mutex<Vec<J>>> = OnceLock::new();
    CALLS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Start the fake gateway once per process and point the daemon at it
/// (env, read by the push config resolver). Every test calls this before
/// starting its daemon, so the first push config the process resolves
/// already sees it.
fn ensure_fake_gateway() {
    static URL: OnceLock<String> = OnceLock::new();
    let url = URL.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake gateway");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                std::thread::spawn(move || serve_gateway(stream));
            }
        });
        format!("http://127.0.0.1:{port}")
    });
    std::env::set_var(k2_core::push::ENV_GATEWAY_URL, url);
    std::env::set_var(k2_core::push::ENV_GATEWAY_TOKEN, "fake-gateway-token");
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
    let v: J = serde_json::from_slice(&body).expect("gateway got JSON");
    gateway_calls().lock().expect("calls").push(v);
    let reply = r#"{"results":[]}"#;
    let mut out = stream;
    let _ = out.write_all(
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len())
            .as_bytes(),
    );
}

fn gateway_mark() -> usize {
    gateway_calls().lock().expect("calls").len()
}

/// Dispatch payloads since `mark` that match `pred`, waiting up to 8 s for
/// at least `want` of them.
fn wait_pushes(mark: usize, want: usize, pred: impl Fn(&J) -> bool) -> Vec<J> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let got: Vec<J> = gateway_calls().lock().expect("calls")[mark..]
            .iter()
            .map(|c| c["payload"].clone())
            .filter(|p| pred(p))
            .collect();
        if got.len() >= want || Instant::now() > deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// ── Setup ─────────────────────────────────────────────────────────────

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev_home.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_shim.take() {
                Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
                None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    ensure_fake_gateway();
    let tmp = std::env::temp_dir().join(format!("k2-thread-read-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    std::fs::create_dir_all(tmp.join(".k2")).expect(".k2");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _restore = Restore {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    f();
}

/// The test DB is one per process: forget every conversation and read
/// state an earlier test in this binary left, so each test's totals count
/// only its own Threads and chats.
fn isolate_read_state() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch(
        "DELETE FROM overlay_conversations; DELETE FROM project_groups; \
         DELETE FROM read_cursors; DELETE FROM read_people WHERE person != 'owner';",
    )
    .expect("isolate read state");
}

/// A workspace with a pinned Chat: (project id, handle, conversation id).
fn seed_thread(prefix: &str) -> (String, String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/thread-read-{handle}-{id}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    (id, handle, conv)
}

fn login(port: u16, username: &str) -> String {
    ok(port, "POST", &format!("/cli/users/add?token={OWNER_TOKEN}"), Some(serde_json::json!({ "username": username, "password": "password123" })));
    let r = ok(port, "POST", "/cli/auth/login", Some(serde_json::json!({ "username": username, "password": "password123" })));
    r["token"].as_str().expect("token").to_string()
}

fn passport(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: workspace_uuid.to_string(), agent_address: "tr-agent".to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// Wake the agent (`k2 msg`, which posts nothing to the Thread) so the
/// card answers below land on a live Chat. A wake used to mint a new pin
/// when the seeded id had no transcript on disk (the shim writes none),
/// moving the Thread; it keeps the pin now (0.46.3 smoke). Returns the
/// conversation (the seeded one, still empty: seqs start at 1 there).
fn warm_up(port: u16, handle: &str, seeded: &str) -> String {
    let v = ok(
        port,
        "POST",
        &format!("/cli/workspace/msg?token={OWNER_TOKEN}&workspace={handle}&text=warm-up&from=tester&wake=true&wake_timeout=2"),
        None,
    );
    assert_eq!(v["success"].as_bool(), Some(true), "the warm-up wake: {v}");
    for _ in 0..3 {
        let v = ok(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={handle}&limit=1"), None);
        assert_eq!(v["conversation_id"], seeded, "a wake must keep the pinned Chat: {v}");
        assert_eq!(v["items"], serde_json::json!([]), "a k2 msg wake posts nothing to the Thread: {v}");
        std::thread::sleep(Duration::from_millis(200));
    }
    seeded.to_string()
}

/// An agent-shaped Thread post (owner token, not the compose bar: the
/// daemon stamps the room handle, no person author).
fn agent_post(port: u16, addr: &str, text: &str) -> i64 {
    let v = ok(port, "POST", &format!("/cli/thread/post?token={OWNER_TOKEN}"), Some(serde_json::json!({ "addr": addr, "text": text })));
    assert!(v["author"].is_null(), "an agent-shaped post has no person author: {v}");
    v["seq"].as_i64().expect("seq")
}

/// A person's compose-bar post with `token`.
fn person_post(port: u16, token: &str, addr: &str, text: &str) -> i64 {
    let v = ok(port, "POST", &format!("/cli/thread/post?token={token}"), Some(serde_json::json!({ "addr": addr, "text": text, "via": "compose" })));
    assert!(v["author"].is_object(), "a compose post carries its person: {v}");
    v["seq"].as_i64().expect("seq")
}

fn unread(port: u16, token: &str) -> J {
    ok(port, "GET", &format!("/cli/thread/unread?token={token}"), None)
}

fn conv_state<'a>(summary: &'a J, id: &str) -> Option<&'a J> {
    summary["byConversation"].as_array().expect("byConversation").iter().find(|c| c["id"] == id)
}

fn read(port: u16, token: &str, body: J) -> J {
    ok(port, "POST", &format!("/cli/thread/read?token={token}"), Some(body))
}

// ── 1. Routes ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_routes_keep_one_cursor_per_person_and_refuse_agents() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_read_state();
        let (ws, h, seeded) = seed_thread("tr1");
        let gina = login(port, "trgina");

        let boot = ok(port, "GET", "/boot-status", None);
        assert!(
            boot["features"].as_array().expect("features").iter().any(|f| f == "thread-read-v1"),
            "feature key: {boot}"
        );

        let conv = warm_up(port, &h, &seeded);
        assert_eq!(agent_post(port, &h, "first"), 1);
        assert_eq!(agent_post(port, &h, "second"), 2);
        let o = unread(port, OWNER_TOKEN);
        assert_eq!(o["person"], "owner");
        let st = conv_state(&o, &conv).unwrap_or_else(|| panic!("owner sees the Thread: {o}"));
        assert_eq!((st["unread"].as_i64(), st["waiting"].as_i64(), st["muted"].as_bool()), (Some(2), Some(0), Some(false)), "{st}");
        assert_eq!(st["kind"], "thread");
        assert_eq!(st["projectId"], ws.as_str());
        assert_eq!((st["lastReadSeq"].as_i64(), st["lastSeq"].as_i64(), st["seq"].as_i64()), (Some(0), Some(2), Some(0)), "{st}");
        assert_eq!((o["total"].as_i64(), o["waitingTotal"].as_i64()), (Some(2), Some(0)), "the warm-up was the owner's own: {o}");
        // Gina (seen before this Thread existed) has both unread.
        let g = unread(port, &gina);
        assert_eq!(g["person"], "trgina");
        assert_eq!(conv_state(&g, &conv).unwrap_or_else(|| panic!("gina sees it: {g}"))["unread"], 2);

        // The owner reads everything: their counts clear, gina's don't.
        let r = read(port, OWNER_TOKEN, serde_json::json!({ "conversationId": conv }));
        assert_eq!((r["changed"].as_bool(), r["unread"].as_i64(), r["seq"].as_i64(), r["total"].as_i64()), (Some(true), Some(0), Some(2), Some(0)), "{r}");
        assert!(conv_state(&unread(port, OWNER_TOKEN), &conv).is_none(), "nothing unread, waiting or muted: not listed");
        assert_eq!(conv_state(&unread(port, &gina), &conv).expect("gina")["unread"], 2, "a person sees only their own state");

        // Gina reads by address, forward only.
        let r = read(port, &gina, serde_json::json!({ "addr": h, "seq": 1 }));
        assert_eq!((r["changed"].as_bool(), r["unread"].as_i64(), r["lastReadSeq"].as_i64()), (Some(true), Some(1), Some(1)), "{r}");
        for back in [1, 0] {
            let r = read(port, &gina, serde_json::json!({ "addr": h, "seq": back }));
            assert_eq!((r["changed"].as_bool(), r["lastReadSeq"].as_i64()), (Some(false), Some(1)), "never backwards: {r}");
        }

        // Gina's own post advances her cursor; it is unread for the owner.
        assert_eq!(person_post(port, &gina, &h, "my reply"), 3);
        let g = unread(port, &gina);
        assert!(conv_state(&g, &conv).is_none(), "her own post never counts and reads through it: {g}");
        let o = unread(port, OWNER_TOKEN);
        let st = conv_state(&o, &conv).unwrap_or_else(|| panic!("owner: {o}"));
        assert_eq!((st["unread"].as_i64(), st["lastReadSeq"].as_i64()), (Some(1), Some(2)), "{st}");
        // The owner's own post does the same for the owner.
        assert_eq!(person_post(port, OWNER_TOKEN, &h, "thanks"), 4);
        let o = unread(port, OWNER_TOKEN);
        assert!(conv_state(&o, &conv).is_none(), "the owner read through their own post: {o}");

        // thread/latest rows carry the PERSON's state; an agent's do not.
        agent_post(port, &h, "update");
        let v = ok(port, "GET", &format!("/cli/thread/latest?token={gina}&addrs={h}"), None);
        let row = &v["items"][0];
        assert_eq!((row["unread"].as_i64(), row["waiting"].as_i64(), row["muted"].as_bool(), row["lastReadSeq"].as_i64()), (Some(2), Some(0), Some(false), Some(3)), "{row}");
        let agent = passport(&ws);
        let v = ok(port, "GET", &format!("/cli/thread/latest?token={agent}&addrs={h}"), None);
        let row = &v["items"][0];
        assert!(row.get("unread").is_none() && row.get("lastReadSeq").is_none(), "agents have no read state: {row}");

        // 405 on GET of the POST routes (before auth), and POST on the GET.
        for p in ["/cli/thread/read", "/cli/thread/mute", "/cli/project-group/read"] {
            for t in [OWNER_TOKEN, ""] {
                let r = http(port, "GET", &format!("{p}?token={t}"), None);
                assert_eq!(r.status, 405, "GET {p}: {}", r.body);
            }
        }
        let r = http(port, "POST", &format!("/cli/thread/unread?token={OWNER_TOKEN}"), Some("{}"));
        assert_eq!(r.status, 405, "{}", r.body);

        // Agents and anonymous callers are refused; nothing changes.
        for (who, tok) in [("agent passport", agent.as_str()), ("no token", "")] {
            let r = http(port, "GET", &format!("/cli/thread/unread?token={tok}"), None);
            assert_eq!(r.status, 403, "{who} unread: {}", r.body);
            let r = http(port, "POST", &format!("/cli/thread/read?token={tok}"), Some(&serde_json::json!({ "conversationId": conv }).to_string()));
            assert_eq!(r.status, 403, "{who} read: {}", r.body);
            let r = http(port, "POST", &format!("/cli/thread/mute?token={tok}"), Some(&serde_json::json!({ "conversationId": conv, "muted": true }).to_string()));
            assert_eq!(r.status, 403, "{who} mute: {}", r.body);
            let r = http(port, "POST", &format!("/cli/project-group/read?token={tok}"), Some(r#"{"group":"x"}"#));
            assert_eq!(r.status, 403, "{who} project read: {}", r.body);
        }
        assert_eq!(conv_state(&unread(port, &gina), &conv).expect("gina")["unread"], 2, "untouched by refused calls");

        // Errors are loud.
        let r = http(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(r#"{"conversationId":"no-such-conversation"}"#));
        assert_eq!(r.status, 404, "{}", r.body);
        let r = http(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(r#"{"addr":"nosuchaddr123"}"#));
        assert_eq!(r.status, 404, "{}", r.body);
        let r = http(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(&serde_json::json!({ "conversationId": conv, "seq": -3 }).to_string()));
        assert_eq!(r.status, 400, "{}", r.body);
        let r = http(port, "POST", &format!("/cli/thread/read?token={OWNER_TOKEN}"), Some(r#"{}"#));
        assert_eq!(r.status, 400, "{}", r.body);
        let r = http(port, "GET", &format!("/cli/thread/unread?token={OWNER_TOKEN}&kind=dm"), None);
        assert_eq!(r.status, 400, "{}", r.body);
    });
}

// ── 2. Waiting, mute, project chats ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_mute_and_project_chats_through_the_routes() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_read_state();
        let (_ws, h, seeded) = seed_thread("tr2");
        let conv = warm_up(port, &h, &seeded);
        agent_post(port, &h, "context");
        let card = ok(port, "POST", &format!("/cli/thread/ask?token={OWNER_TOKEN}"), Some(serde_json::json!({ "addr": h, "prompt": "Ship it?", "options": "Ship,Wait" })));
        let card_id = card["id"].as_str().expect("card id").to_string();

        let o = unread(port, OWNER_TOKEN);
        let st = conv_state(&o, &conv).expect("listed");
        assert_eq!((st["unread"].as_i64(), st["waiting"].as_i64()), (Some(1), Some(1)), "the card waits, it is not unread: {st}");
        assert_eq!((o["total"].as_i64(), o["waitingTotal"].as_i64()), (Some(2), Some(1)), "{o}");

        // Reading never clears waiting.
        let r = read(port, OWNER_TOKEN, serde_json::json!({ "conversationId": conv }));
        assert_eq!((r["unread"].as_i64(), r["waiting"].as_i64(), r["total"].as_i64(), r["waitingTotal"].as_i64()), (Some(0), Some(1), Some(1), Some(1)), "{r}");

        // Mute: keeps its count, leaves the badge; waiting still counts.
        agent_post(port, &h, "more");
        let m = ok(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(serde_json::json!({ "conversationId": conv, "muted": true })));
        assert_eq!((m["changed"].as_bool(), m["muted"].as_bool(), m["unread"].as_i64(), m["waiting"].as_i64()), (Some(true), Some(true), Some(1), Some(1)), "{m}");
        assert_eq!((m["total"].as_i64(), m["waitingTotal"].as_i64()), (Some(1), Some(1)), "muted unread leaves the badge: {m}");
        let o = unread(port, OWNER_TOKEN);
        assert!(o["muted"].as_array().expect("muted").iter().any(|x| x["kind"] == "thread" && x["id"] == conv.as_str()), "mute read-back: {o}");
        let row = &ok(port, "GET", &format!("/cli/thread/latest?token={OWNER_TOKEN}&addrs={h}"), None)["items"][0];
        assert_eq!(row["muted"], true, "{row}");
        let r = http(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(&serde_json::json!({ "conversationId": conv }).to_string()));
        assert_eq!(r.status, 400, "muted is required: {}", r.body);

        // Only the answer clears waiting.
        ok(port, "POST", &format!("/cli/thread/answer?token={OWNER_TOKEN}"), Some(serde_json::json!({ "addr": h, "id": card_id, "answer": "Ship" })));
        let o = unread(port, OWNER_TOKEN);
        let st = conv_state(&o, &conv).expect("still listed: muted");
        assert_eq!((st["waiting"].as_i64(), st["muted"].as_bool()), (Some(0), Some(true)), "{st}");
        assert_eq!((o["total"].as_i64(), o["waitingTotal"].as_i64()), (Some(0), Some(0)), "{o}");
        let m = ok(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(serde_json::json!({ "addr": h, "muted": false })));
        assert_eq!((m["muted"].as_bool(), m["total"].as_i64()), (Some(false), Some(1)), "unmuted: its unread is back in the badge: {m}");

        // Project chat: own lines never count, an agent's do.
        let (member_ws, member_handle, _) = seed_thread("tr2m");
        let g = ok(port, "POST", &format!("/cli/project-group/create?token={OWNER_TOKEN}"), Some(serde_json::json!({ "name": format!("Launch {}", &conv[..6]) })));
        let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).unwrap_or_else(|| panic!("group id: {g}")).to_string();
        ok(port, "POST", &format!("/cli/project-group/add-member?token={OWNER_TOKEN}"), Some(serde_json::json!({ "group": gid, "workspace": member_ws })));
        let own = ok(port, "POST", &format!("/cli/project-group/msg?token={OWNER_TOKEN}"), Some(serde_json::json!({ "group": gid, "body": "kickoff" })));
        assert_eq!((own["seq"].as_i64(), own["authorKind"].as_str()), (Some(1), Some("person")), "{own}");
        let agent = ok(port, "POST", &format!("/cli/project-group/msg?token={OWNER_TOKEN}"), Some(serde_json::json!({ "group": gid, "body": "on it", "author": member_handle })));
        assert_eq!((agent["seq"].as_i64(), agent["authorKind"].as_str()), (Some(2), Some("agent")), "{agent}");
        let o = ok(port, "GET", &format!("/cli/thread/unread?token={OWNER_TOKEN}&kind=project"), None);
        let st = conv_state(&o, &gid).unwrap_or_else(|| panic!("project listed: {o}"));
        assert_eq!((st["kind"].as_str(), st["unread"].as_i64(), st["lastReadSeq"].as_i64(), st["lastSeq"].as_i64()), (Some("project"), Some(1), Some(1), Some(2)), "{st}");
        assert!(o["byConversation"].as_array().expect("list").iter().all(|c| c["kind"] == "project"), "kind filters the list: {o}");
        let list = ok(port, "GET", &format!("/cli/project-group/list?token={OWNER_TOKEN}"), None);
        let row = list["groups"].as_array().expect("groups").iter().find(|r| r["id"] == gid.as_str()).expect("row");
        assert_eq!((row["unread"].as_i64(), row["waiting"].as_i64(), row["muted"].as_bool(), row["lastSeq"].as_i64()), (Some(1), Some(0), Some(false), Some(2)), "{row}");
        let msgs = ok(port, "GET", &format!("/cli/project-group/messages?token={OWNER_TOKEN}&group={gid}"), None);
        let seqs: Vec<i64> = msgs["messages"].as_array().expect("messages").iter().map(|m| m["seq"].as_i64().expect("seq")).collect();
        assert_eq!(seqs, vec![1, 2]);
        let r = ok(port, "POST", &format!("/cli/project-group/read?token={OWNER_TOKEN}"), Some(serde_json::json!({ "group": gid })));
        assert_eq!((r["kind"].as_str(), r["unread"].as_i64(), r["seq"].as_i64()), (Some("project"), Some(0), Some(2)), "{r}");
        let m = ok(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(serde_json::json!({ "kind": "project", "group": gid, "muted": true })));
        assert_eq!((m["kind"].as_str(), m["muted"].as_bool()), (Some("project"), Some(true)), "{m}");
        let r = http(port, "POST", &format!("/cli/project-group/read?token={OWNER_TOKEN}"), Some(r#"{"group":"no such project at all"}"#));
        assert_eq!(r.status, 404, "{}", r.body);
    });
}

// ── 3. Live frames ────────────────────────────────────────────────────

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn events_socket(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("events WS opens");
    // The hello frame says the subscription (and its presence entry) is live.
    let hello = next_kind(&mut ws, "hello", "", Duration::from_secs(5)).await;
    assert!(hello.is_some(), "no hello");
    ws
}

/// The next frame of `kind` (and `id`, when not empty) within `wait`.
async fn next_kind(ws: &mut Ws, kind: &str, id: &str, wait: Duration) -> Option<J> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => return None,
            Ok(None) => panic!("events socket closed"),
            Ok(Some(Err(e))) => panic!("events socket error: {e}"),
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) => {
                let v: J = serde_json::from_str(&t).expect("frame JSON");
                if v["kind"] == kind && (id.is_empty() || v["id"] == id) {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_frames_reach_every_socket_of_that_person_and_nobody_else() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_read_state();
        let (_ws, h, conv) = seed_thread("tr3");
        let gina = login(port, "trfgina");
        // The owner's phone: a Connect login the owner linked = the owner person.
        let phone = login(port, "trfphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(serde_json::json!({ "accounts": ["trfphone"] })));
        agent_post(port, &h, "one");
        agent_post(port, &h, "two");
        futures_block(async {
            let mut desk = events_socket(port, OWNER_TOKEN).await;
            let mut desk2 = events_socket(port, OWNER_TOKEN).await;
            let mut phone_ws = events_socket(port, &phone).await;
            let mut gina_ws = events_socket(port, &gina).await;
            // Frames for the posts above may still be in flight: let them land.
            for ws in [&mut desk, &mut desk2, &mut phone_ws, &mut gina_ws] {
                while next_kind(ws, "thread_read", "", Duration::from_millis(400)).await.is_some() {}
            }

            // The owner reads on the desktop: every owner socket hears it.
            tokio::task::block_in_place(|| read(port, OWNER_TOKEN, serde_json::json!({ "conversationId": conv, "seq": 1 })));
            for (name, ws) in [("desktop", &mut desk), ("second window", &mut desk2), ("linked phone", &mut phone_ws)] {
                let f = next_kind(ws, "thread_read", &conv, Duration::from_secs(5))
                    .await
                    .unwrap_or_else(|| panic!("{name} got no thread_read"));
                assert_eq!(
                    (f["seq"].as_i64(), f["lastReadSeq"].as_i64(), f["lastSeq"].as_i64(), f["unread"].as_i64(), f["waiting"].as_i64(), f["muted"].as_bool()),
                    (Some(1), Some(1), Some(2), Some(1), Some(0), Some(false)),
                    "{name}: {f}"
                );
                assert_eq!((f["total"].as_i64(), f["waitingTotal"].as_i64()), (Some(1), Some(0)), "{name}: {f}");
                assert!(f.get("person").is_none(), "the person key never rides the wire: {f}");
            }
            assert!(
                next_kind(&mut gina_ws, "thread_read", &conv, Duration::from_millis(1500)).await.is_none(),
                "another person's socket never gets the owner's read"
            );

            // An agent post: each person's sockets get THEIR counts.
            tokio::task::block_in_place(|| agent_post(port, &h, "three"));
            let f = next_kind(&mut gina_ws, "thread_read", &conv, Duration::from_secs(5)).await.expect("gina hears the post");
            assert_eq!(f["unread"].as_i64(), Some(3), "gina never read: {f}");
            let f = next_kind(&mut desk, "thread_read", &conv, Duration::from_secs(5)).await.expect("owner hears the post");
            assert_eq!(f["unread"].as_i64(), Some(2), "the owner read 1 of 3: {f}");
            let f = next_kind(&mut phone_ws, "thread_read", &conv, Duration::from_secs(5)).await.expect("the phone hears the post");
            assert_eq!(f["unread"].as_i64(), Some(2), "same person, same counts: {f}");
            let f = next_kind(&mut desk2, "thread_read", &conv, Duration::from_secs(5)).await.expect("the second window hears the post");
            assert_eq!(f["unread"].as_i64(), Some(2), "{f}");

            // Gina mutes on her device: only her sockets hear it.
            tokio::task::block_in_place(|| {
                ok(port, "POST", &format!("/cli/thread/mute?token={gina}"), Some(serde_json::json!({ "conversationId": conv, "muted": true })))
            });
            let f = next_kind(&mut gina_ws, "thread_read", &conv, Duration::from_secs(5)).await.expect("gina's mute frame");
            assert_eq!((f["muted"].as_bool(), f["unread"].as_i64(), f["total"].as_i64()), (Some(true), Some(3), Some(0)), "{f}");
            assert!(
                next_kind(&mut desk2, "thread_read", &conv, Duration::from_millis(1500)).await.is_none(),
                "the owner's sockets never get gina's mute"
            );
        });
    });
}

// ── 4. Push data, mute and the self-push fix ──────────────────────────

fn register(port: u16, token: &str, label: &str) -> String {
    let id = format!("dev-tr-{label}-{}", uuid::Uuid::new_v4());
    ok(
        port,
        "POST",
        &format!("/cli/push/register-device?token={token}"),
        Some(serde_json::json!({ "deviceId": id, "platform": "apns", "token": format!("apns-{label}-{}", uuid::Uuid::new_v4()) })),
    );
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushes_carry_totals_skip_muted_people_and_never_the_author() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_read_state();
        let (_ws, h, conv) = seed_thread("tr4");
        let gina = login(port, "trpgina");
        let owner_dev = register(port, OWNER_TOKEN, "owner");
        let gina_dev = register(port, &gina, "gina");
        // Pushes say whose they are with `badgeKey`, never a name.
        let owner_key = unread(port, OWNER_TOKEN)["badgeKey"].as_str().expect("owner badgeKey").to_string();
        let gina_key = unread(port, &gina)["badgeKey"].as_str().expect("gina badgeKey").to_string();
        assert_ne!(owner_key, gina_key);
        let test_start = gateway_mark();

        // A Thread push per person, each with that person's totals.
        let mark = gateway_mark();
        agent_post(port, &h, "build is green");
        let thread_push = |p: &J, key: &str, c: &str| {
            p["data"]["kind"] == "thread" && p["data"]["badgeKey"] == key && p["data"]["conversationId"] == c
        };
        let got = wait_pushes(mark, 2, |p| p["data"]["conversationId"] == conv.as_str());
        let owner_p = got.iter().find(|p| thread_push(p, &owner_key, &conv)).unwrap_or_else(|| panic!("owner push: {got:?}"));
        let gina_p = got.iter().find(|p| thread_push(p, &gina_key, &conv)).unwrap_or_else(|| panic!("gina push: {got:?}"));
        for p in [owner_p, gina_p] {
            assert_eq!(p["data"]["unreadTotal"], "1", "one unread Thread line: {p}");
            assert_eq!(p["data"]["waitingTotal"], "0", "{p}");
            assert!(p.get("badge").is_none() && p.get("aps").is_none(), "never aps.badge (TP27): {p}");
        }

        // Gina mutes a second Thread before anything is posted in it (its
        // catalog row exists, as after a chatter line): the first post
        // pushes the owner and never gina.
        let (ws2, h2, conv2) = seed_thread("tr4b");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::overlay::catalog::ensure_conversation(&conn, &conv2, &ws2).expect("catalog row");
        }
        let m = ok(port, "POST", &format!("/cli/thread/mute?token={gina}"), Some(serde_json::json!({ "conversationId": conv2, "muted": true })));
        assert_eq!(m["muted"], true, "{m}");
        let mark = gateway_mark();
        agent_post(port, &h2, "deploying now");
        let got = wait_pushes(mark, 1, |p| p["data"]["conversationId"] == conv2.as_str());
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["data"]["badgeKey"], owner_key.as_str(), "{}", got[0]);
        assert_eq!(got[0]["data"]["unreadTotal"], "2", "the owner has 1 + 1 unread across both Threads: {}", got[0]);
        std::thread::sleep(Duration::from_millis(1500));
        let muted = wait_pushes(mark, 0, |p| p["data"]["conversationId"] == conv2.as_str() && p["data"]["badgeKey"] == gina_key.as_str());
        assert!(muted.is_empty(), "a muted Thread never pushes the person who muted it: {muted:?}");

        // Project chat: gina's OWN post never pushes her phone; the owner's does.
        let g = ok(port, "POST", &format!("/cli/project-group/create?token={OWNER_TOKEN}"), Some(serde_json::json!({ "name": format!("Push {}", &conv[..6]) })));
        let gid = g["group"]["id"].as_str().or_else(|| g["id"].as_str()).unwrap_or_else(|| panic!("group id: {g}")).to_string();
        let mark = gateway_mark();
        ok(port, "POST", &format!("/cli/project-group/msg?token={gina}"), Some(serde_json::json!({ "group": gid, "body": "hi all" })));
        let project_push = |p: &J| p["data"]["kind"] == "project" && p["data"]["groupId"] == gid.as_str();
        let got = wait_pushes(mark, 1, |p| project_push(p) && p["data"]["badgeKey"] == owner_key.as_str());
        assert_eq!(got.len(), 1, "the owner's phone hears gina's post: {got:?}");
        assert_eq!(got[0]["title"], "K2");
        assert!(!got[0].to_string().contains("hi all"), "content-free: {}", got[0]);
        assert!(got[0]["data"]["unreadTotal"].as_str().is_some_and(|t| t.parse::<i64>().is_ok_and(|n| n >= 1)), "{}", got[0]);
        assert_eq!(got[0]["data"]["waitingTotal"], "0");
        std::thread::sleep(Duration::from_millis(1500));
        let mine = wait_pushes(mark, 0, |p| project_push(p) && p["data"]["badgeKey"] == gina_key.as_str());
        assert!(mine.is_empty(), "a person's own project post never pushes their own phone: {mine:?}");

        // The owner mutes the chat: gina's next post pushes nobody.
        ok(port, "POST", &format!("/cli/thread/mute?token={OWNER_TOKEN}"), Some(serde_json::json!({ "kind": "project", "group": gid, "muted": true })));
        let mark = gateway_mark();
        ok(port, "POST", &format!("/cli/project-group/msg?token={gina}"), Some(serde_json::json!({ "group": gid, "body": "anyone?" })));
        std::thread::sleep(Duration::from_millis(1500));
        let any = wait_pushes(mark, 0, |p| project_push(p));
        assert!(any.is_empty(), "the owner muted it and gina wrote it: no push at all: {any:?}");
        // Gina gets the owner's line (the owner's own post skips the owner).
        let mark = gateway_mark();
        ok(port, "POST", &format!("/cli/project-group/msg?token={OWNER_TOKEN}"), Some(serde_json::json!({ "group": gid, "body": "here" })));
        let got = wait_pushes(mark, 1, |p| project_push(p));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["data"]["badgeKey"], gina_key.as_str(), "{}", got[0]);

        // Privacy (0.46.1): nothing this test sent through the relay names a
        // person: no `principal` key, no username, not the owner.
        let all: Vec<J> = gateway_calls().lock().expect("calls")[test_start..].iter().map(|c| c["payload"].clone()).collect();
        assert_eq!(all.len(), 5, "2 Thread + 1 muted-Thread + 2 project pushes");
        for p in &all {
            assert!(p["data"].get("principal").is_none(), "principal in {p}");
            let wire = p.to_string();
            assert!(!wire.contains("trpgina") && !wire.contains("\"owner\""), "a name rides the push: {wire}");
            assert!(p["data"]["badgeKey"].as_str().is_some_and(|k| k == owner_key || k == gina_key), "{p}");
        }

        // Leave no phones behind for other tests in this process.
        for (tok, id) in [(OWNER_TOKEN, owner_dev), (gina.as_str(), gina_dev)] {
            ok(port, "POST", &format!("/cli/push/unregister-device?token={tok}"), Some(serde_json::json!({ "deviceId": id })));
        }
    });
}

// ── 5. badgeKey: one per person per server, never a name ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn badge_key_is_one_per_person_across_logins_and_never_a_name() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        isolate_read_state();
        let phone = login(port, "trkphone");
        ok(port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(serde_json::json!({ "accounts": ["trkphone"] })));
        // Connect usernames are [a-z0-9_-] only; an email-like name is
        // covered by the k2-core privacy test.
        let mail = login(port, "rosson_long-2");
        let key_of = |tok: &str| unread(port, tok)["badgeKey"].as_str().expect("badgeKey").to_string();
        let owner_key = key_of(OWNER_TOKEN);
        assert_eq!(owner_key.len(), 32);
        assert!(owner_key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{owner_key}");
        assert_eq!(key_of(&phone), owner_key, "the owner token and a linked login are one person: one key");
        let mail_key = key_of(&mail);
        assert_ne!(mail_key, owner_key, "another person, another key");
        assert!(!mail_key.contains("rosson") && !owner_key.contains("owner") && !owner_key.contains("trkphone"));

        // Stable across a fresh session of the same login, and handed back
        // on register-device and GET /cli/push/device.
        let r = ok(port, "POST", "/cli/auth/login", Some(serde_json::json!({ "username": "trkphone", "password": "password123" })));
        let phone2 = r["token"].as_str().expect("token").to_string();
        assert_ne!(phone2, phone);
        assert_eq!(key_of(&phone2), owner_key, "a re-login keeps the key");
        let dev = format!("dev-trk-{}", uuid::Uuid::new_v4());
        let reg = ok(
            port,
            "POST",
            &format!("/cli/push/register-device?token={phone2}"),
            Some(serde_json::json!({ "deviceId": dev, "platform": "apns", "token": format!("apns-trk-{}", uuid::Uuid::new_v4()) })),
        );
        assert_eq!(reg["badgeKey"], owner_key.as_str(), "register reply: {reg}");
        let again = ok(
            port,
            "POST",
            &format!("/cli/push/register-device?token={phone2}"),
            Some(serde_json::json!({ "deviceId": dev, "platform": "apns", "token": format!("apns-trk-rot-{}", uuid::Uuid::new_v4()) })),
        );
        assert_eq!(again["badgeKey"], owner_key.as_str(), "a re-registration (token rotation) keeps it");
        let got = ok(port, "GET", &format!("/cli/push/device?token={phone2}&deviceId={dev}"), None);
        assert_eq!(got["badgeKey"], owner_key.as_str(), "device read-back: {got}");
        ok(port, "POST", &format!("/cli/push/unregister-device?token={phone2}"), Some(serde_json::json!({ "deviceId": dev })));
    });
}
