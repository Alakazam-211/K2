//! The routes behind `k2 workspace list|create|open|launch|profile|update`
//! admit no agent passport, for the agent's own workspace or any other,
//! through the real dispatcher (temp HOME, a real daemon on a port).
//!
//! None of them is an agent verb (`session_token::is_agent_verb`), so the
//! `/cli/` catch-all's `token_ok` (the owner token or a person's login)
//! refuses a passport with the plain 403. No grant or switch admits one.
//! The owner token reaches each handler. A refused create makes no folder.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;

const OWNER: &str = "ws-passport-routes-owner-token";
const REFUSED: &str = "Invalid or missing auth token";

struct Resp {
    status: u16,
    raw: String,
}

/// One request. `bearer` rides the Authorization header, as the CLI
/// sends it; `None` sends no header.
fn http(port: u16, method: &str, path: &str, bearer: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let len = if method == "POST" { "Content-Length: 0\r\n" } else { "" };
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}{len}\r\n");
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
            let n = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
            if n.is_some_and(|n| rest.len() >= n) {
                break;
            }
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read {method} {path}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no HTTP response to {method} {path}: {text:?}"));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("status code");
    Resp { status, raw: rest.to_string() }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn project_id_for(path: &Path) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM projects WHERE path = ?1", [path.to_string_lossy().to_string()], |r| r.get(0))
        .unwrap_or_else(|e| panic!("{} is not registered: {e}", path.display()))
}

/// The requests the workspace verbs send for one workspace `ws` (a new
/// folder `fresh` for create), as (method, path and query).
fn routes_for(ws: &Path, fresh: &Path) -> Vec<(&'static str, String)> {
    let p = enc(&ws.to_string_lossy());
    vec![
        ("GET", format!("/cli/projects/list?project={p}")),
        ("GET", format!("/cli/agent/list?project={p}")),
        ("POST", format!("/cli/workspace/create?project={p}&path={}", enc(&fresh.to_string_lossy()))),
        ("POST", format!("/cli/workspace/open?project={p}&path={p}")),
        ("GET", format!("/cli/workspace/resolve?project={p}&q={p}")),
        ("POST", format!("/cli/workspace/ensure-canonical-session?project={p}")),
        ("GET", format!("/cli/workspace/agent-display-name?project={p}")),
        ("POST", format!("/cli/agent/update?project={p}&agent=acme&field=role&value=Lead")),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_routes_refuse_an_agent_passport_for_every_workspace() {
    let t = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let home = dunce::canonicalize(t.path()).expect("canonical home");
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // The owner makes two workspaces: the agent's own and another one.
    let own = home.join("ws-own");
    let other = home.join("ws-other");
    for ws in [&own, &other] {
        let r = http(port, "POST", &format!("/cli/workspace/create?path={}&no_wiki=1&no_agents_md=1", enc(&ws.to_string_lossy())), Some(OWNER));
        assert_eq!(r.status, 200, "owner create {}: {}", ws.display(), r.raw);
    }
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-ws-passport",
        HookPrincipal { workspace_uuid: project_id_for(&own), agent_address: "ws-passport-agent".into() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    assert!(session_token::validate_hook(&passport).is_some(), "the passport is live");
    let who = http(port, "GET", "/cli/whoami", Some(passport.as_str()));
    assert!(who.status != 403 && !who.raw.contains(REFUSED), "the passport passes auth on an agent verb: {} {}", who.status, who.raw);

    // The passport, on its own workspace and on the other: refused, plain 403.
    for (label, ws, fresh) in [("own", &own, home.join("ws-new-own")), ("other", &other, home.join("ws-new-other"))] {
        for (method, path) in routes_for(ws, &fresh) {
            assert!(!session_token::is_agent_verb(path.split('?').next().expect("path")), "{path} is an agent verb");
            let r = http(port, method, &path, Some(passport.as_str()));
            assert_eq!(r.status, 403, "passport, {label} workspace, {method} {path}: {}", r.raw);
            let body: serde_json::Value = serde_json::from_str(&r.raw).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.raw));
            assert_eq!(body["error"], REFUSED, "passport, {label} workspace, {method} {path}: {}", r.raw);
            // `?token=` (older clients) is the same passport, the same answer.
            let q = format!("{path}&token={passport}");
            let r = http(port, method, &q, None);
            assert_eq!((r.status, r.raw.contains(REFUSED)), (403, true), "passport ?token=, {label}, {method} {path}: {}", r.raw);
        }
        assert!(!fresh.exists(), "a refused create made {}", fresh.display());
    }

    // The owner token reaches every handler (a handler's own 400 for an
    // agent or workspace that isn't there is not a refusal). Its
    // ensure-canonical-session names a folder that isn't a workspace, so
    // nothing starts; its open adds a folder that isn't one yet.
    let fresh = home.join("ws-new-owner");
    let to_open = home.join("ws-to-open");
    std::fs::create_dir_all(&to_open).expect("mkdir ws-to-open");
    let none = enc(&home.join("ws-none").to_string_lossy());
    for (method, path) in routes_for(&other, &fresh) {
        let path = if path.starts_with("/cli/workspace/ensure-canonical-session") {
            format!("/cli/workspace/ensure-canonical-session?project={none}")
        } else if path.starts_with("/cli/workspace/open") {
            format!("/cli/workspace/open?path={}&no_wiki=1&no_agents_md=1", enc(&to_open.to_string_lossy()))
        } else {
            path
        };
        let r = http(port, method, &path, Some(OWNER));
        assert!(r.status != 403 && !r.raw.contains(REFUSED), "owner {method} {path}: {} {}", r.status, r.raw);
        if !path.starts_with("/cli/workspace/ensure-canonical-session") && !path.starts_with("/cli/agent/update") {
            assert_eq!(r.status, 200, "owner {method} {path}: {}", r.raw);
        }
    }
    assert!(fresh.is_dir(), "the owner's create made {}", fresh.display());
    project_id_for(&to_open);
}
