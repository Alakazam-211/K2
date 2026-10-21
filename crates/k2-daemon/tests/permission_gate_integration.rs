//! Permission cards lane B (prd-agent-settings-cards-v1 rev 3.1 §2; vs-live
//! C1, C8): the two switches and the gate-aware route tables, end to end on
//! a real daemon (TCP port, tunnel ingress and an agent's cell socket).
//!
//! Its own test binary on purpose: it flips the process-wide global switch,
//! which would race every other test that walks the route tables or
//! `/boot-status`. One test function, so nothing in here races either.
//!
//! What it proves:
//! - **Off is today.** With the global switch off, the family answers each
//!   (transport × caller × method) cell exactly as the off-goldens captured
//!   it (`tests/golden/permission_off/daemon/unknown/`), and it answers the
//!   same again after a flip on and back off, and with the agent's own
//!   switch left on while the server's is off.
//! - **On, the rows are live.** The 405 guard admits a POST, a GET is never
//!   refused before auth, a login meets the row's floor (no
//!   `route_unclassified`), and the passport reaches the cell's dispatch (the
//!   family is an agent verb).
//! - **The switches.** Only the Owner turns the global one on; the flip lists
//!   `permission-cards-v1` on `/boot-status`, publishes a settings frame and
//!   writes a `permission-switch` audit line. The per-agent one is written
//!   through `/cli/agent-access/set {toggle: permission_cards}` by an Owner
//!   or Admin login (a Member gets the route floor), is never in Project
//!   JSON, and `enabled_for` needs both.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const OWNER: &str = "owner-token-permission-gate";

struct Resp {
    status_line: String,
    content_type: String,
    body: String,
}

fn parse_resp(text: &str) -> Resp {
    let status_line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}")).trim_end().to_string();
    assert!(status_line.starts_with("HTTP/1."), "not an HTTP response: {text:?}");
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text, ""));
    let content_type = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-type").then(|| v.trim().to_string())
        })
        .unwrap_or_default();
    Resp { status_line, content_type, body: body.to_string() }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
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
    parse_resp(&String::from_utf8_lossy(&out))
}

async fn tcp(port: u16, method: &str, path_q: &str, token: Option<&str>, body: Option<&str>) -> Resp {
    let pq = match token {
        Some(t) => {
            let sep = if path_q.contains('?') { '&' } else { '?' };
            format!("{path_q}{sep}token={}", enc(t))
        }
        None => path_q.to_string(),
    };
    let method = method.to_string();
    let body = body.map(str::to_string);
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn cell(sock: &Path, method: &str, path_q: &str, bearer: &str, body: Option<&str>) -> Resp {
    let raw = match body {
        Some(b) => format!(
            "{method} {path_q} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_q} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\n\r\n"),
    };
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write request");
    stream.flush().await.ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    parse_resp(&String::from_utf8_lossy(&buf))
}

fn render(label: &str, method: &str, path: &str, r: &Resp) -> String {
    format!("## {label} {method} {path}\n{}\ncontent-type: {}\n{}\n", r.status_line, r.content_type, r.body)
}

fn json(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", r.status_line, r.body))
}

/// A recording sink for the daemon's hook events (the settings frame).
struct Sink(Arc<Mutex<Vec<(String, Value)>>>);
impl k2_core::agent_hooks::AgentHookEventSink for Sink {
    fn emit(&self, event: k2_core::agent_hooks::HookEvent, payload: Value) {
        self.0.lock().unwrap().push((event.event_name().to_string(), payload));
    }
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Every `/cli/permission/*` cell the off-goldens pin, rendered. The same
/// callers, transports and methods; compared with itself across states and
/// spot-checked against the golden fixtures.
#[allow(clippy::too_many_arguments)]
async fn off_matrix(
    port: u16,
    tunnel: u16,
    sock: &Path,
    passport: &str,
    callers: &[(&str, Option<String>)],
) -> String {
    let mut text = String::new();
    for path in ["/cli/permission/request", "/cli/permission/enabled", "/cli/permission/pending", "/cli/permission/approve"] {
        for method in ["GET", "POST", "PUT", "DELETE"] {
            let body = (method != "GET").then_some(r#"{"action":"compute.use","why":"x"}"#);
            for (kind, tok) in callers {
                let r = tcp(port, method, path, tok.as_deref(), body).await;
                text.push_str(&render(&format!("tcp {kind}"), method, path, &r));
                let r = tcp(tunnel, method, path, tok.as_deref(), body).await;
                text.push_str(&render(&format!("tunnel {kind}"), method, path, &r));
            }
            let r = cell(sock, method, path, passport, body).await;
            text.push_str(&render("cell passport", method, path, &r));
        }
    }
    text
}

fn features(r: &Resp) -> Vec<String> {
    json(r)["features"]
        .as_array()
        .unwrap_or_else(|| panic!("no features: {}", r.body))
        .iter()
        .map(|f| f.as_str().expect("feature string").to_string())
        .collect()
}

fn audit_lines(event: &str) -> Vec<Value> {
    k2_core::auth_audit::tail(500)
        .expect("read the access audit")
        .into_iter()
        .filter(|l| l["event"] == event)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gate_follows_both_switches_and_off_is_today() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2pgi{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    let events = Arc::new(Mutex::new(Vec::new()));
    k2_core::agent_hooks::set_sink(Box::new(Sink(events.clone())));

    let d = test_harness::start(OWNER).await;
    let (port, tunnel) = (d.port, d.tunnel_port);
    assert!(!k2_core::permission::gate::server_on(), "the global switch defaults off");

    // One workspace, its agent on its own cell socket.
    let ws_path = home.join("ws-pgi-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'pgi-billing', ?2, 'pgi-billing')",
            rusqlite::params![ws_id, ws_path.to_string_lossy()],
        )
        .unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-pgi",
        HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "pgi-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(Duration::from_millis(80)).await;

    let mut sessions = std::collections::BTreeMap::new();
    for (user, role) in [
        ("pgi-owner", k2_core::connect_users::Role::Owner),
        ("pgi-admin", k2_core::connect_users::Role::Admin),
        ("pgi-member", k2_core::connect_users::Role::Member),
    ] {
        k2_core::connect_users::add_user(user, "correct-horse-battery-staple-1").expect("add user");
        k2_core::connect_users::set_role(user, role).expect("set role");
        sessions.insert(user, k2_core::connect_users::create_session(user));
    }
    let mut callers: Vec<(&str, Option<String>)> = vec![
        ("none", None),
        ("bad-token", Some("k2-bad-token-permission-gate".to_string())),
        ("owner-token", Some(OWNER.to_string())),
        ("passport", Some(passport.clone())),
    ];
    for (user, tok) in &sessions {
        callers.push((user, Some(tok.clone())));
    }

    // ── 1. off: today's unknown-route answers ─────────────────────────
    let off = off_matrix(port, tunnel, &sock, &passport, &callers).await;
    let golden = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/permission_off/daemon/unknown/permission_request.txt"),
    )
    .expect("off-golden fixture");
    for (label, method) in [("tcp passport", "POST"), ("cell passport", "POST"), ("tcp pog-member", "GET"), ("tcp owner-token", "POST")] {
        let golden_case = golden
            .split("## ")
            .find(|c| c.starts_with(&format!("{label}\n> {method} /cli/permission/request\n")))
            .unwrap_or_else(|| panic!("golden has {label} {method}"));
        let want_answer = golden_case.lines().skip(3).collect::<Vec<_>>().join("\n");
        let mine = label.replace("pog-member", "pgi-member");
        let got_case = off
            .split("## ")
            .find(|c| c.starts_with(&format!("{mine} {method} /cli/permission/request\n")))
            .unwrap_or_else(|| panic!("matrix has {mine} {method}"));
        let got_answer = got_case.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(got_answer.trim_end(), want_answer.trim_end(), "{label} {method} differs from the off-golden");
    }
    let r = tcp(port, "GET", "/boot-status", None, None).await;
    assert!(!features(&r).contains(&"permission-cards-v1".to_string()), "listed while off: {}", r.body);

    // ── 2. the global switch: Owner only ──────────────────────────────
    for user in ["pgi-admin", "pgi-member"] {
        let r = tcp(port, "POST", "/cli/settings/update", Some(&sessions[user]), Some(r#"{"permissionCards":true}"#)).await;
        assert!(r.status_line.contains("403"), "{user}: {} {}", r.status_line, r.body);
        assert!(!k2_core::permission::gate::server_on(), "{user} opened the gate");
    }
    events.lock().unwrap().clear();
    let r = tcp(port, "POST", "/cli/settings/update", Some(&sessions["pgi-owner"]), Some(r#"{"permissionCards":true}"#)).await;
    assert!(r.status_line.contains("200"), "{} {}", r.status_line, r.body);
    assert_eq!(json(&r)["permissionCards"], true);
    assert!(k2_core::permission::gate::server_on(), "the Owner's write opens the gate at once");
    let frames: Vec<Value> = events.lock().unwrap().iter().filter(|(e, _)| e == "sync:settings").map(|(_, p)| p.clone()).collect();
    assert!(
        frames.iter().any(|p| p["permissionCards"] == true && p["feature"] == "permission-cards-v1"),
        "the flip publishes a settings frame: {frames:?}"
    );
    let r = tcp(port, "GET", "/boot-status", None, None).await;
    assert!(features(&r).contains(&"permission-cards-v1".to_string()), "not listed while on: {}", r.body);
    assert!(
        audit_lines("permission-switch").iter().any(|l| l["outcome"] == "server on"),
        "a permission-switch audit line: {:?}",
        audit_lines("permission-switch")
    );
    // Re-writing the same value is not a flip: no second frame.
    events.lock().unwrap().clear();
    let r = tcp(port, "POST", "/cli/settings/update", Some(OWNER), Some(r#"{"permissionCards":true}"#)).await;
    assert!(r.status_line.contains("200"), "{}", r.body);
    assert!(
        !events.lock().unwrap().iter().any(|(_, p)| p.get("permissionCards").is_some()),
        "no flip, no permission frame"
    );

    // ── 3. on: the rows are live (agent switch still off) ─────────────
    let r = tcp(port, "POST", "/cli/permission/request", Some(OWNER), Some("{}")).await;
    assert_ne!(r.body, r#"{"error":"method not allowed for this route"}"#, "the 405 guard admits it while on");
    let r = tcp(port, "GET", "/cli/permission/request", Some(OWNER), None).await;
    assert_ne!(r.body, r#"{"error":"POST required"}"#, "the family keeps its method guard after auth");
    let r = tcp(port, "GET", "/cli/permission/pending", Some(&sessions["pgi-member"]), None).await;
    assert_eq!(json(&r)["error"], "role_required", "a Member meets the row's Admin floor: {}", r.body);
    let r = tcp(port, "GET", "/cli/permission/pending", Some(&sessions["pgi-admin"]), None).await;
    assert_ne!(json(&r)["error"], "route_unclassified", "an Admin passes the table: {}", r.body);
    let r = cell(&sock, "POST", "/cli/permission/request", &passport, Some("{}")).await;
    assert_ne!(
        r.body, r#"{"error":"Invalid or missing auth token"}"#,
        "the family is an agent verb while on, so the cell dispatches it"
    );

    // ── 4. the per-agent switch ───────────────────────────────────────
    let set = |v: bool| format!(r#"{{"toggle":"permission_cards","workspace":"pgi-billing","value":{v}}}"#);
    let r = tcp(port, "POST", "/cli/agent-access/set", Some(&sessions["pgi-member"]), Some(&set(true))).await;
    assert_eq!(json(&r)["error"], "role_required", "a Member: {}", r.body);
    let r = cell(&sock, "POST", "/cli/agent-access/set", &passport, Some(&set(true))).await;
    assert_eq!(r.body, r#"{"error":"Invalid or missing auth token"}"#, "an agent never sets its own switch");
    assert!(!k2_core::permission::gate::enabled_for(&ws_id));
    let r = tcp(port, "POST", "/cli/agent-access/set", Some(&sessions["pgi-admin"]), Some(&set(true))).await;
    assert!(r.status_line.contains("200"), "an Admin: {} {}", r.status_line, r.body);
    assert_eq!(json(&r)["agentsCanRequestPermissions"], true);
    assert!(k2_core::workspace::settings::agents_can_request_permissions(&ws_id));
    assert!(k2_core::permission::gate::enabled_for(&ws_id), "both on");
    let r = tcp(port, "POST", "/cli/agent-access/set", Some(OWNER), Some(r#"{"toggle":"nope","workspace":"pgi-billing","value":true}"#)).await;
    assert!(json(&r).to_string().contains("'permission_cards'"), "the toggle list names it: {}", r.body);
    assert!(
        audit_lines("permission-switch").iter().any(|l| l["outcome"] == "agent pgi-billing on"),
        "{:?}",
        audit_lines("permission-switch")
    );
    // Never in Project JSON (C8).
    let r = tcp(port, "GET", "/cli/projects/list", Some(OWNER), None).await;
    assert!(r.status_line.contains("200"), "{}", r.body);
    assert!(!r.body.contains("RequestPermissions") && !r.body.contains("request_permissions"), "{}", r.body);

    // ── 5. back off, with the agent's switch left on: today again ─────
    events.lock().unwrap().clear();
    let r = tcp(port, "POST", "/cli/settings/update", Some(OWNER), Some(r#"{"permissionCards":false}"#)).await;
    assert!(r.status_line.contains("200"), "{}", r.body);
    assert!(!k2_core::permission::gate::server_on());
    assert!(!k2_core::permission::gate::enabled_for(&ws_id), "server off, agent on → off");
    assert!(events.lock().unwrap().iter().any(|(_, p)| p["permissionCards"] == false), "the off flip publishes a frame too");
    let r = tcp(port, "GET", "/boot-status", None, None).await;
    assert!(!features(&r).contains(&"permission-cards-v1".to_string()));
    let again = off_matrix(port, tunnel, &sock, &passport, &callers).await;
    assert_eq!(again, off, "server off with the agent on answers exactly as before any switch was touched");
    // And with the agent's switch back off.
    let r = tcp(port, "POST", "/cli/agent-access/set", Some(OWNER), Some(&set(false))).await;
    assert!(r.status_line.contains("200"), "{}", r.body);
    let again = off_matrix(port, tunnel, &sock, &passport, &callers).await;
    assert_eq!(again, off);
    // The env force-on opens it without the setting.
    std::env::set_var(k2_core::permission::ENV_FORCE_ON, "1");
    let r = tcp(port, "GET", "/boot-status", None, None).await;
    assert!(features(&r).contains(&"permission-cards-v1".to_string()), "K2_PERMISSION_CARDS=1 forces it on");
    std::env::remove_var(k2_core::permission::ENV_FORCE_ON);
    drop(d);
}
