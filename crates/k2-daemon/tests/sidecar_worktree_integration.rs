//! Workbench W1 (0.46.7) — worktree sidecars, headless.
//!
//! Real dispatcher (`test_harness::start`, in-process, ephemeral port, temp
//! `$HOME`), raw HTTP over loopback, no client attached. Harness binaries
//! are shims under `K2_TEST_AGENT_SHIM_DIR` that record their argv and then
//! `exec cat`. Each workspace is a real git repository (repo-local
//! identity), so `git worktree add`, cherry-pick and the guarded remove run
//! for real; the Trash is the test trash (`K2_TEST_TRASH_DIR`).
//!
//! Covers B1 (`sidecar new --worktree`: folder, branch, `workspaces` row,
//! cwd, absolute brief path, list fields, cap, no moving a plain sidecar,
//! the retired name), B2 (interrupt), B3 (review; land's refusals leave
//! main untouched; a conflict aborts; land + auto-retire; retire refuses
//! unlanded work until discard), agents refused on land / retire and their
//! verbs over the passport, and the HTTP shape (floors, 405).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER: &str = "owner-token-sidecar-worktree-w1";

struct TestEnv {
    home: PathBuf,
    shim_log: PathBuf,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, s) in k2_daemon::v2_session_map::snapshot() {
            if s.cwd.as_ref().is_some_and(|p| p.starts_with(&self.home)) {
                k2_daemon::v2_session_map::unregister(&key);
                s.kill();
            }
        }
        for (k, v) in self.prev.drain(..) {
            match v {
                Some(p) => std::env::set_var(k, p),
                None => std::env::remove_var(k),
            }
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos();
    let home = PathBuf::from(format!("/tmp/k2wt-{:x}", nanos % 0xffff_ffff));
    let shim_dir = home.join("shim");
    let shim_log = home.join("shim-log");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    std::fs::create_dir_all(&shim_log).expect("shim log");
    let home = dunce::canonicalize(&home).expect("canonical home");
    let shim_log = dunce::canonicalize(&shim_log).expect("canonical log");
    let script = format!(
        "#!/bin/sh\n\
         n=$(/usr/bin/basename \"$0\")\n\
         if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
         f=\"{log}/$n-$(/bin/date +%s)-$$\"\n\
         printf '%s\\0' \"$@\" > \"$f.tmp\"\n\
         /bin/pwd > \"$f.cwd\"\n\
         /bin/mv \"$f.tmp\" \"$f.argv\"\n\
         exec /bin/cat\n",
        log = shim_log.display()
    );
    k2_core::test_env::write_executable(&shim_dir.join("claude"), &script);
    let prev = ["HOME", "K2_TEST_AGENT_SHIM_DIR", "K2_SIDECAR_CAP", "K2_TEST_TRASH_DIR"]
        .into_iter()
        .map(|k| (k, std::env::var_os(k)))
        .collect();
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    std::env::set_var("K2_TEST_TRASH_DIR", home.join(".Trash"));
    std::env::remove_var("K2_SIDECAR_CAP");
    TestEnv { home, shim_log, prev, _guard: guard }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

// ── HTTP ──────────────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn req(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(60))).expect("timeout");
    let raw_req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    stream.write_all(raw_req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset) => break,
            Err(e) => panic!("{method} {path_and_query}: read: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
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

fn post(port: u16, path: &str, token: &str, body: &Value) -> Resp {
    req(port, "POST", &format!("{path}?token={}", enc(token)), Some(&body.to_string()))
}

fn get(port: u16, path: &str, token: &str, q: &str) -> Resp {
    req(port, "GET", &format!("{path}?token={}&{q}", enc(token)), None)
}

fn code(r: &Resp) -> String {
    json_of(&r.body)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

fn ok(r: Resp) -> Value {
    assert_eq!(r.status, 200, "{}", r.body);
    json_of(&r.body)
}

fn provision_login(port: u16, username: &str, role: &str) -> String {
    let r = req(port, "POST", &format!("/cli/users/add?token={OWNER}"), Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)));
    assert_eq!(r.status, 200, "users/add; {}", r.body);
    if role != "member" {
        let r = req(port, "POST", &format!("/cli/users/set-role?token={OWNER}"), Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)));
        assert_eq!(r.status, 200, "set-role; {}", r.body);
    }
    let r = req(port, "POST", "/cli/auth/login", Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)));
    assert_eq!(r.status, 200, "login; {}", r.body);
    json_of(&r.body)["token"].as_str().expect("token").to_string()
}

// ── workspace + git ──────────────────────────────────────────────────

struct Ws {
    id: String,
    path: PathBuf,
    handle: String,
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// A registered workspace that is a git repository on `main` (one commit).
fn seed_repo(env: &TestEnv, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).expect("ws dir");
    sh(&path, &["init", "-q"]);
    sh(&path, &["config", "user.name", "K2 Test"]);
    sh(&path, &["config", "user.email", "k2-test@example.com"]);
    std::fs::write(path.join("a.txt"), "one\ntwo\n").expect("a");
    sh(&path, &["add", "-A"]);
    sh(&path, &["commit", "-q", "-m", "init"]);
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()])
        .expect("project");
    conn.execute("INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')", params![uuid::Uuid::new_v4().to_string(), id])
        .expect("workspaces row");
    Ws { id, path, handle: handle.to_string() }
}

fn commit_in(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).expect("write");
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-q", "-m", msg]);
    sh(dir, &["rev-parse", "HEAD"])
}

fn new_wt(port: u16, token: &str, ws: &Ws, name: &str, brief: Option<&str>) -> Resp {
    let mut body = json!({ "workspace": ws.handle, "name": name, "harness": "claude", "worktree": true });
    if let Some(b) = brief {
        body["brief"] = json!(b);
    }
    post(port, "/cli/sidecar/new", token, &body)
}

fn review(port: u16, token: &str, ws: &Ws, name: &str) -> Resp {
    get(port, "/cli/sidecar/review", token, &format!("workspace={}&name={}", enc(&ws.handle), enc(name)))
}

fn list_row(port: u16, ws: &Ws, handle: &str) -> Value {
    let v = ok(get(port, "/cli/sidecar/list", OWNER, &format!("workspace={}", enc(&ws.handle))));
    v["sidecars"]
        .as_array()
        .expect("sidecars")
        .iter()
        .find(|r| r["handle"] == handle)
        .cloned()
        .unwrap_or_else(|| panic!("no row {handle} in {v}"))
}

fn worktree_rows(ws: &Ws) -> Vec<(String, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT type, worktree_path FROM workspaces WHERE project_id = ?1 AND type = 'worktree'").expect("prep");
    st.query_map(params![ws.id], |r| Ok((r.get(0)?, r.get(1)?))).expect("q").map(|r| r.expect("row")).collect()
}

fn branch_exists(root: &Path, b: &str) -> bool {
    !sh(root, &["branch", "--list", b]).is_empty()
}

fn run_files(env: &TestEnv) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(&env.shim_log)
        .expect("log")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("argv"))
        .collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).expect("mtime"));
    files
}

/// The first harness run after `before` runs: its argv and cwd.
fn next_run(env: &TestEnv, before: usize) -> (Vec<String>, String) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let files = run_files(env);
        if files.len() > before {
            let f = files.last().expect("a run");
            let raw = std::fs::read(f).expect("argv");
            let argv = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
            let cwd = std::fs::read_to_string(f.with_extension("cwd")).unwrap_or_default().trim().to_string();
            return (argv, cwd);
        }
        assert!(Instant::now() < deadline, "no new harness run within 20 s (had {before})");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn passport(ws: &Ws) -> String {
    let out = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: ws.id.clone(),
        project_id: Some(ws.id.clone()),
        cwd: ws.path.to_string_lossy().into_owned(),
        command: Some("claude".to_string()),
        args: Some(vec!["--dangerously-skip-permissions".to_string()]),
        cols: 80,
        rows: 24,
        canonical_key: Some(ws.id.clone()),
        env: std::collections::HashMap::new(),
        env_owner_set: false,
        launch_prompt: None,
        label: None,
    })
    .expect("spawn the main session");
    let sid: SessionId = out.session_id;
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.id.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

// ── B1 + B2 + B3, the owner's happy path ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_new_worktree_review_interrupt_land_and_retire() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_repo(&env, &unique("wt"));
    let main_before = sh(&ws.path, &["rev-parse", "main"]);

    // B1: new --worktree.
    let before = run_files(&env).len();
    let v = ok(new_wt(d.port, OWNER, &ws, "fix-login", Some("# Fix the login\n")));
    let address = format!("{}/fix-login", ws.handle);
    assert_eq!(v["address"], address.as_str());
    assert_eq!(
        v["worktree"],
        json!({ "branch": "k2/fix-login", "dir": ".worktrees/k2-fix-login", "base": "main", "baseSha": main_before, "onto": "main" })
    );
    let wt = dunce::canonicalize(ws.path.join(".worktrees/k2-fix-login")).expect("the worktree exists");
    assert!(branch_exists(&ws.path, "k2/fix-login"));
    assert_eq!(sh(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "k2/fix-login");
    // The harness runs in the worktree and is pointed at the brief's absolute path.
    let (argv, cwd) = next_run(&env, before);
    assert_eq!(dunce::canonicalize(&cwd).expect("cwd"), wt, "the sidecar runs in its worktree");
    let pointer = argv.last().expect("argv");
    let abs_brief = ws.path.join(".k2/sidecars/fix-login/BRIEF.md");
    assert!(pointer.contains(&abs_brief.to_string_lossy().to_string()), "absolute brief path: {pointer}");
    assert_eq!(v["brief"]["path"], ".k2/sidecars/fix-login/BRIEF.md");
    // A `workspaces` row of type worktree (the Changes tab sees it).
    let rows = worktree_rows(&ws);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(dunce::canonicalize(rows[0].1.as_deref().expect("path")).expect("p"), wt);

    // list: the worktree fields and state.
    let row = list_row(d.port, &ws, "fix-login");
    assert_eq!(row["worktree"]["branch"], "k2/fix-login");
    assert_eq!(row["worktree"]["dir"], ".worktrees/k2-fix-login");
    assert_eq!(row["worktree"]["ahead"], 0);
    assert!(!row["worktree"]["dir"].as_str().unwrap_or("/").starts_with('/'), "no host path");

    // B2: interrupt a live sidecar (Esc for claude); it keeps running.
    let r = ok(post(d.port, "/cli/sidecar/interrupt", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login" })));
    assert_eq!((r["interrupted"].clone(), r["key"].clone(), r["state"].clone()), (json!(true), json!("esc"), json!("live")), "{r}");
    let pg = row["paneGroupId"].as_str().expect("pg").to_string();
    assert!(k2_daemon::v2_session_map::lookup_by_agent_name(&format!("tab-{pg}")).is_some_and(|s| s.is_child_alive()));

    // The agent's work: one commit, plus a scratch file.
    let tip = commit_in(&wt, "login.txt", "fixed\n", "fix: the login");
    let r = ok(post(d.port, "/cli/sidecar/stop", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login", "force": true })));
    assert_eq!(r["stopped"], true);
    let row = list_row(d.port, &ws, "fix-login");
    assert_eq!((row["taskState"].clone(), row["worktree"]["ahead"].clone()), (json!("review"), json!(1)), "{row}");

    // review: what Land would do.
    let rv = ok(review(d.port, OWNER, &ws, "fix-login"));
    assert_eq!(rv["onto"], "main");
    assert_eq!(rv["ontoHead"], main_before.as_str());
    assert_eq!(rv["tip"], tip.as_str());
    assert_eq!(rv["commits"], json!([{ "sha": tip, "subject": "fix: the login" }]));
    assert_eq!(rv["files"], json!([{ "status": "A", "path": "login.txt" }]));
    assert_eq!((rv["canLand"].clone(), rv["blocked"].clone()), (json!(true), Value::Null), "{rv}");

    // Land's refusals: nothing changes on main.
    let r = post(d.port, "/cli/sidecar/land", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login" }));
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "expectTip is required: {}", r.body);
    let r = post(d.port, "/cli/sidecar/land", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login", "expectTip": "0000000000" }));
    assert_eq!((r.status, code(&r)), (409, "stale".to_string()), "{}", r.body);
    std::fs::write(ws.path.join("a.txt"), "one\nmine\n").expect("dirty main");
    let r = post(d.port, "/cli/sidecar/land", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login", "expectTip": tip }));
    assert_eq!((r.status, code(&r)), (409, "main_has_uncommitted_changes".to_string()), "{}", r.body);
    assert_eq!(json_of(&r.body)["error"]["files"], json!(["a.txt"]));
    assert_eq!(std::fs::read_to_string(ws.path.join("a.txt")).expect("a"), "one\nmine\n", "the person's edit stays");
    sh(&ws.path, &["checkout", "--", "a.txt"]);
    assert_eq!(sh(&ws.path, &["rev-parse", "main"]), main_before);

    // Land (and the default auto-retire).
    let r = ok(post(
        d.port,
        "/cli/sidecar/land",
        OWNER,
        &json!({ "workspace": ws.handle, "name": "fix-login", "expectTip": &tip[..12], "expectOnto": main_before }),
    ));
    assert_eq!(r["count"], 1, "{r}");
    assert_eq!(r["from"], main_before.as_str());
    assert_eq!(sh(&ws.path, &["rev-parse", "main"]), r["to"].as_str().expect("to"));
    assert_eq!(std::fs::read_to_string(ws.path.join("login.txt")).expect("landed"), "fixed\n");
    assert_eq!(r["retired"]["how"], "landed", "{r}");
    assert_eq!(r["retired"]["branchDeleted"], true);
    assert_eq!(r["retireError"], Value::Null);
    assert!(!wt.exists(), "the worktree went to the Trash");
    // The rename is immediate; the move into the (test) Trash runs behind it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_dir(env.home.join(".Trash")).map(|d| d.count() > 0).unwrap_or(false) {
        assert!(Instant::now() < deadline, "the worktree never reached the (test) Trash");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!branch_exists(&ws.path, "k2/fix-login"));
    assert!(worktree_rows(&ws).is_empty(), "the workspaces row is gone");
    let row = list_row(d.port, &ws, "fix-login");
    assert_eq!((row["taskState"].clone(), row["worktree"]["retiredHow"].clone()), (json!("landed"), json!("landed")), "{row}");

    // The name stays with its Thread.
    let r = new_wt(d.port, OWNER, &ws, "fix-login", None);
    assert_eq!((r.status, code(&r)), (409, "sidecar_retired".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/sidecar/land", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login", "expectTip": tip }));
    assert_eq!((r.status, code(&r)), (410, "sidecar_retired".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/sidecar/retire", OWNER, &json!({ "workspace": ws.handle, "name": "fix-login" }));
    assert_eq!((r.status, code(&r)), (409, "already_retired".to_string()), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_aborts_and_retire_refuses_unlanded_work_until_discard() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_repo(&env, &unique("cf"));
    ok(new_wt(d.port, OWNER, &ws, "clash", None));
    let wt = ws.path.join(".worktrees/k2-clash");
    let tip = commit_in(&wt, "a.txt", "one\nTHEIRS\n", "theirs");
    let main_head = commit_in(&ws.path, "a.txt", "one\nOURS\n", "ours");

    let r = post(d.port, "/cli/sidecar/land", OWNER, &json!({ "workspace": ws.handle, "name": "clash", "expectTip": tip }));
    assert_eq!((r.status, code(&r)), (409, "conflict".to_string()), "{}", r.body);
    assert_eq!(json_of(&r.body)["error"]["files"], json!(["a.txt"]));
    assert_eq!(sh(&ws.path, &["rev-parse", "HEAD"]), main_head, "main is exactly where it was");
    assert_eq!(std::fs::read_to_string(ws.path.join("a.txt")).expect("a"), "one\nOURS\n");
    assert!(sh(&ws.path, &["status", "--porcelain", "--untracked-files=no"]).is_empty(), "no pick in progress, clean");
    let rv = ok(review(d.port, OWNER, &ws, "clash"));
    assert_eq!(rv["commits"].as_array().map(Vec::len), Some(1), "still waiting to land");

    // Retire without discard: refused, everything stays.
    std::fs::write(wt.join("scratch.txt"), "unsaved\n").expect("scratch");
    let r = post(d.port, "/cli/sidecar/retire", OWNER, &json!({ "workspace": ws.handle, "name": "clash" }));
    assert_eq!((r.status, code(&r)), (409, "unlanded_work".to_string()), "{}", r.body);
    let e = json_of(&r.body);
    assert_eq!(e["error"]["files"], json!(["scratch.txt"]));
    assert_eq!(e["error"]["commits"][0]["sha"], tip.as_str());
    assert!(wt.join("scratch.txt").exists() && branch_exists(&ws.path, "k2/clash"));

    // With discard: gone (worktree to the Trash, branch deleted).
    let r = ok(post(d.port, "/cli/sidecar/retire", OWNER, &json!({ "workspace": ws.handle, "name": "clash", "discard": true })));
    assert_eq!((r["how"].clone(), r["branchDeleted"].clone()), (json!("discarded"), json!(true)), "{r}");
    assert_eq!(r["discardedFiles"], json!(["scratch.txt"]));
    assert!(!wt.exists());
    assert!(!branch_exists(&ws.path, "k2/clash"));
    assert_eq!(sh(&ws.path, &["rev-parse", "HEAD"]), main_head, "discard never touches main");
    assert_eq!(list_row(d.port, &ws, "clash")["taskState"], "discarded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cap_and_no_moving_a_plain_sidecar_and_bad_input() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_repo(&env, &unique("cp"));

    // A plain sidecar's name can't become a worktree sidecar.
    ok(post(d.port, "/cli/sidecar/new", OWNER, &json!({ "workspace": ws.handle, "name": "plain", "harness": "claude" })));
    ok(post(d.port, "/cli/sidecar/stop", OWNER, &json!({ "workspace": ws.handle, "name": "plain", "force": true })));
    let r = new_wt(d.port, OWNER, &ws, "plain", None);
    assert_eq!((r.status, code(&r)), (409, "name_taken".to_string()), "{}", r.body);
    assert!(!ws.path.join(".worktrees/k2-plain").exists());
    // review / land on a plain sidecar.
    let r = review(d.port, OWNER, &ws, "plain");
    assert_eq!((r.status, code(&r)), (409, "not_a_worktree_sidecar".to_string()), "{}", r.body);

    // Bad input.
    let r = post(d.port, "/cli/sidecar/new", OWNER, &json!({ "workspace": ws.handle, "name": "b", "harness": "claude", "base": "main" }));
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "base without worktree: {}", r.body);
    let r = post(d.port, "/cli/sidecar/new", OWNER, &json!({ "workspace": ws.handle, "name": "b", "harness": "claude", "worktree": "yes" }));
    assert_eq!((r.status, code(&r)), (400, "bad_request".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/sidecar/new", OWNER, &json!({ "workspace": ws.handle, "name": "b", "harness": "claude", "worktree": true, "base": "no-such-ref" }));
    assert_eq!(code(&r), "base_not_found", "{}", r.body);
    assert!(!ws.path.join(".worktrees/k2-b").exists() && !branch_exists(&ws.path, "k2/b"), "nothing made");

    // Cap 1: a stopped worktree sidecar still counts until retired.
    std::env::set_var("K2_SIDECAR_CAP", "1");
    ok(new_wt(d.port, OWNER, &ws, "one", None));
    ok(post(d.port, "/cli/sidecar/stop", OWNER, &json!({ "workspace": ws.handle, "name": "one", "force": true })));
    let r = new_wt(d.port, OWNER, &ws, "two", None);
    assert_eq!((r.status, code(&r)), (409, "worktree_cap".to_string()), "{}", r.body);
    assert!(!ws.path.join(".worktrees/k2-two").exists(), "refused before git");
    ok(post(d.port, "/cli/sidecar/retire", OWNER, &json!({ "workspace": ws.handle, "name": "one" })));
    ok(new_wt(d.port, OWNER, &ws, "two", None));
    std::env::remove_var("K2_SIDECAR_CAP");

    // A sleeping worktree sidecar resumes in its worktree.
    ok(post(d.port, "/cli/sidecar/stop", OWNER, &json!({ "workspace": ws.handle, "name": "two", "force": true })));
    let before = run_files(&env).len();
    let v = ok(post(d.port, "/cli/sidecar/new", OWNER, &json!({ "workspace": ws.handle, "name": "two", "harness": "claude" })));
    assert_eq!((v["resumed"].clone(), v["worktree"]["branch"].clone()), (json!(true), json!("k2/two")), "{v}");
    let (_, cwd) = next_run(&env, before);
    assert_eq!(
        dunce::canonicalize(&cwd).expect("cwd"),
        dunce::canonicalize(ws.path.join(".worktrees/k2-two")).expect("wt"),
        "resumed in its own worktree"
    );
}

// ── agents and the HTTP shape ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_ask_a_person_to_land_and_retire_and_people_need_the_floor() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_repo(&env, &unique("ag"));
    ok(new_wt(d.port, OWNER, &ws, "task", None));
    let wt = ws.path.join(".worktrees/k2-task");
    let tip = commit_in(&wt, "t.txt", "t\n", "task work");
    let main_before = sh(&ws.path, &["rev-parse", "main"]);

    let agent = passport(&ws);
    // review is a read the agent may do.
    let rv = ok(review(d.port, &agent, &ws, "task"));
    assert_eq!(rv["tip"], tip.as_str());
    // land / retire: refused with what the card needs; nothing changes.
    let r = post(d.port, "/cli/sidecar/land", &agent, &json!({ "workspace": ws.handle, "name": "task", "expectTip": tip }));
    assert_eq!((r.status, code(&r)), (403, "land_needs_person".to_string()), "{}", r.body);
    let e = json_of(&r.body);
    assert_eq!((e["error"]["sidecar"].clone(), e["error"]["tip"].clone()), (json!("task"), json!(tip)), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap_or("").contains("k2 thread ask"), "{e}");
    let r = post(d.port, "/cli/sidecar/retire", &agent, &json!({ "workspace": ws.handle, "name": "task", "discard": true }));
    assert_eq!((r.status, code(&r)), (403, "retire_needs_person".to_string()), "{}", r.body);
    assert_eq!(json_of(&r.body)["error"]["discard"], "yes");
    assert_eq!(sh(&ws.path, &["rev-parse", "main"]), main_before);
    assert!(wt.exists() && branch_exists(&ws.path, "k2/task"));
    // interrupt: the sidecar rules (switch off → gated).
    let r = post(d.port, "/cli/sidecar/interrupt", &agent, &json!({ "workspace": ws.handle, "name": "task" }));
    assert_eq!((r.status, code(&r)), (403, "gated".to_string()), "{}", r.body);

    // People: a Member may review and interrupt, not land or retire.
    let member = provision_login(d.port, &unique("m"), "member");
    ok(review(d.port, &member, &ws, "task"));
    ok(post(d.port, "/cli/sidecar/interrupt", &member, &json!({ "workspace": ws.handle, "name": "task" })));
    for path in ["/cli/sidecar/land", "/cli/sidecar/retire"] {
        let r = post(d.port, path, &member, &json!({ "workspace": ws.handle, "name": "task", "expectTip": tip }));
        assert_eq!(r.status, 403, "{path}: {}", r.body);
        assert_eq!(json_of(&r.body)["error"], "role_required", "{path}: {}", r.body);
    }
    // An Admin lands.
    let admin = provision_login(d.port, &unique("a"), "admin");
    let r = ok(post(d.port, "/cli/sidecar/land", &admin, &json!({ "workspace": ws.handle, "name": "task", "expectTip": tip })));
    assert_eq!(r["count"], 1);

    // GET on the POST routes → 405.
    for path in ["/cli/sidecar/interrupt", "/cli/sidecar/land", "/cli/sidecar/retire"] {
        let r = req(d.port, "GET", &format!("{path}?token={OWNER}"), None);
        assert_eq!(r.status, 405, "GET {path}: {}", r.body);
    }
}

#[test]
fn route_rows_and_agent_verbs() {
    use k2_daemon::routes::route_policy::{lookup, Floor};
    for (path, get, post) in [
        ("/cli/sidecar/interrupt", None, Some(Floor::Member)),
        ("/cli/sidecar/land", None, Some(Floor::Admin)),
        ("/cli/sidecar/retire", None, Some(Floor::Admin)),
        ("/cli/sidecar/review", Some(Floor::Member), None),
    ] {
        let row = lookup(path).unwrap_or_else(|| panic!("{path} has a policy row"));
        assert_eq!((row.get, row.post), (get, post), "{path}");
        assert!(k2_daemon::session_token::is_agent_verb(path), "{path} reaches its handler for a passport");
        assert!(k2_daemon::sidecar_routes::is_route(path), "{path}");
    }
    for path in ["/cli/sidecar/interrupt", "/cli/sidecar/land", "/cli/sidecar/retire"] {
        assert!(k2_daemon::sidecar_routes::is_post_route(path), "{path} is POST-only");
    }
    assert!(!k2_daemon::sidecar_routes::is_post_route("/cli/sidecar/review"));
}
