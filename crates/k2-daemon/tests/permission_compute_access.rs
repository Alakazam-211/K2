//! An agent's own compute access as a permission card (0.46.9, Rosson
//! 2026-11-05): "make it something they can request if they have
//! permission requesting enabled."
//!
//! On a real in-process daemon, an agent passport on the TCP port:
//! - **off** (the server's "Permission cards (Beta)" off, then on with the
//!   agent's own switch off): `k2 compute access on` and a compute verb are
//!   refused with today's bytes, no `permission` object, and an ask raises
//!   no card;
//! - **both on**: the same refusals end with `Ask with: k2 permission
//!   request compute.use …` (only for its own workspace, only `on`, never
//!   `compute_manage`); the printed command (what the CLI sends for it) is
//!   accepted; an Admin's Allow turns "Allow compute nodes" on for that
//!   workspace only, never "Allow managing compute nodes" and never a
//!   second workspace's; the answer is in Access Audit; the agent's
//!   compute verbs then work. A request can't name a workspace or another
//!   value, and a second ask is `already_done`.
//!
//! Its own binary: it flips the process-wide switch. No vault, keychain,
//! real `~/.k2` or running daemon is touched.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};

const OWNER: &str = "owner-token-permission-compute-access";
const ASK: &str = "k2 permission request compute.use --why \"<what you need it for>\"";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
}

fn parse(text: &str) -> Resp {
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    parse(&String::from_utf8_lossy(&out))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn call(port: u16, method: &str, path: &str, token: &str, body: Option<Value>) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    let pq = format!("{path}{sep}token={}", enc(token));
    let method = method.to_string();
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    call(port, "POST", path, token, Some(body)).await
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Ws {
    id: String,
    handle: String,
}

fn workspace(home: &std::path::Path, handle: &str) -> Ws {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).unwrap();
    let path = path.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path]).unwrap();
    // A pinned chat: the card goes on its Thread.
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").unwrap();
    Ws { id, handle: handle.to_string() }
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pca-int-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

fn passport(ws: &Ws, pane: &str) -> String {
    k2_daemon::session_token::mint_session_token(
        &SessionId::new(),
        pane,
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.handle.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// "Allow compute nodes" for this workspace.
fn compute_on(ws: &Ws) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::store::agents_can_use_compute(&conn, &ws.id)
}

/// "Allow managing compute nodes" for this workspace.
fn manage_on(ws: &Ws) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let v: i64 = conn
        .query_row("SELECT agents_can_manage_compute FROM projects WHERE id = ?1", [&ws.id], |r| r.get(0))
        .expect("the workspace row");
    v == 1
}

/// Every permission request this workspace ever made.
fn asks(ws: &Ws) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1", [&ws.id], |r| r.get(0))
        .expect("count permission requests")
}

/// What `k2 compute access on` sends from a K2 terminal (no workspace:
/// the passport's own).
fn access_on() -> Value {
    json!({ "toggle": "compute", "value": true })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_asks_for_its_own_compute_access_and_only_that_turns_on() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pca{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let admin = login("pcaadmin", Role::Admin);
    let member = login("pcamember", Role::Member);
    let a = workspace(&home, "pca-gardener");
    let b = workspace(&home, "pca-billing");
    let pa = passport(&a, "pane-pca-a");
    assert!(!compute_on(&a) && !compute_on(&b) && !manage_on(&a) && !manage_on(&b), "everything starts off");

    // ── off: the server's switch off, then on with the agent's off ──────
    for (state, server) in [("server off", false), ("server on, agent off", true)] {
        let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": server })).await;
        assert_eq!(r.status, 200, "{state}: {}", r.body);
        let r = post(port, "/cli/agent-access/set", &pa, access_on()).await;
        assert_eq!(r.status, 403, "{state}: {}", r.body);
        assert_eq!(
            r.body, r#"{"error":{"code":"forbidden","hint":"only the owner or an admin login may change agent access"}}"#,
            "{state}: today's bytes"
        );
        let r = call(port, "GET", "/cli/compute/nodes", &pa, None).await;
        assert_eq!((r.status, r.json()["error"]["code"].as_str()), (403, Some("compute_off")), "{state}: {}", r.body);
        assert!(r.json().get("permission").is_none(), "{state}: taught while off: {}", r.body);
        // Asking anyway raises no card.
        let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.use", "params": {}, "why": "Run the suite" })).await;
        assert!(matches!(r.status, 401 | 403), "{state}: an ask while off is refused: {} {}", r.status, r.body);
        assert!(!r.body.contains("\"card\""), "{state}: no card: {}", r.body);
        assert_eq!(asks(&a), 0, "{state}: no request was stored");
        assert!(!compute_on(&a), "{state}");
    }

    // ── both on ─────────────────────────────────────────────────────────
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": a.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // `k2 compute access on`, from a K2 terminal (no workspace) or naming
    // its own: the refusal is unchanged and ends with the ask.
    for body in [access_on(), json!({ "toggle": "compute", "value": true, "workspace": a.handle })] {
        let r = post(port, "/cli/agent-access/set", &pa, body.clone()).await;
        assert_eq!(r.status, 403, "{body}: {}", r.body);
        let v = r.json();
        assert_eq!(v["error"]["code"], "forbidden", "{body}: the refusal itself is unchanged: {v}");
        assert_eq!(
            v["permission"],
            json!({ "action": "compute.use", "params": {}, "approvers": "owner or admin", "command": ASK }),
            "{body}: {v}"
        );
    }
    // A compute verb (what `k2 compute run` meets) asks for the same action.
    let r = call(port, "GET", "/cli/compute/nodes", &pa, None).await;
    assert_eq!(r.json()["permission"]["command"], ASK, "{}", r.body);
    // Another workspace's switch, off, and "Allow managing compute nodes":
    // not requestable.
    for body in [
        json!({ "toggle": "compute", "value": true, "workspace": b.handle }),
        json!({ "toggle": "compute", "value": false }),
        json!({ "toggle": "compute_manage", "value": true }),
        json!({ "toggle": "compute_manage", "value": true, "workspace": a.handle }),
    ] {
        let r = post(port, "/cli/agent-access/set", &pa, body.clone()).await;
        assert_eq!(r.status, 403, "{body}: {}", r.body);
        let v = r.json();
        assert_eq!(v["permission"]["requestable"], false, "{body}: {v}");
        assert!(v["permission"].get("command").is_none(), "{body}: {v}");
    }

    // A request can't name a workspace or any other value.
    for params in [json!({ "workspace": b.id }), json!({ "workspace": a.id }), json!({ "node": "k2n1-m4" })] {
        let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.use", "params": params, "why": "Run the suite" })).await;
        assert_eq!(r.status, 400, "{params}: {}", r.body);
    }
    assert_eq!(asks(&a), 0, "no request was stored for a refused ask");

    // The card's words, plainly (a dry run shows them, stores nothing).
    let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.use", "params": {}, "why": "Run the suite", "dryRun": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert!(
        v["text"].as_str().is_some_and(|t| t.starts_with("Let pca-gardener run jobs on this server's compute nodes")),
        "{v}"
    );
    assert!(
        v["consequences"].as_str().is_some_and(|t| t.contains("every machine switched on here")),
        "the card says it covers every machine: {v}"
    );
    assert_eq!(v["setting"], "Allow compute nodes", "{v}");
    assert_eq!(v["approvers"], "owner or admin", "{v}");
    assert!(!v.to_string().contains('\u{2014}'), "no em dashes on the card: {v}");

    // The printed command, as the CLI sends it.
    let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.use", "params": {}, "why": "Run the suite on the minis" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "pending", "{v}");
    let card = v["card"].as_str().unwrap_or_else(|| panic!("no card: {v}")).to_string();
    let request_id = v["id"].as_str().unwrap_or_else(|| panic!("no id: {v}")).to_string();
    let answer = |who: String| {
        let body = json!({ "addr": a.handle, "id": card, "answer": "Allow" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    // A Member can't allow it; the agent itself can't either.
    let r = answer(member.clone()).await;
    assert_eq!((r.status, r.json()["error"]["code"].as_str()), (403, Some("action_card_person_only")), "{}", r.body);
    let r = answer(pa.clone()).await;
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    assert!(!compute_on(&a), "a refused answer applies nothing");

    // An Admin allows it: that workspace's switch only.
    let r = answer(admin.clone()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);
    assert!(compute_on(&a), "Allow compute nodes is on for the asking workspace");
    assert!(!compute_on(&b), "a second workspace is unaffected");
    assert!(!manage_on(&a) && !manage_on(&b), "Allow managing compute nodes is never turned on by this card");

    // Audited like every card: the answer, by whom, and the switch itself.
    let lines = k2_core::auth_audit::tail(500).expect("read the audit");
    assert!(
        lines.iter().any(|l| l["event"] == "permission-answer"
            && l["user"] == "login:pcaadmin"
            && l["outcome"].as_str().is_some_and(|o| o.contains(&request_id) && o.contains("compute.use approved"))),
        "no permission-answer line for {request_id}: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l["event"] == "agent-access-set" && l["outcome"].as_str().is_some_and(|o| o.contains("compute=true"))),
        "no agent-access-set line: {lines:?}"
    );

    // The agent's compute verbs work now; a second ask has nothing to ask.
    let r = call(port, "GET", "/cli/compute/nodes", &pa, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/permission/request", &pa, json!({ "action": "compute.use", "params": {}, "why": "again" })).await;
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("already_done")), "{}", r.body);

    drop(d);
}
