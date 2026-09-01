//! Skin slice 1+2 — guest list + `k2skn_` passes + Thread rooms + grid 403.
//!
//! Driven through the real dispatcher (`k2_daemon::test_harness::start`).
//! Loud contracts (prd-skin-auth-v1 §8):
//!   1. Owner-tier mint (`owner_role_identity`); Admin/Member 403.
//!   2. Mint `thread:read` → GET `/cli/thread` 200; grid + bytes 403
//!      `{"error":"skin tokens cannot use the terminal"}`.
//!   3. Token without `thread:post` → POST `/cli/thread/post` 403.
//!   4. Revoke → next GET 401.
//!   5. Skin tokens are NOT `token_ok` (settings-class GET 403).
//!   6. POST-only mutations: GET twins 405.
//!   7. Nested label `skin` is reserved (400 loud).
//!   8. Overlay WS: thread:read accepted; chatterlog frames filtered.
//!
//! ISOLATION: `$HOME` is process-wide — every test serializes on `TEST_LOCK`
//! and redirects HOME to a fresh tempdir (`~/.k2/skin.db`).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay::OverlayDoc;
use k2_core::session::SessionId;
use k2_daemon::overlay_ws::{self, OverlayFrame};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-skin-slice12";
const TERMINAL_403: &str = r#"{"error":"skin tokens cannot use the terminal"}"#;

struct Resp {
    status: u16,
    body: String,
    headers: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    http_host(port, method, path_and_query, body, "127.0.0.1")
}

fn http_host(
    port: u16,
    method: &str,
    path_and_query: &str,
    body: Option<&str>,
    host: &str,
) -> Resp {
    http_host_ex(port, method, path_and_query, body, host, "")
}

fn http_host_ex(
    port: u16,
    method: &str,
    path_and_query: &str,
    body: Option<&str>,
    host: &str,
    extra_headers: &str,
) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    let extra = if extra_headers.is_empty() {
        String::new()
    } else if extra_headers.ends_with("\r\n") {
        extra_headers.to_string()
    } else {
        format!("{extra_headers}\r\n")
    };
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp {
                    status,
                    body,
                    headers: headers_of(&raw),
                };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if let Some((status, body, _)) = try_parse(&raw) {
                    return Resp {
                        status,
                        body,
                        headers: headers_of(&raw),
                    };
                }
                panic!("read timeout: {e:?} raw={}", String::from_utf8_lossy(&raw));
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp {
        status,
        body,
        headers: headers_of(&raw),
    }
}

fn headers_of(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.split("\r\n\r\n").next().unwrap_or("").to_string()
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    for line in headers.lines() {
        let Some(colon) = line.find(':') else {
            continue;
        };
        if line[..colon].eq_ignore_ascii_case(name) {
            return Some(line[colon + 1..].trim().to_string());
        }
    }
    None
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-skin-it-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn login(port: u16, username: &str, password: &str) -> String {
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "login must succeed; body={}", r.body);
    json(&r.body)["token"]
        .as_str()
        .expect("login token")
        .to_string()
}

fn provision_role(port: u16, username: &str, password: &str, role: &str) -> String {
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "users/add({username}); {}", r.body);
    if role != "member" {
        let r = http(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role; {}", r.body);
    }
    login(port, username, password)
}

fn seed_thread_addr(handle: &str) -> (String, String) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/skin-it-{handle}-{id}");
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(
        &conn,
        &format!("ws-{conv}"),
        &id,
        None,
        Some(&conv),
        "claude",
        "system",
        "running",
    )
    .expect("pin");
    (id, conv)
}

fn seed_sidecar(project_id: &str, conv: &str, slug: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::allocate_ordinal(&conn, project_id, conv).expect("ordinal");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('claude', ?1, ?2, 0, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?2",
        params![conv, slug],
    )
    .expect("name");
    conn.execute(
        "INSERT INTO workspace_tab_sessions \
         (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch()) \
         ON CONFLICT(project_id, pane_group_id) DO UPDATE SET session_id = excluded.session_id",
        params![
            project_id,
            format!("pane-{conv}"),
            format!("tab-pane-{conv}"),
            conv
        ],
    )
    .expect("tab");
}

fn assert_skin_room(r: &Resp) {
    assert_eq!(r.status, 403, "skin_room status; {}", r.body);
    let v = json(&r.body);
    assert_eq!(v["ok"], false, "{}", r.body);
    assert_eq!(v["error"]["code"], "skin_room", "{}", r.body);
    assert_eq!(
        v["error"]["hint"], "this pass cannot use that agent",
        "{}",
        r.body
    );
}

fn add_user(port: u16, username: &str) {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}"}}"#)),
    );
    assert_eq!(r.status, 200, "skin user add; {}", r.body);
}

fn mint(port: u16, name: &str, caps: &[&str], rooms: &[&str]) -> (String, String) {
    let caps_json = serde_json::to_string(&caps).unwrap();
    let rooms_json = serde_json::to_string(&rooms).unwrap();
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"name":"{name}","caps":{caps_json},"rooms":{rooms_json}}}"#
        )),
    );
    assert_eq!(r.status, 200, "mint; {}", r.body);
    let v = json(&r.body);
    let token = v["token"].as_str().expect("token once").to_string();
    assert!(
        token.starts_with("k2skn_"),
        "prefix must be k2skn_, got {token}"
    );
    assert!(
        !token.starts_with("k2sk_"),
        "must never mint k2sk_: {token}"
    );
    (v["id"].as_str().expect("id").to_string(), token)
}

/// Workspace-agent passport — same registry mint as K2_HOOK_SCOPED spawn.
fn mint_scoped_hook() -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: "skin-it-ws".to_string(),
            agent_address: "skin-it-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_slice12_thread_rooms_grid_403() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let handle = format!("skinsales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        add_user(port, "guest");

        let (_id, read_tok) = mint(port, "guest-read", &["thread:read"], &[&handle]);

        let get = http(
            port,
            "GET",
            &format!("/cli/thread?token={read_tok}&addr={handle}"),
            None,
        );
        assert_eq!(get.status, 200, "thread:read GET thread; {}", get.body);
        let snap = json(&get.body);
        assert_eq!(snap["ok"], true, "{snap}");
        assert_eq!(snap["collection"], "thread");

        let grid = http(
            port,
            "GET",
            &format!("/cli/sessions/grid?token={read_tok}&session=nope"),
            None,
        );
        assert_eq!(grid.status, 403, "grid must 403; {}", grid.body);
        assert_eq!(
            grid.body.trim(),
            TERMINAL_403,
            "grid 403 body is pinned: {}",
            grid.body
        );

        let bytes = http(
            port,
            "GET",
            &format!("/cli/sessions/bytes?token={read_tok}&session=nope"),
            None,
        );
        assert_eq!(bytes.status, 403, "bytes must 403; {}", bytes.body);
        assert_eq!(bytes.body.trim(), TERMINAL_403);

        let post = http(
            port,
            "POST",
            &format!("/cli/thread/post?token={read_tok}"),
            Some(&format!(r#"{{"addr":"{handle}","text":"nope"}}"#)),
        );
        assert_eq!(post.status, 403, "no thread:post; {}", post.body);
        assert!(
            post.body.contains("thread:post"),
            "missing-cap must name thread:post: {}",
            post.body
        );

        let (_pid, post_tok) = mint(
            port,
            "guest-post",
            &["thread:read", "thread:post"],
            &[&handle],
        );
        let posted = http(
            port,
            "POST",
            &format!("/cli/thread/post?token={post_tok}"),
            Some(&format!(r#"{{"addr":"{handle}","text":"hello-skin"}}"#)),
        );
        assert_eq!(posted.status, 200, "thread:post; {}", posted.body);

        let chatter = http(
            port,
            "GET",
            &format!("/cli/chatterlog?token={read_tok}"),
            None,
        );
        assert_eq!(chatter.status, 403, "chatterlog; {}", chatter.body);

        let whoami = http(port, "GET", &format!("/cli/whoami?token={read_tok}"), None);
        assert_eq!(
            whoami.status, 403,
            "skin must not be token_ok; {}",
            whoami.body
        );

        let owner_get = http(
            port,
            "GET",
            &format!("/cli/thread?token={OWNER_TOKEN}&addr={handle}"),
            None,
        );
        assert_eq!(
            owner_get.status, 200,
            "owner still reads thread; {}",
            owner_get.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_revoke_is_401_and_mutations_are_post_only() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let handle = format!("skinrev{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        add_user(port, "revokee");
        let (id, tok) = mint(port, "revokee", &["thread:read"], &[&handle]);

        let ok = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={handle}"),
            None,
        );
        assert_eq!(ok.status, 200, "{}", ok.body);

        let rev = http(
            port,
            "POST",
            &format!("/cli/skin-tokens/revoke?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"id":"{id}"}}"#)),
        );
        assert_eq!(rev.status, 200, "{}", rev.body);

        let gone = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={handle}"),
            None,
        );
        assert_eq!(gone.status, 401, "revoked GET must 401; {}", gone.body);
        assert!(
            gone.body.contains("invalid or revoked skin token"),
            "{}",
            gone.body
        );

        let get_revoke = http(
            port,
            "GET",
            &format!("/cli/skin-tokens/revoke?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get_revoke.status, 405, "GET revoke; {}", get_revoke.body);

        let get_remove = http(
            port,
            "GET",
            &format!("/cli/skin/users/remove?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get_remove.status, 405, "GET remove; {}", get_remove.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_mint_is_owner_tier_admin_member_403() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        add_user(port, "stay");
        let handle = format!("skinown{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        let admin = provision_role(port, "skinadmin", "hunter2-strong-1", "admin");
        let member = provision_role(port, "skinmember", "hunter2-strong-2", "member");

        for (tok, who) in [(&admin, "admin"), (&member, "member")] {
            let r = http(
                port,
                "POST",
                &format!("/cli/skin-tokens?token={tok}"),
                Some(r#"{"name":"stay","caps":["thread:read"]}"#),
            );
            assert_eq!(r.status, 403, "{who} must not mint; {}", r.body);
            let list = http(port, "GET", &format!("/cli/skin-tokens?token={tok}"), None);
            assert_eq!(list.status, 403, "{who} list; {}", list.body);
        }

        let owner_role = provision_role(port, "skinowner", "hunter2-strong-3", "owner");
        let r = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={owner_role}"),
            Some(&format!(
                r#"{{"name":"stay","caps":["thread:read"],"rooms":["{handle}"]}}"#
            )),
        );
        assert_eq!(r.status, 200, "Owner-ROLE may mint; {}", r.body);
        let minted = json(&r.body);
        let raw = minted["token"].as_str().expect("token");
        assert!(raw.starts_with("k2skn_"));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_workspace_agent_hook_can_list_but_not_mint() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        add_user(port, "guest");
        let handle = format!("skinhook{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        let (_id, raw_secret) = mint(port, "guest", &["thread:read"], &[&handle]);
        let hook = mint_scoped_hook();

        let users = http(port, "GET", &format!("/cli/skin/users?token={hook}"), None);
        assert_eq!(
            users.status, 200,
            "workspace-agent hook GET users; {}",
            users.body
        );
        let u = json(&users.body);
        assert!(u["users"].is_array(), "{}", users.body);
        assert!(
            !users.body.contains("Invalid or missing auth token"),
            "{}",
            users.body
        );

        let tokens = http(port, "GET", &format!("/cli/skin-tokens?token={hook}"), None);
        assert_eq!(
            tokens.status, 200,
            "workspace-agent hook GET tokens; {}",
            tokens.body
        );
        let t = json(&tokens.body);
        let list = t["tokens"].as_array().expect("tokens array");
        assert!(!list.is_empty(), "{}", tokens.body);
        for row in list {
            assert!(
                row.get("token").is_none(),
                "list must not return raw secret: {row}"
            );
            let prefix = row["prefix"].as_str().unwrap_or("");
            assert!(
                prefix.starts_with("k2skn_"),
                "prefix is the public stub: {prefix}"
            );
        }
        assert!(
            !tokens.body.contains(&raw_secret),
            "list must not echo the raw k2skn_ secret"
        );

        let mint_denied = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={hook}"),
            Some(r#"{"name":"guest","caps":["thread:read"]}"#),
        );
        assert_eq!(
            mint_denied.status, 403,
            "hook must not mint; {}",
            mint_denied.body
        );
        assert!(
            mint_denied.body.contains("owner_only"),
            "valid hook on mutation must teach owner_only: {}",
            mint_denied.body
        );
        assert!(
            !mint_denied.body.contains("Invalid or missing auth token"),
            "must not look like a broken passport: {}",
            mint_denied.body
        );

        let add_denied = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={hook}"),
            Some(r#"{"username":"intruder"}"#),
        );
        assert_eq!(add_denied.status, 403, "{}", add_denied.body);
        assert!(
            add_denied.body.contains("owner_only"),
            "{}",
            add_denied.body
        );

        let door = http(
            port,
            "GET",
            &format!("/cli/skin/front-door?token={hook}"),
            None,
        );
        assert_eq!(
            door.status, 403,
            "front-door stays owner-only; {}",
            door.body
        );
        assert!(
            door.body.contains("owner_only"),
            "valid hook on GET front-door must teach owner_only: {}",
            door.body
        );

        let missing = http(port, "GET", "/cli/skin/users", None);
        assert_eq!(missing.status, 403, "{}", missing.body);
        assert!(
            missing.body.contains("Invalid or missing auth token"),
            "missing credential stays classic forbidden: {}",
            missing.body
        );
        assert!(!missing.body.contains("owner_only"));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_nested_label_skin_is_400_loud() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let db = k2_core::db::shared();
        let conn = db.lock();
        let pid = uuid::Uuid::new_v4().to_string();
        conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'skinws', '/tmp/skin-rsv', 'skinws')",
                params![pid],
            )
            .expect("project");
        drop(conn);

        let r = http(
            port,
            "POST",
            &format!("/cli/tunnel/subdomains/claim?token={OWNER_TOKEN}&label=skin&project={pid}"),
            Some(""),
        );
        assert_eq!(r.status, 400, "reserved skin; {}", r.body);
        assert!(
            r.body.contains("reserved_label"),
            "must fail loud: {}",
            r.body
        );
        assert!(
            r.body.contains("Pick another nested label") || r.body.contains("k2 study skins"),
            "must point at another label, not a Caddy hostname: {}",
            r.body
        );
        assert!(
            !r.body.contains("38472"),
            "must not teach 38472 as a publish target: {}",
            r.body
        );

        let err = k2_core::published_services::normalize_name("skin").unwrap_err();
        assert!(err.contains("reserved_label"), "{err}");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_front_door_persists_connect_url_from_tunnel() {
    let _g = lock();
    with_temp_home(|| {
        let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
        cfg.subdomain = "rosson".into();
        k2_core::tunnel::config::save(&cfg).expect("save tunnel.json");

        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let set = http(
            port,
            "POST",
            &format!("/cli/skin/front-door?token={OWNER_TOKEN}"),
            Some(r#"{"mode":"connect","apply":false}"#),
        );
        assert_eq!(set.status, 200, "{}", set.body);
        let v = json(&set.body);
        assert_eq!(v["mode"], "connect");
        assert_eq!(v["url"], "https://skin.rosson.k2.dev");
        assert_eq!(v["connectUrl"], "https://skin.rosson.k2.dev");
        assert_eq!(v["listen"], "127.0.0.1:38472");
        assert_eq!(v["nested"]["label"], "skin");
        assert_eq!(v["applied"], false);

        let get = http(
            port,
            "GET",
            &format!("/cli/skin/front-door?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get.status, 200, "{}", get.body);
        let g = json(&get.body);
        assert_eq!(g["url"], "https://skin.rosson.k2.dev");
        assert_eq!(g["connectUrl"], "https://skin.rosson.k2.dev");
        assert!(g.get("caddy").is_some(), "{}", get.body);

        let direct = http(
            port,
            "POST",
            &format!("/cli/skin/front-door?token={OWNER_TOKEN}"),
            Some(r#"{"mode":"direct","url":"https://skin.app.com","hint":"Caddy","apply":false}"#),
        );
        assert_eq!(direct.status, 200, "{}", direct.body);
        assert_eq!(json(&direct.body)["mode"], "direct");
        assert_eq!(json(&direct.body)["url"], "https://skin.app.com");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_front_door_apply_without_caddy_is_400() {
    let _g = lock();
    with_temp_home(|| {
        let prev_path = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let r = http(
            port,
            "POST",
            &format!("/cli/skin/front-door?token={OWNER_TOKEN}"),
            Some(r#"{"mode":"connect"}"#),
        );
        match prev_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(r.status, 400, "apply without caddy must 400; {}", r.body);
        assert!(
            r.body.contains("caddy_missing"),
            "must name caddy_missing: {}",
            r.body
        );
        assert!(
            r.body.contains("brew install caddy") || r.body.contains("apt install"),
            "must teach install: {}",
            r.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_hydra_get_owner_status_and_post_off_not_running() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let missing = http(port, "GET", "/cli/skin/hydra", None);
        assert_eq!(missing.status, 403, "{}", missing.body);

        let get = http(
            port,
            "GET",
            &format!("/cli/skin/hydra?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get.status, 200, "{}", get.body);
        let v = json(&get.body);
        assert_eq!(v["supported"], cfg!(target_os = "linux"), "{}", get.body);
        assert_eq!(v["enabled"], false, "{}", get.body);
        assert_eq!(v["running"], false, "{}", get.body);
        assert!(v.get("publicUrl").is_some(), "{}", get.body);
        assert!(v.get("adminUrl").is_some(), "{}", get.body);
        assert!(
            v.get("hint").and_then(|h| h.as_str()).is_some(),
            "{}",
            get.body
        );
        if !cfg!(target_os = "linux") {
            assert!(
                v["hint"].as_str().unwrap_or("").contains("LINUX"),
                "Mac banner: {}",
                get.body
            );
        }

        let off = http(
            port,
            "POST",
            &format!("/cli/skin/hydra?token={OWNER_TOKEN}"),
            Some(r#"{"enabled":false,"apply":true}"#),
        );
        assert_eq!(off.status, 200, "{}", off.body);
        let o = json(&off.body);
        assert_eq!(o["enabled"], false, "{}", off.body);
        assert_eq!(o["running"], false, "{}", off.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_host_belt_403s_grid_login_v1_not_thread() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let handle = format!("skindoor{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        add_user(port, "guest");
        let (_id, tok) = mint(port, "guest", &["thread:read"], &[&handle]);
        let skin_host = "skin.rosson.k2.dev";

        let grid = http_host(
            port,
            "GET",
            &format!("/cli/sessions/grid?token={OWNER_TOKEN}&session=nope"),
            None,
            skin_host,
        );
        assert_eq!(
            grid.status, 403,
            "skin Host + owner token grid; {}",
            grid.body
        );
        assert!(
            grid.body
                .contains("skin front door does not proxy this path"),
            "{}",
            grid.body
        );

        let bytes = http_host(
            port,
            "GET",
            &format!("/cli/sessions/bytes?token={OWNER_TOKEN}&session=nope"),
            None,
            skin_host,
        );
        assert_eq!(bytes.status, 403, "skin Host + bytes; {}", bytes.body);

        let login = http_host(
            port,
            "POST",
            "/cli/auth/login",
            Some(r#"{"username":"x","password":"y"}"#),
            skin_host,
        );
        assert_eq!(login.status, 403, "skin Host + login; {}", login.body);

        let v1 = http_host(
            port,
            "GET",
            &format!("/v1/ping?token={OWNER_TOKEN}"),
            None,
            skin_host,
        );
        assert_eq!(v1.status, 403, "skin Host + /v1; {}", v1.body);

        let term = http_host(
            port,
            "GET",
            &format!("/cli/terminal/write?token={OWNER_TOKEN}&id=nope"),
            None,
            skin_host,
        );
        assert_eq!(term.status, 403, "skin Host + terminal; {}", term.body);

        let thread = http_host(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={handle}"),
            None,
            skin_host,
        );
        assert_eq!(
            thread.status, 200,
            "skin Host must still allow thread: {}",
            thread.body
        );

        assert!(
            k2_daemon::skin_routes::skin_host_forbidden("/cli/thread").is_none(),
            "Host belt must not 403 /cli/thread"
        );
        assert!(
            k2_daemon::skin_routes::skin_host_forbidden("/cli/overlay/events").is_none(),
            "Host belt must not 403 overlay"
        );
        assert!(
            k2_daemon::skin_routes::skin_host_forbidden("/cli/terminal/write").is_some(),
            "Host belt must 403 terminal write"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlay_ws_accepts_skin_read_and_filters_chatterlog() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        add_user(port, "wsguest");
        let handle = format!("skinws{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_project_id, conv) = seed_thread_addr(&handle);
        let (_id, tok) = mint(port, "wsguest", &["thread:read"], &[&handle]);

        futures_block(async {
            let url =
                format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={tok}");
            let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
                .await
                .expect("skin overlay WS must accept thread:read");

            tokio::time::sleep(Duration::from_millis(50)).await;

            overlay_ws::publish(OverlayFrame {
                collection: "chatterlog".into(),
                seq: 1,
                id: "clog".into(),
                doc: None,
                conversation_id: None,
            });
            overlay_ws::publish(OverlayFrame {
                collection: "thread".into(),
                seq: 2,
                id: "tid".into(),
                doc: Some(OverlayDoc::text(
                    "tid".into(),
                    "k2".into(),
                    "guest".into(),
                    "hi".into(),
                    "thread",
                )),
                conversation_id: Some(conv.clone()),
            });

            let received = timeout(Duration::from_secs(3), ws.next())
                .await
                .expect("timed out waiting for overlay frame")
                .expect("ws open")
                .expect("message Ok");
            let text = match received {
                Message::Text(t) => t,
                other => panic!("expected text, got {other:?}"),
            };
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("frame JSON");
            assert_eq!(
                parsed["collection"], "thread",
                "skin WS must not see chatterlog; got {parsed}"
            );
            assert_eq!(parsed["id"], "tid");

            match timeout(Duration::from_millis(200), ws.next()).await {
                Err(_) => {}
                Ok(None) => {}
                Ok(Some(Ok(Message::Text(t)))) => {
                    panic!("skin WS must not receive chatterlog/extra frame: {t}")
                }
                Ok(Some(other)) => panic!("unexpected extra frame: {other:?}"),
            }
        });
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_mint_rooms_required_and_unknown_handle_400() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        add_user(port, "guest");
        let handle = format!("skinsales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);

        let leftover_user = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"username":"guest","caps":["thread:read"],"rooms":["x"]}"#),
        );
        assert_eq!(
            leftover_user.status, 400,
            "username in body; {}",
            leftover_user.body
        );
        assert!(
            leftover_user
                .body
                .contains("use name (platform label), not username"),
            "{}",
            leftover_user.body
        );

        let missing = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"name":"guest","caps":["thread:read"]}"#),
        );
        assert_eq!(missing.status, 400, "mint without rooms; {}", missing.body);
        assert!(
            missing
                .body
                .contains("rooms must include at least one workspace"),
            "{}",
            missing.body
        );

        let empty = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"name":"guest","caps":["thread:read"],"rooms":[]}"#),
        );
        assert_eq!(empty.status, 400, "mint empty rooms; {}", empty.body);

        let unknown = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"name":"guest","caps":["thread:read"],"rooms":["not-a-handle"]}"#),
        );
        assert_eq!(unknown.status, 400, "unknown handle; {}", unknown.body);
        assert!(
            unknown.body.contains("unknown workspace handle"),
            "{}",
            unknown.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_rooms_acl_http_ws_list_and_compose() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let sales = format!("sales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("other{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (sales_id, sales_pin) = seed_thread_addr(&sales);
        let (_other_id, _other_pin) = seed_thread_addr(&other);
        let sidecar_conv = uuid::Uuid::new_v4().to_string();
        seed_sidecar(&sales_id, &sidecar_conv, "reviewer");
        add_user(port, "guest");
        let (_id, tok) = mint(port, "guest", &["thread:read", "thread:post"], &[&sales]);

        let ok = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={sales}"),
            None,
        );
        assert_eq!(ok.status, 200, "sales handle; {}", ok.body);

        let pin_ok = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={sales_pin}"),
            None,
        );
        assert_eq!(pin_ok.status, 200, "pinned Chat uuid; {}", pin_ok.body);

        let deny_other = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={other}"),
            None,
        );
        assert_skin_room(&deny_other);
        assert!(
            !deny_other.body.contains(&other),
            "must not echo the other handle: {}",
            deny_other.body
        );

        let unknown = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr=no-such-agent"),
            None,
        );
        assert_skin_room(&unknown);

        let sidecar_addr = format!("{sales}/reviewer");
        let deny_sidecar = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={sidecar_addr}"),
            None,
        );
        assert_skin_room(&deny_sidecar);

        let deny_sidecar_uuid = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok}&addr={sidecar_conv}"),
            None,
        );
        assert_skin_room(&deny_sidecar_uuid);

        let posted = http(
            port,
            "POST",
            &format!("/cli/thread/post?token={tok}"),
            Some(&format!(r#"{{"addr":"{sales}","text":"hello-skin"}}"#)),
        );
        assert_eq!(posted.status, 200, "post sales; {}", posted.body);

        let post_other = http(
            port,
            "POST",
            &format!("/cli/thread/post?token={tok}"),
            Some(&format!(r#"{{"addr":"{other}","text":"nope"}}"#)),
        );
        assert_skin_room(&post_other);

        let compose = http(
            port,
            "POST",
            &format!("/cli/thread/post?token={tok}"),
            Some(&format!(
                r#"{{"addr":"{sales}","text":"pty","via":"compose"}}"#
            )),
        );
        assert_eq!(compose.status, 403, "via=compose; {}", compose.body);

        let grid = http(
            port,
            "GET",
            &format!("/cli/sessions/grid?token={tok}&session=nope"),
            None,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert_eq!(grid.body.trim(), TERMINAL_403);

        let chatter = http(port, "GET", &format!("/cli/chatterlog?token={tok}"), None);
        assert_eq!(chatter.status, 403, "chatter; {}", chatter.body);

        let agents = http(port, "GET", &format!("/cli/skin/agents?token={tok}"), None);
        assert_eq!(agents.status, 200, "{}", agents.body);
        let av = json(&agents.body);
        let list = av["agents"].as_array().expect("agents array");
        assert_eq!(list.len(), 1, "{}", agents.body);
        assert_eq!(list[0]["handle"], sales, "{}", agents.body);
        assert_eq!(list[0]["projectId"], sales_id, "{}", agents.body);
        let display_name = list[0]["displayName"].as_str().unwrap_or("");
        assert!(
            !display_name.is_empty(),
            "displayName must be a non-empty string: {}",
            agents.body
        );
        assert!(
            !agents.body.contains(&other),
            "list must not contain other handle: {}",
            agents.body
        );

        let owner_agents = http(
            port,
            "GET",
            &format!("/cli/skin/agents?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            owner_agents.status, 403,
            "owner agents; {}",
            owner_agents.body
        );

        let owner_other = http(
            port,
            "GET",
            &format!("/cli/thread?token={OWNER_TOKEN}&addr={other}"),
            None,
        );
        assert_eq!(
            owner_other.status, 200,
            "owner still reads other; {}",
            owner_other.body
        );

        // Skin Thread post may wake a PTY and restamp the pin. WS the live Chat.
        let live_pin = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            WorkspaceSession::get(&conn, &sales_id)
                .ok()
                .flatten()
                .and_then(|s| s.session_id)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| sales_pin.clone())
        };
        futures_block(async {
            let url = format!(
                "ws://127.0.0.1:{port}/cli/overlay/events?conversation={live_pin}&token={tok}"
            );
            let (_ws, resp) = tokio_tungstenite::connect_async(&url)
                .await
                .expect("pinned Chat WS must upgrade");
            assert_eq!(resp.status(), 101, "upgrade; {resp:?}");
        });

        let ws_other = http(
            port,
            "GET",
            &format!("/cli/overlay/events?conversation={sidecar_conv}&token={tok}"),
            None,
        );
        assert_eq!(ws_other.status, 403, "sidecar WS; {}", ws_other.body);
        assert_skin_room(&ws_other);

        let random = uuid::Uuid::new_v4().to_string();
        let ws_rand = http(
            port,
            "GET",
            &format!("/cli/overlay/events?conversation={random}&token={tok}"),
            None,
        );
        assert_eq!(ws_rand.status, 403, "random UUID WS; {}", ws_rand.body);
        assert_ne!(ws_rand.status, 101, "must not upgrade");

        let missing_conv = http(
            port,
            "GET",
            &format!("/cli/overlay/events?token={tok}"),
            None,
        );
        assert_eq!(
            missing_conv.status, 400,
            "missing conversation; {}",
            missing_conv.body
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_empty_rooms_dark_rename_delete_apply_hook() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let sales = format!("sales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other = format!("other{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (sales_id, sales_pin) = seed_thread_addr(&sales);
        seed_thread_addr(&other);
        add_user(port, "guest");
        let (id_a, tok_a) = mint(port, "guest-a", &["thread:read"], &[&sales]);
        let (id_b, tok_b) = mint(port, "guest-b", &["thread:read"], &[&sales]);

        let clear = http(
            port,
            "POST",
            &format!("/cli/skin-tokens/rooms?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"id":"{id_a}","rooms":[]}}"#)),
        );
        assert_eq!(clear.status, 200, "{}", clear.body);
        let dark = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok_a}&addr={sales}"),
            None,
        );
        assert_skin_room(&dark);
        let dark_ws = http(
            port,
            "GET",
            &format!("/cli/overlay/events?conversation={sales_pin}&token={tok_a}"),
            None,
        );
        assert_eq!(dark_ws.status, 403, "empty rooms WS; {}", dark_ws.body);
        assert_ne!(dark_ws.status, 101);
        assert_skin_room(&dark_ws);

        let path = format!("/tmp/skin-it-{sales}-{sales_id}");
        let new_handle = format!("renamed{}", &uuid::Uuid::new_v4().to_string()[..8]);
        k2_core::workspace::handle::set_workspace_handle(&path, &new_handle)
            .expect("rename handle");
        let renamed = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok_b}&addr={new_handle}"),
            None,
        );
        assert_eq!(renamed.status, 200, "rename still allows; {}", renamed.body);

        let apply = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"username":"guest","rooms":["{other}"],"applyTokens":true}}"#
            )),
        );
        assert_eq!(apply.status, 200, "{}", apply.body);
        let listed = http(
            port,
            "GET",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(listed.status, 200, "{}", listed.body);
        let listed_v = json(&listed.body);
        let tokens = listed_v["tokens"].as_array().expect("tokens");
        for row in tokens {
            if row["id"] == id_a {
                let handles = row["roomHandles"].as_array().expect("roomHandles");
                assert!(
                    handles.is_empty(),
                    "apply-tokens must not touch platform token a: {row}"
                );
            }
            if row["id"] == id_b {
                let handles = row["roomHandles"].as_array().expect("roomHandles");
                assert!(
                    handles.iter().any(|h| h == &new_handle || h == &sales),
                    "apply-tokens must not rewrite platform token b: {row}"
                );
                assert!(
                    !handles.iter().any(|h| h == &other),
                    "apply-tokens is sessions only: {row}"
                );
            }
        }

        add_user(port, "otherguest");
        let (id_c, tok_c) = mint(port, "other-c", &["thread:read"], &[&new_handle]);
        let (id_d, _tok_d) = mint(port, "other-d", &["thread:read"], &[&new_handle]);
        let no_apply = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"username":"otherguest","rooms":["{other}"]}}"#
            )),
        );
        assert_eq!(no_apply.status, 200, "{}", no_apply.body);
        let listed2 = http(
            port,
            "GET",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            None,
        );
        let listed2_v = json(&listed2.body);
        let tokens2 = listed2_v["tokens"].as_array().expect("tokens");
        let row_c = tokens2.iter().find(|t| t["id"] == id_c).expect("c");
        let row_d = tokens2.iter().find(|t| t["id"] == id_d).expect("d");
        let handles_c = row_c["roomHandles"].as_array().expect("c handles");
        let handles_d = row_d["roomHandles"].as_array().expect("d handles");
        assert!(
            handles_c.iter().any(|h| h == &new_handle),
            "without applyTokens key c stays renamed sales: {row_c}"
        );
        assert!(
            handles_d.iter().any(|h| h == &new_handle),
            "without applyTokens key d stays renamed sales: {row_d}"
        );

        let hook = mint_scoped_hook_for(&sales_id);
        let hook_sales = http(
            port,
            "GET",
            &format!("/cli/thread?token={hook}&addr={new_handle}"),
            None,
        );
        assert_eq!(
            hook_sales.status, 200,
            "hook same-workspace overlay; {}",
            hook_sales.body
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM projects WHERE id = ?1", params![sales_id])
                .expect("delete workspace");
        }
        let gone = http(
            port,
            "GET",
            &format!("/cli/thread?token={tok_c}&addr={new_handle}"),
            None,
        );
        assert_skin_room(&gone);
        let agents = http(
            port,
            "GET",
            &format!("/cli/skin/agents?token={tok_c}"),
            None,
        );
        assert_eq!(agents.status, 200, "{}", agents.body);
        assert!(
            !agents.body.contains(&new_handle) && !agents.body.contains(&sales),
            "deleted workspace omitted: {}",
            agents.body
        );
    });
}

fn set_password(port: u16, username: &str, password: &str) {
    let r = http(
        port,
        "POST",
        &format!("/cli/skin/users/password?token={OWNER_TOKEN}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )),
    );
    assert_eq!(r.status, 200, "set password; {}", r.body);
    assert!(
        !r.body.contains("password_hash") && !r.body.contains("passwordHash"),
        "hash must not be on the wire: {}",
        r.body
    );
}

fn cookie_get(port: u16, path: &str, host: &str, cookie: &str) -> Resp {
    http_host_ex(port, "GET", path, None, host, &format!("Cookie: {cookie}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_login_session_cookie_logout_and_host_fold() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let handle = format!("skinlogin{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        add_user(port, "guest");
        let rooms = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"guest","rooms":["{handle}"]}}"#)),
        );
        assert_eq!(rooms.status, 200, "default rooms; {}", rooms.body);
        set_password(port, "guest", "s3cret-horse");

        let get_login = http(port, "GET", "/cli/skin/login", None);
        assert_eq!(get_login.status, 405, "GET login; {}", get_login.body);
        assert!(
            get_login.body.contains("POST required"),
            "{}",
            get_login.body
        );

        let unknown = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"nope","password":"s3cret-horse"}"#),
        );
        let wrong = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"nope"}"#),
        );
        assert_eq!(unknown.status, 401, "{}", unknown.body);
        assert_eq!(wrong.status, 401, "{}", wrong.body);
        assert_eq!(
            unknown.body, wrong.body,
            "unknown and wrong password must be the same 401 body"
        );
        assert_eq!(
            unknown.body.trim(),
            r#"{"error":"invalid username or password"}"#
        );
        assert!(!unknown.body.contains("guest"));
        assert!(!unknown.body.contains("nope"));

        let ok = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        assert_eq!(ok.status, 200, "{}", ok.body);
        let v = json(&ok.body);
        assert_eq!(v["ok"], true, "{}", ok.body);
        let token = v["token"].as_str().expect("token").to_string();
        assert!(token.starts_with("k2skn_"), "{token}");
        let caps = v["caps"].as_array().expect("caps");
        assert!(
            caps.iter().any(|c| c == "thread:read"),
            "unassigned login Thread-only; {}",
            ok.body
        );
        assert!(
            !caps.iter().any(|c| c == "files:read"),
            "unassigned must not silent-add files; {}",
            ok.body
        );
        assert!(v["role"].is_null(), "unassigned role is null; {}", ok.body);
        let listed_sessions = http(
            port,
            "GET",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(listed_sessions.status, 200, "{}", listed_sessions.body);
        let listed_v = json(&listed_sessions.body);
        let listed_arr = listed_v["tokens"].as_array().expect("tokens");
        assert!(
            listed_arr.is_empty(),
            "list is platform-only; must hide login session: {}",
            listed_sessions.body
        );
        assert!(!ok.body.contains("password_hash"));
        assert!(!ok.body.contains("passwordHash"));
        let set_cookie = header_value(&ok.headers, "set-cookie").expect("Set-Cookie");
        assert!(set_cookie.contains("k2_skin_session="), "{set_cookie}");
        assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
        assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
        assert!(!set_cookie.contains("SameSite=Strict"), "{set_cookie}");
        assert!(set_cookie.contains("Path=/"), "{set_cookie}");
        assert!(set_cookie.contains(&token), "{set_cookie}");

        let thread = cookie_get(
            port,
            &format!("/cli/thread?addr={handle}"),
            "skin.rosson.k2.dev",
            &format!("k2_skin_session={token}"),
        );
        assert_eq!(thread.status, 200, "skin.* cookie fold; {}", thread.body);

        k2_core::skin::set_front_door("direct", Some("https://skin.app.com"), None, None)
            .expect("direct door");
        let direct = cookie_get(
            port,
            &format!("/cli/thread?addr={handle}"),
            "skin.app.com",
            &format!("k2_skin_session={token}"),
        );
        assert_eq!(
            direct.status, 200,
            "Direct Host cookie fold; {}",
            direct.body
        );

        let operator = cookie_get(
            port,
            &format!("/cli/thread?addr={handle}"),
            "127.0.0.1",
            &format!("k2_skin_session={token}"),
        );
        assert_ne!(
            operator.status, 200,
            "operator Host must not fold skin cookie; {}",
            operator.body
        );

        let grid = cookie_get(
            port,
            "/cli/sessions/grid?session=nope",
            "skin.rosson.k2.dev",
            &format!("k2_skin_session={token}"),
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        // Host belt 403s grid on `skin.*` before the token terminal check.
        assert!(
            grid.body
                .contains("skin front door does not proxy this path")
                || grid.body.trim() == TERMINAL_403,
            "grid 403; {}",
            grid.body
        );

        let logout = http_host_ex(
            port,
            "POST",
            "/cli/skin/logout",
            Some("{}"),
            "skin.rosson.k2.dev",
            &format!("Cookie: k2_skin_session={token}"),
        );
        assert_eq!(logout.status, 200, "{}", logout.body);
        let clear = header_value(&logout.headers, "set-cookie").expect("clear cookie");
        assert!(clear.contains("Max-Age=0"), "{clear}");
        let dead = http(
            port,
            "GET",
            &format!("/cli/thread?token={token}&addr={handle}"),
            None,
        );
        assert_eq!(dead.status, 401, "revoked session; {}", dead.body);

        let (_id, static_tok) = mint(port, "guest", &["thread:read"], &[&handle]);
        let static_logout = http(
            port,
            "POST",
            &format!("/cli/skin/logout?token={static_tok}"),
            Some("{}"),
        );
        assert!(
            static_logout.status == 400 || static_logout.status == 403,
            "static logout; {}",
            static_logout.body
        );
        let still = http(
            port,
            "GET",
            &format!("/cli/thread?token={static_tok}&addr={handle}"),
            None,
        );
        assert_eq!(
            still.status, 200,
            "static key survives logout; {}",
            still.body
        );

        let session2 = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"guest","password":"s3cret-horse"}"#),
        );
        let session_tok = json(&session2.body)["token"]
            .as_str()
            .expect("session2")
            .to_string();
        set_password(port, "guest", "new-pass-word");
        let after_reset = http(
            port,
            "GET",
            &format!("/cli/thread?token={session_tok}&addr={handle}"),
            None,
        );
        assert_eq!(
            after_reset.status, 401,
            "password reset revokes sessions; {}",
            after_reset.body
        );
        let static_after = http(
            port,
            "GET",
            &format!("/cli/thread?token={static_tok}&addr={handle}"),
            None,
        );
        assert_eq!(
            static_after.status, 200,
            "static survives password reset; {}",
            static_after.body
        );

        let hook = mint_scoped_hook();
        let hook_pw = http(
            port,
            "POST",
            &format!("/cli/skin/users/password?token={hook}"),
            Some(r#"{"username":"guest","password":"x"}"#),
        );
        assert_eq!(hook_pw.status, 403, "agent passport; {}", hook_pw.body);
        assert!(hook_pw.body.contains("owner_only"), "{}", hook_pw.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_login_empty_rooms_thread_403_connect_cookie_not_a_pass() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let handle = format!("skindark{}", &uuid::Uuid::new_v4().to_string()[..8]);
        seed_thread_addr(&handle);
        add_user(port, "emptyrooms");
        set_password(port, "emptyrooms", "s3cret-horse");
        // default_rooms stay []
        let ok = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"emptyrooms","password":"s3cret-horse"}"#),
        );
        assert_eq!(ok.status, 200, "{}", ok.body);
        let token = json(&ok.body)["token"].as_str().expect("token").to_string();
        let dark = http(
            port,
            "GET",
            &format!("/cli/thread?token={token}&addr={handle}"),
            None,
        );
        assert_skin_room(&dark);

        let connect_tok = provision_role(port, "opuser", "op-pass-word", "member");
        let connect_on_skin = cookie_get(
            port,
            &format!("/cli/thread?addr={handle}"),
            "skin.rosson.k2.dev",
            &format!("k2_session={connect_tok}"),
        );
        assert_ne!(
            connect_on_skin.status, 200,
            "Connect cookie is not a skin pass; {}",
            connect_on_skin.body
        );

        let skin_on_op = cookie_get(
            port,
            "/cli/auth/whoami",
            "127.0.0.1",
            &format!("k2_skin_session={token}"),
        );
        assert_ne!(
            skin_on_op.status, 200,
            "skin cookie is not Connect on operator Host; {}",
            skin_on_op.body
        );

        let caddy = k2_core::skin_door::render_caddyfile(&k2_core::skin_door::CaddyfileSpec {
            daemon_port: 18789,
            loopback_port: 38472,
            extra_listen: None,
            ui_port: Some(5173),
            skin_host: None,
            mail_host: None,
            bind_http80: false,
        });
        assert!(caddy.contains("handle /cli/skin/login"), "{caddy}");
        assert!(caddy.contains("/login*"), "{caddy}");
        assert!(caddy.contains("handle /cli/skin/logout"), "{caddy}");
    });
}

fn mint_scoped_hook_for(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal {
            workspace_uuid: workspace_uuid.to_string(),
            agent_address: "skin-it-agent".to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn seed_files_workspace(handle: &str) -> (String, std::path::PathBuf) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let dir = std::env::temp_dir().join(format!("k2-skin-fs-{handle}-{id}"));
    std::fs::create_dir_all(&dir).expect("mkdir workspace");
    std::fs::write(dir.join("README.md"), b"hello sales\n").expect("readme");
    let path = dir
        .canonicalize()
        .unwrap_or(dir.clone())
        .to_string_lossy()
        .into_owned();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path],
    )
    .expect("seed files project");
    (id, dir)
}

fn assert_missing_cap(r: &Resp, cap: &str) {
    assert_eq!(r.status, 403, "missing cap status; {}", r.body);
    assert_ne!(r.status, 200, "must not be 200; {}", r.body);
    assert!(
        r.body.contains(cap),
        "403 body should name {cap}: {}",
        r.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_files_http_ws_rooms_and_jail() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let sales = format!("sales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let julie = format!("julie{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_sales_id, sales_dir) = seed_files_workspace(&sales);
        let (_julie_id, julie_dir) = seed_files_workspace(&julie);
        add_user(port, "guest");

        let (_id, thread_tok) = mint(port, "guest-thread", &["thread:read"], &[&sales]);
        let thread_dir = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={thread_tok}&workspace={sales}&path=."),
            None,
        );
        assert_missing_cap(&thread_dir, "files:read");
        assert!(
            !thread_dir.body.contains("skin_room"),
            "thread-only must be missing cap, not skin_room: {}",
            thread_dir.body
        );

        let (_id, write_only) = mint(port, "guest-write", &["files:write"], &[&sales]);
        let write_only_dir = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={write_only}&workspace={sales}&path=."),
            None,
        );
        assert_missing_cap(&write_only_dir, "files:read");

        let (_id, read_tok) = mint(port, "guest-files", &["files:read"], &[&sales]);
        let ok = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={read_tok}&workspace={sales}&path=."),
            None,
        );
        assert_eq!(ok.status, 200, "files:read sales; {}", ok.body);
        let entries = json(&ok.body)
            .as_array()
            .unwrap_or_else(|| panic!("read-dir array; {}", ok.body))
            .clone();
        assert!(
            entries.iter().any(|e| e["name"] == "README.md"),
            "sales tree; {}",
            ok.body
        );
        for e in &entries {
            let p = e["path"].as_str().unwrap_or("");
            assert!(
                !p.starts_with('/'),
                "skin read-dir paths must be workspace-relative, got {p}; {}",
                ok.body
            );
        }

        let other = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={read_tok}&workspace={julie}&path=."),
            None,
        );
        assert_skin_room(&other);

        let unknown = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={read_tok}&workspace=not-a-handle&path=."),
            None,
        );
        assert_skin_room(&unknown);

        let jail = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={read_tok}&workspace={sales}&path=../"),
            None,
        );
        assert!(
            jail.status == 400 || jail.status == 403,
            "jail ../ status; {}",
            jail.body
        );
        assert_ne!(jail.status, 200);

        let abs = http(
            port,
            "GET",
            &format!("/cli/fs/read-file?token={read_tok}&workspace={sales}&path=/etc/passwd"),
            None,
        );
        assert!(
            abs.status == 400 || abs.status == 403,
            "jail abs; {}",
            abs.body
        );
        assert!(!abs.body.contains("root:"), "must never leak /etc/passwd");

        let readme = http(
            port,
            "GET",
            &format!("/cli/fs/read-file?token={read_tok}&workspace={sales}&path=README.md"),
            None,
        );
        assert_eq!(readme.status, 200, "{}", readme.body);
        let rv = json(&readme.body);
        assert!(
            rv["content"].as_str().unwrap_or("").contains("hello sales"),
            "{}",
            readme.body
        );

        let no_write = http(
            port,
            "POST",
            &format!("/cli/fs/write-file?token={read_tok}"),
            Some(&format!(
                r#"{{"workspace":"{sales}","path":"notes.md","content":"n"}}"#
            )),
        );
        assert_missing_cap(&no_write, "files:write");

        let (_id, rw_tok) = mint(port, "guest-rw", &["files:read", "files:write"], &[&sales]);

        let missing_ws = http(port, "GET", &format!("/cli/fs/events?token={rw_tok}"), None);
        assert_eq!(
            missing_ws.status, 400,
            "missing workspace; {}",
            missing_ws.body
        );

        let ws_wrong = http(
            port,
            "GET",
            &format!("/cli/fs/events?workspace={julie}&token={rw_tok}"),
            None,
        );
        assert_eq!(ws_wrong.status, 403, "wrong ws; {}", ws_wrong.body);
        assert_ne!(ws_wrong.status, 101);
        assert_skin_room(&ws_wrong);

        futures_block(async {
            let url =
                format!("ws://127.0.0.1:{port}/cli/fs/events?workspace={sales}&token={rw_tok}");
            let (mut ws, resp) = tokio_tungstenite::connect_async(&url)
                .await
                .expect("allowed files WS must upgrade");
            assert_eq!(resp.status(), 101, "upgrade; {resp:?}");
            tokio::time::sleep(Duration::from_millis(80)).await;

            let write_path = format!("/cli/fs/write-file?token={rw_tok}");
            let write_body =
                format!(r#"{{"workspace":"{sales}","path":"notes.md","content":"from-skin"}}"#);
            let wrote = tokio::task::spawn_blocking(move || {
                http(port, "POST", &write_path, Some(&write_body))
            })
            .await
            .expect("write join");
            assert_eq!(wrote.status, 200, "write; {}", wrote.body);
            let notes = sales_dir.join("notes.md");
            assert_eq!(
                std::fs::read_to_string(&notes).expect("notes on disk"),
                "from-skin"
            );
            assert!(
                !julie_dir.join("notes.md").exists(),
                "must not write julie's tree"
            );

            let frame = timeout(Duration::from_secs(3), ws.next())
                .await
                .expect("timed out waiting for fs_changed")
                .expect("ws closed")
                .expect("ws error");
            let Message::Text(text) = frame else {
                panic!("expected text frame, got {frame:?}");
            };
            let v: serde_json::Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("json {e}: {text}"));
            assert_eq!(v["kind"], "fs_changed", "{text}");
            assert_eq!(v["workspace"], sales, "{text}");
            let paths = v["paths"].as_array().expect("paths");
            assert!(
                paths.iter().any(|p| p.as_str() == Some("notes.md")),
                "relative notes.md; {text}"
            );
            assert!(
                !text.contains(julie_dir.to_string_lossy().as_ref()),
                "must not leak julie path: {text}"
            );
        });

        let owner_dir = http(
            port,
            "GET",
            &format!(
                "/cli/fs/read-dir?token={OWNER_TOKEN}&path={}",
                sales_dir.to_string_lossy()
            ),
            None,
        );
        assert_eq!(
            owner_dir.status, 200,
            "owner read-dir without workspace=; {}",
            owner_dir.body
        );

        let grid = http(
            port,
            "GET",
            &format!("/cli/sessions/grid?token={rw_tok}&session=nope"),
            None,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert_eq!(grid.body.trim(), TERMINAL_403);

        let events = http(
            port,
            "GET",
            &format!("/cli/sessions/events?token={rw_tok}"),
            None,
        );
        assert_eq!(events.status, 403, "sessions/events; {}", events.body);
        assert_ne!(events.status, 101);

        let info = http(port, "GET", &format!("/cli/fs/info?token={rw_tok}"), None);
        assert_eq!(info.status, 403, "fs/info; {}", info.body);

        let _ = std::fs::remove_dir_all(&sales_dir);
        let _ = std::fs::remove_dir_all(&julie_dir);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_roles_http_assign_snapshot_rewrite_and_connect_names() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let sales = format!("sales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (_sales_id, sales_dir) = seed_files_workspace(&sales);
        add_user(port, "bob");
        add_user(port, "cara");
        set_password(port, "bob", "s3cret-horse");
        set_password(port, "cara", "s3cret-horse");
        let rooms = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"cara","rooms":["{sales}"]}}"#)),
        );
        assert_eq!(rooms.status, 200, "cara default rooms; {}", rooms.body);

        let username_body = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"username":"dentist","rooms":[]}"#),
        );
        assert_eq!(username_body.status, 400, "{}", username_body.body);
        assert!(
            username_body
                .body
                .contains("use name (role label), not username"),
            "{}",
            username_body.body
        );

        for name in ["member", "admin", "owner", "viewer"] {
            let r = http(
                port,
                "POST",
                &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
                Some(&format!(r#"{{"name":"{name}","rooms":[]}}"#)),
            );
            assert_eq!(r.status, 400, "connect name {name}; {}", r.body);
            assert!(
                r.body.contains("Connect role names cannot be skin roles"),
                "{name} {}",
                r.body
            );
        }

        let unknown_cap = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"xray","caps":["tickets:read"],"rooms":[]}"#),
        );
        assert_eq!(unknown_cap.status, 400, "{}", unknown_cap.body);
        assert!(
            unknown_cap.body.contains("unknown capability"),
            "{}",
            unknown_cap.body
        );

        let created = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"dentist","caps":["thread:read","thread:post","files:read"],"rooms":["{sales}"]}}"#
            )),
        );
        assert_eq!(created.status, 200, "{}", created.body);
        let role = json(&created.body);
        assert_eq!(role["name"], "dentist", "{}", created.body);
        assert!(
            role["caps"]
                .as_array()
                .expect("caps")
                .iter()
                .any(|c| c == "files:read"),
            "{}",
            created.body
        );

        let dup = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"Dentist","rooms":[]}"#),
        );
        assert_eq!(dup.status, 400, "{}", dup.body);
        assert!(dup.body.contains("already exists"), "{}", dup.body);

        let get_roles = http(
            port,
            "GET",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(get_roles.status, 200, "{}", get_roles.body);
        let listed = json(&get_roles.body);
        assert_eq!(listed["roles"].as_array().expect("roles").len(), 1);

        let get_twin = http(port, "GET", "/cli/skin/roles/update", None);
        assert_eq!(get_twin.status, 405, "GET twin; {}", get_twin.body);

        let hook = mint_scoped_hook();
        let hook_list = http(port, "GET", &format!("/cli/skin/roles?token={hook}"), None);
        assert_eq!(hook_list.status, 200, "hook GET roles; {}", hook_list.body);
        let hook_post = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={hook}"),
            Some(r#"{"name":"other","rooms":[]}"#),
        );
        assert_eq!(hook_post.status, 403, "{}", hook_post.body);
        assert!(hook_post.body.contains("owner_only"), "{}", hook_post.body);

        let assign = http(
            port,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"bob","role":"dentist"}"#),
        );
        assert_eq!(assign.status, 200, "{}", assign.body);
        let users = http(
            port,
            "GET",
            &format!("/cli/skin/users?token={OWNER_TOKEN}"),
            None,
        );
        let users_v = json(&users.body);
        let bob = users_v["users"]
            .as_array()
            .expect("users")
            .iter()
            .find(|u| u["username"] == "bob")
            .expect("bob");
        assert_eq!(bob["roleName"], "dentist", "{}", users.body);
        assert!(bob["roleId"].as_str().is_some(), "{}", users.body);

        let blocked_rooms = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"bob","rooms":["{sales}"]}}"#)),
        );
        assert_eq!(blocked_rooms.status, 400, "{}", blocked_rooms.body);
        assert!(
            blocked_rooms
                .body
                .contains("guest 'bob' has role 'dentist'"),
            "{}",
            blocked_rooms.body
        );

        let login = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        assert_eq!(login.status, 200, "{}", login.body);
        let lv = json(&login.body);
        let sess = lv["token"].as_str().expect("token").to_string();
        assert_eq!(lv["role"], "dentist", "{}", login.body);
        let login_caps = lv["caps"].as_array().expect("caps");
        assert!(
            login_caps.iter().any(|c| c == "files:read"),
            "{}",
            login.body
        );
        let listed_tokens = http(
            port,
            "GET",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            None,
        );
        assert!(
            json(&listed_tokens.body)["tokens"]
                .as_array()
                .expect("tokens")
                .is_empty(),
            "list hides session: {}",
            listed_tokens.body
        );

        let fs = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={sess}&workspace={sales}&path=."),
            None,
        );
        assert_eq!(fs.status, 200, "daemon files for role session; {}", fs.body);

        let grid = http(
            port,
            "GET",
            &format!("/cli/sessions/grid?token={sess}&session=nope"),
            None,
        );
        assert_eq!(grid.status, 403, "grid; {}", grid.body);
        assert_eq!(grid.body.trim(), TERMINAL_403);

        let plat = mint(port, "bob", &["thread:read"], &[&sales]);
        let plat_before = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={}&workspace={sales}&path=.", plat.1),
            None,
        );
        assert_missing_cap(&plat_before, "files:read");

        let no_write_yet = http(
            port,
            "POST",
            &format!("/cli/fs/write-file?token={sess}"),
            Some(&format!(
                r#"{{"workspace":"{sales}","path":"notes.md","content":"n"}}"#
            )),
        );
        assert_missing_cap(&no_write_yet, "files:write");

        let updated = http(
            port,
            "POST",
            &format!("/cli/skin/roles/update?token={OWNER_TOKEN}"),
            Some(
                r#"{"name":"dentist","caps":["thread:read","thread:post","files:read","files:write"]}"#,
            ),
        );
        assert_eq!(updated.status, 200, "{}", updated.body);
        let wrote = http(
            port,
            "POST",
            &format!("/cli/fs/write-file?token={sess}"),
            Some(&format!(
                r#"{{"workspace":"{sales}","path":"notes.md","content":"n"}}"#
            )),
        );
        assert_eq!(
            wrote.status, 200,
            "live session gained files:write; {}",
            wrote.body
        );
        let plat_after = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={}&workspace={sales}&path=.", plat.1),
            None,
        );
        assert_missing_cap(&plat_after, "files:read");

        let dark = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={OWNER_TOKEN}"),
            Some(r#"{"name":"dark","rooms":[]}"#),
        );
        assert_eq!(dark.status, 200, "{}", dark.body);
        let assign_dark = http(
            port,
            "POST",
            &format!("/cli/skin/roles/assign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"cara","role":"dark"}"#),
        );
        assert_eq!(assign_dark.status, 200, "{}", assign_dark.body);
        let cara_login = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"cara","password":"s3cret-horse"}"#),
        );
        assert_eq!(cara_login.status, 200, "{}", cara_login.body);
        let cara_tok = json(&cara_login.body)["token"]
            .as_str()
            .expect("cara token")
            .to_string();
        let cara_thread = http(
            port,
            "GET",
            &format!("/cli/thread?token={cara_tok}&addr={sales}"),
            None,
        );
        assert_skin_room(&cara_thread);

        let rm_assigned = http(
            port,
            "POST",
            &format!("/cli/skin/roles/remove?token={OWNER_TOKEN}"),
            Some(r#"{"name":"dentist"}"#),
        );
        assert_eq!(rm_assigned.status, 400, "{}", rm_assigned.body);
        assert!(
            rm_assigned
                .body
                .contains("role 'dentist' is assigned; unassign guests first"),
            "{}",
            rm_assigned.body
        );
        let still = http(
            port,
            "GET",
            &format!("/cli/fs/read-dir?token={sess}&workspace={sales}&path=."),
            None,
        );
        assert_eq!(
            still.status, 200,
            "session still resolves after failed remove; {}",
            still.body
        );

        let unassign = http(
            port,
            "POST",
            &format!("/cli/skin/roles/unassign?token={OWNER_TOKEN}"),
            Some(r#"{"username":"bob"}"#),
        );
        assert_eq!(unassign.status, 200, "{}", unassign.body);
        let rooms_again = http(
            port,
            "POST",
            &format!("/cli/skin/users/rooms?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"bob","rooms":["{sales}"]}}"#)),
        );
        assert_eq!(rooms_again.status, 200, "{}", rooms_again.body);
        let rm = http(
            port,
            "POST",
            &format!("/cli/skin/roles/remove?token={OWNER_TOKEN}"),
            Some(r#"{"name":"dentist"}"#),
        );
        assert_eq!(rm.status, 200, "{}", rm.body);

        let bob_login2 = http(
            port,
            "POST",
            "/cli/skin/login",
            Some(r#"{"username":"bob","password":"s3cret-horse"}"#),
        );
        let bob2 = json(&bob_login2.body);
        assert!(bob2["role"].is_null(), "{}", bob_login2.body);
        let bob2_caps = bob2["caps"].as_array().expect("caps");
        assert!(
            !bob2_caps.iter().any(|c| c == "files:read"),
            "unassigned Thread-only; {}",
            bob_login2.body
        );

        let still_username = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"username":"vercel","rooms":["x"]}"#),
        );
        assert_eq!(still_username.status, 400, "{}", still_username.body);
        assert!(
            still_username
                .body
                .contains("use name (platform label), not username"),
            "{}",
            still_username.body
        );
        let empty_plat = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(r#"{"name":"emptyplat","rooms":[]}"#),
        );
        assert_eq!(empty_plat.status, 400, "{}", empty_plat.body);

        let _ = std::fs::remove_dir_all(&sales_dir);
    });
}

fn set_agents_manage_skin(port: u16, project: &str, enable: i64) -> Resp {
    http(
        port,
        "POST",
        &format!("/cli/agents-manage-skin?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"project":"{project}","enable":{enable}}}"#)),
    )
}

fn assert_owner_only(r: &Resp, label: &str) {
    assert_eq!(r.status, 403, "{label}; {}", r.body);
    assert!(
        r.body.contains("owner_only"),
        "{label} must teach owner_only: {}",
        r.body
    );
    assert!(
        !r.body.contains("Invalid or missing auth token"),
        "{label} must not look like a broken passport: {}",
        r.body
    );
    assert!(
        !r.body.contains("\"gated\""),
        "{label} must stay owner_only not gated: {}",
        r.body
    );
}

fn assert_classic_forbidden(r: &Resp, label: &str) {
    assert_eq!(r.status, 403, "{label}; {}", r.body);
    assert!(
        r.body.contains("Invalid or missing auth token"),
        "{label} must stay classic forbidden: {}",
        r.body
    );
    assert!(
        !r.body.contains("owner_only"),
        "{label} must not be owner_only: {}",
        r.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skin_agents_can_manage_skin_toggle_gates_mutations() {
    let _g = lock();
    with_temp_home(|| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let sales_handle = format!("skinsales{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let other_handle = format!("skinother{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (sales_id, _) = seed_thread_addr(&sales_handle);
        let (other_id, _) = seed_thread_addr(&other_handle);

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let col: i64 = conn
                .query_row(
                    "SELECT agents_can_manage_skin FROM projects WHERE id = ?1",
                    params![sales_id],
                    |r| r.get(0),
                )
                .expect("column on k2so.db projects");
            assert_eq!(col, 0, "existing projects row must default 0");
        }

        add_user(port, "stay");
        let skin_db = k2_core::paths::k2_home().join("skin.db");
        assert!(skin_db.exists(), "skin.db exists after roster write");
        let skin_conn = rusqlite::Connection::open(&skin_db).expect("open skin.db");
        let mut stmt = skin_conn
            .prepare("SELECT name FROM pragma_table_info('principals')")
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .expect("cols")
            .filter_map(Result::ok)
            .collect();
        assert!(
            !cols.iter().any(|c| c == "agents_can_manage_skin"),
            "drizzle 0115 is k2so.db projects, not skin.db: {cols:?}"
        );

        let hook_a = mint_scoped_hook_for(&sales_id);
        let hook_b = mint_scoped_hook_for(&other_id);

        let list_off = http(
            port,
            "GET",
            &format!("/cli/skin/users?token={hook_a}"),
            None,
        );
        assert_eq!(
            list_off.status, 200,
            "GET list stays ungated toggle-off; {}",
            list_off.body
        );

        let owner_add = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={OWNER_TOKEN}"),
            Some(r#"{"username":"ownerbob"}"#),
        );
        assert_eq!(owner_add.status, 200, "owner works toggle-off; {}", owner_add.body);

        let mutate_off = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={hook_a}"),
            Some(r#"{"username":"bob"}"#),
        );
        assert_owner_only(&mutate_off, "default OFF agent mutate");
        assert!(
            mutate_off
                .body
                .contains("Allow this agent to manage Skin Access"),
            "manage OFF hint must name the Agent-tab toggle: {}",
            mutate_off.body
        );

        let missing = http(port, "POST", "/cli/skin/users", Some(r#"{"username":"x"}"#));
        assert_classic_forbidden(&missing, "missing token mutate");
        let garbage = http(
            port,
            "POST",
            "/cli/skin/users?token=not-a-real-passport",
            Some(r#"{"username":"x"}"#),
        );
        assert_classic_forbidden(&garbage, "garbage token mutate");

        let get_toggle = http(port, "GET", "/cli/agents-manage-skin", None);
        assert_eq!(
            get_toggle.status, 405,
            "GET toggle writer 405; {}",
            get_toggle.body
        );

        let agent_toggle = http(
            port,
            "POST",
            &format!("/cli/agents-manage-skin?token={hook_a}"),
            Some(&format!(r#"{{"project":"{sales_id}","enable":1}}"#)),
        );
        assert_owner_only(&agent_toggle, "agent cannot flip the toggle");

        let member = provision_role(port, "skinmember2", "hunter2-strong-9", "member");
        let member_toggle = http(
            port,
            "POST",
            &format!("/cli/agents-manage-skin?token={member}"),
            Some(&format!(r#"{{"project":"{sales_id}","enable":1}}"#)),
        );
        assert_classic_forbidden(&member_toggle, "Connect member toggle");
        let member_mutate = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={member}"),
            Some(r#"{"username":"frommember"}"#),
        );
        assert_classic_forbidden(&member_mutate, "Connect member skin mutate");

        let ws_set = http(
            port,
            "POST",
            &format!("/cli/workspace/set?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"project":"{sales_id}","fields":{{"agents_can_manage_skin":1}}}}"#
            )),
        );
        assert_eq!(ws_set.status, 400, "workspace/set unknown field; {}", ws_set.body);
        assert!(
            ws_set.body.contains("unknown setting field")
                || ws_set.body.contains("unknown setting"),
            "must name the unknown field: {}",
            ws_set.body
        );
        {
            let db = k2_core::db::shared();
            let col: i64 = db
                .lock()
                .query_row(
                    "SELECT agents_can_manage_skin FROM projects WHERE id = ?1",
                    params![sales_id],
                    |r| r.get(0),
                )
                .expect("column");
            assert_eq!(col, 0, "workspace/set must not write the column");
        }

        let on = set_agents_manage_skin(port, &sales_id, 1);
        assert_eq!(on.status, 200, "owner enable; {}", on.body);
        let on_v = json(&on.body);
        assert_eq!(on_v["agentsCanManageSkin"], true, "{}", on.body);

        let add = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={hook_a}"),
            Some(r#"{"username":"bob"}"#),
        );
        assert_eq!(add.status, 200, "toggle ON user add; {}", add.body);
        let add_v = json(&add.body);
        let actor = add_v["actor"].as_str().expect("actor");
        assert_ne!(actor, "owner-token", "{}", add.body);
        assert_ne!(actor, "owner", "{}", add.body);
        assert!(
            actor.starts_with("agent:"),
            "actor must be agent:<handle>: {}",
            add.body
        );

        let role = http(
            port,
            "POST",
            &format!("/cli/skin/roles?token={hook_a}"),
            Some(&format!(
                r#"{{"name":"dentist","caps":["thread:read"],"rooms":["{sales_handle}"]}}"#
            )),
        );
        assert_eq!(role.status, 200, "toggle ON role create; {}", role.body);
        let role_v = json(&role.body);
        assert_ne!(role_v["actor"], "owner-token", "{}", role.body);

        let assign = http(
            port,
            "POST",
            &format!("/cli/skin/roles/assign?token={hook_a}"),
            Some(r#"{"username":"bob","role":"dentist"}"#),
        );
        assert_eq!(assign.status, 200, "toggle ON user role; {}", assign.body);

        let tok = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={hook_a}"),
            Some(&format!(
                r#"{{"name":"vercel","caps":["thread:read"],"rooms":["{sales_handle}"]}}"#
            )),
        );
        assert_eq!(tok.status, 200, "toggle ON skin-token create; {}", tok.body);
        let tok_v = json(&tok.body);
        let secret = tok_v["token"].as_str().expect("secret once");
        assert!(secret.starts_with("k2skn_"), "{secret}");
        assert_ne!(tok_v["actor"], "owner-token", "{}", tok.body);

        let list_after = http(
            port,
            "GET",
            &format!("/cli/skin-tokens?token={hook_a}"),
            None,
        );
        assert_eq!(list_after.status, 200, "{}", list_after.body);
        assert!(
            !list_after.body.contains(secret),
            "list must not echo the raw secret"
        );

        let cross = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={hook_b}"),
            Some(r#"{"username":"fromb"}"#),
        );
        assert_owner_only(&cross, "hook from B while A is ON");

        let door = http(
            port,
            "POST",
            &format!("/cli/skin/front-door?token={hook_a}"),
            Some(r#"{"mode":"direct"}"#),
        );
        assert_owner_only(&door, "front-door still owner-only with toggle ON");
        assert!(
            !door.body.contains("Allow this agent to manage Skin Access"),
            "leftover keeps today's hint: {}",
            door.body
        );
        let hydra = http(
            port,
            "POST",
            &format!("/cli/skin/hydra?token={hook_a}"),
            Some(r#"{"enabled":true}"#),
        );
        assert_owner_only(&hydra, "hydra still owner-only with toggle ON");
        assert!(
            !hydra.body.contains("Allow this agent to manage Skin Access"),
            "leftover hydra keeps today's hint: {}",
            hydra.body
        );

        let guest_mutate = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={secret}"),
            Some(r#"{"username":"fromguest"}"#),
        );
        assert_classic_forbidden(&guest_mutate, "k2skn_ never manages roster");

        let key = http(
            port,
            "POST",
            &format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
            Some(r#"{"label":"skin-v1-key"}"#),
        );
        assert_eq!(key.status, 200, "mint k2sk_; {}", key.body);
        let k2sk = json(&key.body)["key"].as_str().expect("k2sk_").to_string();
        assert!(k2sk.starts_with("k2sk_"), "{k2sk}");
        let v1_mutate = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={k2sk}"),
            Some(r#"{"username":"fromv1"}"#),
        );
        assert_classic_forbidden(&v1_mutate, "/v1 k2sk_ never manages");

        let off = set_agents_manage_skin(port, &sales_id, 0);
        assert_eq!(off.status, 200, "owner disable; {}", off.body);
        let after_off = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={hook_a}"),
            Some(r#"{"username":"afteroff"}"#),
        );
        assert_owner_only(&after_off, "toggle OFF again");
    });
}
