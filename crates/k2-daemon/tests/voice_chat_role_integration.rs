//! Voice & Chat only logins end to end (prd-voice-chat-only-members-v1 and
//! the store-reviewer extension), through the REAL dispatcher
//! (`test_harness::start`) and the real login store:
//!
//! - the owner makes the reviewer login with its shares in one add;
//! - it signs in with a password (no forced change), whoami says
//!   `voice_chat` / `voiceChatOnly`;
//! - it lists its shares, reads and posts in the ONE shared agent's Thread
//!   (a compose post as itself), and views the shared Garden through the
//!   phone's remote Garden door;
//! - it is refused everywhere else: terminal spawn, grid and session
//!   sockets, `/events`, `/status`, the vault, agents, other Threads, users;
//! - a live role change applies with no sign-out, and a Member socket closes
//!   when its login becomes Voice & Chat only;
//! - an Admin moves people only between Member and Voice & Chat only; a
//!   phone can't be paired for the role.
//!
//! Isolation: a temp `$HOME` per test (one at a time, `TEST_LOCK`), the
//! agent shim, the process's test DB. Every check fails loudly.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value as J};

const OWNER_TOKEN: &str = "owner-token-voice-chat-role-integration";
const PW: &str = "review-pass-Strong-1";

static TEST_LOCK: Mutex<()> = Mutex::new(());

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
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
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
        Err(e) => panic!("read: {e:?}"),
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

fn get(port: u16, path: &str, token: &str) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    http(port, "GET", &format!("{path}{sep}token={token}"), None)
}

fn post(port: u16, path: &str, token: &str, body: J) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    http(port, "POST", &format!("{path}{sep}token={token}"), Some(&body.to_string()))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

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
    let tmp = std::env::temp_dir().join(format!("k2-voice-chat-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    k2_core::test_env::write_executable(&shim_dir.join("claude"), "#!/bin/sh\nexec cat\n");
    let _restore = Restore {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    f();
}

/// A workspace with a pinned Chat: `(project id, handle)`.
fn agent(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/vc-role-{handle}-{id}")],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    (id, handle)
}

fn login(port: u16, user: &str, password: &str) -> J {
    let r = http(port, "POST", "/cli/auth/login", Some(&json!({"username": user, "password": password}).to_string()));
    assert_eq!(r.status, 200, "login {user}: {}", r.body);
    r.json()
}

/// 403 role_required for a voice_chat login.
fn assert_role_required(r: &Resp, what: &str) {
    assert_eq!(r.status, 403, "{what}: {}", r.body);
    let v = r.json();
    assert_eq!(v["error"], "role_required", "{what}: {v}");
    assert_eq!(v["role"], "voice_chat", "{what}: {v}");
    assert!(!r.body.contains("invalid or missing token"), "{what}: never the re-login text: {}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reviewer_login_reaches_voice_one_agent_and_one_garden_and_nothing_else() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let (a_id, a) = agent("vcdemo");
        let (_b_id, b) = agent("vcother");
        let zen = k2_core::zen::ZenFiles::local();
        zen.setup().expect("zen setup");
        let garden = zen.gardens()[0].id.clone();

        // ── the owner makes the reviewer login, shares in the same add ──
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({
            "username": "reviewer", "password": PW, "mustChangePassword": false,
            "role": "voice_chat", "shareAgents": [a], "shareGardens": [garden],
        }));
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!(v["role"], "voice_chat", "{v}");
        assert_eq!(v["shares"]["agents"][0]["address"], a.as_str(), "{v}");
        assert_eq!(v["shares"]["gardens"][0]["id"], garden.as_str(), "{v}");
        // Unknown share: 404, no login made.
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "rev2", "password": PW, "role": "voice_chat", "shareAgents": ["no-such-agent"]}));
        assert_eq!(r.status, 404, "{}", r.body);
        assert_eq!(k2_core::connect_users::role_for_user("rev2"), None);

        // ── password sign-in, no forced change; whoami ──
        let l = login(port, "reviewer", PW);
        assert_eq!(l["mustChangePassword"], false, "{l}");
        let tok = l["token"].as_str().expect("token").to_string();
        let w = get(port, "/cli/auth/whoami", &tok).json();
        assert_eq!((w["role"].as_str(), w["voiceChatOnly"].as_bool(), w["canClone"].as_bool()), (Some("voice_chat"), Some(true), Some(false)), "{w}");
        let ow = get(port, "/cli/auth/whoami", OWNER_TOKEN).json();
        assert_eq!(ow["voiceChatOnly"], false, "{ow}");

        // ── shares ──
        let s = get(port, "/cli/shared", &tok);
        assert_eq!(s.status, 200, "{}", s.body);
        let s = s.json();
        assert_eq!(s["agents"].as_array().map(Vec::len), Some(1), "{s}");
        assert_eq!(s["agents"][0]["id"], a_id.as_str());
        assert_eq!(s["gardens"][0]["id"], garden.as_str());
        let r = get(port, "/cli/shared", OWNER_TOKEN);
        assert_eq!((r.status, r.json()["error"]["code"].as_str().map(str::to_string)), (403, Some("voice_chat_only".into())));

        // Post to the shared agent: a compose post as the reviewer.
        let text = format!("hi from review {}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
        let r = post(port, "/cli/shared/thread/post", &tok, json!({"agent": a, "text": text}));
        assert_eq!(r.status, 200, "{}", r.body);
        let p = r.json();
        assert_eq!(p["via"], "compose", "{p}");
        assert!(p["from"].as_str().is_some_and(|f| f.contains("reviewer")), "stamped as the person, never the owner: {p}");
        let r = get(port, &format!("/cli/shared/thread?agent={a}"), &tok);
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains(&text), "the post reads back: {}", r.body);
        // Another agent, via=thread, the generic Thread routes: refused.
        let r = post(port, "/cli/shared/thread/post", &tok, json!({"agent": b, "text": "x"}));
        assert_eq!((r.status, r.json()["error"]["code"].clone()), (403, json!("not_shared")));
        let r = get(port, &format!("/cli/shared/thread?agent={b}"), &tok);
        assert_eq!((r.status, r.json()["error"]["code"].clone()), (403, json!("not_shared")));
        let r = post(port, "/cli/shared/thread/post", &tok, json!({"agent": a, "text": "x", "via": "thread"}));
        assert_eq!((r.status, r.json()["error"]["code"].clone()), (403, json!("compose_only")));
        assert_role_required(&get(port, &format!("/cli/thread?addr={a}"), &tok), "GET /cli/thread");
        assert_role_required(&post(port, "/cli/thread/post", &tok, json!({"addr": a, "text": "x", "via": "compose"})), "POST /cli/thread/post");

        // ── the shared Garden, read-only, on the phone's door ──
        let r = get(port, "/cli/zen/remote/gardens", &tok);
        assert_eq!(r.status, 200, "{}", r.body);
        let g = r.json();
        assert_eq!((g["person"].as_str(), g["readOnly"].as_bool()), (Some("shared"), Some(true)), "{g}");
        assert_eq!(g["gardens"].as_array().map(Vec::len), Some(1), "{g}");
        let r = get(port, &format!("/cli/zen/remote/page?garden={garden}"), &tok);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_role_required(&get(port, &format!("/cli/zen/remote/agents-order?garden={garden}"), &tok), "agents-order");
        assert_role_required(&get(port, "/cli/zen/get", &tok), "zen/get");

        // ── voice: the gate lets it in (the preview is off here) ──
        let r = get(port, "/cli/voice/rooms", &tok);
        assert_eq!(r.status, 404, "{}", r.body);
        assert_eq!(r.json()["error"]["code"], "voice_rooms_off", "past the role gate: {}", r.body);
        assert_role_required(&post(port, "/cli/voice/moderate", &tok, json!({})), "voice/moderate");
        // V3: the presence summary is empty and reads nothing.
        let r = get(port, "/cli/presence/summary", &tok);
        assert_eq!((r.status, r.body.as_str()), (200, r#"{"workspaces":[],"agentActivity":[],"activeProjectIds":[]}"#));

        // ── refused everywhere else ──
        for (m, path) in [
            ("POST", "/cli/terminal/spawn"),
            ("POST", "/cli/sessions/v2/spawn"),
            ("GET", "/cli/sessions/grid"),
            ("GET", "/cli/sessions/events"),
            ("GET", "/cli/projects/list"),
            ("GET", "/cli/agent/list"),
            ("POST", "/cli/agents/create"),
            ("GET", "/cli/vault/list"),
            ("GET", "/cli/settings/get"),
            ("GET", "/cli/users"),
            ("POST", "/cli/users/add"),
            ("POST", "/cli/users/set-shares"),
            ("GET", "/cli/fs/read-dir"),
            ("GET", "/cli/mail/status"),
            ("GET", "/cli/db/list"),
            ("POST", "/cli/sew/pass"),
            ("GET", "/cli/compute/nodes"),
            ("GET", "/cli/app/list"),
        ] {
            let r = if m == "POST" { post(port, path, &tok, json!({})) } else { get(port, path, &tok) };
            if r.status == 404 && r.body.contains("route_unclassified") {
                continue;
            }
            assert_role_required(&r, &format!("{m} {path}"));
        }
        assert_role_required(&get(port, "/events", &tok), "/events");
        assert_role_required(&get(port, "/status", &tok), "/status");

        // A phone pairing for the role is refused, the owner's included.
        let r = post(port, "/cli/auth/device/pair-code", OWNER_TOKEN, json!({"user": "reviewer"}));
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.json()["error"]["code"], "voice_chat_no_phone");

        // ── live: no sign-out either way ──
        let r = post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "reviewer", "role": "member"}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["live"], true);
        assert_eq!(get(port, "/cli/projects/list", &tok).status, 200, "a Member now, same token");
        assert_eq!(get(port, "/cli/shared", &tok).status, 403, "shares act only for voice_chat");
        let r = post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "reviewer", "role": "voice_chat"}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_role_required(&get(port, "/cli/projects/list", &tok), "after the downgrade");
        assert_eq!(get(port, "/cli/auth/whoami", &tok).status, 200, "still signed in");

        // ── an Admin: Member <-> Voice & Chat only, nothing more ──
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "adm", "password": PW, "mustChangePassword": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "adm", "role": "admin"})).status, 200);
        let adm = login(port, "adm", PW)["token"].as_str().expect("token").to_string();
        let r = post(port, "/cli/users/set-role", &adm, json!({"username": "reviewer", "role": "member"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = post(port, "/cli/users/set-role", &adm, json!({"username": "reviewer", "role": "admin"}));
        assert_eq!((r.status, r.json()["error"].clone()), (403, json!("role_change_not_allowed")), "an Admin never makes an Admin");
        let r = post(port, "/cli/users/set-role", &adm, json!({"username": "reviewer", "role": "voice_chat"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = post(port, "/cli/users/set-shares", &adm, json!({"username": "reviewer", "agents": [a], "gardens": []}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["shares"]["gardens"], json!([]));
        let r = post(port, "/cli/users/set-shares", &adm, json!({"username": "adm", "agents": [a]}));
        assert_eq!(r.status, 400, "an Admin's own row is not voice_chat: {}", r.body);
        // The list shows the shares.
        let users = get(port, "/cli/users", OWNER_TOKEN).json();
        let row = users["users"].as_array().expect("users").iter().find(|u| u["username"] == "reviewer").cloned().expect("row");
        assert_eq!(row["role"], "voice_chat");
        assert_eq!(row["shares"]["agents"][0]["address"], a.as_str(), "{row}");

        // Every role change wrote one audit line {from, to}.
        let audit = std::fs::read_to_string(std::path::Path::new(&std::env::var("HOME").expect("home")).join(".k2/auth-audit.jsonl"))
            .expect("audit log");
        assert!(audit.contains("\"set-role\"") && audit.contains("member->voice_chat"), "{audit}");
    });
}

/// A Member's open socket closes within a tick of the login becoming Voice &
/// Chat only (§7), without a sign-out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_socket_closes_when_the_login_becomes_voice_chat_only() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;
        let r = post(port, "/cli/users/add", OWNER_TOKEN, json!({"username": "mover", "password": PW, "mustChangePassword": false}));
        assert_eq!(r.status, 200, "{}", r.body);
        let tok = login(port, "mover", PW)["token"].as_str().expect("token").to_string();
        futures_block(async {
            let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={tok}");
            let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("a Member opens the app socket");
            let r = post(port, "/cli/users/set-role", OWNER_TOKEN, json!({"username": "mover", "role": "voice_chat"}));
            assert_eq!(r.status, 200, "{}", r.body);
            let closed = tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    match ws.next().await {
                        None | Some(Err(_)) => return,
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => return,
                        Some(Ok(_)) => continue,
                    }
                }
            })
            .await;
            assert!(closed.is_ok(), "the socket must close within a tick of the downgrade");
            // A new upgrade is refused at the door with role_required.
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let req = format!("ws://127.0.0.1:{port}/cli/sessions/events?token={tok}").into_client_request().expect("req");
            match tokio_tungstenite::connect_async(req).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 403),
                other => panic!("a voice_chat login must not open the app socket: {:?}", other.map(|_| "opened")),
            }
        });
        assert_eq!(get(port, "/cli/auth/whoami", &tok).status, 200, "no sign-out");
    });
}

/// Follow-up 1: `/boot-status` off this box (the tunnel ingress, as a phone
/// reaches it) with a Voice & Chat only session is the FULL view, like any
/// live login, so the phone's stale-session check (`view != "full"` after
/// sending a token) never fires; its `features` is the one list every view
/// carries. With no token the default compat view carries the same list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_status_gives_a_voice_chat_session_the_full_view_and_every_feature() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = post(d.port, "/cli/users/add", OWNER_TOKEN, json!({"username": "bsrev", "password": PW, "mustChangePassword": false, "role": "voice_chat"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let tok = login(d.port, "bsrev", PW)["token"].as_str().expect("token").to_string();
        let full = http(d.tunnel_port, "GET", &format!("/boot-status?token={tok}"), None);
        assert_eq!(full.status, 200, "{}", full.body);
        let full = full.json();
        assert_eq!(full["view"], "full", "{full}");
        let open = http(d.tunnel_port, "GET", "/boot-status", None).json();
        assert_ne!(open["view"], "full", "no token off-box is not the full view: {open}");
        for v in [&full, &open] {
            let feats: Vec<&str> = v["features"].as_array().expect("features").iter().filter_map(|f| f.as_str()).collect();
            for want in ["voice-chat-role-v1", "gardens-remote-v1"] {
                assert!(feats.contains(&want), "{want} in {feats:?}");
            }
        }
        assert_eq!(full["features"], open["features"], "one list for every view");
    });
}
