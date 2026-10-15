//! The singleton `sew_server` row (migration 0166). No row = not installed.

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub status: String,
    pub pinned_version: String,
    pub installed_version: Option<String>,
    pub previous_version: Option<String>,
    pub install_id: Option<String>,
    pub master_key_ref: Option<String>,
    pub master_key_created_at: Option<i64>,
    pub master_next_ref: Option<String>,
    pub master_prev_ref: Option<String>,
    pub recovery_shown_at: Option<i64>,
    pub host: Option<String>,
    pub host_mode: Option<String>,
    pub app_url: Option<String>,
    pub mirror_kind: String,
    pub mirror_path: Option<String>,
    pub ship_kind: String,
    pub ship_path: Option<String>,
    pub enable_progress: Option<Value>,
    pub upgrade_progress: Option<Value>,
    pub rotate_progress: Option<Value>,
    pub restore_progress: Option<Value>,
    pub doctor: Option<Value>,
    pub doctor_at: Option<i64>,
    pub last_error: Option<String>,
    pub installed_at: Option<i64>,
    pub updated_at: i64,
}

impl Row {
    pub fn new(pinned: &str, now: i64) -> Self {
        Row {
            status: "installing".into(),
            pinned_version: pinned.into(),
            mirror_kind: "none".into(),
            ship_kind: "none".into(),
            updated_at: now,
            ..Default::default()
        }
    }
}

const COLS: &str = "status, pinned_version, installed_version, previous_version, install_id, master_key_ref, \
    master_key_created_at, master_next_ref, master_prev_ref, recovery_shown_at, host, host_mode, app_url, \
    mirror_kind, mirror_path, ship_kind, ship_path, enable_progress_json, upgrade_progress_json, \
    rotate_progress_json, restore_progress_json, doctor_json, doctor_at, last_error, installed_at, updated_at";

fn json_col(s: Option<String>) -> Option<Value> {
    s.and_then(|t| serde_json::from_str(&t).ok())
}

fn text(v: &Option<Value>) -> Option<String> {
    v.as_ref().map(Value::to_string)
}

pub fn load() -> Option<Row> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(&format!("SELECT {COLS} FROM sew_server WHERE id = 1"), [], |r| {
        Ok(Row {
            status: r.get(0)?,
            pinned_version: r.get(1)?,
            installed_version: r.get(2)?,
            previous_version: r.get(3)?,
            install_id: r.get(4)?,
            master_key_ref: r.get(5)?,
            master_key_created_at: r.get(6)?,
            master_next_ref: r.get(7)?,
            master_prev_ref: r.get(8)?,
            recovery_shown_at: r.get(9)?,
            host: r.get(10)?,
            host_mode: r.get(11)?,
            app_url: r.get(12)?,
            mirror_kind: r.get(13)?,
            mirror_path: r.get(14)?,
            ship_kind: r.get(15)?,
            ship_path: r.get(16)?,
            enable_progress: json_col(r.get(17)?),
            upgrade_progress: json_col(r.get(18)?),
            rotate_progress: json_col(r.get(19)?),
            restore_progress: json_col(r.get(20)?),
            doctor: json_col(r.get(21)?),
            doctor_at: r.get(22)?,
            last_error: r.get(23)?,
            installed_at: r.get(24)?,
            updated_at: r.get(25)?,
        })
    })
    .optional()
    .unwrap_or_else(|e| {
        k2_core::log_debug!("[sew] read sew_server: {e}");
        None
    })
}

/// Insert or replace the one row.
pub fn save(row: &Row) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO sew_server (id, {COLS}) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
             ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)"
        ),
        params![
            row.status,
            row.pinned_version,
            row.installed_version,
            row.previous_version,
            row.install_id,
            row.master_key_ref,
            row.master_key_created_at,
            row.master_next_ref,
            row.master_prev_ref,
            row.recovery_shown_at,
            row.host,
            row.host_mode,
            row.app_url,
            row.mirror_kind,
            row.mirror_path,
            row.ship_kind,
            row.ship_path,
            text(&row.enable_progress),
            text(&row.upgrade_progress),
            text(&row.rotate_progress),
            text(&row.restore_progress),
            text(&row.doctor),
            row.doctor_at,
            row.last_error,
            row.installed_at,
            row.updated_at,
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("sew_server: {e}"))
}

/// Load, change, save. `None` when there is no row.
pub fn update(now: i64, f: impl FnOnce(&mut Row)) -> Result<Option<Row>, String> {
    let Some(mut row) = load() else { return Ok(None) };
    f(&mut row);
    row.updated_at = now;
    save(&row)?;
    Ok(Some(row))
}

pub fn delete() -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM sew_server WHERE id = 1", []).map(|_| ()).map_err(|e| format!("sew_server: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_row_round_trips_and_the_check_constraints_hold() {
        let _db = k2_core::db::scoped_for_test();
        assert!(load().is_none(), "no row = not installed");
        let mut r = Row::new("0.0.2", 10);
        r.enable_progress = Some(serde_json::json!({"steps": {"master-key": {"at": 10}}, "current": "stage"}));
        r.app_url = Some("https://hearth.box.k2.dev".into());
        save(&r).unwrap();
        assert_eq!(load().unwrap(), r);
        let r2 = update(20, |r| r.status = "running".into()).unwrap().unwrap();
        assert_eq!(r2.updated_at, 20);
        assert_eq!(load().unwrap().status, "running");
        // CHECKs: unknown status, mirror kind `k2`, a second row.
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(conn.execute("UPDATE sew_server SET status = 'bogus' WHERE id = 1", []).is_err());
        assert!(conn.execute("UPDATE sew_server SET mirror_kind = 'k2' WHERE id = 1", []).is_err());
        assert!(conn
            .execute("INSERT INTO sew_server (id, status, pinned_version, updated_at) VALUES (2, 'running', 'x', 1)", [])
            .is_err());
        drop(conn);
        delete().unwrap();
        assert!(load().is_none());
    }

    #[test]
    fn the_manage_sew_switch_defaults_off_and_round_trips() {
        let _db = k2_core::db::scoped_for_test();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-sew', 'Ops', '/t/ops-sew', 'ops-sew')", [])
                .unwrap();
        }
        assert!(!k2_core::workspace::settings::agents_can_manage_chat("w-sew"), "off by default");
        k2_core::workspace::settings::set_agents_can_manage_chat("w-sew", true).unwrap();
        assert!(k2_core::workspace::settings::agents_can_manage_chat("w-sew"));
        assert!(!k2_core::workspace::settings::agents_can_manage_chat("w-missing"), "fail closed");
        assert!(k2_core::workspace::settings::set_agents_can_manage_chat("w-missing", true).is_err());
        let db = k2_core::db::shared();
        let conn = db.lock();
        let p = k2_core::db::schema::Project::get(&conn, "w-sew").unwrap();
        assert_eq!(p.agents_can_manage_chat, 1);
    }
}
