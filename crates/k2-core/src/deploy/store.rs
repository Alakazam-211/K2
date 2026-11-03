//! `seeds`, `apps` and `deployments` rows (migration 0165, §9.1). Every
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

/// Seeds still referenced: any App's live or preview Seed, the last
/// `keep_deployments` Seeds that went live and stayed live on an App
/// (0.46.8, [`went_live_seeds`]: a restart or re-host of the same Seed uses
/// no place, nor does a deploy that failed or crashed in its watch window,
/// so a crash loop or a run of bad deploys can't push a rollback target out), a Seed of an
/// unfinished deployment (`pending` / `starting`), and any Seed a
/// **pending** held request names (0.46.3, prd-app-resources-disk-guard-v1
/// §9.1: a held deploy or install carries `seed`, a held `manage` replay
/// `body.seed`; any hex string of 8+ characters in a pending request's body
/// keeps every Seed it is a prefix of, so a new request shape can't lose
/// its Seed). The held-request table must exist: a database without it
/// is an error, never "nothing pending".
pub fn referenced_seeds(conn: &Connection, keep_deployments: usize) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut out = referenced_by_apps(conn, keep_deployments)?;
    let mut st = conn.prepare("SELECT from_seed, to_seed FROM deployments WHERE result IN ('pending', 'starting')")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?;
    for r in rows {
        let (a, b) = r?;
        out.extend(a);
        out.extend(b);
    }
    let mut tokens = std::collections::BTreeSet::new();
    let mut st = conn.prepare("SELECT body_json FROM app_widenings WHERE status = 'pending'")?;
    for body in st.query_map([], |r| r.get::<_, String>(0))? {
        let body = body?;
        match serde_json::from_str::<J>(&body) {
            Ok(v) => hex_tokens(&v, &mut tokens),
            // Unreadable: keep anything its text could name.
            Err(_) => {
                for w in body.split(|c: char| !c.is_ascii_hexdigit()) {
                    if w.len() >= 8 {
                        tokens.insert(w.to_ascii_lowercase());
                    }
                }
            }
        }
    }
    if !tokens.is_empty() {
        let mut st = conn.prepare("SELECT hash FROM seeds")?;
        for h in st.query_map([], |r| r.get::<_, String>(0))? {
            let h = h?;
            if tokens.iter().any(|t| h.starts_with(t.as_str())) {
                out.insert(h);
            }
        }
    }
    Ok(out)
}

/// Every hex string of 8–64 characters anywhere in `v` (lowercased).
fn hex_tokens(v: &J, out: &mut std::collections::BTreeSet<String>) {
    match v {
        J::String(s) => {
            let t = s.trim();
            if (8..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.insert(t.to_ascii_lowercase());
            }
        }
        J::Array(a) => a.iter().for_each(|x| hex_tokens(x, out)),
        J::Object(m) => m.values().for_each(|x| hex_tokens(x, out)),
        _ => {}
    }
}

/// Live, preview and the last `keep_deployments` Seeds that went live on
/// each App. The newest of these is what a rollback with no `--to` goes to
/// ([`went_live_seeds`] is its pick too), so that target is always kept.
fn referenced_by_apps(conn: &Connection, keep_deployments: usize) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    let mut st = conn.prepare("SELECT live_seed, preview_seed FROM apps")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?;
    for r in rows {
        let (a, b) = r?;
        out.extend(a);
        out.extend(b);
    }
    let mut st = conn.prepare("SELECT DISTINCT app_id FROM deployments")?;
    let app_ids: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for app_id in app_ids {
        out.extend(went_live_seeds(conn, &app_id, keep_deployments)?);
    }
    Ok(out)
}

/// The `error` of a deployment someone chose to leave (a rollback through
/// the warm instance, by a person or an agent): it ran fine until then. A
/// crash in the watch window has `<why> inside the watch window; …` here.
/// Both have the result `rolled_back`; this text is what tells them apart,
/// and it is what rows written by earlier releases already hold.
pub const ROLLED_BACK_BY_CHOICE: &str = "rolled back";

/// Did this deployment go live and hold? A rollback someone chose doesn't
/// undo that; a crash in the watch window does, whether K2 switched back
/// (`rolled_back`) or had nothing warm to switch to (`exited` by the end of
/// the window: a deploy into a stopped App, extra ports, a warm instance
/// that already died).
pub fn stayed_live(d: &DeploymentRow) -> bool {
    let crashed = match d.result.as_str() {
        RESULT_ROLLED_BACK => d.error.as_deref() != Some(ROLLED_BACK_BY_CHOICE),
        RESULT_EXITED => matches!((d.finished_at, d.watch_until), (Some(end), Some(until)) if end <= until),
        _ => false,
    };
    d.live_at.is_some() && !crashed
}

/// [`stayed_live`] as SQL, over `deployments`' own columns.
fn stayed_live_sql() -> String {
    format!(
        "live_at IS NOT NULL AND (result != '{RESULT_ROLLED_BACK}' OR error = '{ROLLED_BACK_BY_CHOICE}') \
         AND (result != '{RESULT_EXITED}' OR finished_at IS NULL OR watch_until IS NULL OR finished_at > watch_until)"
    )
}

/// The Seeds that went live on an App and stayed live, newest first, each
/// once. A restart, re-host or rollback moves a Seed to the front. A deploy
/// that never went live isn't here, nor is one that crashed in its watch
/// window: those take no place and are no rollback target. At most `limit`.
pub fn went_live_seeds(conn: &Connection, app_id: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(&format!(
        "SELECT to_seed FROM deployments WHERE app_id = ?1 AND to_seed IS NOT NULL AND {} \
         GROUP BY to_seed ORDER BY MAX(started_at) DESC, MAX(rowid) DESC LIMIT ?2",
        stayed_live_sql()
    ))?;
    let v = st.query_map(params![app_id, limit as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Would a rollback from `live` to a Seed at `target_revision` cross an
/// applied data migration with no way down (§7.3)? Then it is refused.
pub fn crosses_migration(live: &SeedRow, target_revision: i64) -> bool {
    live.data_revision > target_revision && !live.manifest.get("dataDown").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// A Seed that went live on an App, is still stored and isn't the live one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackTarget {
    pub seed: String,
    pub started_at: i64,
    /// One of the App's last `keep_deployments`: stays until newer deploys
    /// push it out. Otherwise the sweep frees it when its grace ends.
    pub kept: bool,
    /// A rollback to it is refused (`rollback_crosses_migration`).
    pub crosses_migration: bool,
}

impl RollbackTarget {
    /// Safe to name to someone as a place to go back to.
    pub fn usable(&self) -> bool {
        self.kept && !self.crosses_migration
    }
}

/// An App's [`RollbackTarget`]s, newest first.
pub fn rollback_targets(conn: &Connection, app_id: &str, live_seed: Option<&str>, keep_deployments: usize) -> rusqlite::Result<Vec<RollbackTarget>> {
    let kept: std::collections::BTreeSet<String> = went_live_seeds(conn, app_id, keep_deployments)?.into_iter().collect();
    let live = match live_seed {
        Some(h) => get_seed(conn, h)?,
        None => None,
    };
    let mut st = conn.prepare(&format!(
        "SELECT d.to_seed, MAX(d.started_at), s.data_revision FROM deployments d JOIN seeds s ON s.hash = d.to_seed \
         WHERE d.app_id = ?1 AND {} \
         GROUP BY d.to_seed ORDER BY MAX(d.started_at) DESC, MAX(d.rowid) DESC",
        stayed_live_sql()
    ))?;
    let rows = st.query_map(params![app_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (seed, started_at, revision) = row?;
        if live_seed != Some(seed.as_str()) {
            let crosses = live.as_ref().is_some_and(|l| crosses_migration(l, revision));
            out.push(RollbackTarget { kept: kept.contains(&seed), crosses_migration: crosses, seed, started_at });
        }
    }
    Ok(out)
}

/// The newest deployment of this App that went live on `token` (a full hash
/// or a prefix of 8+ hex characters) whose Seed is no longer stored: how a
/// rollback tells a Seed that was freed from one that never ran here (a
/// Seed that only ever failed never ran).
pub fn freed_deployment_of_seed(conn: &Connection, app_id: &str, token: &str) -> rusqlite::Result<Option<DeploymentRow>> {
    let t = token.trim().to_ascii_lowercase();
    if t.len() < 8 || t.len() > 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    conn.query_row(
        &format!(
            "SELECT {DEP_COLS} FROM deployments WHERE app_id = ?1 AND to_seed LIKE ?2 AND live_at IS NOT NULL AND to_seed NOT IN (SELECT hash FROM seeds) \
             ORDER BY started_at DESC, rowid DESC LIMIT 1"
        ),
        params![app_id, format!("{t}%")],
        dep_row,
    )
    .optional()
}

/// Every stored Seed's hash.
pub fn seed_hashes(conn: &Connection) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut st = conn.prepare("SELECT hash FROM seeds")?;
    let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// What the sweep keeps a Seed for (prd-app-resources-disk-guard-v1 §9.1;
/// 0.46.8, Rosson: was 30 days and 10): a Seed goes once it is past the
/// last 5 that went live on its App and 3 days past its last use, the later
/// of its `last_used_at` and when it last stopped serving ([`replaced_at`]).
pub const SWEEP_MAX_AGE_SECS: i64 = 3 * 86_400;
pub const SWEEP_KEEP_DEPLOYMENTS: usize = 5;

/// When each Seed last stopped serving: the moment a later deployment went
/// live in its place. `last_used_at` is stamped when a Seed is built or
/// starts, so a Seed live for a month has a month-old stamp on the day it is
/// replaced. Read from the rows, not stamped, so it holds for Seeds replaced
/// before this release too.
fn replaced_at(conn: &Connection) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
    let mut st = conn.prepare("SELECT from_seed, MAX(live_at) FROM deployments WHERE live_at IS NOT NULL AND from_seed IS NOT NULL GROUP BY from_seed")?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// The Seeds the sweep would remove now: not referenced
/// ([`referenced_seeds`]) and past `max_age_secs` since their last use.
/// Reads only (the caller holds the DB lock for this pick, never for the
/// deletes).
pub fn sweep_candidates(conn: &Connection, now: i64, max_age_secs: i64, keep_deployments: usize) -> rusqlite::Result<Vec<SeedRow>> {
    let keep = referenced_seeds(conn, keep_deployments)?;
    let replaced = replaced_at(conn)?;
    Ok(list_seeds(conn)?
        .into_iter()
        .filter(|s| !keep.contains(&s.hash) && now - s.last_used_at.max(replaced.get(&s.hash).copied().unwrap_or(i64::MIN)) >= max_age_secs)
        .collect())
}

/// What [`detach_seed`] did with one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detach {
    /// Out of the store and its row gone. The caller removes this tree.
    Moved(std::path::PathBuf),
    /// No longer a candidate (a deploy started using it since the pick).
    Kept,
    /// Still a candidate, but its tree couldn't be moved: nothing changed.
    Failed(String),
}

/// Take one candidate out of the store, fast enough to run under the DB
/// lock: check again that it is still a candidate (a deploy may have
/// started using it since the pick), move its tree aside (one rename, out
/// of the store's name: a build of the same tree stores a fresh copy), and
/// delete its row. The caller removes a [`Detach::Moved`] tree with
/// [`super::seed::remove_detached`] **after** releasing the lock.
pub fn detach_seed(conn: &Connection, hash: &str, now: i64, max_age_secs: i64, keep_deployments: usize) -> rusqlite::Result<Detach> {
    let Some(row) = get_seed(conn, hash)? else { return Ok(Detach::Kept) };
    let replaced: Option<i64> =
        conn.query_row("SELECT MAX(live_at) FROM deployments WHERE live_at IS NOT NULL AND from_seed = ?1", params![hash], |r| r.get(0))?;
    if now - row.last_used_at.max(replaced.unwrap_or(i64::MIN)) < max_age_secs || referenced_seeds(conn, keep_deployments)?.contains(hash) {
        return Ok(Detach::Kept);
    }
    let aside = match super::seed::detach_tree(hash) {
        Ok(p) => p,
        Err(e) => return Ok(Detach::Failed(e.message)),
    };
    conn.execute("DELETE FROM seeds WHERE hash = ?1", params![hash])?;
    Ok(Detach::Moved(aside))
}

/// The sweep in one call, for tests: pick, detach, remove. Returns the
/// hashes removed. The daemon's sweep does the same steps but holds the
/// shared DB lock only for the pick and each detach, never while deleting a
/// tree (`apps::disk_watch`).
#[cfg(test)]
pub(crate) fn sweep_seeds(conn: &Connection, now: i64, max_age_secs: i64, keep_deployments: usize) -> rusqlite::Result<Vec<String>> {
    let mut removed = Vec::new();
    for s in sweep_candidates(conn, now, max_age_secs, keep_deployments)? {
        match detach_seed(conn, &s.hash, now, max_age_secs, keep_deployments)? {
            Detach::Moved(aside) => {
                super::seed::remove_detached(&aside).unwrap();
                removed.push(s.hash);
            }
            Detach::Kept => {}
            Detach::Failed(e) => panic!("detach {}: {e}", s.hash),
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
/// Parked (0.46.3, prd-agent-app-permissions-v1 §6, Q8): stopped and held
/// by the server's owner, with no maintaining workspace
/// (`owner_workspace = ''`). It doesn't serve and doesn't restart; the
/// owner hands it to a workspace (`k2 app unpark <app> --to <ws>`) or
/// uninstalls it. A workspace removal's Apps (`--apps park`) and today's
/// orphans (the boot sweep) land here.
pub const STATE_PARKED: &str = "parked";
/// prd-app-runtime-update-continuity-v1 A4/R6: the owner wants it running
/// but K2's own engine (or helper) can't start it now. Never a crash: no
/// backoff step, no breaker; retried every 60 s and at once when the engine
/// changes (R43). No CHECK on `apps.state`, so no migration.
pub const STATE_WAITING_ENGINE: &str = "waiting_engine";

/// The Apps a workspace maintains (its `owner_workspace`), by id.
pub fn owned_app_ids(conn: &Connection, workspace: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT app_id FROM apps WHERE owner_workspace = ?1 ORDER BY app_id")?;
    let v = st.query_map(params![workspace], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// The last guard under every workspace removal (Q8): a workspace that
/// still maintains Apps is never removed under them (the App would keep
/// serving with no agent). The daemon's routes answer `409
/// workspace_has_apps` with the three ways out before they get here.
pub fn workspace_has_apps_error(conn: &Connection, workspace: &str) -> Result<(), String> {
    let apps = owned_app_ids(conn, workspace).map_err(|e| format!("checking the workspace's Apps: {e}"))?;
    if apps.is_empty() {
        return Ok(());
    }
    Err(format!(
        "workspace_has_apps: this workspace maintains {} ({}). Hand them over, uninstall or park them first: \
         k2 app release <workspace> --apps handover:<other-workspace>|uninstall|park",
        if apps.len() == 1 { "an App" } else { "Apps" },
        apps.join(", ")
    ))
}

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
    /// Per-App disk ceilings (0168): JSON, NULL = K2's default ceiling
    /// (`k2_root_proto::apps::IoMax::from_column`).
    pub io_max_json: Option<String>,
    /// The `process` exception's device grants (0168): a JSON array of
    /// tokens (`dri-render128`, …).
    pub devices_json: String,
    /// Who approved the `process` exception, when and how (0168).
    pub exception_json: Option<String>,
    /// 0221 (update continuity R14): `auto` | `handover` | `window` |
    /// `manual` | `off`.
    pub rehost_policy: String,
    /// 0221: `HH:MM-HH:MM` box time, for policy `window`.
    pub rehost_window: Option<String>,
    /// 0221 (Rosson Q4 + Q16): `pause` (default) | `defer`.
    pub rehost_when_no_overlap: String,
    /// 0221 (R20, S3): the warm instance kept for rollback and its deadline.
    pub warm_deployment: Option<String>,
    pub warm_until: Option<i64>,
}

const APP_COLS: &str = "app_id, service_id, seed_id, owner_workspace, path, source, folder_path, live_seed, live_tree_hash, \
                        live_deployment, preview_seed, isolation, memory_limit_bytes, front_port, state, state_reason, \
                        created_by, created_at, updated_at, io_max_json, devices_json, exception_json, rehost_policy, \
                        rehost_window, rehost_when_no_overlap, warm_deployment, warm_until";

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
        io_max_json: r.get(19)?,
        devices_json: r.get(20)?,
        exception_json: r.get(21)?,
        rehost_policy: r.get(22)?,
        rehost_window: r.get(23)?,
        rehost_when_no_overlap: r.get(24)?,
        warm_deployment: r.get(25)?,
        warm_until: r.get(26)?,
    })
}

impl AppRow {
    /// The device tokens granted to the `process` exception.
    pub fn devices(&self) -> Vec<String> {
        serde_json::from_str(&self.devices_json).unwrap_or_default()
    }

    /// The open path's `process` exception (§4.3): owner-approved, walled
    /// by the unit sandbox. Not stage 1's preview process engine (that one
    /// is `path = guarded`, `isolation = process`).
    pub fn is_process_exception(&self) -> bool {
        self.path == "open" && self.isolation == "process"
    }
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

/// Per-App disk ceilings (JSON, `None` = K2's default ceiling).
pub fn set_io_max(conn: &Connection, app_id: &str, io_max_json: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET io_max_json = ?2, updated_at = ?3 WHERE app_id = ?1",
        params![app_id, io_max_json, super::now()],
    )?;
    Ok(())
}

/// The `process` exception on (`Some(approval)`: `path = open`,
/// `isolation = process`, the device grants) or off (`None`: back to the
/// guarded container, no devices). One statement, so a reader never sees
/// half of it.
pub fn set_process_exception(
    conn: &Connection,
    app_id: &str,
    approval: Option<(&str, &[String])>,
) -> rusqlite::Result<()> {
    match approval {
        Some((exception_json, devices)) => conn.execute(
            "UPDATE apps SET path = 'open', isolation = 'process', devices_json = ?2, exception_json = ?3, updated_at = ?4 WHERE app_id = ?1",
            params![app_id, serde_json::to_string(devices).unwrap_or_else(|_| "[]".into()), exception_json, super::now()],
        )?,
        None => conn.execute(
            "UPDATE apps SET path = 'guarded', isolation = 'container', devices_json = '[]', exception_json = NULL, updated_at = ?2 WHERE app_id = ?1",
            params![app_id, super::now()],
        )?,
    };
    Ok(())
}

/// `container` (the guarded engine) or `process` (stage 1). The column's
/// CHECK holds the rest out.
pub fn set_isolation(conn: &Connection, app_id: &str, isolation: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET isolation = ?2, updated_at = ?3 WHERE app_id = ?1",
        params![app_id, isolation, super::now()],
    )?;
    Ok(())
}

pub fn delete_app(conn: &Connection, app_id: &str) -> rusqlite::Result<bool> {
    // It served until now, and the rows that would say so are about to go.
    conn.execute("UPDATE seeds SET last_used_at = ?2 WHERE hash = (SELECT live_seed FROM apps WHERE app_id = ?1)", params![app_id, super::now()])?;
    conn.execute("DELETE FROM deployments WHERE app_id = ?1", params![app_id])?;
    Ok(conn.execute("DELETE FROM apps WHERE app_id = ?1", params![app_id])? > 0)
}

// ── deployments ──────────────────────────────────────────────────────

pub const KIND_DEPLOY: &str = "deploy";
pub const KIND_ROLLBACK: &str = "rollback";
pub const KIND_AUTO_ROLLBACK: &str = "auto_rollback";
pub const KIND_RESTART: &str = "restart";
pub const KIND_START: &str = "start";
/// 0.46.4 (update continuity D3): the same Seed on a fresh instance after
/// a K2 update (no CHECK on `kind`).
pub const KIND_REHOST: &str = "rehost";
/// An App handover's re-key (prd-app-handoff-v1 §4.10): the live Seed on a
/// fresh instance so it picks up new keys, a new DSN and a pass in the new
/// workspace's room. K2's own move, as quiet as a re-host.
pub const KIND_HANDOVER: &str = "handover";

/// A re-start of the live Seed K2 runs on its own (a re-host, a handover's
/// re-key): no agent wake on failure, its owner module tells.
pub fn is_k2_move(kind: &str) -> bool {
    kind == KIND_REHOST || kind == KIND_HANDOVER
}

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
    /// The structured failure code (0221, A4): `error` keeps the
    /// `<code>: <message>` text for older readers.
    pub error_code: Option<String>,
}

const DEP_COLS: &str = "id, app_id, kind, from_seed, to_seed, tree_hash, commit_ref, actor, reason, result, error, port, pid, \
                        started_at, live_at, finished_at, watch_until, error_code";

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
        error_code: r.get(17)?,
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

/// A deployment failed with `code` (stored structurally, A4) and `message`
/// (`error` = `<code>: <message>`, as before 0221).
pub fn set_failed(conn: &Connection, id: &str, code: &str, message: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE deployments SET result = ?2, error = ?3, error_code = ?4, finished_at = ?5 WHERE id = ?1",
        params![id, RESULT_FAILED, format!("{code}: {message}"), code, super::now()],
    )?;
    Ok(())
}

/// One App's deployments that ended badly (`failed`, `exited`) and finished
/// at or after `since` (unix seconds), newest first: the desired-state boot
/// rule (R35) reads their codes and times.
pub fn bad_endings_since(conn: &Connection, app_id: &str, since: i64) -> rusqlite::Result<Vec<DeploymentRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {DEP_COLS} FROM deployments WHERE app_id = ?1 AND result IN ('failed','exited') \
         AND COALESCE(finished_at, started_at) >= ?2 ORDER BY started_at DESC, rowid DESC"
    ))?;
    let v = st.query_map(params![app_id, since], dep_row)?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// R14 / Q4 + Q16: an App's re-host settings (`None` keeps a field).
pub fn set_rehost_settings(
    conn: &Connection,
    app_id: &str,
    policy: Option<&str>,
    window: Option<Option<&str>>,
    when_no_overlap: Option<&str>,
) -> rusqlite::Result<()> {
    if let Some(p) = policy {
        conn.execute("UPDATE apps SET rehost_policy = ?2, updated_at = ?3 WHERE app_id = ?1", params![app_id, p, super::now()])?;
    }
    if let Some(w) = window {
        conn.execute("UPDATE apps SET rehost_window = ?2, updated_at = ?3 WHERE app_id = ?1", params![app_id, w, super::now()])?;
    }
    if let Some(n) = when_no_overlap {
        conn.execute("UPDATE apps SET rehost_when_no_overlap = ?2, updated_at = ?3 WHERE app_id = ?1", params![app_id, n, super::now()])?;
    }
    Ok(())
}

/// R10: the runtime fingerprint of a deployment that went live.
pub fn set_runtime_json(conn: &Connection, id: &str, runtime: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE deployments SET runtime_json = ?2 WHERE id = ?1", params![id, runtime])?;
    Ok(())
}

/// R10: a deployment's fingerprint (`None`: none written, = due).
pub fn runtime_json(conn: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT runtime_json FROM deployments WHERE id = ?1", params![id], |r| r.get(0)).optional().map(Option::flatten)
}

/// R32: the readiness answer probed once after the gate.
pub fn set_ready_json(conn: &Connection, id: &str, ready: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE deployments SET ready_json = ?2 WHERE id = ?1", params![id, ready])?;
    Ok(())
}

/// The App's newest deployment (R43: did its last attempt fail K2-side?).
pub fn last_deployment(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<DeploymentRow>> {
    Ok(list_deployments(conn, app_id, 1)?.into_iter().next())
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
        c.execute_batch(include_str!("../../drizzle_sql/0165_app_runtime_core.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0170_app_walls_lite.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        // 0221: `deployments.error_code` (read by every deployment row).
        c.execute_batch(include_str!("../../drizzle_sql/0221_app_rehost.sql")).unwrap();
        // The held-request table the Seed keep set reads (0168's
        // `app_widenings`; the rest of 0168 needs tables this fixture lacks).
        for stmt in include_str!("../../drizzle_sql/0168_app_grace.sql").split("--> statement-breakpoint") {
            if stmt.contains("CREATE TABLE IF NOT EXISTS app_widenings") || stmt.contains("ON app_widenings") {
                c.execute_batch(stmt).unwrap();
            }
        }
        c
    }

    /// A held request (0168's CHECK predates 0183's door kinds; the keep
    /// set reads only the body and status, so `config` stands in).
    fn held(c: &Connection, id: &str, body: &str, status: &str) {
        c.execute(
            "INSERT INTO app_widenings (id, kind, app_id, app_label, workspace_id, body_json, diff_json, summary, reasons_json, requested_by, requested_at, expires_at, status) \
             VALUES (?1, 'config', 'jim', 'jim', 'ws', ?2, '{}', 's', '[]', 'agent:ws', 1, 9999999999, ?3)",
            params![id, body, status],
        )
        .unwrap();
    }

    fn store_tree(h: &crate::test_env::TempHome, c: &Connection, name: &str, body: &str, at: i64) -> String {
        let f = h.path().join(name);
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("s.js"), body).unwrap();
        let st = super::super::seed::store_folder(&f, &std::collections::BTreeSet::new()).unwrap();
        let mut row = seed('0', at);
        row.hash = st.hash.clone();
        row.size_bytes = st.size_bytes as i64;
        upsert_seed(c, &row).unwrap();
        c.execute("UPDATE seeds SET last_used_at = ?2 WHERE hash = ?1", params![st.hash, at]).unwrap();
        st.hash
    }

    /// 0.46.3 (prd-app-resources-disk-guard-v1 §9.1, G7): the sweep keeps
    /// live, preview, the last Seeds that went live, unfinished deployments and
    /// every Seed a pending held request names (`seed`, a `manage`
    /// replay's `body.seed`, an 8+ hex prefix); a decided request keeps
    /// nothing. It removes the rest, both tree and row.
    #[test]
    fn the_sweep_keeps_what_a_pending_request_or_unfinished_deployment_needs() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let now = old + SWEEP_MAX_AGE_SECS + 1;
        let live = store_tree(&h, &c, "live", "1", old);
        let held_deploy = store_tree(&h, &c, "hd", "2", old);
        let held_manage = store_tree(&h, &c, "hm", "3", old);
        let held_prefix = store_tree(&h, &c, "hp", "4", old);
        let unfinished = store_tree(&h, &c, "uf", "5", old);
        let denied = store_tree(&h, &c, "dn", "6", old);
        let gone = store_tree(&h, &c, "gone", "7", old);
        let fresh = store_tree(&h, &c, "fresh", "8", now - 60);
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
        set_live(&c, "jim", Some(&live), None).unwrap();
        // `unfinished` never went live; it is kept only because its own
        // deployment is pending.
        let dep = insert_deployment(
            &c,
            &NewDeployment { app_id: "jim", kind: KIND_DEPLOY, from_seed: None, to_seed: Some(&unfinished), commit_ref: None, actor: "owner", reason: None },
        )
        .unwrap();
        c.execute("UPDATE deployments SET started_at = 1 WHERE id = ?1", params![dep]).unwrap();
        for i in 0..11 {
            let d = insert_deployment(
                &c,
                &NewDeployment { app_id: "jim", kind: KIND_DEPLOY, from_seed: None, to_seed: Some(&live), commit_ref: None, actor: "owner", reason: None },
            )
            .unwrap();
            c.execute("UPDATE deployments SET started_at = ?2, result = 'live' WHERE id = ?1", params![d, 100 + i]).unwrap();
        }
        held(&c, "r1", &format!(r#"{{"app":"jim","seed":"{held_deploy}"}}"#), "pending");
        held(&c, "r2", &format!(r#"{{"route":"/cli/app/deploy","body":{{"app":"jim","seed":"{held_manage}"}}}}"#), "pending");
        held(&c, "r3", &format!(r#"{{"route":"/cli/app/deploy","body":{{"seed":"{}"}}}}"#, &held_prefix[..10]), "pending");
        held(&c, "r4", &format!(r#"{{"seed":"{denied}"}}"#), "denied");
        let picks: Vec<String> = sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().map(|s| s.hash).collect();
        let mut want = vec![denied.clone(), gone.clone()];
        want.sort();
        let mut got = picks.clone();
        got.sort();
        assert_eq!(got, want, "only the decided request's Seed and the unreferenced one");
        // A deploy starts using `gone` between the pick and the detach: kept.
        touch_seed(&c, &gone, now).unwrap();
        assert_eq!(detach_seed(&c, &gone, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Kept);
        assert!(super::super::seed::seed_path(&gone).is_dir());
        let removed = sweep_seeds(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(removed, vec![denied.clone()]);
        assert!(!super::super::seed::seed_path(&denied).exists());
        assert!(get_seed(&c, &denied).unwrap().is_none());
        for kept in [&live, &held_deploy, &held_manage, &held_prefix, &unfinished, &gone, &fresh] {
            assert!(super::super::seed::seed_path(kept).is_dir(), "{kept}");
            assert!(get_seed(&c, kept).unwrap().is_some(), "{kept}");
        }
        let leftovers: Vec<_> = std::fs::read_dir(super::super::seeds_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(super::super::seed::DETACHED_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "the detached tree was removed");
        // No held-request table: an error, never "nothing pending".
        c.execute_batch("DROP TABLE app_widenings").unwrap();
        assert!(sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).is_err());
    }

    fn app(c: &Connection, id: &str, manifest_id: &str) {
        insert_app(
            c,
            &NewApp {
                app_id: id,
                service_id: &format!("svc-{id}"),
                seed_id: Some(manifest_id),
                owner_workspace: "ws",
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 40000,
                created_by: "owner",
            },
        )
        .unwrap();
    }

    /// One deployment row with its own time. Live, superseded and
    /// rolled-back rows went live; a failed one never did. A rolled-back row
    /// here is a crash in the watch window, with `auto_rollback`'s text.
    fn dep(c: &Connection, app_id: &str, kind: &str, seed: &str, at: i64, result: &str) -> String {
        dep_from(c, app_id, kind, None, seed, at, result)
    }

    /// [`dep`] replacing `from`, as a real deployment row does.
    fn dep_from(c: &Connection, app_id: &str, kind: &str, from: Option<&str>, seed: &str, at: i64, result: &str) -> String {
        let d = insert_deployment(c, &NewDeployment { app_id, kind, from_seed: from, to_seed: Some(seed), commit_ref: None, actor: "owner", reason: None }).unwrap();
        let live_at = [RESULT_LIVE, RESULT_SUPERSEDED, RESULT_ROLLED_BACK].contains(&result).then_some(at);
        c.execute("UPDATE deployments SET started_at = ?2, live_at = ?3, result = ?4 WHERE id = ?1", params![d, at, live_at, result]).unwrap();
        if result == RESULT_ROLLED_BACK {
            set_result(c, &d, RESULT_ROLLED_BACK, Some("exited inside the watch window; rolled back to abcdef012345")).unwrap();
        }
        d
    }

    /// 0.46.8 (Rosson: "retain the most recent 5 seeds"): the box that
    /// reached 255 Seeds. Previews and soak deploys of one manifest id on
    /// ONE App row come down to its last 5, tree and row; the sweep never
    /// looks at the manifest id. `preview_seed` keeps only the Seed it
    /// names now, not every Seed that ever was a preview.
    #[test]
    fn a_box_with_255_seeds_on_one_manifest_id_comes_down_to_the_last_5() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let now = old + 10_000 + SWEEP_MAX_AGE_SECS;
        app(&c, "site", "site");
        let seeds: Vec<String> = (0..255).map(|i| store_tree(&h, &c, &format!("s{i}"), &format!("build {i}"), old + i)).collect();
        assert!(seeds.iter().all(|s| get_seed(&c, s).unwrap().unwrap().seed_id == "jim"), "one manifest id across every Seed");
        for (i, s) in seeds.iter().enumerate() {
            let from = i.checked_sub(1).map(|p| seeds[p].as_str());
            dep_from(&c, "site", KIND_DEPLOY, from, s, old + i as i64, RESULT_SUPERSEDED);
        }
        let live = seeds[254].clone();
        set_live(&c, "site", Some(&live), None).unwrap();
        let preview = seeds[100].clone();
        c.execute("UPDATE apps SET preview_seed = ?1 WHERE app_id = 'site'", params![preview]).unwrap();

        let t = std::time::Instant::now();
        let picks = sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        eprintln!("[sweep] the pick, under the lock in the daemon: {:?}", t.elapsed());
        assert_eq!(picks.len(), 255 - 5 - 1, "all but the last 5 and the preview");
        // The daemon's loop: lock, detach; unlock, delete the tree.
        let mut longest = std::time::Duration::ZERO;
        let mut removed = Vec::new();
        for s in &picks {
            let t = std::time::Instant::now();
            let Detach::Moved(aside) = detach_seed(&c, &s.hash, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap() else {
                panic!("{} is still a candidate", s.hash)
            };
            longest = longest.max(t.elapsed());
            super::super::seed::remove_detached(&aside).unwrap();
            removed.push(s.hash.clone());
        }
        eprintln!("[sweep] longest detach, under the lock in the daemon: {longest:?}");
        assert_eq!(removed.len(), 249);

        let mut want: std::collections::BTreeSet<String> = seeds[250..].iter().cloned().collect();
        want.insert(preview.clone());
        assert_eq!(seed_hashes(&c).unwrap(), want, "the rows");
        for s in &seeds {
            assert_eq!(super::super::seed::seed_path(s).is_dir(), want.contains(s), "the tree of {s}");
        }
        let left: Vec<String> = std::fs::read_dir(super::super::seeds_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !want.contains(n))
            .collect();
        assert!(left.is_empty(), "nothing else stays in the store: {left:?}");
        // Rollback reaches the last 4, plus the preview while it is stored
        // (a place to go, not one K2 promises to keep).
        let targets = rollback_targets(&c, "site", Some(&live), SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(targets.iter().map(|t| t.seed.clone()).collect::<Vec<_>>(), vec![seeds[253].clone(), seeds[252].clone(), seeds[251].clone(), seeds[250].clone(), preview.clone()]);
        assert_eq!(targets.iter().map(|t| t.kept).collect::<Vec<_>>(), vec![true, true, true, true, false]);
        // Clearing the preview lets it go at the next sweep.
        c.execute("UPDATE apps SET preview_seed = NULL WHERE app_id = 'site'", []).unwrap();
        assert_eq!(sweep_seeds(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), vec![preview]);
    }

    /// Previews on separate App rows with one manifest id: each App row
    /// counts for itself, so its Seeds stay until the App is uninstalled;
    /// then they go like any other unused Seed.
    #[test]
    fn app_rows_sharing_a_manifest_id_each_keep_their_own_and_uninstall_frees_them() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let now = super::super::now();
        let mut lives = Vec::new();
        for a in 0..8 {
            let id = format!("preview-{a}");
            app(&c, &id, "site");
            let s = store_tree(&h, &c, &id, &format!("p{a}"), old);
            dep(&c, &id, KIND_DEPLOY, &s, old, RESULT_LIVE);
            set_live(&c, &id, Some(&s), None).unwrap();
            lives.push(s);
        }
        assert!(sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty(), "every App row keeps its own");
        for a in 0..6 {
            delete_app(&c, &format!("preview-{a}")).unwrap();
        }
        assert!(sweep_candidates(&c, now + 60, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty(), "it served until the uninstall: the grace runs from there");
        let later = now + 60 + SWEEP_MAX_AGE_SECS;
        let removed: std::collections::BTreeSet<String> = sweep_seeds(&c, later, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().collect();
        assert_eq!(removed, lives[..6].iter().cloned().collect(), "an uninstalled App's Seeds go");
    }

    /// Restarts and re-hosts of the live Seed use no place, and neither do
    /// failed deploys: crash restarts or a run of bad deploys never lose the
    /// Seed `k2 app rollback` would go to. A failed Seed itself goes once
    /// unused for the grace.
    #[test]
    fn restarts_and_failures_never_push_the_rollback_target_out() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let now = old + 1_000 + SWEEP_MAX_AGE_SECS;
        app(&c, "jim", "jim");
        let good: Vec<String> = (0..7).map(|i| store_tree(&h, &c, &format!("g{i}"), &format!("good {i}"), old)).collect();
        for (i, s) in good.iter().enumerate() {
            dep(&c, "jim", KIND_DEPLOY, s, old + i as i64, RESULT_SUPERSEDED);
        }
        set_live(&c, "jim", Some(&good[6]), None).unwrap();
        for i in 0..60 {
            dep(&c, "jim", if i % 2 == 0 { KIND_RESTART } else { KIND_REHOST }, &good[6], old + 100 + i, RESULT_SUPERSEDED);
        }
        let bad: Vec<String> = (0..6).map(|i| store_tree(&h, &c, &format!("b{i}"), &format!("bad {i}"), old)).collect();
        for (i, s) in bad.iter().enumerate() {
            dep(&c, "jim", KIND_DEPLOY, s, old + 200 + i as i64, RESULT_FAILED);
        }
        let removed: std::collections::BTreeSet<String> = sweep_seeds(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().collect();
        let mut want: std::collections::BTreeSet<String> = good[..2].iter().cloned().collect();
        want.extend(bad.iter().cloned());
        assert_eq!(removed, want, "the two oldest good Seeds and the failed ones");
        assert_eq!(went_live_seeds(&c, "jim", 2).unwrap(), vec![good[6].clone(), good[5].clone()], "the rollback target, past 66 rows");
        assert!(super::super::seed::seed_path(&good[5]).is_dir());
    }

    /// 0.46.8: the grace runs from when a Seed last served. One live since
    /// a month ago and replaced by five quick deploys today has a month-old
    /// stamp, but it served until today: often the last known good version.
    /// Read from the rows, so it holds for what was replaced before this
    /// release. A Seed replaced long ago still goes.
    #[test]
    fn a_seed_that_served_until_today_gets_its_three_days() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let today = old + 30 * 86_400;
        app(&c, "jim", "jim");
        let earlier = store_tree(&h, &c, "earlier", "e", old);
        let long = store_tree(&h, &c, "long", "l", old);
        dep_from(&c, "jim", KIND_DEPLOY, None, &earlier, old, RESULT_SUPERSEDED);
        dep_from(&c, "jim", KIND_DEPLOY, Some(&earlier), &long, old + 1, RESULT_SUPERSEDED);
        let quick: Vec<String> = (0..5).map(|i| store_tree(&h, &c, &format!("q{i}"), &format!("quick {i}"), today - 300 + i)).collect();
        let mut from = long.clone();
        for (i, s) in quick.iter().enumerate() {
            dep_from(&c, "jim", KIND_DEPLOY, Some(&from), s, today - 300 + i as i64, RESULT_SUPERSEDED);
            from = s.clone();
        }
        set_live(&c, "jim", Some(&quick[4]), None).unwrap();
        assert_eq!(get_seed(&c, &long).unwrap().unwrap().last_used_at, old, "the old stamp this test is about");
        assert!(!referenced_seeds(&c, SWEEP_KEEP_DEPLOYMENTS).unwrap().contains(&long), "past the last 5");

        let picks: Vec<String> = sweep_candidates(&c, today, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().map(|s| s.hash).collect();
        assert_eq!(picks, vec![earlier.clone()], "replaced a month ago: goes. Replaced today: stays");
        assert_eq!(detach_seed(&c, &long, today, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Kept);
        assert!(super::super::seed::seed_path(&long).is_dir());
        // A second before its 3 days from today's replacement, then after.
        let served_until = today - 300;
        assert_eq!(detach_seed(&c, &long, served_until + SWEEP_MAX_AGE_SECS - 1, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Kept);
        let at = served_until + SWEEP_MAX_AGE_SECS;
        assert!(sweep_candidates(&c, at, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().iter().any(|s| s.hash == long));
        assert!(matches!(detach_seed(&c, &long, at, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Moved(_)));
    }

    /// The review's proof, inverted: a Seed that served for 60 days and
    /// stopped an hour ago isn't picked, and the rollback hint doesn't name
    /// it (it is stored, but the sweep will take it when its grace ends).
    /// Everything `usable` names is in the sweep's keep set.
    #[test]
    fn a_long_serving_seed_isnt_freed_an_hour_after_it_stopped_serving() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let day = 86_400;
        let t0 = 1_000_000;
        let now = t0 + 60 * day;
        app(&c, "jim", "jim");
        let long = store_tree(&h, &c, "long", "long serving", t0);
        dep(&c, "jim", KIND_DEPLOY, &long, t0, RESULT_SUPERSEDED);
        let mut ds = Vec::new();
        let mut from = long.clone();
        for i in 0..5i64 {
            let at = now - 3600 + i;
            let s = store_tree(&h, &c, &format!("d{i}"), &format!("new {i}"), at);
            dep_from(&c, "jim", KIND_DEPLOY, Some(&from), &s, at, RESULT_SUPERSEDED);
            from = s.clone();
            ds.push(s);
        }
        set_live(&c, "jim", Some(&ds[4]), None).unwrap();
        assert!(sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty(), "it served until an hour ago");
        assert!(sweep_seeds(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty());
        assert!(super::super::seed::seed_path(&long).is_dir());
        let targets = rollback_targets(&c, "jim", Some(&ds[4]), SWEEP_KEEP_DEPLOYMENTS).unwrap();
        let last = targets.last().unwrap();
        assert_eq!((last.seed.as_str(), last.kept, last.usable()), (long.as_str(), false, false), "listed as stored, never named as a place to go");
        let keep = referenced_seeds(&c, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        let usable: Vec<&RollbackTarget> = targets.iter().filter(|t| t.usable()).collect();
        assert_eq!(usable.len(), 4, "{targets:?}");
        assert!(usable.iter().all(|t| keep.contains(&t.seed)), "the hint names only what the sweep won't take: {targets:?}");
        // Its 3 days from when it stopped, not from its old stamp.
        let picks = sweep_candidates(&c, now - 3600 + 3 * day, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(picks.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(), vec![long.as_str()]);
    }

    /// The review's second proof, inverted, through the same store calls the
    /// supervisor makes: crashed deploys don't take the places of good ones.
    #[test]
    fn auto_rolled_back_seeds_dont_take_the_places_of_good_ones() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        app(&c, "jim", "jim");
        let mk = |n: &str, at: i64| store_tree(&h, &c, n, n, at);
        let live_row = |kind: &str, from: Option<&str>, to: &str| {
            let d = insert_deployment(&c, &NewDeployment { app_id: "jim", kind, from_seed: from, to_seed: Some(to), commit_ref: None, actor: "owner", reason: None }).unwrap();
            set_went_live(&c, &d, 0).unwrap();
            d
        };
        let good: Vec<String> = (0..4).map(|i| mk(&format!("good{i}"), 1_000)).collect();
        for s in &good {
            let d = live_row(KIND_DEPLOY, None, s);
            set_result(&c, &d, RESULT_SUPERSEDED, None).unwrap();
        }
        let p = mk("stable", 1_000);
        live_row(KIND_DEPLOY, None, &p);
        set_live(&c, "jim", Some(&p), None).unwrap();
        let mut bad = Vec::new();
        for i in 0..4 {
            let b = mk(&format!("bad{i}"), 1_000);
            let d = live_row(KIND_DEPLOY, Some(&p), &b);
            set_result(&c, &d, RESULT_ROLLED_BACK, Some("crashed inside the watch window; rolled back to stable")).unwrap();
            live_row(KIND_AUTO_ROLLBACK, Some(&b), &p);
            bad.push(b);
        }
        let top = went_live_seeds(&c, "jim", SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(top, vec![p.clone(), good[3].clone(), good[2].clone(), good[1].clone(), good[0].clone()]);
        let now = crate::deploy::now();
        assert!(sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty(), "the good ones are kept and the crashed ones just stopped");
        let later = now + 5 + SWEEP_MAX_AGE_SECS;
        let picks: std::collections::BTreeSet<String> = sweep_candidates(&c, later, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().map(|s| s.hash).collect();
        assert_eq!(picks, bad.iter().cloned().collect(), "after the grace only the crashed ones go");
    }

    /// A rollback someone chose isn't a crash: the deploy they left ran fine,
    /// so it keeps its place and stays a target. The row's text tells them
    /// apart, with the same `rolled_back` result on both.
    #[test]
    fn a_deploy_someone_chose_to_leave_stays_a_good_rollback_point() {
        let c = conn();
        app(&c, "jim", "jim");
        for ch in ['0', '1', '2', '3', '4', '5', '6'] {
            upsert_seed(&c, &seed(ch, 1)).unwrap();
        }
        let hash = |ch: char| seed(ch, 1).hash;
        for (i, ch) in ['0', '1', '2', '3', '4', '5'].into_iter().enumerate() {
            dep(&c, "jim", KIND_DEPLOY, &hash(ch), 10 + i as i64, RESULT_SUPERSEDED);
        }
        // Left by choice: '6' ran, and someone went back from it to '5'.
        let left = dep(&c, "jim", KIND_DEPLOY, &hash('6'), 20, RESULT_LIVE);
        set_result(&c, &left, RESULT_ROLLED_BACK, Some(ROLLED_BACK_BY_CHOICE)).unwrap();
        // `retire` sets the same result again later, with no text.
        set_result(&c, &left, RESULT_ROLLED_BACK, None).unwrap();
        dep(&c, "jim", KIND_ROLLBACK, &hash('5'), 21, RESULT_LIVE);
        set_live(&c, "jim", Some(&hash('5')), None).unwrap();
        assert!(stayed_live(&get_deployment(&c, &left).unwrap().unwrap()));
        assert_eq!(went_live_seeds(&c, "jim", 2).unwrap(), vec![hash('5'), hash('6')], "a plain rollback goes back to the one left");
        assert_eq!(went_live_seeds(&c, "jim", 5).unwrap().len(), 5);
        assert!(went_live_seeds(&c, "jim", 5).unwrap().contains(&hash('6')), "it keeps its place");
        // Same shape, but a crash: no place, no target.
        c.execute("UPDATE deployments SET error = 'exited inside the watch window; rolled back to x' WHERE id = ?1", params![left]).unwrap();
        assert!(!stayed_live(&get_deployment(&c, &left).unwrap().unwrap()));
        assert!(!went_live_seeds(&c, "jim", 5).unwrap().contains(&hash('6')));
        assert!(rollback_targets(&c, "jim", Some(&hash('5')), SWEEP_KEEP_DEPLOYMENTS).unwrap().iter().all(|t| t.seed != hash('6')));
    }

    /// A crash in the watch window with nothing warm ends `exited`, not
    /// `rolled_back`, and takes no place either; one after the window counts.
    /// The Rust and the SQL agree on every shape, NULLs included.
    #[test]
    fn exited_inside_the_watch_window_takes_no_place_after_it_does() {
        let c = conn();
        app(&c, "jim", "jim");
        let hash = |ch: char| seed(ch, 1).hash;
        for ch in ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'] {
            upsert_seed(&c, &seed(ch, 1)).unwrap();
        }
        // (Seed, finished_at, watch_until, stayed live)
        let shapes = [
            ('1', Some(99), Some(100), false),
            ('2', Some(100), Some(100), false),
            ('3', Some(101), Some(100), true),
            ('4', Some(100), None, true),
            ('5', None, Some(100), true),
        ];
        let mut rows = Vec::new();
        for (i, (ch, end, until, _)) in shapes.iter().enumerate() {
            let d = dep(&c, "jim", KIND_DEPLOY, &hash(*ch), 10 + i as i64, RESULT_LIVE);
            c.execute("UPDATE deployments SET result = ?2, finished_at = ?3, watch_until = ?4 WHERE id = ?1", params![d, RESULT_EXITED, end, until]).unwrap();
            rows.push(d);
        }
        // The real calls, as the crash path makes them: inside the window.
        let d = dep(&c, "jim", KIND_DEPLOY, &hash('6'), 20, RESULT_LIVE);
        set_went_live(&c, &d, super::super::now() + 300).unwrap();
        set_result(&c, &d, RESULT_EXITED, Some("exited 1")).unwrap();
        rows.push(d);
        dep(&c, "jim", KIND_DEPLOY, &hash('0'), 30, RESULT_LIVE);
        set_live(&c, "jim", Some(&hash('0')), None).unwrap();

        let all = went_live_seeds(&c, "jim", 50).unwrap();
        let want = [('1', false), ('2', false), ('3', true), ('4', true), ('5', true), ('6', false)];
        for (d, (ch, stayed)) in rows.iter().zip(want) {
            assert_eq!(stayed_live(&get_deployment(&c, d).unwrap().unwrap()), stayed, "Rust, Seed {ch}");
            assert_eq!(all.contains(&hash(ch)), stayed, "SQL, Seed {ch}");
        }
        for (ch, _, _, stayed) in shapes {
            assert_eq!(rollback_targets(&c, "jim", Some(&hash('0')), SWEEP_KEEP_DEPLOYMENTS).unwrap().iter().any(|t| t.seed == hash(ch)), stayed, "target, Seed {ch}");
        }
        // The crashes don't push out what stayed: the 5 are 0, 5, 4, 3 and
        // the newest before them.
        dep(&c, "jim", KIND_DEPLOY, &hash('8'), 5, RESULT_SUPERSEDED);
        dep(&c, "jim", KIND_DEPLOY, &hash('9'), 4, RESULT_SUPERSEDED);
        assert_eq!(went_live_seeds(&c, "jim", 5).unwrap(), vec![hash('0'), hash('5'), hash('4'), hash('3'), hash('8')]);
    }

    /// 0.46.8: a deploy that went live and crashed in its watch window takes
    /// no place, so four crashing deploys don't push out older good Seeds,
    /// and it is no rollback target: a plain rollback goes to the last good
    /// one, not the one that crashed. Its Seed gets the grace from when it
    /// stopped. A Seed that ran well once and crashed later still counts for
    /// the run that went well.
    #[test]
    fn deploys_rolled_back_take_no_place_and_are_no_target() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let crash_at = old + 30 * 86_400;
        app(&c, "jim", "jim");
        let good: Vec<String> = (0..6).map(|i| store_tree(&h, &c, &format!("g{i}"), &format!("good {i}"), old)).collect();
        let mut from: Option<String> = None;
        for (i, s) in good.iter().enumerate() {
            dep_from(&c, "jim", KIND_DEPLOY, from.as_deref(), s, old + i as i64, RESULT_SUPERSEDED);
            from = Some(s.clone());
        }
        let live = good[5].clone();
        set_live(&c, "jim", Some(&live), None).unwrap();
        let bad: Vec<String> = (0..4).map(|i| store_tree(&h, &c, &format!("b{i}"), &format!("bad {i}"), old)).collect();
        for (i, s) in bad.iter().enumerate() {
            let at = crash_at + i as i64 * 10;
            dep_from(&c, "jim", KIND_DEPLOY, Some(&live), s, at, RESULT_ROLLED_BACK);
            dep_from(&c, "jim", KIND_AUTO_ROLLBACK, Some(s), &live, at + 1, RESULT_LIVE);
        }
        // good[2] is deployed again and crashes: its earlier run still counts.
        dep_from(&c, "jim", KIND_DEPLOY, Some(&live), &good[2], crash_at + 100, RESULT_ROLLED_BACK);
        dep_from(&c, "jim", KIND_AUTO_ROLLBACK, Some(&good[2]), &live, crash_at + 101, RESULT_LIVE);

        assert_eq!(went_live_seeds(&c, "jim", 5).unwrap(), vec![good[5].clone(), good[4].clone(), good[3].clone(), good[2].clone(), good[1].clone()]);
        assert_eq!(went_live_seeds(&c, "jim", 2).unwrap(), vec![live.clone(), good[4].clone()], "a plain rollback: the last good one");
        let targets = rollback_targets(&c, "jim", Some(&live), SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert!(targets.iter().all(|t| !bad.contains(&t.seed)), "a crashed deploy is no target: {targets:?}");
        // The crashed Seeds served until their rollback: not freed at once.
        let picks: Vec<String> = sweep_candidates(&c, crash_at + 200, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().map(|s| s.hash).collect();
        assert_eq!(picks, vec![good[0].clone()], "only the oldest good Seed, replaced long ago");
        let later = crash_at + 200 + SWEEP_MAX_AGE_SECS;
        let removed: std::collections::BTreeSet<String> = sweep_seeds(&c, later, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().into_iter().collect();
        let mut want: std::collections::BTreeSet<String> = bad.iter().cloned().collect();
        want.insert(good[0].clone());
        assert_eq!(removed, want, "after the grace the crashed ones go; the good ones stay");
    }

    /// What a person is told they can go back to: `usable` names only Seeds
    /// K2 is keeping and a rollback would accept. One past the last 5 is
    /// stored but next to be freed; one across a migration is refused.
    #[test]
    fn rollback_targets_say_what_is_kept_and_what_crosses_a_migration() {
        let c = conn();
        app(&c, "jim", "jim");
        for (i, ch) in ['0', '1', '2', '3', '4', '5', '6'].into_iter().enumerate() {
            let mut s = seed(ch, 1);
            s.data_revision = if ch == '3' { 1 } else { 2 };
            upsert_seed(&c, &s).unwrap();
            dep(&c, "jim", KIND_DEPLOY, &s.hash, 10 + i as i64, RESULT_SUPERSEDED);
        }
        let live = seed('6', 1).hash;
        set_live(&c, "jim", Some(&live), None).unwrap();
        let t = rollback_targets(&c, "jim", Some(&live), SWEEP_KEEP_DEPLOYMENTS).unwrap();
        let row = |ch: char| t.iter().find(|x| x.seed == seed(ch, 1).hash).unwrap();
        assert_eq!(t.len(), 6, "{t:?}");
        assert!(row('5').usable() && row('4').usable() && row('2').usable());
        assert!(row('3').kept && row('3').crosses_migration && !row('3').usable(), "kept, but a rollback is refused");
        assert!(!row('1').kept && !row('1').usable(), "stored, next to be freed");
        // A live Seed that declares a way down crosses nothing.
        c.execute("UPDATE seeds SET manifest_json = '{\"dataDown\":true}' WHERE hash = ?1", params![live]).unwrap();
        let t = rollback_targets(&c, "jim", Some(&live), SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert!(t.iter().all(|x| !x.crosses_migration), "{t:?}");
    }

    /// A tree that can't be moved is a failure the caller can report, never
    /// "kept": the row stays, and a failed delete is an error too.
    #[test]
    fn a_seed_that_cant_be_moved_is_reported_as_failed() {
        let _h = crate::test_env::TempHome::new();
        let c = conn();
        let mut bad = seed('0', 1);
        bad.hash = "not-a-seed-hash".into();
        upsert_seed(&c, &bad).unwrap();
        let now = 1 + SWEEP_MAX_AGE_SECS;
        assert!(matches!(detach_seed(&c, &bad.hash, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Failed(_)));
        assert!(get_seed(&c, &bad.hash).unwrap().is_some(), "nothing changed");
        let outside = std::env::temp_dir().join(".tmp-sweep-outside");
        assert!(super::super::seed::remove_detached(&outside).is_err(), "only inside the store");
        let gone = super::super::seeds_dir().join(format!("{}never-there", super::super::seed::DETACHED_PREFIX));
        assert_eq!(super::super::seed::remove_detached(&gone), Ok(()), "already gone is fine");
    }

    /// A Seed being deployed is never deleted, however old its last use:
    /// not picked, and refused again at the detach, which is what a deploy
    /// that started after the pick meets.
    #[test]
    fn a_seed_being_deployed_is_never_deleted() {
        let h = crate::test_env::TempHome::new();
        let c = conn();
        let old = 1_000;
        let now = old + SWEEP_MAX_AGE_SECS + 1;
        app(&c, "jim", "jim");
        let s = store_tree(&h, &c, "s", "1", old);
        let from = store_tree(&h, &c, "from", "2", old);
        let picks = sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(picks.len(), 2, "both are candidates before the deploy");
        for result in [RESULT_PENDING, RESULT_STARTING] {
            let d = insert_deployment(&c, &NewDeployment { app_id: "jim", kind: KIND_DEPLOY, from_seed: Some(&from), to_seed: Some(&s), commit_ref: None, actor: "owner", reason: None }).unwrap();
            c.execute("UPDATE deployments SET result = ?2 WHERE id = ?1", params![d, result]).unwrap();
            assert_eq!(detach_seed(&c, &s, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Kept, "{result}");
            assert_eq!(detach_seed(&c, &from, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap(), Detach::Kept, "{result}: the Seed it comes from");
            assert!(sweep_candidates(&c, now, SWEEP_MAX_AGE_SECS, SWEEP_KEEP_DEPLOYMENTS).unwrap().is_empty(), "{result}");
            assert!(super::super::seed::seed_path(&s).is_dir() && get_seed(&c, &s).unwrap().is_some(), "{result}");
            c.execute("UPDATE deployments SET result = 'failed' WHERE id = ?1", params![d]).unwrap();
        }
    }

    /// What a rollback reads: a Seed that was freed is told from one that
    /// never ran here, by full hash or prefix; junk tokens match nothing.
    #[test]
    fn a_freed_seed_is_found_by_its_deployment() {
        let c = conn();
        app(&c, "jim", "jim");
        app(&c, "kim", "kim");
        let gone: String = std::iter::repeat_n('a', 64).collect();
        let d = dep(&c, "jim", KIND_DEPLOY, &gone, 5, RESULT_SUPERSEDED);
        assert_eq!(freed_deployment_of_seed(&c, "jim", &gone).unwrap().map(|r| r.id), Some(d.clone()));
        assert_eq!(freed_deployment_of_seed(&c, "jim", "AAAAAAAA").unwrap().map(|r| r.id), Some(d));
        assert_eq!(freed_deployment_of_seed(&c, "kim", &gone).unwrap(), None, "another App's");
        assert_eq!(freed_deployment_of_seed(&c, "jim", "aaaa").unwrap(), None, "too short");
        assert_eq!(freed_deployment_of_seed(&c, "jim", "%%%%%%%%").unwrap(), None, "not hex");
        let only_failed: String = std::iter::repeat_n('c', 64).collect();
        dep(&c, "jim", KIND_DEPLOY, &only_failed, 7, RESULT_FAILED);
        assert_eq!(freed_deployment_of_seed(&c, "jim", &only_failed).unwrap(), None, "it never ran here");
        let crashed: String = std::iter::repeat_n('d', 64).collect();
        let d = dep(&c, "jim", KIND_DEPLOY, &crashed, 8, RESULT_ROLLED_BACK);
        assert_eq!(freed_deployment_of_seed(&c, "jim", &crashed).unwrap().map(|r| r.id), Some(d), "it ran, then crashed");
        upsert_seed(&c, &seed('b', 1)).unwrap();
        dep(&c, "jim", KIND_DEPLOY, &seed('b', 1).hash, 6, RESULT_SUPERSEDED);
        assert_eq!(freed_deployment_of_seed(&c, "jim", &seed('b', 1).hash).unwrap(), None, "still stored: not freed");
        let targets = rollback_targets(&c, "jim", None, SWEEP_KEEP_DEPLOYMENTS).unwrap();
        assert_eq!(targets.iter().map(|t| t.seed.as_str()).collect::<Vec<_>>(), vec![seed('b', 1).hash.as_str()], "only what is stored");
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

    /// 0221 (A4): a failure's code is stored beside its text; the R35
    /// window reads failed and exited rows only, newest first.
    #[test]
    fn failures_keep_their_code_and_the_window_reads_bad_endings() {
        let c = conn();
        upsert_seed(&c, &seed('a', 1)).unwrap();
        insert_app(
            &c,
            &NewApp {
                app_id: "kim",
                service_id: "svc-2",
                seed_id: Some("kim"),
                owner_workspace: "ws",
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 40001,
                created_by: "owner",
            },
        )
        .unwrap();
        let new = |kind| {
            insert_deployment(
                &c,
                &NewDeployment { app_id: "kim", kind, from_seed: None, to_seed: Some(&seed('a', 1).hash), commit_ref: None, actor: "system", reason: None },
            )
            .unwrap()
        };
        let a = new(KIND_RESTART);
        set_failed(&c, &a, "engine_protocol_mismatch", "protocol 2 vs 3").unwrap();
        let b = new(KIND_RESTART);
        set_result(&c, &b, RESULT_EXITED, Some("exited")).unwrap();
        let live = new(KIND_RESTART);
        set_went_live(&c, &live, 0).unwrap();
        let row = get_deployment(&c, &a).unwrap().unwrap();
        assert_eq!(
            (row.result.as_str(), row.error.as_deref(), row.error_code.as_deref()),
            (RESULT_FAILED, Some("engine_protocol_mismatch: protocol 2 vs 3"), Some("engine_protocol_mismatch"))
        );
        assert!(row.finished_at.is_some());
        let bad: Vec<String> = bad_endings_since(&c, "kim", 0).unwrap().into_iter().map(|d| d.id).collect();
        assert_eq!(bad.len(), 2, "failed and exited, never live: {bad:?}");
        assert!(bad.contains(&a) && bad.contains(&b));
        assert!(bad_endings_since(&c, "kim", super::super::now() + 10).unwrap().is_empty(), "outside the window");
        assert_eq!(last_deployment(&c, "kim").unwrap().map(|d| d.id), Some(live));
    }
}
