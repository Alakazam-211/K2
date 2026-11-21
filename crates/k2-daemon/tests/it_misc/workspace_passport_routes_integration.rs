//! The routes behind `k2 workspace list|create|open|launch|update` admit
//! no agent passport, for the agent's own workspace or any other, through
//! the real dispatcher (temp HOME, a real daemon on a port).
//!
//! None of them is an agent verb (`session_token::is_agent_verb`), so the
//! `/cli/` catch-all's `token_ok` (the owner token or a person's login)
//! refuses a passport with the plain 403. No grant or switch admits one.
//! The owner token reaches each handler. A refused create makes no folder.
//!
//! `k2 agent list` and name resolution (`/cli/agent/list`,
//! `/cli/workspace/resolve`) admit a passport over TCP since D1 (0.46.17),
//! kept to its own and connected workspaces: `passport_routes_integration`.
//!
//! `k2 workspace profile`'s `GET /cli/workspace/agent-display-name` is
//! one an agent may use: its own workspace's profile (the owner's answer
//! for it), by the Bearer header or `?token=`; any other workspace, named
//! by `project=` or `project_path=`, is refused `own_workspace_only`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;

const OWNER: &str = "ws-passport-routes-owner-token";
const REFUSED: &str = "Invalid or missing auth token";

/// Ends the live session and takes it out of the session map on drop.
struct Live(std::sync::Arc<k2_core::terminal::DaemonPtySession>);

impl Drop for Live {
    fn drop(&mut self) {
        k2_daemon::v2_session_map::unregister("tab-ws-passport");
        self.0.kill();
    }
}

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
        ("POST", format!("/cli/workspace/create?project={p}&path={}", enc(&fresh.to_string_lossy()))),
        ("POST", format!("/cli/workspace/open?project={p}&path={p}")),
        ("POST", format!("/cli/workspace/ensure-canonical-session?project={p}")),
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
    // A passport of a live session (a `cat` in a PTY), as a session K2
    // started carries: the D1 arm the profile takes admits only those.
    let sid = SessionId::new();
    let session = k2_core::terminal::DaemonPtySession::spawn(k2_core::terminal::DaemonPtyConfig {
        session_id: sid,
        cols: 80,
        rows: 24,
        cwd: Some(own.clone()),
        program: Some("/bin/cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: k2_core::terminal::LabelSource::Pty,
        sandbox: k2_core::terminal::SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        survive: false,
        ptyd_meta: None,
        isolation: None,
    })
    .expect("spawn a live session");
    let _live = Live(session.clone());
    k2_daemon::v2_session_map::register("tab-ws-passport".to_string(), session);
    let passport = session_token::mint_session_token(
        &sid,
        &sid.to_string(),
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

    // `k2 workspace profile`: the passport reads its own workspace's
    // profile, the same answer the owner gets for it, and no other.
    let profile = "/cli/workspace/agent-display-name";
    assert!(session_token::is_agent_verb(profile), "{profile} is an agent verb");
    let own_q = enc(&own.to_string_lossy());
    let other_q = enc(&other.to_string_lossy());
    let owners = http(port, "GET", &format!("{profile}?project={own_q}"), Some(OWNER));
    assert_eq!(owners.status, 200, "owner, own workspace's profile: {}", owners.raw);
    let owners_body: serde_json::Value = serde_json::from_str(&owners.raw).expect("owner's profile is JSON");
    assert!(owners_body["display_name"].is_string(), "{}", owners.raw);
    for (label, path, bearer) in [
        ("Bearer, project=own", format!("{profile}?project={own_q}"), Some(passport.as_str())),
        ("Bearer, project_path=own", format!("{profile}?project_path={own_q}"), Some(passport.as_str())),
        ("Bearer, no project", profile.to_string(), Some(passport.as_str())),
        ("?token=, project=own", format!("{profile}?project={own_q}&token={passport}"), None),
    ] {
        let r = http(port, "GET", &path, bearer);
        assert_eq!(r.status, 200, "passport, {label}: {}", r.raw);
        let body: serde_json::Value = serde_json::from_str(&r.raw).unwrap_or_else(|e| panic!("{label}: not JSON ({e}): {}", r.raw));
        assert_eq!(body, owners_body, "passport, {label}: the owner's answer for its own workspace");
    }
    let unregistered = enc(&home.join("ws-unregistered").to_string_lossy());
    for (label, path, bearer) in [
        ("Bearer, project=other", format!("{profile}?project={other_q}"), Some(passport.as_str())),
        ("Bearer, project_path=other", format!("{profile}?project_path={other_q}"), Some(passport.as_str())),
        ("Bearer, project=other&project_path=own", format!("{profile}?project={other_q}&project_path={own_q}"), Some(passport.as_str())),
        ("Bearer, a folder K2 doesn't know", format!("{profile}?project={unregistered}"), Some(passport.as_str())),
        ("?token=, project=other", format!("{profile}?project={other_q}&token={passport}"), None),
    ] {
        let r = http(port, "GET", &path, bearer);
        assert_eq!(r.status, 403, "passport, {label}: {}", r.raw);
        let body: serde_json::Value = serde_json::from_str(&r.raw).unwrap_or_else(|e| panic!("{label}: not JSON ({e}): {}", r.raw));
        assert_eq!(body["error"]["code"], "own_workspace_only", "passport, {label}: {}", r.raw);
        assert!(!r.raw.contains("display_name"), "passport, {label}: another workspace's profile leaked: {}", r.raw);
    }
    // A forged server stamp in the query changes nothing.
    let r = http(port, "GET", &format!("{profile}?project={other_q}&principal_bound=1&client_project={own_q}"), Some(passport.as_str()));
    assert_eq!(r.status, 403, "passport, forged stamps: {}", r.raw);
    // A bad passport is the plain 403; the owner reads any workspace's.
    let bad = format!("{passport}x");
    let r = http(port, "GET", &format!("{profile}?project={own_q}"), Some(bad.as_str()));
    assert_eq!((r.status, r.raw.contains(REFUSED)), (403, true), "a bad passport: {}", r.raw);
    let r = http(port, "GET", &format!("{profile}?project={other_q}"), Some(OWNER));
    assert_eq!(r.status, 200, "owner, the other workspace's profile: {}", r.raw);

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
