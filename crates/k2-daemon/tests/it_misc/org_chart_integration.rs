//! The Org Chart over real HTTP (prd-org-chart-v1 §8 tests 16, 18–23):
//! the real dispatcher in-process (temp `$HOME`, loopback listener), the
//! owner token, Connect logins and agent passports.
//!
//! - Off: every route answers 409 `org_chart_off`, for people and agents.
//! - The switch is an Admin-floor setting (a Member login is refused).
//! - Floors: a Member login can't change the chart; a Voice & Chat login
//!   can't even read it; a GET on a POST route is 405.
//! - "Manage the org chart": off by default; an agent without it gets
//!   `org_manage_off`; only the Owner turns it on.
//! - The head writes its own bulletin (5 lines at most); the feed field
//!   rides an agent's answers until it reads today's bulletin, never a
//!   person's.
//! - Reach: a ticket when permission cards are off, a `connections.add`
//!   card when they are on; a connection added by the owner closes the ask,
//!   resolves the ticket and tells the agent once; a dismissed ticket is
//!   "declined", told once.
//!
//! No vault, keychain or `~/.k2` is touched (temp `$HOME`).

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-org-chart-v1";

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
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2oc-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
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
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
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
    let v = j(r);
    v["error"]["code"].as_str().or_else(|| v["error"].as_str()).unwrap_or_else(|| panic!("no error code: {}", r.body)).to_string()
}

struct Ws {
    id: String,
    path: PathBuf,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()])
        .unwrap();
    Ws { id, path }
}

fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: ws.id.clone() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn login(name: &str, role: k2_core::connect_users::Role) -> String {
    k2_core::connect_users::add_user(name, "correct-horse-battery-staple-9").expect("add user");
    k2_core::connect_users::set_role(name, role).expect("role");
    k2_core::connect_users::create_session(name)
}

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<(String, String)>>,
}

impl k2_daemon::org_chart::Waker for Fake {
    fn wake(&self, project_id: &str, _from: &str, text: &str) -> Result<(), String> {
        self.calls.borrow_mut().push((project_id.into(), text.into()));
        Ok(())
    }
}

fn audit_events(prefix: &str) -> Vec<(String, String)> {
    k2_core::auth_audit::tail(500)
        .expect("read the access audit")
        .into_iter()
        .filter(|v| v["event"].as_str().is_some_and(|e| e.starts_with(prefix)))
        .map(|v| (v["user"].as_str().unwrap_or("").to_string(), v["outcome"].as_str().unwrap_or("").to_string()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn org_chart_floors_switches_bulletin_feed_and_reach_end_to_end() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let billing = seed_ws(&env, "billing");
    let helpdesk = seed_ws(&env, "helpdesk");
    let newbie = seed_ws(&env, "newbie");
    let (pb, pn) = (passport(&billing), passport(&newbie));

    // Off: every route says so, to people and agents alike.
    for (tok, who) in [(OWNER, "owner"), (pn.as_str(), "agent")] {
        let r = get(port, "/cli/organization/list", tok, "").await;
        assert_eq!((r.status, code(&r)), (409, "org_chart_off".to_string()), "{who}: {}", r.body);
        let r = post(port, "/cli/organization/create", tok, json!({"name": "X"})).await;
        assert_eq!((r.status, code(&r)), (409, "org_chart_off".to_string()), "{who}: {}", r.body);
    }
    let r = get(port, "/cli/organization/list", &pn, "").await;
    assert!(!r.body.contains("\"bulletin\":{"), "no feed field while off: {}", r.body);

    // The switch is an Admin-floor setting.
    let member = login("oc-member", k2_core::connect_users::Role::Member);
    let admin = login("oc-admin", k2_core::connect_users::Role::Admin);
    let r = post(port, "/cli/settings/update", &member, json!({"orgChart": true})).await;
    assert_eq!(r.status, 403, "a Member can't turn it on: {}", r.body);
    assert!(!k2_core::app_settings::load().org_chart);
    let r = post(port, "/cli/settings/update", &admin, json!({"orgChartNight": {"runAt": "08:00"}})).await;
    assert_eq!(r.status, 400, "outside 22:00–07:00: {}", r.body);
    let r = post(port, "/cli/settings/update", &admin, json!({"orgChart": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::org_chart::enabled());
    // The settings route audits by role (`admin`).
    assert!(audit_events("org_chart.switch").iter().any(|(u, o)| u == "admin" && o == "on"), "{:?}", audit_events("org_chart.switch"));

    // People: the owner builds it; a GET on a POST route is 405; a Member
    // login can look but not change; Voice & Chat can't even look.
    let r = post(port, "/cli/organization/create", OWNER, json!({"name": "Billing", "purpose": "Invoices, refunds and Stripe."})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for (org, agent) in [("Billing", "billing"), ("Billing", "helpdesk")] {
        let r = post(port, "/cli/organization/member/add", OWNER, json!({"org": org, "agent": agent})).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }
    let r = post(port, "/cli/organization/head", OWNER, json!({"org": "Billing", "agent": "billing"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = get(port, "/cli/organization/create", OWNER, "name=Nope").await;
    assert_eq!(r.status, 405, "{}", r.body);
    let r = post(port, "/cli/organization/create", &member, json!({"name": "Nope"})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = get(port, "/cli/organization/list", &member, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let vc = login("oc-voice", k2_core::connect_users::Role::VoiceChat);
    let r = get(port, "/cli/organization/list", &vc, "").await;
    assert_eq!(r.status, 403, "{}", r.body);

    // An agent without "Manage the org chart".
    let r = post(port, "/cli/organization/create", &pn, json!({"name": "Research"})).await;
    assert_eq!((r.status, code(&r)), (403, "org_manage_off".to_string()), "{}", r.body);
    // Only the Owner turns it on.
    let set = |v: bool| json!({"toggle": "org_chart_manage", "workspace": newbie.id, "value": v});
    let r = post(port, "/cli/agent-access/set", &admin, set(true)).await;
    assert_eq!((r.status, code(&r)), (403, "owner_only".to_string()), "{}", r.body);
    let r = post(port, "/cli/agent-access/set", &pn, set(true)).await;
    assert_eq!(r.status, 403, "an agent never flips its own: {}", r.body);
    let r = post(port, "/cli/agent-access/set", OWNER, set(true)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/organization/create", &pn, json!({"name": "Research"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/organization/layout/tidy", &pn, json!({})).await;
    assert_eq!((r.status, code(&r)), (403, "people_only".to_string()), "even with the switch: {}", r.body);
    let r = post(port, "/cli/agent-access/set", &admin, set(false)).await;
    assert_eq!(r.status, 200, "an Admin may turn it off: {}", r.body);

    // The head writes its own bulletin; at most 5 lines.
    let six: Vec<String> = (1..=6).map(|i| format!("line {i}")).collect();
    let r = post(port, "/cli/organization/bulletin/set", &pb, json!({"lines": six})).await;
    assert_eq!((r.status, code(&r)), (400, "bulletin_too_long".to_string()));
    let r = post(port, "/cli/organization/bulletin/set", &pb, json!({"lines": ["Refunds go here.", "This week: the new flow."]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/organization/bulletin/set", &passport(&helpdesk), json!({"org": "Billing", "lines": ["x"]})).await;
    assert_eq!((r.status, code(&r)), (403, "not_org_head".to_string()));
    // An Owner/Admin may write any org's, audited.
    let r = post(port, "/cli/organization/bulletin/set", &admin, json!({"org": "Billing", "lines": ["Refunds go here.", "Fixed by an Admin."]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(audit_events("org_chart.bulletin").iter().any(|(u, _)| u.contains("oc-admin")));

    // The feed field: an agent's answers carry it until it reads today's.
    let r = get(port, "/cli/organization/list", &pn, "").await;
    let v = j(&r);
    assert_eq!(v["bulletin"]["unread"], true, "{}", r.body);
    // The feed field points at the bulletin; it never carries its text.
    let mut keys: Vec<&str> = v["bulletin"].as_object().expect("the feed field").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["day", "key", "unread"], "{}", r.body);
    let r = get(port, "/cli/organization/list", OWNER, "").await;
    assert!(j(&r).get("bulletin").is_none(), "never a person's: {}", r.body);
    let r = get(port, "/cli/organization/bulletin", &pn, "peek=1").await;
    assert!(j(&r)["text"].as_str().unwrap().contains("Fixed by an Admin."), "live: {}", r.body);
    let r = get(port, "/cli/organization/list", &pn, "").await;
    assert_eq!(j(&r)["bulletin"]["unread"], true, "peek didn't mark");
    let r = get(port, "/cli/organization/bulletin", &pn, "").await;
    assert_eq!(j(&r)["markedRead"], true);
    let r = get(port, "/cli/organization/list", &pn, "").await;
    assert!(j(&r).get("bulletin").is_none(), "read today: quiet: {}", r.body);

    // Reach, permission cards off: a ticket for the Owner and Admins.
    let r = post(port, "/cli/organization/reach", &pn, json!({"org": "Billing", "why": "a user asks about a refund"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["status"], "ticket", "{v}");
    let ticket = v["ticket"].as_str().unwrap().to_string();
    let item = k2_core::feedback::get_item(&ticket).expect("the ticket");
    assert!(item.title.contains("Connect newbie to billing (head of Billing)"), "{}", item.title);
    assert!(item.body.as_deref().unwrap_or("").contains("k2 connections add billing --workspace newbie"));
    assert!(item.assignees.iter().any(|a| a == "oc-admin"), "assigned to the Admins: {:?}", item.assignees);
    let r = post(port, "/cli/organization/reach", &pn, json!({"org": "billing", "why": "again"})).await;
    assert_eq!(j(&r)["status"], "open", "one open ask per pair: {}", r.body);
    // Nothing to tell yet.
    let fake = Fake::default();
    let now = chrono::Utc::now().timestamp();
    assert_eq!(k2_daemon::org_chart::reach::tell_pass(&fake, now), Default::default());
    assert!(fake.calls.borrow().is_empty());
    // The owner connects them (Settings / k2 connections add): the ask
    // closes, the ticket resolves, the agent is told once.
    let pq = format!(
        "/cli/connections?token={}&project={}&action=add&target={}",
        enc(OWNER),
        enc(&newbie.path.to_string_lossy()),
        enc(&billing.path.to_string_lossy())
    );
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(String::new()))).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let rep = k2_daemon::org_chart::reach::tell_pass(&fake, now + 1);
    assert_eq!(rep.connected, 1, "{rep:?}");
    {
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, newbie.id);
        assert!(calls[0].1.contains("You're now connected to billing (Billing's head): k2 msg billing"), "{}", calls[0].1);
    }
    assert_eq!(k2_core::feedback::get_item(&ticket).unwrap().status, "resolved");
    k2_daemon::org_chart::reach::tell_pass(&fake, now + 2);
    assert_eq!(fake.calls.borrow().len(), 1, "told once");
    let r = post(port, "/cli/organization/reach", &pn, json!({"org": "Billing", "why": "now?"})).await;
    let v = j(&r);
    assert_eq!(v["status"], "connected", "{v}");
    assert_eq!(v["msg"], "k2 msg billing \"<your message>\"");

    // A dismissed ticket: declined, told once.
    let r = post(port, "/cli/organization/reach", &passport(&helpdesk), json!({"org": "Research", "why": "x"})).await;
    // Research has no head yet.
    assert_eq!((r.status, code(&r)), (409, "no_head".to_string()), "{}", r.body);
    let r = post(port, "/cli/organization/member/add", OWNER, json!({"org": "Research", "agent": "newbie"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/organization/head", OWNER, json!({"org": "Research", "agent": "newbie"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/organization/reach", &passport(&helpdesk), json!({"org": "Research", "why": "sources?"})).await;
    let t2 = j(&r)["ticket"].as_str().unwrap_or_else(|| panic!("{}", r.body)).to_string();
    k2_core::feedback::set_status(&t2, "dismissed").unwrap();
    let rep = k2_daemon::org_chart::reach::tell_pass(&fake, now + 3);
    assert_eq!(rep.declined, 1, "{rep:?}");
    assert!(fake.calls.borrow().last().unwrap().1.contains("was declined"));
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reach_files_the_connections_add_card_when_cards_are_on() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let head = seed_ws(&env, "support");
    let other = seed_ws(&env, "support-2");
    let asker = seed_ws(&env, "asker");
    let r = post(port, "/cli/settings/update", OWNER, json!({"orgChart": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for (path, body) in [
        ("/cli/organization/create", json!({"name": "Support"})),
        ("/cli/organization/member/add", json!({"org": "Support", "agent": "support"})),
        ("/cli/organization/member/add", json!({"org": "Support", "agent": "support-2"})),
        ("/cli/organization/head", json!({"org": "Support", "agent": "support"})),
    ] {
        let r = post(port, path, OWNER, body).await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    }
    let _ = other;
    // Permission cards on for the server and the asking agent, which has a
    // pinned Chat (its Thread is where the card goes up).
    k2_core::permission::gate::set_server_switch(true);
    k2_core::workspace::settings::set_agents_can_request_permissions(&asker.id, true).unwrap();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3",
            params![uuid::Uuid::new_v4().to_string(), asker.id, uuid::Uuid::new_v4().to_string()],
        )
        .unwrap();
    }
    let pa = passport(&asker);
    let r = post(port, "/cli/organization/reach", &pa, json!({"org": "support", "why": "a customer is stuck on a login"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["status"], "card", "{v}");
    assert_eq!(v["request"]["status"].as_str().map(|s| s != "already_done"), Some(true), "{v}");
    let open = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::org_chart::store::open_reach(&conn, &asker.id, &head.id).unwrap().expect("an open ask")
    };
    assert_eq!(open.via, "card");
    let rid = open.permission_request_id.clone().expect("the card's request id");
    // The manage refusal teaches organization.manage while cards are on.
    let r = post(port, "/cli/organization/create", &pa, json!({"name": "Mine"})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v = j(&r);
    assert_eq!(v["error"]["code"], "org_manage_off");
    assert_eq!(v["permission"]["action"], "organization.manage", "{v}");
    assert!(v["permission"]["command"].as_str().unwrap().starts_with("k2 permission request organization.manage"), "{v}");
    // Connected by any path (here a raw row, as the card's apply writes):
    // the ask closes and the agent is told.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_relations (id, source_project_id, target_project_id, relation_type, created_at) VALUES ('rel-card', ?1, ?2, 'peer', 1)",
            params![asker.id, head.id],
        )
        .unwrap();
    }
    let fake = Fake::default();
    let rep = k2_daemon::org_chart::reach::tell_pass(&fake, chrono::Utc::now().timestamp());
    assert_eq!(rep.connected, 1, "{rep:?} (request {rid})");
    k2_core::permission::gate::set_server_switch(false);
    drop(env);
}
