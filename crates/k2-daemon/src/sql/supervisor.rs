//! Postgres sidecar supervisor. The ONLY module that knows postgres
//! exists as a process. Bake-first: enable never downloads or apt-gets.

use std::sync::atomic::{AtomicBool, Ordering};

use super::sysops::{is_baked, RealSystemOps, SystemOps, PG_UNIT, PG_UNIT_PATHS, PSQL_PATH};
use super::sql_supported; // used by spawn_health_loop

#[allow(dead_code)]
pub const ENABLE_STEPS: &[&str] = &["baked", "start", "catalog"];

pub fn enable_running() -> &'static AtomicBool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    &RUNNING
}

pub fn try_begin_enable() -> bool {
    enable_running()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn end_enable() {
    enable_running().store(false, Ordering::SeqCst);
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_field(col: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        &format!("SELECT {col} FROM sql_server WHERE id = 1"),
        [],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn set_row_field(col: &str, value: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        &format!("UPDATE sql_server SET {col} = ?1, updated_at = ?2 WHERE id = 1"),
        rusqlite::params![value, now_secs()],
    );
}

pub(crate) fn current_status() -> Option<String> {
    row_field("status")
}

fn set_status(status: &str) {
    set_row_field("status", status);
}

fn set_last_error(err: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE sql_server SET last_error = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![err, now_secs()],
    );
}

fn progress_load() -> serde_json::Value {
    row_field("enable_progress_json")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({ "steps": {} }))
}

fn progress_save(v: &serde_json::Value) {
    set_row_field("enable_progress_json", &v.to_string());
}

fn mark_step(step: &str) {
    let mut p = progress_load();
    p["steps"][step] = serde_json::json!({ "at": now_secs() });
    p["current"] = serde_json::Value::Null;
    progress_save(&p);
}

fn set_current(step: &str) {
    let mut p = progress_load();
    p["current"] = serde_json::json!(step);
    progress_save(&p);
}

fn ensure_installing_row() -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO sql_server (id, status, listen, updated_at) \
         VALUES (1, 'installing', 'localhost', ?1) \
         ON CONFLICT(id) DO UPDATE SET status = 'installing', last_error = NULL, updated_at = ?1",
        rusqlite::params![now_secs()],
    )
    .map_err(|e| format!("sql_server upsert: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnableError {
    #[allow(dead_code)]
    Unsupported,
    NotBaked(String),
    InProgress,
    Engine(String),
}

impl EnableError {
    pub fn status_code(&self) -> &'static str {
        match self {
            Self::Unsupported | Self::NotBaked(_) | Self::InProgress => "409 Conflict",
            Self::Engine(_) => "502 Bad Gateway",
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NotBaked(_) => "not_baked",
            Self::InProgress => "enable_in_progress",
            Self::Engine(_) => "engine",
        }
    }
    pub fn hint(&self) -> String {
        match self {
            Self::Unsupported => {
                "the SQL sidecar only works on Linux deployments; this daemon is not Linux"
                    .into()
            }
            Self::NotBaked(h) => h.clone(),
            Self::InProgress => {
                "an enable run is already in progress — poll /cli/db/status".into()
            }
            Self::Engine(h) => h.clone(),
        }
    }
}

const NOT_BAKED_HINT: &str = "Postgres sidecar is not baked on this box. Re-run \
provision-k2-server.sh --bake --with-db (or K2_BAKE_DB=1). The daemon does not apt-get.";

/// Catalog ingest + helper start if the unit is baked. Never apt-get.
pub fn enable_with(ops: &dyn SystemOps) -> Result<serde_json::Value, EnableError> {
    if current_status().as_deref() == Some("running") {
        return Ok(serde_json::json!({
            "ok": true,
            "state": "running",
            "alreadyEnabled": true,
        }));
    }
    if !is_baked(ops) {
        return Err(EnableError::NotBaked(NOT_BAKED_HINT.into()));
    }
    if !try_begin_enable() {
        return Err(EnableError::InProgress);
    }
    let result = run_enable_machine(ops);
    end_enable();
    result
}

#[allow(dead_code)]
pub fn enable() -> Result<serde_json::Value, EnableError> {
    enable_with(&RealSystemOps)
}

fn run_enable_machine(ops: &dyn SystemOps) -> Result<serde_json::Value, EnableError> {
    ensure_installing_row().map_err(EnableError::Engine)?;
    let fail = |step: &str, err: String| -> EnableError {
        let msg = format!("{step}: {err}");
        set_status("error");
        set_last_error(Some(&msg));
        EnableError::Engine(msg)
    };

    set_current("baked");
    mark_step("baked");

    set_current("start");
    ops.run_helper(&["systemctl", "enable", "--now", PG_UNIT], None)
        .map_err(|e| fail("start", e))?;
    mark_step("start");

    set_current("catalog");
    let version_out = ops
        .run_helper(
            &["psql", "-d", "postgres", "-tA"],
            Some(b"SHOW server_version_num;"),
        )
        .map_err(|e| fail("catalog", e))?;
    let major = parse_installed_major(&version_out).unwrap_or(0);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE sql_server SET installed_major = ?1, listen = 'localhost', \
             installed_at = COALESCE(installed_at, ?2), updated_at = ?2 WHERE id = 1",
            rusqlite::params![major, now_secs()],
        );
    }
    mark_step("catalog");

    let mut p = progress_load();
    p["completedAt"] = serde_json::json!(now_secs());
    p["current"] = serde_json::Value::Null;
    progress_save(&p);
    set_status("running");
    set_last_error(None);
    Ok(serde_json::json!({
        "ok": true,
        "state": "running",
        "installedMajor": major,
        "listen": "localhost",
        "alreadyEnabled": false,
    }))
}

pub fn parse_installed_major(raw: &str) -> Option<i64> {
    let token = raw
        .split(|c: char| c.is_whitespace() || c == '.')
        .find(|s| !s.is_empty())?;
    let n: i64 = token.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().ok()?;
    if n >= 100000 {
        Some(n / 10000)
    } else if n >= 10 {
        Some(n)
    } else {
        None
    }
}

pub fn disable_with(ops: &dyn SystemOps) -> Result<(), String> {
    ops.run_helper(&["systemctl", "disable", "--now", PG_UNIT], None)?;
    let _ = current_status();
    set_status("disabled");
    set_last_error(None);
    Ok(())
}

#[allow(dead_code)]
pub fn disable() -> Result<(), String> {
    disable_with(&RealSystemOps)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    NotInstalled,
    Running,
    Degraded(String),
    Stopped(String),
}

impl Health {
    pub fn as_status_str(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not-installed",
            Self::Running => "running",
            Self::Degraded(_) => "degraded",
            Self::Stopped(_) => "stopped",
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Degraded(d) | Self::Stopped(d) => Some(d),
            _ => None,
        }
    }
}

pub fn health_check_with(ops: &dyn SystemOps, ping: &dyn Fn() -> Result<(), String>) -> Health {
    if !PG_UNIT_PATHS.iter().any(|p| ops.path_exists(p)) && !ops.path_exists(PSQL_PATH) {
        return Health::NotInstalled;
    }
    let active = ops.systemctl_query(&["is-active", PG_UNIT]);
    if active != "active" {
        return Health::Stopped(format!(
            "systemd reports the postgresql unit is '{}'",
            if active.is_empty() { "unknown" } else { &active }
        ));
    }
    match ping() {
        Ok(()) => Health::Running,
        Err(e) => Health::Degraded(format!("unit is active but SELECT 1 failed: {e}")),
    }
}

pub fn refresh_health() -> serde_json::Value {
    let ping = || -> Result<(), String> {
        RealSystemOps
            .run_helper(
                &["psql", "-d", "postgres", "-tA"],
                Some(b"SELECT 1;"),
            )
            .map(|_| ())
    };
    let health = health_check_with(&RealSystemOps, &ping);
    persist_health(&health);
    serde_json::json!({
        "state": health.as_status_str(),
        "detail": health.detail(),
    })
}

fn persist_health(health: &Health) {
    let Some(previous) = current_status() else {
        return;
    };
    if matches!(previous.as_str(), "installing" | "disabled" | "error") {
        return;
    }
    let new_status = match health {
        Health::NotInstalled => return,
        _ => health.as_status_str(),
    };
    if previous == new_status {
        return;
    }
    set_status(new_status);
    set_last_error(health.detail());
}

pub fn spawn_health_loop() {
    if !sql_supported() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("sql-health".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            if enable_running().load(Ordering::SeqCst) {
                continue;
            }
            if current_status().is_none() {
                continue;
            }
            let _ = std::panic::catch_unwind(|| {
                let _ = refresh_health();
            });
        });
}

pub fn status_json(include_health: bool) -> serde_json::Value {
    let health = if sql_supported()
        && include_health
        && !enable_running().load(Ordering::SeqCst)
    {
        Some(refresh_health())
    } else {
        None
    };
    type Row = (
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, installed_major, listen, enable_progress_json, last_error \
             FROM sql_server WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok()
    };
    let (state, major, listen, progress, last_error) = match row {
        Some(t) => t,
        None => ("not-installed".to_string(), None, None, None, None),
    };
    let enable_progress = progress
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .unwrap_or(serde_json::Value::Null);
    let port = listen_port(listen.as_deref());
    serde_json::json!({
        "ok": true,
        "supported": sql_supported(),
        "state": state,
        "installedMajor": major,
        "listen": listen,
        "port": port,
        "publishHint": publish_hint(port),
        "enableProgress": enable_progress,
        "lastError": last_error,
        "health": health,
    })
}

/// Loopback Postgres port (D1/D30). Catalog `listen` is `localhost` or
/// `localhost:<port>`; missing/unparseable → 5432.
pub fn listen_port(listen: Option<&str>) -> u16 {
    let raw = listen.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
    if let Some((_, p)) = raw.rsplit_once(':') {
        if let Ok(n) = p.parse::<u16>() {
            if n != 0 {
                return n;
            }
        }
    }
    5432
}

/// One-line off-box recipe. Postgres stays loopback; `k2 publish` fronts
/// the already-listening port. Not a static-IP feature. Not `k2 db expose`.
pub fn publish_hint(port: u16) -> String {
    format!(
        "off-box *.k2.dev: k2 publish subdomain create <label> --target localhost:{port} (port already listening — do not publish run Postgres)"
    )
}

pub fn doctor_with(ops: &dyn SystemOps) -> serde_json::Value {
    let baked = is_baked(ops);
    let active = ops.systemctl_query(&["is-active", PG_UNIT]);
    let ping = ops
        .run_helper(
            &["psql", "-d", "postgres", "-tA"],
            Some(b"SELECT 1;"),
        );
    let recorded_major: Option<i64> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT installed_major FROM sql_server WHERE id = 1",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    };
    let ok = baked && active == "active" && ping.is_ok();
    serde_json::json!({
        "ok": ok,
        "baked": baked,
        "unit": active,
        "select1": ping.is_ok(),
        "installedMajor": recorded_major,
        "listen": "localhost",
        "hint": if ok { serde_json::Value::Null } else {
            serde_json::Value::String(
                if !baked { NOT_BAKED_HINT.into() }
                else { "postgresql unit is not healthy — see k2 db status --health".into() }
            )
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::sysops::FakeSystemOps;

    fn db_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::sql::sql_server_test_lock()
    }
    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM sql_server WHERE id = 1", []);
    }

    #[test]
    fn sql_supported_matches_cfg() {
        assert_eq!(sql_supported(), cfg!(target_os = "linux"));
    }

    #[test]
    fn unbaked_enable_409_no_apt() {
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        let ops = FakeSystemOps::default();
        let err = enable_with(&ops).expect_err("unbaked");
        assert_eq!(err.code(), "not_baked");
        let rec = ops.recorded();
        assert!(
            rec.iter().all(|l| !l.contains("apt")),
            "enable must never apt-get: {rec:?}"
        );
        assert!(
            rec.iter().all(|l| !l.contains("download")),
            "enable must never download: {rec:?}"
        );
    }

    #[test]
    fn baked_enable_starts_unit_and_lands_running() {
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        let ops = FakeSystemOps::baked();
        let v = enable_with(&ops).expect("enable");
        assert_eq!(v["state"], "running");
        assert_eq!(v["installedMajor"], 16);
        assert_eq!(current_status().as_deref(), Some("running"));
        let rec = ops.recorded();
        assert!(
            rec.iter().any(|l| l.contains("systemctl enable --now postgresql")),
            "expected helper start: {rec:?}"
        );
        assert!(rec.iter().all(|l| !l.contains("apt")));
    }

    #[test]
    fn health_skip_shape_and_disable() {
        let ops = FakeSystemOps::baked();
        let h = health_check_with(&ops, &|| Ok(()));
        assert_eq!(h, Health::Running);
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        enable_with(&ops).ok();
        disable_with(&ops).expect("disable");
        assert_eq!(current_status().as_deref(), Some("disabled"));
    }

    #[test]
    fn parse_major_from_server_version_num() {
        assert_eq!(parse_installed_major("160003"), Some(16));
        assert_eq!(parse_installed_major("140013"), Some(14));
        assert_eq!(parse_installed_major(" 160003 \n"), Some(16));
    }

    #[test]
    fn listen_port_defaults_5432_and_parses_catalog() {
        assert_eq!(listen_port(None), 5432);
        assert_eq!(listen_port(Some("")), 5432);
        assert_eq!(listen_port(Some("localhost")), 5432);
        assert_eq!(listen_port(Some("localhost:15432")), 15432);
        assert_eq!(listen_port(Some("127.0.0.1:5433")), 5433);
    }

    #[test]
    fn status_json_reports_port_and_publish_subdomain_not_static_ip() {
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO sql_server (id, status, installed_major, listen, updated_at) \
                 VALUES (1, 'running', 16, 'localhost', 1)",
                [],
            )
            .unwrap();
        }
        let v = status_json(false);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["port"], 5432, "default port: {v}");
        let hint = v["publishHint"].as_str().expect("publishHint present");
        assert!(
            hint.contains("k2 publish subdomain"),
            "hint must mention publish subdomain: {hint}"
        );
        assert!(
            hint.contains("localhost:5432"),
            "hint must name the loopback port: {hint}"
        );
        assert!(
            !hint.to_ascii_lowercase().contains("static ip"),
            "D30: static IP is not a K2 feature: {hint}"
        );
        assert!(
            !hint.contains("k2 db expose"),
            "must not invent k2 db expose: {hint}"
        );
    }
}
