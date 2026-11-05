//! `k2 heartbeat restart <name>` (0.46.9), through the heartbeat dispatch
//! exactly as the TCP arm and the cell socket call it: an agent restarts
//! its own workspace's heartbeat with no switch; another workspace's is
//! refused (`other_workspace`) and nothing changes; a name that is not a
//! heartbeat is refused; History names who did it. Plus the stale-step
//! table a fire uses.

use std::collections::HashMap;

use serde_json::Value;

use super::*;

const ROUTE: &str = "/cli/heartbeat/restart";

struct Ws {
    id: String,
    path: String,
    handle: String,
}

fn workspace(home: &crate::test_support::TempHome, tag: &str, heartbeats: &[&str]) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("insert project");
    for hb in heartbeats {
        let rel = format!(".k2/heartbeats/{hb}/WAKEUP.md");
        std::fs::create_dir_all(dir.join(format!(".k2/heartbeats/{hb}"))).unwrap();
        std::fs::write(dir.join(&rel), "Check the queue.\n").unwrap();
        AgentHeartbeat::insert(&conn, &uuid::Uuid::new_v4().to_string(), &id, hb, "hourly", "{}", &rel, true)
            .expect("insert heartbeat");
        AgentHeartbeat::save_session_id(&conn, &id, hb, "11111111-2222-3333-4444-555555555555").expect("save session");
    }
    Ws { id, path, handle }
}

fn hb_name() -> String {
    format!("default{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
}

fn saved_session(ws: &Ws, name: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, &ws.id, name).expect("query").expect("row").last_session_id
}

/// What `stamp_principal` leaves for an agent's passport: `project` is its
/// own path. No live session behind it, so it is an agent.
fn call_as_agent(me: &Ws, extra: &[(&str, &str)]) -> (String, Value) {
    let mut p: HashMap<String, String> = [
        ("project", me.path.as_str()),
        ("project_id", me.id.as_str()),
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"),
        ("from", me.handle.as_str()),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in extra {
        p.insert(k.to_string(), v.to_string());
    }
    let r = crate::heartbeat_routes::with_request_actor(Some(format!("agent:{}", me.handle)), || {
        crate::misc_routes::dispatch(ROUTE, &p).expect("heartbeat restart is handled")
    });
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn call_as_owner(project: &Ws, extra: &[(&str, &str)]) -> (String, Value) {
    let mut p: HashMap<String, String> =
        [("project".to_string(), project.path.clone())].into_iter().collect();
    for (k, v) in extra {
        p.insert(k.to_string(), v.to_string());
    }
    let r = crate::heartbeat_routes::with_request_actor(Some("owner-token".into()), || {
        crate::misc_routes::dispatch(ROUTE, &p).expect("heartbeat restart is handled")
    });
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn last_change(ws: &Ws, name: &str) -> (Option<String>, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT actor, reason FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3 ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![ws.id, name, k2_core::heartbeats::DECISION_CHANGED],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .expect("a changed row")
}

#[test]
fn an_agent_restarts_its_own_heartbeat_with_no_switch() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let me = workspace(&home, "hbrme", &[t.as_str()]);
    assert!(
        !k2_core::workspace::settings::agents_can_manage_agents(&me.id),
        "the hire-and-manage switch is off for this test"
    );

    let (s, v) = call_as_agent(&me, &[("name", t.as_str())]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["success"], true, "{v}");
    assert_eq!(v["name"], t.as_str(), "{v}");
    assert_eq!(v["stopped"], false, "no session was running: {v}");
    assert_eq!(v["clearedSessionId"], "11111111-2222-3333-4444-555555555555", "{v}");
    assert_eq!(saved_session(&me, &t), None, "the saved conversation is cleared");
    let (actor, reason) = last_change(&me, &t);
    assert_eq!(actor.as_deref(), Some(format!("agent:{}", me.handle).as_str()), "History names the agent");
    let reason = reason.expect("a reason");
    assert!(reason.contains("session restarted"), "{reason}");

    // `hb:<name>`, the address `k2 sessions` prints, reads as the name.
    let hb_addr = format!("hb:{t}");
    let (s, v) = call_as_agent(&me, &[("name", hb_addr.as_str())]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["name"], t.as_str(), "{v}");
}

#[test]
fn an_agent_cannot_restart_another_workspaces_heartbeat() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let me = workspace(&home, "hbrown", &[t.as_str()]);
    let other = workspace(&home, "hbroth", &[t.as_str()]);

    for ws in [other.handle.as_str(), other.path.as_str()] {
        let (s, v) = call_as_agent(&me, &[("name", t.as_str()), ("workspace", ws)]);
        assert_eq!(s, "403 Forbidden", "{ws}: {v}");
        assert_eq!(v["error"]["code"], "other_workspace", "{ws}: {v}");
    }
    assert_eq!(
        saved_session(&other, &t).as_deref(),
        Some("11111111-2222-3333-4444-555555555555"),
        "the other workspace's heartbeat is untouched"
    );
    assert_eq!(
        saved_session(&me, &t).as_deref(),
        Some("11111111-2222-3333-4444-555555555555"),
        "a refusal changes nothing of the agent's own either"
    );

    // The owner may name another workspace.
    let (s, v) = call_as_owner(&me, &[("name", t.as_str()), ("workspace", other.handle.as_str())]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(saved_session(&other, &t), None, "the owner restarted the named workspace's");
    assert_eq!(
        saved_session(&me, &t).as_deref(),
        Some("11111111-2222-3333-4444-555555555555"),
        "and not the caller's own of the same name"
    );
}

#[test]
fn a_name_that_is_not_a_heartbeat_is_refused() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let me = workspace(&home, "hbrnam", &[t.as_str()]);

    let not_hb = [format!("{}/{t}", me.handle), "tab-1234".to_string() + ":x", "hb:".to_string(), "sidecar:reviewer".to_string()];
    for name in &not_hb {
        let (s, v) = call_as_agent(&me, &[("name", name.as_str())]);
        assert_eq!(s, "400 Bad Request", "{name}: {v}");
        let err = v["error"].as_str().unwrap_or_else(|| panic!("{name}: {v}"));
        assert!(err.starts_with("not_a_heartbeat"), "{name}: {err}");
    }
    // A plain name that isn't on the roster: 404, never a success.
    let (s, v) = call_as_agent(&me, &[("name", "reviewer")]);
    assert_eq!(s, "404 Not Found", "{v}");
    assert_eq!(v["error"], k2_core::heartbeats::NO_SUCH_HEARTBEAT, "{v}");
    assert_eq!(
        saved_session(&me, &t).as_deref(),
        Some("11111111-2222-3333-4444-555555555555"),
        "refusals change nothing"
    );
}

#[test]
fn a_chosen_session_is_kept() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let me = workspace(&home, "hbrcho", &[t.as_str()]);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::set_session(&conn, &me.id, &t, Some("99999999-2222-3333-4444-555555555555"), Some("claude"))
            .expect("set session");
    }
    let (s, v) = call_as_agent(&me, &[("name", t.as_str())]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v["keptChosenSession"], true, "{v}");
    assert_eq!(v["clearedSessionId"], Value::Null, "{v}");
    assert_eq!(saved_session(&me, &t).as_deref(), Some("99999999-2222-3333-4444-555555555555"));
}

/// The restart route is POST-only and a Member floor, like its siblings.
#[test]
fn restart_route_is_classified_post_only() {
    let row = crate::routes::route_policy::lookup(ROUTE).expect("classified");
    assert!(row.get.is_none(), "POST-only");
    assert_eq!(row.post.expect("POST floor").as_wire(), "member");
    assert!(crate::routes::route_policy::is_post_shaped_get_verb(ROUTE), "the cell socket admits it");
    assert!(!crate::heartbeat_app_routes::is_app_door(ROUTE), "never an app door");
}

// ── stale_step: what a fire does with a stale verdict ───────────────

fn stale(reason: crate::hb_stale::StaleReason) -> Option<crate::hb_stale::Stale> {
    Some(crate::hb_stale::Stale { reason, detail: "why".into() })
}

#[test]
fn stale_step_table() {
    use crate::hb_stale::StaleReason::*;
    // Healthy: wake as before.
    assert_eq!(stale_step("daily", false, None), StaleStep::Proceed);
    assert_eq!(stale_step("daily", true, None), StaleStep::Proceed);
    // The heartbeat's own session: every reason replaces it.
    for r in [TokenChanged, WeeklyLimit, UsageLimit, SignedOut] {
        assert!(matches!(stale_step("daily", false, stale(r)), StaleStep::Replace(s) if s.reason == r), "{r:?}");
    }
    // A chosen session: a token change alone wakes it as before ...
    assert_eq!(stale_step("daily", true, stale(TokenChanged)), StaleStep::Proceed);
    // ... a limit or sign-in error fails the fire loudly, never retargets.
    for r in [WeeklyLimit, UsageLimit, SignedOut] {
        match stale_step("daily", true, stale(r)) {
            StaleStep::Fail(msg) => {
                assert!(msg.starts_with("stuck: "), "{msg}");
                assert!(msg.contains("k2 heartbeat session daily"), "{msg}");
            }
            other => panic!("{r:?}: {other:?}"),
        }
    }
}
