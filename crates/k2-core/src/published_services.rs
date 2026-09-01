//! Durable published-service rows (`published_services`, migration 0104).
//!
//! A published service is a daemon-owned program for one workspace:
//! a shell command, a cwd, a port, a desired on/off, and an exposure
//! (`local` = `--no-tunnel`, `tunnel` = default nested hostname). Pid
//! is runtime only — `desired` is the SSOT of "should be up".
//!
//! Wire JSON (`GET /cli/publish/list`) is camelCase and never omits
//! keys (`skip_serializing_if` is banned here — empty/null still
//! serializes). Status is computed by the daemon supervisor, not stored.

use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// Internal exposure column. CLI has no `--expose`.
pub const EXPOSE_LOCAL: &str = "local";
pub const EXPOSE_TUNNEL: &str = "tunnel";

pub const DESIRED_RUNNING: &str = "running";
pub const DESIRED_STOPPED: &str = "stopped";

/// `published_services.kind`. Existing rows (pre-0114) are `cmd`.
pub const KIND_CMD: &str = "cmd";
/// Official Dannon skin gateway helper (not a user shell).
pub const KIND_SKIN: &str = "skin";
/// Display sentinel stored in `cmd` for `kind=skin`. Never a shell string.
pub const CMD_SKIN_SENTINEL: &str = "(skin)";

/// Computed runtime status (not a SQLite column).
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_STARTING: &str = "starting";
pub const STATUS_EXITED: &str = "exited";
pub const STATUS_STOPPED: &str = "stopped";
pub const STATUS_UNHEALTHY: &str = "unhealthy";

/// Nested-label charset (control-plane `bad_label`): lowercase letters,
/// digits, and dashes. Used for both tunnel labels and local-only names
/// so a later exposure flip stays valid.
pub fn is_valid_name(name: &str) -> bool {
    let s = name.trim();
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn normalize_name(name: &str) -> Result<String, String> {
    let s = name.trim().to_ascii_lowercase();
    if !is_valid_name(&s) {
        return Err(
            "bad_label: Invalid label (use lowercase letters, digits, and dashes).".into(),
        );
    }
    if crate::skin::is_reserved_nested_label(&s) {
        return Err(crate::skin::reserved_nested_label_error(&s));
    }
    Ok(s)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Durable row. `status` / `url` / `target` are computed at list time.
#[derive(Debug, Clone)]
pub struct PublishedService {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: String,
    pub port: i64,
    pub expose: String,
    pub desired: String,
    pub pid: Option<i64>,
    pub last_exit_code: Option<i64>,
    pub last_started_at: Option<i64>,
    pub last_exited_at: Option<i64>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// `cmd` or `skin`. Always present after 0114.
    pub kind: String,
    /// Workspace-relative UI dir. Empty = bundled login + Thread chrome.
    pub skin_root: String,
}

/// Frozen HTTP Service object (`GET /cli/publish/list`).
/// Every field is always present; nulls serialize as JSON null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceJson {
    pub name: String,
    pub cmd: String,
    pub cwd: String,
    pub port: u16,
    pub expose: String,
    pub desired: String,
    pub status: String,
    pub pid: Option<i64>,
    pub url: Option<String>,
    pub target: String,
    pub error: Option<String>,
    pub last_exit_code: Option<i32>,
    /// Always present (`cmd` or `skin`). Existing rows serialize as `cmd`.
    pub kind: String,
}

impl ServiceJson {
    pub fn from_row(
        row: &PublishedService,
        status: &str,
        url: Option<String>,
        pid: Option<i64>,
    ) -> Self {
        let port = if row.port > 0 && row.port <= u16::MAX as i64 {
            row.port as u16
        } else {
            0
        };
        Self {
            name: row.name.clone(),
            cmd: row.cmd.clone(),
            cwd: row.cwd.clone(),
            port,
            expose: row.expose.clone(),
            desired: row.desired.clone(),
            status: status.to_string(),
            pid,
            url,
            target: format!("127.0.0.1:{port}"),
            error: row.error.clone(),
            last_exit_code: row.last_exit_code.map(|c| c as i32),
            kind: if row.kind.trim().is_empty() {
                KIND_CMD.to_string()
            } else {
                row.kind.clone()
            },
        }
    }
}

const COLS: &str = "id, project_id, name, cmd, cwd, port, expose, desired, pid, \
     last_exit_code, last_started_at, last_exited_at, error, created_at, updated_at, \
     kind, skin_root";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedService> {
    Ok(PublishedService {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        cmd: row.get(3)?,
        cwd: row.get(4)?,
        port: row.get(5)?,
        expose: row.get(6)?,
        desired: row.get(7)?,
        pid: row.get(8)?,
        last_exit_code: row.get(9)?,
        last_started_at: row.get(10)?,
        last_exited_at: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        kind: row.get::<_, String>(15).unwrap_or_else(|_| KIND_CMD.to_string()),
        skin_root: row.get::<_, String>(16).unwrap_or_default(),
    })
}

/// Insert a new service. `name` is stored lowercase. UNIQUE(project_id, name)
/// is enforced by SQLite — callers map the constraint error.
pub fn insert(
    conn: &Connection,
    project_id: &str,
    name: &str,
    cmd: &str,
    cwd: &str,
    port: u16,
    expose: &str,
    desired: &str,
    kind: &str,
    skin_root: &str,
) -> Result<PublishedService> {
    let name = name.trim().to_ascii_lowercase();
    let kind = if kind.trim().is_empty() {
        KIND_CMD
    } else {
        kind.trim()
    };
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_unix();
    conn.execute(
        "INSERT INTO published_services \
         (id, project_id, name, cmd, cwd, port, expose, desired, created_at, updated_at, kind, skin_root) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11)",
        params![
            id,
            project_id,
            name,
            cmd,
            cwd,
            port as i64,
            expose,
            desired,
            now,
            kind,
            skin_root,
        ],
    )?;
    get_by_id(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<PublishedService>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM published_services WHERE id = ?1"),
        params![id],
        map_row,
    )
    .optional()
}

pub fn get_by_project_name(
    conn: &Connection,
    project_id: &str,
    name: &str,
) -> Result<Option<PublishedService>> {
    let name = name.trim().to_ascii_lowercase();
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM published_services WHERE project_id = ?1 AND name = ?2"
        ),
        params![project_id, name],
        map_row,
    )
    .optional()
}

pub fn list_for_project(conn: &Connection, project_id: &str) -> Result<Vec<PublishedService>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM published_services WHERE project_id = ?1 ORDER BY name"
    ))?;
    let rows = stmt.query_map(params![project_id], map_row)?;
    rows.collect()
}

/// Every row with `desired = running` — boot reattach/respawn.
pub fn list_desired_running(conn: &Connection) -> Result<Vec<PublishedService>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM published_services WHERE desired = 'running' ORDER BY created_at, name"
    ))?;
    let rows = stmt.query_map([], map_row)?;
    rows.collect()
}

pub fn update_spec(
    conn: &Connection,
    project_id: &str,
    name: &str,
    cmd: &str,
    cwd: &str,
    port: u16,
    expose: &str,
    kind: &str,
    skin_root: &str,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let kind = if kind.trim().is_empty() {
        KIND_CMD
    } else {
        kind.trim()
    };
    let n = conn.execute(
        "UPDATE published_services SET cmd = ?1, cwd = ?2, port = ?3, expose = ?4, \
         kind = ?5, skin_root = ?6, updated_at = ?7 WHERE project_id = ?8 AND name = ?9",
        params![
            cmd,
            cwd,
            port as i64,
            expose,
            kind,
            skin_root,
            now_unix(),
            project_id,
            name
        ],
    )?;
    Ok(n > 0)
}

pub fn set_desired(
    conn: &Connection,
    project_id: &str,
    name: &str,
    desired: &str,
) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "UPDATE published_services SET desired = ?1, updated_at = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        params![desired, now_unix(), project_id, name],
    )?;
    Ok(n > 0)
}

pub fn set_runtime(
    conn: &Connection,
    project_id: &str,
    name: &str,
    pid: Option<i64>,
    error: Option<&str>,
    last_started_at: Option<i64>,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE published_services SET pid = ?1, error = ?2, last_started_at = COALESCE(?3, last_started_at), \
         updated_at = ?4 WHERE project_id = ?5 AND name = ?6",
        params![pid, error, last_started_at, now_unix(), project_id, name],
    )?;
    Ok(())
}

/// Child exited: clear pid, stamp exit code + time. Does NOT change `desired`.
pub fn mark_exited(
    conn: &Connection,
    project_id: &str,
    name: &str,
    exit_code: Option<i64>,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    let now = now_unix();
    conn.execute(
        "UPDATE published_services SET pid = NULL, last_exit_code = ?1, last_exited_at = ?2, \
         updated_at = ?2 WHERE project_id = ?3 AND name = ?4",
        params![exit_code, now, project_id, name],
    )?;
    Ok(())
}

/// Hostname-step fail: stop-persist so boot cannot resurrect as local-only.
pub fn mark_hostname_failed(
    conn: &Connection,
    project_id: &str,
    name: &str,
    error: &str,
) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    let now = now_unix();
    conn.execute(
        "UPDATE published_services SET desired = 'stopped', pid = NULL, error = ?1, \
         last_exited_at = ?2, updated_at = ?2 WHERE project_id = ?3 AND name = ?4",
        params![error, now, project_id, name],
    )?;
    Ok(())
}

pub fn set_error(conn: &Connection, project_id: &str, name: &str, error: Option<&str>) -> Result<()> {
    let name = name.trim().to_ascii_lowercase();
    conn.execute(
        "UPDATE published_services SET error = ?1, updated_at = ?2 WHERE project_id = ?3 AND name = ?4",
        params![error, now_unix(), project_id, name],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, project_id: &str, name: &str) -> Result<bool> {
    let name = name.trim().to_ascii_lowercase();
    let n = conn.execute(
        "DELETE FROM published_services WHERE project_id = ?1 AND name = ?2",
        params![project_id, name],
    )?;
    Ok(n > 0)
}

pub fn is_unique_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(e, _) => {
            e.code == rusqlite::ErrorCode::ConstraintViolation
                || e.extended_code == 2067 // SQLITE_CONSTRAINT_UNIQUE
                || e.extended_code == 1555 // SQLITE_CONSTRAINT_PRIMARYKEY
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn project(conn: &Connection, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "pub-test", path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn migration_applies_and_roundtrips() {
        let conn = fresh();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='published_services'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "0104 must create published_services");

        let pid = project(&conn, "/tmp/pub-rt");
        let row = insert(
            &conn,
            &pid,
            "Web",
            "npm start",
            "/tmp/pub-rt",
            3000,
            EXPOSE_TUNNEL,
            DESIRED_STOPPED,
            KIND_CMD,
            "",
        )
        .expect("insert");
        assert_eq!(row.name, "web", "name is stored lowercase");
        assert_eq!(row.port, 3000);
        assert_eq!(row.expose, EXPOSE_TUNNEL);
        assert_eq!(row.desired, DESIRED_STOPPED);
        assert_eq!(row.kind, KIND_CMD);
        assert_eq!(row.skin_root, "");
        assert!(row.pid.is_none());
        assert!(row.error.is_none());

        let listed = list_for_project(&conn, &pid).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].cmd, "npm start");
        assert_eq!(listed[0].cwd, "/tmp/pub-rt");
    }

    #[test]
    fn unique_project_name_is_enforced() {
        let conn = fresh();
        let a = project(&conn, "/tmp/pub-a");
        let b = project(&conn, "/tmp/pub-b");
        insert(&conn, &a, "web", "a", "/a", 1, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "").unwrap();
        let dup = insert(&conn, &a, "web", "a2", "/a", 2, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "");
        assert!(dup.is_err(), "same project+name must UNIQUE-fail");
        assert!(is_unique_violation(dup.as_ref().unwrap_err()));
        // Same name on another workspace is fine (local-only may collide).
        insert(&conn, &b, "web", "b", "/b", 3, EXPOSE_LOCAL, DESIRED_STOPPED, KIND_CMD, "")
            .expect("cross-workspace name collision is allowed");
        assert_eq!(list_for_project(&conn, &a).unwrap().len(), 1);
        assert_eq!(list_for_project(&conn, &b).unwrap().len(), 1);
    }

    #[test]
    fn list_does_not_hide_fields() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-json");
        let row = insert(
            &conn,
            &pid,
            "api",
            "python -m http.server",
            "/tmp/pub-json",
            8090,
            EXPOSE_LOCAL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        set_runtime(&conn, &pid, "api", Some(4242), None, Some(now_unix())).unwrap();
        let row = get_by_id(&conn, &row.id).unwrap().unwrap();
        let json = ServiceJson::from_row(&row, STATUS_RUNNING, None, Some(4242));
        let v = serde_json::to_value(&json).unwrap();
        for key in [
            "name",
            "cmd",
            "cwd",
            "port",
            "expose",
            "desired",
            "status",
            "pid",
            "url",
            "target",
            "error",
            "lastExitCode",
            "kind",
        ] {
            assert!(v.get(key).is_some(), "Service JSON must carry {key}");
        }
        assert_eq!(v["kind"], KIND_CMD);
        assert!(v["url"].is_null(), "local-only url is explicit null");
        assert!(v["error"].is_null());
        assert!(v["lastExitCode"].is_null());
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["target"], "127.0.0.1:8090");
        assert_eq!(v["expose"], "local");
        assert_eq!(v["desired"], "running");
        assert_eq!(v["status"], "running");
    }

    #[test]
    fn mark_exited_clears_pid_keeps_desired() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-exit");
        insert(
            &conn,
            &pid,
            "worker",
            "sleep 1",
            "/tmp/pub-exit",
            9,
            EXPOSE_LOCAL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        set_runtime(&conn, &pid, "worker", Some(99), None, Some(now_unix())).unwrap();
        mark_exited(&conn, &pid, "worker", Some(1)).unwrap();
        let row = get_by_project_name(&conn, &pid, "worker").unwrap().unwrap();
        assert!(row.pid.is_none());
        assert_eq!(row.last_exit_code, Some(1));
        assert_eq!(row.desired, DESIRED_RUNNING, "P4: desired stays running");
        assert!(row.last_exited_at.is_some());
    }

    #[test]
    fn hostname_fail_does_not_leave_desired_running() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-hostfail");
        insert(
            &conn,
            &pid,
            "web",
            "npm start",
            "/tmp/pub-hostfail",
            3000,
            EXPOSE_TUNNEL,
            DESIRED_RUNNING,
            KIND_CMD,
            "",
        )
        .unwrap();
        mark_hostname_failed(&conn, &pid, "web", "claim did not land").unwrap();
        let row = get_by_project_name(&conn, &pid, "web").unwrap().unwrap();
        assert_eq!(row.desired, DESIRED_STOPPED);
        assert_eq!(row.expose, EXPOSE_TUNNEL, "expose stays tunnel");
        assert!(row.pid.is_none());
        assert_eq!(row.error.as_deref(), Some("claim did not land"));
        let boot = list_desired_running(&conn).unwrap();
        assert!(
            !boot.iter().any(|r| r.project_id == pid && r.name == "web"),
            "boot must not resurrect a hostname-failed row"
        );
    }

    #[test]
    fn name_charset_matches_bad_label() {
        assert!(normalize_name("Web").unwrap() == "web");
        assert!(normalize_name("a-b-1").is_ok());
        assert!(normalize_name("").is_err());
        assert!(normalize_name("Has_Underscore").is_err());
        assert!(normalize_name("ok!").is_err());
    }

    #[test]
    fn reserved_nested_label_skin_is_400_loud() {
        let err = normalize_name("skin").expect_err("skin is reserved");
        assert!(
            err.contains("reserved_label"),
            "must fail loud with reserved_label, got {err}"
        );
        assert!(
            err.contains("Pick another nested label"),
            "must tell them to pick another name, not a Caddy port: {err}"
        );
        assert!(
            !err.contains("38472"),
            "must not teach 38472 as a publish target: {err}"
        );
        assert!(normalize_name("Skin").is_err());
        assert!(normalize_name("staging").is_ok());
    }

    #[test]
    fn kind_defaults_cmd_and_skin_roundtrips() {
        let conn = fresh();
        let pid = project(&conn, "/tmp/pub-kind");
        let cmd_row = insert(
            &conn,
            &pid,
            "web",
            "npm start",
            "/tmp/pub-kind",
            3000,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            KIND_CMD,
            "",
        )
        .unwrap();
        assert_eq!(cmd_row.kind, KIND_CMD);
        assert_eq!(cmd_row.skin_root, "");
        let json = ServiceJson::from_row(&cmd_row, STATUS_STOPPED, None, None);
        let v = serde_json::to_value(&json).unwrap();
        assert_eq!(v["kind"], "cmd", "kind is always present camelCase");

        let skin_row = insert(
            &conn,
            &pid,
            "agents",
            CMD_SKIN_SENTINEL,
            "/tmp/pub-kind",
            8788,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            KIND_SKIN,
            "ui",
        )
        .unwrap();
        assert_eq!(skin_row.kind, KIND_SKIN);
        assert_eq!(skin_row.cmd, CMD_SKIN_SENTINEL);
        assert_eq!(skin_row.skin_root, "ui");
        let json = ServiceJson::from_row(&skin_row, STATUS_STOPPED, None, None);
        let v = serde_json::to_value(&json).unwrap();
        assert_eq!(v["kind"], "skin");
        assert_eq!(v["cmd"], CMD_SKIN_SENTINEL);

        let bad = insert(
            &conn,
            &pid,
            "nope",
            "x",
            "/tmp/pub-kind",
            1,
            EXPOSE_LOCAL,
            DESIRED_STOPPED,
            "python",
            "",
        );
        assert!(bad.is_err(), "CHECK (kind IN cmd|skin) must reject other kinds");
    }
}
