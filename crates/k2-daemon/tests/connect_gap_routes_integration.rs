//! K2 Connect host-awareness GAP — daemon-route integration regression
//! tests.
//!
//! These drive REAL HTTP POST requests through the REAL
//! `routes::dispatcher::dispatch` (started in-process on an ephemeral
//! 127.0.0.1 port by `k2_daemon::test_harness::start`) and assert the
//! full dispatch + method-gate + token-gate + handler stack for two
//! representative new routes:
//!
//!   * `POST /cli/skills/create` — filesystem-backed (writes
//!     `.k2so/skills/<name>/SKILL.md` under a temp workspace).
//!   * `POST /cli/relations/create` — DB-backed (writes a row into the
//!     shared in-memory `workspace_relations` table).
//!
//! Plus the auth + method gates that protect EVERY new GAP route:
//!   * missing/garbage token → 403 (token gate),
//!   * GET on a POST-only GAP path → 404 (the `is_post && post_allowed`
//!     arm guard can't match a GET, so it falls to the catchall → 404;
//!     never a silent mutation).
//!
//! Harness pattern mirrors `auth_routes_integration.rs`: serialize on a
//! process-wide lock, point `$HOME` at a fresh tempdir, init the
//! in-memory DB, talk to the daemon over a raw loopback TCP socket.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_daemon::test_harness;

/// Serialize: `$HOME` + the shared in-memory DB are process-wide.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-gap-2a";

struct Resp {
    status: u16,
    body: String,
}

/// Fire one raw HTTP request and return the parsed status + body.
fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
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
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
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

/// Redirect `$HOME` to a fresh tempdir, init the in-memory DB, run `f`,
/// restore `$HOME`. Returns the tempdir so the caller can use it as a
/// workspace root. The caller already holds `TEST_LOCK`.
fn with_temp_home<T, F: FnOnce(&std::path::Path) -> T>(f: F) -> T {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2so-gap2a-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();

    let out = f(&tmp);

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

// ─────────────────────────────────────────────────────────────────────
// /cli/skills/create — filesystem-backed round trip
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_create_writes_skill_md_via_real_dispatch() {
    let _g = lock();
    with_temp_home(|workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let pp = workspace.to_string_lossy().to_string();

        let body = serde_json::json!({ "project_path": pp, "name": "gap-skill" }).to_string();
        let r = http(
            d.port,
            "POST",
            &format!("/cli/skills/create?token={OWNER_TOKEN}"),
            Some(&body),
        );
        assert_eq!(r.status, 200, "owner POST create → 200; body={}", r.body);
        assert!(
            workspace.join(".k2so/skills/gap-skill/SKILL.md").exists(),
            "SKILL.md must land on disk through the real route"
        );
        // The handler echoes the created SkillSummary.
        assert!(r.body.contains("gap-skill"), "summary echoed; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_create_rejects_missing_token() {
    let _g = lock();
    with_temp_home(|workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let pp = workspace.to_string_lossy().to_string();
        let body = serde_json::json!({ "project_path": pp, "name": "nope" }).to_string();

        // No token → 403 (token gate), and NOTHING is written.
        let r = http(d.port, "POST", "/cli/skills/create", Some(&body));
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
        assert!(
            !workspace.join(".k2so/skills/nope/SKILL.md").exists(),
            "a 403'd request must not have created the skill"
        );

        // Garbage token → 403 too.
        let r = http(
            d.port,
            "POST",
            "/cli/skills/create?token=garbage",
            Some(&body),
        );
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skills_create_get_is_method_gated() {
    let _g = lock();
    with_temp_home(|_workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        // GET on a POST-only GAP path: the `is_post && post_allowed` arm
        // guard can't match a GET, so it falls to the /cli/ catchall →
        // crate::cli::dispatch → unknown route 404. Never a 200/mutation.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/skills/create?token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(
            r.status, 404,
            "GET on POST-only create must 404 (method gate); body={}",
            r.body
        );
    });
}

// ─────────────────────────────────────────────────────────────────────
// /cli/relations/create — DB-backed round trip
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relations_create_writes_row_via_real_dispatch() {
    let _g = lock();
    with_temp_home(|_workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // workspace_relations has a FK to projects(id), so seed both
        // endpoints first (the route correctly 400s on a dangling id —
        // verified incidentally by the FK error if this seeding is wrong).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::Project::create(
                &conn, "proj-src-gap", "Src", "/tmp/src-gap", "#fff", 0, 0, None, None,
            )
            .expect("seed src project");
            k2_core::db::schema::Project::create(
                &conn, "proj-dst-gap", "Dst", "/tmp/dst-gap", "#fff", 1, 0, None, None,
            )
            .expect("seed dst project");
        }

        let body = serde_json::json!({
            "source_project_id": "proj-src-gap",
            "target_project_id": "proj-dst-gap"
        })
        .to_string();
        let r = http(
            d.port,
            "POST",
            &format!("/cli/relations/create?token={OWNER_TOKEN}"),
            Some(&body),
        );
        assert_eq!(r.status, 200, "owner POST relations/create → 200; body={}", r.body);

        // The created row must be readable back through core (same DB the
        // route wrote to). Confirms the write reached the shared DB, not
        // just that the handler returned 200.
        let rows = k2_core::workspace::relations::workspace_relations_list(
            "proj-src-gap".to_string(),
        )
        .expect("list relations");
        assert!(
            rows.iter().any(|rel| rel.target_project_id == "proj-dst-gap"),
            "created relation must be listable; rows={rows:?}"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relations_create_rejects_missing_token() {
    let _g = lock();
    with_temp_home(|_workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let body = serde_json::json!({
            "source_project_id": "s",
            "target_project_id": "t"
        })
        .to_string();
        let r = http(d.port, "POST", "/cli/relations/create", Some(&body));
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// /cli/relations/list{,-incoming} — host-aware GET list reads
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relations_list_reads_rows_via_real_dispatch() {
    let _g = lock();
    with_temp_home(|_workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Seed two projects + one relation (src oversees dst).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::Project::create(
                &conn, "proj-src-list", "Src", "/tmp/src-list", "#fff", 0, 0, None, None,
            )
            .expect("seed src project");
            k2_core::db::schema::Project::create(
                &conn, "proj-dst-list", "Dst", "/tmp/dst-list", "#fff", 1, 0, None, None,
            )
            .expect("seed dst project");
        }
        k2_core::workspace::relations::workspace_relations_create(
            "proj-src-list".to_string(),
            "proj-dst-list".to_string(),
            None,
        )
        .expect("seed relation");

        // OUTGOING list for the SOURCE project must contain the row, in
        // the camelCase `WorkspaceRelation` shape the renderer parses.
        let r = http(
            d.port,
            "GET",
            &format!("/cli/relations/list?project_id=proj-src-list&token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(r.status, 200, "owner GET relations/list → 200; body={}", r.body);
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&r.body).expect("relations/list returns a JSON array");
        assert_eq!(rows.len(), 1, "exactly the seeded row; body={}", r.body);
        // camelCase keys — the EXACT shape `WorkspaceRelation[]` expects.
        let row = &rows[0];
        assert_eq!(row["sourceProjectId"], "proj-src-list", "body={}", r.body);
        assert_eq!(row["targetProjectId"], "proj-dst-list", "body={}", r.body);
        assert_eq!(row["relationType"], "oversees", "body={}", r.body);
        assert!(row["id"].is_string(), "id present; body={}", r.body);
        assert!(row["createdAt"].is_number(), "createdAt present; body={}", r.body);

        // INCOMING list for the SOURCE is empty; for the TARGET it has the row.
        let empty = http(
            d.port,
            "GET",
            &format!("/cli/relations/list-incoming?project_id=proj-src-list&token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(empty.status, 200, "body={}", empty.body);
        assert_eq!(empty.body, "[]", "src has no incoming; body={}", empty.body);

        let inc = http(
            d.port,
            "GET",
            &format!("/cli/relations/list-incoming?project_id=proj-dst-list&token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(inc.status, 200, "body={}", inc.body);
        let inc_rows: Vec<serde_json::Value> =
            serde_json::from_str(&inc.body).expect("list-incoming returns a JSON array");
        assert_eq!(inc_rows.len(), 1, "dst has one incoming; body={}", inc.body);
        assert_eq!(inc_rows[0]["sourceProjectId"], "proj-src-list", "body={}", inc.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relations_list_rejects_missing_token() {
    let _g = lock();
    with_temp_home(|_workspace| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = http(d.port, "GET", "/cli/relations/list?project_id=x", None);
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
    });
}
