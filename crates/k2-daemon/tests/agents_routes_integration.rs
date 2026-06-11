//! H5 of Phase 4 integration tests — daemon-side
//! `/cli/agents/launch` + `/cli/agents/delegate`.
//!
//! Both routes ultimately call `spawn::spawn_agent_session`
//! (so the resulting session is daemon-owned, lands in
//! `session_map`, and is reachable from every other Phase 4
//! handler). Tests assert:
//!
//! - Bad-input paths (missing required params) → 400
//! - Launch happy path: session_map + registry gain an entry
//!   tagged with the agent name; response carries a terminalId.
//! - Delegate happy path: worktree created + registered in
//!   `workspaces` table; work item moved inbox → active with
//!   `worktree_path` + `branch` stamped in frontmatter; session
//!   spawned in the worktree cwd and registered in session_map.
//!
//! Setup cost is high — delegate requires a real git repo with
//! a HEAD commit. Helpers at the top of this file keep each
//! test body short.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;

use k2_daemon::agents_routes;
use k2_daemon::session_lookup;
use k2_daemon::v2_session_map;

/// Serialize — the DB, session_map, and the global shared
/// TerminalManager are all singletons; tests running in parallel
/// would trample each other.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn tmp_project_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2so-h5-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Register a project row in the `projects` table so
/// `resolve_project_id` finds it. Clears first to avoid
/// cross-test interference on the shared in-memory DB.
fn seed_project(path: &str) -> String {
    // SessionId doubles as a unique string generator here — it's
    // already in the test surface of k2so-core and saves a
    // dev-dep on `uuid`.
    let id = format!("proj-{}", k2_core::session::SessionId::new());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order) \
         VALUES (?1, ?2, ?3, '#123456', 'manager', 0, 0)",
        rusqlite::params![id, path, "test-project"],
    )
    .unwrap();
    id
}

fn clear_projects() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
    );
    let _ = conn.execute("DELETE FROM workspaces", []);
}

fn drain_session_map() {
    // Post-0.39.0: only the v2 map remains; the legacy v1 map was
    // retired along with the Kessel-T0 renderer.
    for (name, _) in v2_session_map::snapshot() {
        v2_session_map::unregister(&name);
    }
}

/// `git init` + initial commit so `git worktree add` has a
/// HEAD to branch from.
fn git_init(project: &Path) {
    let ok = |cmd: &[&str]| {
        let status = std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .current_dir(project)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "git command failed: {:?}\nstderr: {}",
            cmd,
            String::from_utf8_lossy(&status.stderr)
        );
    };
    ok(&["git", "init", "-q"]);
    ok(&["git", "config", "user.email", "test@example.com"]);
    ok(&["git", "config", "user.name", "Test User"]);
    ok(&["git", "config", "commit.gpgsign", "false"]);
    std::fs::write(project.join("README.md"), "seed\n").unwrap();
    ok(&["git", "add", "README.md"]);
    ok(&["git", "commit", "-q", "-m", "seed"]);
}

/// Scaffold the minimum agent layout: `.k2so/agents/<name>/`
/// with an AGENT.md.
fn write_agent_md(project: &Path, name: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AGENT.md"),
        format!(
            "---\nname: {name}\nrole: test agent\ntype: agent-template\n---\n\nTest body.\n",
            name = name
        ),
    )
    .unwrap();
}

// ─────────────────────────────────────────────────────────────────────
// Bad-input paths — don't require a project fixture
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn launch_requires_agent_param() {
    let _g = lock();
    init_for_tests();
    let resp = agents_routes::handle_agents_launch(&params(&[]), "/tmp");
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing agent"));
}

#[tokio::test(flavor = "current_thread")]
async fn delegate_requires_target_param() {
    let _g = lock();
    init_for_tests();
    let resp = agents_routes::handle_agents_delegate(
        &params(&[("file", "/tmp/x.md")]),
        "/tmp",
    );
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing target"));
}

#[tokio::test(flavor = "current_thread")]
async fn delegate_requires_file_param() {
    let _g = lock();
    init_for_tests();
    let resp = agents_routes::handle_agents_delegate(
        &params(&[("target", "alpha")]),
        "/tmp",
    );
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing file"));
}

// ─────────────────────────────────────────────────────────────────────
// Launch happy path
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn launch_fresh_agent_registers_in_session_map() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("launch");
    let proj_str = proj.to_string_lossy().into_owned();
    // Launch doesn't need a git repo — it only resolves
    // project_id + reads AGENT.md. Skipping git_init keeps this
    // test fast.
    let project_id = seed_project(&proj_str);
    write_agent_md(&proj, "alpha");

    // Override command to `true` so the child exits immediately
    // instead of pulling in `claude`. PTY still opens; session_map
    // still gets the entry.
    let resp = agents_routes::handle_agents_launch(
        &params(&[("agent", "alpha"), ("command", "true")]),
        &proj_str,
    );
    assert_eq!(resp.status, "200 OK", "body={}", resp.body);

    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["success"].as_bool(), Some(true));
    assert_eq!(v["agentName"].as_str(), Some("alpha"));
    let tid = v["terminalId"].as_str().expect("terminalId present");
    assert!(!tid.is_empty());

    // Check that the session is findable under the canonical
    // workspace key — bare project_id (post-0.37.5).
    //
    // **0.37.5:** the canonical key for a workspace's primary
    // agent is the workspace's project_id. Pre-0.37.5 it was
    // `<project_id>:<agent_name>`; the suffix was vestigial
    // post-unification and caused the renderer to compute the
    // wrong key (C3PO 5c80bef1). Reverting `spawn.rs:228` to the
    // pre-0.37.5 prefix shape MUST flip this assertion to "FAIL"
    // (verifies the test pins the new contract).
    let canonical_key = project_id.to_string();
    assert!(
        session_lookup::lookup_any(&canonical_key).is_some(),
        "{canonical_key} missing from session maps"
    );
    // Regression guard: the legacy `<pid>:<agent>` shape MUST NOT
    // resolve. If it does, something is double-registering or the
    // production code reverted to the legacy shape.
    let legacy_key = format!("{project_id}:alpha");
    assert!(
        session_lookup::lookup_any(&legacy_key).is_none(),
        "0.37.5 regression: legacy {legacy_key} must NOT be registered"
    );

    drain_session_map();
    clear_projects();
}

// ─────────────────────────────────────────────────────────────────────
// Delegate happy path — creates worktree + moves work + spawns
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn delegate_creates_worktree_and_spawns_session() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("delegate");
    let proj_str = proj.to_string_lossy().into_owned();
    git_init(&proj);
    seed_project(&proj_str);
    write_agent_md(&proj, "builder");

    // Seed a work item in the workspace inbox (delegate moves it
    // to the target agent's active/ folder).
    //
    // Post-Phase-2.1: the workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2_core::inbox::*` primitive). Pre-2.1 this was
    // `.k2so/work/inbox/` — the first-boot migration helper trashes the
    // old location.
    let workspace_inbox = proj.join(".k2so/inbox");
    std::fs::create_dir_all(&workspace_inbox).unwrap();
    let work_file = workspace_inbox.join("fix-widget.md");
    std::fs::write(
        &work_file,
        "---\n\
         title: Fix the widget\n\
         priority: high\n\
         type: task\n\
         source: manual\n\
         assigned_by: test\n\
         created: 2026-04-20\n\
         ---\n\
         Please fix the widget.\n",
    )
    .unwrap();

    let resp = agents_routes::handle_agents_delegate(
        &params(&[
            ("target", "builder"),
            ("file", &work_file.to_string_lossy()),
        ]),
        &proj_str,
    );
    assert_eq!(resp.status, "200 OK", "body={}", resp.body);

    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["success"].as_bool(), Some(true));
    assert_eq!(v["agentName"].as_str(), Some("builder"));
    let branch = v["branch"].as_str().expect("branch present").to_string();
    let worktree = v["worktreePath"]
        .as_str()
        .expect("worktreePath present")
        .to_string();
    let tid = v["terminalId"].as_str().expect("terminalId present");
    assert!(!tid.is_empty());

    // Worktree dir should exist on disk.
    assert!(
        Path::new(&worktree).exists(),
        "worktree dir missing: {worktree}"
    );

    // Work item moved from the workspace inbox → builder's active/.
    assert!(!work_file.exists(), "source file still present");
    let active = proj
        .join(".k2so/agents/builder/work/active")
        .join("fix-widget.md");
    assert!(active.exists(), "work item missing from active/: {active:?}");
    let active_content = std::fs::read_to_string(&active).unwrap();
    assert!(active_content.contains(&format!("branch: {branch}")));

    // Worktree registered in `workspaces` table.
    let n_workspaces: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM workspaces WHERE branch = ?1",
            rusqlite::params![branch],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(n_workspaces, 1, "worktree not registered in workspaces");

    // Session landed in one of the maps under the target agent.
    // Post-A9 the daemon spawn helper produces v2 sessions, so
    // builder lands in v2_session_map; lookup_any handles both.
    assert!(
        session_lookup::lookup_any("builder").is_some(),
        "builder missing from session maps"
    );

    drain_session_map();
    clear_projects();
    // Best-effort cleanup of the worktree dir. git worktree add
    // also wrote a `.git/worktrees/<name>/` record; leaving it
    // behind is harmless because tmp_project_dir randomizes.
    let _ = std::fs::remove_dir_all(&worktree);
}
