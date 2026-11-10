//! Compute git access, the controller side (prd-compute-git-access-v1,
//! GH#78): the real dispatcher in-process (temp `$HOME`, loopback, the
//! vault on a file key there), a fake node session
//! (`compute_ws::test_attach` / `test_frame`) and a fake GitHub API on
//! loopback that checks every App JWT's RS256 signature, issuer and expiry.
//!
//! Covers: setup (App key checked with GitHub, refused classic and login
//! tokens, owner/Admin only); grant (a person; an agent gets
//! `git_access_needs_person` naming the repos; each repo checked with
//! GitHub; another server's node only by name with --shared, App only);
//! the plan's `git_access` filled at assign for a node offering
//! `git-access`; `git_cred_need` answered with a token minted for exactly
//! that repo (contents: read) that opens with the node's one-time key and
//! nothing else; refusals (an ungranted repo, a stale attempt, a revoked
//! grant); the token reused while fresh; revoke deleting it at GitHub; a
//! fine-grained token checked and handed out as it is. No token is ever in
//! an HTTP answer, an event or a file. Fake tokens and keys only.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::compute::proto::frames::{Availability, Caps, Control, Frame, Free, GitCredNeed, Offer};
use k2_core::compute::proto::gitcred::{self, codes as gcodes, NodeKey, Purpose, FEATURE_GIT_ACCESS};
use k2_core::compute::proto::share::{Role, ServerStatus, ShareLimits, ShareView, Trust, FEATURE_QUEUE, FEATURE_SERVERS};
use k2_core::compute::store;
use k2_core::session::SessionId;
use k2_daemon::compute_ws;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-compute-git-v1";
const APP_ID: &str = "4242";
/// Every fake token starts with this (the on-disk and wire scans look for it).
const TOKEN_MARK: &str = "ghs_k2testFAKEGH";
const PAT: &str = "github_pat_11K2TEST0fake_finegrainedTOKENvalue0123456789";

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
        k2_daemon::compute_git::test_set_api_base(None);
        k2_core::compute::retry::set_tuning_for_tests(Default::default());
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cgit-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");
    k2_core::compute::retry::set_tuning_for_tests(k2_core::compute::retry::Tuning { tick_millis: 3_600_000, any_hold_secs: 0, retry_base_secs: 0, ..Default::default() });
    env
}

// ── a fake GitHub API ────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    auth: String,
    body: String,
}

struct FakeGithub {
    base: String,
    public: Vec<u8>,
    seen: Arc<StdMutex<Vec<Seen>>>,
    minted: Arc<StdMutex<Vec<String>>>,
}

/// The App's key (fresh per test) as a PKCS#8 PEM, and its public key for
/// the fake to check signatures with.
fn app_key() -> (String, Vec<u8>) {
    k2_core::compute::git::testing::app_key_pem()
}

/// Is `auth` a `Bearer <JWT>` this App signed, issued now, not expired?
fn jwt_ok(auth: &str, public: &[u8]) -> bool {
    let Some(jwt) = auth.strip_prefix("Bearer ") else { return false };
    let Some(c) = k2_core::compute::git::testing::verify_jwt(jwt, public) else { return false };
    let now = k2_core::compute::now();
    c["iss"] == APP_ID && c["exp"].as_i64().is_some_and(|e| e > now && e <= now + 600) && c["iat"].as_i64().is_some_and(|i| i <= now)
}

fn start_github(public: Vec<u8>) -> FakeGithub {
    let kept = public.clone();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<StdMutex<Vec<Seen>>> = Default::default();
    let minted: Arc<StdMutex<Vec<String>>> = Default::default();
    let (s2, m2) = (seen.clone(), minted.clone());
    std::thread::spawn(move || {
        for c in listener.incoming() {
            let Ok(mut c) = c else { continue };
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // Headers, then Content-Length bytes of body.
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
            let body = String::from_utf8_lossy(&body).to_string();
            s2.lock().unwrap().push(Seen { method: method.clone(), path: path.clone(), auth: auth.clone(), body: body.clone() });
            let jwt = jwt_ok(&auth, &public);
            let (status, out): (u16, Value) = match (method.as_str(), path.as_str()) {
                ("GET", "/app") if jwt => (200, json!({"slug": "acme-compute"})),
                ("GET", "/repos/acme/app/installation") | ("GET", "/repos/acme/lib/installation") if jwt => (200, json!({"id": 42})),
                ("GET", p) if p.ends_with("/installation") && jwt => (404, json!({"message": "Not Found"})),
                ("POST", "/app/installations/42/access_tokens") if jwt => {
                    let b: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    if b["permissions"] != json!({"contents": "read"}) || b["repositories"].as_array().is_none_or(|r| r.len() != 1) {
                        (422, json!({"message": "K2 asked for more than contents:read on one repo"}))
                    } else {
                        let mut m = m2.lock().unwrap();
                        let t = format!("{TOKEN_MARK}{:04}x{}", m.len(), b["repositories"][0].as_str().unwrap_or(""));
                        m.push(t.clone());
                        let exp = chrono::DateTime::from_timestamp(k2_core::compute::now() + 3600, 0).unwrap().to_rfc3339();
                        (201, json!({"token": t, "expires_at": exp}))
                    }
                }
                ("DELETE", "/installation/token") if auth.starts_with(&format!("Bearer {TOKEN_MARK}")) => (204, Value::Null),
                ("GET", "/repos/acme/app") | ("GET", "/repos/acme/lib") if auth == format!("Bearer {PAT}") => (200, json!({"full_name": "acme/app"})),
                _ => (401, json!({"message": "Bad credentials"})),
            };
            let text = if out.is_null() { String::new() } else { out.to_string() };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).as_bytes());
        }
    });
    FakeGithub { base, public: kept, seen, minted }
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
    assert!(!body.contains(TOKEN_MARK) && !body.contains(PAT), "an HTTP answer carries a token: {body}");
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

struct Ws {
    id: String,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", params![id]).unwrap();
    Ws { id }
}

fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(&sid, &sid.to_string(), HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() }, CredMode::ApiKey, Provider::Anthropic)
}

fn node_fp(id: &str) -> String {
    format!("{id:0>64}")
}

fn active_node(id: &str, name: &str, ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let labels = std::collections::BTreeMap::from([("os".to_string(), "linux".to_string())]);
    store::insert_pending_node(&conn, id, name, &node_fp(id), "pem", &labels, "owner", "123456", 1).expect("node");
    store::confirm_node(&conn, id, "owner", 2).expect("confirm");
    store::set_grant(&conn, id, &ws.id, &store::GrantLimits::default(), "owner", 3).expect("grant");
}

fn offer(features: &[&str], share: Option<ShareView>) -> Offer {
    Offer {
        share,
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
        tools: Default::default(),
        protocol: 8,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.9".into(),
        features: features.iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
    }
}

fn tenant_view() -> ShareView {
    ShareView {
        role: Role::Tenant,
        slot: 1,
        shared: true,
        servers: 2,
        owner_label: Some("other.k2.dev".into()),
        state: ServerStatus::Active,
        limits: ShareLimits::new_server(Trust::Team, false),
        running: 0,
        queued: 0,
        cpu_secs_today: 0,
        expires_in_secs: None,
        skew_secs: None,
        disk_used_bytes: None,
        quotes: vec![],
        priority: None,
    }
}

fn row(job: &str) -> store::JobRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::job_by_id(&conn, job).expect("job row")
}

fn events(kind: &str, job: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT detail_json FROM compute_events WHERE kind = ?1 AND job_id = ?2 ORDER BY id").unwrap();
    st.query_map(params![kind, job], |r| r.get::<_, String>(0)).unwrap().map(|x| x.unwrap()).collect()
}

async fn run(port: u16, token: &str, mut body: Value) -> Resp {
    body["argv"] = json!(["true"]);
    body["noSource"] = json!(true);
    body["clientJobId"] = json!(uuid::Uuid::new_v4().simple().to_string());
    post(port, "/cli/compute/run", token, body).await
}

fn need(job: &str, r: &store::JobRow, req: u64, repo: &str, key: &NodeKey) -> Frame {
    Frame::GitCredNeed(GitCredNeed {
        job_id: job.into(),
        generation: r.generation as u32,
        attempt: r.attempt as u32,
        plan_digest: r.plan_digest.clone(),
        req,
        repo: repo.into(),
        purpose: Purpose::Job,
        epk: key.public_b64(),
    })
}

fn binding(node: &str, r: &store::JobRow, req: u64, repo: &str) -> gitcred::Binding {
    gitcred::Binding {
        controller_fp: k2_core::compute::controller_key().expect("controller key").fingerprint(),
        node_fp: node_fp(node),
        job_id: r.id.clone(),
        generation: r.generation as u32,
        attempt: r.attempt as u32,
        plan_digest: r.plan_digest.clone(),
        req,
        repo: repo.into(),
        purpose: Purpose::Job,
    }
}

enum Answer {
    Cred(k2_core::compute::proto::frames::GitCred),
    Refused(String, String),
}

fn answer(conn: &mut compute_ws::TestConn, job: &str, req: u64) -> Answer {
    assert!(compute_ws::test_wait_releases(Duration::from_secs(20)), "release thread never finished");
    let mut answers: Vec<Answer> = conn
        .frames()
        .into_iter()
        .filter_map(|f| match f {
            Frame::GitCred(c) if c.job_id == job && c.req == req => Some(Answer::Cred(c)),
            Frame::GitCredRefused(r) if r.job_id == job && r.req == req => Some(Answer::Refused(r.code, r.message)),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "exactly one answer per git_cred_need");
    answers.pop().unwrap()
}

/// Fail if a token is in any file under `root` (k2.db and its WAL, the
/// compute dir, logs; the vault holds the credential only encrypted).
fn assert_no_token_on_disk(root: &Path, tokens: &[String]) {
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
            for t in tokens {
                assert!(!bytes.windows(t.len()).any(|w| w == t.as_bytes()), "a token is on disk in {}", p.display());
            }
        }
    }
    assert!(scanned > 3, "scanned only {scanned} files under {}", root.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_access_with_a_github_app_end_to_end() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let (pem, public) = app_key();
    let gh = start_github(public);
    k2_daemon::compute_git::test_set_api_base(Some(gh.base.clone()));
    let ws = seed_ws(&env, "gaw");
    active_node("own-1", "k2n4", &ws);
    active_node("ten-1", "tenant-node", &ws);
    let mut node = compute_ws::test_attach("own-1", offer(&[FEATURE_GIT_ACCESS, "artifacts", "stall"], None));
    let _ten = compute_ws::test_attach("ten-1", offer(&[FEATURE_GIT_ACCESS, "artifacts", "stall", FEATURE_QUEUE, FEATURE_SERVERS], Some(tenant_view())));
    let agent = passport(&ws);

    // Nothing yet; an agent reads its own.
    let r = get(d.port, "/cli/compute/git-access", &agent, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["credential"].clone(), j(&r)["grants"].clone()), (Value::Null, json!([])));

    // A grant needs the credential first.
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/app"})).await;
    assert_eq!((r.status, code(&r).as_str()), (409, gcodes::NO_CREDENTIAL), "{}", r.body);

    // Setup: never an agent; never a classic token or a login; the App's
    // key is checked with GitHub (a signed JWT).
    let r = post(d.port, "/cli/compute/git-access/setup", &agent, json!({"app_id": APP_ID, "key_pem": pem})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "owner_only"), "{}", r.body);
    for bad in ["ghp_k2testClassic0123456789abcdef", "gho_k2testLogin0123456789abcdef"] {
        let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"token": bad})).await;
        assert_eq!((r.status, code(&r).as_str()), (400, "git_access_bad_credential"), "{bad}: {}", r.body);
    }
    let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"app_id": "12x", "key_pem": pem})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, "git_access_bad_credential"), "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"app_id": APP_ID, "key_pem": pem})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["credential"]["app"], "acme-compute");
    assert!(gh.seen.lock().unwrap().iter().any(|s| s.method == "GET" && s.path == "/app" && jwt_ok(&s.auth, &gh.public)), "GitHub saw a signed JWT");

    // An agent asks: refused with the repos for its card.
    let r = post(d.port, "/cli/compute/git-access/grant", &agent, json!({"github": ["acme/app", "Acme/Lib"]})).await;
    assert_eq!((r.status, code(&r).as_str()), (403, "git_access_needs_person"), "{}", r.body);
    assert_eq!(j(&r)["error"]["repos"], "acme/app,acme/lib");
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("k2 compute git-access grant --github acme/app,acme/lib"), "{}", r.body);

    // The owner: a repo the App can't reach fails the grant, not a job.
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/missing"})).await;
    assert_eq!((r.status, code(&r).as_str()), (502, "git_access_check_failed"), "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["grants"], json!([{"repo": "acme/app", "key": "github.com/acme/app", "nodes": "all", "shared": false, "grantedBy": "owner", "grantedAt": j(&r)["grants"][0]["grantedAt"]}]));

    // Another server's node: only named, only with --shared, with a warning.
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/lib", "nodes": "tenant-node"})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, gcodes::SHARED_NEEDS_OPTIN), "{}", r.body);
    assert!(j(&r)["error"]["hint"].as_str().unwrap().contains("another K2 server (other.k2.dev)"), "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/lib", "nodes": "tenant-node", "shared": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["warnings"][0].as_str().unwrap().contains("admin (root)"), "{}", r.body);

    // A job on the owned node: its plan names the repos granted there
    // (acme/app by `all`; acme/lib only on the tenant node).
    let r = run(d.port, &agent, json!({"node": "k2n4"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let plan = node
        .frames()
        .iter()
        .find_map(|f| match f {
            Frame::Assign(a) if a.fence.job_id == job => Some(a.plan.clone()),
            _ => None,
        })
        .expect("assigned");
    assert_eq!(plan.git_access, vec!["github.com/acme/app".to_string()]);
    let r1 = row(&job);
    assert_eq!(r1.plan_digest, plan.digest().unwrap(), "the stored plan is what was sent");

    // The release: a token for exactly acme/app, contents:read, that opens
    // with the node's one-time key and binding only.
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("own-1", need(&job, &r1, 1, "github.com/acme/app", &key));
    let Answer::Cred(c) = answer(&mut node, &job, 1) else { panic!("refused") };
    let opened = key.open(&c.epk, &c.ct, &binding("own-1", &r1, 1, "github.com/acme/app")).expect("opens");
    assert!(opened.password.starts_with(TOKEN_MARK) && opened.password.ends_with("xapp"), "{opened:?}");
    assert_eq!(opened.username, "x-access-token");
    assert!(opened.expires_at > k2_core::compute::now() + 3000);
    assert!(NodeKey::generate().unwrap().open(&c.epk, &c.ct, &binding("own-1", &r1, 1, "github.com/acme/app")).is_err(), "another key can't open it");
    // One mint per repo, each for exactly that repo with contents:read
    // (the grant's check minted them; the release reuses acme/app's).
    let mint = gh.seen.lock().unwrap().iter().filter(|s| s.method == "POST").cloned().collect::<Vec<_>>();
    let bodies: Vec<Value> = mint.iter().map(|s| serde_json::from_str::<Value>(&s.body).unwrap()).collect();
    assert_eq!(
        bodies,
        vec![json!({"repositories": ["app"], "permissions": {"contents": "read"}}), json!({"repositories": ["lib"], "permissions": {"contents": "read"}})],
        "{mint:?}"
    );
    let rel = events("git_cred_release", &job);
    assert_eq!(rel.len(), 1);
    assert!(rel[0].contains("github.com/acme/app") && !rel[0].contains(TOKEN_MARK), "{}", rel[0]);

    // Fresh: the same token again, no second mint.
    let key2 = NodeKey::generate().unwrap();
    compute_ws::test_frame("own-1", need(&job, &r1, 2, "github.com/acme/app", &key2));
    let Answer::Cred(c2) = answer(&mut node, &job, 2) else { panic!("refused") };
    assert_eq!(key2.open(&c2.epk, &c2.ct, &binding("own-1", &r1, 2, "github.com/acme/app")).unwrap().password, opened.password);
    assert_eq!(gh.minted.lock().unwrap().iter().filter(|t| t.ends_with("xapp")).count(), 1, "reused while fresh");

    // Refused: a repo not in the plan, a stale attempt.
    compute_ws::test_frame("own-1", need(&job, &r1, 3, "github.com/acme/lib", &NodeKey::generate().unwrap()));
    assert!(matches!(answer(&mut node, &job, 3), Answer::Refused(code, _) if code == gcodes::NOT_GRANTED));
    let mut stale = r1.clone();
    stale.attempt += 1;
    compute_ws::test_frame("own-1", need(&job, &stale, 4, "github.com/acme/app", &NodeKey::generate().unwrap()));
    assert!(matches!(answer(&mut node, &job, 4), Answer::Refused(code, _) if code == gcodes::STALE));
    // Another node asking for this job: dropped, no answer.
    compute_ws::test_frame("ten-1", need(&job, &r1, 5, "github.com/acme/app", &NodeKey::generate().unwrap()));
    assert!(compute_ws::test_wait_releases(Duration::from_secs(20)));
    assert!(node.frames().is_empty(), "another node's ask gets no answer");

    // The agent revokes its own: the App token is deleted at GitHub, the
    // next ask is refused though the plan still names the repo.
    let r = post(d.port, "/cli/compute/git-access/revoke", &agent, json!({"github": "acme/app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((j(&r)["removed"].clone(), j(&r)["tokensDeleted"].clone()), (json!(1), json!(1)), "{}", r.body);
    assert_eq!(j(&r)["runningJobs"][0]["repo"], "acme/app", "{}", r.body);
    assert!(gh.seen.lock().unwrap().iter().any(|s| s.method == "DELETE" && s.auth == format!("Bearer {}", opened.password)), "the token was deleted at GitHub");
    compute_ws::test_frame("own-1", need(&job, &r1, 6, "github.com/acme/app", &NodeKey::generate().unwrap()));
    let Answer::Refused(refused, msg) = answer(&mut node, &job, 6) else { panic!("released after a revoke") };
    assert_eq!(refused, gcodes::NOT_GRANTED);
    assert!(msg.contains("revoked"), "{msg}");
    assert_eq!(events("git_cred_refused", &job).len(), 3, "each refusal is on the job");

    // A fine-grained token never goes to another server's node.
    let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"token": PAT})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/app", "nodes": "tenant-node", "shared": true})).await;
    assert_eq!((r.status, code(&r).as_str()), (400, gcodes::PAT_SHARED), "{}", r.body);

    let mut tokens: Vec<String> = gh.minted.lock().unwrap().clone();
    tokens.push(PAT.to_string());
    assert_no_token_on_disk(&env.home, &tokens);
    compute_ws::test_detach("own-1");
    compute_ws::test_detach("ten-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fine_grained_token_is_checked_and_handed_out_as_it_is() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let (_, public) = app_key();
    let gh = start_github(public);
    k2_daemon::compute_git::test_set_api_base(Some(gh.base.clone()));
    let ws = seed_ws(&env, "pat");
    active_node("own-2", "k2n7", &ws);
    let mut node = compute_ws::test_attach("own-2", offer(&[FEATURE_GIT_ACCESS, "artifacts", "stall"], None));
    let old_node = {
        active_node("old-2", "k2n5", &ws);
        compute_ws::test_attach("old-2", offer(&["artifacts", "stall"], None))
    };
    let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"token": PAT})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["note"].as_str().unwrap().contains("can't mint short-lived tokens"), "{}", r.body);
    // The token is checked against the repo (GitHub says no to another).
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/nope"})).await;
    assert_eq!((r.status, code(&r).as_str()), (502, "git_access_check_failed"), "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["tokenNote"].as_str().is_some(), "{}", r.body);

    // A node without the feature gets no git_access in its plan.
    let agent = passport(&ws);
    let r = run(d.port, &agent, json!({"node": "k2n5"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job_old = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let mut old_node = old_node;
    let p_old = old_node.frames().iter().find_map(|f| match f {
        Frame::Assign(a) if a.fence.job_id == job_old => Some(a.plan.clone()),
        _ => None,
    });
    assert!(p_old.expect("assigned").git_access.is_empty(), "an older node never sees git_access");

    let r = run(d.port, &agent, json!({"node": "k2n7"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let job = j(&r)["job"]["id"].as_str().unwrap().to_string();
    let r1 = row(&job);
    assert_eq!(r1.plan().unwrap().git_access, vec!["github.com/acme/app".to_string()]);
    let key = NodeKey::generate().unwrap();
    compute_ws::test_frame("own-2", need(&job, &r1, 1, "github.com/acme/app", &key));
    let Answer::Cred(c) = answer(&mut node, &job, 1) else { panic!("refused") };
    let opened = key.open(&c.epk, &c.ct, &binding("own-2", &r1, 1, "github.com/acme/app")).expect("opens");
    assert_eq!(opened.password, PAT, "a fine-grained token goes as it is");
    assert!(events("git_cred_release", &job)[0].contains("\"kind\":\"token\""));

    let r = post(d.port, "/cli/compute/git-access/revoke", OWNER, json!({"workspace": ws.id, "all": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(j(&r)["note"].as_str().unwrap().contains("rotate"), "{}", r.body);
    assert_eq!(j(&r)["tokensDeleted"], 0);
    assert!(!gh.seen.lock().unwrap().iter().any(|s| s.method == "DELETE"), "a fine-grained token is never deleted by K2");

    // Cleared: every later ask is refused.
    let r = post(d.port, "/cli/compute/git-access/grant", OWNER, json!({"workspace": ws.id, "github": "acme/app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/compute/git-access/setup", OWNER, json!({"clear": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    compute_ws::test_frame("own-2", need(&job, &r1, 2, "github.com/acme/app", &NodeKey::generate().unwrap()));
    assert!(matches!(answer(&mut node, &job, 2), Answer::Refused(code, _) if code == gcodes::NO_CREDENTIAL));
    assert_no_token_on_disk(&env.home, &[PAT.to_string()]);
    compute_ws::test_detach("own-2");
    compute_ws::test_detach("old-2");
}
