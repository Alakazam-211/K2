//! D1 (0.46.17): an agent's passport over TCP, through the real dispatcher
//! (temp HOME, a real daemon on a port), on the routes agents are meant to
//! use (`passport_routes::ROUTES`, the Zen reads).
//!
//! For a sample across families: the passport is admitted and kept to its
//! own workspace (the same answer the owner gets for it); naming another
//! workspace is refused, or, where the route looks a workspace up, allowed
//! only for a connected one; the owner token still reaches every handler,
//! for any workspace. A sample of owner-only routes still refuses it.

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

struct Env {
    port: u16,
    tunnel_port: u16,
    passport: String,
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
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-d1-passport",
        HookPrincipal { workspace_uuid: ids[0].clone(), agent_address: "d1-agent".into() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    );
    assert!(session_token::validate_hook(&passport).is_some(), "the passport is live");
    let s = |p: &Path| p.to_string_lossy().to_string();
    let e = Env {
        port,
        tunnel_port: d.tunnel_port,
        passport,
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
            assert_eq!(r.status, 403, "passport {route} not connected ({q}): {}", r.raw);
            assert_eq!(code(&r, route), "not_connected", "{}", r.raw);
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
    let r = http(e.port, "POST", &format!("/cli/review/approve?project={own}&branch=agent%2Fx%2Fy&agent=x"), Some(e.passport.as_str()));
    assert_eq!(r.status, 400, "passport review approve, own: the handler's answer: {}", r.raw);
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
        ("POST", "/cli/feedback/create".to_string()),
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
    for path in [format!("/cli/agent/conf?q={}", enc(&e.own)), format!("/cli/context/layers?project={}", enc(&e.own)), "/cli/presets/list".to_string()] {
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
fn repo_with_branches(ws: &Path, branches: &[&str]) -> (git2::Oid, Vec<git2::Oid>) {
    let repo = git2::Repository::init(ws).expect("git init");
    let sig = git2::Signature::now("Acme Dev", "dev@example.com").expect("signature");
    let blob = repo.blob(b"base\n").expect("blob");
    let mut tb = repo.treebuilder(None).expect("tree");
    tb.insert("base.txt", blob, 0o100644).expect("insert");
    let tree = repo.find_tree(tb.write().expect("write tree")).expect("tree");
    let base = repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[]).expect("base commit");
    let parent = repo.find_commit(base).expect("base");
    let tips = branches
        .iter()
        .map(|b| {
            let blob = repo.blob(b.as_bytes()).expect("blob");
            let mut tb = repo.treebuilder(Some(&tree)).expect("tree");
            tb.insert(format!("{}.txt", b.replace('/', "-")), blob, 0o100644).expect("insert");
            let t = repo.find_tree(tb.write().expect("write tree")).expect("tree");
            repo.commit(Some(&format!("refs/heads/{b}")), &sig, &sig, b, &t, &[&parent]).expect("branch commit")
        })
        .collect();
    (base, tips)
}

fn head_of(ws: &Path) -> git2::Oid {
    git2::Repository::open(ws).expect("open").head().expect("head").target().expect("head target")
}

fn branch_tip(ws: &Path, b: &str) -> Option<git2::Oid> {
    let repo = git2::Repository::open(ws).expect("open");
    let found = repo.find_branch(b, git2::BranchType::Local).ok();
    found.and_then(|br| br.get().target())
}

/// A merge of the agent's own work is a person's: approving its own branch,
/// or one whose author can't be told, and `commit --merge` answer
/// `merge_needs_person` with the permission card, and nothing is merged.
/// Another agent's branch in its workspace is admitted and merged.
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
    let (base, tips) = repo_with_branches(ws, &["agent/d1-agent/own-task", "feature/no-author", "agent/backend/task"]);

    for (label, agent, branch) in [
        ("its own branch", "d1-agent", "agent/d1-agent/own-task"),
        ("an author that can't be told", "backend", "feature/no-author"),
        ("agent= and the branch disagree", "d1-agent", "agent/backend/task"),
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
    assert_eq!(branch_tip(ws, "agent/d1-agent/own-task"), Some(tips[0]), "its own branch is still there");

    let r = http(e.port, "POST", &format!("/cli/commit-merge?project={own}&message=wip"), Some(e.passport.as_str()));
    assert_eq!(r.status, 403, "commit --merge: {}", r.raw);
    let v = r.json("commit-merge");
    assert_eq!((v["error"]["code"].as_str(), v["permission"]["action"].as_str()), (Some("merge_needs_person"), Some("git.commit-merge")), "{v}");
    let r = http(e.port, "POST", &format!("/cli/commit?project={own}&message=wip"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "a plain commit stays the agent's: {}", r.raw);

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
/// A review the passport makes records it as the reviewer.
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
    let r = http(e.port, "POST", &format!("/cli/review/approve?project={own}&agent=backend&branch={}", enc("agent/backend/task")), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "approve another agent's branch: {}", r.raw);
    assert_eq!(head_of(ws), tips[0], "merged");
    let r = http(e.port, "POST", &format!("/cli/review/feedback?project={own}&agent=backend&feedback=tighten"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "feedback: {}", r.raw);
    let r = http(e.port, "POST", &format!("/cli/review/reject?project={own}&agent=backend&reason=no"), Some(e.passport.as_str()));
    assert_eq!(r.status, 200, "reject: {}", r.raw);
    for kind in ["review.approved", "review.changes-requested", "review.rejected"] {
        assert_eq!(entries(&e, kind), vec![("d1-agent".to_string(), "backend".to_string())], "{kind}: the passport reviewed backend's work");
    }
}
