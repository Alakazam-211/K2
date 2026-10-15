//! Agent hire through the v1 API, end to end through the REAL dispatcher
//! (`k2_daemon::test_harness`): owner sets the hire root → mints a hire key
//! (owner confirm required even with the owner token) → the key hires a job
//! lead and a linked Desk team with an `Idempotency-Key` → replay → list →
//! links both ways → teardown (real archive-retire) → audit lines.
//!
//! ISOLATION: `$HOME` points at a fresh temp dir (DB is the in-memory test
//! DB; the vault opens on a file key under the temp home; retire archives
//! into the temp home; rolled-back folders go to a temp trash dir). No real
//! AI CLI runs: no member launches.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-v1-agent-hire-0456";

struct Resp {
    status: u16,
    body: Value,
}

fn http(port: u16, method: &str, path_and_query: &str, bearer: Option<&str>, idem: Option<&str>, body: Option<&Value>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut head = format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    if let Some(k) = idem {
        head.push_str(&format!("Idempotency-Key: {k}\r\n"));
    }
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    if body.is_some() {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body_text = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    let body = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
    Resp { status, body }
}

fn owner_q(path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}token={OWNER_TOKEN}")
}

struct Env {
    prev_home: Option<std::ffi::OsString>,
    prev_api: Option<std::ffi::OsString>,
    tmp: std::path::PathBuf,
}

impl Env {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let prev_api = std::env::var_os("K2_API");
        let tmp = std::env::temp_dir().join(format!(
            "k2-v1-agent-hire-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("home")).expect("mkdir home");
        std::env::set_var("HOME", tmp.join("home"));
        std::env::set_var("K2_API", "1");
        k2_core::hire::apply::set_trash_override(Some(tmp.join("trash")));
        Self { prev_home, prev_api, tmp }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.prev_api {
            Some(v) => std::env::set_var("K2_API", v),
            None => std::env::remove_var("K2_API"),
        }
        k2_core::hire::apply::set_trash_override(None);
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hire_key_mints_hires_links_replays_and_retires_end_to_end() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;

    // The owner sets the hire root.
    let root = env.tmp.join("agents");
    let r = http(port, "POST", &owner_q("/cli/agents/hire-root"), None, None, Some(&json!({ "root": root })));
    assert_eq!(r.status, 200, "{}", r.body);
    let root = std::path::PathBuf::from(r.body["root"].as_str().unwrap());
    let r = http(port, "GET", &owner_q("/cli/agents/hire-root"), None, None, None);
    assert_eq!(r.body["default"], false, "{}", r.body);

    // Minting a hire key with only the owner token fails (HT5/T18).
    let mint = json!({ "label": "jobs-app", "hire": { "link": true, "retire": true, "folder": "acme", "maxMembersPerTeam": 4 } });
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&mint));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "vault_confirm_required");
    let confirm = k2_daemon::vault::get().expect("vault open").confirm.mint("hire-key-mint", None, "owner", "local-auth");
    let mut with_confirm = mint.clone();
    with_confirm["confirmToken"] = json!(confirm);
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&with_confirm));
    assert_eq!(r.status, 200, "{}", r.body);
    let key = r.body["key"].as_str().expect("raw key").to_string();
    assert_eq!(r.body["capabilities"]["hire"], true);
    assert_eq!(r.body["capabilities"]["hostSessions"], false);

    // The key hires a job lead.
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-lead"), Some(&json!({
        "agent": { "role": "joblead", "handle": "j1-lead", "name": "Job J1 Lead", "persona": "You run job J1." },
        "onBehalfOf": "alex@acme (job J1)",
    })));
    assert_eq!(r.status, 200, "{}", r.body);
    let lead = r.body["members"][0].clone();
    assert_eq!(lead["handle"], "j1-lead");
    assert!(lead["path"].as_str().unwrap().starts_with(root.join("acme").to_str().unwrap()), "{lead}");

    // …then a Desk team linked to it, with an Idempotency-Key.
    let desk = json!({
        "team": { "name": "Plans Desk J1", "topology": "star" },
        "members": [
            { "role": "librarian", "lead": true, "persona": "Index plan set ps-4471." },
            { "role": "structural" },
        ],
        "linkTo": [{ "workspace": lead["workspaceId"], "kind": "serves", "scope": { "planSets": ["ps-4471"] } }],
        "onBehalfOf": "alex@acme (job J1)",
    });
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-desk"), Some(&desk));
    assert_eq!(r.status, 200, "{}", r.body);
    let team = r.body.clone();
    assert_eq!(team["members"][0]["handle"], "plans-desk-j1-librarian");
    assert_eq!(team["links"][0]["kind"], "serves");
    let inst = team["instance"].as_str().unwrap().to_string();

    // Replay: same key + same body → the same instance, nothing new.
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("job-J1-desk"), Some(&desk));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["changed"], false);
    assert_eq!(r.body["instance"], inst.as_str());

    // Method guards.
    assert_eq!(http(port, "GET", "/v1/agents/hire", Some(&key), None, None).status, 405);
    assert_eq!(http(port, "POST", "/v1/agents/instances", Some(&key), None, Some(&json!({}))).status, 405);

    // List own instances; links both ways.
    let r = http(port, "GET", "/v1/agents/instances", Some(&key), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["instances"].as_array().unwrap().len(), 2, "{}", r.body);
    let r = http(port, "GET", &format!("/v1/agent-teams/links?workspace={}", lead["workspaceId"].as_str().unwrap()), Some(&key), None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["servedBy"][0]["teamName"], "Plans Desk J1", "{}", r.body);
    assert_eq!(r.body["servedBy"][0]["scope"]["planSets"][0], "ps-4471");

    // A key without the hire scope sees nothing.
    let (_, plain) = k2_core::api_keys::create_api_key("plain", None, Some("*"), None, None, None).expect("plain key");
    assert_eq!(http(port, "POST", "/v1/agents/hire", Some(&plain), None, Some(&json!({ "agent": {} }))).status, 404);
    assert_eq!(http(port, "GET", "/v1/agents/instances", Some(&plain), None, None).status, 404);

    // Teardown: unlink, delete the group (past the PoC guard), archive-retire.
    let r = http(port, "POST", "/v1/agents/retire", Some(&key), None, Some(&json!({ "instance": inst })));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["status"], "retired", "{}", r.body);
    assert_eq!(r.body["unlinked"].as_array().unwrap().len(), 1);
    for m in team["members"].as_array().unwrap() {
        let p = m["path"].as_str().unwrap();
        assert!(!std::path::Path::new(p).exists(), "archived (moved): {p}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE path = ?1", rusqlite::params![p], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "deregistered: {p}");
    }
    assert!(k2_core::paths::k2_home().join("archive").is_dir(), "retire archives under the (temp) home");
    let r = http(port, "GET", &format!("/v1/agents/instances?instance={inst}"), Some(&key), None, None);
    assert_eq!(r.body["instance"]["status"], "retired", "{}", r.body);
    let actions: Vec<&str> = r.body["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    for want in ["agent-hire", "agent-link", "agent-unlink", "agent-retire"] {
        assert!(actions.contains(&want), "{want} ledgered: {actions:?}");
    }

    // One auth-audit line per mint, hire, link, retire (HT19).
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit file");
    for want in ["hire-key-mint", "agent-hire", "agent-link", "agent-retire"] {
        assert!(audit.contains(&format!("\"event\":\"{want}\"")), "{want} audited:\n{audit}");
    }
    drop(env);
}

/// S5: a hired agent gets an App login — an App grant on the named App
/// (never the box login), whole rooms, marked as the agent — only within
/// the key's ceiling; a rollback and a teardown remove the login.
#[tokio::test(flavor = "multi_thread")]
async fn hire_key_gives_app_logins_inside_its_ceiling_only() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let r = http(port, "POST", &owner_q("/cli/agents/hire-root"), None, None, Some(&json!({ "root": env.tmp.join("agents") })));
    assert_eq!(r.status, 200, "{}", r.body);

    // An owner-made workspace that publishes the App (and another App).
    let host_ws = env.tmp.join("portal");
    let r = http(port, "POST", "/v1/w", Some(OWNER_TOKEN), None, Some(&json!({ "path": host_ws })));
    assert_eq!(r.status, 200, "{}", r.body);
    let host_id = r.body["id"].as_str().unwrap().to_string();
    let (app, other_app) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mk = |name: &str| {
            k2_core::published_services::insert(
                &conn,
                &host_id,
                name,
                k2_core::published_services::CMD_SKIN_SENTINEL,
                "/tmp",
                1,
                k2_core::published_services::EXPOSE_LOCAL,
                k2_core::published_services::DESIRED_STOPPED,
                k2_core::published_services::KIND_SKIN,
                "",
            )
            .expect("service row")
            .id
        };
        (mk("jobs"), mk("billing"))
    };

    // Mint a hire key whose ceiling names the jobs App, rooms: self only.
    let confirm = k2_daemon::vault::get().expect("vault").confirm.mint("hire-key-mint", None, "owner", "local-auth");
    let mint = json!({ "label": "jobs-app", "confirmToken": confirm, "hire": { "retire": true, "apps": [{ "app": app, "rooms": ["self"] }] } });
    let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&mint));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["hireLimits"]["apps"][0]["app"], app.as_str(), "{}", r.body);
    let key = r.body["key"].as_str().unwrap().to_string();

    // Outside the ceiling: another App, or a room the owner didn't name → 403, nothing written.
    for (login, label) in [
        (json!({ "app": other_app }), "other app"),
        (json!({ "app": app, "rooms": [host_id] }), "unnamed room"),
        (json!({ "app": app, "role": "admin" }), "unnamed role"),
    ] {
        let r = http(port, "POST", "/v1/agents/hire", Some(&key), None, Some(&json!({ "agent": { "handle": "fieldbot", "appLogin": login } })));
        assert_eq!(r.status, 403, "{label}: {}", r.body);
        assert_eq!(r.body["error"]["code"], "exceeds_key", "{label}");
        assert!(!k2_core::skin::principal_exists("fieldbot").unwrap(), "{label}: no login written");
    }

    // Inside: the login is an App grant on `app`, rooms = the agent itself.
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), Some("field-1"), Some(&json!({
        "agent": { "handle": "fieldbot", "appLogin": { "app": app, "password": true } },
    })));
    assert_eq!(r.status, 200, "{}", r.body);
    let m = r.body["members"][0].clone();
    let ws_id = m["workspaceId"].as_str().unwrap().to_string();
    let login = m["appLogin"].clone();
    assert_eq!(login["username"], "fieldbot");
    assert_eq!(login["rooms"], json!([ws_id]));
    assert_eq!(login["wholeRooms"], true);
    assert_eq!(login["password"].as_str().map(str::len), Some(24), "password shown once: {login}");
    let principal = login["principalId"].as_str().unwrap().to_string();
    let grants: Vec<_> = k2_core::skin::list_grants().unwrap().into_iter().filter(|g| g.subject_id == principal).collect();
    assert_eq!(grants.len(), 1, "exactly one grant: {grants:?}");
    assert_eq!(grants[0].kind, "app");
    assert_eq!(grants[0].target_id, app, "an App grant, never the box login (host)");
    assert_eq!(k2_core::skin::principal_agent_workspace(&principal).unwrap().as_deref(), Some(ws_id.as_str()));
    // A replay never prints the password again.
    let r2 = http(port, "POST", "/v1/agents/hire", Some(&key), Some("field-1"), Some(&json!({
        "agent": { "handle": "fieldbot", "appLogin": { "app": app, "password": true } },
    })));
    assert_eq!(r2.body["changed"], false);
    assert!(r2.body["members"][0]["appLogin"].get("password").is_none(), "{}", r2.body);

    // A hire that fails after a member's login was made undoes the login
    // (the second member's unknown context layer fails at apply time).
    let r = http(port, "POST", "/v1/agents/hire", Some(&key), None, Some(&json!({
        "team": { "name": "Roll Team" },
        "members": [
            { "handle": "rollbot", "lead": true, "appLogin": { "app": app } },
            { "handle": "rollbot-two", "context": ["nope:missing-layer"] },
        ],
    })));
    assert!(r.status >= 400, "the hire fails: {}", r.body);
    assert!(r.body["error"]["hint"].as_str().unwrap_or("").contains("nothing was kept"), "{}", r.body);
    assert!(!k2_core::skin::principal_exists("rollbot").unwrap(), "rolled-back login removed");

    // Teardown revokes the login (retire alone wouldn't, HT13).
    let r = http(port, "POST", "/v1/agents/retire", Some(&key), None, Some(&json!({ "instance": r2.body["instance"] })));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["appLoginsRevoked"], json!(["fieldbot"]));
    assert!(!k2_core::skin::principal_exists("fieldbot").unwrap(), "login revoked");
    drop(env);
}

// ── App access limit changes (0.45.6, Settings → API Keys) ───────────────

fn policy(rows: &[(&str, &[&str])]) -> k2_core::skin::RoomPolicy {
    rows.iter().map(|(r, c)| (r.to_string(), c.iter().map(|x| x.to_string()).collect())).collect()
}

fn handle_of(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT handle FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get(0)).expect("handle")
}

fn key_apps(port: u16, id: &str) -> Value {
    let r = http(port, "GET", &owner_q("/cli/api-keys/list"), None, None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    let row = r.body["keys"].as_array().unwrap().iter().find(|k| k["id"] == id).cloned().expect("key row");
    row["hireLimits"]["apps"].clone()
}

fn agent_passport(workspace_id: &str) -> String {
    use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
    let sid = k2_core::session::SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: workspace_id.to_string(), agent_address: "ceiling-agent".to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// The owner changes a hire key's App access limit: the daemon validates
/// it (a real App, real rooms, a role usable on that App), lowers it with
/// no confirm, raises it only with the owner confirm, and refuses every
/// non-owner caller (agent passports, API keys). Hires follow the new
/// limit at once, with "highest role" meaning that role or one no wider.
/// The CLI and the Settings body store the same limit.
#[tokio::test(flavor = "multi_thread")]
async fn owner_changes_a_hire_keys_app_access_limit() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let port = d.port;
    let r = http(port, "POST", &owner_q("/cli/agents/hire-root"), None, None, Some(&json!({ "root": env.tmp.join("agents") })));
    assert_eq!(r.status, 200, "{}", r.body);

    // Rooms: the portal (publishes `jobs` and `billing`), job A, job B.
    let ws = |name: &str| {
        let r = http(port, "POST", "/v1/w", Some(OWNER_TOKEN), None, Some(&json!({ "path": env.tmp.join(name) })));
        assert_eq!(r.status, 200, "{name}: {}", r.body);
        r.body["id"].as_str().unwrap().to_string()
    };
    let (portal, job_a, job_b) = (ws("portal"), ws("job-a"), ws("job-b"));
    let (jobs, billing) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mk = |name: &str| {
            k2_core::published_services::insert(
                &conn,
                &portal,
                name,
                k2_core::published_services::CMD_SKIN_SENTINEL,
                "/tmp",
                1,
                k2_core::published_services::EXPOSE_LOCAL,
                k2_core::published_services::DESIRED_STOPPED,
                k2_core::published_services::KIND_SKIN,
                "",
            )
            .expect("service row")
            .id
        };
        (mk("jobs"), mk("billing"))
    };
    let role = |name: &str, app: &str, rows: &[(&str, &[&str])]| {
        k2_core::skin::create_role_for_app(name, &policy(rows), Some(app)).expect("role").id
    };
    let crew = role("crew", &jobs, &[(&job_a, &["thread:read", "thread:post"])]);
    let lookers = role("lookers", &jobs, &[(&job_a, &["thread:read"])]);
    let foremen = role("foremen", &jobs, &[(&job_b, &["thread:read", "thread:post", "files:read"])]);
    let clerks = role("clerks", &billing, &[(&job_a, &["thread:read"])]);

    // Options for the Settings pickers.
    let r = http(port, "GET", &owner_q("/cli/api-keys/hire-ceiling-options"), None, None, None);
    assert_eq!(r.status, 200, "{}", r.body);
    let jobs_opt = r.body["apps"].as_array().unwrap().iter().find(|a| a["id"] == jobs.as_str()).cloned().expect("jobs listed");
    let role_ids: Vec<&str> = jobs_opt["roles"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert!(role_ids.contains(&crew.as_str()) && role_ids.contains(&foremen.as_str()), "{jobs_opt}");
    assert!(!role_ids.contains(&clerks.as_str()), "another App's role is not offered: {jobs_opt}");
    let room_ids: Vec<&str> = r.body["rooms"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(room_ids[0], "self", "{}", r.body);
    for id in [&portal, &job_a, &job_b] {
        assert!(room_ids.contains(&id.as_str()), "{id} offered: {room_ids:?}");
    }
    assert!(!room_ids.iter().any(|id| id.starts_with('_')), "no audit sentinels: {room_ids:?}");

    // Mint: jobs, rooms self + job A, highest role crew.
    let mint = |label: &str, apps: Value| {
        let confirm = k2_daemon::vault::get().expect("vault").confirm.mint("hire-key-mint", None, "owner", "local-auth");
        let body = json!({ "label": label, "confirmToken": confirm, "hire": { "apps": apps } });
        let r = http(port, "POST", &owner_q("/cli/api-keys/create"), None, None, Some(&body));
        assert_eq!(r.status, 200, "{}", r.body);
        (r.body["id"].as_str().unwrap().to_string(), r.body["key"].as_str().unwrap().to_string())
    };
    let (kid, key) = mint("jobs-limit", json!([{ "app": jobs, "rooms": ["self", job_a], "roles": ["crew"] }]));
    let start = json!([{ "app": jobs, "rooms": ["self", job_a], "roles": [crew] }]);
    assert_eq!(key_apps(port, &kid), start, "roles are stored by id");

    // Validation: every bad limit is refused and nothing changes.
    let set = |body: Value| http(port, "POST", &owner_q("/cli/api-keys/hire-ceiling"), None, None, Some(&body));
    for (apps, label) in [
        (json!([{ "app": "no-such-app" }]), "unknown App"),
        (json!([{ "app": "host" }]), "the box login is not an App"),
        (json!([{ "app": jobs, "rooms": ["no-such-room"] }]), "unknown room"),
        (json!([{ "app": jobs, "rooms": [] }]), "no rooms"),
        (json!([{ "app": jobs, "rooms": ["self"], "roles": ["no-such-role"] }]), "unknown role"),
        (json!([{ "app": jobs, "rooms": ["self"], "roles": [clerks] }]), "another App's role"),
        (json!([{ "app": jobs, "folders": ["plans/"] }]), "unknown field"),
        (json!({ "app": jobs }), "not a list"),
    ] {
        let r = set(json!({ "id": kid, "apps": apps }));
        assert_eq!(r.status, 400, "{label}: {}", r.body);
        assert_eq!(key_apps(port, &kid), start, "{label}: unchanged");
    }
    assert_eq!(set(json!({ "id": kid })).status, 400, "apps is required ([] = none)");
    assert_eq!(set(json!({ "id": "no-such-key", "apps": [] })).status, 404);
    assert_eq!(http(port, "GET", &owner_q("/cli/api-keys/hire-ceiling"), None, None, None).status, 405);

    // Agents never set or change a limit — not even a lowering — nor do
    // API keys or callers with no token.
    let passport = agent_passport(&portal);
    let lower_to_none = json!({ "id": kid, "apps": [] });
    for (who, label) in [
        (format!("/cli/api-keys/hire-ceiling?token={passport}"), "agent passport"),
        (format!("/cli/api-keys/hire-ceiling?token={key}"), "the API key itself"),
        ("/cli/api-keys/hire-ceiling".to_string(), "no token"),
    ] {
        let r = http(port, "POST", &who, None, None, Some(&lower_to_none));
        assert_eq!(r.status, 403, "{label}: {}", r.body);
        let r = http(port, "GET", &who.replace("hire-ceiling", "hire-ceiling-options"), None, None, None);
        assert_eq!(r.status, 403, "{label} options: {}", r.body);
    }
    let r = http(port, "POST", "/cli/api-keys/hire-ceiling", Some(&passport), None, Some(&lower_to_none));
    assert_eq!(r.status, 403, "agent passport as bearer: {}", r.body);
    assert_eq!(key_apps(port, &kid), start, "no agent changed the limit");

    // Hires follow the limit: a role no wider than crew fits; one reaching
    // job B does not.
    let hire = |handle: &str, login: Value| {
        http(port, "POST", "/v1/agents/hire", Some(&key), None, Some(&json!({ "agent": { "handle": handle, "appLogin": login } })))
    };
    let r = hire("looker-1", json!({ "app": jobs, "role": "lookers" }));
    assert_eq!(r.status, 200, "lookers ⊂ crew: {}", r.body);
    let r = hire("foreman-1", json!({ "app": jobs, "role": "foremen" }));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "exceeds_key");

    // A raise (job B + foremen) without the confirm is refused; with it, saved.
    let wide = json!([{ "app": jobs, "rooms": ["self", job_a, job_b], "roles": [foremen] }]);
    let r = set(json!({ "id": kid, "apps": wide }));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "vault_confirm_required");
    assert_eq!(key_apps(port, &kid), start, "unchanged without the confirm");
    let confirm = k2_daemon::vault::get().expect("vault").confirm.mint("hire-key-mint", None, "owner", "local-auth");
    let r = set(json!({ "id": kid, "apps": wide, "confirmToken": confirm }));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["change"], "raise");
    assert_eq!(r.body["confirmed"], true);
    assert_eq!(key_apps(port, &kid), wide);
    let r = set(json!({ "id": kid, "apps": wide, "confirmToken": confirm }));
    assert_eq!(r.status, 200, "an unchanged limit is not a raise: {}", r.body);
    assert_eq!(r.body["change"], "lower");
    let r = hire("foreman-2", json!({ "app": jobs, "role": "foremen" }));
    assert_eq!(r.status, 200, "the raised limit applies at once: {}", r.body);

    // A lowering (fewer rooms, a narrower role) needs no confirm.
    let narrow = json!([{ "app": jobs, "rooms": [job_b], "roles": [foremen] }]);
    let r = set(json!({ "id": kid, "apps": narrow }));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["change"], "lower");
    assert_eq!(r.body["confirmed"], false);
    // …but a role swap that widens (crew is not inside foremen) is a raise.
    let r = set(json!({ "id": kid, "apps": [{ "app": jobs, "rooms": [job_b], "roles": [crew] }] }));
    assert_eq!(r.status, 403, "{}", r.body);
    // No App access: hired agents get no App login.
    let r = set(json!({ "id": kid, "apps": [] }));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(key_apps(port, &kid), json!([]));
    let r = hire("fieldbot-2", json!({ "app": jobs }));
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "exceeds_key");

    // CLI parity: the same lowering through `k2 api-key hire-ceiling`
    // (names and handles) and through the Settings body (ids) stores the
    // same limit.
    let full = json!([{ "app": jobs, "rooms": ["self", job_a, job_b], "roles": [crew] }]);
    let (cli_key, _) = mint("cli-side", full.clone());
    let (ui_key, _) = mint("settings-side", full);
    let cli_home = env.tmp.join("cli-home");
    std::fs::create_dir_all(cli_home.join(".k2")).expect("cli home");
    std::fs::write(cli_home.join(".k2/heartbeat.token"), OWNER_TOKEN).expect("token file");
    let out = std::process::Command::new("bash")
        .arg(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2"))
        .args(["api-key", "hire-ceiling", &cli_key, "--app-login"])
        .arg(format!("{}/jobs:self,{}:lookers", handle_of(&portal), handle_of(&job_a)))
        .env_clear()
        .env("HOME", &cli_home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("K2_PORT", port.to_string())
        .current_dir(&cli_home)
        .output()
        .expect("run cli/k2");
    assert!(
        out.status.success(),
        "k2 api-key hire-ceiling exited {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let r = set(json!({ "id": ui_key, "apps": [{ "app": jobs, "rooms": ["self", job_a], "roles": [lookers] }] }));
    assert_eq!(r.status, 200, "{}", r.body);
    let expect = json!([{ "app": jobs, "rooms": ["self", job_a], "roles": [lookers] }]);
    assert_eq!(key_apps(port, &cli_key), expect, "CLI stored");
    assert_eq!(key_apps(port, &ui_key), expect, "Settings stored");

    // Ledger + one auth-audit line per change.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit file");
    assert!(audit.contains("\"event\":\"hire-key-ceiling\""), "audited:\n{audit}");
    let n: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM hire_events WHERE action = 'hire-key-ceiling' AND result = 'ok'", [], |r| r.get(0)).unwrap()
    };
    assert!(n >= 5, "ledgered changes: {n}");
    drop(env);
}
