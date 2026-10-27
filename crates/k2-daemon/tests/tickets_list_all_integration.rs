//! prd-tickets-badge-orphans — `GET /cli/feedback/list-all` through the
//! REAL dispatcher (`k2_daemon::test_harness::start`).
//!
//!   1. A Member login reads it, and it returns a ticket whose workspace
//!      was removed with `linked: false` and a null workspace.
//!   2. An agent passport (scoped session token) is refused.
//!   3. A POST is 405 (the route is GET-only in `route_policy`).
//!   4. `list?all=1` without `project` is a 400 that points to `list-all`.
//!   5. `waiting-count` counts `waiting` + `needs_discussion`, and `me=1`
//!      scopes it to the calling login (feature `tickets-badge-me`).
//!
//! The app (skin) 404 lives in `publish_skin_gateway.rs`, next to the
//! `waiting-count` 404.
//!
//! ISOLATION: connect-user stores, `$HOME` and the in-memory DB are
//! process-wide, so every test serializes on `TEST_LOCK`. Assertions on
//! the shared DB check membership by id, never a row count.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-tickets-list-all";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(resp) = try_parse(&raw) {
            return resp;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    try_parse(&raw).unwrap_or_else(|| {
        panic!("incomplete response: {:?}", String::from_utf8_lossy(&raw))
    })
}

fn try_parse(raw: &[u8]) -> Option<Resp> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })?;
    if body.len() < content_len {
        return None;
    }
    Some(Resp { status, body: body.to_string() })
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-tickets-list-all-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();

    f();

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&tmp).expect("remove temp HOME");
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// A ticket on a workspace that is then removed (its `projects` row is
/// deleted, as `remove_workspace_db_only` does). Returns the ticket id.
fn seed_unlinked_ticket(title: &str) -> String {
    let uid = uuid::Uuid::new_v4();
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![project_id, format!("gone-{uid}"), format!("/tmp/tb-gone-{uid}")],
        )
        .expect("insert project");
    }
    let item = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id: project_id.clone(),
        agent_name: "scout".to_string(),
        title: title.to_string(),
        ..Default::default()
    })
    .expect("create ticket");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id])
        .expect("delete project");
    item.id
}

fn items(body: &str) -> Vec<serde_json::Value> {
    let v = json(body);
    assert_eq!(v["ok"], serde_json::json!(true), "{body}");
    v["items"].as_array().expect("items array").clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_reads_list_all_with_the_unlinked_ticket() {
    let _g = lock();
    with_temp_home(|| {
        let id = seed_unlinked_ticket("Approve PR");
        connect_users::add_user("tla_member", "password123").expect("add_user");
        let member = connect_users::create_session("tla_member");
        assert_eq!(connect_users::role_for_user("tla_member"), Some(Role::Member));
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        for (who, tok) in [("owner", OWNER_TOKEN), ("member", member.as_str())] {
            let r = http(d.port, "GET", &format!("/cli/feedback/list-all?token={tok}"), None);
            assert_eq!(r.status, 200, "{who} list-all; body={}", r.body);
            let row = items(&r.body)
                .into_iter()
                .find(|row| row["id"] == id.as_str())
                .unwrap_or_else(|| panic!("{who}: list-all must contain {id}: {}", r.body));
            assert_eq!(row["linked"], serde_json::json!(false), "{row}");
            assert_eq!(row["projectName"], serde_json::Value::Null, "{row}");
            assert_eq!(row["projectPath"], serde_json::Value::Null, "{row}");
            assert_eq!(row["title"], serde_json::json!("Approve PR"), "{row}");
            assert_eq!(row["agentName"], serde_json::json!("scout"), "{row}");
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_passport_is_refused_list_all() {
    let _g = lock();
    with_temp_home(|| {
        let id = seed_unlinked_ticket("Deploy?");
        let sid = SessionId::new();
        let principal = HookPrincipal {
            workspace_uuid: "ws-tickets-list-all".to_string(),
            agent_address: "agent-tickets-list-all".to_string(),
        };
        let passport = session_token::mint_session_token(
            &sid,
            "pane-1",
            principal,
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        );
        assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
        assert!(!session_token::is_agent_verb("/cli/feedback/list-all"));
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(d.port, "GET", &format!("/cli/feedback/list-all?token={passport}"), None);
        assert_eq!(r.status, 403, "agent passport must be refused; body={}", r.body);
        assert!(!r.body.contains(&id), "refusal must not leak tickets: {}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_list_all_is_405_and_list_all_flag_without_project_is_400() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let r = http(d.port, "POST", &format!("/cli/feedback/list-all?token={OWNER_TOKEN}"), Some("{}"));
        assert_eq!(r.status, 405, "POST list-all; body={}", r.body);

        let r = http(d.port, "GET", &format!("/cli/feedback/list?all=1&token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 400, "list?all=1 without project; body={}", r.body);
        let v = json(&r.body);
        assert_eq!(
            v["error"]["hint"],
            serde_json::json!("all=1 requires project=; use /cli/feedback/list-all"),
            "{}",
            r.body
        );
    });
}

/// A ticket on a workspace that stays registered, set to `status` and
/// assigned to `assignees`. Returns the ticket id.
fn seed_linked_ticket(title: &str, status: &str, assignees: &[&str]) -> String {
    let uid = uuid::Uuid::new_v4();
    let project_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![project_id, format!("live-{uid}"), format!("/tmp/tb-live-{uid}")],
        )
        .expect("insert project");
    }
    let item = k2_core::feedback::create(k2_core::feedback::NewFeedback {
        project_id,
        agent_name: "scout".to_string(),
        title: title.to_string(),
        ..Default::default()
    })
    .expect("create ticket");
    if status != "waiting" {
        k2_core::feedback::set_status(&item.id, status).expect("set status");
    }
    if !assignees.is_empty() {
        let names: Vec<String> = assignees.iter().map(|s| s.to_string()).collect();
        k2_core::feedback::set_assignees(&item.id, &names).expect("assign");
    }
    item.id
}

/// The badge counts `waiting` + `needs_discussion`; `me=1` scopes it to
/// the calling login: the owner token is `owner` + the owner's display
/// name, a Connect-user session is its username. Answered tickets and
/// other people's tickets never count. Deltas, not totals: the DB is
/// process-wide (every test here holds `TEST_LOCK`, so the delta is ours).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_count_counts_needs_discussion_and_me_scopes_to_the_login() {
    let _g = lock();
    with_temp_home(|| {
        k2_core::app_settings::update(serde_json::json!({ "ownerDisplayName": "Rosson Test" }))
            .expect("set owner display name");
        connect_users::add_user("tlb_member", "password123").expect("add_user");
        let member = connect_users::create_session("tlb_member");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let read = |q: &str, tok: &str, scope: &str| -> i64 {
            let r = http(d.port, "GET", &format!("/cli/feedback/waiting-count?{q}token={tok}"), None);
            assert_eq!(r.status, 200, "waiting-count {q}; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["scope"], serde_json::json!(scope), "{}", r.body);
            v["count"].as_i64().unwrap_or_else(|| panic!("count must be a number: {}", r.body))
        };
        let host_before = read("", OWNER_TOKEN, "host");
        let owner_before = read("me=1&", OWNER_TOKEN, "me");
        let member_before = read("me=1&", member.as_str(), "me");

        seed_linked_ticket("owner waiting", "waiting", &["owner"]);
        seed_linked_ticket("owner by display name", "needs_discussion", &["ROSSON TEST"]);
        seed_linked_ticket("owner answered", "answered", &["owner"]);
        seed_linked_ticket("owner resolved", "resolved", &["owner"]);
        seed_linked_ticket("member talk", "needs_discussion", &["tlb_member"]);
        seed_linked_ticket("nobody's", "waiting", &[]);

        assert_eq!(read("", OWNER_TOKEN, "host") - host_before, 4, "host-wide: 2 waiting + 2 needs_discussion");
        assert_eq!(read("", member.as_str(), "host") - host_before, 4, "host-wide is the same for a member");
        assert_eq!(read("me=1&", OWNER_TOKEN, "me") - owner_before, 2, "owner: owner + display name");
        assert_eq!(read("me=1&", member.as_str(), "me") - member_before, 1, "member: their own ticket only");
    });
}

/// 0.46.0 smoke (Rosson on z3mbpZ): a ticket assigned to `z3thon`, the
/// Connect login he linked as "This is me (owner)", must be "Waiting on
/// me" for the owner token. Unlinked, the owner counts only `owner`
/// assignments. Linked, the owner token AND the `z3thon` session count it,
/// and whoami's `ticketMe` (the board's "me") carries the same names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_linked_account_tickets_are_waiting_on_the_owner() {
    let _g = lock();
    with_temp_home(|| {
        // The in-memory DB is process-wide, and an earlier test in this
        // file leaves waiting tickets assigned to the wire `owner`. Linking
        // makes tlbz3thon count `owner` tickets too, so those leftovers
        // used to land in its delta whenever that test ran first (the
        // order-dependent flake). Start from an empty ticket table (we hold
        // TEST_LOCK; comments and briefs cascade) so the counts below are
        // exactly the tickets this test seeds.
        k2_core::db::shared().lock().execute("DELETE FROM feedback", []).expect("clear tickets");
        k2_core::app_settings::update(serde_json::json!({ "ownerDisplayName": "Rosson Link" }))
            .expect("set owner display name");
        connect_users::add_user("tlbz3thon", "password123").expect("add_user");
        let linked = connect_users::create_session("tlbz3thon");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let read = |tok: &str| -> i64 {
            let r = http(d.port, "GET", &format!("/cli/feedback/waiting-count?me=1&token={tok}"), None);
            assert_eq!(r.status, 200, "waiting-count; body={}", r.body);
            let v = json(&r.body);
            assert_eq!(v["scope"], serde_json::json!("me"), "{}", r.body);
            v["count"].as_i64().unwrap_or_else(|| panic!("count must be a number: {}", r.body))
        };
        let ticket_me = |tok: &str| -> serde_json::Value {
            let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={tok}"), None);
            assert_eq!(r.status, 200, "whoami; body={}", r.body);
            json(&r.body)["ticketMe"].clone()
        };

        // Unlinked: the owner is `owner` + display name; z3thon is z3thon.
        assert_eq!(ticket_me(OWNER_TOKEN), serde_json::json!(["owner", "rosson link"]));
        assert_eq!(ticket_me(linked.as_str()), serde_json::json!(["tlbz3thon"]));
        let owner_before = read(OWNER_TOKEN);
        let linked_before = read(linked.as_str());
        seed_linked_ticket("assigned to the linked login", "waiting", &["TLBz3thon "]);
        seed_linked_ticket("assigned to owner", "needs_discussion", &["owner"]);
        // A legacy ticket assigned by the old owner display name.
        seed_linked_ticket("assigned by the old display name", "waiting", &["Rosson Link"]);
        assert_eq!(read(OWNER_TOKEN) - owner_before, 2, "unlinked owner: the `owner` + legacy display-name tickets");
        assert_eq!(read(linked.as_str()) - linked_before, 1, "unlinked z3thon: only their own ticket");

        // Link z3thon as the owner (Settings → People → "This is me (owner)").
        let r = http(
            d.port,
            "POST",
            &format!("/cli/owner/accounts?token={OWNER_TOKEN}"),
            Some(r#"{"accounts":["tlbz3thon"]}"#),
        );
        assert_eq!(r.status, 200, "link owner account; body={}", r.body);

        let both = serde_json::json!(["owner", "rosson link", "tlbz3thon"]);
        assert_eq!(ticket_me(OWNER_TOKEN), both, "owner token: owner + display + linked");
        assert_eq!(ticket_me(linked.as_str()), both, "linked login: the same person");
        assert_eq!(read(OWNER_TOKEN) - owner_before, 3, "linked owner: all three tickets");
        assert_eq!(
            read(linked.as_str()) - linked_before,
            3,
            "linked login: all three, including the legacy display-name ticket"
        );
        // With the table cleared at the start, the absolute counts are the
        // seeded tickets exactly (no leftovers from other tests).
        assert_eq!((owner_before, linked_before), (0, 0));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_passport_is_refused_waiting_count() {
    let _g = lock();
    with_temp_home(|| {
        let principal = HookPrincipal {
            workspace_uuid: "ws-tickets-badge".to_string(),
            agent_address: "agent-tickets-badge".to_string(),
        };
        let passport = session_token::mint_session_token(
            &SessionId::new(),
            "pane-1",
            principal,
            session_token::CredMode::ApiKey,
            session_token::Provider::Anthropic,
        );
        assert!(!session_token::is_agent_verb("/cli/feedback/waiting-count"));
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        for q in ["", "me=1&"] {
            let r = http(d.port, "GET", &format!("/cli/feedback/waiting-count?{q}token={passport}"), None);
            assert_eq!(r.status, 403, "agent passport must be refused ({q}); body={}", r.body);
        }
        let r = http(d.port, "GET", "/cli/feedback/waiting-count?me=1", None);
        assert_eq!(r.status, 403, "no token must be refused; body={}", r.body);
    });
}
