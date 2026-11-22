//! Reading a terminal reaches only what the caller may read, whichever way
//! the session is named.
//!
//! `/cli/terminal/read` takes a session four ways: `id=` / `session=` (its
//! UUID), `id=<project_id>[:<agent>]` (the canonical key) and `workspace=`
//! (a name, path, id, sidecar or session id). Every form goes through one
//! check (`terminal_routes::may_read_session`):
//!
//! - a workspace-scoped caller (an agent's passport, over TCP or its cell
//!   socket) reads only its own workspace's sessions and those of
//!   workspaces it is connected to; a session K2 can't place is refused;
//! - an App pass and an API key read no terminal;
//! - a remote-session grant reads only its own shell;
//! - the owner token and every login role read as before.
//!
//! The grid and bytes sockets serve only the session their token opens.
//!
//! The real dispatcher in process on a temp `$HOME`, real `/bin/cat`
//! PTYs. Fail-loudly: every wait has a deadline that panics with what it saw.

use crate::support::app_fixture::*;

use std::time::Duration;

use futures_util::StreamExt;
use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

// ── helpers ──────────────────────────────────────────────────────────

/// Spawn a real `/bin/cat` PTY under `agent_name` in `cwd`; its session id.
async fn spawn_cat(port: u16, agent_name: &str, cwd: &std::path::Path) -> String {
    let body = json!({
        "agent_name": agent_name,
        "cwd": cwd.to_string_lossy(),
        "command": k2_core::test_env::pty_idle_program("/bin/cat"),
        "cols": 80,
        "rows": 24,
    });
    let r = post(port, "/cli/sessions/v2/spawn", OWNER, body).await;
    assert_eq!(r.status, 200, "spawn {agent_name}: {}", r.body);
    r.json()["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("spawn {agent_name}: no sessionId: {}", r.body))
        .to_string()
}

/// `GET /cli/terminal/read?<query>` with `token` as `?token=`.
async fn read(port: u16, token: &str, query: &str) -> Resp {
    get(port, "/cli/terminal/read", token, query).await
}

/// `GET /cli/terminal/read?<query>` with `token` as `Authorization: Bearer`
/// (how the `k2` CLI sends a passport).
async fn read_bearer(port: u16, token: &str, query: &str) -> Resp {
    let pq = format!("/cli/terminal/read?{query}");
    let headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
    tokio::task::spawn_blocking(move || req_blocking_with(port, "GET", &pq, None, &headers))
        .await
        .unwrap()
}

/// Every row that answered wrong, so one run shows every form that fails
/// (not only the first); [`finish`] fails the test with the whole list.
static MISSES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn miss(line: String) {
    MISSES.lock().unwrap_or_else(|e| e.into_inner()).push(line);
}

fn assert_refused(r: &Resp, code: Option<&str>, what: &str) {
    if r.status != 403 || r.body.contains("\"lines\"") {
        miss(format!("{what}: must be refused, got {} {}", r.status, r.body));
        return;
    }
    if let Some(code) = code {
        let got = serde_json::from_str::<Value>(&r.body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string));
        if got.as_deref() != Some(code) {
            miss(format!("{what}: refused with the wrong code (want {code}): {}", r.body));
        }
    }
}

fn assert_reads(r: &Resp, what: &str) {
    let lines = serde_json::from_str::<Value>(&r.body).ok().is_some_and(|v| v["lines"].is_array());
    if r.status != 200 || !lines {
        miss(format!("{what}: must read, got {} {}", r.status, r.body));
    }
}

/// Fail with every row that answered wrong.
fn finish() {
    let misses = std::mem::take(&mut *MISSES.lock().unwrap_or_else(|e| e.into_inner()));
    assert!(misses.is_empty(), "{} row(s) answered wrong:\n{}", misses.len(), misses.join("\n"));
}

fn link(a: &str, b: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rid = uuid::Uuid::new_v4().to_string();
    k2_core::db::schema::WorkspaceRelation::create(&conn, &rid, a, b, "collaborator")
        .expect("link workspaces");
}

fn login(username: &str, role: Role) -> String {
    connect_users::add_user(username, "s3cret-horse-battery").expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

/// A box-login guest whose role holds `caps` in `ws`; returns its pass.
async fn guest(port: u16, username: &str, ws: &Ws, caps: &[&str]) -> String {
    let owner_post = |path: &'static str, body: Value| async move {
        let r = post(port, path, OWNER, body).await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    };
    owner_post("/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, "s3cret-horse");
    let role = format!("r-{username}");
    owner_post("/cli/skin/roles", json!({ "name": role })).await;
    owner_post("/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_post("/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": "s3cret-horse" }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

/// Two workspaces, A and B (not connected), each with a canonical session;
/// B also has an agent session under `<B>:helper`.
struct Box2 {
    a: Ws,
    b: Ws,
    sa: String,
    sb: String,
    sb_helper: String,
}

async fn two_workspaces(env: &Env, port: u16) -> Box2 {
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let a = seed_ws(env, &format!("tra{tag}"));
    let b = seed_ws(env, &format!("trb{tag}"));
    let sa = spawn_cat(port, &a.id, &a.path).await;
    let sb = spawn_cat(port, &b.id, &b.path).await;
    let sb_helper = spawn_cat(port, &format!("{}:helper", b.id), &b.path).await;
    Box2 { a, b, sa, sb, sb_helper }
}

/// Every form that names one of B's sessions (`sb`, or `<B>:helper`).
fn forms_naming_b(x: &Box2) -> Vec<(String, &'static str)> {
    vec![
        (format!("id={}", x.sb), "id=<uuid>"),
        (format!("session={}", x.sb), "session=<uuid>"),
        (format!("id={}", enc(&format!("{}:helper", x.b.id))), "id=<project_id>:<agent>"),
        (format!("session={}", enc(&format!("{}:helper", x.b.id))), "session=<project_id>:<agent>"),
        (format!("workspace={}", x.sb), "workspace=<session uuid>"),
        (format!("workspace={}", x.b.handle), "workspace=<name>"),
        (format!("workspace={}", x.b.id), "workspace=<id>"),
        (format!("workspace={}&agent=helper", x.b.handle), "workspace=<name>&agent="),
    ]
}

// ── agent passports ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_passport_reads_only_its_own_and_connected_workspaces_terminals() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let x = two_workspaces(&env, port).await;
    let pa = passport(&x.a);

    // Not connected: every form that names B is refused, as ?token= and
    // as a Bearer header (the CLI's way).
    for (q, form) in forms_naming_b(&x) {
        let r = read(port, &pa, &q).await;
        assert_refused(&r, Some("not_connected"), &format!("A's passport, {form}"));
        let r = read_bearer(port, &pa, &q).await;
        assert_refused(&r, Some("not_connected"), &format!("A's passport (Bearer), {form}"));
    }

    // Its own workspace, every way it can name it.
    for q in [
        format!("id={}", x.sa),
        format!("session={}", x.sa),
        format!("workspace={}", x.sa),
        format!("workspace={}", x.a.handle),
    ] {
        assert_reads(&read(port, &pa, &q).await, &format!("A's passport, own {q}"));
    }

    // A session K2 can't place in any workspace (cwd outside every one).
    let loose = env.path().join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    let s_loose = spawn_cat(port, "tab-trloose", &loose).await;
    let r = read(port, &pa, &format!("id={s_loose}")).await;
    assert_refused(&r, Some("forbidden"), "A's passport, an unplaced session");
    assert_reads(&read(port, OWNER, &format!("id={s_loose}")).await, "owner, an unplaced session");

    // Connected: A's passport reads B's sessions by every form.
    link(&x.a.id, &x.b.id);
    for (q, form) in forms_naming_b(&x) {
        assert_reads(&read(port, &pa, &q).await, &format!("A's passport, connected, {form}"));
    }
    assert_reads(&read(port, &pa, &format!("id={}", x.sb_helper)).await, "connected, helper uuid");
    drop(d);
    finish();
}

/// The same passport over its own cell socket gets the same answers.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_passport_on_its_cell_socket_is_held_to_the_same_rule() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let x = two_workspaces(&env, d.port).await;

    let sid = SessionId::new();
    let token = k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        k2_daemon::session_token::HookPrincipal { workspace_uuid: x.a.id.clone(), agent_address: "agent".into() },
        k2_daemon::session_token::CredMode::ApiKey,
        k2_daemon::session_token::Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);

    let uds = |q: String| {
        let (sock, token) = (sock.clone(), token.clone());
        async move {
            let mut s = tokio::net::UnixStream::connect(&sock).await.expect("connect cell socket");
            let req = format!("GET /cli/terminal/read?{q} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n");
            s.write_all(req.as_bytes()).await.unwrap();
            let mut out = Vec::new();
            let _ = s.read_to_end(&mut out).await;
            let text = String::from_utf8_lossy(&out).to_string();
            let status = text
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("no status line: {text:?}"));
            Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
        }
    };
    for (q, form) in forms_naming_b(&x) {
        assert_refused(&uds(q).await, Some("not_connected"), &format!("cell socket, {form}"));
    }
    assert_reads(&uds(format!("id={}", x.sa)).await, "cell socket, own session");
    // A repeated security parameter is refused here too.
    let r = uds(format!("id={}&id={}", x.sa, x.sb)).await;
    if r.status != 400 || r.body.contains("\"lines\"") {
        miss(format!("cell socket, a repeated id: want 400, got {} {}", r.status, r.body));
    }
    drop(d);
    finish();
}

// ── every other principal ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_principals_read_as_before_or_not_at_all() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let x = two_workspaces(&env, port).await;
    let forms = forms_naming_b(&x);

    // The owner token and every login role: unscoped, read as before.
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let logins = [
        ("owner token", OWNER.to_string()),
        ("Owner login", login(&format!("own{tag}"), Role::Owner)),
        ("Admin login", login(&format!("adm{tag}"), Role::Admin)),
        ("Member login", login(&format!("mem{tag}"), Role::Member)),
    ];
    for (who, tok) in &logins {
        for (q, form) in &forms {
            assert_reads(&read(port, tok, q).await, &format!("{who}, {form}"));
        }
    }

    // An App pass (a guest with rights in A's room): no terminal at all.
    let pass = guest(port, &format!("gst{tag}"), &x.a, &["thread:read", "files:read"]).await;
    for (q, form) in forms.iter().chain([(format!("id={}", x.sa), "own room's id=<uuid>")].iter()) {
        assert_refused(&read_bearer(port, &pass, q).await, None, &format!("App pass, {form}"));
        assert_refused(&read(port, &pass, q).await, None, &format!("App pass (?token=), {form}"));
    }

    // An API key, even one granted A: no terminal at all.
    let granted = format!("[\"{}\"]", x.a.id);
    let (_id, key) = k2_core::api_keys::create_api_key("trkey", None, Some(granted.as_str()), None, None, None)
        .expect("create api key");
    for (q, form) in forms.iter().chain([(format!("id={}", x.sa), "granted workspace id=<uuid>")].iter()) {
        assert_refused(&read(port, &key, q).await, None, &format!("API key, {form}"));
    }

    // A remote-session grant: its own shell only.
    let r = post(port, "/cli/remote-session/enable", OWNER, json!({})).await;
    assert_eq!(r.status, 200, "enable: {}", r.body);
    let r = post(port, "/cli/remote-session/grant", OWNER, json!({ "scope": "shell", "ttlSeconds": 1800, "label": "tr" })).await;
    assert_eq!(r.status, 200, "grant: {}", r.body);
    let grant = r.json()["token"].as_str().expect("grant token").to_string();
    let r = post(port, "/cli/remote-session/shell/spawn", &grant, json!({})).await;
    assert_eq!(r.status, 200, "shell spawn: {}", r.body);
    let shell = r.json()["sessionId"].as_str().expect("shell sessionId").to_string();
    assert_reads(&read(port, &grant, &format!("id={shell}")).await, "grant, its own shell (id=)");
    assert_reads(&read(port, &grant, &format!("session={shell}")).await, "grant, its own shell (session=)");
    for (q, form) in &forms {
        assert_refused(&read(port, &grant, q).await, Some("NO_GRANT"), &format!("grant, {form}"));
    }
    // A bound shell, read through another form by a login: refused, as id= was.
    let member = &logins[3].1;
    assert_refused(&read(port, member, &format!("session={shell}")).await, None, "Member login, a bound shell by session=");
    drop(d);
    finish();
}

/// An unknown grant reads nothing, by any form, with Remote Sessions off
/// or on, on loopback and on the tunnel listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_grant_reads_nothing() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let x = two_workspaces(&env, d.port).await;
    let unknown = "k2rs_unknown";
    let mut forms = forms_naming_b(&x);
    forms.push((format!("id={}", x.sa), "A's id=<uuid>"));
    for on in [false, true] {
        if on {
            let r = post(d.port, "/cli/remote-session/enable", OWNER, json!({})).await;
            assert_eq!(r.status, 200, "enable: {}", r.body);
        }
        let switch = if on { "Remote Sessions on" } else { "Remote Sessions off" };
        for (port, door) in [(d.port, "loopback"), (d.tunnel_port, "tunnel listener")] {
            for (q, form) in &forms {
                let r = read(port, unknown, q).await;
                if r.status == 200 || r.body.contains("\"lines\"") {
                    miss(format!("unknown grant, {switch}, {door}, {form}: read a terminal: {} {}", r.status, r.body));
                }
            }
        }
    }
    drop(d);
    finish();
}

// ── the grid and bytes sockets ───────────────────────────────────────

/// Frames from a grid socket until it closes or `secs` pass.
async fn grid_frames(url: &str, secs: u64) -> Vec<String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("grid WS connect");
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => out.push(t.to_string()),
            Ok(Some(Ok(Message::Binary(_)))) => out.push("<binary>".to_string()),
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => break,
            Ok(Some(Ok(_))) => {}
        }
    }
    out
}

fn has_event(frames: &[String], event: &str) -> bool {
    frames.iter().any(|f| serde_json::from_str::<Value>(f).is_ok_and(|v| v["event"] == event))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_socket_serves_only_the_session_its_token_opens() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let x = two_workspaces(&env, port).await;

    // Something on B's screen, to check what each socket serves.
    let marker = format!("TRMARK{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let r = post(port, "/cli/terminal/write", OWNER, json!({ "id": x.sb, "message": marker })).await;
    assert_eq!(r.status, 200, "write: {}", r.body);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = read(port, OWNER, &format!("id={}&lines=200", x.sb)).await;
        if r.body.contains(&marker) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "marker never reached B's screen: {}", r.body);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let sa = SessionId::parse(&x.sa).expect("uuid");
    let ta = k2_daemon::stream_token::mint(&sa);

    // Its own session: served.
    let own = grid_frames(&format!("ws://127.0.0.1:{port}/cli/sessions/grid?session={}&token={ta}", x.sa), 3).await;
    assert!(has_event(&own, "snapshot"), "own session's grid: {own:?}");

    // A's token with B named as well (a repeated security parameter):
    // refused before the upgrade, nothing of B.
    match tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={}&token={ta}&session={}",
        x.sa, x.sb
    ))
    .await
    {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            if resp.status().as_u16() != 400 {
                miss(format!("grid: refused with {} (want 400)", resp.status()));
            }
        }
        Err(e) => miss(format!("grid: refused with an unexpected error: {e}")),
        Ok((mut ws, _)) => {
            let mut frames = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, ws.next()).await {
                if let Message::Text(t) = m {
                    frames.push(t.to_string());
                }
            }
            let has_b = frames.iter().any(|f| f.contains(&marker));
            let snapshot = has_event(&frames, "snapshot");
            miss(format!("grid: A's token opened a socket with B named (snapshot: {snapshot}, B's screen in it: {has_b})"));
        }
    }

    // The bytes socket serves session-stream sessions: two of them, with
    // something in the second one's byte ring.
    let (ra, rb) = (SessionId::new(), SessionId::new());
    k2_core::session::registry::register(ra);
    let rb_entry = k2_core::session::registry::register(rb);
    rb_entry.publish_bytes(std::sync::Arc::from(format!("{marker}\r\n").into_bytes()));
    let tra = k2_daemon::stream_token::mint(&ra);
    let own_bytes =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/cli/sessions/bytes?session={ra}&token={tra}")).await;
    if let Err(e) = own_bytes {
        miss(format!("bytes: its own session must be served: {e}"));
    }
    let bytes = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/cli/sessions/bytes?session={ra}&token={tra}&session={rb}"
    ))
    .await;
    match bytes {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            if resp.status().as_u16() != 400 {
                miss(format!("bytes: refused with {} (want 400)", resp.status()));
            }
        }
        Err(e) => miss(format!("bytes: refused with an unexpected error: {e}")),
        Ok((mut ws, _)) => {
            let mut seen = Vec::<u8>::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, ws.next()).await {
                if let Message::Binary(b) = m {
                    seen.extend_from_slice(&b);
                }
            }
            let has_b = String::from_utf8_lossy(&seen).contains(&marker);
            miss(format!("bytes: A's token opened B's session ({} bytes, B's screen in it: {has_b})", seen.len()));
        }
    }
    drop(d);
    finish();
}

// ── the lines= cap ───────────────────────────────────────────────────

fn lines_of(r: &Resp) -> Vec<String> {
    assert_eq!(r.status, 200, "read: {}", r.body);
    r.json()["lines"]
        .as_array()
        .unwrap_or_else(|| panic!("lines array: {}", r.body))
        .iter()
        .map(|l| l.as_str().unwrap_or_else(|| panic!("a line is text: {}", r.body)).to_string())
        .collect()
}

fn assert_not_capped(r: &Resp, what: &str) {
    let v = r.json();
    assert!(v.get("capped").is_none() && v.get("note").is_none(), "{what}: no cap note at or under the cap: {}", r.body);
}

fn assert_capped(r: &Resp, asked: &str, max: usize, what: &str) {
    let v = r.json();
    assert_eq!(v["capped"]["max"], json!(max), "{what}: capped.max: {}", r.body);
    assert_eq!(v["capped"]["asked"].to_string(), asked, "{what}: capped.asked: {}", r.body);
    let note = v["note"].as_str().unwrap_or_else(|| panic!("{what}: a note: {}", r.body));
    assert!(note.contains(&format!("capped at {max}")), "{what}: the note names the cap: {note}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_is_capped_at_the_scrollback_and_the_replay_bound() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let a = seed_ws(&env, &format!("trcap{tag}"));

    // A screen with more output than its 5,000-row scrollback holds.
    let body = json!({
        "agent_name": "tab-trcap",
        "cwd": a.path.to_string_lossy(),
        "command": "/bin/sh",
        "args": ["-c", "seq 1 6000; exec cat"],
        "cols": 80,
        "rows": 24,
    });
    let r = post(port, "/cli/sessions/v2/spawn", OWNER, body).await;
    assert_eq!(r.status, 200, "spawn: {}", r.body);
    let s = r.json()["sessionId"].as_str().expect("sessionId").to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = read(port, OWNER, &format!("id={s}&lines=3")).await;
        if lines_of(&r).iter().any(|l| l == "6000") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "seq never finished: {}", r.body);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // At the cap: 5,000 lines, no note.
    let at = read(port, OWNER, &format!("id={s}&lines=5000")).await;
    let at_lines = lines_of(&at);
    assert_eq!(at_lines.len(), 5000, "at the cap");
    assert_not_capped(&at, "screen at the cap");
    assert_eq!(at_lines.last().map(String::as_str), Some("6000"), "the last line is the newest");
    // Under it: as asked.
    let under = read(port, OWNER, &format!("id={s}&lines=10")).await;
    assert_eq!(lines_of(&under).len(), 10, "under the cap");
    assert_not_capped(&under, "screen under the cap");
    // Over it: clamped to the same 5,000, with the note.
    let over = read(port, OWNER, &format!("id={s}&lines=9000")).await;
    assert_eq!(lines_of(&over), at_lines, "over the cap = the last 5,000");
    assert_capped(&over, "9000", 5000, "screen over the cap");
    // A number too large for a usize is the same clamp, not the default 50.
    let huge = read(port, OWNER, &format!("id={s}&lines=99999999999999999999999")).await;
    assert_eq!(lines_of(&huge).len(), 5000, "a huge number");
    assert_eq!(huge.json()["capped"]["max"], json!(5000), "a huge number: {}", huge.body);

    // An older sub-terminal's replay ring: 12,000 frames, capped at 10,000.
    let rid = SessionId::new();
    let entry = k2_core::session::registry::register(rid);
    for i in 0..12_000 {
        entry.publish(k2_core::session::Frame::Text { bytes: format!("row {i}").into_bytes(), style: None });
    }
    let at = read(port, OWNER, &format!("id={rid}&lines=10000")).await;
    let at_rows = lines_of(&at);
    assert_eq!(at_rows.len(), 10_000, "replay at the cap");
    assert_eq!(at_rows.first().map(String::as_str), Some("row 2000"), "replay: the last 10,000 frames");
    assert_eq!(at_rows.last().map(String::as_str), Some("row 11999"), "replay: newest last");
    assert_not_capped(&at, "replay at the cap");
    let over = read(port, OWNER, &format!("id={rid}&lines=20000")).await;
    assert_eq!(lines_of(&over), at_rows, "replay over the cap = the last 10,000");
    assert_capped(&over, "20000", 10_000, "replay over the cap");
    let under = read(port, OWNER, &format!("id={rid}&lines=50")).await;
    let under_rows = lines_of(&under);
    assert_eq!(under_rows.len(), 50, "replay under the cap");
    assert_eq!(under_rows.last().map(String::as_str), Some("row 11999"), "replay under the cap: newest last");
    drop(d);
}

// ── a repeated security parameter ────────────────────────────────────

/// One raw request: `method pq` with no credential but what `pq` carries.
async fn raw_req(port: u16, method: &'static str, pq: String) -> Resp {
    let body = (method == "POST").then(String::new);
    tokio::task::spawn_blocking(move || req_blocking(port, method, &pq, body)).await.unwrap()
}

/// A request that names token, session, id, workspace, agent or pass more
/// than once is refused (400 usage) on every door, before any gate, and
/// reads or writes nothing; the same request with each named once is
/// answered as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_security_parameters_are_refused() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let x = two_workspaces(&env, d.port).await;
    let marker = format!("TRWRITE{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let pairs = [("k2rs_unknown", "k2rs_unknown"), ("a", "b")];
    for (port, door) in [(d.port, "loopback"), (d.tunnel_port, "tunnel listener")] {
        for (a, b) in pairs {
            let what = format!("{door}, token {a:?} then {b:?}");
            let r = raw_req(port, "GET", format!("/cli/terminal/read?token={a}&token={b}&id={}", x.sb)).await;
            if r.status == 200 || r.body.contains("\"lines\"") {
                miss(format!("read, {what}: answered {} {}", r.status, r.body));
            }
            let r = raw_req(
                port,
                "POST",
                format!("/cli/terminal/write?token={a}&token={b}&id={}&message={marker}&no_submit=true", x.sb),
            )
            .await;
            if r.status == 200 || r.body.contains("\"success\":true") {
                miss(format!("write, {what}: answered {} {}", r.status, r.body));
            }
        }
    }
    // Nothing reached B's terminal.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let screen = read(d.port, OWNER, &format!("id={}&lines=200", x.sb)).await;
    assert_eq!(screen.status, 200, "owner read: {}", screen.body);
    if screen.body.contains(&marker) {
        miss(format!("B's terminal received text from a request without one credential: {}", screen.body));
    }
    // Each security key, repeated next to the owner token: 400 usage.
    for (key, a, b) in [
        ("token", OWNER, OWNER),
        ("session", x.sb.as_str(), x.sa.as_str()),
        ("id", x.sa.as_str(), x.sb.as_str()),
        ("workspace", x.a.handle.as_str(), x.b.handle.as_str()),
        ("agent", "helper", "other"),
        ("pass", "a", "b"),
    ] {
        let pq = if key == "token" {
            format!("/cli/terminal/read?token={a}&token={b}&id={}", x.sa)
        } else {
            format!("/cli/terminal/read?token={OWNER}&{key}={a}&{key}={b}")
        };
        let r = raw_req(d.port, "GET", pq).await;
        let code = serde_json::from_str::<Value>(&r.body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string));
        if r.status != 400 || code.as_deref() != Some("usage") {
            miss(format!("repeated {key}: want 400 usage, got {} {}", r.status, r.body));
        }
    }
    // Named once, the owner's read and write answer as before.
    let r = raw_req(d.port, "GET", format!("/cli/terminal/read?token={OWNER}&id={}", x.sa)).await;
    assert_reads(&r, "owner, each key once");
    let r = raw_req(d.port, "POST", format!("/cli/terminal/write?token={OWNER}&id={}&message=once&no_submit=true", x.sa)).await;
    if r.status != 200 {
        miss(format!("owner write, each key once: {} {}", r.status, r.body));
    }
    drop(d);
    finish();
}
