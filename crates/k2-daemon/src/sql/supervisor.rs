//! Postgres sidecar supervisor. The ONLY module that knows postgres
//! exists as a process. Bake-first: enable never downloads or apt-gets.

use std::sync::atomic::{AtomicBool, Ordering};

use super::cluster;
use super::sql_supported;
use super::sysops::{
    guc_show_sql, is_baked, pin_gucs_sql, ram_dropin, RealSystemOps, SystemOps, GUC_PINNED,
    PG_RAM_DROPIN_PATH, PG_RAM_TEMPLATE_DROPIN_PATH, PG_UNIT, PG_UNIT_PATHS, PSQL_PATH,
    RAM_CPU_WEIGHT, RAM_MEMORY_HIGH, RAM_MEMORY_MAX,
}; // used by spawn_health_loop

#[allow(dead_code)]
pub const ENABLE_STEPS: &[&str] = &["baked", "ram", "start", "gucs", "catalog"];

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
                "the SQL sidecar only works on Linux deployments; this daemon is not Linux".into()
            }
            Self::NotBaked(h) => h.clone(),
            Self::InProgress => "an enable run is already in progress — poll /cli/db/status".into(),
            Self::Engine(h) => h.clone(),
        }
    }
}

const NOT_BAKED_HINT: &str = "Postgres sidecar is not baked on this box. Re-run \
provision-k2-server.sh --bake --with-db (or K2_BAKE_DB=1). The daemon does not apt-get.";

/// Catalog ingest + helper start if the unit is baked. Never apt-get.
/// D29 ram fence is applied even when the sidecar is already running so
/// an upgrade re-enable converges MemoryHigh/Max + GUC pins.
pub fn enable_with(ops: &dyn SystemOps) -> Result<serde_json::Value, EnableError> {
    if !is_baked(ops) {
        return Err(EnableError::NotBaked(NOT_BAKED_HINT.into()));
    }
    if current_status().as_deref() == Some("running") {
        if !try_begin_enable() {
            return Err(EnableError::InProgress);
        }
        let result = apply_ram_fence(ops).map(|()| {
            serde_json::json!({
                "ok": true,
                "state": "running",
                "alreadyEnabled": true,
            })
        });
        end_enable();
        return result.map_err(EnableError::Engine);
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

    set_current("ram");
    apply_ram_fence(ops).map_err(|e| fail("ram", e))?;
    mark_step("ram");

    set_current("start");
    ops.run_helper(&["systemctl", "enable", "--now", PG_UNIT], None)
        .map_err(|e| fail("start", e))?;
    mark_step("start");

    set_current("gucs");
    mark_step("gucs");

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
        // The 5432 cluster's record (0.46.3). A live major that differs
        // from the record is never rewritten here: the doctor fails (V6).
        if major > 0 {
            if let Err(e) = cluster::record_primary_major(&conn, major as u32) {
                eprintln!("[db] enable: {e}");
            }
        }
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
    let n: i64 = token
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
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
            if active.is_empty() {
                "unknown"
            } else {
                &active
            }
        ));
    }
    // The wrapper reads "active" whatever its instances do (V3): every
    // other cluster that serves a live database must be up itself.
    for (c, _) in serving_clusters().into_iter().filter(|(c, _)| !c.is_primary()) {
        let a = ops.systemctl_query(&["is-active", &c.unit]);
        if a != "active" {
            return Health::Degraded(format!(
                "the Postgres {} cluster ({}, port {}) serves databases but is '{}'",
                c.major,
                c.unit,
                c.port,
                if a.is_empty() { "unknown" } else { &a }
            ));
        }
    }
    match ping() {
        Ok(()) => Health::Running,
        Err(e) => Health::Degraded(format!("unit is active but SELECT 1 failed: {e}")),
    }
}

/// Each K2 cluster with the number of live databases it serves.
fn serving_clusters() -> Vec<(cluster::Cluster, usize)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let majors = cluster::database_majors_in(&conn);
    cluster::k2_clusters_in(&conn)
        .into_iter()
        .map(|c| {
            let n = majors.iter().filter(|(_, _, m)| *m == c.major).count();
            (c, n)
        })
        .filter(|(_, n)| *n > 0)
        .collect()
}

pub fn refresh_health() -> serde_json::Value {
    let ping = || -> Result<(), String> {
        RealSystemOps
            .run_helper(&["psql", "-d", "postgres", "-tA"], Some(b"SELECT 1;"))
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
            // Once per boot, after the sidecar is up: databases created by
            // older daemons get the k2 helpers + the guest seal on K2's own
            // tables (`_k2_*`). Idempotent; never fatal.
            if current_status().as_deref() == Some("running")
                && !BOOT_SEAL_DONE.swap(true, Ordering::SeqCst)
            {
                // Fold the box's clusters into K2's records (helper 9);
                // older helpers: the records stay as enable wrote them.
                let _ = std::panic::catch_unwind(|| {
                    if let Err(e) = cluster::reconcile(&RealSystemOps) {
                        if !e.starts_with("helper_needed") {
                            eprintln!("[db] clusters: {e}");
                        }
                    }
                });
                let _ = std::panic::catch_unwind(|| {
                    let ops = RealSystemOps;
                    for line in super::ops::seal_existing_databases(&ops) {
                        eprintln!("[db] boot seal: {line}");
                    }
                });
                // `k2 db upgrade`: a move the last daemon died in (§13).
                let _ = std::panic::catch_unwind(super::pgmove::k2::recover_at_boot);
            }
        });
}

static BOOT_SEAL_DONE: AtomicBool = AtomicBool::new(false);

pub fn status_json(include_health: bool) -> serde_json::Value {
    let health = if sql_supported() && include_health && !enable_running().load(Ordering::SeqCst) {
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
    let majors = majors_json();
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
        "clusters": majors["clusters"],
        "databases": majors["databases"],
        "upgradeAvailable": majors["upgradeAvailable"],
        "defaultMajor": majors["defaultMajor"],
        "defaultMajorPinned": majors["defaultMajorPinned"],
        "defaultMajorSource": majors["defaultMajorSource"],
        "supportedMajors": majors["supportedMajors"],
    })
}

/// Majors side by side for status and the doctor (§14, V24: agents read
/// status, so `upgradeAvailable` lives here).
pub fn majors_json() -> serde_json::Value {
    use k2_root_proto::pg_major;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let clusters = cluster::list_in(&conn);
    let clusters = if clusters.is_empty() {
        let p = cluster::primary_in(&conn);
        if p.major > 0 { vec![p] } else { vec![] }
    } else {
        clusters
    };
    let today = pg_major::today_days();
    let dbs = cluster::database_majors_in(&conn);
    let upgrades: Vec<serde_json::Value> = dbs
        .iter()
        .filter(|(_, _, m)| *m > 0)
        .filter_map(|(id, name, m)| {
            let to = pg_major::upgrades_from(*m, today);
            (!to.is_empty()).then(|| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "from": m,
                    "to": to,
                    "check": format!("k2 db upgrade {name} --check"),
                })
            })
        })
        .collect();
    let (default_major, source) = cluster::default_major_source_in(&conn);
    serde_json::json!({
        "clusters": clusters.iter().map(cluster::Cluster::json).collect::<Vec<_>>(),
        "databases": dbs.iter().map(|(id, name, m)| serde_json::json!({
            "id": id,
            "name": name,
            "major": if *m > 0 { serde_json::json!(m) } else { serde_json::Value::Null },
        })).collect::<Vec<_>>(),
        "upgradeAvailable": upgrades,
        "defaultMajor": default_major,
        "defaultMajorPinned": source == "set",
        "defaultMajorSource": source,
        "supportedMajors": pg_major::SUPPORTED,
    })
}

/// Loopback Postgres port (D1/D30). Catalog `listen` is `localhost` or
/// `localhost:<port>`; missing/unparseable → 5432.
pub fn listen_port(listen: Option<&str>) -> u16 {
    let raw = listen
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
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

/// Write the systemd drop-in + pg conf.d (helper), daemon-reload, restart
/// so MemoryHigh/Max attach, then ALTER SYSTEM the D29 GUC pins.
fn apply_ram_fence(ops: &dyn SystemOps) -> Result<(), String> {
    ops.run_helper(&["install-ram-fence"], None)?;
    ops.run_helper(&["systemctl", "daemon-reload"], None)?;
    ops.run_helper(&["systemctl", "restart", PG_UNIT], None)?;
    pin_gucs_with_retry(ops)
}

fn pin_gucs_with_retry(ops: &dyn SystemOps) -> Result<(), String> {
    let sql = pin_gucs_sql();
    let mut last = String::new();
    for i in 0..5 {
        match ops.run_helper(
            &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-tA"],
            Some(sql.as_bytes()),
        ) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last = e;
                if i + 1 < 5 {
                    ops.sleep_ms(400);
                }
            }
        }
    }
    Err(last)
}

fn dropin_present(ops: &dyn SystemOps) -> bool {
    ops.path_exists(PG_RAM_DROPIN_PATH) && ops.path_exists(PG_RAM_TEMPLATE_DROPIN_PATH)
}

fn dropin_matches_policy(ops: &dyn SystemOps) -> bool {
    let read = |p: &str| -> Option<String> {
        ops.read_file(p)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let want = ram_dropin();
    match (read(PG_RAM_DROPIN_PATH), read(PG_RAM_TEMPLATE_DROPIN_PATH)) {
        (Some(a), Some(b)) => a == want && b == want,
        _ => false,
    }
}

fn cluster_unit_name(ops: &dyn SystemOps, major: Option<i64>) -> String {
    // The recorded 5432 cluster's own unit (V4): with two clusters running
    // "the first active postgresql@" could be either.
    let p = cluster::primary();
    if !p.synthesized {
        return p.unit;
    }
    let listed = ops.systemctl_query(&[
        "list-units",
        "--type=service",
        "--state=active",
        "--no-legend",
        "--plain",
        "postgresql@*",
    ]);
    for line in listed.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        if name.starts_with("postgresql@") && name.ends_with(".service") {
            return name.to_string();
        }
        if name.starts_with("postgresql@") {
            return name.to_string();
        }
    }
    if let Some(m) = major {
        if m > 0 {
            return format!("postgresql@{m}-main.service");
        }
    }
    PG_UNIT.to_string()
}

fn parse_systemctl_show(raw: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in raw.lines() {
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

fn parse_mem_bytes(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty()
        || s.eq_ignore_ascii_case("infinity")
        || s.eq_ignore_ascii_case("[not set]")
        || s == "unlimited"
    {
        return None;
    }
    let n: u64 = s.parse().ok()?;
    if n == u64::MAX {
        None
    } else {
        Some(n)
    }
}

fn parse_pg_duration_ms(raw: &str) -> Option<u64> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if s == "0" || s == "0s" || s == "0ms" {
        return Some(0);
    }
    let (num, mul) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix("min") {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        (s.as_str(), 1u64)
    };
    let n: u64 = num.trim().parse().ok()?;
    n.checked_mul(mul)
}

fn parse_pg_bytes(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s == "-1" {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let (num, mul) = if let Some(n) = lower.strip_suffix("gb") {
        (n, 1024u64 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("kb") {
        (n, 1024)
    } else {
        (lower.as_str(), 1)
    };
    let n: u64 = num.trim().parse().ok()?;
    n.checked_mul(mul)
}

fn guc_matches(name: &str, got: &str, want: &str) -> bool {
    if got == want {
        return true;
    }
    match name {
        "statement_timeout" | "idle_in_transaction_session_timeout" => {
            parse_pg_duration_ms(got) == parse_pg_duration_ms(want)
                && parse_pg_duration_ms(want).is_some()
        }
        "shared_buffers" | "work_mem" | "temp_file_limit" => {
            parse_pg_bytes(got) == parse_pg_bytes(want) && parse_pg_bytes(want).is_some()
        }
        "max_connections" | "max_parallel_workers" => got.trim() == want.trim(),
        _ => false,
    }
}

fn collect_gucs(ops: &dyn SystemOps) -> (serde_json::Value, bool) {
    let sql = guc_show_sql();
    let raw = match ops.run_helper(
        &["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-tA"],
        Some(sql.as_bytes()),
    ) {
        Ok(s) => s,
        Err(_) => {
            return (serde_json::json!({}), false);
        }
    };
    let mut got: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('|') {
            got.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let mut obj = serde_json::Map::new();
    let mut all_ok = true;
    for (name, want) in GUC_PINNED {
        match got.get(*name) {
            Some(v) if guc_matches(name, v, want) => {
                obj.insert((*name).to_string(), serde_json::Value::String(v.clone()));
            }
            Some(v) => {
                all_ok = false;
                obj.insert((*name).to_string(), serde_json::Value::String(v.clone()));
            }
            None => {
                all_ok = false;
            }
        }
    }
    if obj.len() != GUC_PINNED.len() {
        all_ok = false;
    }
    (serde_json::Value::Object(obj), all_ok)
}

pub fn doctor_with(ops: &dyn SystemOps) -> serde_json::Value {
    let baked = is_baked(ops);
    let active = ops.systemctl_query(&["is-active", PG_UNIT]);
    let ping = ops.run_helper(&["psql", "-d", "postgres", "-tA"], Some(b"SELECT 1;"));
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
    let ram_unit = cluster_unit_name(ops, recorded_major);
    let show = ops.systemctl_query(&[
        "show",
        &ram_unit,
        "--property=MemoryCurrent,MemoryHigh,MemoryMax,MemoryAccounting",
    ]);
    let props = parse_systemctl_show(&show);
    let current_bytes = props.get("MemoryCurrent").and_then(|s| parse_mem_bytes(s));
    let high_bytes = props.get("MemoryHigh").and_then(|s| parse_mem_bytes(s));
    let max_bytes = props.get("MemoryMax").and_then(|s| parse_mem_bytes(s));
    let accounting = props
        .get("MemoryAccounting")
        .map(|s| s.eq_ignore_ascii_case("yes"))
        == Some(true);
    let dropin = dropin_present(ops);
    let dropin_ok = dropin_matches_policy(ops);
    let within_cap = match (current_bytes, max_bytes) {
        (Some(c), Some(m)) => c <= m,
        _ => false,
    };
    let ram_applied = dropin_ok && accounting && max_bytes.is_some();
    let (gucs, gucs_ok) = if ping.is_ok() {
        collect_gucs(ops)
    } else {
        (serde_json::json!({}), false)
    };
    // Live ≠ recorded major on 5432 is a failure (V6); every other
    // cluster serving databases must be up (V3).
    let live_major = if ping.is_ok() {
        ops.run_helper(&["psql", "-d", "postgres", "-tA"], Some(b"SHOW server_version_num;"))
            .ok()
            .and_then(|o| parse_installed_major(&o))
    } else {
        None
    };
    let primary = cluster::primary();
    let major_mismatch = match (live_major, primary.synthesized) {
        (Some(live), false) => live != primary.major as i64,
        (Some(live), true) => recorded_major.is_some_and(|r| r > 0 && r != live),
        _ => false,
    };
    let mut clusters_ok = true;
    let serving = serving_clusters();
    let clusters_view: Vec<serde_json::Value> = {
        let all = cluster::k2_clusters();
        all.iter()
            .map(|c| {
                let dbs = serving.iter().find(|(s, _)| s.major == c.major).map(|(_, n)| *n).unwrap_or(0);
                let unit_state = if c.is_primary() { active.clone() } else { ops.systemctl_query(&["is-active", &c.unit]) };
                if dbs > 0 && unit_state != "active" {
                    clusters_ok = false;
                }
                let mut v = c.json();
                v["unitState"] = serde_json::json!(unit_state);
                v["databases"] = serde_json::json!(dbs);
                v
            })
            .collect()
    };
    let ok = baked
        && active == "active"
        && ping.is_ok()
        && ram_applied
        && gucs_ok
        && within_cap
        && !major_mismatch
        && clusters_ok;
    let hint = if ok {
        serde_json::Value::Null
    } else if major_mismatch {
        serde_json::Value::String(format!(
            "the 5432 cluster answers Postgres {}, but K2 recorded {} — something outside K2 changed it; do not create or move databases until it is understood",
            live_major.unwrap_or_default(),
            if primary.synthesized { recorded_major.unwrap_or_default() } else { primary.major as i64 }
        ))
    } else if !clusters_ok {
        serde_json::Value::String(
            "a Postgres cluster that serves databases is not running — see clusters[] (k2 db doctor --json)".into(),
        )
    } else if !baked {
        serde_json::Value::String(NOT_BAKED_HINT.into())
    } else if !ram_applied {
        serde_json::Value::String(
            "Postgres RAM fence missing — re-run k2 db enable (MemoryHigh=25% MemoryMax=40% drop-in + GUC caps)"
                .into(),
        )
    } else if !gucs_ok {
        serde_json::Value::String(
            "Postgres GUC caps are not pinned — re-run k2 db enable (work_mem/max_connections/timeouts)"
                .into(),
        )
    } else if !within_cap {
        serde_json::Value::String(
            "Postgres RSS is at or above MemoryMax — OOM should kill Postgres only; check k2 db doctor --json"
                .into(),
        )
    } else {
        serde_json::Value::String(
            "postgresql unit is not healthy — see k2 db status --health".into(),
        )
    };
    serde_json::json!({
        "ok": ok,
        "baked": baked,
        "unit": active,
        "select1": ping.is_ok(),
        "installedMajor": recorded_major,
        "liveMajor": live_major,
        "majorMismatch": major_mismatch,
        "clusters": clusters_view,
        "listen": "localhost",
        "ram": {
            "unit": ram_unit,
            "accounting": accounting,
            "currentBytes": current_bytes,
            "highBytes": high_bytes,
            "maxBytes": max_bytes,
            "high": RAM_MEMORY_HIGH,
            "max": RAM_MEMORY_MAX,
            "cpuWeight": RAM_CPU_WEIGHT,
            "dropin": PG_RAM_DROPIN_PATH,
            "templateDropin": PG_RAM_TEMPLATE_DROPIN_PATH,
            "dropinPresent": dropin,
            "dropinOk": dropin_ok,
            "withinCap": within_cap,
            "oomTarget": "postgres-only",
        },
        "gucs": gucs,
        "gucsOk": gucs_ok,
        "gucsWant": GUC_PINNED
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "hint": hint,
        "skinRlsTemplate": super::SKIN_RLS_TEMPLATE_HINT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::sysops::FakeSystemOps;

    fn db_guard() -> k2_core::test_env::SerialGuard {
        crate::sql::sql_server_test_lock()
    }
    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM sql_server WHERE id = 1", []); let _ = conn.execute("DELETE FROM sql_clusters", []);
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
            rec.iter()
                .any(|l| l.contains("systemctl enable --now postgresql")),
            "expected helper start: {rec:?}"
        );
        assert!(
            rec.iter().any(|l| l.contains("helper install-ram-fence")),
            "enable must write the D29 drop-in via helper: {rec:?}"
        );
        assert!(
            rec.iter().any(|l| l.contains("systemctl daemon-reload")),
            "enable must daemon-reload after the drop-in: {rec:?}"
        );
        assert!(
            rec.iter()
                .any(|l| l.contains("systemctl restart postgresql")),
            "enable must restart so MemoryMax attaches: {rec:?}"
        );
        assert!(rec.iter().all(|l| !l.contains("apt")));
        let files = ops.files.lock().unwrap_or_else(|p| p.into_inner());
        let dropin = files
            .get(crate::sql::sysops::PG_RAM_DROPIN_PATH)
            .expect("wrapper drop-in written");
        let dropin = std::str::from_utf8(dropin).expect("utf8 drop-in");
        assert_eq!(dropin, crate::sql::sysops::ram_dropin());
        let tmpl = files
            .get(crate::sql::sysops::PG_RAM_TEMPLATE_DROPIN_PATH)
            .expect("template drop-in written");
        assert_eq!(std::str::from_utf8(tmpl).expect("utf8"), dropin);
        assert!(
            ops.pg
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .helper_sql
                .iter()
                .any(|s| s.contains("ALTER SYSTEM SET work_mem")),
            "enable must pin GUCs via helper psql ALTER SYSTEM"
        );
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

    #[test]
    fn guc_timeout_1min_matches_60s() {
        assert!(guc_matches("statement_timeout", "1min", "60s"));
        assert!(guc_matches("statement_timeout", "60000ms", "60s"));
        assert!(!guc_matches("statement_timeout", "0", "60s"));
        assert!(guc_matches("work_mem", "8192kB", "8MB"));
        assert!(!guc_matches("max_connections", "100", "50"));
    }

    #[test]
    fn ram_dropin_is_fraction_never_100_and_oom_postgres_only() {
        let d = crate::sql::sysops::ram_dropin();
        assert!(d.contains("MemoryAccounting=yes"), "{d}");
        assert!(d.contains("MemoryHigh=25%"), "{d}");
        assert!(d.contains("MemoryMax=40%"), "{d}");
        assert!(d.contains("CPUWeight=50"), "{d}");
        assert!(d.contains("OOMPolicy=kill"), "{d}");
        assert!(!d.contains("100%"), "never cap at 100% of box RAM: {d}");
        assert!(!d.contains("MemoryMax=infinity"), "{d}");
        let conf = crate::sql::sysops::guc_conf();
        assert!(conf.contains("shared_buffers = 128MB"), "{conf}");
        assert!(conf.contains("work_mem = 8MB"), "{conf}");
        assert!(conf.contains("max_connections = 50"), "{conf}");
        assert!(conf.contains("max_parallel_workers = 2"), "{conf}");
        assert!(conf.contains("statement_timeout = 60s"), "{conf}");
        assert!(
            conf.contains("idle_in_transaction_session_timeout = 60s"),
            "{conf}"
        );
        assert!(conf.contains("temp_file_limit = 1GB"), "{conf}");
        assert!(
            !conf.to_ascii_lowercase().contains("25%"),
            "shared_buffers must not be a RAM percentage: {conf}"
        );
    }

    #[test]
    fn helper_script_allowlists_ram_fence_and_daemon_reload() {
        let helper = include_str!("../../../../scripts/k2-pg-helper");
        assert!(helper.contains("install-ram-fence"), "{helper}");
        assert!(helper.contains("daemon-reload"), "{helper}");
        assert!(helper.contains("MemoryHigh=25%"), "{helper}");
        assert!(helper.contains("MemoryMax=40%"), "{helper}");
        assert!(helper.contains("OOMPolicy=kill"), "{helper}");
        assert!(helper.contains("99-k2-ram.conf"), "{helper}");
        assert!(helper.contains("work_mem = 8MB"), "{helper}");
        assert!(
            !helper.contains("100%"),
            "helper drop-in must never be 100%: {helper}"
        );
        assert!(
            helper.contains("psql -c is forbidden"),
            "agent SQL must not ride argv: {helper}"
        );
        let dropin = crate::sql::sysops::ram_dropin();
        assert!(
            helper.contains(dropin),
            "helper heredoc must match ram_dropin() exactly:\n{dropin:?}\n{helper}"
        );
        let provision = include_str!("../../../../scripts/provision-k2-server.sh");
        assert!(
            provision.contains("install-ram-fence"),
            "bake --with-db must apply the RAM fence: {provision}"
        );
    }

    #[test]
    fn doctor_without_fence_is_not_ok() {
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        let ops = FakeSystemOps::baked();
        let v = doctor_with(&ops);
        assert_eq!(v["baked"], true, "{v}");
        assert_eq!(v["unit"], "active", "{v}");
        assert_eq!(v["select1"], true, "{v}");
        assert_eq!(v["ok"], false, "unfenced baked unit must fail doctor: {v}");
        assert_eq!(v["ram"]["dropinPresent"], false, "{v}");
        assert_eq!(v["ram"]["dropinOk"], false, "{v}");
        assert_eq!(v["gucsOk"], false, "{v}");
        assert_eq!(v["gucs"]["max_connections"], "100", "{v}");
        assert_eq!(v["gucs"]["work_mem"], "4MB", "{v}");
        let hint = v["hint"].as_str().expect("hint");
        assert!(
            hint.contains("RAM fence") || hint.contains("GUC"),
            "hint must name the missing fence: {hint}"
        );
    }

    #[test]
    fn doctor_reports_rss_vs_cap_and_pinned_gucs() {
        let _g = db_guard();
        clean_row();
        k2_core::db::init_for_tests();
        let ops = FakeSystemOps::baked();
        enable_with(&ops).expect("enable");
        let v = doctor_with(&ops);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["ram"]["high"], "25%", "{v}");
        assert_eq!(v["ram"]["max"], "40%", "{v}");
        assert_eq!(v["ram"]["cpuWeight"], "50", "{v}");
        assert_eq!(v["ram"]["oomTarget"], "postgres-only", "{v}");
        assert_eq!(v["ram"]["dropinPresent"], true, "{v}");
        assert_eq!(v["ram"]["dropinOk"], true, "{v}");
        assert_eq!(v["ram"]["accounting"], true, "{v}");
        assert_eq!(v["ram"]["currentBytes"], 268435456_i64, "{v}");
        assert_eq!(v["ram"]["maxBytes"], 6871947673_i64, "{v}");
        assert_eq!(v["ram"]["withinCap"], true, "{v}");
        assert_eq!(
            v["ram"]["unit"], "postgresql@16-main.service",
            "RSS must come from the cluster unit, not the oneshot wrapper: {v}"
        );
        assert_eq!(v["gucsOk"], true, "{v}");
        assert_eq!(v["gucs"]["shared_buffers"], "128MB", "{v}");
        assert_eq!(v["gucs"]["work_mem"], "8MB", "{v}");
        assert_eq!(v["gucs"]["max_connections"], "50", "{v}");
        assert_eq!(v["gucs"]["max_parallel_workers"], "2", "{v}");
        assert_eq!(v["gucs"]["statement_timeout"], "60s", "{v}");
        assert_eq!(
            v["gucs"]["idle_in_transaction_session_timeout"], "60s",
            "{v}"
        );
        assert_eq!(v["gucs"]["temp_file_limit"], "1GB", "{v}");
        assert_eq!(v["gucsWant"]["work_mem"], "8MB", "{v}");
        assert_eq!(v["hint"], serde_json::Value::Null, "{v}");
    }

    #[test]
    fn already_running_enable_still_applies_ram_fence() {
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
        let ops = FakeSystemOps::baked();
        let v = enable_with(&ops).expect("re-enable");
        assert_eq!(v["alreadyEnabled"], true, "{v}");
        let rec = ops.recorded();
        assert!(
            rec.iter().any(|l| l.contains("helper install-ram-fence")),
            "upgrade re-enable must still write the fence: {rec:?}"
        );
        assert!(
            rec.iter().any(|l| l.contains("systemctl daemon-reload")),
            "upgrade re-enable must daemon-reload: {rec:?}"
        );
        let d = doctor_with(&ops);
        assert_eq!(d["ok"], true, "{d}");
        assert_eq!(d["gucs"]["max_connections"], "50", "{d}");
    }
}
