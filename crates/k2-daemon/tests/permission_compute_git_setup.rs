//! The GitHub credential for compute git access as a permission card
//! (0.46.10, `compute.git-access-setup`): the end to end an agent and a
//! person walk, on a real in-process daemon (temp `$HOME`, loopback, the
//! vault on a file key there) with a fake GitHub on loopback that checks
//! every App JWT's signature.
//!
//! With "Permission cards (Beta)" on for the agent:
//! - the agent's grant on a server with no credential is refused
//!   `git_access_needs_setup` and ends with the exact setup ask; its own
//!   `k2 compute git-access setup` teaches the same card; asking for the
//!   grant card itself is refused (409) with that line;
//! - the setup card is a person's: a Member and the agent can't answer
//!   it, and an Admin's Allow before the credential exists decides nothing;
//! - K2's own form posts the App id and key to the setup route as the
//!   Admin (the route `k2 compute git-access setup` uses): the key lands
//!   in this server's vault (system namespace) and nowhere the agent can
//!   read: not an answer it gets, not its Thread, not `k2.db`;
//! - Allow then closes the card; the agent's grant now asks for the
//!   grant card, and an Admin's Allow on it mints a token for that repo at
//!   the fake GitHub;
//! - a fine-grained token from the form replaces the App key in the vault.
//!
//! Its own binary: it flips the process-wide switch. No real `~/.k2`,
//! keychain or running daemon is touched. Fake keys and tokens only.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use k2_vault::{Requester, Scope, SecretStore, Selector, Use};
use serde_json::{json, Value};

const OWNER: &str = "owner-token-permission-compute-git-setup";
const APP_ID: &str = "4242";
const PAT: &str = "github_pat_11K2TEST0fake_finegrainedSETUPCARD0123456789";
const SETUP_ASK: &str = "k2 permission request compute.git-access-setup --why \"<what you need it for>\"";
const GRANT_ASK: &str = "k2 permission request compute.git-access repos=acme/app --why \"<what you need it for>\"";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }
    fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {} {}", self.status, self.body)).to_string()
    }
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
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
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn call(port: u16, method: &str, path: &str, token: &str, body: Option<Value>) -> Resp {
    let sep = if path.contains('?') { '&' } else { '?' };
    let pq = format!("{path}{sep}token={}", enc(token));
    let method = method.to_string();
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    call(port, "POST", path, token, Some(body)).await
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        k2_daemon::compute_git::test_set_api_base(None);
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Ws {
    id: String,
    handle: String,
    conv: String,
}

fn workspace(home: &Path, handle: &str) -> Ws {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).unwrap();
    let path = path.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path]).unwrap();
    conn.execute("UPDATE projects SET agents_can_use_compute = 1 WHERE id = ?1", rusqlite::params![id]).unwrap();
    // A pinned chat: the card goes on its Thread.
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").unwrap();
    Ws { id, handle: handle.to_string(), conv }
}

fn login(user: &str, role: Role) -> String {
    k2_core::connect_users::add_user(user, "password-for-pcgs-int-9").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(user, role).expect("set_role");
    }
    k2_core::connect_users::create_session(user)
}

fn passport(ws: &Ws) -> String {
    k2_daemon::session_token::mint_session_token(
        &SessionId::new(),
        "pane-pcgs",
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.handle.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// What the vault holds for compute git access (system namespace).
fn vault_value(name: &str) -> Option<String> {
    let v = k2_daemon::vault::get().expect("the vault is open");
    match v.resolve(&Selector::system(Scope::engine("compute-git"), name), Use::System, &Requester::System("compute-git".into())) {
        Ok(r) => Some(String::from_utf8(r.value.expose().to_vec()).expect("utf-8")),
        Err(e) if e.status == 404 || e.code == "secret_not_active" || e.code == "secret_destroyed" => None,
        Err(e) => panic!("the vault refused {name}: {}", e.code),
    }
}

/// Fail if `secret` is in any file under `root` (k2.db and its WAL, logs;
/// the vault holds it only encrypted).
fn assert_not_on_disk(root: &Path, secret: &str) {
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
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()), "the credential is on disk in {}", p.display());
        }
    }
    assert!(scanned > 3, "scanned only {scanned} files under {}", root.display());
}

// ── a fake GitHub API (App JWTs checked) ─────────────────────────────

struct FakeGithub {
    base: String,
    minted: Arc<StdMutex<Vec<String>>>,
    seen: Arc<StdMutex<Vec<(String, String)>>>,
}

fn start_github(public: Vec<u8>) -> FakeGithub {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let minted: Arc<StdMutex<Vec<String>>> = Default::default();
    let seen: Arc<StdMutex<Vec<(String, String)>>> = Default::default();
    let (m2, s2) = (minted.clone(), seen.clone());
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
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v| v.trim().parse::<usize>().ok()))
                .unwrap_or(0);
            while body.len() < len {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            let mut first = head.lines().next().unwrap_or("").split(' ');
            let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
            let auth = head
                .lines()
                .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string()))
                .unwrap_or_default();
            s2.lock().unwrap().push((method.clone(), path.clone()));
            let jwt = auth.strip_prefix("Bearer ").and_then(|t| k2_core::compute::git::testing::verify_jwt(t, &public)).is_some_and(|c| c["iss"] == APP_ID);
            let (status, out): (u16, Value) = match (method.as_str(), path.as_str()) {
                ("GET", "/app") if jwt => (200, json!({"slug": "acme-compute"})),
                ("GET", "/repos/acme/app/installation") if jwt => (200, json!({"id": 42})),
                ("POST", "/app/installations/42/access_tokens") if jwt => {
                    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    if b["permissions"] != json!({"contents": "read"}) {
                        (422, json!({"message": "more than contents:read"}))
                    } else {
                        let mut m = m2.lock().unwrap();
                        let t = format!("ghs_k2testSETUPCARD{:04}x{}", m.len(), b["repositories"][0].as_str().unwrap_or(""));
                        m.push(t.clone());
                        let exp = chrono::DateTime::from_timestamp(k2_core::compute::now() + 3600, 0).unwrap().to_rfc3339();
                        (201, json!({"token": t, "expires_at": exp}))
                    }
                }
                ("GET", "/repos/acme/app") if auth == format!("Bearer {PAT}") => (200, json!({"full_name": "acme/app"})),
                _ => (401, json!({"message": "Bad credentials"})),
            };
            let text = out.to_string();
            let _ = c.write_all(
                format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len())
                    .as_bytes(),
            );
        }
    });
    FakeGithub { base, minted, seen }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_setup_card_takes_the_credential_into_the_vault_and_the_grant_mints_after() {
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pcgs{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_COMPUTE", "1");

    let d = test_harness::start(OWNER).await;
    let port = d.port;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let (pem, public) = k2_core::compute::git::testing::app_key_pem();
    // A line of the key's body: never in anything the agent can read.
    let pem_line = pem.lines().find(|l| l.len() > 40 && !l.starts_with("-----")).expect("a PEM body line").to_string();
    let gh = start_github(public);
    k2_daemon::compute_git::test_set_api_base(Some(gh.base.clone()));

    let admin = login("pcgsadmin", Role::Admin);
    let member = login("pcgsmember", Role::Member);
    let ws = workspace(&home, "pcgs-builder");
    let agent = passport(&ws);
    let r = post(port, "/cli/settings/update", OWNER, json!({ "permissionCards": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, json!({ "toggle": "permission_cards", "workspace": ws.id, "value": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Everything the agent is told, to check for the key at the end.
    let mut agent_saw: Vec<String> = Vec::new();

    // ── no credential: every door names the setup card ──────────────────
    let r = post(port, "/cli/compute/git-access/grant", &agent, json!({ "github": "acme/app" })).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.code().as_str()), (403, "git_access_needs_setup"), "{}", r.body);
    let v = r.json();
    assert_eq!(v["permission"]["command"], SETUP_ASK, "{v}");
    assert_eq!(v["permission"]["approvers"], "owner or admin", "{v}");
    assert!(v["error"]["hint"].as_str().unwrap_or("").contains("this server has no GitHub credential yet"), "{v}");
    // Its own setup (a person's): the same card, nothing stored.
    let r = post(port, "/cli/compute/git-access/setup", &agent, json!({ "token": PAT })).await;
    agent_saw.push(r.body.clone());
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], SETUP_ASK, "{}", r.body);
    // The grant card can't be asked yet: 409 with the exact setup line.
    let r = post(port, "/cli/permission/request", &agent, json!({ "action": "compute.git-access", "params": { "repos": "acme/app" }, "why": "clone it" })).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.code().as_str()), (409, "git_access_needs_setup"), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap_or("").ends_with(SETUP_ASK), "{}", r.body);
    let r = call(port, "GET", "/cli/compute/git-access", &agent, None).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.json()["credential"].clone()), (200, Value::Null), "{}", r.body);

    // ── the setup card, as the printed line sends it ────────────────────
    let r = post(port, "/cli/permission/request", &agent, json!({ "action": "compute.git-access-setup", "params": {}, "why": "jobs need to clone acme/app" })).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let card = r.json()["card"].as_str().unwrap_or_else(|| panic!("no card: {}", r.body)).to_string();
    let answer = |who: String, card: String| {
        let body = json!({ "addr": ws.handle, "id": card, "answer": "Allow" });
        async move { post(port, "/cli/thread/answer", &who, body).await }
    };
    // A Member can't allow it; the agent can't either.
    let r = answer(member.clone(), card.clone()).await;
    assert_eq!((r.status, r.code().as_str()), (403, "action_card_person_only"), "{}", r.body);
    let r = answer(agent.clone(), card.clone()).await;
    agent_saw.push(r.body.clone());
    assert_eq!(r.status, 403, "the agent answers its own card: {}", r.body);
    // An Admin's Allow before the form: nothing decided.
    let r = answer(admin.clone(), card.clone()).await;
    assert_eq!((r.status, r.code().as_str()), (409, "git_access_setup_form_first"), "{}", r.body);
    assert_eq!(vault_value("github-app-key"), None, "nothing in the vault yet");

    // ── K2's own form: the setup route, as the Admin ────────────────────
    // A Member can't post it (the route's own floor).
    let r = post(port, "/cli/compute/git-access/setup", &member, json!({ "app_id": APP_ID, "key_pem": pem })).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(port, "/cli/compute/git-access/setup", &admin, json!({ "app_id": APP_ID, "key_pem": pem })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["credential"], json!({ "kind": "app", "appId": APP_ID, "app": "acme-compute" }), "{}", r.body);
    assert!(!r.body.contains(&pem_line), "the answer never echoes the key");
    // (The vault keeps the value trimmed; compared without printing it.)
    let kept = vault_value("github-app-key").expect("the key is in this server's vault");
    assert!(kept.trim() == pem.trim(), "the vault holds exactly the key the form sent");
    // Now Allow closes the card.
    let r = answer(admin.clone(), card.clone()).await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("already_done")), "{}", r.body);
    let r = call(port, "GET", "/cli/compute/git-access", &agent, None).await;
    agent_saw.push(r.body.clone());
    assert_eq!(r.json()["credential"]["kind"], "app", "{}", r.body);
    assert_eq!(r.json()["credential"]["appId"], APP_ID, "{}", r.body);

    // ── the grant, on its own card, mints at GitHub ─────────────────────
    let r = post(port, "/cli/compute/git-access/grant", &agent, json!({ "github": "acme/app" })).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.code().as_str()), (403, "git_access_needs_person"), "{}", r.body);
    assert_eq!(r.json()["permission"]["command"], GRANT_ASK, "{}", r.body);
    let r = post(port, "/cli/permission/request", &agent, json!({ "action": "compute.git-access", "params": { "repos": "acme/app" }, "why": "clone it" })).await;
    agent_saw.push(r.body.clone());
    assert_eq!((r.status, r.json()["status"].as_str()), (200, Some("pending")), "{}", r.body);
    let grant_card = r.json()["card"].as_str().expect("card").to_string();
    assert_ne!(grant_card, card, "the grant is its own card");
    let r = answer(admin.clone(), grant_card).await;
    assert_eq!((r.status, r.json()["action"]["result"]["outcome"].as_str()), (200, Some("approved")), "{}", r.body);
    let grants = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::git::grants_of(&conn, &ws.id)
    };
    assert_eq!(grants.iter().map(|g| (g.repo.as_str(), g.nodes.as_str())).collect::<Vec<_>>(), vec![("github.com/acme/app", "*")], "{grants:?}");
    let minted = gh.minted.lock().unwrap().clone();
    assert!(minted.len() == 1 && minted[0].ends_with("xapp"), "one token minted for acme/app with the stored key: {minted:?}");
    assert!(gh.seen.lock().unwrap().iter().any(|(m, p)| m == "POST" && p == "/app/installations/42/access_tokens"), "GitHub minted it");

    // ── the agent never had the key ─────────────────────────────────────
    let page = k2_core::overlay::read_thread_page(&ws.conv, None, None, 500).expect("the Thread");
    let thread = serde_json::to_string(&page.items.iter().map(|i| &i.doc).collect::<Vec<_>>()).expect("thread json");
    assert!(thread.contains("GitHub access for compute jobs"), "the setup card is on the agent's Thread");
    for (i, seen) in agent_saw.iter().chain(std::iter::once(&thread)).enumerate() {
        assert!(!seen.contains(&pem_line) && !seen.contains(PAT), "the agent saw the credential ({i}): {seen}");
    }
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = conn.prepare("SELECT params_json || why || COALESCE(result_message, '') FROM permission_requests WHERE workspace_id = ?1").unwrap();
        st.query_map([&ws.id], |r| r.get::<_, String>(0)).unwrap().map(|x| x.unwrap()).collect::<Vec<_>>()
    };
    assert_eq!(rows.len(), 2, "two requests: setup and grant: {rows:?}");
    assert!(rows.iter().all(|r| !r.contains(&pem_line)), "{rows:?}");
    assert_not_on_disk(&home, &pem_line);

    // ── the form's other choice: a fine-grained token replaces the key ──
    let r = post(port, "/cli/compute/git-access/setup", &admin, json!({ "token": PAT })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["credential"]["kind"], "token", "{}", r.body);
    assert_eq!(vault_value("github-token").as_deref(), Some(PAT), "the token is in the vault");
    assert_eq!(vault_value("github-app-key"), None, "the App key is gone");
    assert_not_on_disk(&home, PAT);
    drop(d);
}
