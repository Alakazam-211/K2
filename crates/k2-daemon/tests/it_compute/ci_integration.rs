//! `k2 ci` (0.46.10, prd-ci-on-compute-for-clients-v1 C6a), end to end on
//! the controller: the real dispatcher in-process (temp `$HOME`, loopback,
//! the vault on a file key there), a real git repo with a bare "GitHub"
//! remote (`url.<bare>.insteadOf https://github.com/acme/app.git`), a fake
//! node session (`compute_ws::test_attach`) that the jobs are placed on,
//! jobs finished by writing their rows (the fake compute's outcome), and a
//! fake GitHub statuses endpoint on loopback that records each POST.
//!
//! Covers: `watch on` (defaults from the working tree's `.k2/ci.toml`, the
//! remote checked now), the loop starting a run per new tip with one
//! `--tier ci --supersede ci/main/<check>` job per check through the
//! normal job path, `pending` then `success` / `failure` statuses with the
//! workspace vault's token, the verdict, a second push, a manual `k2 ci
//! run --check`, `status` by run / commit / branch; refusals (compute
//! switch off, another workspace, a remote that doesn't answer, no
//! config, a grant without `--ci`, a Member-less owner path), `init`
//! (refuses to overwrite, `--force`, a placeholder), `freeze`. The token
//! is never in an HTTP answer or a file.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Free, JobState, Offer};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-ci-v1";
const TOKEN: &str = "github_pat_11K2TESTci0statusesFAKEtoken0123456789";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: k2_core::test_env::SerialGuard,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        std::env::remove_var("K2_CI");
        k2_daemon::ci_engine::test_set_statuses_api(None);
        k2_core::compute::retry::set_tuning_for_tests(Default::default());
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2ci-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    // CI on compute is its own preview (0.46.10 ships it dark).
    std::env::set_var("K2_CI", "1");
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, retry_base_secs: 0, ..Default::default() });
    env
}

// ── a fake GitHub statuses endpoint ──────────────────────────────────

#[derive(Debug, Clone)]
struct Post {
    path: String,
    auth: String,
    body: Value,
}

struct FakeGithub {
    base: String,
    posts: Arc<StdMutex<Vec<Post>>>,
}

impl FakeGithub {
    fn posts(&self) -> Vec<Post> {
        self.posts.lock().unwrap().clone()
    }
    /// `(context, state)` of every status for `sha`, in order.
    fn states(&self, sha: &str) -> Vec<(String, String)> {
        self.posts()
            .into_iter()
            .filter(|p| p.path == format!("/repos/acme/app/statuses/{sha}"))
            .map(|p| (p.body["context"].as_str().unwrap().to_string(), p.body["state"].as_str().unwrap().to_string()))
            .collect()
    }
}

fn start_github() -> FakeGithub {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let posts: Arc<StdMutex<Vec<Post>>> = Default::default();
    let p2 = posts.clone();
    std::thread::spawn(move || {
        for c in listener.incoming() {
            let Ok(mut c) = c else { continue };
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head, mut body) = loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break (String::from_utf8_lossy(&buf).to_string(), Vec::new());
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break (String::from_utf8_lossy(&buf[..i]).to_string(), buf[i + 4..].to_vec());
                }
            };
            let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
            while body.len() < len {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            let mut first = head.lines().next().unwrap_or("").split(' ');
            let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
            let auth = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
            let ok = method == "POST" && path.starts_with("/repos/acme/app/statuses/") && auth == format!("Bearer {TOKEN}");
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if ok {
                p2.lock().unwrap().push(Post { path: path.clone(), auth, body: v });
            }
            let (status, text) = if ok { (201, r#"{"state":"ok"}"#) } else { (401, r#"{"message":"Bad credentials"}"#) };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).as_bytes());
        }
    });
    FakeGithub { base, posts }
}

// ── the daemon ───────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len()),
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
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    assert!(!body.contains(TOKEN), "an HTTP answer carries the token: {body}");
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

async fn tick() {
    tokio::task::spawn_blocking(k2_daemon::ci_engine::tick_now).await.unwrap();
}

// ── fixtures ─────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const CI_TOML: &str = "version = 1\n\
[watch]\nremote = \"origin\"\nbranches = [\"main\"]\n\
[statuses]\ntoken_secret = \"K2_CI_STATUSES_TOKEN\"\n\
[checks.unit]\nargv = [\"true\"]\nmem_gb = 1\n\
[checks.lint]\nargv = [\"sh\", \"-c\", \"exit 3\"]\nmem_gb = 1\n";

struct Ws {
    id: String,
    path: PathBuf,
}

/// A workspace repo whose `origin` is "github.com/acme/app" (rewritten to a
/// bare repo here) with `.k2/ci.toml` committed and pushed.
fn seed_repo_ws(env: &Env, handle: &str, ci_toml: Option<&str>) -> (Ws, PathBuf) {
    let bare = env.home.join(format!("{handle}-remote.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    if let Some(t) = ci_toml {
        std::fs::create_dir_all(path.join(".k2")).unwrap();
        std::fs::write(path.join(".k2/ci.toml"), t).unwrap();
    }
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "one"]);
    git(&path, &["remote", "add", "origin", "https://github.com/acme/app.git"]);
    git(&path, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/acme/app.git"]);
    git(&path, &["push", "-q", "origin", "main"]);
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
        conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![id]).unwrap();
    }
    (Ws { id, path }, bare)
}

fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(&sid, &sid.to_string(), HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() }, CredMode::ApiKey, Provider::Anthropic)
}

fn node_fp(id: &str) -> String {
    format!("{id:0>64}")
}

fn active_node(id: &str, name: &str, ws: &Ws, allow_ci: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &node_fp(id), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, &ws.id, &store::GrantLimits { allow_ci: Some(allow_ci), ..Default::default() }, "owner", 3).expect("grant");
}

fn offer() -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 8000, mem_bytes: 32 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: Free { cpu_millis: 8000, mem_bytes: 16 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 4,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: [("git".to_string(), "2.43.0".to_string())].into(),
        protocol: 8,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.9".into(),
        features: vec!["artifacts".into(), "stall".into()],
        host: None,
        pkg: None,
        sim: None,
    }
}

fn put_token(ws: &Ws, hosts: Vec<String>) {
    let v = k2_daemon::vault::get().expect("vault open");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "K2_CI_STATUSES_TOKEN", Kind::ApiKey);
    r.hosts = hosts;
    v.put(r, SecretBytes::from_slice(TOKEN.as_bytes()), &Actor::Owner).expect("put the statuses token");
}

/// Finish a job as the node would have: its row ends `done` with `exit`.
fn finish_job(job: &str, exit: i32) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = k2_core::compute::now();
    store::set_job_state(&conn, job, JobState::Running, None, None, now - 90).unwrap();
    store::set_job_exit(&conn, job, Some(exit), None);
    store::set_job_state(&conn, job, JobState::Done, None, None, now).unwrap();
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn check<'a>(run: &'a Value, name: &str) -> &'a Value {
    run["checks"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no check {name}: {run}"))
}

/// Fail if the token is in any file under `root`.
fn assert_no_token_on_disk(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    let mut scanned = 0;
    while let Some(p) = stack.pop() {
        let m = std::fs::symlink_metadata(&p).unwrap();
        if m.is_dir() {
            for e in std::fs::read_dir(&p).unwrap() {
                stack.push(e.unwrap().path());
            }
        } else if m.is_file() {
            let bytes = std::fs::read(&p).unwrap();
            scanned += 1;
            assert!(!bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()), "the token is on disk in {}", p.display());
        }
    }
    assert!(scanned > 3, "scanned only {scanned} files under {}", root.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watch_runs_each_push_on_compute_and_posts_statuses() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let gh = start_github();
    k2_daemon::ci_engine::test_set_statuses_api(Some(gh.base.clone()));
    let (ws, _bare) = seed_repo_ws(&env, "ciapp", Some(CI_TOML));
    let sha1 = git(&ws.path, &["rev-parse", "HEAD"]);
    active_node("ci-1", "rig7", &ws, true);
    let mut node = compute_ws::test_attach("ci-1", offer());
    put_token(&ws, vec![gh.base.clone()]);
    let agent = passport(&ws);

    // Nothing set up yet.
    let r = get(d.port, "/cli/ci/status", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["watch"].clone(), j(&r)["runs"].clone()), (Value::Null, json!([])));

    // The agent turns the watch on for its own workspace: the branches
    // come from .k2/ci.toml's [watch]; the remote answers now.
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let w = j(&r)["watch"].clone();
    assert_eq!((w["on"].clone(), w["remote"].clone(), w["branches"].clone(), w["tier"].clone()), (json!(true), json!("origin"), json!(["main"]), json!("ci")));
    assert_eq!(j(&r)["tips"], json!([{"branch": "main", "sha": sha1}]));

    // The loop: one run for the tip, one CI job per check on the node.
    tick().await;
    let r = get(d.port, "/cli/ci/status", &agent, "").await;
    let runs = j(&r)["runs"].clone();
    assert_eq!(runs.as_array().unwrap().len(), 1, "{}", r.body);
    let run1 = runs[0].clone();
    assert_eq!((run1["sha"].clone(), run1["ref"].clone(), run1["trigger"].clone(), run1["state"].clone()), (json!(sha1), json!("main"), json!("watch"), json!("running")));
    assert_eq!(run1["statuses"], json!({"slug": "acme/app", "context": "k2-ci", "tokenSecret": "K2_CI_STATUSES_TOKEN"}));
    let unit_job = check(&run1, "unit")["job"].as_str().expect("unit job").to_string();
    let lint_job = check(&run1, "lint")["job"].as_str().expect("lint job").to_string();
    let u = row(&unit_job);
    assert_eq!((u.workspace_id.as_str(), u.tier.as_str(), u.supersede_key.as_deref(), u.argv.clone()), (ws.id.as_str(), "ci", Some("ci/main/unit"), vec!["true".to_string()]));
    assert!(u.submitter.as_deref().is_some_and(|s| s.contains("/ci-") && s.ends_with("/ci")), "{:?}", u.submitter);
    // Placed on the granted node (it may still sit in the server's queue
    // until the next kick: the test's ticker is slowed down).
    assert_eq!((row(&unit_job).node_id.as_str(), row(&lint_job).node_id.as_str()), ("ci-1", "ci-1"), "both placed on the granted node");
    let _ = node.frames();
    let plan = row(&unit_job).plan().expect("plan");
    assert_eq!(plan.src.as_ref().map(|s| s.commit.clone()), Some(sha1.clone()), "the job builds the commit");
    let mut pend = gh.states(&sha1);
    pend.sort();
    assert_eq!(pend, vec![("k2-ci/lint".into(), "pending".into()), ("k2-ci/unit".into(), "pending".into())]);
    assert!(gh.posts().iter().all(|p| p.auth == format!("Bearer {TOKEN}")));

    // The node's work: unit passes, lint exits 3.
    finish_job(&unit_job, 0);
    finish_job(&lint_job, 3);
    tick().await;
    let r = get(d.port, "/cli/ci/status", &agent, &format!("run={}", &run1["id"].as_str().unwrap()[..8])).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let done = j(&r)["run"].clone();
    assert_eq!(done["state"], "failed", "{done}");
    assert_eq!((check(&done, "unit")["state"].clone(), check(&done, "unit")["status"].clone()), (json!("success"), json!("success")));
    assert_eq!(check(&done, "lint")["state"], "failure");
    assert!(check(&done, "lint")["description"].as_str().unwrap().contains("exit 3"), "{done}");
    let mut finals = gh.states(&sha1);
    finals.sort();
    assert_eq!(
        finals,
        vec![
            ("k2-ci/lint".into(), "failure".into()),
            ("k2-ci/lint".into(), "pending".into()),
            ("k2-ci/unit".into(), "pending".into()),
            ("k2-ci/unit".into(), "success".into())
        ]
    );
    // By commit and by branch too.
    let r = get(d.port, "/cli/ci/status", &agent, &format!("run={}", &sha1[..10])).await;
    assert_eq!(j(&r)["run"]["id"], run1["id"]);
    let r = get(d.port, "/cli/ci/status", &agent, "run=main").await;
    assert_eq!(j(&r)["run"]["id"], run1["id"]);
    let r = get(d.port, "/cli/ci/status", &agent, "run=deadbeef00").await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_run_not_found"));

    // A second push: the next poll runs it.
    std::fs::write(ws.path.join("a.txt"), "two\n").unwrap();
    git(&ws.path, &["commit", "-q", "-am", "two"]);
    git(&ws.path, &["push", "-q", "origin", "main"]);
    let sha2 = git(&ws.path, &["rev-parse", "HEAD"]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE ci_workspaces SET last_poll_at = NULL WHERE workspace_id = ?1", params![ws.id]).unwrap();
    }
    tick().await;
    let r = get(d.port, "/cli/ci/status", &agent, "").await;
    let runs = j(&r)["runs"].clone();
    assert_eq!(runs.as_array().unwrap().len(), 2, "{}", r.body);
    assert_eq!((runs[0]["sha"].clone(), runs[0]["state"].clone()), (json!(sha2), json!("running")));
    tick().await;
    assert_eq!(j(&get(d.port, "/cli/ci/status", &agent, "").await)["runs"].as_array().unwrap().len(), 2, "a tip runs once");

    // By hand: one check, on HEAD (a branch name gives the ref).
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": "main", "checks": ["unit"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let manual = j(&r)["run"].clone();
    assert_eq!((manual["sha"].clone(), manual["trigger"].clone(), manual["ref"].clone()), (json!(sha2), json!("manual"), json!("main")));
    assert_eq!(manual["checks"].as_array().unwrap().len(), 1);
    assert!(manual["by"].as_str().unwrap().starts_with("agent:"), "{manual}");
    let r = post(d.port, "/cli/ci/run", &agent, json!({"checks": ["nope"]})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_check_not_found"), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("lint, unit"));

    // Off.
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["changed"].clone(), j(&r)["watch"]["on"].clone()), (json!(true), json!(false)));

    // Each change is audited; the token never lands on disk.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let kinds: Vec<String> = conn
            .prepare("SELECT kind FROM compute_events WHERE workspace_id = ?1 AND kind LIKE 'ci.%' ORDER BY id")
            .unwrap()
            .query_map(params![ws.id], |r| r.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect();
        assert_eq!(kinds, vec!["ci.watch.on", "ci.run", "ci.watch.off"]);
    }
    assert_no_token_on_disk(&env.home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ci_refusals_init_and_freeze() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let gh = start_github();
    k2_daemon::ci_engine::test_set_statuses_api(Some(gh.base.clone()));
    // No .k2/ci.toml committed; the grant lacks --ci.
    let (ws, _bare) = seed_repo_ws(&env, "cibare", None);
    let (other, _) = seed_repo_ws(&env, "ciother", None);
    active_node("cr-1", "rig8", &ws, false);
    let _node = compute_ws::test_attach("cr-1", offer());
    let agent = passport(&ws);

    // Another workspace: forbidden. Its switch off: compute_off.
    let r = get(d.port, "/cli/ci/status", &agent, &format!("workspace={}", other.id)).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "forbidden"), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agents_can_use_compute = 0 WHERE id = ?1", params![other.id]).unwrap();
    }
    let r = get(d.port, "/cli/ci/status", &passport(&other), "").await;
    assert_eq!((r.status, code(&r).as_str()), (403, "compute_off"), "{}", r.body);
    let r = post(d.port, "/cli/ci/watch", &passport(&other), json!({"on": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "compute_off"), "{}", r.body);

    // No config at HEAD: run says what to do.
    let r = post(d.port, "/cli/ci/run", &agent, json!({})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_no_config"), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("k2 ci init"), "{}", r.body);

    // init: a placeholder that fails until edited; never overwrites.
    let r = post(d.port, "/cli/ci/init", &agent, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["placeholder"].clone(), j(&r)["ignored"].clone()), (json!(true), json!(false)));
    let file = ws.path.join(".k2/ci.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    k2_core::compute::proto::ci_config::CiConfig::parse(&text).expect("init writes a config K2 reads");
    let r = post(d.port, "/cli/ci/init", &agent, json!({})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_config_exists"), "{}", r.body);
    let r = post(d.port, "/cli/ci/init", &agent, json!({"force": true, "check": "unit", "argv": ["sh", "-c", "echo a,b"], "memGb": 1, "statusesToken": "K2_CI_STATUSES_TOKEN"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let cfg = k2_core::compute::proto::ci_config::CiConfig::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(cfg.checks["unit"].argv, vec!["sh".to_string(), "-c".to_string(), "echo a,b".to_string()]);
    assert_eq!(cfg.statuses.unwrap().token_secret, "K2_CI_STATUSES_TOKEN");
    let r = post(d.port, "/cli/ci/init", &agent, json!({"force": true, "os": "windows"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_config_invalid"), "{}", r.body);
    git(&ws.path, &["add", ".k2/ci.toml"]);
    git(&ws.path, &["commit", "-q", "-m", "ci"]);

    // A grant without --ci: the run is stored, its check not submitted,
    // and the statuses token is missing (said on the check, not fatal).
    let r = post(d.port, "/cli/ci/run", &agent, json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let run = j(&r)["run"].clone();
    assert_eq!(run["state"], "error", "{run}");
    let unit = check(&run, "unit");
    assert!(unit["description"].as_str().unwrap().contains("not submitted: ci_not_allowed"), "{run}");
    assert!(unit["statusError"].as_str().unwrap().contains("no secret K2_CI_STATUSES_TOKEN"), "{run}");
    assert!(gh.posts().is_empty());
    // --tier standard runs on the plain grant.
    let r = post(d.port, "/cli/ci/run", &agent, json!({"tier": "standard"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let run = j(&r)["run"].clone();
    assert_eq!((run["state"].clone(), run["tier"].clone()), (json!("running"), json!("standard")));
    let job = check(&run, "unit")["job"].as_str().unwrap().to_string();
    assert_eq!(row(&job).tier, "normal");
    // A token not bound to the API's site: the check says which command fixes it.
    put_token(&ws, vec!["api.github.com".into()]);
    tick().await;
    let r = get(d.port, "/cli/ci/status", &agent, &format!("run={}", run["id"].as_str().unwrap())).await;
    let err = check(&j(&r)["run"], "unit")["statusError"].as_str().unwrap().to_string();
    assert!(err.contains("k2 vault hosts K2_CI_STATUSES_TOKEN --add 127.0.0.1"), "{err}");

    // watch on: the remote must answer now.
    git(&ws.path, &["remote", "add", "nowhere", &env.home.join("missing.git").to_string_lossy()]);
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": true, "remote": "nowhere"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_remote_unreachable"), "{}", r.body);
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": true, "remote": "--upload-pack=x"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_watch_invalid"), "{}", r.body);
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": true, "pollSecs": 5})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_watch_invalid"), "{}", r.body);
    // --skip-current: today's tips are seen, only new pushes run.
    let r = post(d.port, "/cli/ci/watch", OWNER, json!({"workspace": ws.id, "on": true, "skipCurrent": true, "branches": "main"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["watch"]["by"], "owner");
    let before = j(&get(d.port, "/cli/ci/status", &agent, "limit=50").await)["runs"].as_array().unwrap().len();
    tick().await;
    assert_eq!(j(&get(d.port, "/cli/ci/status", &agent, "limit=50").await)["runs"].as_array().unwrap().len(), before, "the current tip was skipped");

    // freeze: in the future, at most 30 days; off clears it.
    let now = k2_core::compute::now();
    let r = post(d.port, "/cli/ci/freeze", &agent, json!({"until": now - 10})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = post(d.port, "/cli/ci/freeze", &agent, json!({"until": now + 40 * 86_400})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = post(d.port, "/cli/ci/freeze", &agent, json!({"until": now + 86_400, "why": "0.46.10 release"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["freeze"]["why"], "0.46.10 release");
    assert_eq!(j(&get(d.port, "/cli/ci/status", &agent, "").await)["freeze"]["until"], json!(now + 86_400));
    let r = post(d.port, "/cli/ci/freeze", &agent, json!({"off": true})).await;
    assert_eq!((r.status, j(&r)["freeze"].clone()), (200, Value::Null));

    // People name the workspace; the owner may; nobody else gets in.
    let r = get(d.port, "/cli/ci/status", OWNER, "").await;
    assert_eq!((r.status, code(&r).as_str()), (404, "workspace_not_found"), "{}", r.body);
    let r = get(d.port, "/cli/ci/status", "not-a-token", &format!("workspace={}", ws.id)).await;
    assert!(matches!(r.status, 401 | 403), "a bad token is refused: {} {}", r.status, r.body);
    let r = get(d.port, "/cli/ci/status", OWNER, &format!("workspace={}", ws.id)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Mutations are POST only.
    let r = get(d.port, "/cli/ci/run", OWNER, &format!("workspace={}", ws.id)).await;
    assert_eq!(r.status, 405, "{}", r.body);
}

/// prd-compute-blacksmith-class-v1 O1 (§4.5.2–§4.5.4, BK-10), end to end:
/// a run that ends sends ONE `[ci]` message (no per-check `[compute]` note:
/// `finalize` of a check job delivers nothing), a monitor fires where it
/// says, `k2 ci stats` reads the run, `k2 compute logs --search` finds a
/// line in a check job's log and narrows by check and state; refusals.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_ci_message_per_run_monitors_stats_and_log_search() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let toml = "version = 1\n[checks.unit]\nargv = [\"true\"]\nmem_gb = 1\n[checks.lint]\nargv = [\"sh\", \"-c\", \"exit 3\"]\nmem_gb = 1\n";
    let (ws, _bare) = seed_repo_ws(&env, "o1app", Some(toml));
    let (other, _) = seed_repo_ws(&env, "o1other", None);
    let sha = git(&ws.path, &["rev-parse", "HEAD"]);
    active_node("o1-1", "rig9", &ws, true);
    let _node = compute_ws::test_attach("o1-1", offer());
    let agent = passport(&ws);
    k2_daemon::ci_engine::test_sink_open();

    // Monitors: refusals, then one fail-streak of 1 on main.
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"name": "slow", "kind": "slow"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_monitor_invalid"), "slow needs --check: {}", r.body);
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"name": "Bad Name", "kind": "new-flaky"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_monitor_invalid"), "{}", r.body);
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"name": "away", "kind": "new-flaky", "to": "o1other"})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "forbidden"), "an agent messages only its own workspace: {}", r.body);
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"name": "main-red", "kind": "fail-streak", "branch": "main", "count": 1, "cooldown": "1h"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["monitor"]["to"], json!({"kind": "workspace", "target": "o1app"}));
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"name": "main-red", "kind": "queue-wait"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, "ci_monitor_exists"), "{}", r.body);
    let r = get(d.port, "/cli/ci/monitors", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["monitors"].as_array().unwrap().len(), 1);
    assert_eq!(j(&r)["monitors"][0]["watches"], "main failed 1 run in a row");
    let r = get(d.port, "/cli/ci/monitors", &agent, &format!("workspace={}", other.id)).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "forbidden"), "{}", r.body);
    let r = get(d.port, "/cli/ci/monitor", &agent, "").await;
    assert_eq!(r.status, 405, "a mutation is POST only: {}", r.body);

    // A run on main: unit passes, lint fails.
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": "main"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let run = j(&r)["run"].clone();
    let short = run["id"].as_str().unwrap()[..8].to_string();
    let unit_job = check(&run, "unit")["job"].as_str().unwrap().to_string();
    let lint_job = check(&run, "lint")["job"].as_str().unwrap().to_string();
    // Their logs, as the node streamed them.
    k2_core::compute::logs::append(&unit_job, 1, 1, k2_core::compute::proto::frames::LogStream::Out, b"running 3 tests\ntest ok ... ok\n").unwrap();
    k2_core::compute::logs::append(&lint_job, 1, 1, k2_core::compute::proto::frames::LogStream::Err, b"thread 'main' panicked at src/lib.rs:4\n").unwrap();
    finish_job(&unit_job, 0);
    finish_job(&lint_job, 3);
    tick().await;
    let msgs = k2_daemon::ci_engine::test_sink_take();
    assert_eq!(msgs.len(), 2, "one [ci] message and one monitor message: {msgs:?}");
    let to_ws = k2_core::ci_monitors::To::Workspace(ws.id.clone());
    assert_eq!(msgs[0].0, to_ws);
    assert_eq!(msgs[0].1, format!("[ci] run {short} failed · main @ {} · 1 passed, 1 failed, 0 error · slowest: unit 1m30s · k2 ci status {short}", &sha[..12]));
    assert_eq!(msgs[1].0, to_ws);
    assert!(msgs[1].1.starts_with("[ci] monitor main-red: main failed 1 run in a row"), "{}", msgs[1].1);
    tick().await;
    assert!(k2_daemon::ci_engine::test_sink_take().is_empty(), "once per run");
    // BK-10: the check jobs' own ends send no [compute] note.
    for job in [&unit_job, &lint_job] {
        k2_daemon::compute_ws::finalize(job);
        assert!(row(job).notified_at.is_some(), "marked, so nothing sends it later");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(k2_core::inbox::list_all(&ws.path).is_empty(), "no [compute] note in the inbox: {:?}", k2_core::inbox::list_all(&ws.path));

    // Stats.
    let r = get(d.port, "/cli/ci/stats", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let checks = j(&r)["checks"].clone();
    let by = |n: &str| checks.as_array().unwrap().iter().find(|c| c["check"] == n).cloned().unwrap_or_else(|| panic!("{n}: {checks}"));
    assert_eq!((by("unit")["runs"].as_i64(), by("unit")["passed"].as_i64(), by("unit")["p50Secs"].as_i64()), (Some(1), Some(1), Some(90)));
    assert_eq!((by("lint")["failed"].as_i64(), by("lint")["failureRate"].as_f64()), (Some(1), Some(1.0)));
    assert_eq!(by("unit")["cacheHitRate"], Value::Null, "no generations yet");
    let r = get(d.port, "/cli/ci/stats", &agent, "check=unit&branch=nope").await;
    assert_eq!(j(&r)["checks"], json!([]), "{}", r.body);
    let r = get(d.port, "/cli/ci/stats", &agent, "days=500").await;
    assert_eq!(r.status, 400, "{}", r.body);

    // Log search.
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=panicked").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["matches"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!((v["matches"][0]["job"].as_str(), v["matches"][0]["check"].as_str(), v["matches"][0]["stream"].as_str()), (Some(lint_job.as_str()), Some("lint"), Some("err")));
    assert_eq!(v["matches"][0]["line"], "thread 'main' panicked at src/lib.rs:4");
    assert_eq!(v["cut"], Value::Null);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=PANICKED").await;
    assert!(j(&r)["matches"].as_array().unwrap().is_empty(), "case matters without fold");
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=PANICKED&fold=1").await;
    assert_eq!(j(&r)["matches"].as_array().unwrap().len(), 1, "{}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=panicked&check=unit").await;
    assert!(j(&r)["matches"].as_array().unwrap().is_empty(), "--check unit: {}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=ok&state=passed").await;
    let jobs: Vec<String> = j(&r)["matches"].as_array().unwrap().iter().map(|m| m["job"].as_str().unwrap().to_string()).collect();
    assert_eq!(jobs, vec![unit_job.clone()], "--state passed: {}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=test&limit=1").await;
    assert_eq!(j(&r)["cut"], "limit", "{}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=x").await;
    assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "too short: {}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, "search=panicked&since=30d").await;
    assert_eq!(r.status, 400, "logs are kept 14 days: {}", r.body);
    let r = get(d.port, "/cli/compute/logs/search", &agent, &format!("search=panicked&workspace={}", other.id)).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "forbidden"), "{}", r.body);

    // mute and rm.
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"action": "mute", "name": "main-red", "for": "1d"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["monitor"]["mutedUntil"].as_i64().unwrap() > k2_core::compute::now());
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"action": "mute", "name": "main-red", "for": "40d"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "ci_monitor_invalid"), "{}", r.body);
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"action": "rm", "name": "main-red"})).await;
    assert_eq!((r.status, j(&r)["removed"].as_str()), (200, Some("main-red")), "{}", r.body);
    let r = post(d.port, "/cli/ci/monitor", &agent, json!({"action": "rm", "name": "main-red"})).await;
    assert_eq!((r.status, code(&r).as_str()), (404, "ci_monitor_not_found"), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let kinds: Vec<String> = conn
            .prepare("SELECT kind FROM compute_events WHERE workspace_id = ?1 AND kind LIKE 'ci.monitor.%' ORDER BY id")
            .unwrap()
            .query_map(params![ws.id], |r| r.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect();
        assert_eq!(kinds, vec!["ci.monitor.add", "ci.monitor.mute", "ci.monitor.rm"], "audited");
    }
    k2_daemon::ci_engine::test_sink_close();
}

/// prd-compute-blacksmith-class-v1 B2 (§4.1.4, §4.1.5, §10): the trusted
/// cache is saved only by CI checks of a trusted branch at a pushed commit,
/// as the plan's `cache_commit` says; `--ref main` on a local commit and a
/// plain run at main save only scratch; an agent narrows the trusted
/// branches but never widens them; a node without cache generations gets
/// no cache fields; the nodes' lineages reach `k2 compute cache --lineage`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trusted_cache_is_only_for_verified_trusted_branch_checks() {
    use k2_core::compute::proto::frames::Frame;
    use k2_core::compute::proto::gens::{CacheCommit, LineageInfo, LineageStatus};
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let (ws, _bare) = seed_repo_ws(&env, "trustapp", Some(CI_TOML));
    let sha1 = git(&ws.path, &["rev-parse", "HEAD"]);
    active_node("gn-1", "k2g7", &ws, true);
    let mut gens_offer = offer();
    gens_offer.protocol = 10;
    gens_offer.features.extend(["cache-gens".to_string(), "job-uid".to_string()]);
    let mut node = compute_ws::test_attach("gn-1", gens_offer);
    let agent = passport(&ws);

    // The trusted branches: main by default; an agent can't add one.
    let r = get(d.port, "/cli/ci/cache/trust", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["branches"].clone(), j(&r)["default"].clone()), (json!(["main"]), json!(true)));
    let r = post(d.port, "/cli/ci/cache/trust/set", &agent, json!({"add": ["feat"]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "cache_trust_needs_person"), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("only a person"), "{}", r.body);
    let r = post(d.port, "/cli/ci/cache/trust/set", &agent, json!({"set": ["main", "feat"]})).await;
    assert_eq!(code(&r), "cache_trust_needs_person", "--set that adds is widening too");
    let r = post(d.port, "/cli/ci/cache/trust/set", OWNER, json!({"workspace": ws.id, "add": ["rel"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["branches"], json!(["main", "rel"]));
    let r = post(d.port, "/cli/ci/cache/trust/set", &agent, json!({"remove": ["rel"]})).await;
    assert_eq!((r.status, j(&r)["branches"].clone()), (200, json!(["main"])), "an agent narrows: {}", r.body);

    // A watched push to main: its checks may save the trusted cache, and
    // the plan names the verified branch.
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    tick().await;
    let run = j(&get(d.port, "/cli/ci/status", &agent, "").await)["runs"][0].clone();
    let unit = check(&run, "unit")["job"].as_str().expect("unit job").to_string();
    let plan = row(&unit).plan().expect("plan");
    assert_eq!((plan.cache_commit, plan.cache_ref.as_deref()), (Some(CacheCommit::Trusted), Some("main")), "watched trusted push");
    let _ = node.frames();

    // A local commit labelled main by hand: not on the remote's main, so
    // it saves only scratch/main (rule 1, BK-1); the check fetched the
    // remote's main into a K2-owned ref, never a local branch.
    std::fs::write(ws.path.join("a.txt"), "local\n").unwrap();
    git(&ws.path, &["commit", "-q", "-am", "local only"]);
    let local = git(&ws.path, &["rev-parse", "HEAD"]);
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": local, "ref": "main", "checks": ["unit"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_local = check(&j(&r)["run"], "unit")["job"].as_str().unwrap().to_string();
    let plan = row(&job_local).plan().expect("plan");
    assert_eq!((plan.cache_commit, plan.cache_ref.as_deref()), (Some(CacheCommit::Scratch), Some("main")), "a local commit never saves trusted");
    assert_eq!(git(&ws.path, &["rev-parse", "refs/k2-ci/main"]), sha1, "the remote's main, in K2's own ref");
    // The same commit once pushed: trusted.
    git(&ws.path, &["push", "-q", "origin", "main"]);
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": local, "ref": "main", "checks": ["unit"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_pushed = check(&j(&r)["run"], "unit")["job"].as_str().unwrap().to_string();
    assert_eq!(row(&job_pushed).plan().unwrap().cache_commit, Some(CacheCommit::Trusted), "pushed to main: trusted");

    // The remote is pinned (review item 1): the agent repoints origin at a
    // repo it controls and pushes there; its checks save only scratch.
    let evil = env.home.join("trustapp-evil.git");
    std::fs::create_dir_all(&evil).unwrap();
    git(&evil, &["init", "-q", "--bare"]);
    git(&ws.path, &["remote", "set-url", "origin", evil.to_str().unwrap()]);
    std::fs::write(ws.path.join("a.txt"), "evil\n").unwrap();
    git(&ws.path, &["commit", "-q", "-am", "pushed to another remote"]);
    git(&ws.path, &["push", "-q", "origin", "main"]);
    let evil_sha = git(&ws.path, &["rev-parse", "HEAD"]);
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": evil_sha, "ref": "main", "checks": ["unit"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_evil = check(&j(&r)["run"], "unit")["job"].as_str().unwrap().to_string();
    assert_eq!(row(&job_evil).plan().unwrap().cache_commit, Some(CacheCommit::Scratch), "a repointed origin: no trusted save");
    // Only a person records the new remote; then the same commit is trusted.
    let r = post(d.port, "/cli/ci/cache/trust/set", &agent, json!({"set": ["main"]})).await;
    assert_eq!(r.status, 200, "an agent's same-list set is no widening: {}", r.body);
    assert!(!j(&r)["remote"].as_str().unwrap_or_default().contains("trustapp-evil"), "an agent never records a remote: {}", r.body);
    let r = post(d.port, "/cli/ci/cache/trust/set", OWNER, json!({"workspace": ws.id, "set": ["main"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["remote"].as_str().unwrap().ends_with("trustapp-evil"), "{}", r.body);
    let r = post(d.port, "/cli/ci/run", &agent, json!({"rev": evil_sha, "ref": "main", "checks": ["unit"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_evil2 = check(&j(&r)["run"], "unit")["job"].as_str().unwrap().to_string();
    assert_eq!(row(&job_evil2).plan().unwrap().cache_commit, Some(CacheCommit::Trusted), "a person re-set trust with the new remote");
    // Back to the first remote (the project key below is its).
    git(&ws.path, &["remote", "set-url", "origin", "https://github.com/acme/app.git"]);

    // A plain run at main saves only scratch/main (its argv is the
    // caller's); a body naming the trusted cache is not read.
    let r = post(d.port, "/cli/compute/run", &agent, json!({"argv": ["true"], "sha": "main", "node": "k2g7", "cacheCommit": "trusted", "clientJobId": "b2-plain-run-at-main-01"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let plan = row(&job).plan().unwrap();
    assert_eq!((plan.cache_commit, plan.cache_ref.as_deref()), (Some(CacheCommit::Scratch), Some("main")), "a plain run never saves trusted");

    // A node without cache generations gets no cache fields; a secret
    // job's fork (`none`) only goes to a node giving it its own uid.
    let jrow = row(&job);
    let fitted = k2_daemon::compute_place::fit_plan(&plan, &jrow, &offer().features, None).unwrap();
    assert_eq!((fitted.cache_commit, fitted.cache_ref), (None, None), "stripped for a node without cache-gens");
    let mut secret_plan = plan.clone();
    (secret_plan.cache_commit, secret_plan.cache_ref) = (Some(CacheCommit::None), None);
    let gens_only = vec!["cache-gens".to_string()];
    assert_eq!(k2_daemon::compute_place::fit_plan(&secret_plan, &jrow, &gens_only, None).unwrap().cache_commit, None, "no job-uid: no secret fork");
    let both = vec!["cache-gens".to_string(), "job-uid".to_string()];
    assert_eq!(k2_daemon::compute_place::fit_plan(&secret_plan, &jrow, &both, None).unwrap().cache_commit, Some(CacheCommit::None));

    // The node's lineages reach `k2 compute cache --lineage`.
    let key = row(&unit).plan().unwrap().src.unwrap().project_key;
    let li = |lineage: &str, seq: u64| LineageInfo { key: key.clone(), name: "target".into(), lineage: lineage.into(), head_seq: seq, head_commit: sha1.clone(), bytes: 1 << 30, last_used: 1, forks: 0 };
    compute_ws::test_frame("gn-1", Frame::LineageStatus(LineageStatus { at: 1, lineages: vec![li("trusted", 3), li("scratch/feat", 1)], truncated: false }));
    let r = get(d.port, "/cli/compute/cache/lineages", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["nodes"][0]["lineages"].as_array().unwrap().len(), 2, "{}", r.body);
    let r = get(d.port, "/cli/compute/cache/lineages", &agent, "lineage=trusted").await;
    assert_eq!(j(&r)["nodes"][0]["lineages"].as_array().unwrap().len(), 1, "{}", r.body);
    assert_eq!(j(&r)["nodes"][0]["lineages"][0]["headSeq"], 3);

    // Every run of this workspace ends here (its [ci] messages never land
    // in a later test's sink), and the watch goes off.
    let lint = check(&run, "lint")["job"].as_str().expect("lint job").to_string();
    for jb in [&unit, &lint, &job_local, &job_pushed, &job_evil, &job_evil2] {
        // A newer run on main superseded some (cancelled): those have ended.
        if !matches!(row(jb).state.as_str(), "done" | "failed" | "cancelled" | "timeout" | "interrupted") {
            finish_job(jb, 0);
        }
    }
    tick().await;
    let r = post(d.port, "/cli/ci/watch", &agent, json!({"on": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let runs = j(&get(d.port, "/cli/ci/status", &agent, "").await)["runs"].clone();
    assert!(runs.as_array().unwrap().iter().all(|r| r["state"] != "running"), "{runs}");
}
