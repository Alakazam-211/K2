//! The 0233 rows: the agent switch (`projects.agents_can_use_sim`),
//! `sim_sessions` (one per workspace) and `sim_audit`.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::lane::{DeviceProfile, LaneKind};
use super::stream::SimState;

/// "Allow mobile devices" for this workspace (default off).
pub fn agents_can_use_sim(conn: &Connection, project_id: &str) -> bool {
    conn.query_row("SELECT agents_can_use_sim FROM projects WHERE id = ?1", params![project_id], |r| r.get::<_, i64>(0))
        .map(|v| v != 0)
        .unwrap_or(false)
}

pub fn set_agents_can_use_sim(conn: &Connection, project_id: &str, on: bool) -> Result<(), String> {
    let n = conn
        .execute("UPDATE projects SET agents_can_use_sim = ?2 WHERE id = ?1", params![project_id, on as i64])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("workspace {project_id} not found"));
    }
    Ok(())
}

/// One `sim_sessions` row.
#[derive(Debug, Clone, PartialEq)]
pub struct SimRow {
    pub id: i64,
    pub project_id: String,
    pub lane: LaneKind,
    pub device_id: Option<String>,
    pub profile: Option<DeviceProfile>,
    pub lane_state: Value,
    pub state: SimState,
    pub reason: Option<String>,
    pub pid: Option<u32>,
    pub booted_at: Option<i64>,
    pub last_used_at: i64,
    pub last_run_at: Option<i64>,
    pub created_at: i64,
    pub created_by: String,
}

impl SimRow {
    /// `s<id>` on the wire.
    pub fn wire_id(&self) -> String {
        format!("s{}", self.id)
    }
}

const COLS: &str = "id, project_id, lane, device_id, profile_json, lane_state, state, reason, pid, booted_at, last_used_at, last_run_at, created_at, created_by";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SimRow> {
    let lane: String = r.get(2)?;
    let state: String = r.get(6)?;
    let profile: Option<String> = r.get(4)?;
    let lane_state: String = r.get(5)?;
    let pid: Option<i64> = r.get(8)?;
    Ok(SimRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        // The CHECK constraints keep these valid; a bad value is a loud
        // conversion error, never a silent default.
        lane: LaneKind::parse(&lane).ok_or_else(|| rusqlite::Error::InvalidColumnType(2, lane.clone(), rusqlite::types::Type::Text))?,
        device_id: r.get(3)?,
        profile: profile.as_deref().and_then(|p| serde_json::from_str(p).ok()),
        lane_state: serde_json::from_str(&lane_state).unwrap_or(Value::Object(Default::default())),
        state: SimState::parse(&state).ok_or_else(|| rusqlite::Error::InvalidColumnType(6, state.clone(), rusqlite::types::Type::Text))?,
        reason: r.get(7)?,
        pid: pid.and_then(|p| u32::try_from(p).ok()),
        booted_at: r.get(9)?,
        last_used_at: r.get(10)?,
        last_run_at: r.get(11)?,
        created_at: r.get(12)?,
        created_by: r.get(13)?,
    })
}

pub fn by_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<SimRow>> {
    conn.query_row(&format!("SELECT {COLS} FROM sim_sessions WHERE project_id = ?1"), params![project_id], row).optional()
}

pub fn by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<SimRow>> {
    conn.query_row(&format!("SELECT {COLS} FROM sim_sessions WHERE id = ?1"), params![id], row).optional()
}

pub fn all(conn: &Connection) -> rusqlite::Result<Vec<SimRow>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM sim_sessions ORDER BY id"))?;
    let rows = st.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Rows that hold a slot against the box limit (booting / running /
/// stopping), other than `except_project`.
pub fn count_occupying(conn: &Connection, except_project: Option<&str>) -> rusqlite::Result<u32> {
    conn.query_row(
        "SELECT COUNT(*) FROM sim_sessions WHERE state IN ('booting','running','stopping') AND project_id IS NOT ?1",
        params![except_project],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u32)
}

/// Start (or restart) the workspace's row as `booting` on `lane`. Keeps
/// the id when the row exists (so `s<n>` is stable per workspace).
pub fn begin_boot(conn: &Connection, project_id: &str, lane: LaneKind, now: i64, by: &str) -> rusqlite::Result<SimRow> {
    conn.execute(
        "INSERT INTO sim_sessions (project_id, lane, state, last_used_at, created_at, created_by)
         VALUES (?1, ?2, 'booting', ?3, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET lane = excluded.lane, state = 'booting', reason = NULL, last_used_at = excluded.last_used_at",
        params![project_id, lane.as_str(), now, by],
    )?;
    by_project(conn, project_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn set_running(
    conn: &Connection,
    id: i64,
    device_id: &str,
    profile: &DeviceProfile,
    lane_state: &Value,
    pid: Option<u32>,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sim_sessions SET state = 'running', reason = NULL, device_id = ?2, profile_json = ?3, lane_state = ?4,
         pid = ?5, booted_at = ?6, last_used_at = ?6, last_run_at = ?6 WHERE id = ?1",
        params![
            id,
            device_id,
            serde_json::to_string(profile).unwrap_or_default(),
            lane_state.to_string(),
            pid.map(|p| p as i64),
            now
        ],
    )?;
    Ok(())
}

pub fn set_state(conn: &Connection, id: i64, state: SimState, reason: Option<&str>, now: i64) -> rusqlite::Result<()> {
    let ran = if state.occupies_slot() { None } else { Some(now) };
    conn.execute(
        "UPDATE sim_sessions SET state = ?2, reason = ?3,
           pid = CASE WHEN ?2 IN ('stopped','failed') THEN NULL ELSE pid END,
           last_run_at = COALESCE(?4, last_run_at)
         WHERE id = ?1",
        params![id, state.as_str(), reason, ran],
    )?;
    Ok(())
}

pub fn set_lane_state(conn: &Connection, id: i64, lane_state: &Value, pid: Option<u32>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sim_sessions SET lane_state = ?2, pid = ?3 WHERE id = ?1",
        params![id, lane_state.to_string(), pid.map(|p| p as i64)],
    )?;
    Ok(())
}

pub fn touch(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE sim_sessions SET last_used_at = MAX(last_used_at, ?2) WHERE id = ?1", params![id, now])?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sim_sessions WHERE id = ?1", params![id])?;
    Ok(())
}

/// Append an audit row. Never put typed text, a secret or a token in
/// `detail`. A failed write is logged, never fatal.
pub fn audit(conn: &Connection, at: i64, actor: &str, project_id: Option<&str>, lane: Option<LaneKind>, action: &str, detail: Value) {
    if let Err(e) = conn.execute(
        "INSERT INTO sim_audit (at, actor, project_id, lane, action, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![at, actor, project_id, lane.map(|l| l.as_str()), action, detail.to_string()],
    ) {
        crate::log_debug!("[sim] WARN audit write failed ({action}): {e}");
    }
}

/// One audit row.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    pub at: i64,
    pub actor: String,
    pub project_id: Option<String>,
    pub lane: Option<String>,
    pub action: String,
    pub detail: Value,
}

pub fn recent_audit(conn: &Connection, project_id: Option<&str>, limit: u32) -> rusqlite::Result<Vec<AuditRow>> {
    let mut st = conn.prepare(
        "SELECT at, actor, project_id, lane, action, detail FROM sim_audit
         WHERE (?1 IS NULL OR project_id = ?1) ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = st
        .query_map(params![project_id, limit], |r| {
            let d: String = r.get(5)?;
            Ok(AuditRow {
                at: r.get(0)?,
                actor: r.get(1)?,
                project_id: r.get(2)?,
                lane: r.get(3)?,
                action: r.get(4)?,
                detail: serde_json::from_str(&d).unwrap_or(Value::Null),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("fk");
        crate::db::run_migrations(&conn).expect("migrations");
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('p1', 'one', '/tmp/p1'), ('p2', 'two', '/tmp/p2')", [])
            .expect("projects");
        conn
    }

    fn profile() -> DeviceProfile {
        DeviceProfile { id: "pixel_9".into(), name: "Pixel 9".into(), width_pt: 412, height_pt: 923, scale: 2.625, os: "android-35".into(), default: true }
    }

    #[test]
    fn switch_defaults_off_and_toggles() {
        let conn = db();
        assert!(!agents_can_use_sim(&conn, "p1"), "off by default (Rosson 2026-10-27)");
        set_agents_can_use_sim(&conn, "p1", true).expect("set");
        assert!(agents_can_use_sim(&conn, "p1"));
        assert!(!agents_can_use_sim(&conn, "p2"));
        assert!(set_agents_can_use_sim(&conn, "nope", true).is_err());
    }

    #[test]
    fn one_row_per_workspace_with_a_stable_id_and_slot_counting() {
        let conn = db();
        let r = begin_boot(&conn, "p1", LaneKind::Android, 100, "owner").expect("boot");
        assert_eq!(r.state, SimState::Booting);
        assert_eq!(count_occupying(&conn, None).expect("count"), 1);
        assert_eq!(count_occupying(&conn, Some("p1")).expect("count"), 0);
        set_running(&conn, r.id, "k2-abc@5554", &profile(), &serde_json::json!({"grpcPort": 8554}), Some(42), 110).expect("run");
        let got = by_project(&conn, "p1").expect("q").expect("row");
        assert_eq!(got.state, SimState::Running);
        assert_eq!(got.profile.expect("profile").name, "Pixel 9");
        assert_eq!(got.pid, Some(42));
        set_state(&conn, r.id, SimState::Stopped, Some("idle"), 200).expect("stop");
        let got = by_project(&conn, "p1").expect("q").expect("row");
        assert_eq!((got.state, got.reason.as_deref(), got.pid, got.last_run_at), (SimState::Stopped, Some("idle"), None, Some(200)));
        assert_eq!(count_occupying(&conn, None).expect("count"), 0);
        let again = begin_boot(&conn, "p1", LaneKind::Android, 300, "owner").expect("reboot");
        assert_eq!(again.id, r.id, "the id is stable per workspace");
        assert_eq!(again.wire_id(), format!("s{}", r.id));
        conn.execute("DELETE FROM projects WHERE id = 'p1'", []).expect("delete project");
        assert!(by_project(&conn, "p1").expect("q").is_none(), "rows go with their workspace");
    }

    #[test]
    fn audit_rows_round_trip() {
        let conn = db();
        audit(&conn, 5, "owner", Some("p1"), Some(LaneKind::Android), "boot", serde_json::json!({"profile": "pixel_9"}));
        audit(&conn, 6, "agent:one", Some("p2"), None, "install", serde_json::json!({"app": "com.acme"}));
        let all = recent_audit(&conn, None, 10).expect("audit");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].action, "install");
        let p1 = recent_audit(&conn, Some("p1"), 10).expect("audit");
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].lane.as_deref(), Some("android"));
    }
}
