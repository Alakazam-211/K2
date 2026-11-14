//! K2 speed pass (prd-k2-speed-pass-v1), headless: the real dispatcher in
//! process on a temp `$HOME`, with the main database opened the way the
//! daemon opens it (`k2_core::db::init_database`: a file in WAL mode), not
//! the in-memory test database.
//!
//! ISOLATION: the database handle is process-wide and set once, so every
//! test here needs a process of its own. Run with cargo-nextest, or one test
//! per `cargo test … -- --exact <name>` call.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use rusqlite::params;

/// Open the main database under the temp `$HOME` exactly as the daemon does
/// at boot, and prove it is the file (WAL), not the in-memory test DB.
fn open_file_db() {
    let db = k2_core::db::init_database().expect("init_database under the temp HOME");
    let mode: String = db.lock().query_row("PRAGMA journal_mode", [], |r| r.get(0)).expect("journal_mode");
    assert_eq!(mode.to_ascii_lowercase(), "wal", "the main database is the WAL file");
}

/// `n` projects with `per` workspaces each and a saved layout per
/// workspace. Returns (project id, handle) of each.
fn seed_projects(env: &Env, n: usize, per: usize) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute_batch("BEGIN").expect("begin");
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("budget{i:03}");
        let path = env.path().join(format!("ws-{handle}"));
        conn.execute(
            "INSERT INTO projects (id, name, path, handle, tab_order) VALUES (?1, ?2, ?3, ?2, ?4)",
            params![id, handle, path.to_string_lossy(), i as i64],
        )
        .expect("project");
        for w in 0..per {
            let ws_id = uuid::Uuid::new_v4().to_string();
            k2_core::db::schema::Workspace::create(&conn, &ws_id, &id, None, "branch", Some("main"), &format!("main-{w}"), w as i64, None)
                .expect("workspace");
            let layout = serde_json::json!({
                "version": 2,
                "columns": [{ "items": [{ "id": format!("tab-{ws_id}"), "type": "terminal", "title": "Chat", "pinned": true }] }],
                "activeItemId": format!("tab-{ws_id}"),
            })
            .to_string();
            conn.execute(
                "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) VALUES (?1, ?2, ?3, ?4, unixepoch())",
                params![format!("{id}:{ws_id}"), id, ws_id, layout],
            )
            .expect("layout");
        }
        out.push((id, handle));
    }
    conn.execute_batch("COMMIT").expect("commit");
    out
}

/// Pin a Chat for a project; its conversation id.
fn pin_chat(project_id: &str) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), project_id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

fn post_lines(conv: &str, project_id: &str, from: &str, n: usize) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for i in 0..n {
        k2_core::overlay::post_thread(&conn, conv, project_id, from, "owner", &format!("line {i}: the agent's progress note"), "cli")
            .expect("post");
    }
}

/// The sizes (bytes) measured when this test was written, with 20% on top
/// (measured: 373,691 / 210,601 / 8,094 / 9,438 / 115 / 2). A later slice
/// that makes a payload smaller lowers its line here.
const BUDGETS: &[(&str, usize)] = &[
    ("projects/list", 448_430),
    ("workspace-layouts/load-all", 252_722),
    ("thread?limit=25", 9_713),
    ("thread/unread", 11_326),
    ("activity/snapshot", 138),
    ("agents/running", 3),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_load_payloads_stay_under_their_budgets() {
    let env = setup(&[]);
    open_file_db();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;

    let projects = seed_projects(&env, 200, 3);
    // The owner is seen before anything is posted, so the lines below are
    // unread for them (a person seen for the first time starts all read).
    let first = get(port, "/cli/thread/unread", OWNER, "").await;
    assert_eq!(first.status, 200, "{}", first.body);

    let mut convs = Vec::new();
    for (id, handle) in projects.iter().take(50) {
        let conv = pin_chat(id);
        post_lines(&conv, id, handle, 3);
        convs.push(conv);
    }
    let (big_id, big_handle) = &projects[0];
    post_lines(&convs[0], big_id, big_handle, 5_000 - 3);

    let routes: Vec<(&str, String)> = vec![
        ("projects/list", "/cli/projects/list".into()),
        ("workspace-layouts/load-all", "/cli/workspace-layouts/load-all".into()),
        ("thread?limit=25", format!("/cli/thread?addr={big_handle}&limit=25")),
        ("thread/unread", "/cli/thread/unread".into()),
        ("activity/snapshot", "/cli/activity/snapshot".into()),
        ("agents/running", "/cli/agents/running".into()),
    ];
    let mut table = String::from("\npayload budget (200 projects x 3 workspaces, 50 Threads, one of 5,000 lines):\n");
    let mut over = Vec::new();
    for (name, path) in &routes {
        let (p, q) = path.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((path.clone(), String::new()));
        let r = get(port, &p, OWNER, &q).await;
        assert_eq!(r.status, 200, "{name}: {}", r.body);
        let bytes = r.body.len();
        let budget = BUDGETS.iter().find(|(n, _)| n == name).map(|(_, b)| *b).unwrap_or_else(|| panic!("no budget for {name}"));
        table.push_str(&format!("  {name:<28} {bytes:>9} bytes   budget {budget}\n"));
        if bytes > budget {
            over.push(format!("{name}: {bytes} > {budget}"));
        }
    }
    eprintln!("{table}");
    // The seed reached the routes (a budget over an empty answer proves nothing).
    let unread = get(port, "/cli/thread/unread", OWNER, "").await.json();
    assert!(unread["total"].as_i64().expect("total") >= 50, "the seeded Threads are unread: {unread}");
    let list = get(port, "/cli/projects/list", OWNER, "").await.json();
    assert!(list.as_array().expect("list").len() >= 200, "200 projects listed");
    assert!(over.is_empty(), "over budget: {over:?}{table}");
}
