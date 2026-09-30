//! Split-view V14/V15/V16 — workspace layout revisions over real HTTP.
//!
//! Driven through the REAL dispatcher (`k2_daemon::test_harness::start`),
//! so the method gate, allowlist, auth, and handlers all run.
//!
//! Contract:
//!   1. `POST /cli/workspace-layouts/save` without `baseRevision` → 200,
//!      last-write-wins (old clients, D4).
//!   2. With the current `baseRevision` → 200, revision + 1, and a
//!      `TabOrderChanged` broadcast.
//!   3. With a stale `baseRevision` → 409
//!      `{"error":"layout_revision_conflict","revision":<stored>}`; the
//!      stored layout is unchanged and nothing is broadcast.
//!   4. `GET /cli/workspace-layouts/load?with_revision=1` →
//!      `{"layoutJson":<string|null>,"revision":<n>}` (0 for a missing row);
//!      the default response stays the bare serialized string.
//!   5. `load-all` rows carry `revision`.
//!   6. A GET to `/cli/workspace-layouts/save` does not write.
//!   7. Two writers: A closes a tab at base N → N+1; B's stale layout
//!      (still holding the tab) at base N → 409; the tab stays closed.
//!
//! ISOLATION: the process-global in-memory DB is shared, so each test
//! seeds its own uniquely named project + workspace rows; tests serialize
//! on `TEST_LOCK` because the harness mutates process-wide state. The
//! harness spawns no agent; `K2_TEST_AGENT_SHIM_DIR` points at an empty
//! dir so any stray spawn fails instead of running a real CLI.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::session_events::{self, SessionEvent};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-deadbeef-layout-revision";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
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
    let (status, body, _) = try_parse(&raw).unwrap_or_else(|| {
        panic!(
            "could not parse response: {:?}",
            String::from_utf8_lossy(&raw)
        )
    });
    Resp { status, body }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
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
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn uid(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    format!("layrev-{tag}-{}-{nanos}", std::process::id())
}

/// Empty agent-shim dir: nothing here may spawn a real CLI.
fn guard_agent_spawns() {
    let dir = std::env::temp_dir().join(format!("k2-layrev-shim-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("shim dir");
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &dir);
}

/// Seed a project + workspace row (FKs are ON in the test DB).
fn seed_layout_key(tag: &str) -> (String, String) {
    k2_core::db::init_for_tests();
    let project_id = uid(&format!("{tag}-p"));
    let workspace_id = uid(&format!("{tag}-w"));
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::create(
        &conn,
        &project_id,
        "Layout Revision Test",
        &format!("/tmp/{project_id}"),
        "#fff",
        0,
        0,
        None,
        None,
    )
    .expect("seed project");
    k2_core::db::schema::Workspace::create(
        &conn,
        &workspace_id,
        &project_id,
        None,
        "default",
        None,
        "main",
        0,
        None,
    )
    .expect("seed workspace");
    (project_id, workspace_id)
}

fn save(port: u16, pid: &str, wid: &str, layout: &serde_json::Value, base: Option<i64>) -> Resp {
    let mut body = serde_json::json!({
        "projectId": pid,
        "workspaceId": wid,
        "layoutJson": layout.to_string(),
    });
    if let Some(b) = base {
        body["baseRevision"] = serde_json::json!(b);
    }
    http(
        port,
        "POST",
        &format!("/cli/workspace-layouts/save?token={OWNER_TOKEN}"),
        Some(&body.to_string()),
    )
}

/// `with_revision=1` load → (parsed layout or None, revision).
fn load_rev(port: u16, pid: &str, wid: &str) -> (Option<serde_json::Value>, i64) {
    let r = http(
        port,
        "GET",
        &format!(
            "/cli/workspace-layouts/load?token={OWNER_TOKEN}&project_id={pid}&workspace_id={wid}&with_revision=1"
        ),
        None,
    );
    assert_eq!(r.status, 200, "load with_revision; body={}", r.body);
    let v = json(&r.body);
    let obj = v.as_object().expect("with_revision load is an object");
    assert_eq!(obj.len(), 2, "exactly layoutJson + revision: {v}");
    let revision = v["revision"].as_i64().expect("revision is an integer");
    let layout = match &v["layoutJson"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(json(s)),
        other => panic!("layoutJson must be string or null, got {other}"),
    };
    (layout, revision)
}

fn saved_revision(r: &Resp) -> i64 {
    assert_eq!(r.status, 200, "save; body={}", r.body);
    let v = json(&r.body);
    assert_eq!(v["success"], serde_json::json!(true), "{v}");
    v["revision"].as_i64().expect("save response revision")
}

fn assert_conflict(r: &Resp, stored: i64) {
    assert_eq!(r.status, 409, "stale base must 409; body={}", r.body);
    assert_eq!(
        json(&r.body),
        serde_json::json!({ "error": "layout_revision_conflict", "revision": stored }),
    );
}

/// Revisions of every `TabOrderChanged` for this key received so far.
fn drain_tab_order_revisions(
    rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
    pid: &str,
    wid: &str,
) -> Vec<i64> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(SessionEvent::TabOrderChanged {
                project,
                workspace,
                revision,
                ..
            }) if project == pid && workspace == wid => out.push(revision),
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => return out,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                panic!("event receiver lagged by {n}; test cannot vouch for broadcasts")
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                panic!("session event bus closed")
            }
        }
    }
}

fn tab(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": id,
        "paneGroups": {
            format!("pg-{id}"): { "items": [{ "id": format!("it-{id}"), "type": "terminal" }] }
        }
    })
}

fn layout_with(ids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "version": 2,
        "tabs": ids.iter().map(|id| tab(id)).collect::<Vec<_>>(),
    })
}

fn tab_ids(layout: &serde_json::Value) -> Vec<String> {
    layout["tabs"]
        .as_array()
        .expect("tabs array")
        .iter()
        .map(|t| t["id"].as_str().expect("tab id").to_string())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_without_base_is_last_write_wins_and_current_base_bumps() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("lww");
    let mut rx = session_events::subscribe();

    let r1 = saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a"]), None));
    assert_eq!(r1, 1);
    let r2 = saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a", "b"]), None));
    assert_eq!(r2, 2, "no base → still accepted, revision rises");

    let r3 = saved_revision(&save(d.port, &pid, &wid, &layout_with(&["b"]), Some(2)));
    assert_eq!(r3, 3, "current base → revision + 1");
    let (layout, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, 3);
    assert_eq!(tab_ids(&layout.expect("row present")), vec!["b"]);

    assert_eq!(
        drain_tab_order_revisions(&mut rx, &pid, &wid),
        vec![1, 2, 3],
        "every accepted save broadcasts its revision"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_base_is_409_and_leaves_layout_unchanged() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("stale");

    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a"]), Some(0))), 1);
    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a", "b"]), Some(1))), 2);

    let mut rx = session_events::subscribe();
    let r = save(d.port, &pid, &wid, &layout_with(&["stale"]), Some(1));
    assert_conflict(&r, 2);
    assert!(
        drain_tab_order_revisions(&mut rx, &pid, &wid).is_empty(),
        "a refused save must not broadcast"
    );

    let (layout, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, 2, "refused save must not bump the revision");
    assert_eq!(tab_ids(&layout.expect("row present")), vec!["a", "b"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_with_revision_shapes_and_default_stays_bare() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("load");

    // Missing row.
    assert_eq!(load_rev(d.port, &pid, &wid), (None, 0));
    let bare = http(
        d.port,
        "GET",
        &format!("/cli/workspace-layouts/load?token={OWNER_TOKEN}&project_id={pid}&workspace_id={wid}"),
        None,
    );
    assert_eq!(bare.status, 200, "{}", bare.body);
    assert_eq!(json(&bare.body), serde_json::Value::Null, "default load of a missing row is null");

    // Existing row.
    let layout = layout_with(&["x"]);
    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout, None)), 1);
    let (got, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, 1);
    assert_eq!(got.expect("row present"), layout);

    let bare = http(
        d.port,
        "GET",
        &format!("/cli/workspace-layouts/load?token={OWNER_TOKEN}&project_id={pid}&workspace_id={wid}"),
        None,
    );
    assert_eq!(bare.status, 200, "{}", bare.body);
    let bare_v = json(&bare.body);
    let bare_s = bare_v.as_str().expect("default load stays a bare JSON string");
    assert_eq!(json(bare_s), layout);

    // load-all rows carry revision.
    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["x", "y"]), Some(1))), 2);
    let all = http(
        d.port,
        "GET",
        &format!("/cli/workspace-layouts/load-all?token={OWNER_TOKEN}"),
        None,
    );
    assert_eq!(all.status, 200, "{}", all.body);
    let rows = json(&all.body);
    let row = rows
        .as_array()
        .expect("load-all is an array")
        .iter()
        .find(|r| r["projectId"] == serde_json::json!(pid) && r["workspaceId"] == serde_json::json!(wid))
        .unwrap_or_else(|| panic!("load-all must include the seeded row: {rows}"));
    assert_eq!(row["revision"], serde_json::json!(2), "{row}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_to_save_does_not_write() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("get");

    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["keep"]), None)), 1);

    let body = serde_json::json!({
        "projectId": pid,
        "workspaceId": wid,
        "layoutJson": layout_with(&["evil"]).to_string(),
    })
    .to_string();
    let r = http(
        d.port,
        "GET",
        &format!("/cli/workspace-layouts/save?token={OWNER_TOKEN}"),
        Some(&body),
    );
    assert_eq!(r.status, 405, "GET save must be refused; body={}", r.body);

    let (layout, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, 1, "GET must not bump the revision");
    assert_eq!(tab_ids(&layout.expect("row present")), vec!["keep"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_layout_save_keeps_revision_rising() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("empty");

    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["only"]), Some(0))), 1);
    let empty = serde_json::json!({ "version": 2, "tabs": [] });
    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &empty, Some(1))), 2);
    let (layout, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, 2);
    assert_eq!(layout.expect("empty layout is a row, not a delete"), empty);
    // A stale window holding the closed tab cannot bring it back.
    assert_conflict(&save(d.port, &pid, &wid, &layout_with(&["only"]), Some(1)), 2);
    assert_eq!(saved_revision(&save(d.port, &pid, &wid, &layout_with(&["new"]), Some(2))), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_writers_stale_layout_cannot_resurrect_closed_tab() {
    let _g = lock();
    guard_agent_spawns();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let (pid, wid) = seed_layout_key("two");

    // Both windows start from revision N with tabs [a, b].
    let n = saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a", "b"]), None));
    let (_, base_a) = load_rev(d.port, &pid, &wid);
    let (_, base_b) = load_rev(d.port, &pid, &wid);
    assert_eq!((base_a, base_b), (n, n));

    // Writer A closes tab b.
    let after_a = saved_revision(&save(d.port, &pid, &wid, &layout_with(&["a"]), Some(base_a)));
    assert_eq!(after_a, n + 1);

    // Writer B, still holding b, adds c on its stale base.
    let r = save(d.port, &pid, &wid, &layout_with(&["a", "b", "c"]), Some(base_b));
    assert_conflict(&r, n + 1);

    let (layout, rev) = load_rev(d.port, &pid, &wid);
    assert_eq!(rev, n + 1);
    let ids = tab_ids(&layout.expect("row present"));
    assert_eq!(ids, vec!["a"], "closed tab b must stay closed");
}
