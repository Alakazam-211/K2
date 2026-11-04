//! The install-job store (migration 0304, R41/A23): one row per one-click
//! install, the step it is on and why it waits; the lineage of each suite
//! App (entry, variant, version, follow, forked, bad versions); and the
//! suite-label records (A7: `{label, app, frontPort, boundAt}`, the URL is
//! derived from the box's subdomain when read).
//!
//! A job resumes after a daemon restart from its `step` (the next step to
//! run): every step before it is done and recorded in `steps_json`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The steps, in order (A5): the address is bound before the first deploy,
/// because Hearth and Meet refuse to boot without `K2_APP_ORIGIN`.
pub const STEP_DOWNLOAD: &str = "download";
pub const STEP_CHECK: &str = "check";
pub const STEP_INSTALL: &str = "install";
pub const STEP_ADDRESS: &str = "address";
pub const STEP_DEPLOY: &str = "deploy";
pub const STEP_APPROVE: &str = "approve";
pub const STEP_SEW_SETUP: &str = "sew_setup";
pub const STEP_HEALTHY: &str = "healthy";
pub const STEP_READY: &str = "ready";
pub const STEP_DONE: &str = "done";
pub const STEPS: &[&str] = &[
    STEP_DOWNLOAD,
    STEP_CHECK,
    STEP_INSTALL,
    STEP_ADDRESS,
    STEP_DEPLOY,
    STEP_APPROVE,
    STEP_SEW_SETUP,
    STEP_HEALTHY,
    STEP_READY,
];

pub const STATE_RUNNING: &str = "running";
pub const STATE_WAITING: &str = "waiting";
pub const STATE_FAILED: &str = "failed";
pub const STATE_DONE: &str = "done";

/// Why a job waits (`wait_reason`), each a named state the sheet and the
/// CLI show.
pub const WAIT_APPROVAL: &str = "waiting_approval";
pub const WAIT_SEW_RIGHTS: &str = "waiting_sew_rights";

pub const ADDRESS_SUITE_LABEL: &str = "suite_label";
pub const ADDRESS_LABEL: &str = "label";

/// The step after `step` (`done` after the last).
pub fn next_step(step: &str) -> &'static str {
    match STEPS.iter().position(|s| *s == step) {
        Some(i) if i + 1 < STEPS.len() => STEPS[i + 1],
        _ => STEP_DONE,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDone {
    pub step: String,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: String,
    pub entry: String,
    pub version: String,
    pub variant: String,
    pub app_id: String,
    pub workspace_id: String,
    pub address_kind: String,
    pub address: String,
    /// The audit actor (`agent:<ws>`, `owner`, `user:<name>`).
    pub actor: String,
    /// The caller, so a resumed step acts as the same principal.
    #[serde(skip)]
    pub who_json: String,
    #[serde(skip)]
    pub pin_json: String,
    pub options: serde_json::Value,
    pub step: String,
    pub state: String,
    pub wait_reason: Option<String>,
    /// The open question a wait names (`Q13`), until Rosson answers it.
    pub wait_question: Option<String>,
    pub wait_requests: Vec<String>,
    pub wait_hint: Option<String>,
    pub error_code: Option<String>,
    pub error_hint: Option<String>,
    pub seed_hash: Option<String>,
    pub deployment_id: Option<String>,
    pub space_id: Option<String>,
    pub steps: Vec<StepDone>,
    pub step_started_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

impl JobRow {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), STATE_RUNNING | STATE_WAITING)
    }
}

const COLS: &str = "id, entry, version, variant, app_id, workspace_id, address_kind, address, actor, who_json, pin_json, \
                    options_json, step, state, wait_reason, wait_question, wait_requests, wait_hint, error_code, error_hint, \
                    seed_hash, deployment_id, space_id, steps_json, step_started_at, created_at, updated_at, finished_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    let json_err = |i: usize, e: serde_json::Error| rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e));
    let options: String = r.get(11)?;
    let waits: String = r.get(16)?;
    let steps: String = r.get(23)?;
    Ok(JobRow {
        id: r.get(0)?,
        entry: r.get(1)?,
        version: r.get(2)?,
        variant: r.get(3)?,
        app_id: r.get(4)?,
        workspace_id: r.get(5)?,
        address_kind: r.get(6)?,
        address: r.get(7)?,
        actor: r.get(8)?,
        who_json: r.get(9)?,
        pin_json: r.get(10)?,
        options: serde_json::from_str(&options).map_err(|e| json_err(11, e))?,
        step: r.get(12)?,
        state: r.get(13)?,
        wait_reason: r.get(14)?,
        wait_question: r.get(15)?,
        wait_requests: serde_json::from_str(&waits).map_err(|e| json_err(16, e))?,
        wait_hint: r.get(17)?,
        error_code: r.get(18)?,
        error_hint: r.get(19)?,
        seed_hash: r.get(20)?,
        deployment_id: r.get(21)?,
        space_id: r.get(22)?,
        steps: serde_json::from_str(&steps).map_err(|e| json_err(23, e))?,
        step_started_at: r.get(24)?,
        created_at: r.get(25)?,
        updated_at: r.get(26)?,
        finished_at: r.get(27)?,
    })
}

pub fn insert(conn: &Connection, j: &JobRow) -> rusqlite::Result<()> {
    conn.execute(
        &format!("INSERT INTO suite_install_jobs ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)"),
        params![
            j.id,
            j.entry,
            j.version,
            j.variant,
            j.app_id,
            j.workspace_id,
            j.address_kind,
            j.address,
            j.actor,
            j.who_json,
            j.pin_json,
            j.options.to_string(),
            j.step,
            j.state,
            j.wait_reason,
            j.wait_question,
            serde_json::to_string(&j.wait_requests).expect("strings serialize"),
            j.wait_hint,
            j.error_code,
            j.error_hint,
            j.seed_hash,
            j.deployment_id,
            j.space_id,
            serde_json::to_string(&j.steps).expect("steps serialize"),
            j.step_started_at,
            j.created_at,
            j.updated_at,
            j.finished_at,
        ],
    )?;
    Ok(())
}

/// Write back everything a step may change.
pub fn save(conn: &Connection, j: &JobRow) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE suite_install_jobs SET step = ?2, state = ?3, wait_reason = ?4, wait_question = ?5, wait_requests = ?6, \
         wait_hint = ?7, error_code = ?8, error_hint = ?9, seed_hash = ?10, deployment_id = ?11, space_id = ?12, \
         steps_json = ?13, step_started_at = ?14, updated_at = ?15, finished_at = ?16, options_json = ?17 WHERE id = ?1",
        params![
            j.id,
            j.step,
            j.state,
            j.wait_reason,
            j.wait_question,
            serde_json::to_string(&j.wait_requests).expect("strings serialize"),
            j.wait_hint,
            j.error_code,
            j.error_hint,
            j.seed_hash,
            j.deployment_id,
            j.space_id,
            serde_json::to_string(&j.steps).expect("steps serialize"),
            j.step_started_at,
            j.updated_at,
            j.finished_at,
            j.options.to_string(),
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<JobRow>> {
    conn.query_row(&format!("SELECT {COLS} FROM suite_install_jobs WHERE id = ?1"), params![id], row).optional()
}

/// Every job, newest first (`app` narrows to one App's).
pub fn list(conn: &Connection, app: Option<&str>) -> rusqlite::Result<Vec<JobRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM suite_install_jobs WHERE (?1 IS NULL OR app_id = ?1) ORDER BY created_at DESC, id DESC LIMIT 200"
    ))?;
    let rows = st.query_map(params![app], row)?;
    rows.collect()
}

/// Jobs still running or waiting, oldest first (the driver's order).
pub fn active(conn: &Connection) -> rusqlite::Result<Vec<JobRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM suite_install_jobs WHERE state IN ('running', 'waiting') ORDER BY created_at ASC, id ASC"
    ))?;
    let rows = st.query_map([], row)?;
    rows.collect()
}

// ── lineage ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lineage {
    pub app_id: String,
    pub entry: String,
    pub variant: String,
    pub version: String,
    pub seed_hash: String,
    /// R36: follow new versions. Recorded only: no update loop reads it in
    /// 0.46.9's first slice (Q7 recommends on).
    pub follow: bool,
    pub forked: bool,
    pub bad_versions: Vec<String>,
    pub job_id: String,
    pub installed_by: String,
    pub installed_at: i64,
}

pub fn lineage_insert(conn: &Connection, l: &Lineage) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO suite_apps (app_id, entry, variant, version, seed_hash, follow, forked, bad_versions, job_id, installed_by, installed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(app_id) DO UPDATE SET entry = excluded.entry, variant = excluded.variant, version = excluded.version, \
         seed_hash = excluded.seed_hash, job_id = excluded.job_id",
        params![
            l.app_id,
            l.entry,
            l.variant,
            l.version,
            l.seed_hash,
            l.follow as i64,
            l.forked as i64,
            serde_json::to_string(&l.bad_versions).expect("strings serialize"),
            l.job_id,
            l.installed_by,
            l.installed_at,
        ],
    )?;
    Ok(())
}

fn lineage_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Lineage> {
    let bad: String = r.get(7)?;
    Ok(Lineage {
        app_id: r.get(0)?,
        entry: r.get(1)?,
        variant: r.get(2)?,
        version: r.get(3)?,
        seed_hash: r.get(4)?,
        follow: r.get::<_, i64>(5)? != 0,
        forked: r.get::<_, i64>(6)? != 0,
        bad_versions: serde_json::from_str(&bad)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?,
        job_id: r.get(8)?,
        installed_by: r.get(9)?,
        installed_at: r.get(10)?,
    })
}

const LCOLS: &str = "app_id, entry, variant, version, seed_hash, follow, forked, bad_versions, job_id, installed_by, installed_at";

pub fn lineage_get(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<Lineage>> {
    conn.query_row(&format!("SELECT {LCOLS} FROM suite_apps WHERE app_id = ?1"), params![app_id], lineage_row).optional()
}

pub fn lineage_of_entry(conn: &Connection, entry: &str) -> rusqlite::Result<Vec<Lineage>> {
    let mut st = conn.prepare(&format!("SELECT {LCOLS} FROM suite_apps WHERE entry = ?1 ORDER BY installed_at"))?;
    let rows = st.query_map(params![entry], lineage_row)?;
    rows.collect()
}

pub fn lineage_delete(conn: &Connection, app_id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM suite_apps WHERE app_id = ?1", params![app_id])? > 0)
}

// ── suite-label records (A7) ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteLabel {
    pub label: String,
    pub app_id: String,
    pub front_port: i64,
    pub bound_at: i64,
    pub bound_by: String,
}

fn label_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SuiteLabel> {
    Ok(SuiteLabel { label: r.get(0)?, app_id: r.get(1)?, front_port: r.get(2)?, bound_at: r.get(3)?, bound_by: r.get(4)? })
}

/// Record `label` for `app_id`. `Err` names the App already holding it
/// (one App per label).
pub fn label_bind(conn: &Connection, l: &SuiteLabel) -> Result<(), String> {
    if let Some(h) = label_get(conn, &l.label).map_err(|e| e.to_string())? {
        if h.app_id != l.app_id {
            return Err(h.app_id);
        }
    }
    conn.execute(
        "INSERT INTO suite_labels (label, app_id, front_port, bound_at, bound_by) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(label) DO UPDATE SET front_port = excluded.front_port",
        params![l.label, l.app_id, l.front_port, l.bound_at, l.bound_by],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn label_get(conn: &Connection, label: &str) -> rusqlite::Result<Option<SuiteLabel>> {
    conn.query_row("SELECT label, app_id, front_port, bound_at, bound_by FROM suite_labels WHERE label = ?1", params![label], label_row)
        .optional()
}

/// An App's suite labels, in the order they were bound.
pub fn labels_of(conn: &Connection, app_id: &str) -> rusqlite::Result<Vec<SuiteLabel>> {
    let mut st = conn
        .prepare("SELECT label, app_id, front_port, bound_at, bound_by FROM suite_labels WHERE app_id = ?1 ORDER BY bound_at, label")?;
    let rows = st.query_map(params![app_id], label_row)?;
    rows.collect()
}

pub fn label_release(conn: &Connection, label: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM suite_labels WHERE label = ?1", params![label])? > 0)
}
