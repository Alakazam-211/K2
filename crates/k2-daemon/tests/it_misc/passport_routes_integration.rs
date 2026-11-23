//! D1 (0.46.17): an agent's passport over TCP, through the real dispatcher
//! (temp HOME, a real daemon on a port), on the routes agents are meant to
//! use (`passport_routes::ROUTES`, the Zen reads).
//!
//! For a sample across families: the passport is admitted and kept to its
//! own workspace (the same answer the owner gets for it); naming another
//! workspace is refused, or, where the route looks a workspace up, allowed
//! only for a connected one; the owner token still reaches every handler,
//! for any workspace. A sample of owner-only routes still refuses it.
//!
//! D1b: tickets as the agent (`k2 tickets ask|list|show|comment|resolve|
//! assign` and the rules read): its own workspace's tickets only, its own
//! name on what it writes, `canEdit: false` on the rules, and the owner
//! token unchanged.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;
use serde_json::Value as J;

const OWNER: &str = "d1-passport-routes-owner-token";
const REFUSED: &str = "Invalid or missing auth token";

struct Resp {
    status: u16,
    raw: String,
}

impl Resp {
    fn json(&self, what: &str) -> J {
        serde_json::from_str(&self.raw).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {}", self.raw))
    }
}

/// One request; `bearer` rides the Authorization header as the CLI sends it.
fn http(port: u16, method: &str, path: &str, bearer: Option<&str>) -> Resp {
    http_body(port, method, path, bearer, "")
}

/// [`http`] with a request body (POST).
fn http_body(port: u16, method: &str, path: &str, bearer: Option<&str>, body: &str) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let len = if method == "POST" { format!("{ctype}Content-Length: {}\r\n", body.len()) } else { String::new() };
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}{len}\r\n{body}");
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

fn ticket(project_id: &str, title: &str) -> String {
    k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: project_id.to_string(),
        session_id: None,
        session_kind: None,
        agent_name: "d1-agent".into(),
        kind: "question".into(),
        title: title.into(),
        body: Some("a body".into()),
        options: None,
        priority: 3,
    })
    .unwrap_or_else(|e| panic!("file ticket {title}: {e}"))
    .id
}

fn code(r: &Resp, what: &str) -> String {
    let v = r.json(what);
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("{what}: no error code: {}", r.raw)).to_string()
}

/// A live session in the session map (a `cat` in a PTY, a K2 terminal),
/// ended and unregistered on drop.
struct Live {
    key: String,
    session: std::sync::Arc<k2_core::terminal::DaemonPtySession>,
}

impl Drop for Live {
    fn drop(&mut self) {
        k2_daemon::v2_session_map::unregister(&self.key);
        self.session.kill();
    }
}

/// A passport for a live session under map key `key` in workspace `ws_id`,
/// as a session K2 started carries.
fn live_passport(cwd: &Path, ws_id: &str, key: &str, address: &str) -> (String, Live) {
    let sid = SessionId::new();
    let session = k2_core::terminal::DaemonPtySession::spawn(k2_core::terminal::DaemonPtyConfig {
        session_id: sid,
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
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
    })
    .expect("spawn a live session");
    k2_daemon::v2_session_map::register(key.to_string(), session.clone());
    let passport = session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws_id.to_string(), agent_address: address.to_string() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    assert!(session_token::validate_hook(&passport).is_some(), "the passport is valid");
    (passport, Live { key: key.to_string(), session })
}

struct Env {
    port: u16,
    tunnel_port: u16,
    passport: String,
    _live: Live,
    own: String,
    other: String,
    peer: String,
    own_id: String,
    other_id: String,
    peer_id: String,
}

async fn env(t: &k2_core::test_env::TempHome) -> (Env, test_harness::TestDaemon) {
    k2_core::db::init_for_tests();
    let home = dunce::canonicalize(t.path()).expect("canonical home");
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let mut paths = Vec::new();
    for name in ["d1-own", "d1-other", "d1-peer"] {
        let ws = home.join(name);
        let r = http(port, "POST", &format!("/cli/workspace/create?path={}&no_wiki=1&no_agents_md=1", enc(&ws.to_string_lossy())), Some(OWNER));
        assert_eq!(r.status, 200, "owner create {}: {}", ws.display(), r.raw);
        paths.push(ws);
    }
    let ids: Vec<String> = paths.iter().map(|p| project_id_for(p)).collect();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceRelation::create(&conn, &uuid::Uuid::new_v4().to_string(), &ids[0], &ids[2], "collaborator")
            .expect("connect own and peer");
    }
    let (passport, live) = live_passport(&paths[0], &ids[0], "tab-d1-agent", "d1-agent");
    let s = |p: &Path| p.to_string_lossy().to_string();
    let e = Env {
        port,
        tunnel_port: d.tunnel_port,
        passport,
        _live: live,
        own: s(&paths[0]),
        other: s(&paths[1]),
        peer: s(&paths[2]),
        own_id: ids[0].clone(),
        other_id: ids[1].clone(),
        peer_id: ids[2].clone(),
    };
    (e, d)
}

/// Reads keyed by `project=`: the passport reads its own workspace (the
/// owner's answer for it, by Bearer and by `?token=`, with or without
/// `project=`), is refused another, and a forged stamp changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_workspace_reads_are_admitted_and_scoped() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let (own, other) = (enc(&e.own), enc(&e.other));
    // context list, feed, tickets list, wiki status, settings, reviews.
    for route in ["/cli/context/layers", "/cli/feed", "/cli/feedback/list", "/cli/wiki/status", "/cli/settings", "/cli/agents/list"] {
        let owners = http(e.port, "GET", &format!("{route}?project={own}"), Some(OWNER));
        assert_eq!(owners.status, 200, "owner {route} own: {}", owners.raw);
        let want = owners.json(route);
        for (label, path, bearer) in [
            ("Bearer, project=own", format!("{route}?project={own}"), Some(e.passport.as_str())),
            ("Bearer, no project", route.to_string(), Some(e.passport.as_str())),
            ("?token=, project=own", format!("{route}?project={own}&token={}", e.passport), None),
        ] {
            let r = http(e.port, "GET", &path, bearer);
            assert_eq!(r.status, 200, "passport {route}, {label}: {}", r.raw);
            assert_eq!(r.json(route), want, "passport {route}, {label}: the owner's answer for its own workspace");
        }
        for (label, path) in [
            ("project=other", format!("{route}?project={other}")),
            ("project_path=other", format!("{route}?project_path={other}")),
            ("project=other by id", format!("{route}?project={}", e.other_id)),
            ("connected, still not its own", format!("{route}?project={}", enc(&e.peer))),
            ("forged stamps", format!("{route}?project={other}&principal_bound=1&project_id={}&client_project={own}", e.other_id)),
        ] {
            let r = http(e.port, "GET", &path, Some(e.passport.as_str()));
            assert_eq!(r.status, 403, "passport {route}, {label}: {}", r.raw);
            assert_eq!(code(&r, route), "own_workspace_only", "passport {route}, {label}");
        }
        let r = http(e.port, "GET", &format!("{route}?project={other}"), Some(OWNER));
        assert_eq!(r.status, 200, "owner {route} other: {}", r.raw);
    }
    // A bad passport gets the plain 403 the route always gave.
    let bad = format!("{}x", e.passport);
    let r = http(e.port, "GET", &format!("/cli/context/layers?project={own}"), Some(bad.as_str()));
    assert_eq!((r.status, r.raw.contains(REFUSED)), (403, true), "a bad passport: {}", r.raw);
}

/// Tickets by id: its own workspace's ticket, never another's (the answer
/// an unknown id gets); the owner reads both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tickets_show_is_kept_to_its_own_workspace() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let mine = ticket(&e.own_id, "Own question");
    let theirs = ticket(&e.other_id, "Other question");
    let r = http(e.port, "GET", &format!("/cli/feedback/list?project={}", enc(&e.own)), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "{}", r.raw);
    let ids: Vec<String> = r.json("list")["items"].as_array().expect("items").iter().map(|i| i["id"].as_str().expect("id").to_string()).collect();
    assert_eq!(ids, vec![mine.clone()], "its own workspace's tickets only: {}", r.raw);
    let r = http(e.port, "GET", &format!("/cli/feedback/show?id={mine}"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "own ticket: {}", r.raw);
    assert_eq!(r.json("show")["title"], "Own question", "{}", r.raw);
    let unknown = http(e.port, "GET", "/cli/feedback/show?id=ffffffffffff", Some(e.passport.as_str()));
    for id in [theirs.as_str(), &theirs[..8]] {
        let r = http(e.port, "GET", &format!("/cli/feedback/show?id={id}"), Some(e.passport.as_str()));
        assert_eq!(r.status, unknown.status, "another workspace's ticket {id}: {}", r.raw);
        assert_eq!(code(&r, "show"), "not_found", "{}", r.raw);
        assert!(!r.raw.contains("Other question"), "another workspace's ticket leaked: {}", r.raw);
    }
    for id in [&mine, &theirs] {
        let r = http(e.port, "GET", &format!("/cli/feedback/show?id={id}"), Some(OWNER));
        assert_eq!(r.status, 200, "owner show {id}: {}", r.raw);
    }
}

/// Lookups by `q=` (agent conf, workspace resolve): its own and a connected
/// workspace, never one it isn't connected to. The agent list narrows to
/// the same set. The owner sees every workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_lookups_reach_own_and_connected_workspaces_only() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    for route in ["/cli/agent/conf", "/cli/workspace/resolve"] {
        for (label, q) in [("own", &e.own), ("connected", &e.peer)] {
            let owners = http(e.port, "GET", &format!("{route}?q={}", enc(q)), Some(OWNER));
            assert_eq!(owners.status, 200, "owner {route} {label}: {}", owners.raw);
            let r = http(e.port, "GET", &format!("{route}?q={}", enc(q)), Some(e.passport.as_str()));
            assert_eq!(r.status, 200, "passport {route} {label}: {}", r.raw);
            assert_eq!(r.json(route), owners.json(route), "passport {route} {label}");
        }
        for q in [e.other.clone(), e.other_id.clone()] {
            let r = http(e.port, "GET", &format!("{route}?q={}", enc(&q)), Some(e.passport.as_str()));
            assert_eq!(r.status, 404, "passport {route} not connected ({q}): {}", r.raw);
            assert_eq!(code(&r, route), "not_found", "{}", r.raw);
            assert!(!r.raw.contains(&e.other), "the other workspace's path leaked: {}", r.raw);
            let r = http(e.port, "GET", &format!("{route}?q={}", enc(&q)), Some(OWNER));
            assert_eq!(r.status, 200, "owner {route} other: {}", r.raw);
        }
    }
    let ids = |r: &Resp| -> Vec<String> {
        let mut v: Vec<String> = r.json("agent list")["agents"].as_array().expect("agents").iter().map(|a| a["id"].as_str().expect("id").to_string()).collect();
        v.sort();
        v
    };
    let r = http(e.port, "GET", "/cli/agent/list", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "{}", r.raw);
    let mut want = vec![e.own_id.clone(), e.peer_id.clone()];
    want.sort();
    assert_eq!(ids(&r), want, "own and connected only: {}", r.raw);
    let r = http(e.port, "GET", "/cli/agent/list", Some(OWNER));
    assert!(ids(&r).contains(&e.other_id), "the owner sees every workspace: {}", r.raw);
}

/// A shared read (presets) and a Zen read are admitted; Zen's person-only
/// reads still refuse the passport.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_and_zen_reads_are_admitted() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let owners = http(e.port, "GET", "/cli/presets/list", Some(OWNER));
    assert_eq!(owners.status, 200, "{}", owners.raw);
    let r = http(e.port, "GET", "/cli/presets/list", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.json("presets"), owners.json("presets"));
    // The settings copy a Member login reads, never the owner's full one.
    let r = http(e.port, "GET", "/cli/settings/get", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "{}", r.raw);
    let passports = r.json("settings");
    let mut redacted = http(e.port, "GET", "/cli/settings/get", Some(OWNER)).json("settings");
    k2_daemon::settings_routes::redact_settings_value(&mut redacted);
    assert_eq!(passports, redacted, "the passport gets the redacted copy");

    let r = http_body(e.port, "POST", "/cli/zen/setup", Some(OWNER), "{}");
    assert_eq!(r.status, 200, "zen setup: {}", r.raw);
    for route in ["/cli/zen/status", "/cli/zen/validate", "/cli/zen/gardens"] {
        let r = http(e.port, "GET", route, Some(e.passport.as_str()));
        assert_eq!(r.status, 200, "passport {route}: {}", r.raw);
    }
    for route in ["/cli/zen/history", "/cli/zen/sync", "/cli/zen/get"] {
        let r = http(e.port, "GET", route, Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "passport {route}: {}", r.raw);
        assert_eq!(r.json(route)["error"], "zen_local_only", "{}", r.raw);
    }
}

/// Writes (`k2 done`, `k2 review approve`): its own workspace, recorded as
/// that workspace's; another workspace and a path for a name are refused
/// and change nothing; the owner still reaches the handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_run_in_its_own_workspace_only() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let (own, other) = (enc(&e.own), enc(&e.other));
    let r = http(e.port, "POST", &format!("/cli/done?project={own}&agent=d1-agent"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "passport done: {}", r.raw);
    assert_eq!(r.json("done")["event"], "task.done", "{}", r.raw);
    let feed = http(e.port, "GET", &format!("/cli/feed?project={own}"), Some(OWNER));
    assert!(feed.raw.contains("task.done"), "the done is in its own workspace's feed: {}", feed.raw);

    for (label, path) in [
        ("done, other", format!("/cli/done?project={other}&agent=d1-agent")),
        ("review approve, other", format!("/cli/review/approve?project={other}&branch=agent%2Fx%2Fy&agent=x")),
        ("agent complete, a path for the file", format!("/cli/agent/complete?project={own}&agent=x&file=..%2F..%2Fsecret.md")),
    ] {
        let r = http(e.port, "POST", &path, Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "passport {label}: {}", r.raw);
        assert_eq!(code(&r, label), "own_workspace_only", "{label}");
    }
    let feed = http(e.port, "GET", &format!("/cli/feed?project={other}"), Some(OWNER));
    assert!(!feed.raw.contains("task.done"), "a refused done wrote nothing: {}", feed.raw);

    // The owner reaches the handlers for any workspace (review approve's
    // own 400: the folder isn't a git repository).
    let r = http(e.port, "POST", &format!("/cli/done?project={other}&agent=d1-agent"), Some(OWNER));
    assert_eq!(r.status, 200, "owner done other: {}", r.raw);
    let r = http(e.port, "POST", &format!("/cli/review/approve?project={other}&branch=agent%2Fx%2Fy&agent=x"), Some(OWNER));
    assert_eq!(r.status, 400, "owner review approve: {}", r.raw);
    // `x` is no skill of its workspace, so whose branch it is can't be told:
    // a person's step (the card), never the handler.
    let r = http(e.port, "POST", &format!("/cli/review/approve?project={own}&branch=agent%2Fx%2Fy&agent=x"), Some(e.passport.as_str()));
    assert_eq!((r.status, code(&r, "approve")), (403, "merge_needs_person".to_string()), "passport review approve, own workspace: {}", r.raw);
}

/// Owner-only routes keep refusing a passport, for its own workspace too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_only_routes_still_refuse_a_passport() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = enc(&e.own);
    for (method, path) in [
        ("GET", "/cli/tunnel/subdomains".to_string()),
        ("GET", "/cli/companion/projects".to_string()),
        ("GET", "/cli/tunnel/status".to_string()),
        ("GET", "/cli/projects/list".to_string()),
        ("POST", format!("/cli/workspace/set?project={own}&key=api_skip_permissions&value=1")),
        ("POST", format!("/cli/workspace/set-agent-display-name?project={own}&name=Renamed")),
        ("POST", format!("/cli/trust-folders?project={own}")),
        ("POST", "/cli/feedback/answer".to_string()),
        ("POST", "/cli/context/add".to_string()),
        ("POST", "/cli/agents/hire-root".to_string()),
    ] {
        let p = path.split('?').next().expect("path");
        assert!(!k2_daemon::passport_routes::is_route(p), "{p} is admitted");
        let r = http(e.port, method, &path, Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "passport {method} {path}: {}", r.raw);
    }
    let r = http(e.port, "GET", "/cli/zen/history", Some(e.passport.as_str()));
    assert_eq!(r.status, 403, "{}", r.raw);
}

/// Never through the tunnel: the same passport that is admitted locally
/// gets the route's own refusal when the request came in through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passport_through_the_tunnel_is_refused() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    for path in [
        format!("/cli/agent/conf?q={}", enc(&e.own)),
        format!("/cli/context/layers?project={}", enc(&e.own)),
        "/cli/presets/list".to_string(),
        // `k2 workspace profile`, which took a passport through the tunnel
        // before D1.
        format!("/cli/workspace/agent-display-name?project={}", enc(&e.own)),
    ] {
        let r = http(e.port, "GET", &path, Some(e.passport.as_str()));
        assert_eq!(r.status, 200, "locally, {path}: {}", r.raw);
        let r = http(e.tunnel_port, "GET", &path, Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "through the tunnel, {path}: {}", r.raw);
        assert!(r.raw.contains(REFUSED), "through the tunnel, {path}: the route's own refusal: {}", r.raw);
    }
}

/// Cards on for the agent's workspace, off again when the test ends.
struct CardsOn;
impl CardsOn {
    fn for_workspace(ws_id: &str) -> Self {
        k2_core::permission::gate::set_server_switch(true);
        k2_core::workspace::settings::set_agents_can_request_permissions(ws_id, true).expect("the agent's switch");
        CardsOn
    }
}
impl Drop for CardsOn {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

/// A git repo in `ws` with one commit on its current branch and, for each
/// name, a branch one commit ahead. Returns (base commit, branch tips).
/// Made by the real git CLI (`k2_core::git_fixture`).
fn repo_with_branches(ws: &Path, branches: &[&str]) -> (String, Vec<String>) {
    use k2_core::git_fixture as fx;
    fx::init_at(ws);
    std::fs::write(ws.join("base.txt"), "base\n").expect("base.txt");
    let base = fx::commit_all(ws, "base");
    let main = fx::git(ws, &["symbolic-ref", "--short", "HEAD"]);
    let tips = branches
        .iter()
        .map(|b| {
            fx::git(ws, &["checkout", "-q", "-b", b, &base]);
            std::fs::write(ws.join(format!("{}.txt", b.replace('/', "-"))), b.as_bytes()).expect("branch file");
            let tip = fx::commit_all(ws, b);
            fx::git(ws, &["checkout", "-q", &main]);
            tip
        })
        .collect();
    (base, tips)
}

/// Check `branch` out in a worktree at `at` (an agent's worktree).
fn add_worktree(ws: &Path, branch: &str, at: &Path) {
    k2_core::git_fixture::git(ws, &["worktree", "add", "-q", at.to_str().expect("utf8"), branch]);
}

fn head_of(ws: &Path) -> String {
    k2_core::git_fixture::git(ws, &["rev-parse", "HEAD"])
}

fn branch_tip(ws: &Path, b: &str) -> Option<String> {
    let out = k2_core::git_fixture::git_cmd(ws).args(["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")]).output().expect("rev-parse");
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A merge of the agent's own work is a person's: approving its own branch,
/// or one whose author can't be told, and `commit --merge` answer
/// `merge_needs_person` with the permission card, and nothing is merged.
/// A plain commit is a person's too (`commit_needs_person`, the
/// `git.commit` card); the owner's runs as before. Another agent's branch
/// in its workspace is admitted and merged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_own_merge_is_a_card_for_a_person() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let _cards = CardsOn::for_workspace(&e.own_id);
    let own = enc(&e.own);
    let ws = Path::new(&e.own);
    // `backend` is a skill of its workspace (another agent there).
    let skill = k2_core::workspace::agent_identity::skills_dir(&e.own).join("backend");
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(skill.join("SKILL.md"), "---\nname: backend\nrole: dev\n---\nBackend.\n").expect("skill");
    let (base, tips) = repo_with_branches(ws, &["agent/d1-agent/own-task", "feature/no-author", "agent/backend/task", "agent/backend/no-worktree"]);
    let home = ws.parent().expect("home");
    add_worktree(ws, "agent/backend/task", &home.join("wt-backend-task"));

    for (label, agent, branch) in [
        ("its own branch", "d1-agent", "agent/d1-agent/own-task"),
        ("an author that can't be told", "backend", "feature/no-author"),
        ("agent= and the branch disagree", "d1-agent", "agent/backend/task"),
        ("another agent's branch that isn't its worktree branch", "backend", "agent/backend/no-worktree"),
    ] {
        let path = format!("/cli/review/approve?project={own}&agent={}&branch={}", enc(agent), enc(branch));
        let r = http(e.port, "POST", &path, Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "{label}: {}", r.raw);
        let v = r.json(label);
        assert_eq!(v["error"]["code"], "merge_needs_person", "{label}: {v}");
        assert_eq!(v["error"]["branch"], branch, "{label}: {v}");
        assert_eq!(v["permission"]["action"], "git.review-approve", "{label}: the card: {v}");
        let command = v["permission"]["command"].as_str().unwrap_or_else(|| panic!("{label}: no command: {v}"));
        assert!(command.starts_with("k2 permission request git.review-approve "), "{label}: {command}");
        assert!(command.contains(&format!("branch={branch}")), "{label}: {command}");
        assert_eq!(head_of(ws), base, "{label}: nothing merged");
    }
    assert_eq!(branch_tip(ws, "agent/d1-agent/own-task"), Some(tips[0].clone()), "its own branch is still there");

    let r = http(e.port, "POST", &format!("/cli/commit-merge?project={own}&message=wip"), Some(e.passport.as_str()));
    assert_eq!(r.status, 403, "commit --merge: {}", r.raw);
    let v = r.json("commit-merge");
    assert_eq!((v["error"]["code"].as_str(), v["permission"]["action"].as_str()), (Some("merge_needs_person"), Some("git.commit-merge")), "{v}");
    // A plain commit starts a person's agent preset: its own card.
    let r = http(e.port, "POST", &format!("/cli/commit?project={own}&message=wip"), Some(e.passport.as_str()));
    assert_eq!(r.status, 403, "a plain commit: {}", r.raw);
    let v = r.json("commit");
    assert_eq!((v["error"]["code"].as_str(), v["permission"]["action"].as_str()), (Some("commit_needs_person"), Some("git.commit")), "{v}");
    let command = v["permission"]["command"].as_str().unwrap_or_else(|| panic!("commit: no command: {v}"));
    assert!(command.starts_with("k2 permission request git.commit ") && command.contains("message=wip"), "{command}");
    // The owner's commit runs as before.
    let r = http(e.port, "POST", &format!("/cli/commit?project={own}&message=wip"), Some(OWNER));
    assert_eq!(r.status, 200, "the owner's commit: {}", r.raw);
    assert_eq!(r.json("owner commit")["action"], "commit", "{}", r.raw);

    // Another agent of its workspace: admitted, merged.
    let path = format!("/cli/review/approve?project={own}&agent=backend&branch={}", enc("agent/backend/task"));
    let r = http(e.port, "POST", &path, Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "another agent's branch: {}", r.raw);
    assert_eq!(head_of(ws), tips[2], "merged (fast-forward)");
    assert_eq!(branch_tip(ws, "agent/backend/task"), None, "the merged branch is removed");
}

fn feed(e: &Env) -> Vec<J> {
    let r = http(e.port, "GET", &format!("/cli/feed?project={}&limit=200", enc(&e.own)), Some(OWNER));
    assert_eq!(r.status, 200, "feed: {}", r.raw);
    r.json("feed")["feed"].as_array().unwrap_or_else(|| panic!("no feed: {}", r.raw)).clone()
}

fn entries(e: &Env, kind: &str) -> Vec<(String, String)> {
    feed(e)
        .iter()
        .filter(|f| f["type"] == kind)
        .map(|f| (f["actor"].as_str().unwrap_or("").to_string(), f["to"].as_str().unwrap_or("").to_string()))
        .collect()
}

/// `k2 done` records the passport's own name: absent takes it, the same
/// name passes, another name (or a path) is refused and records nothing.
/// A review the passport makes records it as the reviewer, on the feed and
/// in the feedback file reject and feedback write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_actor_is_the_passport() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = enc(&e.own);
    for (label, q) in [("no agent=", String::new()), ("agent= its own name", "&agent=d1-agent".to_string())] {
        let before = entries(&e, "task.done").len();
        let r = http(e.port, "POST", &format!("/cli/done?project={own}{q}"), Some(e.passport.as_str()));
        assert_eq!(r.status, 200, "{label}: {}", r.raw);
        let after = entries(&e, "task.done");
        assert_eq!(after.len(), before + 1, "{label}: one record");
        assert!(after.iter().all(|(actor, _)| actor == "d1-agent"), "{label}: the passport's name: {after:?}");
    }
    let before = feed(&e).len();
    for (label, q) in [("another name", "agent=backend"), ("a path", "agent=..%2F..%2Fx")] {
        let r = http(e.port, "POST", &format!("/cli/done?project={own}&{q}"), Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "{label}: {}", r.raw);
        let v = r.json(label);
        assert_eq!((v["error"]["code"].as_str(), v["error"]["agent"].as_str()), (Some("not_this_agent"), Some("d1-agent")), "{label}: {v}");
    }
    assert_eq!(feed(&e).len(), before, "a refused done records nothing");
    // The owner still names the agent.
    let r = http(e.port, "POST", &format!("/cli/done?project={own}&agent=backend"), Some(OWNER));
    assert_eq!(r.status, 200, "owner: {}", r.raw);
    assert!(entries(&e, "task.done").iter().any(|(actor, _)| actor == "backend"), "the owner's done keeps its agent");

    // Reviews of another agent: the passport is the reviewer.
    let skill = k2_core::workspace::agent_identity::skills_dir(&e.own).join("backend");
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(skill.join("SKILL.md"), "---\nname: backend\nrole: dev\n---\nBackend.\n").expect("skill");
    let ws = Path::new(&e.own);
    let (_, tips) = repo_with_branches(ws, &["agent/backend/task"]);
    add_worktree(ws, "agent/backend/task", &ws.parent().expect("home").join("wt-actor-backend"));
    let r = http(e.port, "POST", &format!("/cli/review/approve?project={own}&agent=backend&branch={}", enc("agent/backend/task")), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "approve another agent's branch: {}", r.raw);
    assert_eq!(head_of(ws), tips[0], "merged");
    // The feedback file names the reviewer the feed records. Reject and
    // feedback write the same day's file, so each is read, then removed.
    let inbox = k2_core::workspace::scheduler::agent_work_dir(&e.own, "backend", "inbox");
    let review_file = |what: &str, body: &str| -> String {
        let files: Vec<_> = std::fs::read_dir(&inbox)
            .unwrap_or_else(|err| panic!("{what}: no inbox at {}: {err}", inbox.display()))
            .map(|f| f.expect("inbox entry").path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("review-feedback-")))
            .collect();
        assert_eq!(files.len(), 1, "{what}: one feedback file: {files:?}");
        let text = std::fs::read_to_string(&files[0]).expect("read the feedback file");
        std::fs::remove_file(&files[0]).expect("remove the feedback file");
        assert!(text.contains(body), "{what}: {text}");
        let by: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("assigned_by: ")).collect();
        assert_eq!(by.len(), 1, "{what}: one assigned_by line: {text}");
        by[0].to_string()
    };
    let r = http(e.port, "POST", &format!("/cli/review/feedback?project={own}&agent=backend&feedback=tighten"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "feedback: {}", r.raw);
    assert_eq!(review_file("feedback", "tighten"), "d1-agent", "feedback: the file names the passport");
    // Reject writes its file only when there is done work to send back.
    std::fs::create_dir_all(k2_core::workspace::scheduler::agent_work_dir(&e.own, "backend", "done")).expect("done dir");
    let r = http(e.port, "POST", &format!("/cli/review/reject?project={own}&agent=backend&reason=no-tests"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "reject: {}", r.raw);
    assert_eq!(review_file("reject", "no-tests"), "d1-agent", "reject: the file names the passport");
    for kind in ["review.approved", "review.changes-requested", "review.rejected"] {
        assert_eq!(entries(&e, kind), vec![("d1-agent".to_string(), "backend".to_string())], "{kind}: the passport reviewed backend's work");
    }
    // The owner's file still names `reviewer`.
    let r = http(e.port, "POST", &format!("/cli/review/feedback?project={own}&agent=backend&feedback=owner-note"), Some(OWNER));
    assert_eq!(r.status, 200, "owner feedback: {}", r.raw);
    assert_eq!(review_file("owner feedback", "owner-note"), "reviewer", "the owner's file");
}

/// `k2 agent complete` moves the passport's own work item, as `k2 done`
/// records its own name: no `agent=` takes the passport's name, the same
/// name passes, another name (or a path) is refused and moves nothing.
/// The owner still names the agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_complete_is_the_passports_own_work() {
    use k2_core::workspace::scheduler::agent_work_dir;
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = enc(&e.own);
    // No persona, so each agent has its own work folders.
    let persona_dir = k2_core::workspace::agent_identity::workspace_agent_path(&e.own);
    if persona_dir.exists() {
        std::fs::remove_dir_all(&persona_dir).expect("clear the persona");
    }
    let item = |agent: &str, file: &str| {
        let active = agent_work_dir(&e.own, agent, "active");
        std::fs::create_dir_all(&active).expect("active dir");
        std::fs::write(active.join(file), "---\ntitle: t\n---\nwork\n").expect("work item");
    };
    let is_in = |agent: &str, folder: &str, file: &str| agent_work_dir(&e.own, agent, folder).join(file).exists();
    for (label, q, file) in [("no agent=", "", "one.md"), ("agent= its own name", "&agent=d1-agent", "two.md")] {
        item("d1-agent", file);
        let r = http(e.port, "POST", &format!("/cli/agent/complete?project={own}&file={file}{q}"), Some(e.passport.as_str()));
        assert_eq!(r.status, 200, "{label}: {}", r.raw);
        assert_eq!(r.json(label)["agent"], "d1-agent", "{label}: the record names the passport: {}", r.raw);
        assert!(is_in("d1-agent", "done", file) && !is_in("d1-agent", "active", file), "{label}: its own item moved to done");
    }
    item("backend", "theirs.md");
    for (label, agent) in [("another agent", "backend"), ("a path", "..%2F..%2Fx")] {
        let r = http(e.port, "POST", &format!("/cli/agent/complete?project={own}&agent={agent}&file=theirs.md"), Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "{label}: {}", r.raw);
        let v = r.json(label);
        assert_eq!((v["error"]["code"].as_str(), v["error"]["agent"].as_str()), (Some("not_this_agent"), Some("d1-agent")), "{label}: {v}");
    }
    assert!(is_in("backend", "active", "theirs.md") && !is_in("backend", "done", "theirs.md"), "a refused complete moves nothing");
    let r = http(e.port, "POST", &format!("/cli/agent/complete?project={own}&agent=backend&file=theirs.md"), Some(OWNER));
    assert_eq!(r.status, 200, "owner: {}", r.raw);
    assert!(is_in("backend", "done", "theirs.md"), "the owner still names the agent");
}

/// Only a passport of a live, local session: an API cell's, a `k2 job`
/// run's, and one whose session isn't live are refused on the D1 routes
/// and the Zen reads, where the live one is admitted. (A microVM agent:
/// `passport_routes::tests::only_a_live_local_session_is_admitted`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_live_local_passport_is_admitted() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let r = http_body(e.port, "POST", "/cli/zen/setup", Some(OWNER), "{}");
    assert_eq!(r.status, 200, "zen setup: {}", r.raw);
    let own = Path::new(&e.own);
    let (api, _api_live) = live_passport(own, &e.own_id, &format!("api-d1key-{}", uuid::Uuid::new_v4()), "d1-api");
    let mint = |address: &str| {
        session_token::mint_session_token(
            &SessionId::new(),
            "pane-not-live",
            HookPrincipal { workspace_uuid: e.own_id.clone(), agent_address: address.to_string() },
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        )
    };
    let job = mint("job:nightly");
    let gone = mint("d1-agent");
    let conf = format!("/cli/agent/conf?q={}", enc(&e.own));
    // `k2 workspace profile` too, which took any valid passport before D1.
    let profile = format!("/cli/workspace/agent-display-name?project={}", enc(&e.own));
    for route in [conf.as_str(), profile.as_str(), "/cli/presets/list", "/cli/zen/gardens", "/cli/zen/status"] {
        let r = http(e.port, "GET", route, Some(e.passport.as_str()));
        assert_eq!(r.status, 200, "the live passport, {route}: {}", r.raw);
        for (label, p) in [("an API cell", &api), ("a k2 job run", &job), ("no live session", &gone)] {
            assert!(session_token::validate_hook(p).is_some(), "{label}: the passport itself is valid");
            let r = http(e.port, "GET", route, Some(p.as_str()));
            assert_eq!(r.status, 403, "{label}, {route}: {}", r.raw);
        }
    }
}

/// A name more than one workspace answers to (one connected, one not), or
/// a name it can't reach, is a plain not-found for a passport: no handles,
/// no paths, no "did you mean". The owner still gets the candidates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_lookups_never_list_other_workspaces() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for id in [&e.peer_id, &e.other_id] {
            conn.execute("UPDATE projects SET name = 'Globex Team' WHERE id = ?1", [id]).expect("shared display name");
        }
    }
    let owners = http(e.port, "GET", &format!("/cli/agent/conf?q={}", enc("Globex Team")), Some(OWNER));
    assert_eq!(owners.status, 409, "the owner is told it's ambiguous: {}", owners.raw);
    for route in ["/cli/agent/conf", "/cli/workspace/resolve"] {
        for q in ["Globex Team", "Globex", "d1-othr"] {
            let r = http(e.port, "GET", &format!("{route}?q={}", enc(q)), Some(e.passport.as_str()));
            assert_eq!(r.status, 404, "passport {route} {q}: {}", r.raw);
            assert_eq!(code(&r, route), "not_found", "{}", r.raw);
            for leak in [e.other.as_str(), e.peer.as_str(), "d1-other", "d1-peer", "did you mean"] {
                assert!(!r.raw.contains(leak), "passport {route} {q}: {leak} leaked: {}", r.raw);
            }
        }
    }
}

/// Skills by name: a passport names a skill with letters, digits, - and _
/// only, and never reaches the workspace's own agent folder (`.k2/agent/`):
/// "", "." and any name while the workspace has a persona are refused for
/// delete and profile, and the folder stays. Another skill is the agent's
/// to read and remove. Creating a skill is never a passport's (it always
/// writes the workspace's own agent folder): the route's own refusal,
/// with or without a persona, and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_never_reach_the_workspaces_own_agent_folder() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = enc(&e.own);
    let persona_dir = k2_core::workspace::agent_identity::workspace_agent_path(&e.own);
    let skill = k2_core::workspace::agent_identity::skills_dir(&e.own).join("backend");
    // No persona: another skill is the agent's.
    if persona_dir.exists() {
        std::fs::remove_dir_all(&persona_dir).expect("clear the persona");
    }
    std::fs::create_dir_all(&skill).expect("skill dir");
    // A skill caught mid-migration keeps an AGENT.md (what profile reads).
    std::fs::write(skill.join("AGENT.md"), "---\nname: backend\nrole: dev\n---\nBackend.\n").expect("skill");
    let r = http(e.port, "GET", &format!("/cli/agents/profile?project={own}&agent=backend"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "profile of another skill: {}", r.raw);
    assert!(r.raw.contains("Backend."), "{}", r.raw);
    let r = http(e.port, "GET", &format!("/cli/agents/list?project={own}"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "skills list: {}", r.raw);
    assert!(r.raw.contains("backend"), "{}", r.raw);
    let r = http(e.port, "POST", &format!("/cli/skills/regenerate?project={own}"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "skills regenerate: {}", r.raw);
    let r = http(e.port, "POST", &format!("/cli/agents/delete?project={own}&name=backend"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "delete another skill: {}", r.raw);
    assert!(!skill.exists(), "the skill is gone");
    let create_refused = |when: &str| {
        for name in ["", ".", "d1", "backend", "newskill", "a/b"] {
            let r = http(e.port, "POST", &format!("/cli/agents/create?project={own}&name={}&role=x", enc(name)), Some(e.passport.as_str()));
            assert_eq!(r.status, 403, "create {name:?}, {when}: {}", r.raw);
            assert!(r.raw.contains(REFUSED), "create {name:?}, {when}: the route's own refusal: {}", r.raw);
        }
    };
    assert!(!k2_daemon::passport_routes::is_route("/cli/agents/create"), "create is not a passport route");
    create_refused("no persona");
    assert!(!persona_dir.exists(), "a refused create wrote no agent folder");
    assert!(!k2_core::workspace::agent_identity::skills_dir(&e.own).join("newskill").exists(), "a refused create wrote no skill");

    // The workspace's own persona.
    std::fs::create_dir_all(&persona_dir).expect("persona dir");
    std::fs::write(persona_dir.join("AGENT.md"), "---\nname: d1\nrole: lead\n---\nLead.\n").expect("persona");
    create_refused("with a persona");
    for (method, route, key) in [("POST", "/cli/agents/delete", "name"), ("GET", "/cli/agents/profile", "agent")] {
        for name in ["", ".", "..", "d1", "backend", "a/b"] {
            let path = format!("{route}?project={own}&{key}={}&role=x", enc(name));
            let r = http(e.port, method, &path, Some(e.passport.as_str()));
            assert_eq!(r.status, 403, "{route} {key}={name:?}: {}", r.raw);
            assert_eq!(code(&r, route), "own_workspace_only", "{route} {name:?}");
            assert!(!r.raw.contains("Lead."), "{route} {name:?}: the persona leaked: {}", r.raw);
        }
    }
    assert!(persona_dir.join("AGENT.md").exists(), "the workspace's own agent folder stays");
}

/// A workspace folder inside a repo above it: approving even another
/// agent's worktree branch is the card, and the repo above is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approve_in_a_folder_inside_another_repo_is_the_card() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let home = Path::new(&e.own).parent().expect("home").to_path_buf();
    let outer = home.join("outer-repo");
    std::fs::create_dir_all(&outer).expect("outer");
    let (base, _) = repo_with_branches(&outer, &["agent/backend/task"]);
    add_worktree(&outer, "agent/backend/task", &home.join("wt-outer-backend"));
    let inner = outer.join("inner");
    let r = http(e.port, "POST", &format!("/cli/workspace/create?path={}&no_wiki=1&no_agents_md=1", enc(&inner.to_string_lossy())), Some(OWNER));
    assert_eq!(r.status, 200, "owner creates the inner workspace: {}", r.raw);
    let inner_id = project_id_for(&inner);
    let skill = k2_core::workspace::agent_identity::skills_dir(&inner.to_string_lossy()).join("backend");
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(skill.join("SKILL.md"), "---\nname: backend\nrole: dev\n---\nBackend.\n").expect("skill");
    let (passport, _live) = live_passport(&inner, &inner_id, "tab-d1-inner", "d1-inner");
    let path = format!("/cli/review/approve?project={}&agent=backend&branch={}", enc(&inner.to_string_lossy()), enc("agent/backend/task"));
    let r = http(e.port, "POST", &path, Some(passport.as_str()));
    assert_eq!(r.status, 403, "{}", r.raw);
    assert_eq!(code(&r, "approve"), "merge_needs_person", "{}", r.raw);
    assert_eq!(head_of(&outer), base, "the repo above is untouched");
    assert!(branch_tip(&outer, "agent/backend/task").is_some(), "its branch is still there");
}

/// Ticket ids that share a prefix with another workspace's: the passport's
/// own ticket by its unique prefix answers with that ticket; the shared
/// prefix is a plain not-found that lists no ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ticket_show_never_lists_other_workspaces_ids() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let mine = ticket(&e.own_id, "Own question");
    let theirs = ticket(&e.other_id, "Other question");
    let (mine_id, theirs_id) = ("abcd1111-0000-4000-8000-000000000001", "abcd2222-0000-4000-8000-000000000002");
    {
        // Give both tickets ids that share a prefix, in every table that
        // points at a ticket.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let children: Vec<(String, String)> = {
            let mut st = conn
                .prepare("SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f WHERE m.type = 'table' AND f.\"table\" = 'feedback'")
                .expect("foreign keys");
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect("query");
            rows.collect::<Result<_, _>>().expect("rows")
        };
        conn.execute_batch("PRAGMA foreign_keys = OFF").expect("fk off");
        for (new, old) in [(mine_id, mine.as_str()), (theirs_id, theirs.as_str())] {
            conn.execute("UPDATE feedback SET id = ?1 WHERE id = ?2", [new, old]).expect("ticket id");
            for (table, col) in &children {
                conn.execute(&format!("UPDATE \"{table}\" SET \"{col}\" = ?1 WHERE \"{col}\" = ?2"), [new, old]).expect("child id");
            }
        }
        conn.execute_batch("PRAGMA foreign_keys = ON").expect("fk on");
    }
    let r = http(e.port, "GET", "/cli/feedback/show?id=abcd1", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "its own ticket by a unique prefix: {}", r.raw);
    assert_eq!(r.json("show")["id"], mine_id, "{}", r.raw);
    for prefix in ["abcd", "abcd2"] {
        let r = http(e.port, "GET", &format!("/cli/feedback/show?id={prefix}"), Some(e.passport.as_str()));
        assert_eq!(r.status, 404, "{prefix}: {}", r.raw);
        assert_eq!(code(&r, "show"), "not_found", "{}", r.raw);
        assert!(!r.raw.contains(theirs_id) && !r.raw.contains(mine_id), "{prefix}: an id leaked: {}", r.raw);
    }
    let r = http(e.port, "GET", "/cli/feedback/show?id=abcd", Some(OWNER));
    assert_eq!(code(&r, "show"), "ambiguous_id", "the owner is told: {}", r.raw);
}

/// The wiki over TCP: its own workspace's index and note; the map of every
/// workspace (scope=k2, host, fleet) and note ids of another workspace or
/// of that map (`<id>::<note>`, the focus-group and project nodes) are
/// refused. The owner still reads the map.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_reads_stay_in_its_own_workspace() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = enc(&e.own);
    for ws in [&e.own, &e.other] {
        let r = http(e.port, "POST", &format!("/cli/wiki/seed?project={}", enc(ws)), Some(OWNER));
        assert_eq!(r.status, 200, "owner seeds {ws}: {}", r.raw);
    }
    let owners = http(e.port, "GET", &format!("/cli/wiki/index?project={own}"), Some(OWNER));
    assert_eq!(owners.status, 200, "{}", owners.raw);
    let r = http(e.port, "GET", &format!("/cli/wiki/index?project={own}"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "its own index: {}", r.raw);
    assert_eq!(r.json("index"), owners.json("index"), "the owner's answer for its own workspace");
    for scope in ["k2", "host", "fleet", "K2", " fleet "] {
        let r = http(e.port, "GET", &format!("/cli/wiki/index?project={own}&scope={}", enc(scope)), Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "scope={scope:?}: {}", r.raw);
        assert_eq!(code(&r, "index"), "own_workspace_only", "scope={scope:?}");
    }
    for id in [format!("{}::Home.md", e.other_id), format!("{}::Home.md", e.own_id), "__focusgroup__::g1".to_string(), "__project__::p1".to_string(), format!("{}::__workspace__", e.other_id)] {
        let r = http(e.port, "GET", &format!("/cli/wiki/note?project={own}&id={}", enc(&id)), Some(e.passport.as_str()));
        assert_eq!(r.status, 403, "note {id}: {}", r.raw);
        assert_eq!(code(&r, "note"), "own_workspace_only", "note {id}");
    }
    let r = http(e.port, "GET", &format!("/cli/wiki/index?scope=k2&project={own}"), Some(OWNER));
    assert_eq!(r.status, 200, "the owner reads the map: {}", r.raw);
}

// ── D1b: tickets as the agent ─────────────────────────────────────────

/// A ticket write with the passport (JSON body, Bearer as the CLI sends it).
fn post_json(e: &Env, path: &str, bearer: &str, body: J) -> Resp {
    http_body(e.port, "POST", path, Some(bearer), &body.to_string())
}

fn item(e: &Env, id: &str) -> J {
    let r = http(e.port, "GET", &format!("/cli/feedback/show?id={id}"), Some(OWNER));
    assert_eq!(r.status, 200, "owner show {id}: {}", r.raw);
    r.json("show")
}

/// `k2 tickets ask|list|show|comment|resolve|assign` from a session: the
/// passport files in its own workspace under its own name and session (the
/// body names neither, as the CLI sends it), reads and writes its own
/// tickets, and its comment is always its own words, never a person's answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_and_works_its_own_tickets_with_its_passport() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let pass = e.passport.as_str();

    // ask: no project, and its own project named: both file in its own
    // workspace, under the passport's name and session.
    let sid = session_token::validate_hook(pass).expect("the passport is valid").session_id;
    let mut filed = Vec::new();
    for project in [None, Some(e.own.as_str())] {
        let mut body = serde_json::json!({
            "title": "Ship it?",
            "kind": "fyi",
            "assignees": ["owner"],
        });
        if let Some(p) = project {
            body["project"] = serde_json::json!(p);
        }
        let r = post_json(&e, "/cli/feedback/create", pass, body);
        assert_eq!(r.status, 200, "passport ask (project {project:?}): {}", r.raw);
        let v = r.json("ask");
        let id = v["id"].as_str().expect("id").to_string();
        let stored = item(&e, &id);
        assert_eq!(stored["projectId"], e.own_id.as_str(), "filed in its own workspace: {stored}");
        let name = stored["agentName"].as_str().expect("agentName");
        assert!(!name.is_empty() && name != "owner", "asked by the agent: {stored}");
        assert_eq!(stored["sessionId"], sid.as_str(), "the session is the passport's: {stored}");
        assert_eq!(stored["assignees"], serde_json::json!(["owner"]), "{stored}");
        filed.push((id, name.to_string()));
    }
    let (id, name) = filed[0].clone();

    // list and show: its own.
    let r = http(e.port, "GET", "/cli/feedback/list", Some(pass));
    assert_eq!(r.status, 200, "{}", r.raw);
    let listed: Vec<String> = r.json("list")["items"].as_array().expect("items").iter().map(|i| i["id"].as_str().expect("id").to_string()).collect();
    assert!(listed.contains(&id) && listed.contains(&filed[1].0), "its tickets are listed: {}", r.raw);
    let r = http(e.port, "GET", &format!("/cli/feedback/show?id={}", &id[..8]), Some(pass));
    assert_eq!(r.status, 200, "show by prefix: {}", r.raw);

    // comment: an option pick in the body changes nothing; stored under the
    // agent's name, the ticket not answered.
    let r = post_json(&e, "/cli/feedback/comment", pass, serde_json::json!({ "id": &id[..8], "body": "Looking now", "optionPick": true }));
    assert_eq!(r.status, 200, "passport comment: {}", r.raw);
    assert_eq!(r.json("comment")["author"], name.as_str(), "{}", r.raw);
    let stored = item(&e, &id);
    assert_eq!(stored["status"], "waiting", "an agent comment never answers: {stored}");
    assert!(stored["answer"].is_null(), "{stored}");

    // assign: its own ticket.
    let r = post_json(&e, "/cli/feedback/assign", pass, serde_json::json!({ "id": id, "usernames": [] }));
    assert_eq!(r.status, 200, "passport assign: {}", r.raw);
    assert_eq!(item(&e, &id)["assignees"], serde_json::json!([]));

    // resolve: settled with the outcome under its own name, then resolved.
    let r = post_json(&e, "/cli/feedback/resolve", pass, serde_json::json!({ "id": id, "status": "answered", "answer": "Shipped" }));
    assert_eq!(r.status, 200, "passport resolve --answered: {}", r.raw);
    let stored = item(&e, &id);
    assert_eq!((stored["status"].as_str(), stored["answer"].as_str()), (Some("answered"), Some("Shipped")), "{stored}");
    let last = stored["comments"].as_array().expect("comments").last().cloned().expect("a comment");
    assert_eq!(last["author"], name.as_str(), "settled under the agent's name: {stored}");
    let r = post_json(&e, "/cli/feedback/resolve", pass, serde_json::json!({ "id": id }));
    assert_eq!(r.status, 200, "passport resolve: {}", r.raw);
    assert_eq!(item(&e, &id)["status"], "resolved");
}

/// A ticket body names only the passport, as `agent=` does on `done` and
/// `agent complete`: an `agentName` or `sessionId` on ask, or an `author` on
/// comment and resolve, that differs is refused (`not_this_agent`, naming
/// the field) and nothing is written; the same value passes, and so does none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ticket_body_naming_another_agent_or_session_is_refused() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let pass = e.passport.as_str();
    let sid = session_token::validate_hook(pass).expect("the passport is valid").session_id;
    let not_this_agent = |r: &Resp, field: &str, me: &str, what: &str| {
        assert_eq!(r.status, 403, "{what}: {}", r.raw);
        let v = r.json(what);
        assert_eq!(v["error"]["code"], "not_this_agent", "{what}: {}", r.raw);
        assert_eq!(v["error"]["field"], field, "{what}: {}", r.raw);
        assert_eq!(v["error"]["agent"], me, "{what}: {}", r.raw);
        assert!(v["error"]["hint"].as_str().expect("hint").contains("send that name or none"), "{what}: {}", r.raw);
    };

    // No names: filed under the passport's name and session.
    let r = post_json(&e, "/cli/feedback/create", pass, serde_json::json!({ "title": "Plain", "kind": "fyi" }));
    assert_eq!(r.status, 200, "ask, no names: {}", r.raw);
    let id = r.json("ask")["id"].as_str().expect("id").to_string();
    let stored = item(&e, &id);
    let me = stored["agentName"].as_str().expect("agentName").to_string();
    assert!(!me.is_empty(), "{stored}");
    assert_eq!(stored["sessionId"], sid.as_str(), "{stored}");

    // The same name and session pass.
    let r = post_json(&e, "/cli/feedback/create", pass, serde_json::json!({ "title": "Same", "kind": "fyi", "agentName": me, "sessionId": sid }));
    assert_eq!(r.status, 200, "ask, its own names: {}", r.raw);
    let same = item(&e, r.json("ask")["id"].as_str().expect("id"));
    assert_eq!((same["agentName"].as_str(), same["sessionId"].as_str()), (Some(me.as_str()), Some(sid.as_str())), "{same}");

    // Another agent or session: refused, nothing filed.
    for (body, field) in [
        (serde_json::json!({ "title": "Forged", "kind": "fyi", "agentName": "owner" }), "agentName"),
        (serde_json::json!({ "title": "Forged", "kind": "fyi", "agentName": 7 }), "agentName"),
        (serde_json::json!({ "title": "Forged", "kind": "fyi", "sessionId": "forged-session" }), "sessionId"),
        (serde_json::json!({ "title": "Forged", "kind": "fyi", "agentName": me, "sessionId": "forged-session" }), "sessionId"),
    ] {
        let r = post_json(&e, "/cli/feedback/create", pass, body.clone());
        not_this_agent(&r, field, &me, &format!("ask {body}"));
    }
    let r = http(e.port, "GET", &format!("/cli/feedback/list?project={}&all=1", enc(&e.own)), Some(OWNER));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert!(!r.raw.contains("Forged"), "a refused ask filed nothing: {}", r.raw);

    // comment: another author refused and nothing added; its own, or none, passes.
    let was = item(&e, &id);
    let r = post_json(&e, "/cli/feedback/comment", pass, serde_json::json!({ "id": id, "body": "As a person", "author": "owner" }));
    not_this_agent(&r, "author", &me, "comment as owner");
    assert_eq!(item(&e, &id)["comments"], was["comments"], "a refused comment added nothing");
    for body in [serde_json::json!({ "id": id, "body": "Mine", "author": me }), serde_json::json!({ "id": id, "body": "Mine too" })] {
        let r = post_json(&e, "/cli/feedback/comment", pass, body.clone());
        assert_eq!(r.status, 200, "comment {body}: {}", r.raw);
        assert_eq!(r.json("comment")["author"], me.as_str(), "{}", r.raw);
    }

    // resolve --answered: another author refused and the ticket unchanged;
    // its own name passes.
    let was = item(&e, &id);
    let r = post_json(&e, "/cli/feedback/resolve", pass, serde_json::json!({ "id": id, "status": "answered", "answer": "Done", "author": "backend" }));
    not_this_agent(&r, "author", &me, "resolve as backend");
    let now = item(&e, &id);
    for key in ["status", "answer", "comments"] {
        assert_eq!(now[key], was[key], "{key} unchanged: {now}");
    }
    let r = post_json(&e, "/cli/feedback/resolve", pass, serde_json::json!({ "id": id, "status": "answered", "answer": "Done", "author": me }));
    assert_eq!(r.status, 200, "resolve as itself: {}", r.raw);
    assert_eq!(item(&e, &id)["status"], "answered");
}

/// Another workspace's ticket answers as an unknown id does, for every
/// ticket verb, and nothing changes on it. Filing in another workspace,
/// a person's steps and `answer` are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_is_refused_on_another_workspaces_ticket() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let pass = e.passport.as_str();
    let theirs = ticket(&e.other_id, "Other question");
    let peers = ticket(&e.peer_id, "Peer question");
    let before = [item(&e, &theirs), item(&e, &peers)];
    let unknown = post_json(&e, "/cli/feedback/comment", pass, serde_json::json!({ "id": "ffffffffffff", "body": "x" }));
    assert_eq!((unknown.status, code(&unknown, "unknown")), (404, "not_found".to_string()), "{}", unknown.raw);
    for id in [theirs.as_str(), &theirs[..8], peers.as_str()] {
        for (path, body) in [
            ("/cli/feedback/comment", serde_json::json!({ "id": id, "body": "mine now" })),
            ("/cli/feedback/resolve", serde_json::json!({ "id": id })),
            ("/cli/feedback/resolve", serde_json::json!({ "id": id, "status": "answered", "answer": "done" })),
            ("/cli/feedback/assign", serde_json::json!({ "id": id, "usernames": ["owner"] })),
        ] {
            let r = post_json(&e, path, pass, body);
            assert_eq!(r.status, unknown.status, "passport {path} {id}: {}", r.raw);
            assert_eq!(code(&r, path), "not_found", "passport {path} {id}");
            assert!(!r.raw.contains("question"), "another workspace's ticket leaked: {}", r.raw);
        }
        let r = http(e.port, "GET", &format!("/cli/feedback/show?id={id}"), Some(pass));
        assert_eq!(code(&r, "show"), "not_found", "{}", r.raw);
    }
    for (id, was) in [&theirs, &peers].into_iter().zip(before) {
        let stored = item(&e, id);
        for key in ["status", "comments", "assignees", "answer", "updatedAt"] {
            assert_eq!(stored[key], was[key], "{key} unchanged: {stored}");
        }
    }
    let r = http(e.port, "GET", &format!("/cli/feedback/list?project={}", enc(&e.other)), Some(pass));
    assert_eq!((r.status, code(&r, "list")), (403, "own_workspace_only".to_string()), "{}", r.raw);

    // ask in another workspace (by path, by id, a connected one): refused,
    // nothing filed there.
    for project in [e.other.as_str(), e.other_id.as_str(), e.peer.as_str()] {
        let r = post_json(&e, "/cli/feedback/create", pass, serde_json::json!({ "project": project, "title": "Elsewhere", "kind": "fyi" }));
        assert_eq!(r.status, 403, "passport ask in {project}: {}", r.raw);
        assert_eq!(code(&r, "ask"), "own_workspace_only", "{project}");
    }
    for ws in [&e.other, &e.peer] {
        let r = http(e.port, "GET", &format!("/cli/feedback/list?project={}&all=1", enc(ws)), Some(OWNER));
        assert!(!r.raw.contains("Elsewhere"), "a refused ask filed nothing: {}", r.raw);
    }

    // Its own ticket: dismiss, plan and reopen are a person's; answer too.
    let mine = ticket(&e.own_id, "Own question");
    let was = item(&e, &mine);
    for status in ["dismissed", "planned", "waiting", "needs_discussion"] {
        let r = post_json(&e, "/cli/feedback/resolve", pass, serde_json::json!({ "id": mine, "status": status }));
        assert_eq!((r.status, code(&r, status)), (403, "person_only".to_string()), "{status}: {}", r.raw);
    }
    let r = post_json(&e, "/cli/feedback/answer", pass, serde_json::json!({ "id": mine, "answer": "yes" }));
    assert_eq!((r.status, r.raw.contains("invalid or missing token")), (403, true), "answer: {}", r.raw);
    let stored = item(&e, &mine);
    for key in ["status", "comments", "answer", "updatedAt"] {
        assert_eq!(stored[key], was[key], "{key} unchanged: {stored}");
    }

    // Through the tunnel the passport misses the arm, as on every D1 route:
    // its own ticket and the rules meet their routes' own refusal.
    let r = http_body(e.tunnel_port, "POST", "/cli/feedback/comment", Some(pass), &serde_json::json!({ "id": mine, "body": "x" }).to_string());
    assert_eq!(r.status, 403, "comment through the tunnel: {}", r.raw);
    let r = http(e.tunnel_port, "GET", "/cli/tickets/settings", Some(pass));
    assert_eq!(r.status, 403, "rules through the tunnel: {}", r.raw);
    assert_eq!(item(&e, &mine)["comments"], was["comments"], "nothing came in through the tunnel");

    // A bad passport gets the plain refusal it always got.
    let bad = format!("{}x", e.passport);
    let r = post_json(&e, "/cli/feedback/comment", &bad, serde_json::json!({ "id": mine, "body": "x" }));
    assert_eq!((r.status, r.raw.contains("invalid or missing token")), (403, true), "bad passport: {}", r.raw);
}

/// The tickets arm takes only a passport of a live, local session, as the
/// D1 routes do: an API cell's, a `k2 job` run's and one whose session
/// isn't live meet the routes' own refusal, and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ticket_write_needs_a_live_local_passport() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let own = Path::new(&e.own);
    let (api, _api_live) = live_passport(own, &e.own_id, &format!("api-d1bkey-{}", uuid::Uuid::new_v4()), "d1b-api");
    let mint = |address: &str| {
        session_token::mint_session_token(
            &SessionId::new(),
            "pane-not-live",
            HookPrincipal { workspace_uuid: e.own_id.clone(), agent_address: address.to_string() },
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        )
    };
    let job = mint("job:nightly");
    let gone = mint("d1-agent");
    let mine = ticket(&e.own_id, "Own question");
    let was = item(&e, &mine);
    for (label, p) in [("an API cell", &api), ("a k2 job run", &job), ("no live session", &gone)] {
        assert!(session_token::validate_hook(p).is_some(), "{label}: the passport itself is valid");
        let r = post_json(&e, "/cli/feedback/comment", p, serde_json::json!({ "id": mine, "body": "x" }));
        assert_eq!(r.status, 403, "{label}, comment: {}", r.raw);
        let r = post_json(&e, "/cli/feedback/create", p, serde_json::json!({ "title": "Not live", "kind": "fyi" }));
        assert_eq!(r.status, 403, "{label}, ask: {}", r.raw);
        let r = http(e.port, "GET", "/cli/tickets/settings", Some(p.as_str()));
        assert_eq!(r.status, 403, "{label}, rules: {}", r.raw);
    }
    assert_eq!(item(&e, &mine)["comments"], was["comments"], "no refused passport commented");
    let r = http(e.port, "GET", &format!("/cli/feedback/list?project={}&all=1", enc(&e.own)), Some(OWNER));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert!(!r.raw.contains("Not live"), "no refused passport filed a ticket: {}", r.raw);
    let r = http(e.port, "GET", "/cli/tickets/settings", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "the live passport reads the rules: {}", r.raw);
}

/// The ticket rules: the passport reads them (`canEdit: false`) and still
/// can't change them (`owner_only`); the owner reads and changes them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_reads_the_ticket_rules_and_never_changes_them() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let r = http(e.port, "GET", "/cli/tickets/settings", Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "passport read: {}", r.raw);
    let agents = r.json("settings");
    assert_eq!(agents["canEdit"], false, "{}", r.raw);
    let owners = http(e.port, "GET", "/cli/tickets/settings", Some(OWNER)).json("owner settings");
    assert_eq!(owners["canEdit"], true, "{owners}");
    for key in ["briefPolicy", "assigneePolicy", "defaultAssignee"] {
        assert_eq!(agents[key], owners[key], "{key}");
    }
    let r = post_json(&e, "/cli/tickets/settings", e.passport.as_str(), serde_json::json!({ "briefPolicy": "off" }));
    assert_eq!((r.status, code(&r, "settings write")), (403, "owner_only".to_string()), "{}", r.raw);
    assert_eq!(k2_core::app_settings::load().tickets.brief_policy, owners["briefPolicy"].as_str().expect("briefPolicy"));
    let r = http(e.port, "GET", "/cli/tickets/settings", Some("not-a-token"));
    assert_eq!(r.status, 403, "{}", r.raw);
    let r = post_json(&e, "/cli/tickets/settings", OWNER, serde_json::json!({ "briefPolicy": "off" }));
    assert_eq!(r.status, 200, "owner write: {}", r.raw);
    assert_eq!(k2_core::app_settings::load().tickets.brief_policy, "off");
}

/// The owner token is unchanged: any workspace's ticket, any step, the
/// body's author as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_token_still_works_every_ticket() {
    let t = k2_core::test_env::TempHome::new();
    let (e, _d) = env(&t).await;
    let r = post_json(&e, "/cli/feedback/create", OWNER, serde_json::json!({ "project": e.other, "title": "Owner filed", "kind": "fyi", "agentName": "named-agent" }));
    assert_eq!(r.status, 200, "owner ask in another workspace: {}", r.raw);
    let id = r.json("ask")["id"].as_str().expect("id").to_string();
    assert_eq!(item(&e, &id)["agentName"], "named-agent");
    let r = post_json(&e, "/cli/feedback/comment", OWNER, serde_json::json!({ "id": id, "body": "an agent's words", "author": "named-agent" }));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.json("comment")["author"], "named-agent");
    let r = post_json(&e, "/cli/feedback/assign", OWNER, serde_json::json!({ "id": id, "usernames": ["owner"] }));
    assert_eq!(r.status, 200, "{}", r.raw);
    let r = post_json(&e, "/cli/feedback/answer", OWNER, serde_json::json!({ "id": id, "answer": "yes" }));
    assert_eq!(r.status, 200, "owner answer: {}", r.raw);
    assert_eq!(item(&e, &id)["status"], "answered");
    let r = post_json(&e, "/cli/feedback/resolve", OWNER, serde_json::json!({ "id": id, "status": "dismissed" }));
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(item(&e, &id)["status"], "dismissed");
}
