//! `seeds`, `apps` and `deployments` rows (migration 0150, §9.1). Every
//! function takes the caller's connection, so tests run on their own DB.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value as J;

// ── seeds ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedRow {
    pub hash: String,
    pub seed_id: String,
    pub version: Option<String>,
    pub host_mode: String,
    pub manifest: J,
    pub created_with: J,
    pub report: J,
    pub signer: String,
    pub origin: String,
    pub built_for: Option<String>,
    pub built_from: Option<String>,
    /// Git HEAD of the source folder when it was a git checkout (litmus L3).
    pub source_ref: Option<String>,
    pub data_revision: i64,
    pub size_bytes: i64,
    pub created_at: i64,
    pub last_used_at: i64,
}

fn seed_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SeedRow> {
    let j = |s: String| serde_json::from_str(&s).unwrap_or(J::String(s));
    Ok(SeedRow {
        hash: r.get(0)?,
        seed_id: r.get(1)?,
        version: r.get(2)?,
        host_mode: r.get(3)?,
        manifest: j(r.get(4)?),
        created_with: j(r.get(5)?),
        report: j(r.get(6)?),
        signer: r.get(7)?,
        origin: r.get(8)?,
        built_for: r.get(9)?,
        built_from: r.get(10)?,
        source_ref: r.get(11)?,
        data_revision: r.get(12)?,
        size_bytes: r.get(13)?,
        created_at: r.get(14)?,
        last_used_at: r.get(15)?,
    })
}

const SEED_COLS: &str = "hash, seed_id, version, host_mode, manifest_json, created_with, report_json, signer, origin, \
                         built_for, built_from, source_ref, data_revision, size_bytes, created_at, last_used_at";

/// Insert a Seed row, or refresh `last_used_at` (and the report) when the
/// same tree was stored before. The first signer and `createdWith` stay.
pub fn upsert_seed(conn: &Connection, s: &SeedRow) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO seeds ({SEED_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(hash) DO UPDATE SET last_used_at = excluded.last_used_at, report_json = excluded.report_json"
        ),
        params![
            s.hash,
            s.seed_id,
            s.version,
            s.host_mode,
            s.manifest.to_string(),
            s.created_with.to_string(),
            s.report.to_string(),
            s.signer,
            s.origin,
            s.built_for,
            s.built_from,
            s.source_ref,
            s.data_revision,
            s.size_bytes,
            s.created_at,
            s.last_used_at,
        ],
    )?;
    Ok(())
}

pub fn get_seed(conn: &Connection, hash: &str) -> rusqlite::Result<Option<SeedRow>> {
    conn.query_row(&format!("SELECT {SEED_COLS} FROM seeds WHERE hash = ?1"), params![hash], seed_row)
        .optional()
}

/// A Seed by full hash or a unique prefix of at least 8 hex characters.
pub fn find_seed(conn: &Connection, token: &str) -> rusqlite::Result<Option<SeedRow>> {
    let t = token.trim().to_ascii_lowercase();
    if t.len() == 64 {
        return get_seed(conn, &t);
    }
    if t.len() < 8 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let mut st = conn.prepare(&format!("SELECT {SEED_COLS} FROM seeds WHERE hash LIKE ?1 LIMIT 2"))?;
    let v: Vec<SeedRow> = st.query_map(params![format!("{t}%")], seed_row)?.collect::<Result<_, _>>()?;
    Ok(if v.len() == 1 { v.into_iter().next() } else { None })
}

pub fn touch_seed(conn: &Connection, hash: &str, at: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE seeds SET last_used_at = ?2 WHERE hash = ?1", params![hash, at])?;
    Ok(())
}

pub fn list_seeds(conn: &Connection) -> rusqlite::Result<Vec<SeedRow>> {
    let mut st = conn.prepare(&format!("SELECT {SEED_COLS} FROM seeds ORDER BY created_at DESC"))?;
    let v = st.query_map([], seed_row)?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Seeds still referenced: any App's live or preview Seed, or a Seed in
/// one of an App's last `keep_deployments` deployments (§7.1).
pub fn referenced_seeds(conn: &Connection, keep_deployments: usize) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    let mut st = conn.prepare("SELECT live_seed, preview_seed FROM apps")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?;
    for r in rows {
        let (a, b) = r?;
        out.extend(a);
        out.extend(b);
    }
    let mut st = conn.prepare(
        "SELECT from_seed, to_seed FROM (SELECT from_seed, to_seed, ROW_NUMBER() OVER (PARTITION BY app_id ORDER BY started_at DESC, rowid DESC) AS n FROM deployments) WHERE n <= ?1",
    )?;
    let rows = st.query_map(params![keep_deployments as i64], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?;
    for r in rows {
        let (a, b) = r?;
        out.extend(a);
        out.extend(b);
    }
    Ok(out)
}

/// The sweep (§7.1): Seeds not referenced and unused for `max_age_secs`.
/// Removes the tree and the row; returns the hashes removed. Open
/// widening requests (0151, stage 3) will join the referenced set.
pub fn sweep_seeds(conn: &Connection, now: i64, max_age_secs: i64, keep_deployments: usize) -> rusqlite::Result<Vec<String>> {
    let keep = referenced_seeds(conn, keep_deployments)?;
    let mut removed = Vec::new();
    for s in list_seeds(conn)? {
        if keep.contains(&s.hash) || now - s.last_used_at < max_age_secs {
            continue;
        }
        if super::seed::remove_tree(&s.hash).is_ok() {
            conn.execute("DELETE FROM seeds WHERE hash = ?1", params![s.hash])?;
            removed.push(s.hash);
        }
    }
    Ok(removed)
}

// ── apps ─────────────────────────────────────────────────────────────

pub const STATE_INSTALLING: &str = "installing";
pub const STATE_RUNNING: &str = "running";
pub const STATE_STOPPED: &str = "stopped";
pub const STATE_CRASHLOOPING: &str = "crashlooping";
pub const STATE_FAILED: &str = "failed";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRow {
    pub app_id: String,
    pub service_id: String,
    pub seed_id: Option<String>,
    pub owner_workspace: String,
    pub path: String,
    pub source: String,
    pub folder_path: Option<String>,
    pub live_seed: Option<String>,
    pub live_tree_hash: Option<String>,
    pub live_deployment: Option<String>,
    pub preview_seed: Option<String>,
    pub isolation: String,
    /// `None` = no per-App memory limit (the default; Rosson 2026-10-10).
    pub memory_limit_bytes: Option<i64>,
    pub front_port: Option<i64>,
    pub state: String,
    pub state_reason: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

const APP_COLS: &str = "app_id, service_id, seed_id, owner_workspace, path, source, folder_path, live_seed, live_tree_hash, \
                        live_deployment, preview_seed, isolation, memory_limit_bytes, front_port, state, state_reason, \
                        created_by, created_at, updated_at";

fn app_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppRow> {
    Ok(AppRow {
        app_id: r.get(0)?,
        service_id: r.get(1)?,
        seed_id: r.get(2)?,
        owner_workspace: r.get(3)?,
        path: r.get(4)?,
        source: r.get(5)?,
        folder_path: r.get(6)?,
        live_seed: r.get(7)?,
        live_tree_hash: r.get(8)?,
        live_deployment: r.get(9)?,
        preview_seed: r.get(10)?,
        isolation: r.get(11)?,
        memory_limit_bytes: r.get(12)?,
        front_port: r.get(13)?,
        state: r.get(14)?,
        state_reason: r.get(15)?,
        created_by: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}

pub struct NewApp<'a> {
    pub app_id: &'a str,
    pub service_id: &'a str,
    pub seed_id: Option<&'a str>,
    pub owner_workspace: &'a str,
    pub isolation: &'a str,
    pub memory_limit_bytes: Option<i64>,
    pub front_port: u16,
    pub created_by: &'a str,
}

pub fn insert_app(conn: &Connection, a: &NewApp<'_>) -> rusqlite::Result<()> {
    let now = super::now();
    conn.execute(
        "INSERT INTO apps (app_id, service_id, seed_id, owner_workspace, isolation, memory_limit_bytes, front_port, state, created_by, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            a.app_id,
            a.service_id,
            a.seed_id,
            a.owner_workspace,
            a.isolation,
            a.memory_limit_bytes,
            a.front_port as i64,
            STATE_INSTALLING,
            a.created_by,
            now
        ],
    )?;
    Ok(())
}

pub fn get_app(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<AppRow>> {
    conn.query_row(&format!("SELECT {APP_COLS} FROM apps WHERE app_id = ?1"), params![app_id], app_row)
        .optional()
}

pub fn get_app_by_service(conn: &Connection, service_id: &str) -> rusqlite::Result<Option<AppRow>> {
    conn.query_row(&format!("SELECT {APP_COLS} FROM apps WHERE service_id = ?1"), params![service_id], app_row)
        .optional()
}

pub fn list_apps(conn: &Connection, workspace: Option<&str>) -> rusqlite::Result<Vec<AppRow>> {
    match workspace {
        Some(w) => {
            let mut st = conn.prepare(&format!("SELECT {APP_COLS} FROM apps WHERE owner_workspace = ?1 ORDER BY app_id"))?;
            let v = st.query_map(params![w], app_row)?.collect::<Result<_, _>>()?;
            Ok(v)
        }
        None => {
            let mut st = conn.prepare(&format!("SELECT {APP_COLS} FROM apps ORDER BY app_id"))?;
            let v = st.query_map([], app_row)?.collect::<Result<_, _>>()?;
            Ok(v)
        }
    }
}

pub fn set_state(conn: &Connection, app_id: &str, state: &str, reason: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET state = ?2, state_reason = ?3, updated_at = ?4 WHERE app_id = ?1",
        params![app_id, state, reason, super::now()],
    )?;
    Ok(())
}

pub fn set_live(conn: &Connection, app_id: &str, seed: Option<&str>, deployment: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET live_seed = ?2, live_deployment = ?3, updated_at = ?4 WHERE app_id = ?1",
        params![app_id, seed, deployment, super::now()],
    )?;
    Ok(())
}

pub fn set_memory_limit(conn: &Connection, app_id: &str, bytes: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET memory_limit_bytes = ?2, updated_at = ?3 WHERE app_id = ?1",
        params![app_id, bytes, super::now()],
    )?;
    Ok(())
}

pub fn delete_app(conn: &Connection, app_id: &str) -> rusqlite::Result<bool> {
    conn.execute("DELETE FROM deployments WHERE app_id = ?1", params![app_id])?;
    Ok(conn.execute("DELETE FROM apps WHERE app_id = ?1", params![app_id])? > 0)
}

// ── deployments ──────────────────────────────────────────────────────

pub const KIND_DEPLOY: &str = "deploy";
pub const KIND_ROLLBACK: &str = "rollback";
pub const KIND_AUTO_ROLLBACK: &str = "auto_rollback";
pub const KIND_RESTART: &str = "restart";
pub const KIND_START: &str = "start";

pub const RESULT_PENDING: &str = "pending";
pub const RESULT_STARTING: &str = "starting";
pub const RESULT_LIVE: &str = "live";
pub const RESULT_FAILED: &str = "failed";
pub const RESULT_STALLED: &str = "stalled";
pub const RESULT_ROLLED_BACK: &str = "rolled_back";
pub const RESULT_SUPERSEDED: &str = "superseded";
pub const RESULT_STOPPED: &str = "stopped";
pub const RESULT_EXITED: &str = "exited";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentRow {
    pub id: String,
    pub app_id: String,
    pub kind: String,
    pub from_seed: Option<String>,
    pub to_seed: Option<String>,
    pub tree_hash: Option<String>,
    /// git HEAD of the source, else the tree hash (litmus L3).
    pub commit_ref: Option<String>,
    pub actor: String,
    pub reason: Option<String>,
    pub result: String,
    pub error: Option<String>,
    pub port: Option<i64>,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub live_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub watch_until: Option<i64>,
}

const DEP_COLS: &str = "id, app_id, kind, from_seed, to_seed, tree_hash, commit_ref, actor, reason, result, error, port, pid, \
                        started_at, live_at, finished_at, watch_until";

fn dep_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRow> {
    Ok(DeploymentRow {
        id: r.get(0)?,
        app_id: r.get(1)?,
        kind: r.get(2)?,
        from_seed: r.get(3)?,
        to_seed: r.get(4)?,
        tree_hash: r.get(5)?,
        commit_ref: r.get(6)?,
        actor: r.get(7)?,
        reason: r.get(8)?,
        result: r.get(9)?,
        error: r.get(10)?,
        port: r.get(11)?,
        pid: r.get(12)?,
        started_at: r.get(13)?,
        live_at: r.get(14)?,
        finished_at: r.get(15)?,
        watch_until: r.get(16)?,
    })
}

pub struct NewDeployment<'a> {
    pub app_id: &'a str,
    pub kind: &'a str,
    pub from_seed: Option<&'a str>,
    pub to_seed: Option<&'a str>,
    pub commit_ref: Option<&'a str>,
    pub actor: &'a str,
    pub reason: Option<&'a str>,
}

pub fn insert_deployment(conn: &Connection, d: &NewDeployment<'_>) -> rusqlite::Result<String> {
    let id = format!("d-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    conn.execute(
        "INSERT INTO deployments (id, app_id, kind, from_seed, to_seed, tree_hash, commit_ref, actor, reason, result, started_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, d.app_id, d.kind, d.from_seed, d.to_seed, d.commit_ref, d.actor, d.reason, RESULT_PENDING, super::now()],
    )?;
    Ok(id)
}

pub fn get_deployment(conn: &Connection, id: &str) -> rusqlite::Result<Option<DeploymentRow>> {
    conn.query_row(&format!("SELECT {DEP_COLS} FROM deployments WHERE id = ?1"), params![id], dep_row)
        .optional()
}

/// One App's deployments, newest first.
pub fn list_deployments(conn: &Connection, app_id: &str, limit: usize) -> rusqlite::Result<Vec<DeploymentRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {DEP_COLS} FROM deployments WHERE app_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT ?2"
    ))?;
    let v = st.query_map(params![app_id, limit as i64], dep_row)?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Deployments still pending/starting (for the stall check and boot).
pub fn unfinished_deployments(conn: &Connection) -> rusqlite::Result<Vec<DeploymentRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {DEP_COLS} FROM deployments WHERE result IN ('pending','starting') ORDER BY started_at"
    ))?;
    let v = st.query_map([], dep_row)?.collect::<Result<_, _>>()?;
    Ok(v)
}

pub fn set_result(conn: &Connection, id: &str, result: &str, error: Option<&str>) -> rusqlite::Result<()> {
    let finished = matches!(
        result,
        RESULT_FAILED | RESULT_ROLLED_BACK | RESULT_SUPERSEDED | RESULT_STOPPED | RESULT_EXITED | RESULT_STALLED
    );
    conn.execute(
        "UPDATE deployments SET result = ?2, error = COALESCE(?3, error), finished_at = CASE WHEN ?4 THEN ?5 ELSE finished_at END WHERE id = ?1",
        params![id, result, error, finished, super::now()],
    )?;
    Ok(())
}

pub fn set_started(conn: &Connection, id: &str, port: u16, pid: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE deployments SET result = ?2, port = ?3, pid = ?4 WHERE id = ?1",
        params![id, RESULT_STARTING, port as i64, pid],
    )?;
    Ok(())
}

pub fn set_went_live(conn: &Connection, id: &str, watch_until: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE deployments SET result = ?2, live_at = ?3, watch_until = ?4 WHERE id = ?1",
        params![id, RESULT_LIVE, super::now(), watch_until],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0150_app_runtime_core.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        c
    }

    pub(crate) fn seed(hash_char: char, at: i64) -> SeedRow {
        SeedRow {
            hash: std::iter::repeat_n(hash_char, 64).collect(),
            seed_id: "jim".into(),
            version: Some("1.0.0".into()),
            host_mode: "cmd".into(),
            manifest: serde_json::json!({}),
            created_with: serde_json::json!({}),
            report: serde_json::json!({"ok": true}),
            signer: "owner".into(),
            origin: "local".into(),
            built_for: None,
            built_from: None,
            source_ref: None,
            data_revision: 0,
            size_bytes: 1,
            created_at: at,
            last_used_at: at,
        }
    }

    #[test]
    fn seed_prefix_lookup_and_referenced_set() {
        let c = conn();
        upsert_seed(&c, &seed('a', 1)).unwrap();
        upsert_seed(&c, &seed('b', 1)).unwrap();
        assert!(find_seed(&c, "aaaaaaaa").unwrap().is_some());
        assert!(find_seed(&c, "aaaa").unwrap().is_none(), "too short");
        insert_app(
            &c,
            &NewApp {
                app_id: "jim",
                service_id: "svc-1",
                seed_id: Some("jim"),
                owner_workspace: "ws",
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 40000,
                created_by: "owner",
            },
        )
        .unwrap();
        let a = get_app(&c, "jim").unwrap().unwrap();
        assert_eq!(a.memory_limit_bytes, None, "no per-App memory limit by default");
        set_live(&c, "jim", Some(&seed('a', 1).hash), None).unwrap();
        let r = referenced_seeds(&c, 10).unwrap();
        assert!(r.contains(&seed('a', 1).hash));
        assert!(!r.contains(&seed('b', 1).hash));
        let id = insert_deployment(
            &c,
            &NewDeployment { app_id: "jim", kind: KIND_DEPLOY, from_seed: None, to_seed: Some(&seed('b', 1).hash), commit_ref: None, actor: "owner", reason: None },
        )
        .unwrap();
        assert!(referenced_seeds(&c, 10).unwrap().contains(&seed('b', 1).hash));
        assert_eq!(get_deployment(&c, &id).unwrap().unwrap().result, RESULT_PENDING);
        assert_eq!(unfinished_deployments(&c).unwrap().len(), 1);
    }
}
