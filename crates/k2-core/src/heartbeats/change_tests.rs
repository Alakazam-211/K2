//! App heartbeats surface (prd-app-heartbeats-surface-v1) — core pieces:
//! AH13 not-found errors, the AH12 instructions writer, AH18 change rows
//! with an actor, and `wait::open_episode` ignoring `changed`.

use super::*;
use crate::db::schema::AgentHeartbeat;

const WAKEUP_REL: &str = ".k2/heartbeats/hb/WAKEUP.md";

fn seed(label: &str) -> (String, String) {
    crate::db::init_for_tests();
    let dir = std::env::temp_dir().join(format!(
        "k2-hb-change-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let wakeup = dir.join(WAKEUP_REL);
    std::fs::create_dir_all(wakeup.parent().expect("wakeup parent")).expect("mkdir");
    std::fs::write(&wakeup, "---\ndescription: keep me\n---\n\nOld body.\n").expect("write");
    let project_path = dir.to_string_lossy().into_owned();
    let project_id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, 'hb-change', ?2)",
        rusqlite::params![project_id, project_path],
    )
    .expect("insert project");
    AgentHeartbeat::insert(
        &conn,
        &uuid::Uuid::new_v4().to_string(),
        &project_id,
        "hb",
        "hourly",
        r#"{"every_seconds":900}"#,
        WAKEUP_REL,
        true,
    )
    .expect("insert heartbeat");
    (project_path, project_id)
}

#[test]
fn missing_names_are_no_such_heartbeat_never_success_ah13() {
    let (path, _id) = seed("missing");
    for err in [
        k2so_heartbeat_set_enabled(path.clone(), "ghost".into(), false).unwrap_err(),
        k2so_heartbeat_edit(path.clone(), "ghost".into(), "daily".into(), r#"{"time":"07:00"}"#.into())
            .unwrap_err(),
        k2so_heartbeat_archive(path.clone(), "ghost".into()).unwrap_err(),
        k2so_heartbeat_unarchive(path.clone(), "ghost".into()).unwrap_err(),
        k2so_heartbeat_rename(path.clone(), "ghost".into(), "spirit".into()).unwrap_err(),
        k2so_heartbeat_remove(path.clone(), "ghost".into()).unwrap_err(),
        k2so_heartbeat_set_instructions(path.clone(), "ghost".into(), "x".into()).unwrap_err(),
    ] {
        assert!(is_no_such_heartbeat(&err), "expected no_such_heartbeat, got {err}");
    }
    // Re-archiving an archived row stays an idempotent no-op.
    k2so_heartbeat_archive(path.clone(), "hb".into()).expect("archive");
    k2so_heartbeat_archive(path.clone(), "hb".into()).expect("re-archive is a no-op");
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn set_instructions_keeps_frontmatter_refuses_blank_ah12() {
    let (path, _id) = seed("instr");
    let err = k2so_heartbeat_set_instructions(path.clone(), "hb".into(), "  \n ".into())
        .unwrap_err();
    assert!(err.starts_with("instructions_required"), "{err}");
    k2so_heartbeat_set_instructions(path.clone(), "hb".into(), "Sweep the inbox.".into())
        .expect("set instructions");
    let raw = std::fs::read_to_string(std::path::Path::new(&path).join(WAKEUP_REL)).expect("read");
    assert_eq!(raw, "---\ndescription: keep me\n---\n\nSweep the inbox.\n");
    let (body, rel) = k2so_heartbeat_instructions(&path, "hb").expect("read back");
    assert_eq!(body, "Sweep the inbox.");
    assert_eq!(rel, WAKEUP_REL);
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn record_change_writes_actor_and_open_episode_ignores_it_ah18() {
    let (path, id) = seed("actor");
    record_change(&path, "hb", "disabled", Some("app:bob"));
    let rows = k2so_heartbeat_fires_list(path.clone(), Some(10)).expect("fires");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].decision, DECISION_CHANGED);
    assert_eq!(rows[0].reason.as_deref(), Some("disabled"));
    assert_eq!(rows[0].actor.as_deref(), Some("app:bob"));
    assert_eq!(rows[0].schedule_name.as_deref(), Some("hb"));
    {
        let db = crate::db::shared();
        let conn = db.lock();
        let episode = wait::open_episode(&conn, &id, "hb", None);
        assert!(episode.is_none(), "changed rows must not open a wait episode");
    }
    assert_eq!(actor_phrase(Some("app:bob")), "app user bob");
    assert_eq!(actor_phrase(Some("user:rosson")), "user rosson");
    assert_eq!(actor_phrase(Some("agent:sales")), "agent sales");
    assert_eq!(actor_phrase(Some("app-token:kiosk")), "app token kiosk");
    assert_eq!(actor_phrase(Some("owner-token")), "the owner");
    assert_eq!(actor_phrase(None), "the scheduler");
    let _ = std::fs::remove_dir_all(&path);
}
