//! 0.37.8 — `use_workspace_session` branch routes the heartbeat fire
//! through `workspace_msg::deliver_live` (the `k2so msg --wake`
//! primitive) so the WAKEUP.md prompt lands in the workspace's pinned
//! chat session instead of the heartbeat's own saved session.
//!
//! Pinning contracts the test guards:
//! 1. `smart_launch` returns success with a `workspace_session:*`
//!    branch tag when `use_workspace_session = 1`.
//! 2. The v2 session it spawned is registered under the canonical
//!    workspace_id key in `v2_session_map` (= the same key the chat
//!    tab attaches to via `agent-chat:<pid>`), NOT under the
//!    heartbeat's own key.
//! 3. The heartbeat row's `last_session_id` and `active_terminal_id`
//!    stay UNTOUCHED on the new path — un-checking the flag must
//!    leave the historical session intact.
//! 4. A `heartbeat_fires` audit row is written with decision `fired`
//!    so `k2so heartbeat status <name>` reflects the decision.
//!
//! Test substitutes `claude` with `cat` via
//! `K2SO_WAKE_HEADLESS_TEST_COMMAND` so we don't need claude on PATH
//! and don't burn API calls.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire, WorkspaceSession};

use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-hb-uws-test-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'manager')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            "hb-uws-test",
        ],
    )
    .unwrap();
    project_path
}

fn write_primary_agent(project: &Path, name: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!("---\nname: {name}\ntype: manager\n---\n# {name}\n");
    std::fs::write(dir.join("AGENT.md"), body).unwrap();
}

fn write_wakeup(project: &Path, hb_name: &str, body: &str) -> String {
    let rel = format!(".k2so/heartbeats/{hb_name}/WAKEUP.md");
    let abs = project.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, body).unwrap();
    rel
}

struct TestCommandGuard {
    prior: Option<String>,
}

impl TestCommandGuard {
    fn set(cmd: &str) -> Self {
        let prior = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND").ok();
        std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", cmd);
        Self { prior }
    }
}

impl Drop for TestCommandGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", v),
            None => std::env::remove_var("K2SO_WAKE_HEADLESS_TEST_COMMAND"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn use_workspace_session_routes_through_deliver_live_and_leaves_heartbeat_fields_untouched() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-uws-ws-1";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let wakeup_rel = write_wakeup(&project, "uws-hb", "the wakeup body\n");

    let prior_session_id = "00000000-1111-2222-3333-444444444444";
    let prior_active_terminal_id = "deadbeef-aaaa-bbbb-cccc-eeeeffffeeee";

    // Seed a heartbeat row WITH historical session_id + active_terminal_id
    // so we can assert those stay untouched after the use_workspace_session
    // fire. Default-on flag.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            "uws-hb-id",
            workspace_id,
            "uws-hb",
            "hourly",
            "{}",
            &wakeup_rel,
            true,
        )
        .expect("seed heartbeat");
        // Stamp the heartbeat with historical session info so we can
        // assert it stays put on the new path.
        AgentHeartbeat::save_active_terminal_id(
            &conn,
            workspace_id,
            "uws-hb",
            prior_active_terminal_id,
        )
        .expect("seed active_terminal_id");
        conn.execute(
            "UPDATE workspace_heartbeats SET last_session_id = ?1, use_workspace_session = 1 \
             WHERE project_id = ?2 AND name = ?3",
            rusqlite::params![prior_session_id, workspace_id, "uws-hb"],
        )
        .expect("seed last_session_id + use_workspace_session = 1");
    }

    // Fire — should route through workspace_msg::deliver_live → fresh_fire
    // (no workspace_sessions row exists yet, so Branch 3 of the cascade).
    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "uws-hb");

    assert_eq!(
        result.get("success").and_then(|v| v.as_bool()),
        Some(true),
        "use_workspace_session fire should succeed; got {result}"
    );
    assert_eq!(
        result.get("decision").and_then(|v| v.as_str()),
        Some("fired"),
        "decision should be 'fired'; got {result}"
    );
    let branch = result
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        branch.starts_with("workspace_session:"),
        "branch should be tagged workspace_session:*; got {branch}"
    );
    let target_session_id = result
        .get("targetSessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        !target_session_id.is_empty(),
        "targetSessionId should be the spawned v2 session id"
    );

    // Allow the spawn_and_inject DB writes to settle — they happen on
    // the same thread but the test grabs new connections below.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Contract 1: v2_session_map registers the new session under the
    // canonical workspace_id key (same key chat tab attaches to). The
    // pinned chat tab + the heartbeat fire converge on this session.
    let v2 = v2_session_map::lookup_by_agent_name(workspace_id);
    assert!(
        v2.is_some(),
        "v2_session_map missing canonical workspace_id={workspace_id}"
    );
    assert_eq!(
        v2.unwrap().session_id.to_string(),
        target_session_id,
        "v2 session id must match deliver_live's targetSessionId"
    );

    // Contract 2: workspace_sessions row populated by deliver_live
    // (same row the chat tab reads on refresh).
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws_session = WorkspaceSession::get(&conn, workspace_id)
        .expect("query workspace_sessions")
        .expect("workspace_sessions row should exist after fire");
    assert_eq!(
        ws_session.active_terminal_id.as_deref(),
        Some(target_session_id.as_str()),
        "workspace_sessions.active_terminal_id stamped to live PTY"
    );

    // Contract 3: heartbeat fields STAY untouched. last_session_id +
    // active_terminal_id retain their historical values — un-checking
    // the flag must restore the legacy targeting with the prior
    // session intact.
    let hb = AgentHeartbeat::get_by_name(&conn, workspace_id, "uws-hb")
        .expect("query heartbeat row")
        .expect("heartbeat row should exist");
    assert_eq!(
        hb.last_session_id.as_deref(),
        Some(prior_session_id),
        "use_workspace_session fire must NOT overwrite heartbeat.last_session_id"
    );
    assert_eq!(
        hb.active_terminal_id.as_deref(),
        Some(prior_active_terminal_id),
        "use_workspace_session fire must NOT overwrite heartbeat.active_terminal_id"
    );
    assert!(
        hb.last_fired.is_some(),
        "last_fired stamped on success"
    );

    // Contract 4: heartbeat_fires audit row written with decision='fired'.
    let fires = HeartbeatFire::list_by_schedule_name(&conn, workspace_id, "uws-hb", 5)
        .expect("query heartbeat_fires");
    assert!(!fires.is_empty(), "audit row should be written");
    let latest = &fires[0];
    assert_eq!(latest.decision, "fired",
        "audit row decision should be 'fired'; got {latest:?}");

    drop(conn);
    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_off_uses_legacy_heartbeat_cascade_unchanged() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-uws-ws-2";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let wakeup_rel = write_wakeup(&project, "legacy-hb", "the wakeup body\n");

    // Seed heartbeat with use_workspace_session = 0 (default-off).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            "legacy-hb-id",
            workspace_id,
            "legacy-hb",
            "hourly",
            "{}",
            &wakeup_rel,
            true,
        )
        .expect("seed heartbeat");
    }

    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "legacy-hb");

    assert_eq!(
        result.get("success").and_then(|v| v.as_bool()),
        Some(true),
        "default-off flag should still fire successfully; got {result}"
    );
    let branch = result
        .get("branch")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Legacy path produces "fresh_fire" / "injected" / "resume_and_fire"
    // — never "workspace_session:*".
    assert!(
        !branch.starts_with("workspace_session:"),
        "default-off heartbeat must NOT route through workspace_session path; got branch={branch}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Legacy path stamps heartbeat.last_session_id + active_terminal_id.
    let db = k2_core::db::shared();
    let conn = db.lock();
    let hb = AgentHeartbeat::get_by_name(&conn, workspace_id, "legacy-hb")
        .expect("query heartbeat row")
        .expect("heartbeat row should exist");
    assert!(
        hb.last_session_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        "legacy path should stamp heartbeat.last_session_id"
    );
    let hb_terminal = hb
        .active_terminal_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .expect("legacy path should stamp heartbeat.active_terminal_id");

    drop(conn);

    // 0.37.8 lane separation: the heartbeat's PTY must register under
    // its own per-heartbeat canonical key, NOT under bare workspace_id.
    // Reverting either the canonical_key override in `wake_headless` or
    // the field on `SpawnWorkspaceSessionRequest` MUST flip this to FAIL.
    let heartbeat_key = format!("{workspace_id}:hb:legacy-hb");
    let hb_session = v2_session_map::lookup_by_agent_name(&heartbeat_key)
        .expect("v2_session_map missing per-heartbeat canonical key after legacy fire");
    assert_eq!(
        hb_session.session_id.to_string(),
        hb_terminal,
        "per-heartbeat slot's session must be the same PTY heartbeat row points at"
    );
    assert!(
        v2_session_map::lookup_by_agent_name(workspace_id).is_none(),
        "legacy heartbeat fire must NOT register under bare workspace_id={workspace_id} \
         (chat tab's slot — pre-0.37.8 lane collapse regression guard)"
    );

    // workspace_sessions row must be untouched by a heartbeat fire —
    // that row is the chat tab's lane.
    let db2 = k2_core::db::shared();
    let conn2 = db2.lock();
    let ws_session = WorkspaceSession::get(&conn2, workspace_id)
        .expect("query workspace_sessions");
    assert!(
        ws_session.is_none()
            || ws_session
                .as_ref()
                .and_then(|r| r.active_terminal_id.as_deref())
                .map(|t| t != hb_terminal)
                .unwrap_or(true),
        "workspace_sessions.active_terminal_id must NOT match the heartbeat PTY id; \
         got {ws_session:?} matching hb_terminal={hb_terminal}"
    );
    drop(conn2);

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Lane-separation regression: chat tab + heartbeat = TWO entries
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_tab_and_heartbeat_register_under_separate_canonical_keys() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _cmd_guard = TestCommandGuard::set("cat");

    let workspace_id = "hb-uws-ws-3-lane";
    let project = setup_project(workspace_id);
    write_primary_agent(&project, "manager");
    let project_path = project.to_string_lossy().into_owned();
    let wakeup_rel = write_wakeup(&project, "lane-hb", "the wakeup body\n");

    // Seed heartbeat (default flag-off, the lane-collapse case from
    // pre-0.37.8). Use_workspace_session = 0 so this exercises the
    // legacy heartbeat cascade path that previously collided.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            "lane-hb-id",
            workspace_id,
            "lane-hb",
            "hourly",
            "{}",
            &wakeup_rel,
            true,
        )
        .expect("seed heartbeat");
    }

    // 1. First simulate the chat tab opening: register a v2 session
    //    under bare workspace_id (the chat-tab path). spawn_wake_headless
    //    with heartbeat_name=None is the same path the chat tab
    //    triggers via /cli/sessions/v2/spawn after AgentChatPane mount.
    let chat_terminal = k2_daemon::wake_headless::spawn_wake_headless(
        "manager",
        &project_path,
        "chat tab opens",
        None,
    )
    .expect("chat-tab spawn");

    // 2. Now fire the heartbeat. Pre-0.37.8 idempotency would return
    //    the chat tab's session and the wakeup would land in the chat
    //    tab. Post-fix the heartbeat gets its own slot.
    let result = k2_daemon::heartbeat_launch::smart_launch(&project_path, "lane-hb");
    assert_eq!(
        result.get("success").and_then(|v| v.as_bool()),
        Some(true),
        "lane-test heartbeat should fire successfully; got {result}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 3. v2_session_map should hold TWO distinct entries:
    //    - bare `<workspace_id>` → chat tab session
    //    - `<workspace_id>:hb:lane-hb` → heartbeat session
    let chat_slot = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("chat tab slot must remain registered under bare workspace_id");
    let hb_slot = v2_session_map::lookup_by_agent_name(&format!("{workspace_id}:hb:lane-hb"))
        .expect("heartbeat slot must register under per-heartbeat canonical key");

    assert_eq!(
        chat_slot.session_id.to_string(),
        chat_terminal,
        "chat tab slot must still point at the chat tab's PTY id"
    );
    assert_ne!(
        hb_slot.session_id.to_string(),
        chat_terminal,
        "heartbeat PTY MUST be different from chat tab PTY (lane collapse regression guard)"
    );

    // 4. workspace_sessions.terminal_id must still point at the chat
    //    tab's PTY, not the heartbeat's. Pre-fix the heartbeat fire's
    //    `k2so_agents_lock` clobbered this column with its own PTY id
    //    (because we removed the gate, that was the second contributor
    //    to the lane collapse).
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws_session = WorkspaceSession::get(&conn, workspace_id)
        .expect("query workspace_sessions")
        .expect("chat tab spawn populates workspace_sessions row");
    assert_eq!(
        ws_session.terminal_id.as_deref(),
        Some(chat_terminal.as_str()),
        "workspace_sessions.terminal_id stays bonded to chat tab PTY (NOT clobbered by heartbeat fire)"
    );

    // 5. workspace_heartbeats.active_terminal_id points at the
    //    heartbeat's own PTY, NOT the chat tab's.
    let hb = AgentHeartbeat::get_by_name(&conn, workspace_id, "lane-hb")
        .expect("query heartbeat row")
        .expect("heartbeat row should exist");
    assert_eq!(
        hb.active_terminal_id.as_deref(),
        Some(hb_slot.session_id.to_string().as_str()),
        "heartbeat.active_terminal_id points at heartbeat PTY, not chat tab PTY"
    );
    assert_ne!(
        hb.active_terminal_id.as_deref(),
        Some(chat_terminal.as_str()),
        "heartbeat.active_terminal_id must NOT equal chat tab PTY id"
    );

    drop(conn);
    v2_session_map::clear_for_tests();
}
